#[cfg(windows)]
use super::process_witness::{ProcessWitness, writer};
use super::*;

fn script(script: &str) -> Command {
    #[cfg(windows)]
    let mut command = Command::new("powershell.exe");
    #[cfg(not(windows))]
    let mut command = Command::new("sh");
    #[cfg(windows)]
    command.args(["-NoProfile", "-NonInteractive", "-Command", script]);
    #[cfg(not(windows))]
    command.args(["-c", script]);
    command
}

#[test]
fn slow_commands_are_bounded() {
    #[cfg(windows)]
    let command = script("Start-Sleep -Seconds 30");
    #[cfg(not(windows))]
    let command = script("sleep 30");
    let started = Instant::now();
    let error = run_bounded(command, Duration::from_millis(250), 1024, "slow test").unwrap_err();
    assert!(error.to_string().contains("timed out"));
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[test]
fn both_output_streams_are_drained_and_oversized_output_is_rejected() {
    #[cfg(windows)]
    let command =
        script("[Console]::Out.Write(('o' * 100000)); [Console]::Error.Write(('e' * 100000))");
    #[cfg(not(windows))]
    let command =
        script("head -c 100000 /dev/zero | tr '\\0' o; head -c 100000 /dev/zero | tr '\\0' e >&2");
    let output = run_bounded(command, Duration::from_secs(5), 200000, "output test").unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout.len(), 100000);
    assert_eq!(output.stderr.len(), 100000);
    #[cfg(windows)]
    let command = script("[Console]::Out.Write(('x' * 100000))");
    #[cfg(not(windows))]
    let command = script("head -c 100000 /dev/zero");
    let error =
        run_bounded(command, Duration::from_secs(5), 1024, "large output test").unwrap_err();
    assert!(error.to_string().contains("too large"));
}

#[cfg(windows)]
#[test]
fn timing_out_git_stops_its_hook_process_tree() {
    let repository = TestRepository::new();
    repository.write("file.txt", "contents\n");
    stage_all(&repository.0).unwrap();
    fs::write(
        repository.0.join("test-hooks/pre-commit"),
        format!(
            "#!/bin/sh\n{} &\nwait\n",
            writer("timeout", "hook-completed.txt")
        ),
    )
    .unwrap();
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(&repository.0)
        .args(["commit", "-m", "Blocked by hook"]);
    let (ready, wait_until_ready) = mpsc::channel();
    let worker = thread::spawn(move || {
        run_bounded_after_ready(
            command,
            Duration::from_millis(300),
            OUTPUT_LIMIT,
            "commit hook test",
            wait_until_ready,
        )
    });
    let descendant = ProcessWitness::open(&repository.0, "timeout");
    ready.send(()).unwrap();
    let error = worker.join().unwrap().unwrap_err();
    assert!(error.to_string().contains("timed out"));
    descendant.assert_stopped();
    assert!(
        !repository.0.join("hook-completed.txt").exists(),
        "the timed-out hook must not continue running"
    );
    assert!(recent_log(&repository.0, 1).unwrap().is_empty());
}

#[test]
fn a_commit_hook_may_run_longer_than_the_query_limit() {
    let repository = TestRepository::new();
    repository.write("file.txt", "contents\n");
    stage_all(&repository.0).unwrap();
    fs::write(
        repository.0.join("test-hooks/pre-commit"),
        "#!/bin/sh\nsleep 1\nprintf 'checked' > hook-completed.txt\n",
    )
    .unwrap();
    set_test_timeouts(Duration::from_millis(300), Duration::from_secs(60));
    let committed = commit(&repository.0, "Checked by a slow hook");
    let slow_query = run_bounded_with_input(
        script(if cfg!(windows) {
            "Start-Sleep -Seconds 5"
        } else {
            "sleep 5"
        }),
        timeout(Access::Query),
        OUTPUT_LIMIT,
        "slow query",
        None,
        None,
    );
    set_test_timeouts(QUERY_TIMEOUT, MUTATION_TIMEOUT);
    committed.unwrap();
    assert!(repository.0.join("hook-completed.txt").is_file());
    assert_eq!(
        recent_log(&repository.0, 1).unwrap()[0].subject,
        "Checked by a slow hook"
    );
    assert!(
        slow_query
            .unwrap_err()
            .to_string()
            .contains("timed out after 0 seconds")
    );
}

#[cfg(windows)]
#[test]
fn cancelling_a_commit_stops_its_hook_and_explains_the_remaining_index_lock() {
    let repository = TestRepository::new();
    repository.write("file.txt", "contents\n");
    stage_all(&repository.0).unwrap();
    fs::write(
        repository.0.join("test-hooks/pre-commit"),
        format!(
            "#!/bin/sh\n: > .git/index.lock\n{} &\nwait\n",
            writer("cancel", "hook-completed.txt")
        ),
    )
    .unwrap();
    let cancellation = Cancellation::default();
    let root = repository.0.clone();
    let worker = {
        let cancellation = cancellation.clone();
        thread::spawn(move || cancellable(&cancellation, || commit(&root, "Interrupted")))
    };
    let descendant = ProcessWitness::open(&repository.0, "cancel");
    let started = Instant::now();
    cancellation.cancel();
    let error = worker.join().unwrap().unwrap_err().to_string();
    assert!(started.elapsed() < Duration::from_secs(3), "{error}");
    assert!(error.contains("cancelled"), "{error}");
    let lock = repository.0.join(".git").join("index.lock");
    // The hook stands in for a Git step that holds the index lock when it
    // is stopped, such as the checkout of a switch or pull. vEdit explains
    // the remaining lock and leaves the decision to remove it to the user.
    assert!(lock.is_file());
    assert!(error.contains("index.lock remains"), "{error}");
    assert!(error.contains("vEdit does not delete it"), "{error}");
    descendant.assert_stopped();
    assert!(
        !repository.0.join("hook-completed.txt").exists(),
        "the cancelled hook must not continue running"
    );
    fs::remove_file(&lock).unwrap();
    assert!(recent_log(&repository.0, 1).unwrap().is_empty());
    // Outside a cancellable scope, the next Git action is not cancelled.
    // Its successful hook has no reason to repeat the interrupted wait.
    fs::write(
        repository.0.join("test-hooks/pre-commit"),
        "#!/bin/sh\nexit 0\n",
    )
    .unwrap();
    commit(&repository.0, "After removing the lock").unwrap();
}

#[test]
fn index_lock_hint_follows_a_linked_worktree_and_defers_to_git_messages() {
    let repository = TestRepository::new();
    repository.write("file.txt", "contents\n");
    repository.commit_all("base");
    let linked = repository.0.join("linked tree");
    repository.git(&["worktree", "add", "-b", "linked", linked.to_str().unwrap()]);
    // Git derives the administrative folder's name from the worktree's.
    let mut administrative = fs::read_dir(repository.0.join(".git").join("worktrees"))
        .unwrap()
        .map(|entry| entry.unwrap().path());
    let expected = administrative.next().unwrap().join("index.lock");
    assert!(administrative.next().is_none());
    assert_eq!(
        fs::canonicalize(index_lock_path(&linked).unwrap().parent().unwrap()).unwrap(),
        fs::canonicalize(expected.parent().unwrap()).unwrap()
    );
    let interrupted = with_index_lock_hint(&linked, GitError::new("switch branch", "cancelled"));
    assert!(!interrupted.to_string().contains("index.lock"));
    fs::write(&expected, "").unwrap();
    let interrupted = with_index_lock_hint(&linked, GitError::new("switch branch", "cancelled"));
    assert!(interrupted.to_string().contains("index.lock remains"));
    // Git names a lock held by another process itself; that message stays.
    let error = stage_all(&linked).unwrap_err().to_string();
    assert_eq!(error.matches("index.lock").count(), 1, "{error}");
    assert!(expected.is_file(), "vEdit must never remove the lock");
    fs::remove_file(&expected).unwrap();
    repository.git(&["worktree", "remove", "--force", linked.to_str().unwrap()]);
}

#[cfg(windows)]
#[test]
fn helpers_that_outlive_git_keep_running_unless_they_hold_its_pipes() {
    let repository = TestRepository::new();
    repository.write("file.txt", "contents\n");
    stage_all(&repository.0).unwrap();
    // A helper with detached pipes survives the successful command. Hold it
    // alive until after commit returned, then let that exact writer finish.
    fs::write(
        repository.0.join("test-hooks/post-commit"),
        format!(
            "#!/bin/sh\n{} >/dev/null 2>&1 </dev/null &\nwhile [ ! -e .git/detached-hook.release ]; do sleep 0.02; done\n",
            writer("detached", "detached-helper.txt")
        ),
    )
    .unwrap();
    let root = repository.0.clone();
    let worker = thread::spawn(move || commit(&root, "With detached helper"));
    let detached = ProcessWitness::open(&repository.0, "detached");
    // Retain the process object before exercising Git's release policy. Even
    // a broken release cannot leave us opening a recycled PID afterwards.
    fs::write(repository.0.join(".git/detached-hook.release"), "continue").unwrap();
    worker.join().unwrap().unwrap();
    detached.assert_running();
    assert!(!repository.0.join("detached-helper.txt").exists());
    detached.release();
    detached.assert_stopped();
    assert!(repository.0.join("detached-helper.txt").exists());

    // The attached helper inherits Git's pipes and must be stopped after the
    // existing two-second grace. Its process handle proves no later write is
    // possible without sleeping until the original six-second write deadline.
    repository.write("file.txt", "changed\n");
    stage_paths(&repository.0, &[PathBuf::from("file.txt")]).unwrap();
    fs::write(
        repository.0.join("test-hooks/post-commit"),
        format!(
            "#!/bin/sh\n{} &\nwhile [ ! -e .git/attached-hook.release ]; do sleep 0.02; done\n",
            writer("attached", "attached-helper.txt")
        ),
    )
    .unwrap();
    let root = repository.0.clone();
    let worker = thread::spawn(move || commit(&root, "With attached helper"));
    let attached = ProcessWitness::open(&repository.0, "attached");
    // Keep Git itself alive until the descendant handle is retained. Its
    // actual exit, and therefore the unmodified grace clock, happens next.
    let started = Instant::now();
    fs::write(repository.0.join(".git/attached-hook.release"), "continue").unwrap();
    worker.join().unwrap().unwrap();
    let elapsed = started.elapsed();
    assert!(
        elapsed >= EXIT_PIPE_GRACE && elapsed < Duration::from_secs(5),
        "{elapsed:?}"
    );
    attached.assert_stopped();
    assert!(!repository.0.join("attached-helper.txt").exists());
}

#[test]
fn user_configuration_cannot_change_parsed_git_output() {
    let repository = TestRepository::new();
    repository.write("Ünïcode plan.txt", "first\n");
    repository.commit_all("base");
    let base = repository.head_hash();
    let tree = String::from_utf8(repository.git(&["rev-parse", "HEAD^{tree}"]).stdout)
        .unwrap()
        .trim()
        .to_owned();
    // A commit with an SSH signature that cannot be verified here. With
    // log.showSignature, Git prints "No signature" before formatted output.
    let object = format!(
        "tree {tree}\nparent {base}\nauthor A <a@example.invalid> 1700000000 +0000\ncommitter A <a@example.invalid> 1700000000 +0000\ngpgsig -----BEGIN SSH SIGNATURE-----\n U1NIU0lH\n -----END SSH SIGNATURE-----\n\nSigned ü\n\nBody\n"
    );
    let signed = raw_git_with_input(
        &repository.0,
        &string_args(&["hash-object", "-t", "commit", "-w", "--stdin"]),
        "test fixture",
        Some(object.into_bytes()),
    )
    .unwrap();
    let signed = String::from_utf8(signed.stdout).unwrap().trim().to_owned();
    repository.git(&["update-ref", "refs/heads/main", &signed]);
    repository.write("Ünïcode plan.txt", "first\nsecond\n");
    repository.write("untracked.txt", "new\n");
    for (key, value) in [
        ("log.showSignature", "true"),
        ("gpg.format", "ssh"),
        ("color.ui", "always"),
        ("color.diff", "always"),
        ("color.status", "always"),
        ("color.branch", "always"),
        ("core.quotePath", "false"),
        ("diff.noprefix", "true"),
        ("diff.mnemonicPrefix", "true"),
        ("diff.srcPrefix", "SRC/"),
        ("diff.dstPrefix", "DST/"),
        ("diff.relative", "true"),
        ("diff.external", "false-diff-tool-that-does-not-exist"),
        ("status.relativePaths", "false"),
        ("i18n.logOutputEncoding", "ISO-8859-1"),
        ("log.decorate", "full"),
        ("format.pretty", "oneline"),
    ] {
        repository.git(&["config", key, value]);
    }

    let details = commit_details(&repository.0, &signed).unwrap();
    assert_eq!(details.hash, signed);
    assert_eq!(details.message.trim_end(), "Signed ü\n\nBody");
    let log = recent_log(&repository.0, 2).unwrap();
    assert_eq!(log[0].hash, signed);
    assert_eq!(log[0].subject, "Signed ü");
    let headlines = commit_headlines(&repository.0, std::slice::from_ref(&signed)).unwrap();
    assert_eq!(headlines[0].subject, "Signed ü");
    let comparison = resolve_review(&repository.0, &ReviewSource::Commit(signed.clone())).unwrap();
    assert_eq!(comparison.old, base);
    assert_eq!(comparison.new.as_deref(), Some(signed.as_str()));
    assert!(
        comparison.title.contains("Signed ü"),
        "{}",
        comparison.title
    );
    let (status, tracking, _) = status_with_head(&repository.0).unwrap();
    assert_eq!(tracking.name.as_deref(), Some("main"));
    let names: Vec<_> = status.files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(names, ["Ünïcode plan.txt", "untracked.txt"]);
    let patch = diff(&repository.0, Path::new("Ünïcode plan.txt"), false).unwrap();
    assert!(
        patch.starts_with("diff --git \"a/\\303\\234n\\303\\257code plan.txt\" \"b/"),
        "{patch}"
    );
    assert!(!patch.contains('\u{1b}'), "{patch}");
    assert!(patch.contains("\n+second\n"), "{patch}");
}

#[test]
fn index_lock_hint_follows_a_main_worktree_without_git_processes() {
    let repository = TestRepository::new();
    assert_eq!(
        index_lock_path(&repository.0),
        Some(repository.0.join(".git").join("index.lock"))
    );
}

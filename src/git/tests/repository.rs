use super::*;

#[test]
fn refs_that_moved_after_the_status_read_read_head_again() {
    let repository = TestRepository::new();
    repository.commit_all("Initial");
    repository.git(&["branch", "feature"]);
    let previous = history_snapshot(&repository.0).unwrap();
    // A commit and a branch switch between the status and the ref listing.
    let (status, _, head) = status_with_head(&repository.0).unwrap();
    repository.write("next.txt", "next\n");
    repository.commit_all("Next");
    take_command_count();
    let (snapshot, refs) = history_snapshot_after_status(
        &repository.0,
        Some(&previous),
        status.branch.as_deref(),
        head.as_ref(),
    )
    .unwrap();
    // The ref listing and the repeated HEAD read.
    assert_eq!(take_command_count(), 2);
    assert_eq!(snapshot, history_snapshot(&repository.0).unwrap());
    assert!(snapshot.refs[0].current);
    assert_eq!(snapshot.refs[0].hash, repository.head_hash());
    assert_eq!(
        main_comparison_after_history(&repository.0, &refs)
            .unwrap()
            .unwrap()
            .ahead,
        0
    );
    let (status, _, head) = status_with_head(&repository.0).unwrap();
    repository.git(&["switch", "feature"]);
    let (snapshot, refs) = history_snapshot_after_status(
        &repository.0,
        Some(&previous),
        status.branch.as_deref(),
        head.as_ref(),
    )
    .unwrap();
    assert_eq!(snapshot, history_snapshot(&repository.0).unwrap());
    assert_eq!(snapshot.refs[0].name, "feature");
    assert_eq!(
        main_comparison_after_history(&repository.0, &refs).unwrap(),
        main_comparison(&repository.0).unwrap()
    );
}

#[test]
fn detects_repository_from_a_nested_directory() {
    let repository = TestRepository::new();
    let nested = repository.0.join("src");
    fs::create_dir(&nested).unwrap();
    let detected = repository_root(&nested).unwrap().unwrap();
    assert_eq!(
        fs::canonicalize(detected).unwrap(),
        fs::canonicalize(&repository.0).unwrap()
    );
    let outside = TestRepository::empty_directory();
    assert_eq!(repository_root(&outside.0).unwrap(), None);
    assert!(recent_log(&outside.0, 6).is_err());
    assert!(repository_root(&outside.0.join("missing-directory")).is_err());
}

#[test]
fn project_repository_requires_the_exact_parent_but_accepts_roots_and_worktrees() {
    let repository = TestRepository::new();
    let nested = repository.0.join("examples");
    fs::create_dir(&nested).unwrap();
    assert!(project_repository(&nested, None).unwrap().root.is_none());
    let selected = project_repository(&nested, Some(&repository.0)).unwrap();
    assert!(selected.root.is_some() && selected.parent.is_some());
    let other = TestRepository::new();
    assert!(
        project_repository(&nested, Some(&other.0))
            .unwrap()
            .root
            .is_none()
    );
    let own = project_repository(&fs::canonicalize(&repository.0).unwrap(), None).unwrap();
    assert!(own.root.is_some() && own.parent.is_none());
    repository.commit_all("Initial");
    let worktree = TestRepository::empty_directory();
    repository.git(&["worktree", "add", "--detach", worktree.0.to_str().unwrap()]);
    assert!(worktree.0.join(".git").is_file());
    let own = project_repository(&worktree.0, None).unwrap();
    assert!(own.root.is_some() && own.parent.is_none());
    initialize(&nested).unwrap();
    let own = project_repository(&nested, Some(&repository.0)).unwrap();
    assert!(own.root.is_some() && own.parent.is_none());
}

#[test]
fn idle_status_and_tracking_use_one_git_process() {
    let repository = TestRepository::new();
    repository.commit_all("Initial");
    repository.git(&["branch", "tracking"]);
    repository.git(&["config", "branch.main.remote", "."]);
    repository.git(&["config", "branch.main.merge", "refs/heads/tracking"]);
    GIT_COMMAND_COUNT.with(|count| count.set(0));
    let (status, tracking) = status_with_tracking(&repository.0).unwrap();
    assert!(status.files.is_empty());
    assert_eq!(status.branch.as_deref(), Some("main"));
    assert_eq!(tracking.upstream.as_deref(), Some("tracking"));
    assert_eq!((tracking.ahead, tracking.behind), (0, 0));
    assert_eq!(GIT_COMMAND_COUNT.with(|count| count.get()), 1);
}

#[test]
fn combined_status_preserves_changes_and_branch_states() {
    let repository = TestRepository::new();
    let check = || {
        let (combined, tracking) = status_with_tracking(&repository.0).unwrap();
        assert_eq!(combined, status(&repository.0).unwrap());
        assert_eq!(tracking, branch_info(&repository.0).unwrap());
    };
    // An unborn branch and untracked files still have a useful status.
    repository.write("--über old name.txt", "initial\n");
    check();
    stage_all(&repository.0).unwrap();
    check();
    commit(&repository.0, "Initial").unwrap();
    repository.git(&["branch", "tracking"]);
    repository.git(&["config", "branch.main.remote", "."]);
    repository.git(&["config", "branch.main.merge", "refs/heads/tracking"]);
    repository.write("local.txt", "local\n");
    repository.commit_all("Ahead");
    repository.git(&["checkout", "tracking"]);
    repository.write("remote.txt", "remote\n");
    repository.commit_all("Behind");
    repository.git(&["checkout", "main"]);
    check();
    fs::rename(
        repository.0.join("--über old name.txt"),
        repository.0.join("--über renamed file.txt"),
    )
    .unwrap();
    stage_all(&repository.0).unwrap();
    repository.write("--über renamed file.txt", "working change\n");
    repository.write("untracked file.txt", "new\n");
    check();
    repository.commit_all("Rename");
    repository.git(&["checkout", "--detach"]);
    check();
}

#[test]
fn combined_status_preserves_merge_conflicts() {
    let repository = TestRepository::new();
    repository.write("conflict.txt", "base\n");
    repository.commit_all("Base");
    repository.git(&["branch", "other"]);
    repository.write("conflict.txt", "main\n");
    repository.commit_all("Main");
    repository.git(&["checkout", "other"]);
    repository.write("conflict.txt", "other\n");
    repository.commit_all("Other");
    let merge = raw_git(
        &repository.0,
        &string_args(&["merge", "main"]),
        "create merge conflict",
    )
    .unwrap();
    assert!(!merge.status.success());
    let (combined, _) = status_with_tracking(&repository.0).unwrap();
    assert_eq!(combined, status(&repository.0).unwrap());
    assert_eq!(combined.files.len(), 1);
    assert!(combined.files[0].is_conflicted());
}

#[test]
fn staging_and_unstaging_before_first_commit_preserve_files() {
    let repository = TestRepository::new();
    let file = "--über file.rs";
    repository.write(file, "fn main() {}\n");
    stage_paths(&repository.0, &[PathBuf::from(file)]).unwrap();
    let repository_status = status(&repository.0).unwrap();
    assert_eq!(
        working_tree_stats(&repository.0, &repository_status.files)
            .unwrap()
            .staged[file]
            .added,
        1
    );
    assert!(
        repository_status
            .files
            .iter()
            .any(|entry| entry.path == file && entry.is_staged())
    );
    unstage_paths(&repository.0, &[PathBuf::from(file)]).unwrap();
    assert_eq!(
        fs::read_to_string(repository.0.join(file)).unwrap(),
        "fn main() {}\n"
    );
    assert!(
        status(&repository.0)
            .unwrap()
            .files
            .iter()
            .any(|entry| entry.path == file && entry.is_untracked())
    );
    stage_all(&repository.0).unwrap();
    unstage_all(&repository.0).unwrap();
    assert!(
        status(&repository.0)
            .unwrap()
            .files
            .iter()
            .all(FileStatus::is_untracked)
    );
}

#[test]
fn corrupt_head_does_not_look_like_an_unborn_branch_or_modify_the_index() {
    let repository = TestRepository::new();
    repository.write("code.rs", "original\n");
    repository.commit_all("Initial");
    repository.write("code.rs", "staged\n");
    stage_paths(&repository.0, &[PathBuf::from("code.rs")]).unwrap();
    let index = fs::read(repository.0.join(".git/index")).unwrap();
    fs::write(repository.0.join(".git/refs/heads/main"), "invalid ref\n").unwrap();
    assert!(recent_log(&repository.0, 6).is_err());
    assert!(unstage_all(&repository.0).is_err());
    assert_eq!(fs::read(repository.0.join(".git/index")).unwrap(), index);
}

#[test]
fn relative_paths_cannot_escape_the_repository() {
    let repository = TestRepository::new();
    for path in [
        PathBuf::from("../outside.txt"),
        repository.0.join("inside.txt"),
        PathBuf::new(),
    ] {
        assert!(stage_paths(&repository.0, std::slice::from_ref(&path)).is_err());
        assert!(unstage_paths(&repository.0, std::slice::from_ref(&path)).is_err());
        assert!(diff(&repository.0, &path, false).is_err());
    }
    assert!(commit(&repository.0, " \n\t").is_err());
    assert!(stage_paths(&repository.0, &[]).is_ok());
}

#[test]
fn porcelain_v2_preserves_paths_and_rejects_truncated_records() {
    let (status, tracking, head) = parse_status_with_tracking(
        b"# branch.oid (initial)\0# branch.head main\0# future.header data\0\
          1 .M N... 100644 100644 100644 a b tab\tand\nnewline.txt\0\
          2 R. N... 100644 100644 100644 a b R100 new name.txt\0old name.txt\0\
          u UU N... 100644 100644 100644 100644 a b c conflict.txt\0\
          ? new file.txt\0",
    )
    .unwrap();
    assert_eq!(tracking.name.as_deref(), Some("main"));
    assert_eq!(head, Some(StatusHead::Unborn));
    assert_eq!(status.files.len(), 4);
    assert_eq!(status.files[0].path, "tab\tand\nnewline.txt");
    assert_eq!(status.files[0].index, FileChange::None);
    assert_eq!(status.files[0].worktree, FileChange::Modified);
    assert_eq!(status.files[1].path, "new name.txt");
    assert_eq!(
        status.files[1].original_path.as_deref(),
        Some("old name.txt")
    );
    assert!(status.files[2].is_conflicted());
    assert!(status.files[3].is_untracked());
    for invalid in [
        b"# branch.head main".as_slice(),
        b"# branch.head main\0# branch.ab 1 -2\0",
        b"# branch.head main\0# branch.ab +1 -x\0",
        b"# branch.head main\0? \0",
        b"# branch.head main\x001 .M missing fields\0",
        b"# branch.head main\x002 R. N... 100644 100644 100644 a b R100 new.txt\0",
    ] {
        assert!(parse_status_with_tracking(invalid).is_err());
    }
}

#[test]
fn rejects_malformed_status_records_and_preserves_conflicts() {
    assert!(parse_status(b"R  new.txt\0").is_err());
    assert!(parse_status(b"M\0").is_err());
    let files = parse_status(
        b"UU conflict.txt\0?? untracked.txt\0AA added-twice.txt\0DD deleted-twice.txt\0",
    )
    .unwrap();
    assert_eq!(files[0].index, FileChange::Unmerged);
    assert_eq!(files[0].worktree, FileChange::Unmerged);
    assert!(files[1].is_untracked());
    assert!(!files[1].is_staged());
    assert!(files[0].is_conflicted() && files[2].is_conflicted() && files[3].is_conflicted());
    assert!(!files[1].is_conflicted());
}

#[test]
fn worktree_porcelain_keeps_main_linked_detached_bare_and_prunable_entries() {
    let main = "a".repeat(40);
    let linked = "b".repeat(40);
    let zero = "0".repeat(40);
    let output = format!(
        "worktree C:/Code/vEdit\0HEAD {main}\0branch refs/heads/main\0\0\
         worktree C:/Code/vEdit worktrees/Agent Überarbeitung\0HEAD {linked}\0branch refs/heads/feature/Überarbeitung\0locked\0\0\
         worktree C:/Code/detached\nline\0HEAD {main}\0detached\0locked agent busy\0future-attribute value\0\0\
         worktree C:/Code/gone\0HEAD {linked}\0detached\0prunable gitdir file points to non-existent location\0\0\
         worktree C:/Code/new branch\0HEAD {zero}\0branch refs/heads/first\0\0"
    );
    let entries = parse_worktree_list(output.as_bytes(), 0).unwrap();
    assert_eq!(entries.len(), 5);
    assert_eq!(entries[0].path, PathBuf::from("C:/Code/vEdit"));
    assert_eq!(entries[0].head.as_deref(), Some(main.as_str()));
    assert_eq!(entries[0].branch.as_deref(), Some("main"));
    assert!(!entries[0].bare && !entries[0].detached);
    assert_eq!(
        entries[1].path,
        PathBuf::from("C:/Code/vEdit worktrees/Agent Überarbeitung")
    );
    assert_eq!(entries[1].branch.as_deref(), Some("feature/Überarbeitung"));
    assert_eq!(entries[1].locked.as_deref(), Some(""));
    assert_eq!(entries[2].path, PathBuf::from("C:/Code/detached\nline"));
    assert!(entries[2].detached && entries[2].branch.is_none());
    assert_eq!(entries[2].locked.as_deref(), Some("agent busy"));
    assert!(
        entries[3]
            .prunable
            .as_deref()
            .unwrap()
            .contains("non-existent")
    );
    assert_eq!(entries[4].head, None, "an unborn branch has no commit");
    assert_eq!(entries[4].branch.as_deref(), Some("first"));

    let bare = parse_worktree_list(
        format!(
            "worktree C:/Code/repo.git\r\nbare\r\n\r\nworktree C:/Code/repo main\r\nHEAD {main}\r\nbranch refs/heads/main\r\n\r\n"
        )
        .as_bytes(),
        b'\n',
    )
    .unwrap();
    assert_eq!(bare.len(), 2);
    assert!(bare[0].bare && bare[0].head.is_none() && bare[0].branch.is_none());
    assert_eq!(bare[0].path, PathBuf::from("C:/Code/repo.git"));
    assert_eq!(bare[1].path, PathBuf::from("C:/Code/repo main"));
    assert_eq!(bare[1].branch.as_deref(), Some("main"));

    assert!(parse_worktree_list(b"", 0).unwrap().is_empty());
    for invalid in [
        b"HEAD 0000\0\0".as_slice(),
        b"worktree \0\0",
        b"worktree C:/a\0HEAD xyz\0\0",
        b"worktree C:/a\0\0bare\0\0",
    ] {
        assert!(parse_worktree_list(invalid, 0).is_err());
    }
}

#[test]
fn status_oid_and_commit_headlines_parse_without_losing_fields() {
    let hash = "c".repeat(40);
    assert_eq!(
        status_branch_oid(format!("# branch.oid {hash}\0# branch.head main\0").as_bytes()),
        Some(hash.clone())
    );
    assert_eq!(
        status_branch_oid(b"# branch.oid (initial)\0# branch.head main\0"),
        None
    );
    let headlines = parse_headlines(
        format!(
            "{hash}\x001700000000\0Alex Überarbeitung\0Fix: tabs\tand spaces\0\n{}\x001700000100\0Bo\0\0\n",
            "d".repeat(40)
        )
        .as_bytes(),
    )
    .unwrap();
    assert_eq!(headlines.len(), 2);
    assert_eq!(headlines[0].hash, hash);
    assert_eq!(headlines[0].timestamp, 1_700_000_000);
    assert_eq!(headlines[0].author, "Alex Überarbeitung");
    assert_eq!(headlines[0].subject, "Fix: tabs\tand spaces");
    assert_eq!(headlines[1].subject, "");
    assert!(parse_headlines(format!("{hash}\x001\0author\0").as_bytes()).is_err());
    assert!(parse_headlines(format!("{hash}\0soon\0a\0s\0").as_bytes()).is_err());
}

#[test]
fn real_worktrees_report_paths_branches_status_and_headlines_cheaply() {
    let repository = TestRepository::new();
    repository.write("base.txt", "base\n");
    repository.commit_all("Base commit");
    let base = repository.head_hash();
    let holder = TestRepository::empty_directory();
    let linked = holder.0.join("Agent Überarbeitung wt");
    let detached = holder.0.join("detached wt");
    let missing = holder.0.join("missing wt");
    let linked_text = linked.to_str().unwrap();
    repository.git(&[
        "worktree",
        "add",
        "-b",
        "feature/Überarbeitung",
        linked_text,
    ]);
    repository.git(&["worktree", "add", "--detach", detached.to_str().unwrap()]);
    repository.git(&["worktree", "add", "--detach", missing.to_str().unwrap()]);
    repository.git(&[
        "worktree",
        "lock",
        "--reason",
        "agent busy",
        detached.to_str().unwrap(),
    ]);
    fs::remove_dir_all(&missing).unwrap();

    take_command_count();
    let entries = worktree_list(&linked).unwrap();
    assert_eq!(take_command_count(), 1);
    assert_eq!(entries.len(), 4);
    let same = |left: &Path, right: &Path| {
        fs::canonicalize(left).unwrap() == fs::canonicalize(right).unwrap()
    };
    assert!(same(&entries[0].path, &repository.0), "main worktree first");
    assert_eq!(entries[0].branch.as_deref(), Some("main"));
    let linked_entry = entries
        .iter()
        .find(|entry| entry.path.exists() && same(&entry.path, &linked))
        .expect("linked worktree with spaces and non-ASCII characters");
    assert_eq!(
        linked_entry.branch.as_deref(),
        Some("feature/Überarbeitung")
    );
    assert_eq!(linked_entry.head.as_deref(), Some(base.as_str()));
    let detached_entry = entries.iter().find(|entry| entry.locked.is_some()).unwrap();
    assert!(detached_entry.detached && detached_entry.branch.is_none());
    assert_eq!(detached_entry.locked.as_deref(), Some("agent busy"));
    assert!(entries.iter().any(|entry| entry.prunable.is_some()));

    // Worktrees share the fixture repository's local identity and hooks.
    fs::write(linked.join("agent.txt"), "agent work\n").unwrap();
    stage_all(&linked).unwrap();
    commit(&linked, "Agent commit").unwrap();
    fs::write(linked.join("base.txt"), "edited\n").unwrap();
    fs::write(linked.join("new file.txt"), "new\n").unwrap();
    repository.git(&["config", "branch.feature/Überarbeitung.remote", "."]);
    repository.git(&[
        "config",
        "branch.feature/Überarbeitung.merge",
        "refs/heads/main",
    ]);
    take_command_count();
    let status = worktree_status(&linked).unwrap();
    assert_eq!(take_command_count(), 1);
    assert_eq!(status.changes, 2);
    assert_eq!(status.conflicts, 0);
    assert_eq!(
        status.tracking.name.as_deref(),
        Some("feature/Überarbeitung")
    );
    assert_eq!(status.tracking.upstream.as_deref(), Some("main"));
    assert_eq!((status.tracking.ahead, status.tracking.behind), (1, 0));
    let agent_head = status.head.clone().unwrap();
    assert_ne!(agent_head, base);

    take_command_count();
    let headlines = commit_headlines(
        &repository.0,
        &[agent_head.clone(), "e".repeat(40), base.clone()],
    )
    .unwrap();
    assert_eq!(take_command_count(), 1);
    assert_eq!(
        headlines
            .iter()
            .map(|headline| (headline.hash.as_str(), headline.subject.as_str()))
            .collect::<Vec<_>>(),
        [
            (agent_head.as_str(), "Agent commit"),
            (base.as_str(), "Base commit")
        ],
        "a missing object is skipped and input order is kept"
    );
    assert_eq!(headlines[0].author, "vEdit Git Test");
    assert!(headlines[0].timestamp > 0);
    assert!(commit_headlines(&repository.0, &["not-a-hash".into()]).is_err());
    take_command_count();
    assert!(commit_headlines(&repository.0, &[]).unwrap().is_empty());
    assert_eq!(take_command_count(), 0);

    let unborn = TestRepository::new();
    let status = worktree_status(&unborn.0).unwrap();
    assert_eq!(status.head, None);
    assert_eq!(status.tracking.name.as_deref(), Some("main"));
    assert_eq!(worktree_list(&unborn.0).unwrap()[0].head, None);
}

#[test]
fn too_many_untracked_files_fall_back_to_untracked_folders() {
    let repository = TestRepository::new();
    repository.write("tracked.txt", "tracked\n");
    repository.commit_all("base");
    fs::create_dir(repository.0.join("build output")).unwrap();
    for index in 0..200 {
        repository.write(&format!("build output/artifact-{index:03}.o"), "binary");
    }
    repository.write("tracked.txt", "changed\n");
    let (status, _, _) = status_with_head(&repository.0).unwrap();
    assert!(!status.untracked_collapsed);
    assert_eq!(status.files.len(), 201);

    STATUS_LIMIT.with(|limit| limit.set(Some(4096)));
    let collapsed = status_with_head(&repository.0);
    let overview = worktree_status(&repository.0);
    STATUS_LIMIT.with(|limit| limit.set(None));
    let (status, tracking, head) = collapsed.unwrap();
    assert!(status.untracked_collapsed);
    assert_eq!(tracking.name.as_deref(), Some("main"));
    assert!(matches!(head, Some(StatusHead::Commit(_))));
    let names: Vec<_> = status.files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(names, ["tracked.txt", "build output/"]);
    assert_eq!(overview.unwrap().changes, 2);
}

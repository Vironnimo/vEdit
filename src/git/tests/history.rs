use super::*;

#[test]
fn unchanged_history_snapshot_uses_two_git_processes() {
    let repository = TestRepository::new();
    repository.commit_all("Initial");
    let head = repository.head_hash();
    repository.git(&["update-ref", "refs/remotes/origin/main", &head]);
    let previous = history_snapshot(&repository.0).unwrap();
    GIT_COMMAND_COUNT.with(|count| count.set(0));
    let refreshed = history_snapshot_cached(&repository.0, Some(&previous)).unwrap();
    assert_eq!(refreshed, previous);
    assert!(std::sync::Arc::ptr_eq(
        &refreshed.local_only,
        &previous.local_only
    ));
    assert_eq!(GIT_COMMAND_COUNT.with(|count| count.get()), 2);
}

/// The refresh path must give the same snapshot and main comparison as
/// separate HEAD and main-branch reads.
fn assert_refresh_after_status_matches(root: &Path) {
    let previous = history_snapshot(root).ok();
    let (status, _, head) = status_with_head(root).unwrap();
    let (snapshot, refs) = history_snapshot_after_status(
        root,
        previous.as_ref(),
        status.branch.as_deref(),
        head.as_ref(),
    )
    .unwrap();
    assert_eq!(
        snapshot,
        history_snapshot_cached(root, previous.as_ref()).unwrap()
    );
    match (
        main_comparison_after_history(root, &refs),
        main_comparison(root),
    ) {
        (Ok(combined), Ok(separate)) => assert_eq!(combined, separate),
        (Err(_), Err(_)) => {}
        other => panic!("main comparison differs: {other:?}"),
    }
}

#[test]
fn refresh_after_status_matches_separate_head_and_main_reads() {
    let repository = TestRepository::new();
    // Unborn main, then a first commit with remote-tracking refs and a tag.
    assert_refresh_after_status_matches(&repository.0);
    repository.commit_all("Initial");
    let first = repository.head_hash();
    repository.git(&["update-ref", "refs/remotes/origin/main", &first]);
    repository.git(&[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/main",
    ]);
    repository.git(&["tag", "-a", "-m", "First", "v1"]);
    assert_refresh_after_status_matches(&repository.0);
    // A feature branch that is ahead, and main moving on behind it.
    repository.git(&["switch", "-c", "feature"]);
    repository.write("feature.txt", "feature\n");
    repository.commit_all("Feature");
    assert_refresh_after_status_matches(&repository.0);
    repository.git(&["switch", "main"]);
    repository.write("main.txt", "main\n");
    repository.commit_all("Main");
    assert_refresh_after_status_matches(&repository.0);
    repository.git(&["switch", "feature"]);
    assert_refresh_after_status_matches(&repository.0);
    // A feature branch at the main commit and a detached HEAD.
    repository.git(&["switch", "-c", "same", "main"]);
    assert_refresh_after_status_matches(&repository.0);
    repository.git(&["switch", "--detach", "feature"]);
    assert_refresh_after_status_matches(&repository.0);
    repository.git(&["switch", "--detach", "main"]);
    assert_refresh_after_status_matches(&repository.0);
    // master as the base, no base at all, and an orphan branch whose
    // unborn HEAD cannot be compared.
    repository.git(&["switch", "feature"]);
    repository.git(&["branch", "-m", "main", "master"]);
    assert_refresh_after_status_matches(&repository.0);
    repository.git(&["branch", "-m", "master", "trunk"]);
    assert_refresh_after_status_matches(&repository.0);
    repository.git(&["branch", "-m", "trunk", "main"]);
    repository.git(&["switch", "--orphan", "orphan"]);
    assert_refresh_after_status_matches(&repository.0);
}

#[test]
fn idle_refresh_takes_head_from_status_and_base_from_history_refs() {
    let repository = TestRepository::new();
    repository.commit_all("Initial");
    let head = repository.head_hash();
    repository.git(&["update-ref", "refs/remotes/origin/main", &head]);
    let refresh = |previous: &HistorySnapshot| {
        let (status, _, head) = status_with_head(&repository.0).unwrap();
        let (snapshot, refs) = history_snapshot_after_status(
            &repository.0,
            Some(previous),
            status.branch.as_deref(),
            head.as_ref(),
        )
        .unwrap();
        (
            snapshot,
            main_comparison_after_history(&repository.0, &refs).unwrap(),
        )
    };
    // Status and the ref listing; HEAD at main needs no count.
    let previous = history_snapshot(&repository.0).unwrap();
    take_command_count();
    let (snapshot, comparison) = refresh(&previous);
    assert_eq!(take_command_count(), 2);
    assert_eq!(snapshot, previous);
    assert_eq!(
        comparison,
        Some(BranchComparison {
            base: "main".into(),
            ahead: 0,
            behind: 0,
        })
    );
    // A branch away from main still counts its commits.
    repository.git(&["switch", "-c", "feature"]);
    repository.write("feature.txt", "feature\n");
    repository.commit_all("Feature");
    let previous = history_snapshot(&repository.0).unwrap();
    take_command_count();
    let (snapshot, comparison) = refresh(&previous);
    assert_eq!(take_command_count(), 3);
    assert_eq!(snapshot, previous);
    assert_eq!(
        comparison,
        Some(BranchComparison {
            base: "main".into(),
            ahead: 1,
            behind: 0,
        })
    );
}

/// Paired timing of the periodic refresh sequence: separate HEAD and main
/// reads versus HEAD from status and the base from the history refs.
#[test]
#[ignore = "manual benchmark; launches several hundred Git processes"]
fn benchmark_idle_repository_refresh() {
    const COMMITS: usize = 2000;
    const BRANCHES: usize = 200;
    const ROUNDS: u32 = 20;
    let repository = TestRepository::new();
    let mut stream = String::new();
    for index in 1..=COMMITS {
        let message = format!("Change {index}\n");
        let contents = format!("{index}\n");
        stream.push_str(&format!(
            "commit refs/heads/main\nmark :{index}\ncommitter Bench <bench@example.invalid> {} +0000\ndata {}\n{message}M 644 inline file.txt\ndata {}\n{contents}\n",
            1_700_000_000 + index,
            message.len(),
            contents.len(),
        ));
    }
    for index in 0..BRANCHES {
        let mark = COMMITS - index * 7;
        stream.push_str(&format!(
            "reset refs/heads/branch-{index}\nfrom :{mark}\n\nreset refs/remotes/origin/branch-{index}\nfrom :{mark}\n\nreset refs/tags/v{index}\nfrom :{mark}\n\n"
        ));
    }
    let output = raw_git_with_input(
        &repository.0,
        &string_args(&["fast-import", "--quiet"]),
        "import benchmark history",
        Some(stream.into_bytes()),
    )
    .unwrap();
    assert!(output.status.success(), "{output:?}");
    repository.git(&["reset", "--hard", "main"]);
    repository.commit_all("Ignore fixture files");
    repository.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);

    let separate = |previous: &HistorySnapshot| {
        let repo = project_repository(&repository.0, None)
            .unwrap()
            .root
            .unwrap();
        let (status, _) = status_with_tracking(&repo).unwrap();
        working_tree_stats(&repo, &status.files).unwrap();
        let snapshot = history_snapshot_cached(&repo, Some(previous)).unwrap();
        (snapshot, main_comparison(&repo).unwrap())
    };
    let combined = |previous: &HistorySnapshot| {
        let repo = project_repository(&repository.0, None)
            .unwrap()
            .root
            .unwrap();
        let (status, _, head) = status_with_head(&repo).unwrap();
        working_tree_stats(&repo, &status.files).unwrap();
        let (snapshot, refs) = history_snapshot_after_status(
            &repo,
            Some(previous),
            status.branch.as_deref(),
            head.as_ref(),
        )
        .unwrap();
        (
            snapshot,
            main_comparison_after_history(&repo, &refs).unwrap(),
        )
    };
    for branch in ["main", "branch-3"] {
        repository.git(&["switch", "--quiet", branch]);
        let previous = history_snapshot(&repository.0).unwrap();
        assert_eq!(separate(&previous), combined(&previous));
        let mut times = [Duration::ZERO; 2];
        let mut processes = [0; 2];
        type Refresh<'a> =
            &'a dyn Fn(&HistorySnapshot) -> (HistorySnapshot, Option<BranchComparison>);
        let refreshes: [Refresh; 2] = [&separate, &combined];
        for _ in 0..ROUNDS {
            for (index, refresh) in refreshes.iter().enumerate() {
                take_command_count();
                let started = Instant::now();
                let (snapshot, _) = refresh(&previous);
                times[index] += started.elapsed();
                processes[index] += take_command_count();
                assert_eq!(snapshot, previous);
            }
        }
        for (index, name) in ["separate HEAD and main reads", "HEAD from status"]
            .into_iter()
            .enumerate()
        {
            eprintln!(
                "{branch:<9} {name:<29} {:>7.2} ms per refresh, {} Git processes",
                times[index].as_secs_f64() * 1000.0 / f64::from(ROUNDS),
                processes[index] / ROUNDS as usize,
            );
        }
    }
}

#[test]
fn history_has_real_merge_parents_local_remote_refs_and_annotated_tags() {
    let repository = TestRepository::new();
    repository.write("base.txt", "base\n");
    repository.commit_all("Base");
    repository.git(&["checkout", "-b", "feature/split-views"]);
    repository.write("feature.txt", "feature\n");
    repository.commit_all("Feature");
    let feature = repository.head_hash();
    repository.git(&["checkout", "main"]);
    repository.write("main.txt", "main\n");
    repository.commit_all("Main");
    repository.git(&["merge", "--no-ff", "--no-edit", "feature/split-views"]);
    let merge = repository.head_hash();
    repository.git(&["update-ref", "refs/remotes/origin/main", &merge]);
    repository.git(&[
        "update-ref",
        "refs/remotes/origin/feature/split-views",
        &feature,
    ]);
    repository.git(&[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/main",
    ]);
    repository.git(&["tag", "-a", "v0.4.4", "-m", "Release"]);
    let blob = String::from_utf8(repository.git(&["rev-parse", "HEAD:base.txt"]).stdout).unwrap();
    repository.git(&["update-ref", "refs/tags/a-blob-tag", blob.trim()]);
    let snapshot = history_snapshot(&repository.0).unwrap();
    assert!(
        snapshot
            .refs
            .iter()
            .any(|reference| reference.current && reference.name == "main")
    );
    assert!(
        snapshot
            .refs
            .iter()
            .any(|reference| reference.kind == RefKind::Remote && reference.name == "origin/main")
    );
    assert!(
        snapshot
            .refs
            .iter()
            .any(|reference| reference.kind == RefKind::Tag
                && reference.name == "v0.4.4"
                && reference.hash == merge)
    );
    assert!(
        !snapshot
            .refs
            .iter()
            .any(|reference| reference.name == "origin/HEAD" || reference.name == "a-blob-tag")
    );
    let page = history_page(&repository.0, &snapshot, 0, 20).unwrap();
    assert!(!page.has_more);
    assert_eq!(page.commits.len(), 4);
    assert_eq!(page.commits[0].hash, merge);
    assert_eq!(page.commits[0].parents.len(), 2);
    assert_eq!(page.commits[0].parents[1], feature);
    assert!(
        page.commits[0]
            .refs
            .iter()
            .any(|reference| reference.current)
    );
    let positions: HashMap<_, _> = page
        .commits
        .iter()
        .enumerate()
        .map(|(index, commit)| (&commit.hash, index))
        .collect();
    for (index, commit) in page.commits.iter().enumerate() {
        for parent in &commit.parents {
            assert!(
                positions[parent] > index,
                "parents must follow all their children"
            );
        }
    }
    repository.git(&["checkout", "--detach", &feature]);
    let detached = history_snapshot(&repository.0).unwrap();
    assert!(detached.refs.iter().any(|reference| reference.current
        && reference.kind == RefKind::Head
        && reference.hash == feature));
    assert!(
        !detached
            .refs
            .iter()
            .any(|reference| reference.current && reference.kind == RefKind::Local)
    );
}

#[test]
fn history_pages_cover_long_history_and_stay_frozen_when_refs_move() {
    let repository = TestRepository::new();
    // One import produces enough genuine commits to cross several pages
    // without spawning a Git process for each fixture commit.
    let mut input = String::new();
    for mark in 1..=137 {
        let message = format!("History {mark}\n");
        let contents = format!("Revision {mark}\n");
        input.push_str(&format!("commit refs/heads/main\nmark :{mark}\ncommitter vEdit Test <test@example.invalid> {} +0000\ndata {}\n{}",
            1_700_000_000 + mark, message.len(), message));
        if mark > 1 {
            input.push_str(&format!("from :{}\n", mark - 1));
        }
        input.push_str(&format!(
            "M 100644 inline history.txt\ndata {}\n{}\n",
            contents.len(),
            contents
        ));
    }
    input.push_str("done\n");
    let output = raw_git_with_input(
        &repository.0,
        &string_args(&["fast-import", "--quiet", "--done"]),
        "build history fixture",
        Some(input.into_bytes()),
    )
    .unwrap();
    assert!(output.status.success(), "{}", output_text(&output));
    let snapshot = history_snapshot(&repository.0).unwrap();
    let full = history_page(&repository.0, &snapshot, 0, 1000).unwrap();
    assert_eq!(full.commits.len(), 137);
    let first = history_page(&repository.0, &snapshot, 0, HISTORY_PAGE_SIZE).unwrap();
    assert!(first.has_more);
    assert_eq!(first.commits.len(), HISTORY_PAGE_SIZE);
    // A rewound local branch and a new remote branch must not change the
    // traversal or decorations already being browsed.
    repository.git(&["update-ref", "refs/heads/main", &full.commits[10].hash]);
    repository.git(&[
        "update-ref",
        "refs/remotes/origin/review",
        &full.commits[5].hash,
    ]);
    assert_ne!(snapshot, history_snapshot(&repository.0).unwrap());
    let mut commits = first.commits;
    while commits.len() < full.commits.len() {
        let page =
            history_page(&repository.0, &snapshot, commits.len(), HISTORY_PAGE_SIZE).unwrap();
        assert!(!page.commits.is_empty());
        let has_more = page.has_more;
        commits.extend(page.commits);
        assert_eq!(has_more, commits.len() < full.commits.len());
    }
    assert_eq!(commits, full.commits);
    assert!(
        !history_page(&repository.0, &snapshot, commits.len(), HISTORY_PAGE_SIZE)
            .unwrap()
            .has_more
    );
}

#[test]
fn remote_only_unmerged_branch_is_in_history() {
    let repository = TestRepository::new();
    repository.write("base.txt", "base\n");
    repository.commit_all("Base");
    repository.git(&["checkout", "-b", "review"]);
    repository.write("review.txt", "review\n");
    repository.commit_all("Remote review");
    let review = repository.head_hash();
    repository.git(&["checkout", "main"]);
    repository.git(&["update-ref", "refs/remotes/origin/review", &review]);
    repository.git(&["branch", "-D", "review"]);
    let snapshot = history_snapshot(&repository.0).unwrap();
    let page = history_page(&repository.0, &snapshot, 0, 10).unwrap();
    let commit = page
        .commits
        .iter()
        .find(|commit| commit.hash == review)
        .unwrap();
    assert_eq!(commit.subject, "Remote review");
    assert!(commit.refs.iter().any(
        |reference| reference.name == "origin/review" && reference.kind == RefKind::Remote
    ));
    assert_eq!(page.commits.len(), 2);
}

#[test]
fn main_comparison_is_independent_of_tracking_and_counts_both_sides() {
    let repository = TestRepository::new();
    assert_eq!(main_comparison(&repository.0).unwrap(), None);
    repository.commit_all("Initial");
    repository.git(&["switch", "-c", "feature"]);
    repository.write("feature.txt", "first\n");
    repository.commit_all("Feature one");
    repository.git(&["remote", "add", "origin", repository.0.to_str().unwrap()]);
    repository.git(&["update-ref", "refs/remotes/origin/feature", "HEAD"]);
    repository.git(&["config", "branch.feature.remote", "origin"]);
    repository.git(&["config", "branch.feature.merge", "refs/heads/feature"]);
    repository.write("feature.txt", "second\n");
    repository.commit_all("Feature two");
    repository.git(&["switch", "main"]);
    repository.write("main.txt", "base advanced\n");
    repository.commit_all("Main advanced");
    assert_eq!(
        main_comparison(&repository.0).unwrap(),
        Some(BranchComparison {
            base: "main".into(),
            ahead: 0,
            behind: 0,
        })
    );
    repository.git(&["switch", "feature"]);
    assert_eq!(
        main_comparison(&repository.0).unwrap(),
        Some(BranchComparison {
            base: "main".into(),
            ahead: 2,
            behind: 1,
        })
    );
    let (_, tracking) = status_with_tracking(&repository.0).unwrap();
    assert_eq!(tracking.upstream.as_deref(), Some("origin/feature"));
    assert_eq!((tracking.ahead, tracking.behind), (1, 0));
    assert!(tracking.divergence_known);
    repository.git(&["update-ref", "-d", "refs/remotes/origin/feature"]);
    let (_, tracking) = status_with_tracking(&repository.0).unwrap();
    assert_eq!(tracking.upstream.as_deref(), Some("origin/feature"));
    assert!(!tracking.divergence_known);
    repository.git(&["branch", "-m", "main", "master"]);
    assert_eq!(
        main_comparison(&repository.0).unwrap().unwrap().base,
        "master"
    );
    repository.git(&["switch", "--detach"]);
    assert_eq!(main_comparison(&repository.0).unwrap().unwrap().ahead, 2);
}

#[test]
fn new_repository_has_a_branch_and_empty_history() {
    let repository = TestRepository::new();
    assert_eq!(
        current_branch(&repository.0).unwrap().as_deref(),
        Some("main")
    );
    assert!(recent_log(&repository.0, 6).unwrap().is_empty());
    assert_eq!(
        branch_info(&repository.0).unwrap(),
        BranchInfo {
            name: Some("main".into()),
            upstream: None,
            ahead: 0,
            behind: 0,
            divergence_known: false,
        }
    );
}

#[test]
fn renames_keep_both_paths_and_history_preserves_messages() {
    let repository = TestRepository::new();
    repository.write("old name.txt", "same contents\n");
    repository.commit_all("Initial subject with spaces and ü");
    fs::rename(
        repository.0.join("old name.txt"),
        repository.0.join("new name.txt"),
    )
    .unwrap();
    stage_all(&repository.0).unwrap();
    let renamed = status(&repository.0)
        .unwrap()
        .files
        .into_iter()
        .find(|entry| entry.path == "new name.txt")
        .unwrap();
    assert_eq!(renamed.index, FileChange::Renamed);
    assert_eq!(renamed.original_path.as_deref(), Some("old name.txt"));
    commit(&repository.0, "Rename with a literal $(shell) message").unwrap();
    let log = recent_log(&repository.0, 2).unwrap();
    assert_eq!(log.len(), 2);
    assert_eq!(log[0].subject, "Rename with a literal $(shell) message");
    assert_eq!(log[1].subject, "Initial subject with spaces and ü");
    assert_eq!(log[0].author, "vEdit Git Test");
    assert_eq!(log[0].hash.len(), 40);
    assert!(log[0].date.contains('T') && log[0].date.len() >= 20);
    assert!(log[0].timestamp > 0);
    repository.git(&["checkout", "--detach"]);
    assert_eq!(branch_info(&repository.0).unwrap().name, None);
}

#[test]
fn branch_tracking_counts_ahead_and_behind_without_fetching() {
    let repository = TestRepository::new();
    repository.write("common.txt", "initial\n");
    repository.commit_all("Initial");
    repository.git(&["branch", "tracking"]);
    repository.git(&["config", "branch.main.remote", "."]);
    repository.git(&["config", "branch.main.merge", "refs/heads/tracking"]);
    repository.write("main.txt", "main\n");
    repository.commit_all("Main branch commit");
    repository.git(&["checkout", "tracking"]);
    repository.write("upstream.txt", "upstream\n");
    repository.commit_all("Tracking branch commit");
    repository.git(&["checkout", "main"]);
    assert_eq!(
        branch_info(&repository.0).unwrap(),
        BranchInfo {
            name: Some("main".into()),
            upstream: Some("tracking".into()),
            ahead: 1,
            behind: 1,
            divergence_known: true,
        }
    );
    assert!(pull(&repository.0).is_err());
    assert_eq!(
        fs::read_to_string(repository.0.join("main.txt")).unwrap(),
        "main\n"
    );
    assert!(!repository.0.join("upstream.txt").exists());
}

#[test]
fn remote_reachability_is_frozen_and_includes_ancestors_and_local_merges() {
    let repo = TestRepository::new();
    repo.write("file.txt", "base");
    repo.commit_all("base");
    let base = repo.head_hash();
    repo.git(&["update-ref", "refs/remotes/origin/main", &base]);
    repo.git(&["checkout", "-b", "topic"]);
    repo.write("topic.txt", "topic");
    repo.commit_all("topic");
    let topic = repo.head_hash();
    repo.git(&["checkout", "main"]);
    repo.write("file.txt", "local");
    repo.commit_all("local");
    repo.git(&["merge", "--no-ff", "--no-edit", "topic"]);
    let merge = repo.head_hash();
    let snapshot = history_snapshot(&repo.0).unwrap();
    repo.git(&["update-ref", "refs/remotes/origin/main", &merge]);
    let page = history_page(&repo.0, &snapshot, 0, 50).unwrap();
    assert!(
        page.commits
            .iter()
            .find(|c| c.hash == base)
            .unwrap()
            .on_remote
    );
    assert!(
        !page
            .commits
            .iter()
            .find(|c| c.hash == topic)
            .unwrap()
            .on_remote
    );
    assert!(
        !page
            .commits
            .iter()
            .find(|c| c.hash == merge)
            .unwrap()
            .on_remote
    );
    let refreshed = history_snapshot(&repo.0).unwrap();
    assert!(
        history_page(&repo.0, &refreshed, 0, 50)
            .unwrap()
            .commits
            .iter()
            .all(|c| c.on_remote)
    );
    repo.git(&["update-ref", "-d", "refs/remotes/origin/main"]);
    assert!(
        recent_log(&repo.0, 50)
            .unwrap()
            .iter()
            .all(|c| !c.on_remote)
    );
}

#[test]
fn named_branch_switches_preserve_conflicting_files_and_track_remote_branches() {
    let repo = TestRepository::new();
    repo.write("file.txt", "base");
    repo.commit_all("base");
    repo.git(&["checkout", "-b", "topic"]);
    repo.write("file.txt", "topic");
    repo.commit_all("topic");
    assert_eq!(switch_branch(&repo.0, "refs/heads/main").unwrap(), "main");
    repo.write("file.txt", "unsaved on disk");
    assert!(switch_branch(&repo.0, "refs/heads/topic").is_err());
    assert_eq!(
        fs::read_to_string(repo.0.join("file.txt")).unwrap(),
        "unsaved on disk"
    );
    assert_eq!(current_branch(&repo.0).unwrap().as_deref(), Some("main"));
    assert!(switch_branch(&repo.0, "--discard-changes").is_err());
    assert!(switch_branch(&repo.0, "refs/heads/missing").is_err());
    repo.git(&["restore", "file.txt"]);
    repo.git(&["remote", "add", "origin", "."]);
    repo.git(&[
        "update-ref",
        "refs/remotes/origin/review",
        &repo.head_hash(),
    ]);
    assert_eq!(
        switch_branch(&repo.0, "refs/remotes/origin/review").unwrap(),
        "review"
    );
    assert_eq!(
        branch_info(&repo.0).unwrap().upstream.as_deref(),
        Some("origin/review")
    );
}

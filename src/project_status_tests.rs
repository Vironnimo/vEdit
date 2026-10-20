use super::*;

fn fixture(count: usize) -> VEditApp {
    let mut app = VEditApp::empty(egui::Context::default());
    for index in 0..count {
        app.projects.push(Project::new(
            std::env::temp_dir().join(format!("vedit-project-status-{index}")),
        ));
        app.projects[index].last_git_indicator_refresh =
            Instant::now() - Duration::from_secs(40 + index as u64);
    }
    app
}

fn summary(root: &Path, changes: usize) -> Result<Option<GitSummary>, String> {
    Ok(Some(GitSummary {
        repository: root.to_path_buf(),
        status: git::WorktreeStatus {
            head: Some("a".repeat(40)),
            tracking: git::BranchInfo {
                name: Some("main".into()),
                upstream: Some("origin/main".into()),
                ahead: 1,
                behind: 0,
                divergence_known: true,
            },
            changes,
            conflicts: 0,
        },
    }))
}

fn read_uncommitted_count(
    root: &Path,
    approved_parent: Option<&Path>,
) -> Result<Option<usize>, String> {
    read_git_summary(root, approved_parent)
        .map(|summary| summary.map(|summary| summary.status.changes))
}

fn pending(app: &mut VEditApp, index: usize, id: u64) -> PathBuf {
    let root = app.projects[index].root.clone();
    app.projects[index].git_indicator_request = Some(id);
    app.git_indicator_requests.insert(id, root.clone());
    root
}

#[test]
fn project_tab_tooltip_summarizes_branch_divergence_and_conflicts() {
    let root = PathBuf::from("C:/Code/vEdit");
    let mut project = Project::new(root.clone());
    assert_eq!(project.git_tab_summary(), None);
    let mut summary = summary(&root, 2).unwrap().unwrap();
    summary.status.tracking.behind = 2;
    project.git_summary = Some(summary.clone());
    assert_eq!(
        project.git_tab_summary().as_deref(),
        Some("Branch main · ↑1 ↓2")
    );
    summary.status.tracking.ahead = 0;
    summary.status.tracking.behind = 0;
    summary.status.conflicts = 2;
    project.git_summary = Some(summary.clone());
    assert_eq!(
        project.git_tab_summary().as_deref(),
        Some("Branch main · up to date · 2 conflicts")
    );
    summary.status.conflicts = 0;
    summary.status.tracking.divergence_known = false;
    project.git_summary = Some(summary.clone());
    assert_eq!(
        project.git_tab_summary().as_deref(),
        Some("Branch main · ↑? ↓?")
    );
    summary.status.tracking.upstream = None;
    project.git_summary = Some(summary.clone());
    assert_eq!(
        project.git_tab_summary().as_deref(),
        Some("Branch main · no upstream")
    );
    summary.status.tracking.name = None;
    project.git_summary = Some(summary);
    assert_eq!(
        project.git_tab_summary().as_deref(),
        Some("Detached at aaaaaaa")
    );
}

#[test]
fn refreshes_oldest_inactive_projects_with_a_global_budget() {
    let mut app = fixture(5);
    assert_eq!(app.inactive_git_indicator_candidates(), [4, 3]);
    let removed = pending(&mut app, 4, 100);
    app.projects.pop();
    assert_eq!(app.inactive_git_indicator_candidates(), [3]);
    pending(&mut app, 3, 101);
    assert!(app.inactive_git_indicator_candidates().is_empty());
    app.finish_git_indicator(removed.clone(), 100, summary(&removed, 9));
    assert_eq!(app.inactive_git_indicator_candidates(), [2]);
}

#[test]
fn recent_full_refresh_loading_and_git_actions_do_not_launch_badge_workers() {
    let mut app = fixture(4);
    app.projects[1].last_git_refresh = Instant::now();
    app.projects[2].git_loading = true;
    assert_eq!(app.inactive_git_indicator_candidates(), [3]);
    app.git_busy = Some(app.projects[0].root.clone());
    assert!(app.inactive_git_indicator_candidates().is_empty());
}

#[test]
fn close_and_reopen_or_replaced_requests_reject_stale_counts() {
    let mut app = fixture(2);
    let root = pending(&mut app, 1, 200);
    app.projects[1] = Project::new(root.clone());
    app.projects[1].git_indicator_count = Some(0);
    pending(&mut app, 1, 201);
    app.finish_git_indicator(root.clone(), 200, summary(&root, 7));
    assert_eq!(app.projects[1].uncommitted_change_count(), Some(0));
    assert!(app.projects[1].git_summary.is_none());
    assert_eq!(app.projects[1].git_indicator_request, Some(201));
    app.finish_git_indicator(root.clone(), 201, summary(&root, 2));
    assert_eq!(app.projects[1].uncommitted_change_count(), Some(2));
    let status = &app.projects[1].git_summary.as_ref().unwrap().status;
    assert_eq!(status.tracking.upstream.as_deref(), Some("origin/main"));
    assert_eq!(status.tracking.ahead, 1);
    assert!(app.git_indicator_requests.is_empty());
}

#[test]
fn status_failure_is_unknown_and_retries_without_overwriting_full_git_data() {
    let mut app = fixture(2);
    app.projects[1].git_indicator_count = Some(3);
    app.projects[1].git_summary = summary(&app.projects[1].root.clone(), 3).unwrap();
    app.projects[1].git_branch = Some("keep-branch".into());
    app.projects[1].git_error = Some("keep-full-refresh-error".into());
    let previous_refresh = app.projects[1].last_git_refresh;
    let root = pending(&mut app, 1, 300);
    app.finish_git_indicator(root, 300, Err("unavailable".into()));
    let project = &app.projects[1];
    assert_eq!(project.uncommitted_change_count(), None);
    assert!(project.git_summary.is_none(), "unknown, not stale");
    assert_eq!(project.git_branch.as_deref(), Some("keep-branch"));
    assert_eq!(
        project.git_error.as_deref(),
        Some("keep-full-refresh-error")
    );
    assert_eq!(project.last_git_refresh, previous_refresh);
    assert!(!project.git_loading);
    assert!(app.inactive_git_indicator_candidates().is_empty());
    app.projects[1].last_git_indicator_refresh =
        Instant::now() - REFRESH_INTERVAL - Duration::from_secs(1);
    assert_eq!(app.inactive_git_indicator_candidates(), [1]);
}

#[test]
fn full_snapshot_updates_count_and_invalidates_an_older_indicator() {
    let mut app = fixture(2);
    let root = pending(&mut app, 1, 400);
    app.projects[1].git_status_request = Some(401);
    app.sender
        .send(WorkerResult::GitStatus {
            root: root.clone(),
            request_id: 401,
            result: Ok(Box::new(GitSnapshot {
                root: Some(root.clone()),
                parent_root: None,
                branch: Some("main".into()),
                tracking: None,
                main_comparison: Ok(None),
                files: vec![
                    git::FileStatus {
                        path: "change.txt".into(),
                        original_path: None,
                        index: git::FileChange::Modified,
                        worktree: git::FileChange::None,
                    },
                    git::FileStatus {
                        path: "conflict.txt".into(),
                        original_path: None,
                        index: git::FileChange::Unmerged,
                        worktree: git::FileChange::Unmerged,
                    },
                ],
                stats: git::WorkingTreeStats::default(),
                stats_error: None,
                head: None,
                fingerprint: 0,
                history: Ok(GitHistoryRefresh {
                    snapshot: git::HistorySnapshot {
                        refs: vec![git::CommitRef {
                            hash: "b".repeat(40),
                            name: "main".into(),
                            kind: git::RefKind::Local,
                            current: true,
                        }],
                        ..Default::default()
                    },
                    page: None,
                }),
            })),
        })
        .unwrap();
    app.poll();
    assert_eq!(app.projects[1].uncommitted_change_count(), Some(2));
    let full = app.projects[1].git_summary.clone().unwrap();
    assert_eq!(full.repository, root);
    assert_eq!(full.status.head, Some("b".repeat(40)));
    assert_eq!(full.status.tracking.name.as_deref(), Some("main"));
    assert!(!full.status.tracking.divergence_known);
    assert_eq!((full.status.changes, full.status.conflicts), (2, 1));
    assert!(app.projects[1].git_indicator_request.is_none());
    assert_eq!(app.git_indicator_requests.len(), 1);
    app.finish_git_indicator(root.clone(), 400, summary(&root, 0));
    assert_eq!(app.projects[1].uncommitted_change_count(), Some(2));
    assert_eq!(app.projects[1].git_summary, Some(full));
    assert!(app.git_indicator_requests.is_empty());
}

#[test]
fn stale_full_status_cannot_overwrite_a_reopened_projects_badge_or_loading_state() {
    let mut app = fixture(2);
    let root = app.projects[1].root.clone();
    app.projects[1] = Project::new(root.clone());
    app.projects[1].git_loading = true;
    app.projects[1].git_status_request = Some(501);
    app.projects[1].git_indicator_count = Some(4);
    pending(&mut app, 1, 502);
    app.sender
        .send(WorkerResult::GitStatus {
            root,
            request_id: 500,
            result: Err("old repository refresh".into()),
        })
        .unwrap();
    app.poll();
    let project = &app.projects[1];
    assert_eq!(project.uncommitted_change_count(), Some(4));
    assert_eq!(project.git_status_request, Some(501));
    assert_eq!(project.git_indicator_request, Some(502));
    assert!(project.git_loading);
    assert!(project.git_error.is_none());
}

#[test]
fn real_repository_reports_untracked_staged_modified_and_clean_status() {
    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);
    let root = std::env::temp_dir().join(format!(
        "vedit-project-tabs-git-{}-{}",
        std::process::id(),
        NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed),
    ));
    fs::create_dir(&root).unwrap();
    assert_eq!(read_uncommitted_count(&root, None).unwrap(), None);
    git::initialize(&root).unwrap();
    let hooks = root.join(".git").join("isolated-test-hooks");
    fs::create_dir_all(&hooks).unwrap();
    for (key, value) in [
        ("user.name", "vEdit Test"),
        ("user.email", "vedit@example.invalid"),
        ("commit.gpgsign", "false"),
        ("core.hooksPath", hooks.to_str().unwrap()),
    ] {
        let output = std::process::Command::new("git")
            .args(["-C"])
            .arg(&root)
            .args(["config", key, value])
            .output()
            .unwrap();
        assert!(output.status.success());
    }
    assert_eq!(read_uncommitted_count(&root, None).unwrap(), Some(0));
    fs::write(root.join("file.txt"), "first\n").unwrap();
    assert_eq!(read_uncommitted_count(&root, None).unwrap(), Some(1));
    let nested = root.join("examples");
    fs::create_dir(&nested).unwrap();
    assert_eq!(read_uncommitted_count(&nested, None).unwrap(), None);
    assert_eq!(
        read_uncommitted_count(&nested, Some(&root)).unwrap(),
        Some(1)
    );
    git::stage_all(&root).unwrap();
    assert_eq!(read_uncommitted_count(&root, None).unwrap(), Some(1));
    git::commit(&root, "initial").unwrap();
    assert_eq!(read_uncommitted_count(&root, None).unwrap(), Some(0));
    fs::write(root.join("file.txt"), "second\n").unwrap();
    assert_eq!(read_uncommitted_count(&root, None).unwrap(), Some(1));
    let mut app = fixture(1);
    app.projects.push(Project::new(root.clone()));
    app.refresh_inactive_git_indicators();
    let deadline = Instant::now() + Duration::from_secs(8);
    while !app.git_indicator_requests.is_empty() {
        assert!(
            Instant::now() < deadline,
            "inactive status worker timed out"
        );
        app.poll();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(app.active_project, 0);
    assert_eq!(app.projects[1].uncommitted_change_count(), Some(1));
    assert!(app.projects[1].git_changes.is_empty());
    assert!(app.projects[1].recent_commits.is_empty());
    git::stage_all(&root).unwrap();
    git::commit(&root, "save change").unwrap();
    assert_eq!(read_uncommitted_count(&root, None).unwrap(), Some(0));
    app.projects[1].last_git_indicator_refresh =
        Instant::now() - REFRESH_INTERVAL - Duration::from_secs(1);
    app.refresh_inactive_git_indicators();
    let deadline = Instant::now() + Duration::from_secs(8);
    while !app.git_indicator_requests.is_empty() {
        assert!(
            Instant::now() < deadline,
            "inactive clean status worker timed out"
        );
        app.poll();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(app.projects[1].uncommitted_change_count(), Some(0));
    // This unique, explicitly created temporary repository is the only target.
    assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
    assert!(
        root.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("vedit-project-tabs-git-")
    );
    fs::remove_dir_all(root).unwrap();
}

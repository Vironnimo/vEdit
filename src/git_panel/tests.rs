use super::file_tree::{ChangeGroup, TreeContext, TreeFile, queue_file_diff, queue_tree_stage};
use super::*;

fn file(path: &str, stats: Option<git::LineStats>) -> TreeFile {
    TreeFile {
        path: path.to_owned(),
        original_path: None,
        status: git::FileChange::Modified,
        stats,
        conflicted: false,
    }
}

fn counts(added: u64, removed: u64) -> git::LineStats {
    git::LineStats {
        added,
        removed,
        binary: false,
    }
}

#[test]
fn working_cache_updates_statistics_even_when_status_is_unchanged() {
    let files = vec![git::FileStatus {
        path: "src/main.rs".into(),
        original_path: None,
        index: git::FileChange::None,
        worktree: git::FileChange::Modified,
    }];
    let mut stats = git::WorkingTreeStats::default();
    stats.unstaged.insert(files[0].path.clone(), counts(1, 2));
    let mut state = GitViewState::default();
    state.update_working_cache(&files, &stats);
    let cached_files = state.working_cache.as_ref().unwrap().files.as_ptr();
    state.update_working_cache(&files, &stats);
    assert_eq!(
        state.working_cache.as_ref().unwrap().files.as_ptr(),
        cached_files
    );

    stats.unstaged.insert(files[0].path.clone(), counts(40, 3));
    state.update_working_cache(&files, &stats);
    let changed = &state.working_cache.as_ref().unwrap().groups[1].1;
    assert_eq!((changed.summary.added, changed.summary.removed), (40, 3));
    stats.unstaged.clear();
    state.update_working_cache(&files, &stats);
    assert_eq!(
        state.working_cache.as_ref().unwrap().groups[1]
            .1
            .summary
            .known,
        0
    );
}

#[test]
fn keyboard_rows_reuse_cache_and_follow_expansion_index_and_history_changes() {
    let root = Path::new("C:/git-cache-fixture");
    let mut files = vec![git::FileStatus {
        path: "src/main.rs".into(),
        original_path: None,
        index: git::FileChange::None,
        worktree: git::FileChange::Modified,
    }];
    let mut state = GitViewState::default();
    state.update_working_cache(&files, &git::WorkingTreeStats::default());
    assert!(state.update_keyboard_rows(root, true, 1, &[]));
    assert!(!state.update_keyboard_rows(root, true, 1, &[]));
    assert_eq!(state.keyboard_rows.len(), 3);
    state
        .expanded
        .insert("working:changes:folder:src".into(), false);
    assert!(state.update_keyboard_rows(root, true, 1, &[]));
    assert_eq!(state.keyboard_rows.len(), 2);
    files[0].index = git::FileChange::Modified;
    files[0].worktree = git::FileChange::None;
    state.update_working_cache(&files, &git::WorkingTreeStats::default());
    assert!(state.update_keyboard_rows(root, true, 1, &[]));
    assert_eq!(
        state.keyboard_rows[2],
        git_row_id(root, "working:staged:file:src/main.rs")
    );
    state.section = GitSection::History;
    let mut commits = vec![git::CommitSummary {
        hash: "first".into(),
        parents: vec![],
        refs: vec![],
        author: String::new(),
        date: String::new(),
        timestamp: 0,
        on_remote: false,
        subject: String::new(),
    }];
    assert!(state.update_keyboard_rows(root, true, 1, &commits));
    assert!(!state.update_keyboard_rows(root, true, 1, &commits));
    commits[0].hash = "replacement".into();
    assert!(state.update_keyboard_rows(root, true, 2, &commits));
    assert_eq!(state.keyboard_rows, [git_row_id(root, "replacement")]);
    assert!(state.update_keyboard_rows(root, false, 2, &commits));
    assert!(state.keyboard_rows.is_empty());
}

#[test]
#[ignore = "Manual paired headless Git panel benchmark; prints timings without timing assertions"]
fn benchmark_large_working_tree_panel() {
    use egui_kittest::Harness;
    for count in [1_000, 5_000] {
        let mut app = VEditApp::empty(egui::Context::default());
        let mut project = Project::new(PathBuf::from("C:/vedit-git-panel-benchmark"));
        project.git_root = Some(project.root.clone());
        project.git_branch = Some("main".into());
        project.last_git_refresh = Instant::now() + Duration::from_secs(3600);
        for index in 0..count {
            let path = format!("src/group{:02}/file{index:05}.rs", index / 100);
            project
                .git_stats
                .unstaged
                .insert(path.clone(), counts(12, 3));
            project.git_changes.push(git::FileStatus {
                path,
                original_path: None,
                index: git::FileChange::None,
                worktree: git::FileChange::Modified,
            });
        }
        app.projects.push(project);
        let mut screen = Harness::builder().with_size([400.0, 700.0]).build_ui_state(
            |ui, app: &mut VEditApp| {
                app.ctx = ui.ctx().clone();
                app.git_ui(ui);
            },
            app,
        );
        set_style(&screen.ctx);
        screen.run_steps(4);
        let mut samples = [Vec::new(), Vec::new()];
        let expected_rows = screen.state().projects[0].git_view.keyboard_rows.clone();
        for pair in 0..5 {
            for mode in [pair % 2, 1 - pair % 2] {
                screen.ctx.data_mut(|data| {
                    data.insert_temp(egui::Id::new("git-panel-benchmark-uncached"), mode == 0)
                });
                screen.run_steps(2);
                let started = Instant::now();
                screen.run_steps(10);
                samples[mode].push(started.elapsed().as_secs_f64() * 100.0);
                assert_eq!(
                    screen.state().projects[0].git_view.keyboard_rows,
                    expected_rows
                );
                std::hint::black_box(screen.output());
            }
        }
        for timings in &mut samples {
            timings.sort_by(f64::total_cmp);
        }
        eprintln!(
            "Git panel {count} changed files: previous frame work {:.3} ms/frame, cached/deferred {:.3} ms/frame (5 paired samples, 10 frames/sample)",
            samples[0][2], samples[1][2]
        );
    }
}

#[test]
fn tree_keeps_nested_folders_and_stable_order_without_filesystem_access() {
    let tree = FileTree::from_files([
        file("src/renderer/z.rs", Some(counts(5, 2))),
        file("README.md", Some(counts(2, 0))),
        file("docs/Pläne/Überblick.md", None),
        file("src/renderer/a.rs", Some(counts(1, 3))),
        file("src/main.rs", Some(counts(7, 4))),
    ]);
    assert_eq!(
        tree.folders.keys().cloned().collect::<Vec<_>>(),
        ["docs", "src"]
    );
    assert_eq!(tree.files[0].path, "README.md");
    let renderer = &tree.folders["src"].folders["renderer"];
    assert_eq!(
        renderer
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        ["src/renderer/a.rs", "src/renderer/z.rs"]
    );
    assert_eq!(renderer.summary.added, 6);
    assert_eq!(renderer.summary.removed, 5);
    assert_eq!(tree.summary.total, 5);
    assert_eq!(tree.summary.known, 4);
    assert_eq!(tree.summary.added, 15);
    assert_eq!(tree.summary.removed, 9);
    assert_eq!(
        tree.folders["docs"].folders["Pläne"].files[0].path,
        "docs/Pläne/Überblick.md"
    );
}

#[test]
fn partially_staged_file_has_two_distinct_rows_and_stats() {
    let files = vec![git::FileStatus {
        path: "src/main.rs".into(),
        original_path: None,
        index: git::FileChange::Modified,
        worktree: git::FileChange::Modified,
    }];
    let stats = git::WorkingTreeStats {
        staged: HashMap::from([("src/main.rs".to_owned(), counts(12, 3))]),
        unstaged: HashMap::from([("src/main.rs".to_owned(), counts(2, 7))]),
    };
    let cache = WorkingTreeCache::new(&files, &stats);
    let staged = &cache.groups[0].1.folders["src"].files[0];
    let unstaged = &cache.groups[1].1.folders["src"].files[0];
    assert_eq!(staged.stats, Some(counts(12, 3)));
    assert_eq!(unstaged.stats, Some(counts(2, 7)));
    assert_eq!(cache.groups[0].1.summary.total, 1);
    assert_eq!(cache.groups[1].1.summary.total, 1);
    assert_eq!(cache.groups[2].1.summary.total, 0);
    assert_ne!(
        TreeContext::Working(ChangeGroup::Staged).key(),
        TreeContext::Working(ChangeGroup::Changes).key()
    );
}

#[test]
fn group_filters_deleted_conflicted_and_untracked_files_correctly() {
    let files = [
        git::FileStatus {
            path: "old.rs".into(),
            original_path: None,
            index: git::FileChange::Deleted,
            worktree: git::FileChange::None,
        },
        git::FileStatus {
            path: "new.rs".into(),
            original_path: None,
            index: git::FileChange::None,
            worktree: git::FileChange::Untracked,
        },
        git::FileStatus {
            path: "conflict.rs".into(),
            original_path: None,
            index: git::FileChange::Unmerged,
            worktree: git::FileChange::Unmerged,
        },
    ];
    let cache = WorkingTreeCache::new(&files, &git::WorkingTreeStats::default());
    assert_eq!(
        cache.groups[0]
            .1
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        ["conflict.rs", "old.rs"]
    );
    assert_eq!(cache.groups[1].1.files[0].path, "conflict.rs");
    assert!(cache.groups[1].1.files[0].conflicted);
    assert_eq!(cache.groups[2].1.files[0].path, "new.rs");
    assert_eq!(cache.groups[2].1.summary.known, 0);
}

#[test]
fn bulk_folder_action_includes_rename_source_and_deduplicates_paths() {
    let mut renamed = file("src/new.rs", Some(counts(1, 1)));
    renamed.status = git::FileChange::Renamed;
    renamed.original_path = Some("src/old.rs".into());
    let tree = FileTree::from_files([
        renamed,
        file("src/old.rs", None),
        file("src/nested/other.rs", None),
        file("docs/plan.md", None),
    ]);
    let mut actions = Vec::new();
    queue_tree_stage(&tree.folders["src"], ChangeGroup::Changes, &mut actions);
    assert_eq!(actions.len(), 1);
    let GitUiAction::Stage(paths) = &actions[0] else {
        panic!("folder should stage, not unstage")
    };
    assert_eq!(
        paths,
        &[
            PathBuf::from("src/nested/other.rs"),
            PathBuf::from("src/new.rs"),
            PathBuf::from("src/old.rs")
        ]
    );
    actions.clear();
    queue_tree_stage(&tree.folders["src"], ChangeGroup::Staged, &mut actions);
    assert!(matches!(actions.as_slice(), [GitUiAction::Unstage(_)]));
}

#[test]
fn staged_rename_unstaged_edit_and_copies_do_not_mutate_original_sources() {
    let files = [git::FileStatus {
        path: "src/new.rs".into(),
        original_path: Some("src/old.rs".into()),
        index: git::FileChange::Renamed,
        worktree: git::FileChange::Modified,
    }];
    let cache = WorkingTreeCache::new(&files, &git::WorkingTreeStats::default());
    let mut actions = Vec::new();
    queue_tree_stage(&cache.groups[1].1, ChangeGroup::Changes, &mut actions);
    assert!(
        matches!(actions.as_slice(), [GitUiAction::Stage(paths)] if paths == &[PathBuf::from("src/new.rs")])
    );
    actions.clear();
    queue_tree_stage(&cache.groups[0].1, ChangeGroup::Staged, &mut actions);
    assert!(
        matches!(actions.as_slice(), [GitUiAction::Unstage(paths)] if paths == &[PathBuf::from("src/new.rs"), PathBuf::from("src/old.rs")])
    );
    let mut copied = file("copy.rs", None);
    copied.status = git::FileChange::Copied;
    copied.original_path = Some("source.rs".into());
    assert_eq!(copied.action_paths(), [PathBuf::from("copy.rs")]);
}

#[test]
fn commit_file_action_always_requests_that_commits_patch() {
    let file = file("src/deleted.rs", Some(counts(0, 9)));
    let hash = "0123456789abcdef0123456789abcdef01234567";
    let mut actions = Vec::new();
    queue_file_diff(&file, TreeContext::Commit(hash), &mut actions);
    assert!(
        matches!(actions.as_slice(), [GitUiAction::HistoricalDiff { hash: requested, path }] if requested == hash && path == Path::new("src/deleted.rs"))
    );
    actions.clear();
    queue_file_diff(
        &file,
        TreeContext::Working(ChangeGroup::Staged),
        &mut actions,
    );
    assert!(matches!(
        actions.as_slice(),
        [GitUiAction::Diff {
            staged: true,
            untracked: false,
            ..
        }]
    ));
    actions.clear();
    queue_file_diff(
        &file,
        TreeContext::Working(ChangeGroup::Untracked),
        &mut actions,
    );
    assert!(matches!(
        actions.as_slice(),
        [GitUiAction::Diff {
            staged: false,
            untracked: true,
            ..
        }]
    ));
}

#[test]
fn unknown_and_binary_line_stats_remain_distinguishable_from_zero_changes() {
    let tree = FileTree::from_files([
        file("large.rs", None),
        file(
            "image.png",
            Some(git::LineStats {
                added: 0,
                removed: 0,
                binary: true,
            }),
        ),
        file("empty.md", Some(counts(0, 0))),
    ]);
    assert_eq!(tree.summary.total, 3);
    assert_eq!(tree.summary.known, 2);
    assert_eq!(tree.summary.binary, 1);
    assert!(
        tree.files
            .iter()
            .find(|file| file.path == "large.rs")
            .unwrap()
            .stats
            .is_none()
    );
    assert_eq!(
        tree.files
            .iter()
            .find(|file| file.path == "empty.md")
            .unwrap()
            .stats,
        Some(counts(0, 0))
    );
}

use super::*;
use egui_kittest::kittest::Queryable;

fn git_workspace() -> MorrowApp {
    let mut app = MorrowApp::empty(egui::Context::default());
    visual_tests::workspace(&mut app);
    app.sidebar = SidebarView::SourceControl;
    let project = &mut app.projects[0];
    project.git_root = Some(project.root.clone());
    project.last_git_refresh = Instant::now() + Duration::from_secs(3600);
    project.git_changes = vec![
        git::FileStatus {
            path: "src/main.rs".into(),
            original_path: None,
            index: git::FileChange::Modified,
            worktree: git::FileChange::Modified,
        },
        git::FileStatus {
            path: "src/ui/old_panel.rs".into(),
            original_path: None,
            index: git::FileChange::None,
            worktree: git::FileChange::Deleted,
        },
        git::FileStatus {
            path: "docs/design notes.md".into(),
            original_path: None,
            index: git::FileChange::None,
            worktree: git::FileChange::Untracked,
        },
        git::FileStatus {
            path: "assets/logo.png".into(),
            original_path: None,
            index: git::FileChange::Added,
            worktree: git::FileChange::None,
        },
    ];
    project.git_stats.staged.insert(
        "src/main.rs".into(),
        git::LineStats {
            added: 24,
            removed: 6,
            binary: false,
        },
    );
    project.git_stats.staged.insert(
        "assets/logo.png".into(),
        git::LineStats {
            added: 0,
            removed: 0,
            binary: true,
        },
    );
    project.git_stats.unstaged.insert(
        "src/main.rs".into(),
        git::LineStats {
            added: 3,
            removed: 1,
            binary: false,
        },
    );
    project.git_stats.unstaged.insert(
        "src/ui/old_panel.rs".into(),
        git::LineStats {
            added: 0,
            removed: 42,
            binary: false,
        },
    );
    project.git_stats.unstaged.insert(
        "docs/design notes.md".into(),
        git::LineStats {
            added: 18,
            removed: 0,
            binary: false,
        },
    );
    project.commit_message = "Improve repository navigation".into();
    for (hash, parents, subject, date) in [
        ('a', "bc", "Improve repository navigation", "2026-09-26"),
        ('b', "d", "Keep Markdown and source together", "2026-09-25"),
        ('c', "d", "Preserve selections in split views", "2026-09-24"),
        ('d', "", "Create the editor workspace", "2026-09-23"),
    ] {
        project.recent_commits.push(git::CommitSummary {
            hash: hash.to_string().repeat(40),
            parents: parents
                .chars()
                .map(|parent| parent.to_string().repeat(40))
                .collect(),
            refs: match hash {
                'a' => vec![git::CommitRef {
                    hash: "a".repeat(40),
                    name: "main".into(),
                    kind: git::RefKind::Local,
                    current: true,
                }],
                'b' => vec![git::CommitRef {
                    hash: "b".repeat(40),
                    name: "origin/main".into(),
                    kind: git::RefKind::Remote,
                    current: false,
                }],
                'c' => vec![
                    git::CommitRef {
                        hash: "c".repeat(40),
                        name: "feature/split-views".into(),
                        kind: git::RefKind::Local,
                        current: false,
                    },
                    git::CommitRef {
                        hash: "c".repeat(40),
                        name: "origin/feature/split-views".into(),
                        kind: git::RefKind::Remote,
                        current: false,
                    },
                ],
                _ => Vec::new(),
            },
            author: "Alex Taylor".into(),
            date: date.into(),
            subject: subject.into(),
        });
    }
    let details = git::CommitDetails {
        hash: "a".repeat(40), parents: vec!["b".repeat(40), "c".repeat(40)], author: "Alex Taylor".into(), author_email: "alex@example.test".into(), date: "2026-09-26T14:32:18+02:00".into(),
        message: "Improve repository navigation\n\nGroup changed files by folder and keep staged changes separate.\nCommit details now show the complete message and historical patches.\n\nReviewed with the editor's Vim navigation enabled.\n".into(),
        files: vec![
            git::CommitFile { path: "src/git_panel.rs".into(), original_path: None, status: git::FileChange::Added, stats: git::LineStats { added: 128, removed: 0, binary: false } },
            git::CommitFile { path: "src/app.rs".into(), original_path: None, status: git::FileChange::Modified, stats: git::LineStats { added: 18, removed: 46, binary: false } },
            git::CommitFile { path: "docs/editor guide.md".into(), original_path: Some("docs/guide.md".into()), status: git::FileChange::Renamed, stats: git::LineStats { added: 6, removed: 2, binary: false } },
            git::CommitFile { path: "assets/logo.png".into(), original_path: None, status: git::FileChange::Modified, stats: git::LineStats { added: 0, removed: 0, binary: true } },
        ],
    };
    project
        .git_view
        .detail_cache
        .insert(details.hash.clone(), details);
    app
}

#[test]
fn git_changes_history_commit_and_diff_snapshots() {
    let mut screen = visual_tests::harness(git_workspace());
    visual_tests::snapshot(&mut screen, "morrow_git_changes");
    screen.get_by_label("History").click();
    screen.run_steps(3);
    assert_eq!(
        screen.state().projects[0].git_view.section,
        git_panel::GitSection::History
    );
    visual_tests::snapshot(&mut screen, "morrow_git_history");
    screen.get_by_label("Improve repository navigation").click();
    screen.run_steps(3);
    let state = &screen.state().projects[0].git_view;
    assert!(state.detail_open && !state.detail_loading);
    assert_eq!(state.detail.as_ref().unwrap().hash, "a".repeat(40));
    assert!(
        screen
            .query_by_label_contains("Group changed files by folder")
            .is_some()
    );
    visual_tests::snapshot(&mut screen, "morrow_git_commit");
    screen.state_mut().diff = Some(("src/git_panel.rs · aaaaaaaa".into(), "diff --git a/src/git_panel.rs b/src/git_panel.rs\nindex 1234567..abcdef0 100644\n--- a/src/git_panel.rs\n+++ b/src/git_panel.rs\n@@ -4,5 +4,7 @@ impl RepositoryView {\n     fn refresh(&mut self) {\n-        self.files = load_changes();\n-        self.show_flat_list();\n+        self.staged = load_staged_changes();\n+        self.working = load_working_changes();\n+        self.show_folder_tree();\n+        self.show_line_statistics();\n     }\n }\n".into()));
    visual_tests::snapshot(&mut screen, "morrow_git_diff");
}

#[test]
fn cached_commit_click_and_escape_preserve_editor_contents() {
    let mut screen = visual_tests::harness(git_workspace());
    screen.run_steps(3);
    let before = screen.state().tab().unwrap().text.clone();
    screen.get_by_label("History").click();
    screen.run_steps(2);
    screen.get_by_label("Improve repository navigation").click();
    screen.run_steps(2);
    assert!(screen.state().git_detail_open());
    screen.key_press(Key::D);
    screen.run_steps(1);
    assert_eq!(screen.state().tab().unwrap().text, before);
    screen.key_press(Key::Escape);
    screen.run_steps(3);
    assert!(!screen.state().git_detail_open());
    assert_eq!(screen.state().tab().unwrap().text, before);
    assert!(!screen.state().tab().unwrap().dirty);
}

#[test]
fn older_commit_result_does_not_replace_the_new_selection() {
    let mut app = git_workspace();
    app.projects[0].git_view.selected_commit = Some("b".repeat(40));
    app.projects[0].git_view.detail_loading = true;
    let detail = app.projects[0].git_view.detail_cache[&"a".repeat(40)].clone();
    app.sender
        .send(WorkerResult::GitCommit {
            root: app.projects[0].root.clone(),
            hash: detail.hash.clone(),
            result: Ok(detail),
        })
        .unwrap();
    app.poll();
    let state = &app.projects[0].git_view;
    assert_eq!(
        state.selected_commit.as_deref(),
        Some("b".repeat(40).as_str())
    );
    assert!(state.detail_loading && state.detail.is_none());
}

#[test]
fn dismissed_or_older_diff_result_cannot_reopen_or_replace_the_current_patch() {
    let mut app = git_workspace();
    app.begin_git_diff("First file".into());
    let old = app.diff_generation;
    app.begin_git_diff("Second file".into());
    app.sender
        .send(WorkerResult::Diff {
            generation: old,
            title: "First file".into(),
            result: Ok("old patch".into()),
        })
        .unwrap();
    app.poll();
    assert_eq!(app.diff.as_ref().unwrap().0, "Second file");
    assert!(app.diff_loading);
    app.diff = None;
    app.sender
        .send(WorkerResult::Diff {
            generation: app.diff_generation,
            title: "Second file".into(),
            result: Ok("late patch".into()),
        })
        .unwrap();
    app.poll();
    assert!(app.diff.is_none());
}

#[test]
fn bookmarks_and_visible_jump_snapshots() {
    let mut app = git_workspace();
    app.sidebar = SidebarView::Explorer;
    for (index, line, column, label) in [
        (0, 11, 9, "Workspace construction"),
        (0, 18, 9, "Open a file"),
        (1, 7, 1, "Project checklist"),
    ] {
        let tab = &app.projects[0].tabs[index];
        let context = tab.text.lines().nth(line - 1).unwrap().to_owned();
        let crate::bookmarks::ToggleResult::Added(id) =
            app.bookmarks
                .toggle(tab.path.as_ref().unwrap(), line, column, Some(&context))
        else {
            panic!("distinct fixture bookmarks")
        };
        app.bookmarks.set_label(id, label);
    }
    let mut screen = visual_tests::harness(app);
    screen.run_steps(3);
    screen.state_mut().open_bookmarks();
    visual_tests::snapshot(&mut screen, "morrow_bookmarks");
    screen.state_mut().cancel_navigation();
    screen.run_steps(3);
    screen.state_mut().start_screen_jump();
    screen.run_steps(3);
    screen
        .state_mut()
        .screen_jump
        .as_mut()
        .unwrap()
        .state
        .set_query("wo");
    visual_tests::snapshot(&mut screen, "morrow_screen_jump");
    let jump = screen.state().screen_jump.as_ref().unwrap();
    assert!(jump.state.targets.len() >= 3);
    assert_eq!(
        screen.state().tab().unwrap().text,
        screen.state().tab().unwrap().saved_text
    );
    assert!(!screen.state().tab().unwrap().dirty);
}

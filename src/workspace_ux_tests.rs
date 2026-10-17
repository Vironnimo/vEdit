//! Workspace feedback and dialogs at the native minimum window size.
use super::*;
use egui_kittest::{Harness, kittest::Queryable};

fn fixture(count: usize) -> VEditApp {
    let mut app = VEditApp::empty(egui::Context::default());
    app.settings.autosave = false;
    for name in ["alpha", "beta"].into_iter().take(count) {
        let root = std::env::temp_dir().join("vedit-ux-fixture").join(name);
        let mut project = Project::new(root.clone());
        project.index = Some(Arc::new(workspace::WorkspaceIndex::empty(root.clone())));
        let mut tab = EditorTab::new(Some(root.join("src/lib.rs")), "lib.rs".into());
        tab.text = "saved text\n".into();
        tab.saved_text.clone_from(&tab.text);
        project.tabs.push(tab);
        app.projects.push(project);
    }
    app.bind_current_pane();
    app
}

fn harness(app: VEditApp) -> Harness<'static, VEditApp> {
    let mut screen = Harness::builder()
        .with_size([960.0, 640.0])
        .with_os(egui::os::OperatingSystem::Windows)
        .build_ui_state(
            |ui, app: &mut VEditApp| {
                app.ctx = ui.ctx().clone();
                app.last_disk_check = Instant::now();
                app.last_session_save = Instant::now();
                app.session_dirty = false;
                for project in &mut app.projects {
                    project.last_git_refresh = Instant::now();
                    for tab in &mut project.tabs {
                        tab.last_edit = Instant::now();
                    }
                }
                app.draw_ui(ui);
            },
            app,
        );
    set_style(&screen.ctx);
    screen.ctx.all_styles_mut(|style| {
        style.animation_time = 0.0;
        style.visuals.text_cursor.blink = false;
        style.interaction.tooltip_delay = 0.0;
    });
    screen.run_steps(3);
    screen
}

#[test]
fn dirty_project_confirmation_identifies_its_scope_and_escape_keeps_documents() {
    let mut app = fixture(2);
    for project in &mut app.projects {
        project.tabs[0].text = "unsaved text\n".into();
        project.tabs[0].dirty = true;
    }
    app.request_close(CloseAction::Project(1));
    let mut screen = harness(app);
    assert!(
        screen
            .query_by_label("1 file with unsaved changes.")
            .is_some()
    );
    assert!(screen.query_by_label_contains("beta · src").is_some());
    assert!(screen.query_by_label_contains("alpha · src").is_none());
    screen.key_press(Key::Escape);
    screen.run_steps(3);
    assert!(screen.state().close_action.is_none());
    assert_eq!(screen.state().projects.len(), 2);
    assert!(
        screen
            .state()
            .projects
            .iter()
            .all(|project| project.tabs[0].dirty)
    );
    assert_eq!(screen.state().active_project, 0);
    screen.event(egui::Event::Text("j".into()));
    screen.run_steps(2);
    assert_eq!(screen.state().tab().unwrap().text, "unsaved text\n");
    assert!(screen.state().tab().unwrap().vim.cursor > 0);
}

#[test]
fn many_unsaved_documents_keep_dialog_actions_inside_the_minimum_window() {
    let mut app = fixture(1);
    let root = app.projects[0].root.clone();
    app.projects[0].tabs = (0..64)
        .map(|index| {
            let title = format!("draft-{index}.rs");
            let mut tab = EditorTab::new(Some(root.join(&title)), title);
            tab.text = "draft".into();
            tab.dirty = true;
            tab
        })
        .collect();
    app.bind_current_pane();
    app.request_close(CloseAction::Exit);
    let screen = harness(app);
    assert!(
        screen
            .query_by_label("64 files with unsaved changes.")
            .is_some()
    );
    for label in ["Cancel", "Discard", "Save & close"] {
        let rect = screen.get_by_label(label).rect();
        assert!(
            rect.top() > 0.0 && rect.bottom() < 640.0,
            "{label}: {rect:?}"
        );
    }
}

#[test]
fn quick_open_distinguishes_indexing_failures_and_real_no_matches() {
    let mut app = fixture(1);
    app.projects[0].index = None;
    app.projects[0].indexing = true;
    app.open_palette(Palette::Files);
    let mut screen = harness(app);
    let header = screen.get_by_label("Quick open").rect();
    let loading: Vec<_> = screen
        .query_all_by_label("Indexing files…")
        .map(|node| node.rect())
        .collect();
    assert_eq!(
        loading.len(),
        1,
        "Quick Open reports its own indexing state independently of Explorer"
    );
    assert!(loading.iter().any(|rect| rect.left() >= header.left() - 1.0
        && rect.top() >= header.bottom()
        && rect.bottom() < 640.0));
    let focused = screen
        .ctx
        .memory(|memory| memory.focused())
        .expect("Quick Open focuses its query");
    let focused = screen
        .ctx
        .read_response(focused)
        .expect("the focused field is rendered")
        .rect;
    assert!(focused.left() >= header.left() - 1.0 && focused.top() > header.bottom());
    assert!(screen.query_by_label("No matches").is_none());
    let app = screen.state_mut();
    app.projects[0].indexing = false;
    app.projects[0].index_error = Some("This folder is temporarily unavailable".into());
    screen.run_steps(3);
    assert!(screen.query_by_label("Retry indexing").is_some());
    assert!(
        screen
            .query_all_by_label("This folder is temporarily unavailable")
            .any(|node| {
                let rect = node.rect();
                rect.left() >= header.left() - 1.0
                    && rect.top() > header.bottom()
                    && rect.bottom() < 640.0
            }),
        "the actual indexing error is visible inside Quick Open"
    );
    assert!(screen.query_by_label("No matches").is_none());
    let app = screen.state_mut();
    app.projects[0].index_error = None;
    app.projects[0].index = Some(Arc::new(workspace::WorkspaceIndex::empty(
        app.projects[0].root.clone(),
    )));
    app.palette_query = "missing".into();
    screen.run_steps(3);
    assert!(screen.query_by_label("No matches").is_some());
}

#[test]
fn quick_open_explains_that_a_limited_index_can_miss_files() {
    let mut app = fixture(1);
    app.sidebar_visible = false;
    let mut index = workspace::WorkspaceIndex::empty(app.projects[0].root.clone());
    index.truncated = true;
    app.projects[0].index = Some(Arc::new(index));
    app.open_palette(Palette::Files);
    app.palette_query = "missing".into();
    let screen = harness(app);
    assert!(screen.query_by_label("No matches").is_none());
    assert!(
        screen
            .query_by_label("No matches in the indexed files")
            .is_some()
    );
    let note = screen
        .get_by_label("Index limit reached. Some files may be missing.")
        .rect();
    assert!(note.top() > 0.0 && note.bottom() < 640.0, "{note:?}");
}

#[test]
fn problems_distinguish_duplicate_basenames_and_reveal_full_paths_on_hover() {
    let mut app = fixture(1);
    app.output_visible = true;
    app.problems_visible = true;
    app.projects[0].rust_language_mut().ready = true;
    let root = app.projects[0].root.clone();
    for crate_name in ["crate-a", "crate-b"] {
        app.projects[0].diagnostics.insert(
            root.join(crate_name).join("src/lib.rs"),
            vec![crate::lsp::Diagnostic {
                range: crate::lsp::TextRange::default(),
                severity: 1,
                message: "Missing item".into(),
                source: None,
            }],
        );
    }
    let mut screen = harness(app);
    assert!(screen.query_by_label_contains("crate-a").is_some());
    assert!(screen.query_by_label_contains("crate-b").is_some());
    screen.get_by_label_contains("crate-a").hover();
    screen.run_steps(5);
    let full = crate::paths::display(&root.join("crate-a").join("src/lib.rs"));
    assert!(screen.query_by_label_contains(&full).is_some());
}

#[test]
fn review_distinguishes_identical_relative_paths_across_projects() {
    let mut app = fixture(2);
    app.set_sidebar_view_visible(SidebarView::Explorer, false);
    app.show_sidebar(SidebarView::Review);
    app.review = Some(crate::cli::ReviewSummary {
        title: "Review".into(),
        note: String::new(),
        files: app
            .projects
            .iter()
            .flat_map(|project| project.tabs.iter().map(|tab| tab.id))
            .collect(),
    });
    let mut screen = harness(app);
    assert!(screen.query_by_label_contains("alpha · src").is_some());
    assert!(screen.query_by_label_contains("beta · src").is_some());
    screen.get_by_label_contains("beta · src").hover();
    screen.run_steps(5);
    let full = crate::paths::display(screen.state().projects[1].tabs[0].path.as_ref().unwrap());
    assert!(screen.query_by_label_contains(&full).is_some());
}

#[test]
fn welcome_actions_shortcuts_and_version_fit_the_minimum_window() {
    let screen = harness(fixture(0));
    for label in ["Open a project folder", "Commands at your fingertips"] {
        let rect = screen.get_by_label(label).rect();
        assert!(
            rect.top() >= 47.0 && rect.bottom() < 613.0,
            "{label}: {rect:?}"
        );
    }
    let version = format!("VEDIT  /  {}", env!("CARGO_PKG_VERSION"));
    let version_rect = screen.get_by_label(&version).rect();
    assert!(
        version_rect.bottom() < 613.0,
        "version footer: {version_rect:?}"
    );
}

#[test]
fn diff_navigation_cannot_edit_an_insert_mode_document_behind_the_window() {
    let mut screen = harness(fixture(1));
    screen.event(egui::Event::Text("i".into()));
    screen.run_steps(3);
    assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::Insert);
    let original = screen.state().tab().unwrap().text.clone();
    screen.state_mut().diff = Some((
        "Review fixture".into(),
        format!(
            "@@ -0,0 +1,1000 @@\n{}",
            (0..1000)
                .map(|index| format!("+payload {index}\n"))
                .collect::<String>(),
        ),
    ));
    screen.run_steps(3);
    screen.event(egui::Event::Text("j".into()));
    screen.run_steps(3);
    assert!(screen.state().diff.is_some());
    assert_eq!(screen.state().tab().unwrap().text, original);
    assert!(!screen.state().tab().unwrap().dirty);
    assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::Insert);
}

#[test]
fn closing_a_diff_restores_vim_keyboard_navigation_without_a_pointer_click() {
    let mut screen = harness(fixture(1));
    screen.state_mut().diff = Some(("Review fixture".into(), "@@ -1 +1 @@\n-old\n+new\n".into()));
    screen.run_steps(3);
    screen.key_press(Key::Escape);
    screen.run_steps(2);
    assert!(screen.state().diff.is_none());
    let original = screen.state().tab().unwrap().text.clone();
    screen.event(egui::Event::Text("j".into()));
    screen.run_steps(2);
    assert_eq!(screen.state().tab().unwrap().text, original);
    assert!(screen.state().tab().unwrap().vim.cursor > 0);
}

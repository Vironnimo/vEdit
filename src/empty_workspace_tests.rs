//! An explicitly selected empty workspace is a view, not a lost active tab.
use super::*;
use egui_kittest::Harness;

const SOURCE: &str = "ONLY_A_DOCUMENT_BODY\nsecond line stays intact\n";

fn fixture() -> MorrowApp {
    let mut app = MorrowApp::empty(egui::Context::default());
    app.settings.autosave = false;
    app.settings.smooth_scroll = false;
    app.sidebar_visible = false;
    for name in ["A", "B"] {
        let root = std::env::temp_dir().join(format!("morrow-intentional-empty-{name}"));
        let mut project = Project::new(root.clone());
        project.last_git_refresh = Instant::now();
        if name == "A" {
            for title in ["first.txt", "second.txt"] {
                let mut tab = EditorTab::new(Some(root.join(title)), title.into());
                tab.text = SOURCE.into();
                tab.saved_text = SOURCE.into();
                project.tabs.push(tab);
            }
        }
        app.projects.push(project);
    }
    app.bind_current_pane();
    app
}

fn harness(app: MorrowApp) -> Harness<'static, MorrowApp> {
    let mut screen = Harness::builder()
        .with_size([1280.0, 850.0])
        .with_os(egui::os::OperatingSystem::Windows)
        .build_ui_state(
            |ui, app: &mut MorrowApp| {
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
        style.visuals.text_cursor.blink = false;
        style.animation_time = 0.0;
    });
    screen.run_steps(3);
    screen
}

fn painted(screen: &Harness<'_, MorrowApp>, pane: u64, label: &str) -> bool {
    fn contains(shape: &egui::Shape, rect: egui::Rect, label: &str) -> bool {
        match shape {
            egui::Shape::Text(text) => text.galley.job.text == label && rect.contains(text.pos),
            egui::Shape::Vec(shapes) => shapes.iter().any(|shape| contains(shape, rect, label)),
            _ => false,
        }
    }
    let rect = screen.state().editor_layout.rects[&pane];
    screen
        .output()
        .shapes
        .iter()
        .any(|shape| contains(&shape.shape, rect, label))
}

fn arrangement(app: &MorrowApp) -> serde_json::Value {
    let mut layout = serde_json::to_value(app.editor_session_layout()).unwrap();
    // Rendering initializes viewport offsets and the native cursor selection.
    // This comparison checks the split tree, groups, views and saved Vim cursors.
    for pane in layout["panes"].as_array_mut().unwrap() {
        for state in pane["states"].as_array_mut().unwrap() {
            let state = state.as_object_mut().unwrap();
            state.remove("source_scroll");
            state.remove("preview_scroll");
            state.remove("selection");
        }
    }
    layout
}

#[test]
fn session_restores_empty_project_and_retains_other_projects_split_documents() {
    let mut app = fixture();
    let original = app.projects[0]
        .tabs
        .iter()
        .map(|tab| tab.id)
        .collect::<Vec<_>>();
    app.tab_mut().unwrap().vim.cursor = 12;
    app.capture_pane_navigation();
    let left = app.current_pane_id();
    let right = app.split_active(SplitOrientation::Vertical).unwrap();
    app.tab_mut().unwrap().vim.cursor = 24;
    app.capture_pane_navigation();
    app.switch_project(1);
    assert!(app.tab().is_none());
    let blank = app.current_pane_id();
    let session = app.session();
    assert!(
        session.layout.is_none(),
        "new sessions store layouts inside their projects"
    );
    let saved = serde_json::to_value(&session.projects[1].layout).unwrap();
    assert_eq!(
        saved["panes"][0]["empty_workspace"],
        serde_json::to_value(&app.projects[1].root).unwrap()
    );
    assert_eq!(saved["panes"].as_array().unwrap().len(), 1);
    assert_eq!(saved["focused"], blank);
    let mut restored = fixture();
    let entries = session
        .projects
        .into_iter()
        .enumerate()
        .map(|(index, project)| (index, project.layout))
        .collect();
    restored.restore_project_layouts(entries, session.active_project);
    let fresh = restored.projects[0]
        .tabs
        .iter()
        .map(|tab| tab.id)
        .collect::<Vec<_>>();
    assert!(fresh.iter().all(|id| !original.contains(id)));
    let mut screen = harness(restored);
    assert_eq!(screen.state().active_project, 1);
    assert_eq!(screen.state().editor_layout.focused, blank);
    assert!(screen.state().tab().is_none());
    assert_eq!(screen.state().editor_layout.panes.len(), 1);
    assert!(screen.state().current_pane_tab_ids().is_empty());
    assert!(painted(&screen, blank, "Your workspace is ready."));
    assert!(!painted(&screen, blank, SOURCE));
    screen.state_mut().switch_project(0);
    screen.run_steps(2);
    assert_eq!(screen.state().editor_layout.focused, right);
    assert_eq!(screen.state().editor_layout.panes.len(), 2);
    assert_eq!(screen.state().editor_layout.panes[0].documents, fresh);
    assert_eq!(
        screen.state().editor_layout.panes[1].documents,
        vec![fresh[0]]
    );
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 24);
    assert!(painted(&screen, right, SOURCE));
    screen.state_mut().focus_editor_pane(left).unwrap();
    screen.run_steps(2);
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 12);
    assert_eq!(screen.state().tab().unwrap().text, SOURCE);
    screen.state_mut().switch_project(1);
    screen.run_steps(2);
    assert_eq!(screen.state().active_project, 1);
    assert!(screen.state().tab().is_none());
    assert_eq!(screen.state().current_pane_id(), blank);
    assert!(screen.state().current_pane_tab_ids().is_empty());
}

#[test]
fn new_document_in_empty_project_preserves_other_projects_split_arrangement() {
    let mut app = fixture();
    let right = app.split_active(SplitOrientation::Vertical).unwrap();
    app.tab_mut().unwrap().vim.cursor = 18;
    app.capture_pane_navigation();
    let a_layout = arrangement(&app);
    app.switch_project(1);
    let blank = app.current_pane_id();
    assert_eq!(app.editor_layout.panes.len(), 1);
    assert!(app.current_pane_tab_ids().is_empty());
    app.new_file();
    let new_id = app.tab().unwrap().id;
    let text = "ONLY_B_DOCUMENT_BODY\n";
    app.tab_mut()
        .unwrap()
        .apply(editor::replace_selection("", 0..0, text));
    let mut screen = harness(app);
    assert!(painted(&screen, blank, text));
    assert!(!painted(&screen, blank, SOURCE));
    assert_eq!(screen.state().editor_layout.panes.len(), 1);
    assert_eq!(screen.state().tab().unwrap().id, new_id);
    screen.state_mut().switch_project(0);
    screen.run_steps(2);
    assert_eq!(arrangement(screen.state()), a_layout);
    assert_eq!(screen.state().current_pane_id(), right);
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 18);
    assert!(painted(&screen, right, SOURCE));
    assert!(!painted(&screen, right, text));
    screen.state_mut().switch_project(1);
    screen.run_steps(2);
    assert_eq!(screen.state().current_pane_id(), blank);
    assert_eq!(screen.state().current_pane_tab_ids(), vec![new_id]);
    assert_eq!(screen.state().tab().unwrap().id, new_id);
    assert_eq!(screen.state().projects[1].tabs[0].text, text);
    assert!(screen.state().tab().unwrap().dirty);
    assert_eq!(screen.state().projects[0].tabs.len(), 2);
    assert_eq!(screen.state().projects[1].tabs.len(), 1);
}

#[test]
fn moving_another_projects_tab_into_the_empty_project_is_rejected_without_changes() {
    let mut app = fixture();
    let tab_id = app.tab().unwrap().id;
    let left = app.current_pane_id();
    let right = app.split_active(SplitOrientation::Vertical).unwrap();
    let a_layout = serde_json::to_value(app.editor_session_layout()).unwrap();
    app.switch_project(1);
    assert!(app.tab().is_none());
    let blank = app.current_pane_id();
    let b_layout = serde_json::to_value(app.editor_session_layout()).unwrap();
    let result = app.control_layout(LayoutAction::MoveTab {
        source_pane_id: left,
        pane_id: blank,
        tab_id,
        index: 0,
    });
    assert!(result.is_err());
    assert_eq!(app.active_project, 1);
    assert!(app.tab().is_none());
    assert!(app.current_pane_tab_ids().is_empty());
    assert_eq!(
        serde_json::to_value(app.editor_session_layout()).unwrap(),
        b_layout
    );
    assert_eq!(app.projects[0].tabs.len(), 2);
    app.switch_project(0);
    assert_eq!(
        serde_json::to_value(app.editor_session_layout()).unwrap(),
        a_layout
    );
    assert_eq!(app.current_pane_id(), right);
    assert_eq!(app.tab().unwrap().id, tab_id);
    assert_eq!(app.tab().unwrap().text, SOURCE);
    assert!(!app.tab().unwrap().dirty);
}

#[test]
fn orphan_adoption_and_unrelated_document_close_preserve_the_empty_selection() {
    let mut app = fixture();
    app.switch_project(1);
    let root = app.projects[0].root.clone();
    let mut extra = EditorTab::new(Some(root.join("orphan.txt")), "orphan.txt".into());
    extra.text = "recovered document\n".into();
    extra.saved_text = extra.text.clone();
    let extra_id = extra.id;
    app.projects[0].tabs.push(extra);
    app.adopt_orphan_documents();
    assert!(app.tab().is_none());
    assert!(app.current_pane_tab_ids().is_empty());
    let blank = app.current_pane_id();
    let b_layout = serde_json::to_value(app.editor_session_layout()).unwrap();
    app.switch_project(0);
    assert!(app.current_pane_tab_ids().contains(&extra_id));
    app.switch_project(1);
    app.perform_close(CloseAction::Tab(extra_id));
    assert!(app.tab().is_none());
    assert_eq!(app.active_project, 1);
    assert_eq!(app.current_pane_id(), blank);
    assert!(app.current_pane_tab_ids().is_empty());
    assert_eq!(
        serde_json::to_value(app.editor_session_layout()).unwrap(),
        b_layout
    );
    assert_eq!(app.projects[0].tabs.len(), 2);
    app.restore_focused_pane(false);
    assert!(app.tab().is_none());
    assert_eq!(app.active_project, 1);
}

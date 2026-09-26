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

#[test]
fn session_restores_focused_empty_workspace_and_keeps_existing_group_documents() {
    let mut app = fixture();
    let original = app.projects[0]
        .tabs
        .iter()
        .map(|tab| tab.id)
        .collect::<Vec<_>>();
    app.tab_mut().unwrap().vim.cursor = 12;
    app.capture_pane_navigation();
    let right = app.split_active(SplitOrientation::Vertical).unwrap();
    app.switch_project(1);
    assert!(app.tab().is_none());
    let session = app.session();
    let saved = serde_json::to_value(session.layout).unwrap();
    assert_eq!(
        saved["panes"][1]["empty_workspace"],
        serde_json::to_value(&app.projects[1].root).unwrap()
    );
    let mut restored = fixture();
    restored.set_active_project_context(session.active_project);
    restored.restore_editor_layout(serde_json::from_value(saved).unwrap());
    let fresh = restored.projects[0]
        .tabs
        .iter()
        .map(|tab| tab.id)
        .collect::<Vec<_>>();
    assert!(fresh.iter().all(|id| !original.contains(id)));
    let mut screen = harness(restored);
    assert_eq!(screen.state().active_project, 1);
    assert_eq!(screen.state().editor_layout.focused, right);
    assert!(screen.state().tab().is_none());
    assert_eq!(screen.state().editor_layout.panes[0].documents, fresh);
    assert_eq!(
        screen.state().editor_layout.panes[1].documents,
        vec![fresh[0]]
    );
    assert!(painted(&screen, right, "Your workspace is ready."));
    assert!(!painted(&screen, right, SOURCE));
    screen.state_mut().focus_editor_pane(1).unwrap();
    screen.run_steps(2);
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 12);
    assert_eq!(screen.state().tab().unwrap().text, SOURCE);
    screen.state_mut().focus_editor_pane(right).unwrap();
    screen.run_steps(2);
    assert_eq!(screen.state().active_project, 1);
    assert!(screen.state().tab().is_none());
}

#[test]
fn inactive_empty_view_stays_blank_when_its_workspace_gets_a_document_in_another_pane() {
    let mut app = fixture();
    let right = app.split_active(SplitOrientation::Vertical).unwrap();
    app.switch_project(1);
    app.focus_editor_pane(1).unwrap();
    app.switch_project(1);
    app.new_file();
    let new_id = app.tab().unwrap().id;
    let text = "ONLY_B_DOCUMENT_BODY\n";
    app.tab_mut()
        .unwrap()
        .apply(editor::replace_selection("", 0..0, text));
    let mut screen = harness(app);
    assert!(painted(&screen, 1, text));
    assert!(!painted(&screen, right, text));
    assert!(painted(&screen, right, "Your workspace is ready."));
    assert_eq!(screen.state().tab().unwrap().id, new_id);
    screen.state_mut().focus_editor_pane(right).unwrap();
    screen.run_steps(2);
    assert!(screen.state().tab().is_none());
    assert_eq!(screen.state().active_project, 1);
    screen.event(egui::Event::Text("iMUST_NOT_EDIT".into()));
    screen.run_steps(2);
    assert_eq!(screen.state().projects[1].tabs[0].text, text);
    screen.state_mut().new_file();
    screen.run_steps(2);
    assert!(screen.state().tab().is_some());
    assert_ne!(screen.state().tab().unwrap().id, new_id);
    assert!(
        screen
            .state()
            .editor_layout
            .panes
            .iter()
            .find(|pane| pane.id == right)
            .unwrap()
            .empty_workspace
            .is_none()
    );
    assert_eq!(screen.state().projects[0].tabs.len(), 2);
    assert_eq!(screen.state().projects[1].tabs.len(), 2);
    assert_eq!(screen.state().projects[1].tabs[0].text, text);
}

#[test]
fn moving_a_tab_into_an_intentional_empty_view_replaces_the_placeholder() {
    let mut app = fixture();
    let tab_id = app.tab().unwrap().id;
    let right = app.split_active(SplitOrientation::Vertical).unwrap();
    app.switch_project(1);
    assert!(app.tab().is_none());
    app.control_layout(LayoutAction::MoveTab {
        source_pane_id: 1,
        pane_id: right,
        tab_id,
        index: 0,
    })
    .unwrap();
    assert_eq!(app.tab().unwrap().id, tab_id);
    assert_eq!(app.active_project, 0);
    assert!(
        app.editor_layout
            .panes
            .iter()
            .find(|pane| pane.id == right)
            .unwrap()
            .empty_workspace
            .is_none()
    );
    assert_eq!(app.projects[0].tabs.len(), 2);
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
    assert!(app.current_pane_tab_ids().contains(&extra_id));
    app.perform_close(CloseAction::Tab(extra_id));
    assert!(app.tab().is_none());
    assert_eq!(app.active_project, 1);
    assert_eq!(app.projects[0].tabs.len(), 2);
    app.restore_focused_pane(false);
    assert!(app.tab().is_none());
    assert_eq!(app.active_project, 1);
}

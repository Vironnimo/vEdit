//! Regressions for project identity across split focus, closing and restoration.

use super::*;
use egui_kittest::{Harness, kittest::Queryable};

const DOCUMENT: &str = "alpha 🦀 document\nsecond line\nproject context stays separate\n";

fn fixture() -> MorrowApp {
    let mut app = MorrowApp::empty(egui::Context::default());
    app.settings.autosave = false;
    app.settings.smooth_scroll = false;
    for name in ["A", "B"] {
        let root = std::env::temp_dir().join(format!("morrow-project-context-{name}"));
        let mut project = Project::new(root.clone());
        project.index = Some(Arc::new(workspace::WorkspaceIndex::empty(root.clone())));
        project.last_git_refresh = Instant::now();
        let mut tab = EditorTab::new(Some(root.join("notes.txt")), format!("{name}.txt"));
        tab.text = DOCUMENT.into();
        tab.saved_text = DOCUMENT.into();
        project.tabs.push(tab);
        app.projects.push(project);
    }
    app.bind_current_pane();
    app
}

fn old_search(app: &mut MorrowApp) -> (PathBuf, Arc<AtomicBool>, u64) {
    let root = app.project().unwrap().root.clone();
    app.project_query = "needle".into();
    app.project_search = Some(workspace::SearchResults {
        matches: vec![workspace::SearchMatch {
            path: root.join("only-in-A.txt"),
            line: 3,
            column: 2,
            start_char: 4,
            end_char: 10,
            preview: "old project match".into(),
        }],
        files_searched: 1,
        ..Default::default()
    });
    app.search_running = true;
    app.search_dirty = false;
    app.search_generation = 17;
    app.search_last_edit = Instant::now();
    app.search_cancel = Arc::new(AtomicBool::new(false));
    (root, app.search_cancel.clone(), app.search_generation)
}

fn assert_search_context_reset(app: &MorrowApp, cancel: &AtomicBool, generation: u64) {
    assert!(app.project_search.is_none());
    assert!(!app.search_running);
    assert!(cancel.load(Ordering::Relaxed));
    assert!(app.search_generation > generation);
    assert_eq!(app.project_query, "needle");
    assert!(
        app.search_dirty,
        "the retained query must run against the new root"
    );
}

fn stale_result(app: &mut MorrowApp, root: PathBuf, generation: u64) {
    app.sender
        .send(WorkerResult::Search {
            root,
            generation,
            query: app.project_query.clone(),
            excludes: app.settings.search_excludes.clone(),
            result: Ok(workspace::SearchResults {
                files_searched: 99,
                ..Default::default()
            }),
        })
        .unwrap();
    app.poll();
    assert!(
        app.project_search.is_none(),
        "stale results must not refill the sidebar"
    );
    assert!(!app.search_running);
}

#[test]
fn switching_project_restores_its_splits_and_invalidates_cached_and_running_search() {
    let mut app = fixture();
    let left = app.current_pane_id();
    let right = app
        .split_active(crate::cli::SplitOrientation::Vertical)
        .unwrap();
    let (root, cancel, generation) = old_search(&mut app);
    app.switch_project(1);
    assert_eq!(app.active_project, 1);
    assert_search_context_reset(&app, &cancel, generation);
    stale_result(&mut app, root, generation);
    assert!(
        app.control_layout(crate::cli::LayoutAction::Focus { pane_id: left })
            .is_err()
    );
    assert!(
        app.control_layout(crate::cli::LayoutAction::Focus { pane_id: right })
            .is_err()
    );
    app.switch_project(0);
    app.control_layout(crate::cli::LayoutAction::Focus { pane_id: left })
        .unwrap();
    app.control_layout(crate::cli::LayoutAction::Focus { pane_id: right })
        .unwrap();
    assert_eq!(app.active_project, 0);
    for project in &app.projects {
        assert_eq!(project.tabs[0].text, DOCUMENT);
        assert!(!project.tabs[0].dirty);
        assert!(!project.tabs[0].loading);
    }
}

#[test]
fn closing_the_active_project_resets_search_even_when_the_numeric_index_stays_zero() {
    let mut app = fixture();
    let surviving_root = app.projects[1].root.clone();
    let surviving_tab = app.projects[1].tabs[0].id;
    let (root, cancel, generation) = old_search(&mut app);
    app.request_close(CloseAction::Project(0));
    assert_eq!(app.projects.len(), 1);
    assert_eq!(app.active_project, 0);
    assert_eq!(app.project().unwrap().root, surviving_root);
    assert_eq!(app.tab().unwrap().id, surviving_tab);
    assert_search_context_reset(&app, &cancel, generation);
    stale_result(&mut app, root, generation);
    assert_eq!(app.tab().unwrap().text, DOCUMENT);
    assert!(!app.tab().unwrap().dirty);
}

fn harness(app: MorrowApp) -> Harness<'static, MorrowApp> {
    let mut screen = Harness::builder()
        .with_size([1280.0, 820.0])
        .with_os(egui::os::OperatingSystem::Windows)
        .build_ui_state(
            |ui, app: &mut MorrowApp| {
                app.ctx = ui.ctx().clone();
                app.last_disk_check = Instant::now();
                app.last_session_save = Instant::now();
                app.search_last_edit = Instant::now();
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
        style.scroll_animation = egui::style::ScrollAnimation::none();
    });
    screen.run_steps(3);
    screen
}

fn shortcut(screen: &mut Harness<'_, MorrowApp>, key: Key, modifiers: egui::Modifiers) {
    let modifiers = egui::Modifiers {
        command: modifiers.ctrl,
        ..modifiers
    };
    screen.event(egui::Event::ModifiersChanged(modifiers));
    screen.event(egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    });
    screen.run_steps(1);
    screen.event(egui::Event::Key {
        key,
        physical_key: None,
        pressed: false,
        repeat: false,
        modifiers,
    });
    screen.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    screen.run_steps(3);
}

fn assert_search_focus_is_one_shot(screen: &mut Harness<'_, MorrowApp>, trigger: &str) {
    assert!(screen.state().sidebar == SidebarView::Search, "{trigger}");
    assert_eq!(
        screen.ctx.memory(|memory| memory.focused()),
        Some(egui::Id::new("project_search_query")),
        "{trigger} must focus the project query"
    );
    assert!(!screen.state().project_search_focus, "{trigger}");
    screen.event(egui::Event::Text("needle".into()));
    screen.run_steps(3);
    assert_eq!(screen.state().project_query, "needle", "{trigger}");
    assert_eq!(screen.state().tab().unwrap().text, DOCUMENT, "{trigger}");
    let point = screen.state().editor_rect.unwrap().center();
    screen.hover_at(point);
    screen.event(egui::Event::PointerButton {
        pos: point,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    screen.run_steps(1);
    screen.event(egui::Event::PointerButton {
        pos: point,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    screen.run_steps(3);
    let document = screen
        .state()
        .document_widget_id(screen.state().tab().unwrap().id);
    assert_eq!(
        screen.ctx.memory(|memory| memory.focused()),
        Some(document),
        "{trigger} must not reclaim query focus after returning to the editor"
    );
    screen.event(egui::Event::Text("h".into()));
    screen.run_steps(3);
    assert_eq!(screen.state().project_query, "needle", "{trigger}");
    assert_eq!(screen.state().tab().unwrap().text, DOCUMENT, "{trigger}");
    assert!(!screen.state().tab().unwrap().dirty);
}

#[test]
fn search_shortcut_focuses_the_query_and_then_allows_editor_focus() {
    let mut screen = harness(fixture());
    shortcut(
        &mut screen,
        Key::F,
        egui::Modifiers::CTRL.plus(egui::Modifiers::SHIFT),
    );
    assert_search_focus_is_one_shot(&mut screen, "Ctrl+Shift+F");
}

#[test]
fn view_menu_search_focuses_the_query_and_then_allows_editor_focus() {
    let mut screen = harness(fixture());
    screen.get_by_label("View").click();
    screen.run_steps(3);
    screen.get_by_label("Search project Ctrl+Shift+F").click();
    screen.run_steps(3);
    assert_search_focus_is_one_shot(&mut screen, "View menu");
}

#[test]
fn command_palette_search_focuses_the_query_and_then_allows_editor_focus() {
    let mut screen = harness(fixture());
    shortcut(
        &mut screen,
        Key::P,
        egui::Modifiers::CTRL.plus(egui::Modifiers::SHIFT),
    );
    assert!(screen.state().palette == Some(Palette::Commands));
    screen.event(egui::Event::Text("Search project".into()));
    screen.run_steps(3);
    assert_eq!(screen.state().palette_query, "Search project");
    screen.key_press(Key::Enter);
    screen.run_steps(4);
    assert!(screen.state().palette.is_none());
    assert_search_focus_is_one_shot(&mut screen, "Command palette");
}

#[test]
fn selecting_an_empty_project_keeps_it_active_and_preserves_the_other_buffer() {
    let mut app = fixture();
    app.projects[1].tabs.clear();
    let root = app.projects[1].root.clone();
    let existing = app.projects[0].tabs[0].id;
    let mut screen = harness(app);
    screen.get_by_label("Switch project").click();
    screen.run_steps(3);
    screen
        .get_by_label_contains("morrow-project-context-B · 0 open")
        .click();
    screen.run_steps(4);
    assert_eq!(screen.state().active_project, 1);
    assert_eq!(screen.state().project().unwrap().root, root);
    assert!(screen.state().tab().is_none());
    assert!(screen.query_by_label("Your workspace is ready.").is_some());
    assert_eq!(screen.state().projects[0].tabs[0].id, existing);
    assert_eq!(screen.state().projects[0].tabs[0].text, DOCUMENT);
    assert!(!screen.state().projects[0].tabs[0].dirty);
    screen.get_by_label("Switch project").click();
    screen.run_steps(3);
    screen
        .get_by_label_contains("morrow-project-context-A · 1 open")
        .click();
    screen.run_steps(3);
    assert_eq!(screen.state().active_project, 0);
    assert_eq!(screen.state().tab().unwrap().id, existing);
    assert_eq!(screen.state().tab().unwrap().text, DOCUMENT);
}

struct SessionFixture(PathBuf);

impl SessionFixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "morrow-workspace-session-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        for name in ["B", "C"] {
            fs::create_dir(root.join(name)).unwrap();
            fs::write(root.join(name).join("notes.md"), DOCUMENT).unwrap();
        }
        Self(root)
    }

    fn project(&self, name: &str) -> SessionProject {
        let root = self.0.join(name);
        SessionProject {
            root: root.clone(),
            tabs: vec![SessionTab {
                path: Some(root.join("notes.md")),
                title: format!("{name}.md"),
                ..Default::default()
            }],
            active_tab_index: Some(0),
            ..Default::default()
        }
    }
}

impl Drop for SessionFixture {
    fn drop(&mut self) {
        for name in ["B", "C"] {
            let _ = fs::remove_file(self.0.join(name).join("notes.md"));
            let _ = fs::remove_dir(self.0.join(name));
        }
        let _ = fs::remove_dir(&self.0);
    }
}

#[test]
fn missing_first_project_restores_the_correct_focused_project_groups_and_views() {
    let files = SessionFixture::new();
    assert!(!files.0.join("A").exists());
    let mut app = MorrowApp::empty(egui::Context::default());
    app.settings.autosave = false;
    let saved: split::SessionLayout = serde_json::from_value(serde_json::json!({
        "root": {
            "kind":"split", "orientation":"vertical", "ratio":0.5,
            "first":{"kind":"pane","id":2}, "second":{"kind":"pane","id":3}
        },
        "focused":2,
        "panes":[
            {"id":2,"project":1,"tab":0,"view":"source","cursor":8,
             "documents":[[0,0],[1,0]],
             "states":[
                 {"project":0,"tab":0,"view":"source","cursor":2,"selection":{"start":2,"end":2}},
                 {"project":1,"tab":0,"view":"source","cursor":8,"selection":{"start":8,"end":8}}
             ]},
            {"id":3,"project":2,"tab":0,"view":"preview","cursor":17,
             "documents":[[2,0]],
             "states":[
                 {"project":2,"tab":0,"view":"preview","cursor":17,"selection":{"start":17,"end":17}}
             ]}
        ]
    })).unwrap();
    let mut remapped_layout = saved.clone();
    remapped_layout.remap_projects(&[None, Some(0), Some(1)]);
    let remapped = serde_json::to_value(&remapped_layout).unwrap();
    assert_eq!(remapped["panes"][0]["project"], 0);
    assert_eq!(
        remapped["panes"][0]["documents"],
        serde_json::json!([[0, 0]])
    );
    assert_eq!(remapped["panes"][0]["states"].as_array().unwrap().len(), 1);
    assert_eq!(remapped["panes"][1]["project"], 1);
    assert_eq!(remapped["panes"][1]["states"][0]["project"], 1);
    // Use the same entry point as actual startup. The test must catch an old
    // active index 1 being clamped to the new vector and accidentally selecting C.
    app.restore_session_workspace(
        vec![files.project("A"), files.project("B"), files.project("C")],
        1,
        Some(saved),
    );
    assert_eq!(app.projects.len(), 2);
    assert_eq!(app.projects[0].root, files.0.join("B"));
    assert_eq!(app.projects[1].root, files.0.join("C"));
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.projects.iter().any(|project| project.tabs[0].loading) {
        assert!(
            Instant::now() < deadline,
            "restored project files did not load"
        );
        std::thread::sleep(Duration::from_millis(2));
        app.poll();
    }
    let mut screen = harness(app);
    assert_eq!(screen.state().active_project, 0);
    assert_eq!(screen.state().project().unwrap().root, files.0.join("B"));
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 8);
    assert_eq!(screen.state().tab().unwrap().text, DOCUMENT);
    let restored = serde_json::to_value(screen.state().editor_session_layout()).unwrap();
    assert_eq!(restored["focused"], 2);
    assert_eq!(restored["panes"].as_array().unwrap().len(), 1);
    assert_eq!(restored["panes"][0]["project"], 0);
    assert_eq!(restored["panes"][0]["view"], "source");
    assert_eq!(restored["panes"][0]["cursor"], 8);
    assert_eq!(
        restored["panes"][0]["documents"],
        serde_json::json!([[0, 0]])
    );
    screen.state_mut().switch_project(1);
    screen.run_steps(2);
    let other = serde_json::to_value(screen.state().editor_session_layout()).unwrap();
    assert_eq!(other["focused"], 3);
    assert_eq!(other["panes"].as_array().unwrap().len(), 1);
    assert_eq!(other["panes"][0]["project"], 1);
    assert_eq!(other["panes"][0]["view"], "preview");
    assert_eq!(other["panes"][0]["cursor"], 17);
    assert_eq!(other["panes"][0]["documents"], serde_json::json!([[1, 0]]));
    assert!(screen.state().projects.iter().all(|project| {
        project
            .tabs
            .iter()
            .all(|tab| !tab.dirty && !tab.loading && !tab.saving)
    }));
}

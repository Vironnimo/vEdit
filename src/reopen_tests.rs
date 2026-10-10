use super::*;
use crate::cli::{DocumentView, SplitOrientation};
use egui_kittest::{Harness, kittest::Queryable};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "vedit-reopen-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("A")).unwrap();
        fs::create_dir_all(root.join("B")).unwrap();
        Self(fs::canonicalize(root).unwrap())
    }

    fn app(&self) -> VEditApp {
        let mut app = VEditApp::empty(egui::Context::default());
        app.settings.autosave = false;
        app.recovery_session_path = self.0.join("session.json");
        for name in ["A", "B"] {
            let mut project = Project::new(self.0.join(name));
            project.last_git_refresh = Instant::now();
            app.projects.push(project);
        }
        app.activate_tab(0, 0);
        app
    }

    fn open(&self, app: &mut VEditApp, name: &str) -> u64 {
        let path = app.project().unwrap().root.join(name);
        fs::write(&path, "# α🦀\n\nSaved text\n").unwrap();
        app.open_file(path);
        loaded(app);
        let tab = app.tab_mut().unwrap();
        tab.recovery_path = self.0.join(format!("recovery-{}.json", tab.id));
        tab.id
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let temporary = fs::canonicalize(std::env::temp_dir()).unwrap();
        assert!(self.0.starts_with(temporary));
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn loaded(app: &mut VEditApp) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.tab().is_some_and(|tab| tab.loading) {
        assert!(Instant::now() < deadline, "file did not load");
        app.poll();
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn reopening_reads_current_disk_and_restores_unicode_cursor_and_preview() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    let id = fixture.open(&mut app, "plan.md");
    app.tab_mut().unwrap().set_cursor(7..7);
    app.set_document_view(DocumentView::Preview);
    app.request_close(CloseAction::Tab(id));
    assert!(app.tab().is_none());
    let path = fixture.0.join("A/plan.md");
    fs::write(&path, "# α🦀\n\nChanged on disk\n").unwrap();
    app.command_action("Reopen closed tab");
    loaded(&mut app);
    let tab = app.tab().unwrap();
    assert_ne!(tab.id, id);
    assert_eq!(tab.text, "# α🦀\n\nChanged on disk\n");
    assert_eq!(tab.vim.cursor, 7);
    assert_eq!(tab.agent_view, DocumentView::Preview);
    assert!(!tab.dirty);
    assert!(!app.can_reopen_closed_tab());
}

#[test]
fn cancelled_close_does_not_record_and_discard_only_reopens_saved_text() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    let id = fixture.open(&mut app, "source.txt");
    let tab = app.tab_mut().unwrap();
    tab.text.push_str("discarded edits");
    tab.changed();
    tab.set_cursor(100..100);
    app.request_close(CloseAction::Tab(id));
    assert!(app.close_action.is_some());
    assert!(!app.can_reopen_closed_tab());
    assert!(app.tab().unwrap().dirty);
    app.perform_close(CloseAction::Tab(id));
    app.reopen_closed_tab();
    loaded(&mut app);
    assert_eq!(app.tab().unwrap().text, "# α🦀\n\nSaved text\n");
    assert!(!app.tab().unwrap().dirty);
    assert_eq!(
        app.tab().unwrap().vim.cursor,
        app.tab().unwrap().text.chars().count()
    );

    app.new_file();
    let untitled = app.tab().unwrap().id;
    app.perform_close(CloseAction::Tab(untitled));
    assert!(!app.can_reopen_closed_tab());
}

#[test]
fn histories_stay_with_their_project_and_pop_in_reverse_close_order() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    for name in ["first.txt", "second.txt"] {
        let id = fixture.open(&mut app, name);
        app.request_close(CloseAction::Tab(id));
    }
    app.activate_tab(1, 0);
    assert!(!app.can_reopen_closed_tab());
    app.reopen_closed_tab();
    assert!(app.tab().is_none());
    let b = fixture.open(&mut app, "project-b.txt");
    app.request_close(CloseAction::Tab(b));
    app.activate_tab(0, 0);
    app.reopen_closed_tab();
    loaded(&mut app);
    assert_eq!(app.tab().unwrap().title, "second.txt");
    app.reopen_closed_tab();
    loaded(&mut app);
    assert_eq!(app.tab().unwrap().title, "first.txt");
    assert!(!app.can_reopen_closed_tab());
    app.activate_tab(1, 0);
    app.reopen_closed_tab();
    loaded(&mut app);
    assert_eq!(app.tab().unwrap().title, "project-b.txt");
}

#[test]
fn closing_a_shared_view_reuses_the_dirty_buffer_and_current_pane() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    let id = fixture.open(&mut app, "shared.txt");
    let left = app.current_pane_id();
    let right = app.split_active(SplitOrientation::Vertical).unwrap();
    fixture.open(&mut app, "keep-right.txt");
    app.activate_tab(0, 0);
    let tab = app.tab_mut().unwrap();
    tab.text.push_str("unsaved but still open");
    tab.changed();
    tab.set_cursor(9..9);
    app.request_close(CloseAction::PaneTab { pane_id: right, id });
    assert!(app.close_action.is_none());
    assert!(app.pane_contains_tab(left, id));
    assert!(!app.pane_contains_tab(right, id));
    app.reopen_closed_tab();
    assert_eq!(app.current_pane_id(), right);
    assert!(app.pane_contains_tab(right, id));
    assert_eq!(app.projects[0].tabs.len(), 2);
    let tab = app.tab().unwrap();
    assert_eq!(tab.id, id);
    assert_eq!(tab.vim.cursor, 9);
    assert!(tab.dirty);
    assert!(tab.text.ends_with("unsaved but still open"));
}

#[test]
fn missing_closed_file_reports_load_error_and_older_files_remain_reopenable() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    for name in ["older.txt", "missing.txt"] {
        let id = fixture.open(&mut app, name);
        app.request_close(CloseAction::Tab(id));
    }
    fs::remove_file(fixture.0.join("A/missing.txt")).unwrap();
    app.reopen_closed_tab();
    loaded(&mut app);
    assert!(app.tab().unwrap().error.is_some());
    assert!(!fixture.0.join("A/missing.txt").exists());
    app.reopen_closed_tab();
    loaded(&mut app);
    assert_eq!(app.tab().unwrap().title, "older.txt");
    assert!(app.tab().unwrap().error.is_none());
}

#[test]
fn close_history_is_bounded_and_deduplicates_reclosed_files() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    for i in 0..23 {
        let id = fixture.open(&mut app, &format!("file-{i}.txt"));
        app.request_close(CloseAction::Tab(id));
    }
    assert_eq!(app.projects[0].closed_tabs.len(), 20);
    assert_eq!(
        app.projects[0].closed_tabs[0].path.file_name().unwrap(),
        "file-3.txt"
    );
    let id = fixture.open(&mut app, "file-22.txt");
    app.request_close(CloseAction::Tab(id));
    assert_eq!(app.projects[0].closed_tabs.len(), 20);
}

#[test]
fn keyboard_and_file_menu_reopen_saved_documents() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    let id = fixture.open(&mut app, "keyboard.txt");
    app.request_close(CloseAction::Tab(id));
    let mut screen = Harness::builder()
        .with_size([1000.0, 640.0])
        .with_os(egui::os::OperatingSystem::Windows)
        .build_ui_state(
            |ui, app: &mut VEditApp| {
                app.ctx = ui.ctx().clone();
                app.poll();
                app.draw_ui(ui);
            },
            app,
        );
    screen.run_steps(2);
    screen.key_press_modifiers(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, Key::T);
    screen.run_steps(2);
    loaded(screen.state_mut());
    assert_eq!(screen.state().tab().unwrap().title, "keyboard.txt");
    let id = screen.state().tab().unwrap().id;
    screen.state_mut().request_close(CloseAction::Tab(id));
    screen.run_steps(2);
    screen.get_by_label("Main menu").click();
    screen.run_steps(2);
    screen.get_by_label("File").click();
    screen.run_steps(2);
    screen.get_by_label_contains("Reopen closed tab").click();
    screen.run_steps(2);
    loaded(screen.state_mut());
    assert_eq!(screen.state().tab().unwrap().title, "keyboard.txt");
}

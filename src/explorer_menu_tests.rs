//! Pointer regressions for empty Explorer space and existing item menus.

use super::*;
use egui_kittest::{Harness, kittest::Queryable};

struct Fixture(PathBuf);

impl Fixture {
    fn new(with_files: bool) -> (Self, MorrowApp) {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "morrow-explorer-context-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        if with_files {
            fs::create_dir(root.join("src")).unwrap();
            fs::write(root.join("notes.txt"), "existing document").unwrap();
            fs::write(root.join("src/lib.txt"), "nested document").unwrap();
        }
        let root = fs::canonicalize(root).unwrap();
        let mut app = MorrowApp::empty(egui::Context::default());
        app.settings.autosave = false;
        let mut project = Project::new(root.clone());
        project.last_git_refresh = Instant::now();
        project.index = Some(Arc::new(
            workspace::build_index(&root, Default::default()).unwrap(),
        ));
        app.projects.push(project);
        (Self(root), app)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for path in ["notes.txt", "src/lib.txt", "Plan ü.md"] {
            let _ = fs::remove_file(self.0.join(path));
        }
        for path in ["src", "New folder"] {
            let _ = fs::remove_dir(self.0.join(path));
        }
        let _ = fs::remove_dir(&self.0);
    }
}

fn harness(app: MorrowApp) -> Harness<'static, MorrowApp> {
    let mut screen = Harness::builder()
        .with_size([520.0, 520.0])
        .with_step_dt(1.0 / 60.0)
        .with_os(egui::os::OperatingSystem::Windows)
        .build_ui_state(
            |ui, app: &mut MorrowApp| {
                app.ctx = ui.ctx().clone();
                app.explorer(ui);
                let ctx = ui.ctx().clone();
                app.context_file_dialog_ui(&ctx);
            },
            app,
        );
    screen.run_steps(3);
    screen
}

fn right_click(screen: &mut Harness<'_, MorrowApp>, point: egui::Pos2) {
    screen.hover_at(point);
    screen.event(egui::Event::PointerButton {
        pos: point,
        button: egui::PointerButton::Secondary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    screen.run_steps(1);
    screen.event(egui::Event::PointerButton {
        pos: point,
        button: egui::PointerButton::Secondary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    screen.run_steps(3);
}

fn assert_workspace_menu(screen: &Harness<'_, MorrowApp>) {
    for label in [
        "New file…",
        "New folder…",
        "Copy project path",
        "Reveal in Explorer",
        "Refresh",
    ] {
        assert!(screen.query_by_label(label).is_some(), "missing {label}");
    }
    assert!(screen.query_by_label("Rename…").is_none());
    assert!(screen.query_by_label("Delete").is_none());
    assert!(screen.query_by_label("Open").is_none());
}

#[test]
fn empty_workspace_background_opens_root_creation_menu() {
    let (_fixture, app) = Fixture::new(false);
    let mut screen = harness(app);
    right_click(&mut screen, egui::pos2(100.0, 350.0));
    assert_workspace_menu(&screen);
}

#[test]
fn initial_index_loading_and_error_leave_background_menu_available() {
    for indexing in [true, false] {
        let (_fixture, mut app) = Fixture::new(false);
        app.projects[0].index = None;
        app.projects[0].indexing = indexing;
        app.projects[0].index_error = (!indexing).then(|| "Index unavailable".into());
        let mut screen = harness(app);
        right_click(&mut screen, egui::pos2(100.0, 350.0));
        assert_workspace_menu(&screen);
    }
}

#[test]
fn collapsed_root_keeps_background_creation_menu() {
    let (_fixture, app) = Fixture::new(true);
    let root_title = file_title(&app.projects[0].root);
    let mut screen = harness(app);
    screen.get_by_label(&root_title).click();
    screen.run_steps(2);
    assert!(screen.query_by_label("notes.txt").is_none());
    right_click(&mut screen, egui::pos2(100.0, 350.0));
    assert_workspace_menu(&screen);
}

#[test]
fn background_file_and_folder_dialogs_target_the_project_root() {
    for (label, operation) in [
        ("New file…", FileOperation::NewFile),
        ("New folder…", FileOperation::NewFolder),
    ] {
        let (fixture, app) = Fixture::new(true);
        let mut screen = harness(app);
        right_click(&mut screen, egui::pos2(100.0, 350.0));
        screen.get_by_label(label).click();
        screen.run_steps(2);
        let dialog = screen.state().context_file_dialog.as_ref().unwrap();
        assert_eq!(dialog.operation, operation);
        assert_eq!(dialog.path, fixture.0);
        assert!(dialog.name.is_empty());
        assert!(!dialog.busy);
    }
}

#[test]
fn background_does_not_replace_file_or_folder_context_menu() {
    for (label, is_dir) in [("notes.txt", false), ("src", true)] {
        let (fixture, app) = Fixture::new(true);
        let mut screen = harness(app);
        let point = screen.get_by_label(label).rect().center();
        right_click(&mut screen, point);
        assert!(screen.query_by_label("Rename…").is_some());
        assert!(screen.query_by_label("Copy relative path").is_some());
        assert!(screen.query_by_label("Copy project path").is_none());
        assert_eq!(screen.query_by_label("Open").is_some(), !is_dir);
        screen.get_by_label("New file…").click();
        screen.run_steps(2);
        let dialog = screen.state().context_file_dialog.as_ref().unwrap();
        assert_eq!(dialog.operation, FileOperation::NewFile);
        assert_eq!(
            dialog.path,
            if is_dir {
                fixture.0.join("src")
            } else {
                fixture.0.clone()
            }
        );
    }
}

#[test]
fn background_copy_path_uses_a_normal_windows_path() {
    let (fixture, app) = Fixture::new(false);
    let mut screen = harness(app);
    right_click(&mut screen, egui::pos2(100.0, 350.0));
    screen.get_by_label("Copy project path").click();
    screen.run_steps(1);
    assert!(screen.output().platform_output.commands.iter().any(|command| {
        matches!(command,
            egui::OutputCommand::CopyText(text) if text == &crate::paths::display(&fixture.0)
                && !text.starts_with("\\\\?\\")
        )
    }));
}

#[test]
fn background_refresh_starts_a_new_index_generation() {
    let (_fixture, app) = Fixture::new(false);
    let generation = app.projects[0].index_generation;
    let mut screen = harness(app);
    right_click(&mut screen, egui::pos2(100.0, 350.0));
    screen.get_by_label("Refresh").click();
    screen.run_steps(2);
    assert!(screen.state().projects[0].indexing);
    assert_eq!(screen.state().projects[0].index_generation, generation + 1);
}

#[test]
fn creation_from_background_reindexes_and_opens_only_new_files() {
    for (label, name, is_file) in [
        ("New file…", "Plan ü.md", true),
        ("New folder…", "New folder", false),
    ] {
        let (fixture, app) = Fixture::new(false);
        let generation = app.projects[0].index_generation;
        let mut screen = harness(app);
        right_click(&mut screen, egui::pos2(100.0, 350.0));
        screen.get_by_label(label).click();
        screen.run_steps(2);
        screen
            .state_mut()
            .context_file_dialog
            .as_mut()
            .unwrap()
            .name = name.into();
        screen.get_by_label("Create").click();
        screen.run_steps(1);
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            screen.state_mut().poll();
            let project = &screen.state().projects[0];
            if screen.state().context_file_dialog.is_none()
                && !project.indexing
                && project.tabs.iter().all(|tab| !tab.loading)
            {
                break;
            }
            assert!(Instant::now() < deadline, "creation did not finish");
            std::thread::sleep(Duration::from_millis(5));
        }
        let project = &screen.state().projects[0];
        assert_eq!(project.index_generation, generation + 1);
        let index = project.index.as_ref().unwrap();
        assert!(
            index
                .entries
                .iter()
                .any(|entry| entry.path == fixture.0.join(name))
        );
        assert_eq!(project.tabs.len(), usize::from(is_file));
        if is_file {
            assert_eq!(project.tabs[0].path.as_ref(), Some(&fixture.0.join(name)));
            assert!(!project.tabs[0].dirty);
        } else {
            assert!(fixture.0.join(name).is_dir());
        }
    }
}

//! F8 problem navigation, the pane-local back/forward history and output
//! locations.

use super::*;
use crate::lsp::{Diagnostic, Position, TextRange};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "vedit-navigation-history-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let lines = |name: &str| {
            (1..=40)
                .map(|line| format!("{name} line {line}\n"))
                .collect::<String>()
        };
        fs::write(root.join("a.rs"), lines("a")).unwrap();
        fs::write(root.join("b.rs"), lines("b")).unwrap();
        Self(fs::canonicalize(root).unwrap())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn app(root: &Path) -> VEditApp {
    let mut app = VEditApp::empty(egui::Context::default());
    app.settings.autosave = false;
    let mut project = Project::new(root.to_path_buf());
    project.indexing = false;
    app.projects.push(project);
    app
}

/// Let background loads finish and apply the pending destination, as the
/// frame loop does.
fn settle(app: &mut VEditApp) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        app.poll();
        app.apply_pending_location();
        app.apply_pending_jump();
        let loading = app.projects[0].tabs.iter().any(|tab| tab.loading);
        if !loading && app.pending_location.is_none() && app.pending_jump.is_none() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "documents did not finish loading"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    app.observe_navigation();
}

fn diagnostic(line: u32, character: u32, severity: u8, message: &str) -> Diagnostic {
    let position = Position { line, character };
    Diagnostic {
        range: TextRange {
            start: position,
            end: position,
        },
        severity,
        message: message.into(),
        source: None,
    }
}

/// The active document's file name and one-based cursor line and column.
fn location(app: &VEditApp) -> (String, usize, usize) {
    let tab = app.tab().unwrap();
    let (line, column) = tab.lines.line_col(&tab.text, tab.revision, tab.vim.cursor);
    let name = tab.path.as_ref().unwrap().file_name().unwrap();
    (name.to_string_lossy().into_owned(), line, column)
}

fn status(app: &VEditApp) -> &str {
    app.status.as_ref().map_or("", |(text, _, _)| text.as_str())
}

fn move_to_line(app: &mut VEditApp, line: usize) {
    let tab = app.tab_mut().unwrap();
    let offset: usize = tab
        .text
        .split_inclusive('\n')
        .take(line - 1)
        .map(|text| text.chars().count())
        .sum();
    tab.set_cursor(offset..offset);
    app.observe_navigation();
}

#[test]
fn f8_finishes_the_active_file_then_visits_the_next_file_and_skips_hints() {
    let fixture = Fixture::new();
    let root = fixture.0.clone();
    let mut app = app(&root);
    app.projects[0].diagnostics.insert(
        root.join("a.rs"),
        vec![
            diagnostic(1, 0, 4, "a hint is not a stop"),
            diagnostic(5, 2, 1, "an error"),
            diagnostic(9, 0, 3, "information is not a stop"),
        ],
    );
    app.projects[0]
        .diagnostics
        .insert(root.join("b.rs"), vec![diagnostic(20, 4, 2, "a warning")]);
    app.open_file(root.join("a.rs"));
    settle(&mut app);

    app.go_to_problem(false);
    settle(&mut app);
    assert_eq!(location(&app), ("a.rs".into(), 6, 3));
    assert!(app.tab().unwrap().pending_navigation_feedback || app.tab().unwrap().focus);

    app.go_to_problem(false);
    settle(&mut app);
    assert_eq!(location(&app), ("b.rs".into(), 21, 5));

    app.go_to_problem(false);
    settle(&mut app);
    assert_eq!(location(&app), ("a.rs".into(), 6, 3), "the search wraps");

    app.go_to_problem(true);
    settle(&mut app);
    assert_eq!(location(&app), ("b.rs".into(), 21, 5));

    // Every problem jump is a history entry.
    app.navigate_history(true);
    settle(&mut app);
    assert_eq!(location(&app), ("a.rs".into(), 6, 3));

    let list = app.problem_list();
    assert_eq!(list.counted(), 3, "hints are not counted");
}

#[test]
fn f8_without_errors_or_warnings_says_so_and_stays() {
    let fixture = Fixture::new();
    let root = fixture.0.clone();
    let mut app = app(&root);
    app.projects[0]
        .diagnostics
        .insert(root.join("a.rs"), vec![diagnostic(3, 0, 4, "only a hint")]);
    app.open_file(root.join("a.rs"));
    settle(&mut app);
    app.go_to_problem(false);
    settle(&mut app);
    assert_eq!(location(&app), ("a.rs".into(), 1, 1));
    assert_eq!(status(&app), "No errors or warnings");
}

#[test]
fn back_and_forward_follow_switches_and_large_moves_and_reopen_a_closed_document() {
    let fixture = Fixture::new();
    let root = fixture.0.clone();
    let mut app = app(&root);
    app.open_file(root.join("a.rs"));
    settle(&mut app);
    move_to_line(&mut app, 3);
    // A short move is not a history entry.
    move_to_line(&mut app, 5);

    app.open_file(root.join("b.rs"));
    settle(&mut app);
    move_to_line(&mut app, 30);

    let a = app.projects[0]
        .tabs
        .iter()
        .find(|tab| tab.path.as_ref() == Some(&root.join("a.rs")))
        .unwrap()
        .id;
    app.request_close(CloseAction::Tab(a));
    assert!(
        app.projects[0]
            .tabs
            .iter()
            .all(|tab| tab.path.as_ref() != Some(&root.join("a.rs")))
    );
    app.observe_navigation();
    assert_eq!(location(&app), ("b.rs".into(), 30, 1));

    app.navigate_history(true);
    settle(&mut app);
    assert_eq!(location(&app), ("b.rs".into(), 1, 1));

    app.navigate_history(true);
    settle(&mut app);
    assert_eq!(
        location(&app),
        ("a.rs".into(), 5, 1),
        "the closed document reopens at its last location"
    );

    app.navigate_history(true);
    settle(&mut app);
    assert_eq!(location(&app), ("a.rs".into(), 5, 1));
    assert_eq!(status(&app), "No earlier location in this pane");

    app.navigate_history(false);
    settle(&mut app);
    assert_eq!(location(&app), ("b.rs".into(), 1, 1));
    app.navigate_history(false);
    settle(&mut app);
    assert_eq!(location(&app), ("b.rs".into(), 30, 1));

    // A new navigation drops the forward branch.
    app.navigate_history(true);
    settle(&mut app);
    move_to_line(&mut app, 25);
    app.navigate_history(false);
    assert_eq!(status(&app), "No later location in this pane");
}

#[test]
fn history_shortcuts_and_mouse_buttons_go_back_and_forward() {
    let fixture = Fixture::new();
    let root = fixture.0.clone();
    let mut app = app(&root);
    app.open_file(root.join("a.rs"));
    settle(&mut app);
    app.open_file(root.join("b.rs"));
    settle(&mut app);

    app.command_action("Go back");
    settle(&mut app);
    assert_eq!(location(&app).0, "a.rs");
    app.command_action("Go forward");
    settle(&mut app);
    assert_eq!(location(&app).0, "b.rs");

    let mut screen = egui_kittest::Harness::builder()
        .with_size([400.0, 300.0])
        .build_ui_state(
            |ui, app: &mut VEditApp| {
                app.ctx = ui.ctx().clone();
                app.history_pointer_buttons();
            },
            app,
        );
    let pos = egui::pos2(100.0, 100.0);
    screen.input_mut().events.extend([
        egui::Event::PointerMoved(pos),
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Extra1,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        },
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Extra1,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        },
    ]);
    screen.run_steps(1);
    let mut app = screen.into_state();
    settle(&mut app);
    assert_eq!(location(&app).0, "a.rs");
}

#[test]
fn vim_bracket_d_and_ctrl_o_ctrl_i_use_problems_and_history() {
    let fixture = Fixture::new();
    let root = fixture.0.clone();
    let mut app = app(&root);
    app.settings.vim = true;
    app.settings.smooth_scroll = false;
    app.sidebar_visible = false;
    app.projects[0]
        .diagnostics
        .insert(root.join("a.rs"), vec![diagnostic(11, 2, 1, "an error")]);
    app.open_file(root.join("a.rs"));
    settle(&mut app);
    let mut screen = egui_kittest::Harness::builder()
        .with_size([900.0, 600.0])
        .with_os(egui::os::OperatingSystem::Windows)
        .build_ui_state(
            |ui, app: &mut VEditApp| {
                app.ctx = ui.ctx().clone();
                app.shortcuts();
                app.handle_vim();
                app.apply_pending_location();
                app.apply_pending_jump();
                app.editor_ui(ui);
                app.observe_navigation();
            },
            app,
        );
    screen.run_steps(2);
    let ctrl = |key| egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
    };
    screen
        .input_mut()
        .events
        .extend([egui::Event::Text("]".into()), egui::Event::Text("d".into())]);
    screen.run_steps(2);
    assert_eq!(location(screen.state()), ("a.rs".into(), 12, 3));
    assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::Normal);

    screen.input_mut().events.push(ctrl(Key::O));
    screen.run_steps(2);
    assert_eq!(location(screen.state()), ("a.rs".into(), 1, 1));
    assert!(
        screen.state().picker.is_none(),
        "Ctrl+O does not open files in Vim Normal mode"
    );

    screen.input_mut().events.push(ctrl(Key::I));
    screen.run_steps(2);
    assert_eq!(location(screen.state()), ("a.rs".into(), 12, 3));
    let disk = fs::read_to_string(root.join("a.rs")).unwrap();
    assert_eq!(screen.state().tab().unwrap().text, disk);
}

fn wait_for(app: &mut VEditApp, message: &str, done: impl Fn(&VEditApp) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done(app) {
        assert!(Instant::now() < deadline, "{message}");
        std::thread::sleep(Duration::from_millis(2));
        settle(app);
    }
}

fn output_link(path: &str, line: usize, column: Option<usize>) -> crate::output_links::Link {
    crate::output_links::Link {
        range: 0..path.len(),
        path: path.into(),
        line,
        column,
    }
}

#[test]
fn output_locations_open_from_the_root_then_the_working_directory() {
    let fixture = Fixture::new();
    let root = fixture.0.clone();
    fs::create_dir_all(root.join("tool/pkg")).unwrap();
    fs::write(
        root.join("tool/pkg/run.py"),
        "one
two
three
",
    )
    .unwrap();
    let mut app = app(&root);
    app.open_file(root.join("b.rs"));
    settle(&mut app);
    let cwd = Some(root.join("tool"));

    app.open_output_link(output_link("a.rs", 12, Some(3)), cwd.clone());
    wait_for(&mut app, "the root-relative location did not open", |app| {
        location(app) == ("a.rs".into(), 12, 3)
    });

    app.open_output_link(output_link("pkg/run.py", 2, None), cwd.clone());
    wait_for(&mut app, "the cwd-relative location did not open", |app| {
        location(app) == ("run.py".into(), 2, 1)
    });

    // Only the latest click opens.
    app.open_output_link(output_link("b.rs", 30, None), cwd.clone());
    app.open_output_link(output_link("a.rs", 5, Some(2)), cwd.clone());
    wait_for(&mut app, "the latest click did not open", |app| {
        location(app) == ("a.rs".into(), 5, 2)
    });
    std::thread::sleep(Duration::from_millis(50));
    settle(&mut app);
    assert_eq!(location(&app), ("a.rs".into(), 5, 2));

    app.open_output_link(output_link("src/missing.rs", 1, None), cwd);
    wait_for(&mut app, "a missing file was not reported", |app| {
        status(app).contains("src/missing.rs was not found")
    });
    assert_eq!(location(&app), ("a.rs".into(), 5, 2));

    // Output locations are history entries.
    app.navigate_history(true);
    settle(&mut app);
    assert_eq!(location(&app), ("run.py".into(), 2, 1));
}

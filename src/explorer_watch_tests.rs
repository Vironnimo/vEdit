//! External file changes reach the Explorer and Quick Open without a refresh.

use super::*;
use egui_kittest::{Harness, kittest::Queryable};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "vedit-explorer-watch-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("src/nested")).unwrap();
        fs::write(root.join("notes.txt"), "notes").unwrap();
        fs::write(root.join("src/lib.txt"), "library").unwrap();
        fs::write(root.join("src/nested/deep.txt"), "deep").unwrap();
        Self(fs::canonicalize(root).unwrap())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn harness(root: &Path) -> Harness<'static, VEditApp> {
    let mut app = VEditApp::empty(egui::Context::default());
    app.settings.autosave = false;
    app.file_watching = true;
    app.projects.push(Project::new(root.to_path_buf()));
    app.projects[0].last_git_refresh = Instant::now();
    app.index_project(0);
    let mut setup = egui_wgpu::WgpuSetup::without_display_handle();
    if let egui_wgpu::WgpuSetup::CreateNew(options) = &mut setup {
        options.instance_descriptor.backends = egui_wgpu::wgpu::Backends::GL;
    }
    let mut screen = Harness::builder()
        .with_size([520.0, 620.0])
        .with_step_dt(1.0 / 60.0)
        .with_os(egui::os::OperatingSystem::Windows)
        .wgpu_setup(setup)
        .build_ui_state(
            |ui, app: &mut VEditApp| {
                app.ctx = ui.ctx().clone();
                app.poll();
                app.poll_file_watchers();
                app.sidebar_ui(ui);
            },
            app,
        );
    wait_until(&mut screen, "the watcher did not start", |app| {
        app.projects[0].watcher.is_some()
            && !app.projects[0].indexing
            && app.projects[0]
                .explorer
                .as_ref()
                .is_some_and(|explorer| explorer.pending_count() == 0)
    });
    screen
}

fn wait_until(
    screen: &mut Harness<'static, VEditApp>,
    message: &str,
    done: impl Fn(&VEditApp) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        screen.run_steps(1);
        if done(screen.state()) {
            return;
        }
        assert!(Instant::now() < deadline, "{message}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn indexed(app: &VEditApp, path: &Path) -> bool {
    app.projects[0]
        .index
        .as_ref()
        .is_some_and(|index| index.files.iter().any(|file| file == path))
}

fn click(screen: &mut Harness<'static, VEditApp>, label: &str) {
    let point = screen.get_by_label(label).rect().center();
    screen.input_mut().events.extend([
        egui::Event::PointerMoved(point),
        egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        },
        egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        },
    ]);
    screen.run_steps(3);
}

#[test]
fn external_files_appear_without_refresh_and_keep_selection_and_expansion() {
    let fixture = Fixture::new();
    let root = fixture.0.clone();
    let mut screen = harness(&root);
    click(&mut screen, "src");
    wait_until(&mut screen, "the expanded folder was not listed", |app| {
        app.projects[0]
            .explorer
            .as_ref()
            .is_some_and(|explorer| explorer.has_listing(&root.join("src")))
    });
    screen.run_steps(2);
    click(&mut screen, "nested");
    assert!(screen.state().explorer_selected(&root.join("src/nested")));
    assert!(screen.state().explorer_expanded(&root.join("src")));
    wait_until(&mut screen, "the expanded folder was not listed", |app| {
        app.projects[0]
            .explorer
            .as_ref()
            .is_some_and(|explorer| explorer.has_listing(&root.join("src/nested")))
    });
    let index_generation = screen.state().projects[0].index_generation;

    fs::write(root.join("agent.txt"), "from a terminal").unwrap();
    fs::write(root.join("src/new.rs"), "fn main() {}").unwrap();
    fs::write(root.join("src/nested/plan.md"), "# Plan").unwrap();
    wait_until(&mut screen, "external files did not appear", |app| {
        app.explorer_index().is_some_and(|index| {
            ["agent.txt", "src/new.rs", "src/nested/plan.md"]
                .iter()
                .all(|path| index.has_entry(&root.join(path)))
        }) && ["agent.txt", "src/new.rs", "src/nested/plan.md"]
            .iter()
            .all(|path| indexed(app, &root.join(path)))
    });
    screen.run_steps(3);
    for label in ["agent.txt", "new.rs", "plan.md"] {
        assert!(
            screen.query_by_label(label).is_some(),
            "row {label} missing"
        );
    }
    let app = screen.state();
    assert!(app.explorer_selected(&root.join("src/nested")));
    assert!(app.explorer_expanded(&root.join("src")));
    assert!(app.explorer_expanded(&root.join("src/nested")));
    // A watcher refresh is quiet: no new generation and no indexing state.
    assert_eq!(app.projects[0].index_generation, index_generation);
    assert!(!app.projects[0].indexing);

    // Deleting an expanded folder removes its rows without reporting it as
    // unavailable, and the selection moves to its parent.
    fs::remove_dir_all(root.join("src/nested")).unwrap();
    fs::remove_file(root.join("agent.txt")).unwrap();
    wait_until(&mut screen, "deleted entries remained", |app| {
        app.explorer_index().is_some_and(|index| {
            !index.has_entry(&root.join("src/nested")) && !index.has_entry(&root.join("agent.txt"))
        }) && !indexed(app, &root.join("agent.txt"))
    });
    screen.run_steps(3);
    assert!(screen.query_by_label("nested").is_none());
    assert!(screen.query_by_label("Folder unavailable").is_none());
    assert!(screen.state().explorer_selected(&root.join("src")));
}

#[test]
fn watcher_churn_in_generated_folders_and_git_internals_does_not_reindex() {
    let fixture = Fixture::new();
    let root = fixture.0.clone();
    fs::create_dir_all(root.join(".git/objects")).unwrap();
    fs::create_dir_all(root.join("target/debug")).unwrap();
    let mut screen = harness(&root);
    let request = screen.state().projects[0].index_request;
    for index in 0..20 {
        fs::write(root.join(format!("target/debug/artifact-{index}.o")), "x").unwrap();
        fs::write(root.join(format!(".git/objects/{index}")), "x").unwrap();
    }
    // Content edits of known files do not change any listing either.
    fs::write(root.join("notes.txt"), "edited").unwrap();
    let settle = Instant::now() + crate::fs_watch::MAX_DELAY + Duration::from_millis(300);
    while Instant::now() < settle {
        screen.run_steps(1);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(screen.state().projects[0].index_request, request);
    assert!(!screen.state().projects[0].index_in_flight);
}

#[test]
fn projects_without_git_follow_changes_too() {
    // The fixture has no repository: the watcher alone keeps the tree current.
    let fixture = Fixture::new();
    let root = fixture.0.clone();
    assert!(!root.join(".git").exists());
    let mut screen = harness(&root);
    fs::create_dir(root.join("docs")).unwrap();
    wait_until(&mut screen, "the new folder did not appear", |app| {
        app.explorer_index()
            .is_some_and(|index| index.has_entry(&root.join("docs")))
    });
}

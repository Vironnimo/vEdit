use super::*;
use egui_kittest::{Harness, kittest::Queryable};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "morrow-explorer-filter-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed),
        ));
        for project in ["alpha", "beta"] {
            let project = root.join(project);
            fs::create_dir_all(project.join("target")).unwrap();
            fs::write(project.join("visible.txt"), "visible").unwrap();
            fs::write(project.join("target/generated.txt"), "generated").unwrap();
        }
        Self(root)
    }

    fn app(&self) -> MorrowApp {
        let mut app = MorrowApp::empty(egui::Context::default());
        for name in ["alpha", "beta"] {
            let root = self.0.join(name);
            let mut project = Project::new(root.clone());
            project.index = Some(Arc::new(
                workspace::build_index(&root, workspace::IndexOptions::default()).unwrap(),
            ));
            app.projects.push(project);
        }
        app
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn finish_indexing(app: &mut MorrowApp) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.projects.iter().any(|project| project.indexing) {
        assert!(Instant::now() < deadline, "Explorer reindex did not finish");
        app.poll();
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn explorer_filter_toggle_refreshes_every_project_and_quick_open_without_clearing_the_old_index() {
    let fixture = Fixture::new();
    let app = fixture.app();
    let old: Vec<_> = app
        .projects
        .iter()
        .map(|project| project.index.clone().unwrap())
        .collect();
    let mut screen = Harness::builder().with_size([300.0, 220.0]).build_ui_state(
        |ui, app: &mut MorrowApp| {
            app.ctx = ui.ctx().clone();
            app.explorer_filter_ui(ui);
        },
        app,
    );
    screen.run_steps(2);
    assert!(
        screen
            .query_by_label("Ignore rules and generated folders are hidden.")
            .is_some()
    );
    assert!(
        screen
            .query_by_label("Open a file directly: Ctrl+O")
            .is_some()
    );
    screen.get_by_label("Show ignored files").click();
    screen.run_steps(2);
    assert!(screen.state().settings.show_ignored_files);
    assert!(screen.state().session_dirty);
    for (project, old) in screen.state().projects.iter().zip(&old) {
        assert!(project.indexing);
        assert!(Arc::ptr_eq(project.index.as_ref().unwrap(), old));
        assert!(old.quick_open("generated.txt", 40).is_empty());
    }
    finish_indexing(screen.state_mut());
    for project in &screen.state().projects {
        assert_eq!(
            project
                .index
                .as_ref()
                .unwrap()
                .quick_open("generated.txt", 40),
            vec![project.root.join("target/generated.txt")],
        );
    }
    screen.get_by_label("Show ignored files").click();
    screen.run_steps(2);
    assert!(!screen.state().settings.show_ignored_files);
    finish_indexing(screen.state_mut());
    assert!(screen.state().projects.iter().all(|project| {
        project
            .index
            .as_ref()
            .unwrap()
            .quick_open("generated.txt", 40)
            .is_empty()
    }));
}

#[test]
fn changing_filter_cancels_stale_indexes_and_a_successful_refresh_clears_the_error() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.projects[0].index_error = Some("Temporary indexing error".into());
    app.settings.show_ignored_files = true;
    app.reindex_explorer_visibility();
    let previous = app.projects[0].index_cancel.clone();
    app.settings.show_ignored_files = false;
    app.reindex_explorer_visibility();
    assert!(previous.load(Ordering::Relaxed));
    finish_indexing(&mut app);
    assert!(app.projects[0].index_error.is_none());
    for project in &app.projects {
        assert!(
            project
                .index
                .as_ref()
                .unwrap()
                .quick_open("generated.txt", 40)
                .is_empty()
        );
    }
}

#[test]
fn showing_dependency_manifests_never_starts_rust_analysis_for_a_non_rust_project() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    for project in &app.projects {
        fs::create_dir_all(project.root.join("node_modules/native")).unwrap();
        fs::write(
            project.root.join("node_modules/native/Cargo.toml"),
            "[package]",
        )
        .unwrap();
    }
    for show_ignored_files in [true, false] {
        app.settings.show_ignored_files = show_ignored_files;
        app.reindex_explorer_visibility();
        finish_indexing(&mut app);
        app.sync_languages(false);
        for project in &app.projects {
            assert!(!project.rust_project);
            assert!(project.rust_language().server.is_none());
            assert_eq!(
                project
                    .index
                    .as_ref()
                    .unwrap()
                    .quick_open("Cargo.toml", 40)
                    .len(),
                usize::from(show_ignored_files)
            );
        }
    }
}

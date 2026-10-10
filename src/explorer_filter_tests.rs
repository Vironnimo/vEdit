//! Explorer completeness is independent of search indexing and legacy filters.
use super::*;
use egui_kittest::{Harness, kittest::Queryable};

struct Fixture(PathBuf);

impl Fixture {
    fn new(extra_files: usize) -> Self {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "vedit-lazy-explorer-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir_all(root.join("target")).unwrap();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join(".gitignore"), "secret.txt\n").unwrap();
        fs::write(root.join("visible.txt"), "visible").unwrap();
        fs::write(root.join("secret.txt"), "ignored").unwrap();
        fs::write(root.join("target/generated.txt"), "generated").unwrap();
        for number in 0..extra_files {
            fs::write(root.join(format!("file-{number:04}.txt")), "fixture").unwrap();
        }
        Self(root)
    }

    fn app(&self) -> VEditApp {
        let mut app = VEditApp::empty(egui::Context::default());
        app.settings.autosave = false;
        let mut project = Project::new(self.0.clone());
        project.index = Some(Arc::new(
            workspace::build_index(&self.0, Default::default()).unwrap(),
        ));
        app.projects.push(project);
        app
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn finish(app: &mut VEditApp, directory: &Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        app.poll();
        let project = app.project().unwrap();
        let tree = project.explorer.as_ref().unwrap();
        if !project.indexing && !tree.is_loading(directory) {
            assert!(
                tree.is_loaded(directory),
                "folder failed: {:?}",
                tree.error(directory)
            );
            return;
        }
        assert!(Instant::now() < deadline, "Explorer worker did not finish");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn ignored_and_generated_entries_are_visible_without_changing_quick_open_policy() {
    let fixture = Fixture::new(0);
    let mut app = fixture.app();
    app.settings.show_ignored_files = true;
    app.index_project(0);
    finish(&mut app, &fixture.0);
    let index = app.explorer_index().unwrap();
    for name in ["target", "secret.txt", "visible.txt", ".gitignore"] {
        assert!(index.has_entry(&fixture.0.join(name)), "missing {name}");
    }
    assert!(!index.has_entry(&fixture.0.join(".git")));
    let quick_open = app.project().unwrap().index.as_ref().unwrap();
    assert!(quick_open.quick_open("secret.txt", 40).is_empty());
    assert!(quick_open.quick_open("generated.txt", 40).is_empty());
    assert!(
        !index.directory_loaded(&fixture.0.join("target")),
        "root loading must not recurse"
    );
    app.request_explorer_directory(0, fixture.0.join("target"));
    finish(&mut app, &fixture.0.join("target"));
    assert!(
        app.explorer_index()
            .unwrap()
            .has_entry(&fixture.0.join("target/generated.txt"))
    );
}

#[test]
fn explorer_lists_complete_root_when_the_quick_open_index_is_truncated() {
    let fixture = Fixture::new(64);
    let mut app = fixture.app();
    app.projects[0].index = Some(Arc::new(
        workspace::build_index(
            &fixture.0,
            workspace::IndexOptions {
                max_entries: 3,
                ..Default::default()
            },
        )
        .unwrap(),
    ));
    assert!(app.projects[0].index.as_ref().unwrap().truncated);
    app.projects[0].explorer = Some(workspace::ExplorerTree::new(fixture.0.clone()));
    app.request_explorer_directory(0, fixture.0.clone());
    finish(&mut app, &fixture.0);
    let explorer = app.explorer_index().unwrap();
    assert_eq!(explorer.children(&fixture.0).count(), 68);
    assert!(explorer.has_entry(&fixture.0.join("file-0063.txt")));
    assert!(!explorer.truncated);
    assert!(app.projects[0].index.as_ref().unwrap().truncated);
}

#[test]
fn refresh_retains_visible_snapshot_while_reloading_on_background_workers() {
    let fixture = Fixture::new(0);
    let mut app = fixture.app();
    app.index_project(0);
    finish(&mut app, &fixture.0);
    let old_explorer = app.explorer_index().unwrap();
    let old_quick_open = app.project().unwrap().index.clone().unwrap();
    fs::write(fixture.0.join("new.txt"), "new").unwrap();
    app.index_project(0);
    assert!(Arc::ptr_eq(&app.explorer_index().unwrap(), &old_explorer));
    assert!(Arc::ptr_eq(
        app.project().unwrap().index.as_ref().unwrap(),
        &old_quick_open
    ));
    finish(&mut app, &fixture.0);
    assert!(
        app.explorer_index()
            .unwrap()
            .has_entry(&fixture.0.join("new.txt"))
    );
    assert!(!old_explorer.has_entry(&fixture.0.join("new.txt")));
}

#[test]
fn app_poll_rejects_directory_results_from_a_closed_and_reopened_project() {
    let fixture = Fixture::new(0);
    let mut app = fixture.app();
    let mut previous = workspace::ExplorerTree::new(fixture.0.clone());
    let old_generation = previous.begin(&fixture.0).unwrap();
    let mut replacement = workspace::ExplorerTree::new(fixture.0.clone());
    let new_generation = replacement.begin(&fixture.0).unwrap();
    app.projects[0].explorer = Some(replacement);
    let listing = |name: &str| workspace::ExplorerDirectory {
        root: fixture.0.clone(),
        path: fixture.0.clone(),
        entries: vec![workspace::WorkspaceEntry {
            path: fixture.0.join(name),
            name: name.into(),
            is_dir: false,
        }],
        warnings: Vec::new(),
    };
    for (generation, name) in [(new_generation, "fresh.txt"), (old_generation, "stale.txt")] {
        app.sender
            .send(WorkerResult::ExplorerDirectory {
                root: fixture.0.clone(),
                path: fixture.0.clone(),
                generation,
                result: Ok(listing(name)),
            })
            .unwrap();
    }
    app.poll();
    let index = app.explorer_index().unwrap();
    assert!(index.has_entry(&fixture.0.join("fresh.txt")));
    assert!(!index.has_entry(&fixture.0.join("stale.txt")));
}

#[test]
fn folder_error_waits_for_retry_instead_of_restarting_a_worker_every_frame() {
    let fixture = Fixture::new(0);
    let mut app = fixture.app();
    let mut explorer = workspace::ExplorerTree::new(fixture.0.clone());
    let generation = explorer.begin(&fixture.0).unwrap();
    app.projects[0].explorer = Some(explorer);
    app.sender
        .send(WorkerResult::ExplorerDirectory {
            root: fixture.0.clone(),
            path: fixture.0.clone(),
            generation,
            result: Err("Temporary directory error".into()),
        })
        .unwrap();
    app.poll();
    for _ in 0..3 {
        app.request_explorer_directory(0, fixture.0.clone());
    }
    let tree = app.project().unwrap().explorer.as_ref().unwrap();
    assert_eq!(tree.error(&fixture.0), Some("Temporary directory error"));
    assert_eq!(tree.pending_count(), 0);
    app.retry_explorer();
    finish(&mut app, &fixture.0);
    assert!(
        app.project()
            .unwrap()
            .explorer
            .as_ref()
            .unwrap()
            .error(&fixture.0)
            .is_none()
    );
}

#[test]
fn expanded_folder_reads_keep_concurrent_workers_bounded() {
    let fixture = Fixture::new(0);
    let mut app = fixture.app();
    app.projects[0].explorer = Some(workspace::ExplorerTree::new(fixture.0.clone()));
    for number in 0..16 {
        let path = fixture.0.join(format!("folder-{number:02}"));
        fs::create_dir(&path).unwrap();
        app.request_explorer_directory(0, path);
    }
    assert_eq!(
        app.projects[0].explorer.as_ref().unwrap().pending_count(),
        8
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.projects[0].explorer.as_ref().unwrap().pending_count() != 0 {
        app.poll();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        !app.explorer_index()
            .unwrap()
            .directory_loaded(&fixture.0.join("folder-15"))
    );
}

#[test]
fn synthetic_explorer_snapshots_render_without_filters_limit_banners_or_workers() {
    let mut app = VEditApp::empty(egui::Context::default());
    let root = PathBuf::from("C:/vedit-in-memory-explorer-fixture");
    let mut index = workspace::WorkspaceIndex::empty(root.clone());
    index.truncated = true;
    let mut project = Project::new(root);
    project.index = Some(Arc::new(index));
    app.projects.push(project);
    let mut screen = Harness::builder().with_size([300.0, 220.0]).build_ui_state(
        |ui, app: &mut VEditApp| {
            app.ctx = ui.ctx().clone();
            app.explorer(ui);
        },
        app,
    );
    screen.run_steps(3);
    assert!(screen.query_by_label("Show ignored files").is_none());
    assert!(
        screen
            .query_by_label("Index limit reached. Some files may be missing.")
            .is_none()
    );
    assert!(screen.state().project().unwrap().explorer.is_none());
    assert!(screen.state().receiver.try_recv().is_err());
}

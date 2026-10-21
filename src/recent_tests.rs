use super::*;

fn file(name: &str) -> PathBuf {
    std::env::temp_dir().join(name)
}

#[test]
fn history_is_bounded_deduplicated_and_persisted_in_recent_order() {
    let mut app = VEditApp::empty(egui::Context::default());
    for index in 0..40 {
        app.remember_recent(file(&format!("note-{index}.txt")), Kind::File);
    }
    app.remember_recent(file("note-15.txt"), Kind::File);
    app.remember_recent(file("project"), Kind::Folder);
    assert_eq!(app.recent.len(), LIMIT);
    assert_eq!(app.recent[0].kind, Kind::Folder);
    assert_eq!(app.recent[1].path, file("note-15.txt"));
    assert_eq!(
        app.recent
            .iter()
            .filter(|entry| entry.path == file("note-15.txt"))
            .count(),
        1
    );
    let session: Session =
        serde_json::from_slice(&serde_json::to_vec(&app.session()).unwrap()).unwrap();
    let mut restored = VEditApp::empty(egui::Context::default());
    restored.restore_recent(session.recent);
    assert_eq!(restored.recent, app.recent);
    assert_eq!(
        restored.matching_recent("NOTE 15")[0].path,
        file("note-15.txt")
    );
    restored.clear_recent();
    assert!(restored.session().recent.is_empty());
    let legacy: Session = serde_json::from_str("{}").unwrap();
    assert!(legacy.recent.is_empty());
}

#[test]
fn reopening_an_existing_dirty_buffer_does_not_read_or_replace_it() {
    let mut app = VEditApp::empty(egui::Context::default());
    let index = app.ensure_other_files();
    let path = file("vedit-recent-nonexistent-draft.txt");
    let mut tab = EditorTab::new(Some(path.clone()), "draft.txt".into());
    tab.text = "unsaved 🦀".into();
    tab.dirty = true;
    let id = tab.id;
    app.projects[index].tabs.push(tab);
    app.activate_tab(index, 0);
    app.open_recent(Entry {
        path,
        kind: Kind::File,
    });
    assert_eq!(app.tab().unwrap().id, id);
    assert_eq!(app.tab().unwrap().text, "unsaved 🦀");
    assert!(!app.tab().unwrap().loading);
}

#[test]
fn delayed_open_does_not_steal_focus_and_errors_keep_history() {
    let mut app = VEditApp::empty(egui::Context::default());
    let entry = Entry {
        path: file("unavailable.txt"),
        kind: Kind::File,
    };
    app.remember_recent(entry.path.clone(), entry.kind);
    app.recent_open_request = 3;
    app.finish_recent_open(OpenResult {
        request: 3,
        origin: None,
        entry: entry.clone(),
        result: Err("offline".into()),
    });
    assert!(app.projects.is_empty());
    assert_eq!(app.recent, vec![entry.clone()]);
    assert!(app.status.as_ref().unwrap().0.contains("offline"));
    app.new_file();
    let id = app.tab().unwrap().id;
    app.finish_recent_open(OpenResult {
        request: 3,
        origin: None,
        entry,
        result: Ok(file("unavailable.txt")),
    });
    assert_eq!(app.tab().unwrap().id, id);
    assert_eq!(app.project().unwrap().tabs.len(), 1);
    assert!(app.status.as_ref().unwrap().0.contains("cancelled"));
}

#[test]
fn clearing_history_invalidates_a_pending_open() {
    let mut app = VEditApp::empty(egui::Context::default());
    app.recent_open_request = 9;
    app.clear_recent();
    app.finish_recent_open(OpenResult {
        request: 9,
        origin: None,
        entry: Entry {
            path: file("old.txt"),
            kind: Kind::File,
        },
        result: Ok(file("old.txt")),
    });
    assert!(app.projects.is_empty());
    assert!(app.recent.is_empty());
}

#[test]
fn recent_file_and_folder_open_through_workers_and_survive_closing() {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "vedit-recent-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&root).unwrap();
    let root = fs::canonicalize(root).unwrap();
    let path = root.join("notes café.txt");
    fs::write(&path, "hello 🦀").unwrap();
    let mut app = VEditApp::empty(egui::Context::default());
    app.recovery_session_path = root.join("session.json");
    app.open_recent(Entry {
        path: path.clone(),
        kind: Kind::File,
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while app.tab().is_none_or(|tab| tab.loading) {
        assert!(Instant::now() < deadline);
        app.poll();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(app.tab().unwrap().text, "hello 🦀");
    assert!(app.project().unwrap().is_other_files());
    assert_eq!(app.recent[0].path, path);
    app.perform_close(CloseAction::Project(0));
    assert_eq!(app.recent[0].path, path);
    app.open_recent(Entry {
        path: root.clone(),
        kind: Kind::Folder,
    });
    while app.project().is_none_or(|project| project.indexing) {
        assert!(Instant::now() < deadline);
        app.poll();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(app.active_folder(), Some(root.as_path()));
    assert_eq!(app.recent[0].kind, Kind::Folder);
    drop(app);
    // Only fixture files; no recursive cleanup or access to the live session.
    fs::remove_file(path).unwrap();
    let _ = fs::remove_file(root.join("session.json"));
    fs::remove_dir(root).unwrap();
}

#[test]
fn recent_files_visual_layout() {
    let mut app = VEditApp::empty(egui::Context::default());
    app.recent = vec![
        Entry {
            path: PathBuf::from(r"C:\Downloads\notes.md"),
            kind: Kind::File,
        },
        Entry {
            path: PathBuf::from(r"C:\Code\vEdit"),
            kind: Kind::Folder,
        },
        Entry {
            path: PathBuf::from(r"D:\Projects\website\notes.md"),
            kind: Kind::File,
        },
    ];
    app.open_palette(Palette::Recent);
    let mut screen = super::super::visual_tests::harness(app);
    super::super::visual_tests::snapshot(&mut screen, "vedit_recent_files");
}

#[test]
fn opening_multiple_files_remembers_every_successful_load() {
    let root = std::env::temp_dir().join(format!("vedit-recent-batch-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let root = fs::canonicalize(root).unwrap();
    let paths = [root.join("first.txt"), root.join("second.txt")];
    let mut app = VEditApp::empty(egui::Context::default());
    app.recovery_session_path = root.join("session.json");
    for path in &paths {
        fs::write(path, "hello").unwrap();
        app.open_routed_file(path.clone());
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while app
        .projects
        .iter()
        .flat_map(|project| &project.tabs)
        .any(|tab| tab.loading)
    {
        assert!(Instant::now() < deadline);
        app.poll();
        std::thread::sleep(Duration::from_millis(5));
    }
    let recent: Vec<_> = app.recent.iter().map(|entry| entry.path.clone()).collect();
    drop(app);
    for path in &paths {
        fs::remove_file(path).unwrap();
    }
    let _ = fs::remove_file(root.join("session.json"));
    fs::remove_dir(root).unwrap();
    assert!(paths.iter().all(|path| recent.contains(path)), "{recent:?}");
}

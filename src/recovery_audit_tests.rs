use super::*;

struct RecoveryFixture {
    base: PathBuf,
    root: PathBuf,
}

impl RecoveryFixture {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let base = std::env::temp_dir().join(format!(
            "morrow-recovery-audit-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir_all(base.join("workspace")).unwrap();
        let base = fs::canonicalize(base).unwrap();
        let root = base.join("workspace");
        Self { base, root }
    }

    fn write(&self, name: &str, text: &str) -> PathBuf {
        let path = self.root.join(name);
        fs::write(&path, text).unwrap();
        path
    }

    fn empty_app(&self) -> MorrowApp {
        let mut app = MorrowApp::empty(egui::Context::default());
        app.recovery_session_path = self.base.join("recovery/session.json");
        app.settings.autosave = false;
        app
    }

    fn app(&self) -> MorrowApp {
        let mut app = self.empty_app();
        // These tests exercise document workers, not indexing or Git discovery.
        app.projects.push(Project::new(self.root.clone()));
        app
    }

    fn open(&self, app: &mut MorrowApp, path: PathBuf) -> u64 {
        app.open_file(path);
        wait_for(app, |app| !app.tab().unwrap().loading);
        let tab = app.tab_mut().unwrap();
        tab.recovery_path = self.base.join(format!("recovery/{}.txt", tab.id));
        tab.id
    }

    fn saved_session(&self) -> Session {
        serde_json::from_slice(&fs::read(self.base.join("recovery/session.json")).unwrap()).unwrap()
    }

    fn restore(&self, mut session: Session) -> MorrowApp {
        let mut app = self.empty_app();
        app.settings = session.settings.take().unwrap_or_default();
        if let Some(autosave) = session.autosave {
            app.settings.autosave = autosave;
        }
        app.restore_session_workspace(session.projects, session.active_project, session.layout);
        wait_for(&mut app, |app| {
            app.projects
                .iter()
                .flat_map(|project| &project.tabs)
                .all(|tab| !tab.loading)
        });
        app
    }
}

impl Drop for RecoveryFixture {
    fn drop(&mut self) {
        let temporary_root = fs::canonicalize(std::env::temp_dir()).unwrap();
        assert!(self.base.starts_with(&temporary_root));
        assert!(
            self.base
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("morrow-recovery-audit-")
        );
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn wait_for(app: &mut MorrowApp, condition: impl Fn(&MorrowApp) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        app.poll();
        if condition(app) {
            return;
        }
        assert!(Instant::now() < deadline, "document worker did not finish");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn edit(app: &mut MorrowApp, text: &str) {
    let tab = app.tab_mut().unwrap();
    tab.vim.begin_external_edit(&tab.text);
    tab.text = text.into();
    tab.changed();
}

fn settle_document_writes(app: &mut MorrowApp) {
    wait_for(app, |app| {
        app.projects
            .iter()
            .flat_map(|project| &project.tabs)
            .all(|tab| !tab.loading && !tab.saving && !tab.recovery_pending)
    });
}

fn hold_save_completion(app: &mut MorrowApp) -> WorkerResult {
    let (sender, receiver) = mpsc::channel();
    let normal_sender = std::mem::replace(&mut app.sender, sender);
    app.save_current();
    app.sender = normal_sender;
    assert!(app.tab().unwrap().saving);
    let completion = receiver
        .recv_timeout(Duration::from_secs(8))
        .expect("save worker must finish while its completion is held back");
    assert!(matches!(
        &completion,
        WorkerResult::Saved { result: Ok(_), .. }
    ));
    completion
}

#[test]
fn mixed_line_endings_follow_the_source_when_saving_as_new_text_or_binary_targets() {
    for (label, destination_bytes) in [
        ("new", None),
        ("text", Some(b"unrelated\nexisting\ncontent\n".as_slice())),
        ("binary", Some(b"\xff\x00\xfe".as_slice())),
    ] {
        let fixture = RecoveryFixture::new(&format!("mixed-save-as-{label}"));
        let original = "\u{feff}one\r\ntwo\nthree\r\nfour\r";
        let source = fixture.write("source.txt", original);
        let destination = fixture.root.join("copy.txt");
        let overwrite = destination_bytes.map(|bytes| {
            fs::write(&destination, bytes).unwrap();
            workspace::fingerprint(&destination, MAX_DOCUMENT).unwrap()
        });
        let mut app = fixture.app();
        let id = fixture.open(&mut app, source.clone());
        edit(&mut app, "one\ntwo edited\nthree\nfour\n");
        app.request_save_as(id);
        let request = app.picker.unwrap();
        app.finish_native_dialog(
            request,
            Ok(Some(DialogSelection {
                paths: vec![destination.clone()],
                overwrite,
            })),
        );
        settle_document_writes(&mut app);

        assert_eq!(fs::read_to_string(&source).unwrap(), original);
        assert_eq!(
            fs::read_to_string(&destination).unwrap(),
            "\u{feff}one\r\ntwo edited\nthree\r\nfour\r",
            "the {label} destination must not supply the source's line endings"
        );
        let tab = app.tab().unwrap();
        assert!(!tab.dirty && !tab.saving);
        assert!(tab.error.is_none());
        assert_eq!(tab.line_ending, workspace::LineEnding::Mixed);
        assert!(tab.bom);
    }
}

#[test]
fn mixed_save_as_with_unavailable_original_preserves_the_draft_and_reports_lf() {
    for deleted in [false, true] {
        let fixture = RecoveryFixture::new(if deleted {
            "mixed-save-as-missing-source"
        } else {
            "mixed-save-as-changed-source"
        });
        let source = fixture.write("source.txt", "one\r\ntwo\n");
        let destination = fixture.write("copy.txt", "destination\r\n");
        let overwrite = workspace::fingerprint(&destination, MAX_DOCUMENT).unwrap();
        let mut app = fixture.app();
        let id = fixture.open(&mut app, source.clone());
        edit(&mut app, "My draft 🦀\nsecond line\n");
        if deleted {
            fs::remove_file(&source).unwrap();
        } else {
            fs::write(&source, "external\rchange\r").unwrap();
        }
        app.save_tab_expected(id, Some(destination.clone()), Some(overwrite));
        settle_document_writes(&mut app);

        assert_eq!(
            fs::read_to_string(destination).unwrap(),
            "My draft 🦀\nsecond line\n"
        );
        assert_eq!(app.tab().unwrap().line_ending, workspace::LineEnding::Lf);
        assert!(!app.tab().unwrap().dirty);
        assert!(app.tab().unwrap().error.is_none());
        assert!(
            app.status
                .as_ref()
                .unwrap()
                .0
                .contains("with LF line endings")
        );
        if deleted {
            assert!(!source.exists());
        } else {
            assert_eq!(fs::read_to_string(source).unwrap(), "external\rchange\r");
        }
    }
}

#[test]
fn mixed_save_as_keeps_the_destination_conflict_guard_separate_from_its_format_source() {
    let fixture = RecoveryFixture::new("mixed-save-as-conflicting-target");
    let source = fixture.write("source.txt", "one\r\ntwo\n");
    let destination = fixture.write("copy.txt", "confirmed destination\n");
    let overwrite = workspace::fingerprint(&destination, MAX_DOCUMENT).unwrap();
    let mut app = fixture.app();
    let id = fixture.open(&mut app, source.clone());
    edit(&mut app, "one\nlocal edit\n");
    fs::write(&destination, "external change after confirmation\n").unwrap();
    app.save_tab_expected(id, Some(destination.clone()), Some(overwrite));
    settle_document_writes(&mut app);

    assert_eq!(fs::read_to_string(source).unwrap(), "one\r\ntwo\n");
    assert_eq!(
        fs::read_to_string(destination).unwrap(),
        "external change after confirmation\n"
    );
    assert_eq!(app.tab().unwrap().text, "one\nlocal edit\n");
    assert!(app.tab().unwrap().dirty);
    assert!(app.tab().unwrap().error.is_some());
}

#[test]
fn saving_a_mixed_document_updates_its_format_when_only_one_ending_remains() {
    let fixture = RecoveryFixture::new("mixed-save-format-status");
    let path = fixture.write("source.txt", "one\r\ntwo\n");
    let mut app = fixture.app();
    fixture.open(&mut app, path.clone());
    edit(&mut app, "one\n");
    app.save_current();
    settle_document_writes(&mut app);

    assert_eq!(fs::read_to_string(path).unwrap(), "one\r\n");
    assert_eq!(app.tab().unwrap().line_ending, workspace::LineEnding::CrLf);
    assert!(!app.tab().unwrap().dirty);
}

#[test]
fn recovery_restore_never_adopts_an_external_edit_as_its_save_baseline() {
    let fixture = RecoveryFixture::new("external-baseline");
    let path = fixture.write("notes.txt", "\u{feff}Gespeichert ü\r\nzweite Zeile\r\n");
    let mut app = fixture.app();
    fixture.open(&mut app, path.clone());
    edit(&mut app, "Mein Entwurf 🦀\nzweite Zeile\n");
    app.settings.autosave = true;
    app.flush_recovery();
    let session = fixture.saved_session();
    drop(app);

    let external = "Extern geändert Ω\nzusätzliche Zeile\n";
    fs::write(&path, external).unwrap();
    let mut restored = fixture.restore(session);
    assert_eq!(
        restored.tab().unwrap().text,
        "Mein Entwurf 🦀\nzweite Zeile\n"
    );
    assert!(restored.tab().unwrap().dirty);

    restored.tab_mut().unwrap().last_edit = Instant::now() - Duration::from_secs(2);
    restored.autosave();
    settle_document_writes(&mut restored);
    assert_eq!(fs::read(&path).unwrap(), external.as_bytes());

    restored.save_current();
    settle_document_writes(&mut restored);
    assert_eq!(fs::read(&path).unwrap(), external.as_bytes());
    let tab = restored.tab().unwrap();
    assert_eq!(tab.text, "Mein Entwurf 🦀\nzweite Zeile\n");
    assert!(tab.dirty);
    assert!(
        tab.error.is_some(),
        "the conflicting draft needs a visible error"
    );
}

#[test]
fn recovery_restore_can_save_again_when_the_original_disk_content_is_unchanged() {
    let fixture = RecoveryFixture::new("unchanged-baseline");
    let path = fixture.write("notes.txt", "\u{feff}Original ü\r\nzweite Zeile\r\n");
    let mut app = fixture.app();
    fixture.open(&mut app, path.clone());
    edit(&mut app, "Wiederhergestellt 🦀\nzweite Zeile\n");
    app.flush_recovery();
    let session = fixture.saved_session();
    drop(app);

    let mut restored = fixture.restore(session);
    assert!(restored.tab().unwrap().dirty);
    assert!(restored.tab().unwrap().error.is_none());
    restored.save_current();
    settle_document_writes(&mut restored);
    assert_eq!(
        fs::read(&path).unwrap(),
        "\u{feff}Wiederhergestellt 🦀\r\nzweite Zeile\r\n".as_bytes(),
    );
    assert!(!restored.tab().unwrap().dirty);
    assert!(restored.tab().unwrap().error.is_none());
}

#[test]
fn legacy_recovery_without_a_baseline_preserves_disk_on_auto_and_explicit_save() {
    let fixture = RecoveryFixture::new("legacy-baseline");
    let path = fixture.write("legacy.txt", "\u{feff}Externe aktuelle Datei\r\n");
    let recovery_path = fixture.base.join("legacy-recovery.txt");
    fs::write(&recovery_path, "Alter ungespeicherter Entwurf ä\n").unwrap();
    let session = Session {
        projects: vec![SessionProject {
            root: fixture.root.clone(),
            tabs: vec![SessionTab {
                path: Some(path.clone()),
                title: "legacy.txt".into(),
                recovery_path: Some(recovery_path),
                ..Default::default()
            }],
            ..Default::default()
        }],
        autosave: Some(true),
        ..Default::default()
    };
    let mut restored = fixture.restore(session);
    assert_eq!(
        restored.tab().unwrap().text,
        "Alter ungespeicherter Entwurf ä\n"
    );
    restored.tab_mut().unwrap().last_edit = Instant::now() - Duration::from_secs(2);
    restored.autosave();
    settle_document_writes(&mut restored);
    restored.save_current();
    settle_document_writes(&mut restored);
    assert_eq!(
        fs::read(path).unwrap(),
        "\u{feff}Externe aktuelle Datei\r\n".as_bytes(),
    );
    assert!(restored.tab().unwrap().dirty);
    assert!(restored.tab().unwrap().error.is_some());
}

#[test]
fn undo_to_saved_content_removes_recovery_and_rejects_a_late_snapshot() {
    let fixture = RecoveryFixture::new("undo-recovery");
    let original = "Gespeichert ü\n";
    let path = fixture.write("undo.txt", original);
    let mut app = fixture.app();
    let id = fixture.open(&mut app, path.clone());
    edit(&mut app, "Bereits rückgängig gemachte Änderung 🦀\n");
    app.flush_recovery();
    let tab = app.tab().unwrap();
    let recovery_path = tab.recovery_path.clone();
    let stale_revision = tab.revision;
    let stale_generation = tab.recovery_generation;
    let stale_bytes = fs::read(&recovery_path).unwrap();
    let late_writer = app.recovery_writer.clone();
    let stale_order = late_writer.stamp();

    let tab = app.tab_mut().unwrap();
    assert!(
        tab.vim
            .handle_key(&mut tab.text, VimKey::Char('u'))
            .text_changed
    );
    tab.changed();
    assert_eq!(tab.text, original);
    assert!(!tab.dirty);
    // Deterministically hold the completion of a snapshot from before Undo.
    tab.recovery_pending = true;
    app.autosave();
    wait_for(&mut app, |_| !recovery_path.exists());
    late_writer
        .write(&recovery_path, stale_order, &stale_bytes)
        .unwrap();
    app.sender
        .send(WorkerResult::Recovered {
            id,
            revision: stale_revision,
            generation: stale_generation,
            result: Ok(()),
        })
        .unwrap();
    app.poll();
    assert!(
        !recovery_path.exists(),
        "a pre-Undo worker must not recreate recovery"
    );
    assert_eq!(fs::read_to_string(path).unwrap(), original);
    app.flush_recovery();
    let session = fixture.saved_session();
    drop(app);

    let restored = fixture.restore(session);
    assert_eq!(restored.tab().unwrap().text, original);
    assert!(!restored.tab().unwrap().dirty);
}

#[test]
fn offline_project_keeps_named_and_untitled_recovery_across_two_restarts() {
    let fixture = RecoveryFixture::new("offline-project");
    let path = fixture.write("named.txt", "Gespeichert\n");
    let mut app = fixture.app();
    fixture.open(&mut app, path.clone());
    edit(&mut app, "Benannter Entwurf ü\n");
    app.new_file();
    app.tab_mut().unwrap().recovery_path = fixture.base.join("recovery/untitled.txt");
    edit(&mut app, "Noch unbenannter Entwurf 🦀\n");
    app.flush_recovery();
    let mut session = fixture.saved_session();
    drop(app);

    let unavailable = fixture.base.join("disconnected-workspace");
    assert!(fixture.root.starts_with(&fixture.base));
    assert!(unavailable.starts_with(&fixture.base));
    fs::rename(&fixture.root, unavailable).unwrap();
    for _ in 0..2 {
        let restored = fixture.restore(session.clone());
        assert_eq!(
            restored.projects.len(),
            1,
            "offline recovery must remain reachable"
        );
        let project = &restored.projects[0];
        assert_eq!(project.root, fixture.root);
        assert_eq!(project.tabs.len(), 2);
        assert_eq!(project.tabs[0].path.as_ref(), Some(&path));
        assert_eq!(project.tabs[0].text, "Benannter Entwurf ü\n");
        assert_eq!(project.tabs[1].path, None);
        assert_eq!(project.tabs[1].text, "Noch unbenannter Entwurf 🦀\n");
        assert!(project.tabs.iter().all(|tab| tab.dirty));
        restored.flush_recovery();
        // The next restart reads exactly the newly persisted session.
        session = fixture.saved_session();
        assert_eq!(session.projects.len(), 1);
        assert_eq!(session.projects[0].tabs.len(), 2);
        assert_eq!(session.projects[0].root, fixture.root);
    }
}

#[test]
fn reload_does_not_start_another_worker_while_the_document_is_loading_or_saving() {
    for saving in [false, true] {
        let fixture = RecoveryFixture::new(if saving {
            "reload-saving"
        } else {
            "reload-loading"
        });
        let path = fixture.write("busy.txt", "Original\n");
        let mut app = fixture.app();
        fixture.open(&mut app, path);
        let tab = app.tab_mut().unwrap();
        tab.loading = !saving;
        tab.saving = saving;
        let (sender, receiver) = mpsc::channel();
        app.sender = sender;
        app.reload_current();
        drop(app);
        // A spawned worker owns a sender until it returns Loaded. With no new
        // worker, dropping the app disconnects this isolated channel immediately.
        assert!(matches!(
            receiver.recv_timeout(Duration::from_secs(2)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        ));
    }
}

#[test]
fn reload_completion_preserves_edits_made_after_the_request() {
    let fixture = RecoveryFixture::new("reload-new-edits");
    let path = fixture.write("reload.txt", "Original A\n");
    let mut app = fixture.app();
    let id = fixture.open(&mut app, path.clone());
    let baseline = app.tab().unwrap().fingerprint.clone().unwrap();
    fs::write(&path, "Disk B\n").unwrap();
    let document = workspace::read_document(&path, MAX_DOCUMENT).unwrap();
    app.tab_mut().unwrap().loading = true;
    edit(&mut app, "New draft C 🦀\n");
    let revision = app.tab().unwrap().revision;
    app.sender
        .send(WorkerResult::Loaded {
            id,
            result: Ok(document),
            recovery: None,
        })
        .unwrap();
    app.poll();

    let tab = app.tab().unwrap();
    assert_eq!(tab.text, "New draft C 🦀\n");
    assert_eq!(tab.saved_text, "Original A\n");
    assert_eq!(tab.fingerprint.as_ref(), Some(&baseline));
    assert_eq!(tab.revision, revision);
    assert!(tab.dirty && !tab.loading);
    assert!(tab.error.as_ref().unwrap().contains("new edits"));
    assert_eq!(fs::read_to_string(path).unwrap(), "Disk B\n");
}

#[test]
fn duplicate_reload_completion_cannot_publish_after_the_buffer_is_editable() {
    let fixture = RecoveryFixture::new("reload-duplicate");
    let path = fixture.write("duplicate.txt", "Original A\n");
    let stale = workspace::read_document(&path, MAX_DOCUMENT).unwrap();
    let mut app = fixture.app();
    let id = fixture.open(&mut app, path.clone());
    fs::write(&path, "Disk B\n").unwrap();
    app.tab_mut().unwrap().loading = true;
    app.sender
        .send(WorkerResult::Loaded {
            id,
            result: workspace::read_document(&path, MAX_DOCUMENT),
            recovery: None,
        })
        .unwrap();
    app.poll();
    assert_eq!(app.tab().unwrap().text, "Disk B\n");
    assert!(!app.tab().unwrap().loading);
    let baseline = app.tab().unwrap().fingerprint.clone().unwrap();
    edit(&mut app, "New draft C Ω\n");
    let revision = app.tab().unwrap().revision;
    app.sender
        .send(WorkerResult::Loaded {
            id,
            result: Ok(stale),
            recovery: None,
        })
        .unwrap();
    app.poll();

    let tab = app.tab().unwrap();
    assert_eq!(tab.text, "New draft C Ω\n");
    assert_eq!(tab.saved_text, "Disk B\n");
    assert_eq!(tab.fingerprint.as_ref(), Some(&baseline));
    assert_eq!(tab.revision, revision);
    assert!(tab.dirty && !tab.loading);
    assert!(tab.error.is_none());
    assert_eq!(fs::read_to_string(path).unwrap(), "Disk B\n");
}

#[test]
fn automatic_disk_reload_clears_native_undo_history_without_rendering_a_ui() {
    let fixture = RecoveryFixture::new("native-history");
    let path = fixture.write("native.txt", "Original ü\n");
    let mut app = fixture.app();
    let id = fixture.open(&mut app, path.clone());
    app.settings.vim = false;
    app.context_native_undo_checkpoint(id);
    edit(&mut app, "Zuletzt gespeicherte Änderung\n");
    app.context_native_undo_checkpoint(id);
    app.save_current();
    settle_document_writes(&mut app);
    assert!(
        app.context_native_history(id).0,
        "the setup must have undo history"
    );
    let baseline = app.tab().unwrap().fingerprint.clone().unwrap();

    let external = "\u{feff}Neue externe Fassung 🦀\r\n";
    fs::write(&path, external).unwrap();
    let document = workspace::read_document(&path, MAX_DOCUMENT).unwrap();
    app.sender
        .send(WorkerResult::DiskChanges(vec![(
            id,
            baseline,
            Ok(document),
        )]))
        .unwrap();
    app.poll();
    assert_eq!(app.context_native_history(id), (false, false));
    app.context_native_undo(id, false);
    assert_eq!(app.tab().unwrap().text, "Neue externe Fassung 🦀\n");
    assert!(!app.tab().unwrap().dirty);
    assert!(app.tab().unwrap().bom);
    assert_eq!(app.tab().unwrap().line_ending, workspace::LineEnding::CrLf);
    assert_eq!(fs::read(path).unwrap(), external.as_bytes());
}

#[test]
fn undo_during_save_preserves_the_buffer_that_becomes_dirty_when_save_completes() {
    let fixture = RecoveryFixture::new("undo-during-save");
    let original = "Original ü\n";
    let path = fixture.write("undo-save.txt", original);
    let mut app = fixture.app();
    fixture.open(&mut app, path.clone());
    edit(&mut app, "Zwischenstand B 🦀\n");
    app.flush_recovery();
    let completion = hold_save_completion(&mut app);
    assert_eq!(fs::read_to_string(&path).unwrap(), "Zwischenstand B 🦀\n");

    let tab = app.tab_mut().unwrap();
    assert!(
        tab.vim
            .handle_key(&mut tab.text, VimKey::Char('u'))
            .text_changed
    );
    tab.changed();
    assert_eq!(tab.text, original);
    assert!(!tab.dirty);
    assert!(tab.saving);
    app.autosave();
    app.flush_recovery();
    wait_for(&mut app, |app| {
        app.tab().unwrap().recovery_cleanup_pending.is_none()
    });

    app.sender.send(completion).unwrap();
    app.poll();
    assert!(app.tab().unwrap().dirty);
    assert_eq!(app.tab().unwrap().text, original);
    app.tab_mut().unwrap().last_edit = Instant::now() - Duration::from_secs(2);
    app.autosave();
    settle_document_writes(&mut app);
    let recovered = crate::recovery::read_document(&app.tab().unwrap().recovery_path, MAX_DOCUMENT)
        .expect("the newly dirty buffer must have a recovery snapshot");
    assert_eq!(recovered.text, original);
    app.flush_recovery();
    let session = fixture.saved_session();
    drop(app);

    let restored = fixture.restore(session);
    assert_eq!(restored.tab().unwrap().text, original);
    assert_eq!(restored.tab().unwrap().saved_text, "Zwischenstand B 🦀\n");
    assert!(restored.tab().unwrap().dirty);
    assert!(restored.tab().unwrap().error.is_none());
}

#[test]
fn save_completion_rebases_newer_recovery_and_rejects_a_late_old_baseline() {
    let fixture = RecoveryFixture::new("recovery-save-baseline");
    let path = fixture.write("baseline.txt", "\u{feff}Original A ü\r\n");
    let mut app = fixture.app();
    let id = fixture.open(&mut app, path.clone());
    edit(&mut app, "Zwischenstand B Ω\n");
    let completion = hold_save_completion(&mut app);
    edit(&mut app, "Neuer Entwurf C 🦀\n");
    app.tab_mut().unwrap().last_edit = Instant::now() - Duration::from_secs(2);
    app.autosave();
    wait_for(&mut app, |app| !app.tab().unwrap().recovery_pending);
    assert!(app.tab().unwrap().saving);

    let tab = app.tab().unwrap();
    let recovery_path = tab.recovery_path.clone();
    let stale_revision = tab.revision;
    let stale_generation = tab.recovery_generation;
    let stale_bytes = fs::read(&recovery_path).unwrap();
    let stale_record = crate::recovery::read_document(&recovery_path, MAX_DOCUMENT).unwrap();
    assert_eq!(stale_record.text, "Neuer Entwurf C 🦀\n");
    let late_writer = app.recovery_writer.clone();
    let stale_order = late_writer.stamp();

    app.sender.send(completion).unwrap();
    app.poll();
    app.tab_mut().unwrap().last_edit = Instant::now() - Duration::from_secs(2);
    app.autosave();
    settle_document_writes(&mut app);
    let current = crate::recovery::read_document(&recovery_path, MAX_DOCUMENT).unwrap();
    let saved_disk = workspace::fingerprint(&path, MAX_DOCUMENT).unwrap();
    assert_eq!(current.text, "Neuer Entwurf C 🦀\n");
    assert!(current.baseline.as_ref().unwrap().same_content(&saved_disk));
    assert!(
        !stale_record
            .baseline
            .as_ref()
            .unwrap()
            .same_content(&saved_disk)
    );
    app.flush_recovery();
    let current_bytes = fs::read(&recovery_path).unwrap();

    late_writer
        .write(&recovery_path, stale_order, &stale_bytes)
        .unwrap();
    app.sender
        .send(WorkerResult::Recovered {
            id,
            revision: stale_revision,
            generation: stale_generation,
            result: Ok(()),
        })
        .unwrap();
    app.poll();
    assert_eq!(fs::read(&recovery_path).unwrap(), current_bytes);
    let session = fixture.saved_session();
    drop(app);

    let mut restored = fixture.restore(session);
    assert_eq!(restored.tab().unwrap().text, "Neuer Entwurf C 🦀\n");
    assert!(restored.tab().unwrap().dirty);
    assert!(
        restored.tab().unwrap().error.is_none(),
        "Morrow's own save is not an external conflict"
    );
    restored.save_current();
    settle_document_writes(&mut restored);
    assert_eq!(
        fs::read(path).unwrap(),
        "\u{feff}Neuer Entwurf C 🦀\r\n".as_bytes()
    );
    assert!(!restored.tab().unwrap().dirty);
}

#[test]
fn unreadable_recovery_survives_disk_reload_and_new_recovery_writes() {
    let fixture = RecoveryFixture::new("unreadable-record");
    let path = fixture.write("damaged.txt", "Disk A ü\n");
    let mut app = fixture.app();
    fixture.open(&mut app, path.clone());
    edit(&mut app, "Previous recoverable draft\n");
    app.flush_recovery();
    let original_recovery_path = app.tab().unwrap().recovery_path.clone();
    let damaged = b"Morrow recovery v1\0{incomplete metadata\nvaluable remaining draft bytes\n";
    fs::write(&original_recovery_path, damaged).unwrap();
    let session = fixture.saved_session();
    drop(app);

    let mut restored = fixture.restore(session);
    assert!(restored.tab().unwrap().error.is_some());
    let tab = restored.tab().unwrap();
    let id = tab.id;
    let baseline = tab.fingerprint.clone().unwrap();
    fs::write(&path, "\u{feff}External B Ω\r\n").unwrap();
    let document = workspace::read_document(&path, MAX_DOCUMENT).unwrap();
    restored
        .sender
        .send(WorkerResult::DiskChanges(vec![(
            id,
            baseline,
            Ok(document),
        )]))
        .unwrap();
    restored.poll();
    restored.autosave();
    restored.flush_recovery();
    assert_eq!(fs::read(&original_recovery_path).unwrap(), damaged);

    edit(&mut restored, "Fresh edits C 🦀\n");
    restored.tab_mut().unwrap().last_edit = Instant::now() - Duration::from_secs(2);
    restored.autosave();
    settle_document_writes(&mut restored);
    restored.flush_recovery();
    let new_recovery_path = restored.tab().unwrap().recovery_path.clone();
    assert_ne!(new_recovery_path, original_recovery_path);
    assert!(new_recovery_path.starts_with(&fixture.base));
    assert_eq!(fs::read(&original_recovery_path).unwrap(), damaged);
    assert_eq!(
        crate::recovery::read_document(&new_recovery_path, MAX_DOCUMENT)
            .unwrap()
            .text,
        "Fresh edits C 🦀\n",
    );
    let session = fixture.saved_session();
    assert_eq!(
        session.projects[0].tabs[0].recovery_path.as_ref(),
        Some(&new_recovery_path)
    );
    drop(restored);

    let restarted = fixture.restore(session);
    assert_eq!(restarted.tab().unwrap().text, "Fresh edits C 🦀\n");
    assert!(restarted.tab().unwrap().dirty);
    assert!(restarted.tab().unwrap().error.is_none());
    assert_eq!(fs::read(original_recovery_path).unwrap(), damaged);
}

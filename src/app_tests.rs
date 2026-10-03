use super::*;
use egui_kittest::Harness;

struct Fixture(PathBuf);

impl Fixture {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let root = std::env::temp_dir().join(format!(
            "morrow-app-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).expect("create isolated app fixture");
        Self(fs::canonicalize(root).expect("canonicalize fixture"))
    }

    fn write(&self, name: &str, text: impl AsRef<[u8]>) -> PathBuf {
        let path = self.0.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create fixture parent");
        }
        fs::write(&path, text).expect("write fixture file");
        path
    }

    fn app(&self) -> MorrowApp {
        let ctx = egui::Context::default();
        ctx.set_os(egui::os::OperatingSystem::Windows);
        let mut app = MorrowApp::empty(ctx);
        app.recovery_session_path = self.0.join("recovery/session.json");
        app.settings.autosave = false;
        app.add_project(self.0.clone());
        wait_until(&mut app, |app| app.projects.iter().all(|p| !p.indexing));
        app
    }

    fn open(&self, app: &mut MorrowApp, path: PathBuf) -> u64 {
        app.open_file(path);
        let id = app.tab().expect("opened tab").id;
        wait_until(app, |app| !app.tab().unwrap().loading);
        app.tab_mut().unwrap().recovery_path = self.0.join(format!("recovery/{id}.txt"));
        id
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let temporary_root = fs::canonicalize(std::env::temp_dir()).expect("temporary root");
        assert!(self.0.starts_with(&temporary_root));
        assert!(
            self.0
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("morrow-app-")
        );
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn wait_until(app: &mut MorrowApp, condition: impl Fn(&MorrowApp) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        app.poll();
        if condition(app) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "background app operation did not finish"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn change_tab(app: &mut MorrowApp, text: &str) {
    let tab = app.tab_mut().expect("active tab");
    tab.text = text.into();
    tab.changed();
}

fn key_event(key: Key, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}

fn shortcut_frame(app: &mut MorrowApp, key: Key, modifiers: egui::Modifiers) {
    let ctx = app.ctx.clone();
    let mut output = ctx.run_ui(
        egui::RawInput {
            events: vec![key_event(key, modifiers)],
            ..Default::default()
        },
        |_| app.shortcuts(),
    );
    output.textures_delta.clear();
}

fn editor_harness(app: MorrowApp) -> Harness<'static, MorrowApp> {
    let mut harness = Harness::builder()
        .with_size([1200.0, 760.0])
        .with_os(egui::os::OperatingSystem::Windows)
        .build_ui_state(
            |ui, app: &mut MorrowApp| {
                app.ctx = ui.ctx().clone();
                app.poll();
                app.shortcuts();
                app.handle_vim();
                app.editor_ui(ui);
                app.overlays();
            },
            app,
        );
    harness.run_steps(2);
    let id = harness.state().tab().unwrap().id;
    assert!(
        harness
            .ctx
            .memory(|m| m.has_focus(egui::Id::new(("document", id))))
    );
    harness
}

fn text_key_frame(
    harness: &mut Harness<'_, MorrowApp>,
    key: Key,
    text: &str,
    modifiers: egui::Modifiers,
) {
    let mut released = key_event(key, modifiers);
    if let egui::Event::Key { pressed, .. } = &mut released {
        *pressed = false;
    }
    harness.input_mut().events.extend([
        egui::Event::ModifiersChanged(modifiers),
        key_event(key, modifiers),
        egui::Event::Text(text.into()),
        released,
        egui::Event::ModifiersChanged(egui::Modifiers::NONE),
    ]);
    harness.step();
}

#[test]
fn asynchronous_save_keeps_edits_made_while_a_save_is_in_flight() {
    let fixture = Fixture::new("save-race");
    let path = fixture.write("notes.txt", "original");
    let mut app = fixture.app();
    let id = fixture.open(&mut app, path.clone());

    change_tab(&mut app, "first edit");
    app.save_tab(id, None);
    assert!(app.tab().unwrap().saving);
    // Change the buffer before polling the worker, regardless of how quickly
    // the actual disk write completes.
    change_tab(&mut app, "newer edit");
    wait_until(&mut app, |app| !app.tab().unwrap().saving);

    assert_eq!(fs::read_to_string(&path).unwrap(), "first edit");
    assert_eq!(app.tab().unwrap().saved_text, "first edit");
    assert_eq!(app.tab().unwrap().text, "newer edit");
    assert!(app.tab().unwrap().dirty);

    app.save_tab(id, None);
    wait_until(&mut app, |app| !app.tab().unwrap().saving);
    assert_eq!(fs::read_to_string(path).unwrap(), "newer edit");
    assert!(!app.tab().unwrap().dirty);
}

#[test]
fn external_changes_are_preserved_and_conflicted_editor_edits_remain_dirty() {
    let fixture = Fixture::new("conflict");
    let path = fixture.write("notes.txt", "original");
    let mut app = fixture.app();
    let id = fixture.open(&mut app, path.clone());
    change_tab(&mut app, "my unsaved edit");
    fs::write(&path, "changed by another editor").unwrap();

    app.save_tab(id, None);
    wait_until(&mut app, |app| !app.tab().unwrap().saving);

    assert_eq!(
        fs::read_to_string(path).unwrap(),
        "changed by another editor"
    );
    let tab = app.tab().unwrap();
    assert_eq!(tab.text, "my unsaved edit");
    assert_eq!(tab.saved_text, "original");
    assert!(tab.dirty);
    assert!(tab.error.as_ref().unwrap().contains("changed on disk"));
    assert!(app.status.as_ref().unwrap().1);
}

#[test]
fn save_as_preserves_an_existing_destination_and_the_original_document() {
    let fixture = Fixture::new("save-as-conflict");
    let original = fixture.write("original.txt", "original file");
    let destination = fixture.write("existing.txt", "keep this destination");
    let mut app = fixture.app();
    let id = fixture.open(&mut app, original.clone());
    change_tab(&mut app, "new content");

    app.save_tab(id, Some(destination.clone()));
    wait_until(&mut app, |app| !app.tab().unwrap().saving);

    assert_eq!(
        fs::read_to_string(&destination).unwrap(),
        "keep this destination"
    );
    assert_eq!(fs::read_to_string(&original).unwrap(), "original file");
    assert_eq!(app.tab().unwrap().path.as_ref(), Some(&original));
    assert!(app.tab().unwrap().dirty);
    assert!(app.tab().unwrap().error.is_some());
}

#[test]
fn native_multi_file_result_opens_each_file_in_its_own_workspace() {
    let first = Fixture::new("native-files-one");
    let second = Fixture::new("native-files-two");
    let first_path = first.write("one.rs", "one");
    let second_path = second.write("two.rs", "two");
    let mut app = first.app();
    app.pick_file();
    let request = app.picker.unwrap();
    app.sender
        .send(WorkerResult::NativeDialog {
            request,
            result: Ok(Some(DialogSelection {
                paths: vec![first_path.clone(), second_path.clone()],
                overwrite: None,
            })),
        })
        .unwrap();
    wait_until(&mut app, |app| {
        app.projects.len() == 2
            && app
                .projects
                .iter()
                .flat_map(|project| &project.tabs)
                .all(|tab| !tab.loading)
    });
    assert!(app.picker.is_none());
    assert_eq!(app.projects[0].tabs[0].path.as_ref(), Some(&first_path));
    assert_eq!(app.projects[1].tabs[0].path.as_ref(), Some(&second_path));
    assert_eq!(app.tab().unwrap().text, "two");
}

#[test]
fn native_multi_folder_result_reuses_projects_and_serializes_dialogs() {
    let first = Fixture::new("native-folders-one");
    let second = Fixture::new("native-folders-two");
    let mut app = first.app();
    app.pick_project();
    let request = app.picker.unwrap();
    app.pick_file();
    assert_eq!(app.picker, Some(request));
    app.finish_native_dialog(
        request,
        Ok(Some(DialogSelection {
            paths: vec![first.0.clone(), second.0.clone()],
            overwrite: None,
        })),
    );
    assert!(app.picker.is_none());
    assert_eq!(app.projects.len(), 2);
    assert_eq!(app.project().unwrap().root, second.0);
    wait_until(&mut app, |app| {
        app.projects.iter().all(|project| !project.indexing)
    });
}

#[test]
fn native_folder_cancellation_and_stale_results_preserve_the_document() {
    let fixture = Fixture::new("native-folder-cancel");
    let path = fixture.write("notes.md", "saved");
    let mut app = fixture.app();
    fixture.open(&mut app, path);
    change_tab(&mut app, "unsaved changes");
    app.tab_mut().unwrap().set_cursor(4..4);
    let id = app.tab().unwrap().id;
    app.pick_project();
    let cancelled = app.picker.unwrap();
    app.finish_native_dialog(cancelled, Ok(None));
    app.pick_project();
    let current = app.picker.unwrap();
    app.finish_native_dialog(
        cancelled,
        Ok(Some(DialogSelection {
            paths: vec![std::env::temp_dir()],
            overwrite: None,
        })),
    );
    assert_eq!(app.picker, Some(current));
    app.finish_native_dialog(current, Ok(None));
    assert_eq!(app.projects.len(), 1);
    assert_eq!(app.tab().unwrap().id, id);
    assert_eq!(app.tab().unwrap().text, "unsaved changes");
    assert_eq!(app.tab().unwrap().selection, 4..4);
    assert!(app.tab().unwrap().dirty);
}

#[test]
fn native_save_as_overwrites_only_the_confirmed_fingerprint() {
    let fixture = Fixture::new("native-save-confirmed");
    let original = fixture.write("original.txt", "original");
    let destination = fixture.write("existing.txt", "confirmed old text");
    let mut app = fixture.app();
    let id = fixture.open(&mut app, original.clone());
    change_tab(&mut app, "updated content");
    app.request_save_as(id);
    let request = app.picker.unwrap();
    app.finish_native_dialog(
        request,
        Ok(Some(DialogSelection {
            paths: vec![destination.clone()],
            overwrite: Some(workspace::fingerprint(&destination, MAX_DOCUMENT).unwrap()),
        })),
    );
    wait_until(&mut app, |app| !app.tab().unwrap().saving);
    assert_eq!(fs::read_to_string(&destination).unwrap(), "updated content");
    assert_eq!(fs::read_to_string(original).unwrap(), "original");
    assert_eq!(app.tab().unwrap().path.as_ref(), Some(&destination));
    assert!(!app.tab().unwrap().dirty);

    change_tab(&mut app, "later edits");
    app.request_save_as(id);
    let request = app.picker.unwrap();
    let confirmed = workspace::fingerprint(&destination, MAX_DOCUMENT).unwrap();
    fs::write(&destination, "external change after confirmation").unwrap();
    app.finish_native_dialog(
        request,
        Ok(Some(DialogSelection {
            paths: vec![destination.clone()],
            overwrite: Some(confirmed),
        })),
    );
    wait_until(&mut app, |app| !app.tab().unwrap().saving);
    assert_eq!(
        fs::read_to_string(destination).unwrap(),
        "external change after confirmation"
    );
    assert_eq!(app.tab().unwrap().text, "later edits");
    assert!(app.tab().unwrap().dirty);
    assert!(app.tab().unwrap().error.is_some());
}

#[test]
fn native_save_as_cannot_replace_another_open_document() {
    let fixture = Fixture::new("native-save-open-target");
    let original = fixture.write("original.txt", "original");
    let destination = fixture.write("existing.txt", "destination");
    let mut app = fixture.app();
    fixture.open(&mut app, destination.clone());
    let id = fixture.open(&mut app, original.clone());
    change_tab(&mut app, "unsaved original");
    app.request_save_as(id);
    let request = app.picker.unwrap();
    app.finish_native_dialog(
        request,
        Ok(Some(DialogSelection {
            paths: vec![destination.clone()],
            overwrite: Some(workspace::fingerprint(&destination, MAX_DOCUMENT).unwrap()),
        })),
    );
    assert!(app.picker.is_none());
    assert!(app.save_as.is_none());
    assert_eq!(fs::read_to_string(destination).unwrap(), "destination");
    assert_eq!(app.tab().unwrap().path.as_ref(), Some(&original));
    assert!(app.tab().unwrap().dirty);
    assert!(app.status.as_ref().unwrap().0.contains("already open"));
}

#[test]
fn native_save_as_new_windows_path_reopens_the_same_tab() {
    let fixture = Fixture::new("native-save-new-identity");
    let mut app = fixture.app();
    app.new_file();
    let id = app.tab().unwrap().id;
    change_tab(&mut app, "new document");
    let destination = PathBuf::from(crate::paths::display(&fixture.0)).join("new.txt");
    app.request_save_as(id);
    let request = app.picker.unwrap();
    app.finish_native_dialog(
        request,
        Ok(Some(DialogSelection {
            paths: vec![destination.clone()],
            overwrite: None,
        })),
    );
    wait_until(&mut app, |app| !app.tab().unwrap().saving);
    assert!(!app.tab().unwrap().dirty);
    assert_eq!(
        app.tab().unwrap().path.as_ref(),
        Some(&fs::canonicalize(&destination).unwrap())
    );
    app.open_file(destination);
    assert_eq!(app.project().unwrap().tabs.len(), 1);
    assert_eq!(app.tab().unwrap().id, id);
}

#[test]
fn native_save_as_suspends_autosave_of_its_source_until_destination_is_chosen() {
    let fixture = Fixture::new("native-save-as-autosave");
    let original = fixture.write("source.txt", "original on disk");
    let destination = fixture.0.join("copy.txt");
    let mut app = fixture.app();
    let id = fixture.open(&mut app, original.clone());
    app.settings.autosave = true;
    change_tab(&mut app, "pending copy edits");
    app.tab_mut().unwrap().last_edit = Instant::now() - Duration::from_secs(3);
    app.request_save_as(id);
    app.autosave();
    assert!(
        !app.tab().unwrap().saving,
        "autosave must not race the pending native Save As choice"
    );
    assert_eq!(fs::read_to_string(&original).unwrap(), "original on disk");
    let request = app.picker.unwrap();
    app.finish_native_dialog(
        request,
        Ok(Some(DialogSelection {
            paths: vec![destination.clone()],
            overwrite: None,
        })),
    );
    wait_until(&mut app, |app| !app.tab().unwrap().saving);
    assert_eq!(fs::read_to_string(original).unwrap(), "original on disk");
    assert_eq!(
        fs::read_to_string(destination).unwrap(),
        "pending copy edits"
    );
    assert!(!app.tab().unwrap().dirty);
}

#[test]
fn explicit_named_save_as_during_another_dialog_does_not_become_a_regular_save() {
    let fixture = Fixture::new("native-save-as-queue-intent");
    let source = fixture.write("source.txt", "on disk");
    let mut app = fixture.app();
    let id = fixture.open(&mut app, source.clone());
    change_tab(&mut app, "unsaved source");
    app.pick_project();
    let request = app.picker.unwrap();
    app.request_save_as(id);
    assert_eq!(app.picker, Some(request));
    assert!(app.save_queue.is_empty());
    assert!(
        app.status
            .as_ref()
            .unwrap()
            .0
            .contains("current file dialog")
    );
    app.finish_native_dialog(request, Ok(None));
    app.poll();
    assert!(!app.tab().unwrap().saving);
    assert!(app.tab().unwrap().dirty);
    assert_eq!(fs::read_to_string(source).unwrap(), "on disk");
}

#[test]
fn recovery_runs_for_named_files_when_autosave_is_disabled() {
    let fixture = Fixture::new("recovery-disabled");
    let path = fixture.write("notes.txt", "on disk");
    let mut app = fixture.app();
    fixture.open(&mut app, path.clone());
    change_tab(&mut app, "private unsaved draft");
    app.tab_mut().unwrap().last_edit = Instant::now() - Duration::from_secs(2);

    app.autosave();
    wait_until(&mut app, |app| !app.tab().unwrap().recovery_pending);

    let tab = app.tab().unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "on disk");
    assert_eq!(
        crate::recovery::read_document(&tab.recovery_path, MAX_DOCUMENT)
            .unwrap()
            .text,
        "private unsaved draft"
    );
    assert!(tab.dirty);
    assert_eq!(tab.recovery_revision, tab.revision);
}

#[test]
fn slow_autosave_preserves_recovery_and_fast_autosave_uses_its_own_delay() {
    for (delay_ms, elapsed_ms, should_save) in [(5000, 1200, false), (300, 600, true)] {
        let fixture = Fixture::new("configurable-autosave");
        let path = fixture.write("notes.txt", "on disk");
        let mut app = fixture.app();
        fixture.open(&mut app, path.clone());
        app.settings.autosave = true;
        app.settings.autosave_delay_ms = delay_ms;
        change_tab(&mut app, "new draft");
        app.tab_mut().unwrap().last_edit = Instant::now() - Duration::from_millis(elapsed_ms);
        app.autosave();
        assert_eq!(app.tab().unwrap().saving, should_save);
        wait_until(&mut app, |app| {
            !app.tab().unwrap().saving && !app.tab().unwrap().recovery_pending
        });
        if should_save {
            assert_eq!(fs::read_to_string(&path).unwrap(), "new draft");
        } else {
            assert_eq!(fs::read_to_string(&path).unwrap(), "on disk");
            assert_eq!(
                crate::recovery::read_document(&app.tab().unwrap().recovery_path, MAX_DOCUMENT)
                    .unwrap()
                    .text,
                "new draft"
            );
            app.tab_mut().unwrap().last_edit = Instant::now() - Duration::from_secs(6);
            app.autosave();
            assert!(app.tab().unwrap().saving);
            wait_until(&mut app, |app| !app.tab().unwrap().saving);
            assert_eq!(fs::read_to_string(&path).unwrap(), "new draft");
        }
    }
}

#[test]
fn shutdown_recovery_keeps_latest_named_and_untitled_edits_before_debounce() {
    let fixture = Fixture::new("shutdown-recovery");
    let path = fixture.write("notes.txt", "saved version");
    let mut app = fixture.app();
    fixture.open(&mut app, path.clone());
    change_tab(&mut app, "latest named edit");
    app.new_file();
    app.tab_mut().unwrap().recovery_path = fixture.0.join("recovery/untitled.txt");
    change_tab(&mut app, "latest untitled edit");

    app.flush_recovery();

    assert_eq!(fs::read_to_string(&path).unwrap(), "saved version");
    let saved: Session =
        serde_json::from_slice(&fs::read(&app.recovery_session_path).unwrap()).unwrap();
    assert_eq!(saved.projects[0].tabs.len(), 2);
    assert_eq!(saved.autosave, Some(false));
    assert_eq!(
        crate::recovery::read_document(
            saved.projects[0].tabs[0].recovery_path.as_ref().unwrap(),
            MAX_DOCUMENT
        )
        .unwrap()
        .text,
        "latest named edit"
    );
    assert_eq!(
        crate::recovery::read_document(
            saved.projects[0].tabs[1].recovery_path.as_ref().unwrap(),
            MAX_DOCUMENT
        )
        .unwrap()
        .text,
        "latest untitled edit"
    );
}

#[test]
fn asynchronous_restore_keeps_disk_baseline_and_recovers_a_dirty_named_buffer() {
    let fixture = Fixture::new("restore");
    let path = fixture.write("notes.txt", "saved baseline");
    let baseline = workspace::read_document(&path, MAX_DOCUMENT).unwrap();
    let record = crate::recovery::encode_document(
        "recovered unsaved edits",
        Some(&baseline.fingerprint),
        baseline.line_ending,
        baseline.bom,
    )
    .unwrap();
    let recovery = fixture.write("recovery/notes.txt", record);
    let mut app = fixture.app();
    let mut tab = EditorTab::new(Some(path.clone()), "notes.txt".into());
    tab.recovery_path = recovery.clone();
    tab.loading = true;
    let id = tab.id;
    app.projects[0].tabs.push(tab);
    app.activate_tab(0, 0);
    app.load_tab(id, Some(path), Some(recovery));

    wait_until(&mut app, |app| !app.tab().unwrap().loading);

    let tab = app.tab().unwrap();
    assert_eq!(tab.text, "recovered unsaved edits");
    assert_eq!(tab.saved_text, "saved baseline");
    assert!(tab.dirty);
    assert!(tab.fingerprint.is_some());
    assert!(tab.error.is_none());
}

#[test]
fn tabs_keep_their_document_identity_when_earlier_tabs_and_projects_close() {
    let fixture = Fixture::new("stable-tabs");
    let first = fixture.write("first.txt", "first document");
    let second = fixture.write("second.txt", "second document");
    let mut app = fixture.app();
    let first_id = fixture.open(&mut app, first);
    let second_id = fixture.open(&mut app, second);

    app.request_close(CloseAction::Tab(first_id));

    assert_eq!(app.projects[0].tabs.len(), 1);
    assert_eq!(app.tab().unwrap().id, second_id);
    assert_eq!(app.tab().unwrap().text, "second document");
    assert_eq!(app.projects[0].active_tab, 0);
    assert!(app.close_action.is_none());

    let nested = fixture.0.join("second-project");
    fs::create_dir(&nested).unwrap();
    app.add_project(nested);
    wait_until(&mut app, |app| app.projects.iter().all(|p| !p.indexing));
    app.new_file();
    let project_tab_id = app.tab().unwrap().id;
    assert_ne!(project_tab_id, second_id);
    app.request_close(CloseAction::Project(0));
    assert_eq!(app.active_project, 0);
    assert_eq!(app.tab().unwrap().id, project_tab_id);
}

#[test]
fn a_dirty_tab_cannot_close_before_the_user_chooses_what_to_do() {
    let fixture = Fixture::new("close-dirty");
    let path = fixture.write("notes.txt", "saved text");
    let mut app = fixture.app();
    let id = fixture.open(&mut app, path);
    change_tab(&mut app, "unsaved text");

    app.request_close(CloseAction::Tab(id));

    assert_eq!(app.projects[0].tabs.len(), 1);
    assert_eq!(app.tab().unwrap().text, "unsaved text");
    assert!(matches!(app.close_action, Some(CloseAction::Tab(tab_id)) if tab_id == id));
}

#[test]
fn shifted_shortcuts_open_commands_and_move_to_the_previous_find_match() {
    let fixture = Fixture::new("shift-shortcuts");
    let path = fixture.write("notes.txt", "one one one");
    let mut app = fixture.app();
    fixture.open(&mut app, path);

    shortcut_frame(
        &mut app,
        Key::P,
        egui::Modifiers::COMMAND.plus(egui::Modifiers::SHIFT),
    );
    assert!(matches!(app.palette, Some(Palette::Commands)));
    app.palette = None;
    app.find_visible = true;
    app.find_text = "one".into();
    app.find_match = Some(4);
    app.tab_mut().unwrap().set_cursor(4..7);

    shortcut_frame(&mut app, Key::F3, egui::Modifiers::SHIFT);
    assert_eq!(app.find_match, Some(0));
    assert_eq!(app.tab().unwrap().selection, 0..3);
    shortcut_frame(&mut app, Key::F3, egui::Modifiers::NONE);
    assert_eq!(app.find_match, Some(4));
    assert_eq!(app.tab().unwrap().selection, 4..7);
}

#[test]
fn opening_the_same_file_by_an_equivalent_path_reuses_the_tab() {
    let fixture = Fixture::new("canonical-tabs");
    let path = fixture.write("notes.txt", "one buffer");
    let mut app = fixture.app();
    let id = fixture.open(&mut app, path);
    app.open_file(fixture.0.join(".").join("notes.txt"));
    assert_eq!(app.projects[0].tabs.len(), 1);
    assert_eq!(app.tab().unwrap().id, id);
}

#[test]
fn native_keyboard_typing_selection_and_unicode_paste_work_in_the_real_editor() {
    let fixture = Fixture::new("native-keyboard");
    let path = fixture.write("notes.txt", "");
    let mut app = fixture.app();
    fixture.open(&mut app, path);
    app.settings.vim = false;
    let mut harness = editor_harness(app);

    harness.event(egui::Event::Text("abc".into()));
    harness.step();
    assert_eq!(harness.state().tab().unwrap().text, "abc");
    assert!(harness.state().tab().unwrap().dirty);

    harness.key_press_modifiers(egui::Modifiers::COMMAND, Key::A);
    harness.step();
    assert_eq!(harness.state().tab().unwrap().selection, 0..3);
    harness.event(egui::Event::Paste("ä🙂".into()));
    harness.step();
    assert_eq!(harness.state().tab().unwrap().text, "ä🙂");

    harness.key_press(Key::ArrowLeft);
    harness.step();
    harness.event(egui::Event::Text("X".into()));
    harness.step();
    assert_eq!(harness.state().tab().unwrap().text, "äX🙂");
}

#[test]
fn vim_visual_selection_and_yank_use_unicode_character_positions() {
    let fixture = Fixture::new("vim-visual");
    let path = fixture.write("notes.txt", "ä🙂z\nnext");
    let mut app = fixture.app();
    fixture.open(&mut app, path);
    app.settings.vim = true;
    let mut harness = editor_harness(app);

    harness.event(egui::Event::Text("v".into()));
    harness.event(egui::Event::Text("l".into()));
    harness.step();
    assert_eq!(harness.state().tab().unwrap().vim.mode, VimMode::Visual);
    assert_eq!(harness.state().tab().unwrap().selection, 0..2);
    harness.event(egui::Event::Text("y".into()));
    harness.step();

    assert_eq!(harness.state().tab().unwrap().text, "ä🙂z\nnext");
    assert_eq!(harness.state().tab().unwrap().vim.mode, VimMode::Normal);
    assert_eq!(harness.state().register.as_ref().unwrap().text, "ä🙂");
    assert!(!harness.state().tab().unwrap().dirty);
}

#[test]
fn vim_replace_can_use_a_search_character_without_opening_find() {
    let fixture = Fixture::new("vim-literal");
    let path = fixture.write("notes.txt", "abc");
    let mut app = fixture.app();
    fixture.open(&mut app, path);
    app.settings.vim = true;
    let mut harness = editor_harness(app);

    harness.event(egui::Event::Text("r".into()));
    harness.step();
    harness.event(egui::Event::Text("/".into()));
    harness.step();

    assert_eq!(harness.state().tab().unwrap().text, "/bc");
    assert!(!harness.state().find_visible);
    assert!(harness.state().vim_command.is_none());
}

#[test]
fn vim_insert_paste_replaces_selection_and_undo_restores_the_insert_origin() {
    let fixture = Fixture::new("vim-paste");
    let path = fixture.write("notes.txt", "original");
    let mut app = fixture.app();
    fixture.open(&mut app, path);
    app.settings.vim = true;
    let mut harness = editor_harness(app);

    harness.event(egui::Event::Text("i".into()));
    harness.step();
    assert_eq!(harness.state().tab().unwrap().vim.mode, VimMode::Insert);
    harness.key_press_modifiers(egui::Modifiers::COMMAND, Key::A);
    harness.step();
    harness.event(egui::Event::Paste("ä🙂 replacement".into()));
    harness.step();
    assert_eq!(harness.state().tab().unwrap().text, "ä🙂 replacement");

    harness.key_press(Key::Escape);
    harness.step();
    assert_eq!(harness.state().tab().unwrap().vim.mode, VimMode::Normal);
    assert!(harness.state().tab().unwrap().vim.can_undo());
    let id = harness.state().tab().unwrap().id;
    assert!(
        harness
            .ctx
            .memory(|m| m.has_focus(egui::Id::new(("document", id))))
    );
    harness.event(egui::Event::Text("u".into()));
    harness.step();
    assert_eq!(harness.state().tab().unwrap().text, "original");
    assert!(!harness.state().tab().unwrap().dirty);
}

#[test]
fn native_undo_after_closing_an_earlier_tab_never_restores_another_file() {
    let fixture = Fixture::new("undo-isolation");
    let first = fixture.write("first.txt", "FIRST DOCUMENT");
    let second = fixture.write("second.txt", "SECOND DOCUMENT");
    let mut app = fixture.app();
    let first_id = fixture.open(&mut app, first);
    app.settings.vim = false;
    let mut harness = editor_harness(app);

    harness.key_press_modifiers(egui::Modifiers::COMMAND, Key::A);
    harness.step();
    harness.event(egui::Event::Paste("FIRST EDIT".into()));
    harness.step();
    fixture.open(harness.state_mut(), second);
    harness.run_steps(2);
    harness.key_press_modifiers(egui::Modifiers::COMMAND, Key::A);
    harness.step();
    harness.event(egui::Event::Paste("SECOND EDIT".into()));
    harness.step();
    harness
        .state_mut()
        .perform_close(CloseAction::Tab(first_id));
    harness.run_steps(2);

    harness.key_press_modifiers(egui::Modifiers::COMMAND, Key::Z);
    harness.step();
    let text = &harness.state().tab().unwrap().text;
    assert_eq!(text, "SECOND DOCUMENT");
    assert!(!text.contains("FIRST"));
}

#[test]
fn shifted_visual_line_selection_can_delete_two_lines_and_undo() {
    let fixture = Fixture::new("visual-line");
    let path = fixture.write("notes.txt", "one\ntwo\nthree");
    let mut app = fixture.app();
    fixture.open(&mut app, path);
    let mut harness = editor_harness(app);

    text_key_frame(&mut harness, Key::V, "V", egui::Modifiers::SHIFT);
    harness.event(egui::Event::Text("j".into()));
    harness.step();
    assert_eq!(harness.state().tab().unwrap().vim.mode, VimMode::VisualLine);
    assert_eq!(harness.state().tab().unwrap().selection, 0..8);
    harness.event(egui::Event::Text("d".into()));
    harness.step();
    assert_eq!(harness.state().tab().unwrap().text, "three");
    assert_eq!(harness.state().tab().unwrap().vim.mode, VimMode::Normal);

    harness.event(egui::Event::Text("u".into()));
    harness.step();
    assert_eq!(harness.state().tab().unwrap().text, "one\ntwo\nthree");
    assert!(!harness.state().tab().unwrap().dirty);
}

#[test]
fn a_yanked_line_can_be_pasted_into_another_tab_and_undone_independently() {
    let fixture = Fixture::new("shared-register");
    let source = fixture.write("source.txt", "alpha\nbeta\n");
    let destination = fixture.write("destination.txt", "target\n");
    let mut app = fixture.app();
    let source_id = fixture.open(&mut app, source);
    let mut harness = editor_harness(app);

    harness.event(egui::Event::Text("yy".into()));
    harness.step();
    assert_eq!(harness.state().register.as_ref().unwrap().text, "alpha\n");
    let destination_id = fixture.open(harness.state_mut(), destination);
    harness.run_steps(2);
    harness.event(egui::Event::Text("p".into()));
    harness.step();
    assert_eq!(harness.state().tab().unwrap().text, "target\nalpha\n");
    assert_eq!(harness.state().projects[0].tabs[0].id, source_id);
    assert_eq!(harness.state().projects[0].tabs[0].text, "alpha\nbeta\n");

    harness.event(egui::Event::Text("u".into()));
    harness.step();
    assert_eq!(harness.state().tab().unwrap().id, destination_id);
    assert_eq!(harness.state().tab().unwrap().text, "target\n");
}

#[test]
fn counted_motion_and_dot_repeat_include_text_inserted_by_the_native_widget() {
    let fixture = Fixture::new("dot-native");
    let path = fixture.write("notes.txt", "one two three four");
    let mut app = fixture.app();
    fixture.open(&mut app, path);
    let mut harness = editor_harness(app);

    harness.event(egui::Event::Text("2w".into()));
    harness.step();
    assert_eq!(harness.state().tab().unwrap().vim.cursor, 8);
    harness.event(egui::Event::Text("cw".into()));
    harness.step();
    assert_eq!(harness.state().tab().unwrap().vim.mode, VimMode::Insert);
    harness.event(egui::Event::Text("new".into()));
    harness.step();
    harness.key_press(Key::Escape);
    harness.step();
    assert_eq!(harness.state().tab().unwrap().text, "one two new four");

    harness.event(egui::Event::Text("w.".into()));
    harness.step();
    assert_eq!(harness.state().tab().unwrap().text, "one two new new");
    assert_eq!(harness.state().tab().unwrap().vim.mode, VimMode::Normal);
}

#[test]
fn escape_closes_only_the_top_palette_and_keeps_insert_mode_and_find() {
    let fixture = Fixture::new("popup-escape");
    let path = fixture.write("notes.txt", "document");
    let mut app = fixture.app();
    fixture.open(&mut app, path);
    let mut harness = editor_harness(app);
    harness.event(egui::Event::Text("i".into()));
    harness.step();
    harness.state_mut().find_visible = true;
    harness.key_press_modifiers(egui::Modifiers::COMMAND, Key::P);
    harness.step();
    assert!(matches!(harness.state().palette, Some(Palette::Files)));

    harness.key_press(Key::Escape);
    harness.step();
    harness.run_steps(2);

    assert!(harness.state().palette.is_none());
    assert!(harness.state().find_visible);
    assert_eq!(harness.state().tab().unwrap().vim.mode, VimMode::Insert);
    assert_eq!(harness.state().tab().unwrap().text, "document");
}

#[test]
fn mode_changes_in_one_input_frame_do_not_duplicate_or_lose_insert_text() {
    let fixture = Fixture::new("batched-insert");
    let path = fixture.write("notes.txt", "tail");
    let mut app = fixture.app();
    fixture.open(&mut app, path);
    let mut harness = editor_harness(app);

    harness.input_mut().events.extend([
        egui::Event::Text("i".into()),
        egui::Event::Text("hello".into()),
    ]);
    harness.step();

    assert_eq!(harness.state().tab().unwrap().vim.mode, VimMode::Insert);
    assert_eq!(harness.state().tab().unwrap().text, "hellotail");
}

#[test]
fn escape_then_motion_in_one_input_frame_does_not_type_the_motion() {
    let fixture = Fixture::new("batched-escape");
    let path = fixture.write("notes.txt", "one\ntwo");
    let mut app = fixture.app();
    fixture.open(&mut app, path);
    let mut harness = editor_harness(app);
    harness.event(egui::Event::Text("i".into()));
    harness.step();

    harness.input_mut().events.extend([
        key_event(Key::Escape, egui::Modifiers::NONE),
        egui::Event::Text("j".into()),
    ]);
    harness.step();

    assert_eq!(harness.state().tab().unwrap().vim.mode, VimMode::Normal);
    assert_eq!(harness.state().tab().unwrap().text, "one\ntwo");
    assert_eq!(
        editor::line_col(
            &harness.state().tab().unwrap().text,
            harness.state().tab().unwrap().vim.cursor
        )
        .0,
        2
    );
}

#[test]
fn going_to_the_end_of_a_long_document_scrolls_the_cursor_into_view() {
    let fixture = Fixture::new("long-scroll");
    let text = (0..3000)
        .map(|line| format!("row {line:04}: {}", "readable source ".repeat(6)))
        .collect::<Vec<_>>()
        .join("\n");
    let path = fixture.write("large.txt", &text);
    let mut app = fixture.app();
    fixture.open(&mut app, path);
    let mut harness = editor_harness(app);

    text_key_frame(&mut harness, Key::G, "G", egui::Modifiers::SHIFT);
    harness.run_steps(3);
    let cursor = harness.state().tab().unwrap().vim.cursor;
    assert_eq!(editor::line_col(&text, cursor).0, 3000);
    let (paint, clip) = harness
        .output()
        .shapes
        .iter()
        .find_map(|shape| {
            if let egui::epaint::Shape::Text(paint) = &shape.shape {
                (paint.galley.job.text == text).then_some((paint, shape.clip_rect))
            } else {
                None
            }
        })
        .expect("painted document galley");
    let cursor_rect = paint
        .galley
        .pos_from_cursor(egui::text::CCursor::new(cursor))
        .translate(paint.pos.to_vec2());
    assert!(
        clip.contains(cursor_rect.center()),
        "cursor {cursor_rect:?} is outside editor viewport {clip:?}"
    );
    let interactive = harness
        .state()
        .editor_rect
        .expect("editor interaction area");
    let window = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 760.0));
    assert!(
        window.contains_rect(interactive),
        "editor pointer handling extends outside the visible window: {interactive:?}"
    );
}

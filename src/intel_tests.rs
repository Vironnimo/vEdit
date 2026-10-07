use super::*;
use crate::lsp::{CompletionItem, LspEvent, Position, TextEdit, TextRange};
use egui_kittest::Harness;

struct IntelFixture(PathBuf);

impl IntelFixture {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let path = std::env::temp_dir().join(format!(
            "morrow-intel-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }

    fn app(&self, text: &str) -> (MorrowApp, PathBuf) {
        let path = self.0.join("source.rs");
        fs::write(&path, text).unwrap();
        let mut app = MorrowApp::empty(egui::Context::default());
        app.settings.autosave = false;
        app.recovery_session_path = self.0.join("recovery/session.json");
        app.add_project(self.0.clone());
        app.open_file(path.clone());
        let deadline = Instant::now() + Duration::from_secs(8);
        while app.project().unwrap().indexing || app.tab().unwrap().loading {
            app.poll();
            assert!(Instant::now() < deadline, "fixture did not finish loading");
            std::thread::sleep(Duration::from_millis(5));
        }
        app.tab_mut().unwrap().recovery_path = self.0.join("recovery/source.txt");
        (app, path)
    }
}

impl Drop for IntelFixture {
    fn drop(&mut self) {
        let root = fs::canonicalize(std::env::temp_dir()).unwrap();
        assert!(self.0.starts_with(&root));
        assert!(
            self.0
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("morrow-intel-")
        );
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn item(insert: &str) -> CompletionItem {
    CompletionItem {
        label: insert.into(),
        detail: None,
        insert_text: insert.into(),
        text_edit: None,
        additional_text_edits: Vec::new(),
    }
}

fn range(line: u32, start: u32, end: u32) -> TextRange {
    TextRange {
        start: Position {
            line,
            character: start,
        },
        end: Position {
            line,
            character: end,
        },
    }
}

fn popup(app: &mut MorrowApp, items: Vec<CompletionItem>) {
    let tab = app.tab().unwrap();
    let path = tab.path.clone().unwrap();
    let id = tab.id;
    let revision = tab.revision;
    let cursor = tab.vim.cursor;
    app.projects[0]
        .rust_language_mut()
        .opened
        .insert(path, (revision, 7));
    app.completions = Some(CompletionPopup {
        tab_id: id,
        version: 7,
        cursor,
        items,
        selected: 0,
    });
}

fn fake_language(app: &mut MorrowApp) -> mpsc::SyncSender<LspEvent> {
    let (server, sender) = crate::lsp::LanguageServer::test_channel();
    app.projects[0].rust_language_mut().server = Some(server);
    app.projects[0].rust_language_mut().ready = true;
    let tab = app.tab().unwrap();
    let path = tab.path.clone().unwrap();
    let revision = tab.revision;
    app.projects[0]
        .rust_language_mut()
        .opened
        .insert(path, (revision, 7));
    app.completion_request = Some(41);
    sender
}

fn completion_event(app: &MorrowApp) -> LspEvent {
    let tab = app.tab().unwrap();
    LspEvent::Completions {
        request_id: 41,
        path: tab.path.clone().unwrap(),
        version: 7,
        position: crate::lsp::char_to_position(&tab.text, tab.vim.cursor),
        items: vec![item("complete")],
    }
}

fn key(key: Key) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}

fn keyboard_harness(app: MorrowApp) -> Harness<'static, MorrowApp> {
    let mut harness = Harness::builder()
        .with_size([1200.0, 760.0])
        .build_ui_state(
            |ui, app: &mut MorrowApp| {
                app.ctx = ui.ctx().clone();
                app.shortcuts();
                app.handle_vim();
                app.editor_ui(ui);
                app.overlays();
            },
            app,
        );
    harness.run_steps(2);
    harness.event(egui::Event::Text("i".into()));
    harness.step();
    assert_eq!(harness.state().tab().unwrap().vim.mode, VimMode::Insert);
    harness
}

#[test]
fn utf16_completion_and_additional_imports_update_the_buffer_and_cursor_together() {
    let fixture = IntelFixture::new("utf16-import");
    let original = "// header\n🙂 fo\n";
    let (mut app, _) = fixture.app(original);
    let cursor = original.chars().count() - 1;
    app.tab_mut().unwrap().set_cursor(cursor..cursor);
    let import = "use crate::Thing;\n";
    let mut completion = item("foo()");
    completion.text_edit = Some(TextEdit {
        range: range(1, 3, 5),
        new_text: "foo()".into(),
    });
    completion.additional_text_edits.push(TextEdit {
        range: range(0, 0, 0),
        new_text: import.into(),
    });
    popup(&mut app, vec![completion]);

    app.apply_completion(0);

    let expected = format!("{import}// header\n🙂 foo()\n");
    let expected_cursor = expected[..expected.find("foo()").unwrap()].chars().count() + 5;
    let tab = app.tab().unwrap();
    assert_eq!(tab.text, expected);
    assert_eq!(tab.vim.cursor, expected_cursor);
    assert_eq!(tab.selection, expected_cursor..expected_cursor);
    assert_eq!(tab.saved_text, original);
    assert!(tab.dirty);
    assert!(app.completions.is_none());
}

#[test]
fn completion_fallback_replaces_only_the_identifier_prefix_before_the_cursor() {
    let fixture = IntelFixture::new("fallback");
    let (mut app, _) = fixture.app("🙂 pri + suffix");
    app.tab_mut().unwrap().set_cursor(5..5);
    popup(&mut app, vec![item("println!")]);

    app.apply_completion(0);

    assert_eq!(app.tab().unwrap().text, "🙂 println! + suffix");
    assert_eq!(app.tab().unwrap().vim.cursor, 10);
}

#[test]
fn additional_edits_after_the_main_replacement_do_not_shift_its_cursor() {
    let fixture = IntelFixture::new("multiple-edits");
    let (mut app, _) = fixture.app("old\n🙂 fo\nold_end\n");
    app.tab_mut().unwrap().set_cursor(8..8);
    let mut completion = item("foo()");
    completion.text_edit = Some(TextEdit {
        range: range(1, 3, 5),
        new_text: "foo()".into(),
    });
    completion.additional_text_edits = vec![
        TextEdit {
            range: range(0, 0, 3),
            new_text: "new_name".into(),
        },
        TextEdit {
            range: range(2, 0, 7),
            new_text: "tail".into(),
        },
    ];
    popup(&mut app, vec![completion]);

    app.apply_completion(0);

    assert_eq!(app.tab().unwrap().text, "new_name\n🙂 foo()\ntail\n");
    assert_eq!(app.tab().unwrap().vim.cursor, 16);
}

#[test]
fn current_completion_result_is_accepted_but_old_request_and_version_are_ignored() {
    let fixture = IntelFixture::new("response-version");
    let (mut app, _) = fixture.app("prefix");
    app.tab_mut().unwrap().set_cursor(3..3);
    let sender = fake_language(&mut app);
    let mut wrong_request = completion_event(&app);
    if let LspEvent::Completions { request_id, .. } = &mut wrong_request {
        *request_id = 40;
    }
    sender.send(wrong_request).unwrap();
    app.poll_languages();
    assert!(app.completions.is_none());
    let mut wrong_version = completion_event(&app);
    if let LspEvent::Completions { version, .. } = &mut wrong_version {
        *version = 6;
    }
    sender.send(wrong_version).unwrap();
    app.poll_languages();
    assert!(app.completions.is_none());

    sender.send(completion_event(&app)).unwrap();
    app.poll_languages();
    assert_eq!(app.completions.as_ref().unwrap().version, 7);
    assert_eq!(app.completions.as_ref().unwrap().items[0].label, "complete");
}

#[test]
fn a_result_received_after_editing_or_moving_the_cursor_cannot_reopen_completion() {
    let fixture = IntelFixture::new("late-response");
    let (mut app, _) = fixture.app("prefix");
    app.tab_mut().unwrap().set_cursor(3..3);
    let sender = fake_language(&mut app);
    let old_event = completion_event(&app);
    app.tab_mut().unwrap().set_cursor(4..4);
    sender.send(old_event).unwrap();
    app.poll_languages();
    assert!(app.completions.is_none());

    let old_event = completion_event(&app);
    let tab = app.tab_mut().unwrap();
    tab.text.push('x');
    tab.changed();
    sender.send(old_event).unwrap();
    app.poll_languages();
    assert!(app.completions.is_none());
}

#[test]
fn switching_tabs_cancels_both_a_visible_popup_and_a_late_response() {
    let fixture = IntelFixture::new("tab-cancel");
    let (mut app, _) = fixture.app("prefix");
    let sender = fake_language(&mut app);
    let old_event = completion_event(&app);
    popup(&mut app, vec![item("complete")]);
    app.new_file();

    sender.send(old_event).unwrap();
    app.poll_languages();

    assert!(app.completions.is_none());
    assert_eq!(app.tab().unwrap().text, "");
    assert_eq!(app.projects[0].tabs[0].text, "prefix");
}

#[test]
fn accepting_a_popup_after_the_buffer_changed_cannot_apply_stale_ranges() {
    let fixture = IntelFixture::new("accept-stale");
    let (mut app, _) = fixture.app("prefix");
    app.tab_mut().unwrap().set_cursor(3..3);
    popup(&mut app, vec![item("completed")]);
    let tab = app.tab_mut().unwrap();
    tab.text = "different text".into();
    tab.changed();

    app.apply_completion(0);

    assert_eq!(app.tab().unwrap().text, "different text");
}

#[test]
fn disk_reload_updates_a_clean_document_and_preserves_encoding() {
    let fixture = IntelFixture::new("clean-reload");
    let (mut app, path) = fixture.app("original\n");
    let tab = app.tab().unwrap();
    let id = tab.id;
    let baseline = tab.fingerprint.clone().unwrap();
    fs::write(&path, b"\xef\xbb\xbfnew content\r\n").unwrap();
    let document = workspace::read_document(&path, MAX_DOCUMENT).unwrap();
    let fingerprint = document.fingerprint.clone();
    app.disk_check_running = true;
    app.sender
        .send(WorkerResult::DiskChanges(vec![(
            id,
            baseline,
            Ok(document),
        )]))
        .unwrap();

    app.poll();

    let tab = app.tab().unwrap();
    assert_eq!(tab.text, "new content\n");
    assert_eq!(tab.saved_text, "new content\n");
    assert_eq!(tab.fingerprint.as_ref(), Some(&fingerprint));
    assert_eq!(tab.line_ending, workspace::LineEnding::CrLf);
    assert!(tab.bom);
    assert!(!tab.dirty);
    assert!(!app.disk_check_running);
}

#[test]
fn disk_reload_never_overwrites_a_dirty_buffer_or_adopts_the_external_baseline() {
    let fixture = IntelFixture::new("dirty-reload");
    let (mut app, path) = fixture.app("original");
    let tab = app.tab_mut().unwrap();
    let id = tab.id;
    let baseline = tab.fingerprint.clone().unwrap();
    tab.text = "my unsaved edits".into();
    tab.changed();
    fs::write(&path, "external replacement").unwrap();
    let document = workspace::read_document(&path, MAX_DOCUMENT).unwrap();
    app.sender
        .send(WorkerResult::DiskChanges(vec![(
            id,
            baseline.clone(),
            Ok(document),
        )]))
        .unwrap();

    app.poll();

    let tab = app.tab().unwrap();
    assert_eq!(tab.text, "my unsaved edits");
    assert_eq!(tab.saved_text, "original");
    assert_eq!(tab.fingerprint.as_ref(), Some(&baseline));
    assert!(tab.dirty);
    assert!(tab.error.as_ref().unwrap().contains("changed outside"));
}

#[test]
fn an_old_disk_scan_cannot_replace_a_document_after_a_newer_save_completed() {
    let fixture = IntelFixture::new("stale-disk-scan");
    let (mut app, path) = fixture.app("original");
    let tab = app.tab().unwrap();
    let id = tab.id;
    let baseline = tab.fingerprint.clone().unwrap();
    fs::write(&path, "older external text").unwrap();
    let old_document = workspace::read_document(&path, MAX_DOCUMENT).unwrap();
    fs::write(&path, "new saved text").unwrap();
    let current = workspace::read_document(&path, MAX_DOCUMENT).unwrap();
    let tab = app.tab_mut().unwrap();
    tab.text = "new saved text".into();
    tab.changed();
    tab.saving = true;
    app.sender
        .send(WorkerResult::Saved {
            id,
            path,
            text: current.text,
            result: Ok(workspace::SavedDocument {
                fingerprint: current.fingerprint,
                line_ending: current.line_ending,
                format_fallback: false,
            }),
        })
        .unwrap();
    app.sender
        .send(WorkerResult::DiskChanges(vec![(
            id,
            baseline,
            Ok(old_document),
        )]))
        .unwrap();

    app.poll();

    assert_eq!(app.tab().unwrap().text, "new saved text");
    assert_eq!(app.tab().unwrap().saved_text, "new saved text");
    assert!(!app.tab().unwrap().dirty);
}

#[test]
fn clean_reload_after_a_transient_error_clears_the_error_and_clamps_the_cursor() {
    let fixture = IntelFixture::new("reload-cursor");
    let (mut app, path) = fixture.app("a document with a long last line");
    let tab = app.tab_mut().unwrap();
    let id = tab.id;
    let baseline = tab.fingerprint.clone().unwrap();
    tab.set_cursor(25..25);
    tab.error = Some("temporary read error".into());
    fs::write(&path, "hi").unwrap();
    let document = workspace::read_document(&path, MAX_DOCUMENT).unwrap();
    app.sender
        .send(WorkerResult::DiskChanges(vec![(
            id,
            baseline,
            Ok(document),
        )]))
        .unwrap();

    app.poll();

    let tab = app.tab().unwrap();
    assert_eq!(tab.text, "hi");
    assert!(tab.error.is_none());
    assert!(tab.vim.cursor < tab.text.chars().count());
    assert!(tab.pending_selection.as_ref().unwrap().end <= tab.text.chars().count());
}

#[test]
fn enter_in_the_command_palette_cannot_accept_an_editor_completion() {
    let fixture = IntelFixture::new("completion-palette");
    let (mut app, _) = fixture.app("prefix");
    app.tab_mut().unwrap().set_cursor(3..3);
    popup(&mut app, vec![item("completed")]);
    app.palette = Some(Palette::Commands);
    let ctx = app.ctx.clone();
    let mut intercepted = false;
    let mut output = ctx.run_ui(
        egui::RawInput {
            events: vec![key(Key::Enter)],
            ..Default::default()
        },
        |_| {
            intercepted = app.completion_keys();
        },
    );
    output.textures_delta.clear();
    assert!(!intercepted);
    assert_eq!(app.tab().unwrap().text, "prefix");
}

#[test]
fn enter_in_a_find_field_cannot_accept_an_editor_completion() {
    let fixture = IntelFixture::new("completion-focus");
    let (mut app, _) = fixture.app("prefix");
    app.tab_mut().unwrap().set_cursor(3..3);
    popup(&mut app, vec![item("completed")]);
    let ctx = app.ctx.clone();
    ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("find-field")));
    let mut intercepted = false;
    let mut output = ctx.run_ui(
        egui::RawInput {
            events: vec![key(Key::Enter)],
            ..Default::default()
        },
        |_| {
            intercepted = app.completion_keys();
        },
    );
    output.textures_delta.clear();
    assert!(!intercepted);
    assert_eq!(app.tab().unwrap().text, "prefix");
}

#[test]
fn text_enter_and_more_text_in_one_frame_keep_their_chronological_order() {
    let fixture = IntelFixture::new("text-enter-order");
    let (app, _) = fixture.app("");
    let mut harness = keyboard_harness(app);
    harness.input_mut().events.extend([
        egui::Event::Text("foo".into()),
        key(Key::Enter),
        egui::Event::Text("bar".into()),
    ]);
    harness.step();
    // TextEdit renders the native prefix before the queued Enter and suffix.
    assert!(harness.state().editor_input_pending());
    super::editor_input_tests::settle(&mut harness);
    assert_eq!(harness.state().tab().unwrap().text, "foo\nbar");
}

#[test]
fn native_text_before_escape_in_one_frame_is_committed_and_undoable() {
    let fixture = IntelFixture::new("text-escape-order");
    let (app, _) = fixture.app("tail");
    let mut harness = keyboard_harness(app);
    harness
        .input_mut()
        .events
        .extend([egui::Event::Text("foo".into()), key(Key::Escape)]);
    harness.step();
    // Escape commits only after TextEdit has applied the preceding text.
    assert!(harness.state().editor_input_pending());
    super::editor_input_tests::settle(&mut harness);
    assert_eq!(harness.state().tab().unwrap().text, "footail");
    assert_eq!(harness.state().tab().unwrap().vim.mode, VimMode::Normal);
    harness.event(egui::Event::Text("u".into()));
    harness.step();
    assert_eq!(harness.state().tab().unwrap().text, "tail");
}

#[test]
fn focused_editor_accepts_completion_without_inserting_the_enter_key() {
    let fixture = IntelFixture::new("completion-key");
    let (mut app, _) = fixture.app("pri");
    app.tab_mut().unwrap().set_cursor(0..0);
    let mut harness = keyboard_harness(app);
    harness.state_mut().tab_mut().unwrap().set_cursor(3..3);
    harness.step();
    popup(harness.state_mut(), vec![item("println!")]);

    harness.event(key(Key::Enter));
    harness.step();

    assert_eq!(harness.state().tab().unwrap().text, "println!");
    assert_eq!(harness.state().tab().unwrap().vim.cursor, 8);
    assert!(harness.state().completions.is_none());
}

#[test]
fn one_undo_restores_the_main_completion_and_its_additional_imports() {
    let fixture = IntelFixture::new("completion-undo");
    let original = "🙂 fo\n";
    let (mut app, _) = fixture.app(original);
    app.tab_mut().unwrap().set_cursor(4..4);
    let mut completion = item("foo()");
    completion.text_edit = Some(TextEdit {
        range: range(0, 3, 5),
        new_text: "foo()".into(),
    });
    completion.additional_text_edits.push(TextEdit {
        range: range(0, 0, 0),
        new_text: "use crate::Thing;\n".into(),
    });
    let mut harness = keyboard_harness(app);
    popup(harness.state_mut(), vec![completion]);
    harness.event(key(Key::Enter));
    harness.step();
    assert_eq!(
        harness.state().tab().unwrap().text,
        "use crate::Thing;\n🙂 foo()\n"
    );
    assert!(harness.state().completions.is_none());
    let document_id = egui::Id::new(("document", harness.state().tab().unwrap().id));
    assert!(harness.state().ctx.memory(|m| m.has_focus(document_id)));

    harness.event(key(Key::Escape));
    harness.step();
    assert_eq!(harness.state().tab().unwrap().vim.mode, VimMode::Normal);
    assert!(harness.state().tab().unwrap().vim.can_undo());
    harness.event(egui::Event::Text("u".into()));
    harness.step();

    assert_eq!(harness.state().tab().unwrap().text, original);
    assert!(!harness.state().tab().unwrap().dirty);
}

#[test]
fn clean_external_reload_discards_history_that_could_restore_an_old_document() {
    let fixture = IntelFixture::new("reload-undo");
    let (mut app, path) = fixture.app("initial text");
    let tab = app.tab_mut().unwrap();
    tab.vim.begin_external_edit(&tab.text);
    tab.text = "last saved text".into();
    tab.saved_text = tab.text.clone();
    tab.changed();
    assert!(tab.vim.can_undo());
    let id = tab.id;
    let baseline = tab.fingerprint.clone().unwrap();
    fs::write(&path, "external text").unwrap();
    let document = workspace::read_document(&path, MAX_DOCUMENT).unwrap();
    app.sender
        .send(WorkerResult::DiskChanges(vec![(
            id,
            baseline,
            Ok(document),
        )]))
        .unwrap();
    app.poll();
    assert!(!app.tab().unwrap().vim.can_undo());

    let tab = app.tab_mut().unwrap();
    let result = tab
        .vim
        .handle_key(&mut tab.text, crate::vim::VimKey::Char('u'));
    assert!(!result.text_changed);
    assert_eq!(tab.text, "external text");
    assert_eq!(tab.saved_text, "external text");
    assert!(!tab.dirty);
}

#[test]
fn save_all_prompts_for_multiple_untitled_documents_in_order() {
    let fixture = IntelFixture::new("save-all-queue");
    let (mut app, _) = fixture.app("saved source");
    let mut ids = Vec::new();
    for index in 1..=3 {
        app.new_file();
        let tab = app.tab_mut().unwrap();
        tab.text = format!("draft {index}");
        tab.recovery_path = fixture.0.join(format!("recovery/draft{index}.txt"));
        tab.changed();
        ids.push(tab.id);
    }
    app.save_all();
    assert_eq!(app.save_as, Some(ids[0]));
    assert_eq!(app.save_queue.iter().copied().collect::<Vec<_>>(), ids[1..]);
    for (index, id) in ids.iter().enumerate() {
        assert_eq!(app.save_as, Some(*id));
        let request = app.picker.expect("native save dialog request");
        assert_eq!(request.purpose, DialogPurpose::SaveAs(*id));
        app.sender
            .send(WorkerResult::NativeDialog {
                request,
                result: Ok(Some(DialogSelection {
                    paths: vec![fixture.0.join(format!("draft{index}.txt"))],
                    overwrite: None,
                })),
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            app.poll();
            if app
                .tab_by_id_mut(*id)
                .is_some_and(|tab| !tab.saving && !tab.dirty)
            {
                break;
            }
            assert!(Instant::now() < deadline, "queued untitled save failed");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            fs::read_to_string(fixture.0.join(format!("draft{index}.txt"))).unwrap(),
            format!("draft {}", index + 1)
        );
    }
    assert!(app.save_as.is_none());
    assert!(app.save_queue.is_empty());
}

#[test]
fn cancelling_a_queued_save_preserves_all_remaining_untitled_edits() {
    let fixture = IntelFixture::new("save-all-cancel");
    let (mut app, _) = fixture.app("saved source");
    for index in 1..=2 {
        app.new_file();
        let tab = app.tab_mut().unwrap();
        tab.text = format!("unsaved {index}");
        tab.recovery_path = fixture.0.join(format!("recovery/draft{index}.txt"));
        tab.changed();
    }
    app.save_all();
    assert!(app.save_as.is_some());
    assert_eq!(app.save_queue.len(), 1);

    let request = app.picker.unwrap();
    app.sender
        .send(WorkerResult::NativeDialog {
            request,
            result: Ok(None),
        })
        .unwrap();
    app.poll();

    assert!(app.save_as.is_none());
    assert!(app.save_queue.is_empty());
    assert_eq!(app.projects[0].tabs.len(), 3);
    for index in 1..=2 {
        let tab = &app.projects[0].tabs[index];
        assert_eq!(tab.text, format!("unsaved {index}"));
        assert!(tab.dirty);
        assert!(tab.path.is_none());
    }
}

#[test]
fn one_escape_closes_completion_and_returns_to_vim_normal() {
    let fixture = IntelFixture::new("completion-escape");
    let (app, _) = fixture.app("hello");
    let mut harness = keyboard_harness(app);
    popup(harness.state_mut(), vec![item("hello_world")]);
    harness.event(key(Key::Escape));
    harness.step();
    assert!(harness.state().completions.is_none());
    assert_eq!(harness.state().tab().unwrap().vim.mode, VimMode::Normal);
    harness.event(egui::Event::Text("l".into()));
    harness.step();
    assert_eq!(harness.state().tab().unwrap().text, "hello");
}

#[test]
fn opening_a_file_argument_reuses_its_existing_project() {
    let fixture = IntelFixture::new("arguments");
    let (mut app, _) = fixture.app("hello");
    let nested = fixture.0.join("src/nested.rs");
    fs::create_dir_all(nested.parent().unwrap()).unwrap();
    fs::write(&nested, "fn nested() {}\n").unwrap();
    app.open_argument(nested.clone());
    assert_eq!(app.projects.len(), 1);
    assert_eq!(app.projects[0].tabs.len(), 2);
    assert_eq!(app.tab().unwrap().path.as_ref(), Some(&nested));
}

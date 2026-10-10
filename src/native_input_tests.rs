//! Desktop clipboard/IME events must follow Vim transactions and focus ownership.

use super::*;
use egui_kittest::Harness;

const PRESERVED_REGISTER: &str = "KEEP🦀";

fn fixture(document: &str, vim: bool) -> VEditApp {
    let mut app = VEditApp::empty(egui::Context::default());
    app.settings.autosave = false;
    app.settings.vim = vim;
    app.sidebar_visible = false;
    let root = std::env::temp_dir().join("vedit-native-input-render-only");
    let mut project = Project::new(root.clone());
    project.last_git_refresh = Instant::now();
    let mut tab = EditorTab::new(Some(root.join("notes.txt")), "notes.txt".into());
    tab.text = document.into();
    tab.saved_text = document.into();
    let register = Yank {
        text: PRESERVED_REGISTER.into(),
        linewise: false,
    };
    tab.vim.set_register(register.clone());
    app.register = Some(register);
    project.tabs.push(tab);
    app.projects.push(project);
    app.bind_current_pane();
    app
}

fn harness(app: VEditApp) -> Harness<'static, VEditApp> {
    let mut screen = Harness::builder()
        .with_size([1200.0, 760.0])
        .with_os(egui::os::OperatingSystem::Windows)
        .build_ui_state(
            |ui, app: &mut VEditApp| {
                app.ctx = ui.ctx().clone();
                app.shortcuts();
                app.handle_vim();
                app.editor_ui(ui);
                app.overlays();
            },
            app,
        );
    set_style(&screen.ctx);
    screen.run_steps(3);
    screen
}

fn events(screen: &mut Harness<'_, VEditApp>, events: impl IntoIterator<Item = egui::Event>) {
    screen.input_mut().events.extend(events);
    screen.run_steps(1);
    super::editor_input_tests::settle(screen);
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

fn assert_unedited(screen: &Harness<'_, VEditApp>, document: &str, revision: u64) {
    let tab = screen.state().tab().unwrap();
    assert_eq!(tab.text, document);
    assert_eq!(tab.revision, revision);
    assert!(!tab.dirty);
    assert!(!tab.vim.can_undo());
}

fn assert_copied(screen: &Harness<'_, VEditApp>, expected: &str) {
    assert!(
        screen
            .output()
            .platform_output
            .commands
            .iter()
            .any(|command| {
                matches!(command, egui::OutputCommand::CopyText(text) if text == expected)
            })
    );
    let register = screen.state().register.as_ref().unwrap();
    assert_eq!(register.text, PRESERVED_REGISTER);
    assert!(!register.linewise);
    let local_register = screen.state().tab().unwrap().vim.register().unwrap();
    assert_eq!(local_register.text, PRESERVED_REGISTER);
    assert!(!local_register.linewise);
}

#[test]
fn native_ime_cannot_edit_normal_or_visual_source() {
    let original = "a🦀é测试 last\n";
    for visual in [false, true] {
        let mut screen = harness(fixture(original, true));
        if visual {
            events(&mut screen, [egui::Event::Text("vll".into())]);
        }
        let tab = screen.state().tab().unwrap();
        let revision = tab.revision;
        let cursor = tab.vim.cursor;
        let mode = tab.vim.mode;
        let selection = tab.selection.clone();
        events(
            &mut screen,
            [
                egui::Event::Ime(egui::ImeEvent::Preedit {
                    text: "预览".into(),
                    active_range_chars: None,
                }),
                egui::Event::Ime(egui::ImeEvent::Commit("提交🦀".into())),
            ],
        );
        screen.run_steps(2);
        assert_unedited(&screen, original, revision);
        let tab = screen.state().tab().unwrap();
        assert_eq!(tab.vim.cursor, cursor);
        assert_eq!(tab.vim.mode, mode);
        assert_eq!(tab.selection, selection);
    }
}

#[test]
fn native_ime_commit_updates_unicode_search_in_the_same_batch() {
    let original = "intro 测试 first 测试 last\n";
    let mut screen = harness(fixture(original, true));
    let revision = screen.state().tab().unwrap().revision;
    events(
        &mut screen,
        [
            egui::Event::Text("/".into()),
            egui::Event::Ime(egui::ImeEvent::Preedit {
                text: "预览".into(),
                active_range_chars: None,
            }),
            egui::Event::Ime(egui::ImeEvent::Commit("测试".into())),
        ],
    );
    screen.run_steps(2);
    assert!(screen.state().vim_search_active());
    assert_eq!(
        screen.state().tab().unwrap().vim.cursor,
        "intro ".chars().count()
    );
    assert_unedited(&screen, original, revision);
    let ime = screen
        .output()
        .platform_output
        .ime
        .as_ref()
        .expect("the final frame must enable native Windows IME for the search query");
    assert!(
        ime.rect.height() <= 24.0,
        "IME must belong to the query, not the source document"
    );
    assert!(
        ime.rect.contains_rect(ime.cursor_rect),
        "query IME caret must stay in the visible query field"
    );
    events(&mut screen, [egui::Event::Copy]);
    assert!(!screen.state().vim_search_active());
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 0);
    assert_unedited(&screen, original, revision);
}

#[test]
fn native_ime_insert_is_literal_ordered_and_one_undo_transaction() {
    let original = "tail\n";
    let mut screen = harness(fixture(original, true));
    events(
        &mut screen,
        [
            egui::Event::Text("iA".into()),
            egui::Event::Ime(egui::ImeEvent::Preedit {
                text: "预览".into(),
                active_range_chars: None,
            }),
            egui::Event::Ime(egui::ImeEvent::Commit("测试🦀/dd".into())),
            egui::Event::Text("B".into()),
            key(Key::Escape),
        ],
    );
    assert_eq!(screen.state().tab().unwrap().text, "A测试🦀/ddBtail\n");
    assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::Normal);
    events(&mut screen, [egui::Event::Text("u".into())]);
    assert_eq!(screen.state().tab().unwrap().text, original);
    assert!(!screen.state().tab().unwrap().dirty);
}

#[test]
fn native_copy_cancels_pending_operator_and_preserves_visual_payload() {
    let original = "a🦀éxyz\n";
    let mut screen = harness(fixture(original, true));
    let revision = screen.state().tab().unwrap().revision;
    events(&mut screen, [egui::Event::Text("d".into())]);
    assert_eq!(screen.state().tab().unwrap().vim.pending_command(), "d");
    events(&mut screen, [egui::Event::Copy]);
    assert!(
        screen
            .state()
            .tab()
            .unwrap()
            .vim
            .pending_command()
            .is_empty()
    );
    events(
        &mut screen,
        [egui::Event::Text("vll".into()), egui::Event::Copy],
    );
    assert_copied(&screen, "a🦀é");
    assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::Visual);
    assert_eq!(screen.state().tab().unwrap().selection, 0..3);
    assert_unedited(&screen, original, revision);
}

#[test]
fn native_copy_keeps_insert_active_and_the_group_undoable() {
    let original = "tail\n";
    let mut screen = harness(fixture(original, true));
    events(
        &mut screen,
        [
            egui::Event::Text("i".into()),
            egui::Event::Ime(egui::ImeEvent::Commit("🦀".into())),
            egui::Event::Copy,
        ],
    );
    assert_eq!(screen.state().tab().unwrap().text, "🦀tail\n");
    assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::Insert);
    events(&mut screen, [key(Key::Escape)]);
    events(&mut screen, [egui::Event::Text("u".into())]);
    assert_eq!(screen.state().tab().unwrap().text, original);
}

#[test]
fn native_cut_visual_unicode_preserves_vim_register_and_is_undoable() {
    let original = "a🦀édef\n";
    let mut screen = harness(fixture(original, true));
    events(
        &mut screen,
        [egui::Event::Text("vll".into()), egui::Event::Cut],
    );
    assert_eq!(screen.state().tab().unwrap().text, "def\n");
    assert_copied(&screen, "a🦀é");
    assert!(screen.state().tab().unwrap().vim.can_undo());
    events(&mut screen, [egui::Event::Text("u".into())]);
    assert_eq!(screen.state().tab().unwrap().text, original);
    assert!(!screen.state().tab().unwrap().dirty);
    events(&mut screen, [egui::Event::Text("0p".into())]);
    assert_eq!(screen.state().tab().unwrap().text, "aKEEP🦀🦀édef\n");
}

#[test]
fn native_cut_visual_line_keeps_linewise_register_and_undo() {
    let original = "a🦀é\nsecond\n";
    let mut screen = harness(fixture(original, true));
    events(
        &mut screen,
        [egui::Event::Text("V".into()), egui::Event::Cut],
    );
    assert_eq!(screen.state().tab().unwrap().text, "second\n");
    assert_copied(&screen, "a🦀é\n");
    assert!(!screen.state().register.as_ref().unwrap().linewise);
    events(&mut screen, [egui::Event::Text("u".into())]);
    assert_eq!(screen.state().tab().unwrap().text, original);
}

#[test]
fn native_cut_visual_selection_can_repeat_with_dot() {
    let original = "abcdefghij";
    let mut screen = harness(fixture(original, true));
    events(
        &mut screen,
        [egui::Event::Text("vll".into()), egui::Event::Cut],
    );
    assert_eq!(screen.state().tab().unwrap().text, "defghij");
    assert_copied(&screen, "abc");
    events(&mut screen, [egui::Event::Text(".".into())]);
    assert_eq!(screen.state().tab().unwrap().text, "ghij");
    assert!(
        screen
            .state()
            .tab()
            .unwrap()
            .vim
            .pending_command()
            .is_empty()
    );
    events(&mut screen, [egui::Event::Text("u".into())]);
    assert_eq!(screen.state().tab().unwrap().text, "defghij");
}

#[test]
fn native_cut_cancels_visual_replace_and_text_object_prefixes() {
    for prefix in ["vllr", "vlli"] {
        let original = "a🦀édef\n";
        let mut screen = harness(fixture(original, true));
        events(&mut screen, [egui::Event::Text(prefix.into())]);
        assert_eq!(screen.state().tab().unwrap().selection, 0..3);
        events(&mut screen, [egui::Event::Cut]);
        assert_eq!(
            screen.state().tab().unwrap().text,
            "def\n",
            "prefix {prefix}"
        );
        assert_copied(&screen, "a🦀é");
        events(&mut screen, [egui::Event::Text(".".into())]);
        assert_eq!(screen.state().tab().unwrap().text, "\n", "prefix {prefix}");
        assert!(
            screen
                .state()
                .tab()
                .unwrap()
                .vim
                .pending_command()
                .is_empty()
        );
        events(&mut screen, [egui::Event::Text("u".into())]);
        assert_eq!(screen.state().tab().unwrap().text, "def\n");
        events(&mut screen, [egui::Event::Text("u".into())]);
        assert_eq!(screen.state().tab().unwrap().text, original);
    }
}

#[test]
fn modal_dialogs_block_native_and_insert_source_input_before_overlays() {
    for vim in [false, true] {
        for modal in 0..4 {
            let original = "protected 🦀 document\n";
            let mut screen = harness(fixture(original, vim));
            if vim {
                events(&mut screen, [egui::Event::Text("i".into())]);
            }
            let revision = screen.state().tab().unwrap().revision;
            let document_id = screen.state().tab().unwrap().id;
            match modal {
                0 => screen.state_mut().settings_open = true,
                1 => screen.state_mut().diff = Some(("Review".into(), "+change\n".into())),
                2 => {
                    screen.state_mut().picker = Some(Picker {
                        id: 9,
                        purpose: DialogPurpose::Files,
                    })
                }
                _ => screen.state_mut().close_action = Some(CloseAction::Exit),
            }
            events(
                &mut screen,
                [
                    egui::Event::Text("j".into()),
                    egui::Event::Paste("leaked paste".into()),
                    egui::Event::Ime(egui::ImeEvent::Preedit {
                        text: "预览".into(),
                        active_range_chars: None,
                    }),
                    egui::Event::Ime(egui::ImeEvent::Commit("提交".into())),
                    egui::Event::Cut,
                ],
            );
            screen.run_steps(2);
            let tab = screen.state().tab().unwrap();
            assert_eq!(tab.text, original, "vim={vim}, modal={modal}");
            assert_eq!(tab.revision, revision, "vim={vim}, modal={modal}");
            assert!(!tab.dirty);
            assert!(
                !screen.ctx.memory(
                    |memory| memory.has_focus(screen.state().document_widget_id(document_id))
                )
            );
        }
    }
}

#[test]
fn readonly_normal_source_still_accepts_real_mouse_selection() {
    let original = "a🦀éxyz\n";
    let mut screen = harness(fixture(original, true));
    let shape = screen
        .output()
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(shape) if shape.galley.text() == original => Some(shape.clone()),
            _ => None,
        })
        .expect("source document is painted");
    let point = |character| {
        let rect = shape
            .galley
            .pos_from_cursor(egui::text::CCursor::new(character));
        shape.pos + egui::vec2(rect.left() + 1.0, rect.center().y)
    };
    let start = point(1);
    let end = point(3);
    screen.hover_at(start);
    screen.drag_at(start);
    screen.run_steps(1);
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 1);
    screen.hover_at(end);
    screen.run_steps(1);
    screen.drop_at(end);
    screen.run_steps(1);
    assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::Visual);
    assert_eq!(screen.state().tab().unwrap().selection, 1..3);
    events(&mut screen, [egui::Event::Copy]);
    assert_copied(&screen, "🦀é");
    assert_eq!(screen.state().tab().unwrap().text, original);
}

#[test]
fn native_cut_of_a_mouse_visual_area_repeats_without_arming_an_operator() {
    let original = "a🦀ébcd\n";
    let mut screen = harness(fixture(original, true));
    let shape = screen
        .output()
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(shape) if shape.galley.text() == original => Some(shape.clone()),
            _ => None,
        })
        .expect("source document is painted");
    let point = |character| {
        let rect = shape
            .galley
            .pos_from_cursor(egui::text::CCursor::new(character));
        shape.pos + egui::vec2(rect.left() + 1.0, rect.center().y)
    };
    let start = point(1);
    let end = point(3);
    screen.hover_at(start);
    screen.drag_at(start);
    screen.run_steps(1);
    screen.hover_at(end);
    screen.run_steps(1);
    screen.drop_at(end);
    screen.run_steps(1);
    assert_eq!(screen.state().tab().unwrap().selection, 1..3);
    events(&mut screen, [egui::Event::Cut]);
    assert_eq!(screen.state().tab().unwrap().text, "abcd\n");
    assert_copied(&screen, "🦀é");
    events(&mut screen, [egui::Event::Text(".".into())]);
    assert_eq!(screen.state().tab().unwrap().text, "ad\n");
    assert!(
        screen
            .state()
            .tab()
            .unwrap()
            .vim
            .pending_command()
            .is_empty()
    );
    events(&mut screen, [egui::Event::Text("u".into())]);
    assert_eq!(screen.state().tab().unwrap().text, "abcd\n");
    events(&mut screen, [egui::Event::Text("u".into())]);
    assert_eq!(screen.state().tab().unwrap().text, original);
}

#[test]
fn insert_text_remains_live_while_an_async_save_is_running() {
    for error in [None, Some("Save failed; use Save As".to_owned())] {
        let mut screen = harness(fixture("tail\n", true));
        events(&mut screen, [egui::Event::Text("i".into())]);
        screen.state_mut().tab_mut().unwrap().saving = true;
        screen.state_mut().tab_mut().unwrap().error = error;
        events(&mut screen, [egui::Event::Text("live 🦀 ".into())]);
        assert_eq!(screen.state().tab().unwrap().text, "live 🦀 tail\n");
        assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::Insert);
        events(
            &mut screen,
            [
                egui::Event::Key {
                    key: Key::A,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::CTRL,
                },
                egui::Event::Cut,
            ],
        );
        assert_eq!(screen.state().tab().unwrap().text, "");
        assert_copied(&screen, "live 🦀 tail\n");
        events(&mut screen, [key(Key::Escape)]);
        events(&mut screen, [egui::Event::Text("u".into())]);
        assert_eq!(screen.state().tab().unwrap().text, "tail\n");
    }
}

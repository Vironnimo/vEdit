//! Native input reaches TextEdit before application-owned editing commands.
use super::*;
use egui_kittest::Harness;

fn fixture(text: &str, vim: bool) -> MorrowApp {
    let mut app = MorrowApp::empty(egui::Context::default());
    app.settings.vim = vim;
    app.settings.autosave = false;
    app.settings.smooth_scroll = false;
    app.sidebar_visible = false;
    let mut project = Project::new(std::env::temp_dir().join("morrow-editor-input-unit"));
    let mut tab = EditorTab::new(None, "input.txt".into());
    tab.text = text.into();
    tab.saved_text = text.into();
    project.tabs.push(tab);
    app.projects.push(project);
    app.bind_current_pane();
    app
}

fn harness(text: &str, vim: bool) -> Harness<'static, MorrowApp> {
    let mut screen = Harness::builder()
        .with_size([1100.0, 740.0])
        .with_os(egui::os::OperatingSystem::Windows)
        .build_ui_state(
            |ui, app: &mut MorrowApp| {
                app.ctx = ui.ctx().clone();
                app.shortcuts();
                app.handle_vim();
                app.editor_ui(ui);
            },
            fixture(text, vim),
        );
    set_style(&screen.ctx);
    screen.run_steps(3);
    screen
}

fn key(key: Key, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers {
            command: modifiers.ctrl,
            ..modifiers
        },
    }
}

/// Custom commands resume only after their native prefix has been rendered.
/// Wait for that explicit state transition, never for animation or wall time.
pub(super) fn settle(screen: &mut Harness<'_, MorrowApp>) {
    for _ in 0..32 {
        if !screen.state().editor_input_pending() {
            return;
        }
        screen.run_steps(1);
    }
    panic!("source input continuation did not drain");
}

fn events(screen: &mut Harness<'_, MorrowApp>, events: impl IntoIterator<Item = egui::Event>) {
    screen.input_mut().events.extend(events);
    screen.run_steps(1);
    settle(screen);
}

fn text<'a>(screen: &'a Harness<'_, MorrowApp>) -> &'a str {
    &screen.state().tab().unwrap().text
}

#[test]
fn native_text_undo_text_preserves_order_and_appends_new_keyboard_input() {
    let mut screen = harness("tail", false);
    screen.input_mut().events.extend([
        egui::Event::Text("X".into()),
        key(Key::Z, egui::Modifiers::CTRL),
        egui::Event::Text("Y".into()),
    ]);
    screen.run_steps(1);
    assert_eq!(text(&screen), "Xtail");
    assert!(screen.state().editor_input_pending());
    // This later event follows the saved suffix without invalidating its owner.
    screen
        .input_mut()
        .events
        .push(egui::Event::Text("Z".into()));
    settle(&mut screen);
    assert_eq!(text(&screen), "YZtail");
    events(&mut screen, [key(Key::Z, egui::Modifiers::CTRL)]);
    assert_eq!(text(&screen), "tail");
    events(&mut screen, [key(Key::Y, egui::Modifiers::CTRL)]);
    assert_eq!(text(&screen), "YZtail");
}

#[test]
fn native_text_enter_and_tab_use_the_preceding_text_and_native_caret() {
    let mut screen = harness("", false);
    events(
        &mut screen,
        [
            egui::Event::Text("a".into()),
            key(Key::Enter, egui::Modifiers::NONE),
            egui::Event::Text("b".into()),
            key(Key::Tab, egui::Modifiers::NONE),
            egui::Event::Text("🦀".into()),
        ],
    );
    assert_eq!(text(&screen), "a\nb   🦀");
    assert_eq!(screen.state().tab().unwrap().selection, 7..7);
    events(&mut screen, [key(Key::Z, egui::Modifiers::CTRL)]);
    assert_eq!(text(&screen), "a\nb");
    events(&mut screen, [key(Key::Y, egui::Modifiers::CTRL)]);
    assert_eq!(text(&screen), "a\nb   🦀");
}

#[test]
fn a_leading_native_command_applies_immediately() {
    let mut screen = harness("tail", false);
    screen
        .input_mut()
        .events
        .push(key(Key::Enter, egui::Modifiers::NONE));
    screen.run_steps(1);
    assert_eq!(text(&screen), "\ntail");
    assert_eq!(screen.state().tab().unwrap().selection, 1..1);
    assert!(!screen.state().editor_input_pending());
    events(&mut screen, [key(Key::Z, egui::Modifiers::CTRL)]);
    assert_eq!(text(&screen), "tail");
    events(&mut screen, [key(Key::Y, egui::Modifiers::CTRL)]);
    assert_eq!(text(&screen), "\ntail");
    assert_eq!(screen.state().tab().unwrap().selection, 1..1);
}

#[test]
fn modifier_press_does_not_delay_a_command_without_native_text_before_it() {
    let mut screen = harness("tail", false);
    events(&mut screen, [egui::Event::Text("X".into())]);
    screen.input_mut().events.extend([
        key(Key::ControlLeft, egui::Modifiers::CTRL),
        key(Key::Z, egui::Modifiers::CTRL),
    ]);
    screen.run_steps(1);
    assert_eq!(text(&screen), "tail");
    assert!(!screen.state().editor_input_pending());
}

#[test]
fn vim_native_word_motion_and_paste_match_separate_frames() {
    let mut batched = harness("one two", true);
    let mut separate = harness("one two", true);
    for screen in [&mut batched, &mut separate] {
        events(screen, [egui::Event::Text("A".into())]);
    }
    events(
        &mut batched,
        [
            key(Key::ArrowLeft, egui::Modifiers::CTRL),
            egui::Event::Paste("X".into()),
        ],
    );
    events(&mut separate, [key(Key::ArrowLeft, egui::Modifiers::CTRL)]);
    events(&mut separate, [egui::Event::Paste("X".into())]);
    assert_eq!(text(&batched), "one Xtwo");
    assert_eq!(text(&batched), text(&separate));
    assert_eq!(
        batched.state().tab().unwrap().selection,
        separate.state().tab().unwrap().selection
    );
    events(&mut batched, [key(Key::Z, egui::Modifiers::CTRL)]);
    assert_eq!(text(&batched), "one two");
}

#[test]
fn vim_word_deletion_before_enter_and_escape_keeps_native_modifier_semantics() {
    for (entry, deletion) in [("A", Key::Backspace), ("i", Key::Delete)] {
        let mut batched = harness("one two", true);
        let mut separate = harness("one two", true);
        for screen in [&mut batched, &mut separate] {
            events(screen, [egui::Event::Text(entry.into())]);
        }
        events(
            &mut batched,
            [
                key(deletion, egui::Modifiers::CTRL),
                key(Key::Enter, egui::Modifiers::NONE),
                egui::Event::Text("🦀".into()),
                key(Key::Escape, egui::Modifiers::NONE),
            ],
        );
        events(&mut separate, [key(deletion, egui::Modifiers::CTRL)]);
        events(&mut separate, [key(Key::Enter, egui::Modifiers::NONE)]);
        events(&mut separate, [egui::Event::Text("🦀".into())]);
        events(&mut separate, [key(Key::Escape, egui::Modifiers::NONE)]);
        assert_eq!(text(&batched), text(&separate), "deletion={deletion:?}");
        assert_eq!(batched.state().tab().unwrap().vim.mode, VimMode::Normal);
        events(&mut batched, [egui::Event::Text("u".into())]);
        assert_eq!(text(&batched), "one two");
    }
}

#[test]
fn pending_native_suffix_is_cancelled_after_external_source_change() {
    let mut screen = harness("tail", false);
    screen.input_mut().events.extend([
        egui::Event::Text("X".into()),
        key(Key::Z, egui::Modifiers::CTRL),
        egui::Event::Text("Y".into()),
    ]);
    screen.run_steps(1);
    assert!(screen.state().editor_input_pending());
    let tab = screen.state_mut().tab_mut().unwrap();
    tab.text = "external".into();
    tab.changed();
    settle(&mut screen);
    assert_eq!(text(&screen), "external");
}

#[test]
fn global_new_file_keeps_earlier_text_in_original_document_and_later_save_targets_new_document() {
    let mut screen = harness("tail", false);
    let original_id = screen.state().tab().unwrap().id;
    screen
        .state_mut()
        .settings
        .keybindings
        .insert(keybindings::Action::NewFile.id().into(), Some("F8".into()));
    events(
        &mut screen,
        [
            egui::Event::Text("X".into()),
            key(Key::F8, egui::Modifiers::NONE),
            key(Key::S, egui::Modifiers::CTRL),
        ],
    );
    let app = screen.state();
    assert_eq!(
        app.projects[0]
            .tabs
            .iter()
            .find(|tab| tab.id == original_id)
            .unwrap()
            .text,
        "Xtail"
    );
    assert_ne!(app.tab().unwrap().id, original_id);
    assert!(app.tab().unwrap().text.is_empty());
    assert_eq!(app.save_as, Some(app.tab().unwrap().id));
}

#[test]
fn remapped_save_writes_text_before_its_key_and_keeps_trailing_text_unsaved() {
    let mut screen = harness("tail", false);
    let id = screen.state().tab().unwrap().id;
    let path = std::env::temp_dir().join(format!(
        "morrow-shortcut-save-{}-{id}.txt",
        std::process::id()
    ));
    fs::write(&path, "tail").unwrap();
    {
        let app = screen.state_mut();
        app.settings
            .keybindings
            .insert(keybindings::Action::Save.id().into(), Some("F8".into()));
        let tab = app.tab_mut().unwrap();
        tab.path = Some(path.clone());
        tab.fingerprint = Some(workspace::fingerprint(&path, MAX_DOCUMENT).unwrap());
    }
    events(
        &mut screen,
        [
            egui::Event::Text("X".into()),
            key(Key::F8, egui::Modifiers::NONE),
            egui::Event::Text("Y".into()),
        ],
    );
    let until = Instant::now() + Duration::from_secs(2);
    while screen.state().tab().unwrap().saving && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(5));
        screen.state_mut().poll();
    }
    assert_eq!(fs::read_to_string(&path).unwrap(), "Xtail");
    assert_eq!(text(&screen), "XYtail");
    assert!(screen.state().tab().unwrap().dirty);
    fs::remove_file(path).unwrap();
}

#[test]
fn remapped_native_history_preserves_text_order_and_disables_native_aliases() {
    let mut screen = harness("tail", false);
    screen.state_mut().settings.keybindings.insert(
        keybindings::Action::Undo.id().into(),
        Some("Ctrl+Alt+U".into()),
    );
    screen.state_mut().settings.keybindings.insert(
        keybindings::Action::Redo.id().into(),
        Some("Ctrl+Alt+R".into()),
    );
    let chord = egui::Modifiers::CTRL.plus(egui::Modifiers::ALT);
    events(
        &mut screen,
        [
            egui::Event::Text("X".into()),
            key(Key::U, chord),
            egui::Event::Text("Y".into()),
        ],
    );
    assert_eq!(text(&screen), "Ytail");
    events(&mut screen, [key(Key::Z, egui::Modifiers::CTRL)]);
    assert_eq!(
        text(&screen),
        "Ytail",
        "retired Ctrl+Z must not reach TextEdit"
    );
    events(&mut screen, [key(Key::U, chord)]);
    assert_eq!(text(&screen), "tail");
    for retired in [
        key(Key::Y, egui::Modifiers::CTRL),
        key(Key::Z, egui::Modifiers::CTRL.plus(egui::Modifiers::SHIFT)),
    ] {
        events(&mut screen, [retired]);
        assert_eq!(
            text(&screen),
            "tail",
            "retired redo alias must not reach TextEdit"
        );
    }
    events(&mut screen, [key(Key::R, chord)]);
    assert_eq!(text(&screen), "Ytail");
}

#[test]
fn remapped_duplicate_applies_after_native_prefix_and_keeps_undo() {
    let mut screen = harness("tail", false);
    screen.state_mut().settings.keybindings.insert(
        keybindings::Action::DuplicateLine.id().into(),
        Some("F8".into()),
    );
    events(
        &mut screen,
        [
            egui::Event::Text("X".into()),
            key(Key::F8, egui::Modifiers::NONE),
        ],
    );
    assert_eq!(text(&screen), "Xtail\nXtail");
    events(&mut screen, [key(Key::Z, egui::Modifiers::CTRL)]);
    assert_eq!(text(&screen), "Xtail");
    events(
        &mut screen,
        [key(
            Key::ArrowDown,
            egui::Modifiers::ALT.plus(egui::Modifiers::SHIFT),
        )],
    );
    assert_eq!(
        text(&screen),
        "Xtail",
        "retired duplicate binding must not edit text"
    );
}

#[test]
fn vim_insert_respects_remapped_history_without_native_alias_fallback() {
    let mut screen = harness("tail", true);
    screen.state_mut().settings.keybindings.insert(
        keybindings::Action::Undo.id().into(),
        Some("Ctrl+Alt+U".into()),
    );
    events(&mut screen, [egui::Event::Text("iX".into())]);
    assert_eq!(text(&screen), "Xtail");
    events(&mut screen, [key(Key::Z, egui::Modifiers::CTRL)]);
    assert_eq!(
        text(&screen),
        "Xtail",
        "retired Ctrl+Z must not reach native Insert history"
    );
    events(
        &mut screen,
        [key(
            Key::U,
            egui::Modifiers::CTRL.plus(egui::Modifiers::ALT),
        )],
    );
    assert_eq!(text(&screen), "tail");
}

#[test]
fn alt_editing_chord_consumes_its_native_text_payload() {
    let mut screen = harness("tail", false);
    screen.state_mut().settings.keybindings.insert(
        keybindings::Action::DuplicateLine.id().into(),
        Some("Alt+K".into()),
    );
    events(
        &mut screen,
        [
            egui::Event::Text("X".into()),
            key(Key::K, egui::Modifiers::ALT),
            egui::Event::Text("k".into()),
        ],
    );
    assert_eq!(text(&screen), "Xtail\nXtail");
}

#[test]
fn editing_command_defers_following_workspace_command_until_after_edit() {
    let mut screen = harness("tail", false);
    let wrapped = screen.state().settings.word_wrap;
    events(
        &mut screen,
        [
            key(
                Key::ArrowDown,
                egui::Modifiers::ALT.plus(egui::Modifiers::SHIFT),
            ),
            key(Key::Z, egui::Modifiers::ALT),
            egui::Event::Text("z".into()),
        ],
    );
    assert_eq!(text(&screen), "tail\ntail");
    assert_eq!(screen.state().settings.word_wrap, !wrapped);
}

#[test]
fn remapped_alt_edit_consumes_generated_letter_and_punctuation_but_keeps_real_text() {
    for vim in [false, true] {
        for (binding, key_code, generated) in
            [("Alt+J", Key::J, "j"), ("Alt+Slash", Key::Slash, "/")]
        {
            let mut screen = harness("tail", vim);
            screen.state_mut().settings.keybindings.insert(
                keybindings::Action::DuplicateLine.id().into(),
                Some(binding.into()),
            );
            if vim {
                events(&mut screen, [egui::Event::Text("i".into())]);
            }
            events(
                &mut screen,
                [
                    egui::Event::Text("X".into()),
                    key(key_code, egui::Modifiers::ALT),
                    egui::Event::Text(generated.into()),
                    egui::Event::Text("Y".into()),
                ],
            );
            assert_eq!(
                text(&screen),
                "Xtail\nXYtail",
                "vim={vim}, binding={binding}"
            );
        }
    }
}

#[test]
fn vim_alt_edit_does_not_execute_its_generated_character_as_a_motion_or_search() {
    for (binding, key_code, generated) in [("Alt+I", Key::I, "i"), ("Alt+Slash", Key::Slash, "/")] {
        let mut batched = harness("tail", true);
        let mut separate = harness("tail", true);
        for screen in [&mut batched, &mut separate] {
            screen.state_mut().settings.keybindings.insert(
                keybindings::Action::DuplicateLine.id().into(),
                Some(binding.into()),
            );
        }
        events(
            &mut batched,
            [
                key(key_code, egui::Modifiers::ALT),
                egui::Event::Text(generated.into()),
                egui::Event::Text("iY".into()),
            ],
        );
        events(&mut separate, [key(key_code, egui::Modifiers::ALT)]);
        events(&mut separate, [egui::Event::Text("iY".into())]);
        assert!(!batched.state().vim_search_active(), "binding={binding}");
        assert_eq!(text(&batched), text(&separate), "binding={binding}");
        assert_eq!(
            batched.state().tab().unwrap().vim.mode,
            VimMode::Insert,
            "binding={binding}"
        );
    }
}

#[test]
fn move_line_shortcuts_follow_native_prefix_and_keep_the_moved_caret_undoable() {
    let mut screen = harness("one\ntwo🦀\nthree", false);
    events(
        &mut screen,
        [
            egui::Event::Text("X".into()),
            key(Key::ArrowDown, egui::Modifiers::ALT),
        ],
    );
    assert_eq!(text(&screen), "two🦀\nXone\nthree");
    assert_eq!(screen.state().tab().unwrap().selection, 6..6);
    events(&mut screen, [key(Key::Z, egui::Modifiers::CTRL)]);
    assert_eq!(text(&screen), "Xone\ntwo🦀\nthree");
    assert_eq!(screen.state().tab().unwrap().selection, 1..1);
    events(&mut screen, [key(Key::Y, egui::Modifiers::CTRL)]);
    assert_eq!(text(&screen), "two🦀\nXone\nthree");
    assert_eq!(screen.state().tab().unwrap().selection, 6..6);
    events(&mut screen, [key(Key::ArrowUp, egui::Modifiers::ALT)]);
    assert_eq!(text(&screen), "Xone\ntwo🦀\nthree");
    assert_eq!(screen.state().tab().unwrap().selection, 1..1);
}

#[test]
fn move_line_in_vim_normal_mode_moves_cursor_with_text_and_undoes_once() {
    let original = "one\ntwo🦀\nthree";
    let mut screen = harness(original, true);
    events(&mut screen, [key(Key::ArrowDown, egui::Modifiers::ALT)]);
    assert_eq!(text(&screen), "two🦀\none\nthree");
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 5);
    assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::Normal);
    events(&mut screen, [egui::Event::Text("u".into())]);
    assert_eq!(text(&screen), original);
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 0);
    assert!(!screen.state().tab().unwrap().dirty);
    let revision = screen.state().tab().unwrap().revision;
    events(&mut screen, [key(Key::ArrowUp, egui::Modifiers::ALT)]);
    assert_eq!(text(&screen), original);
    assert_eq!(screen.state().tab().unwrap().revision, revision);
    assert!(!screen.state().tab().unwrap().vim.can_undo());
}

#[test]
fn move_line_at_native_buffer_edges_does_not_dirty_or_create_history() {
    let original = "one\ntwo";
    let mut screen = harness(original, false);
    let revision = screen.state().tab().unwrap().revision;
    events(&mut screen, [key(Key::ArrowUp, egui::Modifiers::ALT)]);
    assert_eq!(text(&screen), original);
    assert_eq!(screen.state().tab().unwrap().revision, revision);
    assert!(!screen.state().tab().unwrap().dirty);
    let id = screen.state().tab().unwrap().id;
    assert_eq!(screen.state().context_native_history(id), (false, false));
}

#[test]
fn move_visual_lines_keeps_the_area_selected_for_repeated_moves_and_undo() {
    let original = "one\ntwo🦀\nthree";
    let mut screen = harness(original, true);
    events(&mut screen, [egui::Event::Text("Vj".into())]);
    events(&mut screen, [key(Key::ArrowDown, egui::Modifiers::ALT)]);
    assert_eq!(text(&screen), "three\none\ntwo🦀");
    assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::VisualLine);
    assert_eq!(screen.state().tab().unwrap().selection, 6..14);
    events(&mut screen, [key(Key::ArrowUp, egui::Modifiers::ALT)]);
    assert_eq!(text(&screen), original);
    assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::VisualLine);
    assert_eq!(screen.state().tab().unwrap().selection, 0..9);
    events(&mut screen, [key(Key::Escape, egui::Modifiers::NONE)]);
    events(&mut screen, [egui::Event::Text("u".into())]);
    assert_eq!(text(&screen), "three\none\ntwo🦀");
}

#[test]
fn duplicate_line_in_vim_follows_the_duplicate_and_can_be_edited_then_undone() {
    let original = "one\ntwo🦀";
    let mut screen = harness(original, true);
    events(
        &mut screen,
        [key(
            Key::ArrowDown,
            egui::Modifiers::ALT.plus(egui::Modifiers::SHIFT),
        )],
    );
    assert_eq!(text(&screen), "one\none\ntwo🦀");
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 4);
    events(&mut screen, [egui::Event::Text("x".into())]);
    assert_eq!(text(&screen), "one\nne\ntwo🦀");
    events(&mut screen, [egui::Event::Text("u".into())]);
    assert_eq!(text(&screen), "one\none\ntwo🦀");
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 4);
    events(&mut screen, [egui::Event::Text("u".into())]);
    assert_eq!(text(&screen), original);
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 0);
}

#[test]
fn altgr_characters_do_not_trigger_layout_commands_in_native_or_vim_insert() {
    for vim in [false, true] {
        let mut screen = harness("tail", vim);
        if vim {
            events(&mut screen, [egui::Event::Text("i".into())]);
        }
        let sidebar = screen.state().sidebar_visible;
        let mut release_right_alt = key(Key::AltRight, egui::Modifiers::NONE);
        if let egui::Event::Key { pressed, .. } = &mut release_right_alt {
            *pressed = false;
        }
        events(
            &mut screen,
            [
                key(
                    Key::AltRight,
                    egui::Modifiers::CTRL.plus(egui::Modifiers::ALT),
                ),
                key(Key::E, egui::Modifiers::CTRL.plus(egui::Modifiers::ALT)),
                egui::Event::Text("€".into()),
                release_right_alt,
                key(Key::M, egui::Modifiers::NONE),
                egui::Event::Text("µ".into()),
            ],
        );
        assert_eq!(text(&screen), "€µtail", "vim={vim}");
        assert_eq!(screen.state().sidebar_visible, sidebar, "vim={vim}");
    }
}

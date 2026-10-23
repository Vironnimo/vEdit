use super::*;
use crate::multicursor::Selection;
use egui_kittest::Harness;

fn harness(text: &str, vim: bool) -> Harness<'static, VEditApp> {
    let mut app = VEditApp::empty(egui::Context::default());
    app.settings.vim = vim;
    app.settings.autosave = false;
    app.sidebar_visible = false;
    let root = std::env::temp_dir().join("multicursor-render-only");
    let mut project = Project::new(root.clone());
    project.index = Some(Arc::new(workspace::WorkspaceIndex::empty(root)));
    project.last_git_refresh = Instant::now();
    let mut tab = EditorTab::new(None, "Multicursor.txt".into());
    tab.text = text.into();
    tab.saved_text = text.into();
    project.tabs.push(tab);
    app.projects.push(project);
    app.bind_current_pane();
    let mut screen = Harness::builder()
        .with_size([1100.0, 740.0])
        .with_os(egui::os::OperatingSystem::Windows)
        .build_ui_state(
            |ui, app: &mut VEditApp| {
                app.ctx = ui.ctx().clone();
                app.shortcuts();
                app.handle_vim();
                app.status_bar(ui);
                app.editor_ui(ui);
            },
            app,
        );
    set_style(&screen.ctx);
    screen.ctx.all_styles_mut(|s| {
        s.animation_time = 0.0;
        s.visuals.text_cursor.blink = false;
    });
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
fn events(screen: &mut Harness<'_, VEditApp>, events: impl IntoIterator<Item = egui::Event>) {
    screen.input_mut().events.extend(events);
    screen.run_steps(1);
    editor_input_tests::settle(screen);
}
fn all(screen: &mut Harness<'_, VEditApp>) {
    events(
        screen,
        [key(Key::L, egui::Modifiers::CTRL | egui::Modifiers::SHIFT)],
    );
}
fn selections(screen: &Harness<'_, VEditApp>) -> Vec<Range<usize>> {
    screen
        .state()
        .tab()
        .unwrap()
        .vim
        .multi
        .selections
        .iter()
        .map(|s| s.range())
        .collect()
}

#[test]
fn multicursor_typing_undo_redo_restores_all_ranges_in_both_modes() {
    for vim in [false, true] {
        let mut screen = harness("foo foo\nfoo", vim);
        all(&mut screen);
        assert_eq!(selections(&screen), vec![0..3, 4..7, 8..11], "vim={vim}");
        events(&mut screen, [egui::Event::Text("🦀".into())]);
        assert_eq!(screen.state().tab().unwrap().text, "🦀 🦀\n🦀");
        assert_eq!(selections(&screen), vec![1..1, 3..3, 5..5]);
        events(&mut screen, [key(Key::Z, egui::Modifiers::CTRL)]);
        assert_eq!(
            screen.state().tab().unwrap().text,
            "foo foo\nfoo",
            "vim={vim}"
        );
        assert_eq!(selections(&screen), vec![0..3, 4..7, 8..11], "vim={vim}");
        events(&mut screen, [key(Key::Y, egui::Modifiers::CTRL)]);
        assert_eq!(screen.state().tab().unwrap().text, "🦀 🦀\n🦀");
        assert_eq!(selections(&screen), vec![1..1, 3..3, 5..5]);
    }
}

#[test]
fn multicursor_native_prefix_and_same_frame_commands_keep_order() {
    let mut screen = harness("foo foo", false);
    events(
        &mut screen,
        [
            egui::Event::Text("x ".into()),
            key(Key::L, egui::Modifiers::CTRL | egui::Modifiers::SHIFT),
            egui::Event::Text("bar".into()),
            key(Key::Z, egui::Modifiers::CTRL),
            egui::Event::Text("baz".into()),
        ],
    );
    assert_eq!(screen.state().tab().unwrap().text, "x baz baz");
}

#[test]
fn multicursor_escape_and_later_undo_recovers_mode() {
    for vim in [false, true] {
        let mut screen = harness("foo foo", vim);
        all(&mut screen);
        events(
            &mut screen,
            [
                egui::Event::Text("bar".into()),
                key(Key::Escape, egui::Modifiers::NONE),
            ],
        );
        assert!(!screen.state().tab().unwrap().vim.multi.active());
        events(&mut screen, [key(Key::Z, egui::Modifiers::CTRL)]);
        assert_eq!(screen.state().tab().unwrap().text, "foo foo");
        assert_eq!(selections(&screen), vec![0..3, 4..7]);
    }
}

#[test]
fn multicursor_paste_settings_and_context_cut_share_transaction() {
    let mut screen = harness("foo foo", true);
    all(&mut screen);
    events(&mut screen, [egui::Event::Paste("first\nsecond".into())]);
    assert_eq!(screen.state().tab().unwrap().text, "first second");
    events(&mut screen, [key(Key::Z, egui::Modifiers::CTRL)]);
    screen.state_mut().settings.multicursor.spread_paste = false;
    events(&mut screen, [egui::Event::Paste("a\nb".into())]);
    assert_eq!(screen.state().tab().unwrap().text, "a\nb a\nb");
    events(&mut screen, [key(Key::Z, egui::Modifiers::CTRL)]);
    let app = screen.state_mut();
    let id = app.tab().unwrap().id;
    app.apply_editor_context_action(id, context_actions::EditorAction::Cut);
    assert_eq!(app.tab().unwrap().text, " ");
    app.apply_editor_context_action(id, context_actions::EditorAction::Undo);
    assert_eq!(app.tab().unwrap().text, "foo foo");
    assert_eq!(app.tab().unwrap().vim.multi.selections.len(), 2);
}

#[test]
fn multicursor_arrows_selection_and_deletion() {
    let mut screen = harness("foo foo", true);
    all(&mut screen);
    events(
        &mut screen,
        [
            key(Key::ArrowRight, egui::Modifiers::NONE),
            key(Key::ArrowLeft, egui::Modifiers::SHIFT),
            key(Key::Backspace, egui::Modifiers::NONE),
        ],
    );
    assert_eq!(screen.state().tab().unwrap().text, "fo fo");
    events(&mut screen, [key(Key::Backspace, egui::Modifiers::CTRL)]);
    assert_eq!(screen.state().tab().unwrap().text, " ");
}

#[test]
fn multicursor_line_commands_do_not_apply_twice_to_same_line() {
    let mut screen = harness("foo foo\nfoo", true);
    all(&mut screen);
    events(&mut screen, [key(Key::Slash, egui::Modifiers::CTRL)]);
    assert_eq!(screen.state().tab().unwrap().text, "// foo foo\n// foo");
    events(&mut screen, [key(Key::Z, egui::Modifiers::CTRL)]);
    events(&mut screen, [key(Key::Tab, egui::Modifiers::NONE)]);
    assert_eq!(screen.state().tab().unwrap().text, "    foo foo\n    foo");
}

#[test]
fn multicursor_splits_keep_independent_selections() {
    let mut screen = harness("foo foo\nfoo", true);
    all(&mut screen);
    let app = screen.state_mut();
    app.capture_pane_navigation();
    let right = app
        .split_active(crate::cli::SplitOrientation::Vertical)
        .unwrap();
    app.tab_mut().unwrap().set_cursor(4..4);
    app.capture_pane_navigation();
    app.focus_editor_pane(1).unwrap();
    assert_eq!(app.tab().unwrap().vim.multi.selections.len(), 3);
    app.focus_editor_pane(right).unwrap();
    assert!(!app.tab().unwrap().vim.multi.active());
    assert_eq!(app.tab().unwrap().vim.cursor, 4);
}

#[test]
fn multicursor_creation_limit_is_atomic_and_settings_persist() {
    let mut screen = harness("foo foo foo", false);
    screen.state_mut().settings.multicursor.limit = 2;
    all(&mut screen);
    assert_eq!(selections(&screen), vec![0..3]);
    assert!(screen.state().status.as_ref().unwrap().0.contains("limit"));
    let encoded = serde_json::to_string(&screen.state().settings).unwrap();
    let restored: Settings = serde_json::from_str(&encoded).unwrap();
    assert_eq!(restored.multicursor, screen.state().settings.multicursor);
}

#[test]
fn multicursor_rebinding_does_not_trigger_in_text_fields() {
    let mut screen = harness("foo foo", false);
    screen
        .ctx
        .memory_mut(|m| m.request_focus(egui::Id::new("another_field")));
    screen
        .input_mut()
        .events
        .push(key(Key::D, egui::Modifiers::CTRL));
    screen.run_steps(1);
    assert!(!screen.state().tab().unwrap().vim.multi.active());
}

#[test]
fn multicursor_render_selections_and_primary_cursor() {
    let mut screen = harness(
        "// Change repeated identifiers together.\nlet customer = load_customer();\nlet label = customer.name();\nshow(customer, label);\n",
        true,
    );
    screen.state_mut().tab_mut().unwrap().set_cursor(45..45);
    screen.run_steps(1);
    all(&mut screen);
    screen.run_steps(2);
    assert!(screen.state().tab().unwrap().vim.multi.active());
    visual_tests::snapshot(&mut screen, "vedit_multicursor");
}

#[test]
fn multicursor_line_ends_and_vertical_creation() {
    let mut screen = harness("abc\nx\nabc", true);
    let app = screen.state_mut();
    app.tab_mut().unwrap().set_cursor(2..2);
    app.multicursor_action(keybindings::Action::AddCursorBelow);
    app.multicursor_action(keybindings::Action::AddCursorBelow);
    assert_eq!(
        app.tab()
            .unwrap()
            .vim
            .multi
            .selections
            .iter()
            .map(|s| s.head)
            .collect::<Vec<_>>(),
        vec![2, 5, 8]
    );
    app.tab_mut().unwrap().vim.multi.selections = vec![Selection::new(0, 9)];
    app.multicursor_action(keybindings::Action::CursorsAtLineEnds);
    assert_eq!(
        app.tab()
            .unwrap()
            .vim
            .multi
            .selections
            .iter()
            .map(|s| s.head)
            .collect::<Vec<_>>(),
        vec![3, 5, 9]
    );
}

#[test]
fn multicursor_smart_newline_places_carets_inside_each_pair() {
    let mut screen = harness("{}\n{}", true);
    let tab = screen.state_mut().tab_mut().unwrap();
    tab.vim.multi.selections = vec![Selection::new(1, 1), Selection::new(4, 4)];
    tab.sync_multicursor();
    events(
        &mut screen,
        [
            key(Key::Enter, egui::Modifiers::NONE),
            egui::Event::Text("x".into()),
        ],
    );
    assert_eq!(
        screen.state().tab().unwrap().text,
        "{\n    x\n}\n{\n    x\n}"
    );
}

#[test]
fn multicursor_line_moves_and_duplicates_follow_the_text() {
    for (command, expected, positions) in [
        (keybindings::Action::MoveLineDown, "b\na\nd\nc", vec![2, 6]),
        (
            keybindings::Action::DuplicateLine,
            "a\na\nb\nc\nc\nd",
            vec![2, 8],
        ),
        (
            keybindings::Action::InsertLineAbove,
            "\na\nb\n\nc\nd",
            vec![0, 5],
        ),
    ] {
        let mut screen = harness("a\nb\nc\nd", true);
        let app = screen.state_mut();
        app.tab_mut().unwrap().vim.multi.selections =
            vec![Selection::new(0, 0), Selection::new(4, 4)];
        app.tab_mut().unwrap().sync_multicursor();
        app.run_shortcut_action(command);
        assert_eq!(app.tab().unwrap().text, expected, "{command:?}");
        assert_eq!(
            app.tab()
                .unwrap()
                .vim
                .multi
                .selections
                .iter()
                .map(|s| s.head)
                .collect::<Vec<_>>(),
            positions,
            "{command:?}"
        );
    }
}

#[test]
fn multicursor_combining_characters_and_emoji_delete_as_graphemes() {
    let mut screen = harness("e\u{301} 👩‍💻", false);
    let tab = screen.state_mut().tab_mut().unwrap();
    tab.vim.multi.selections = vec![Selection::new(2, 2), Selection::new(6, 6)];
    tab.sync_multicursor();
    events(&mut screen, [key(Key::Backspace, egui::Modifiers::NONE)]);
    assert_eq!(screen.state().tab().unwrap().text, " ");
}

fn position(screen: &Harness<'_, VEditApp>, index: usize) -> egui::Pos2 {
    let id = screen
        .state()
        .document_widget_id(screen.state().tab().unwrap().id);
    let geometry = screen
        .ctx
        .data(|d| d.get_temp::<multicursor_ui::Geometry>(id.with("multi_layout")))
        .unwrap();
    geometry.origin
        + geometry
            .galley
            .pos_from_cursor(egui::text::CCursor::new(index))
            .center()
            .to_vec2()
}

fn click_event(pos: egui::Pos2, pressed: bool, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers,
    }
}

#[test]
fn multicursor_alt_click_then_text_in_one_frame_and_toggle() {
    for vim in [false, true] {
        let mut screen = harness("abc\nabc", vim);
        let pos = position(&screen, 4);
        events(
            &mut screen,
            [
                egui::Event::PointerMoved(pos),
                click_event(pos, true, egui::Modifiers::ALT),
                click_event(pos, false, egui::Modifiers::ALT),
                egui::Event::Text("X".into()),
            ],
        );
        assert_eq!(
            screen.state().tab().unwrap().text,
            "Xabc\nXabc",
            "vim={vim}"
        );
        let pos = position(&screen, 1);
        events(
            &mut screen,
            [
                egui::Event::PointerMoved(pos),
                click_event(pos, true, egui::Modifiers::ALT),
                click_event(pos, false, egui::Modifiers::ALT),
            ],
        );
        assert_eq!(selections(&screen).len(), 1);
    }
}

#[test]
fn multicursor_plain_click_exits_without_losing_the_target() {
    let mut screen = harness("foo foo", true);
    all(&mut screen);
    let pos = position(&screen, 1);
    events(
        &mut screen,
        [
            egui::Event::PointerMoved(pos),
            click_event(pos, true, egui::Modifiers::NONE),
            click_event(pos, false, egui::Modifiers::NONE),
        ],
    );
    assert!(!screen.state().tab().unwrap().vim.multi.active());
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 1);
}

#[test]
fn multicursor_box_drag_selects_each_visible_row() {
    let mut screen = harness("abc\nabc\nabc", false);
    let start = position(&screen, 1);
    let end = position(&screen, 10);
    let modifiers = egui::Modifiers::ALT | egui::Modifiers::SHIFT;
    events(
        &mut screen,
        [
            egui::Event::ModifiersChanged(modifiers),
            egui::Event::PointerMoved(start),
            click_event(start, true, modifiers),
        ],
    );
    events(&mut screen, [egui::Event::PointerMoved(end)]);
    events(
        &mut screen,
        [
            click_event(end, false, modifiers),
            egui::Event::ModifiersChanged(egui::Modifiers::NONE),
        ],
    );
    assert_eq!(selections(&screen), vec![1..2, 5..6, 9..10]);
    events(&mut screen, [egui::Event::Text("X".into())]);
    assert_eq!(screen.state().tab().unwrap().text, "aXc\naXc\naXc");
}

#[test]
fn multicursor_ime_commit_inserts_once_at_each_selection() {
    let mut screen = harness("foo foo", true);
    all(&mut screen);
    events(
        &mut screen,
        [egui::Event::Ime(egui::ImeEvent::Preedit {
            text: "に".into(),
            active_range_chars: None,
        })],
    );
    assert_eq!(screen.state().tab().unwrap().text, "foo foo");
    events(
        &mut screen,
        [egui::Event::Ime(egui::ImeEvent::Commit("日本".into()))],
    );
    assert_eq!(screen.state().tab().unwrap().text, "日本 日本");
}

#[test]
fn multicursor_completion_repeats_matching_edits_and_refuses_other_contexts() {
    let mut state = crate::multicursor::State::default();
    state.selections = vec![Selection::new(2, 2), Selection::new(5, 5)];
    let main = crate::lsp::TextEdit {
        range: crate::lsp::TextRange {
            start: crate::lsp::char_to_position("fo fo", 3),
            end: crate::lsp::char_to_position("fo fo", 5),
        },
        new_text: "foo".into(),
    };
    let (text, state) = multicursor_ui::complete("fo fo", &state, &main, &[], 5).unwrap();
    assert_eq!(text, "foo foo");
    assert_eq!(
        state.selections.iter().map(|s| s.head).collect::<Vec<_>>(),
        vec![3, 7]
    );
    assert!(multicursor_ui::complete("xx foo", &state, &main, &[], 5).is_err());
}

#[test]
fn multicursor_settings_patch_is_partial_and_invalid_limit_is_atomic() {
    let mut screen = harness("unchanged", true);
    let app = screen.state_mut();
    let request: crate::cli::Request = serde_json::from_str(
        r#"{"op":"settings","patch":{"multicursor":{"modifier":"ctrl","spread_paste":false}}}"#,
    )
    .unwrap();
    let response = app.control_request(request);
    assert!(response.ok);
    assert_eq!(
        response.result["settings"]["multicursor"]["modifier"],
        "ctrl"
    );
    assert_eq!(response.result["settings"]["multicursor"]["limit"], 1000);
    let before = serde_json::to_value(&app.settings).unwrap();
    let request = serde_json::from_str(
        r#"{"op":"settings","patch":{"vim":false,"multicursor":{"limit":1}}}"#,
    )
    .unwrap();
    let response = app.control_request(request);
    assert!(!response.ok);
    assert_eq!(serde_json::to_value(&app.settings).unwrap(), before);
}

#[test]
fn multicursor_tabs_in_identical_whitespace_keep_their_positions() {
    let mut screen = harness("    ", true);
    let tab = screen.state_mut().tab_mut().unwrap();
    tab.vim.multi.selections = vec![Selection::new(1, 1), Selection::new(3, 3)];
    tab.sync_multicursor();
    events(&mut screen, [key(Key::Tab, egui::Modifiers::NONE)]);
    assert_eq!(screen.state().tab().unwrap().text, "        ");
    assert_eq!(selections(&screen), vec![4..4, 7..7]);
}

#[test]
fn multicursor_newlines_between_delimiters_do_not_overlap() {
    let mut screen = harness("{  }", true);
    let tab = screen.state_mut().tab_mut().unwrap();
    tab.vim.multi.selections = vec![Selection::new(1, 1), Selection::new(2, 2)];
    tab.sync_multicursor();
    events(&mut screen, [key(Key::Enter, egui::Modifiers::NONE)]);
    assert_eq!(screen.state().tab().unwrap().text, "{\n    \n\n    \n}");
    assert_eq!(selections(&screen), vec![6..6, 12..12]);
}

#[test]
fn multicursor_moving_identical_lines_moves_the_cursors() {
    let mut screen = harness("a\na\na\na", true);
    let tab = screen.state_mut().tab_mut().unwrap();
    tab.vim.multi.selections = vec![Selection::new(0, 0), Selection::new(4, 4)];
    tab.sync_multicursor();
    screen
        .state_mut()
        .multi_command(editor_input::Command::MoveLine(true));
    assert_eq!(screen.state().tab().unwrap().text, "a\na\na\na");
    assert_eq!(selections(&screen), vec![2..2, 6..6]);
}

#[test]
fn multicursor_settings_render_and_controls_are_reachable() {
    use egui_kittest::kittest::Queryable;
    let mut app = VEditApp::empty(egui::Context::default());
    visual_tests::workspace(&mut app);
    let mut screen = visual_tests::harness(app);
    screen.state_mut().open_settings_section("Editing");
    screen.run_steps(3);
    screen
        .query_all_by_label("Maximum cursors")
        .next()
        .unwrap()
        .scroll_to_me();
    screen.run_steps(3);
    let label = "Distribute pasted lines across cursors";
    screen.get_by_label(label).click();
    screen.run_steps(3);
    assert!(!screen.state().settings.multicursor.spread_paste);
    visual_tests::snapshot(&mut screen, "vedit_multicursor_settings");
}

#[test]
fn multicursor_vertical_movement_tracks_wrapped_rows_after_typing() {
    let text = format!("{}\n{}", "a".repeat(200), "b".repeat(200));
    let mut screen = harness(&text, false);
    screen.set_size(egui::vec2(500.0, 740.0));
    screen.state_mut().settings.word_wrap = true;
    let tab = screen.state_mut().tab_mut().unwrap();
    tab.vim.multi.selections = vec![Selection::new(5, 5), Selection::new(206, 206)];
    tab.sync_multicursor();
    screen.run_steps(2);
    events(
        &mut screen,
        [
            egui::Event::Text("X".into()),
            key(Key::ArrowDown, egui::Modifiers::NONE),
        ],
    );
    let ranges = selections(&screen);
    assert_eq!(ranges.len(), 2);
    assert!(ranges[0].start > 6 && ranges[0].start < 200, "{ranges:?}");
    assert!(ranges[1].start > 208 && ranges[1].start < 403, "{ranges:?}");
    events(&mut screen, [key(Key::ArrowUp, egui::Modifiers::NONE)]);
    assert_eq!(selections(&screen), vec![6..6, 208..208]);
}

#[test]
fn multicursor_native_history_distinguishes_repeated_document_states() {
    let mut screen = harness("aaaa", false);
    let tab = screen.state_mut().tab_mut().unwrap();
    tab.vim.multi.selections = vec![Selection::new(0, 0), Selection::new(3, 3)];
    tab.sync_multicursor();
    events(&mut screen, [egui::Event::Text("X".into())]);
    events(&mut screen, [key(Key::Backspace, egui::Modifiers::NONE)]);
    let tab = screen.state_mut().tab_mut().unwrap();
    tab.vim.multi.selections = vec![Selection::new(1, 1), Selection::new(3, 3)];
    tab.sync_multicursor();
    events(&mut screen, [egui::Event::Text("Y".into())]);
    events(&mut screen, [key(Key::Z, egui::Modifiers::CTRL)]);
    assert_eq!(selections(&screen), vec![1..1, 3..3]);
    events(&mut screen, [key(Key::Z, egui::Modifiers::CTRL)]);
    events(&mut screen, [key(Key::Z, egui::Modifiers::CTRL)]);
    assert_eq!(screen.state().tab().unwrap().text, "aaaa");
    assert_eq!(selections(&screen), vec![0..0, 3..3]);
    events(&mut screen, [key(Key::Y, egui::Modifiers::CTRL)]);
    events(&mut screen, [key(Key::Y, egui::Modifiers::CTRL)]);
    events(&mut screen, [key(Key::Y, egui::Modifiers::CTRL)]);
    assert_eq!(screen.state().tab().unwrap().text, "aYaaYa");
    assert_eq!(selections(&screen), vec![2..2, 5..5]);
}

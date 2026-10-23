//! Vim's unnamed register and the desktop clipboard are independent stores.

use super::*;
use context_actions::EditorAction;
use egui_kittest::Harness;

const UNNAMED: &str = "KEEP🦀";

fn fixture(document: &str) -> VEditApp {
    let mut app = VEditApp::empty(egui::Context::default());
    app.settings.autosave = false;
    app.sidebar_visible = false;
    let root = std::env::temp_dir().join("vedit-vim-clipboard-render-only");
    let mut project = Project::new(root.clone());
    project.last_git_refresh = Instant::now();
    let mut tab = EditorTab::new(Some(root.join("notes.txt")), "notes.txt".into());
    tab.text = document.into();
    tab.saved_text = document.into();
    let register = Yank {
        text: UNNAMED.into(),
        block_width: None,
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
        .with_step_dt(1.0 / 60.0)
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
}

fn text(screen: &mut Harness<'_, VEditApp>, value: &str) {
    events(screen, [egui::Event::Text(value.into())]);
}

fn key(key: Key, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}

fn release(key: Key) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: false,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}

fn assert_unnamed(app: &VEditApp, expected: &str, linewise: bool) {
    let global = app.register.as_ref().expect("global unnamed register");
    let local = app
        .tab()
        .unwrap()
        .vim
        .register()
        .expect("document register");
    for register in [global, local] {
        assert_eq!(register.text, expected);
        assert_eq!(register.linewise, linewise);
    }
}

fn assert_no_clipboard_write(screen: &Harness<'_, VEditApp>) {
    assert!(
        screen
            .output()
            .platform_output
            .commands
            .iter()
            .all(|command| !matches!(command, egui::OutputCommand::CopyText(_))),
        "a Vim operation must leave the system clipboard untouched"
    );
}

fn assert_system_copy(screen: &Harness<'_, VEditApp>, expected: &str) {
    assert!(
        screen.output().platform_output.commands.iter().any(
            |command| matches!(command, egui::OutputCommand::CopyText(text) if text == expected)
        ),
        "the selected source text must be sent to the system clipboard"
    );
    assert_unnamed(screen.state(), UNNAMED, false);
}

fn select_native(screen: &mut Harness<'_, VEditApp>, mode: VimMode) {
    match mode {
        VimMode::Visual => text(screen, "lvl"),
        VimMode::Insert => {
            text(screen, "i");
            screen.state_mut().tab_mut().unwrap().set_cursor(1..3);
        }
        _ => screen.state_mut().tab_mut().unwrap().set_cursor(1..3),
    }
}

#[test]
fn vim_yank_delete_and_change_never_write_to_the_system_clipboard() {
    for (command, expected, linewise) in [
        ("vlly", "a🦀é", false),
        ("yy", "a🦀é\n", true),
        ("dd", "a🦀é\n", true),
        ("cc", "a🦀é\n", true),
    ] {
        let mut screen = harness(fixture("a🦀é\nsecond\n"));
        text(&mut screen, command);
        assert_unnamed(screen.state(), expected, linewise);
        assert_no_clipboard_write(&screen);
    }
}

#[test]
fn multiline_character_find_repeats_keep_the_target_visible_in_the_editor() {
    let document = format!(
        "a🦀\n{}x first\n{}x last\n",
        "ordinary line\n".repeat(90),
        "ordinary line\n".repeat(90)
    );
    let targets: Vec<_> = document
        .chars()
        .enumerate()
        .filter_map(|(index, ch)| (ch == 'x').then_some(index))
        .collect();
    let mut app = fixture(&document);
    app.settings.smooth_scroll = false;
    let mut screen = harness(app);
    for (command, target) in [("fx", targets[0]), (";", targets[1]), (",", targets[0])] {
        text(&mut screen, command);
        screen.run_steps(3);
        let tab = screen.state().tab().unwrap();
        assert_eq!(tab.vim.cursor, target, "{command}");
        assert_eq!(tab.selection, target..target, "{command}");
        let caret = screen.state().editor_cursor_rect.unwrap();
        let bounds = screen.state().editor_rect.unwrap();
        assert!(
            bounds.contains(caret.center()),
            "{command}: caret {caret:?} in {bounds:?}"
        );
    }
    assert_eq!(screen.state().tab().unwrap().text, document);
    assert_no_clipboard_write(&screen);
}

#[test]
fn multiline_find_change_dot_and_paste_publish_the_correct_unnamed_register() {
    let mut screen = harness(fixture("a🦀\nx b\nx tail"));
    text(&mut screen, "dfx");
    assert_eq!(screen.state().tab().unwrap().text, " b\nx tail");
    assert_unnamed(screen.state(), "a🦀\nx", false);
    text(&mut screen, ".");
    assert_eq!(screen.state().tab().unwrap().text, " tail");
    assert_unnamed(screen.state(), " b\nx", false);
    text(&mut screen, "p");
    assert_eq!(screen.state().tab().unwrap().text, "  b\nxtail");
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 1);
    text(&mut screen, "uuu");
    assert_eq!(screen.state().tab().unwrap().text, "a🦀\nx b\nx tail");
    assert_no_clipboard_write(&screen);
}

#[test]
fn visual_uppercase_p_keeps_the_global_register_for_following_normal_paste() {
    let mut screen = harness(fixture("abcdefghi"));
    text(&mut screen, "v2lP");
    assert_eq!(screen.state().tab().unwrap().text, "KEEP🦀defghi");
    assert_unnamed(screen.state(), UNNAMED, false);
    text(&mut screen, "p");
    assert_eq!(screen.state().tab().unwrap().text, "KEEP🦀KEEP🦀defghi");
    assert_unnamed(screen.state(), UNNAMED, false);
    assert_no_clipboard_write(&screen);
}

#[test]
fn yank_register_zero_is_shared_across_tabs_and_surround_leaves_both_registers_intact() {
    let mut app = fixture("α🦀 beta");
    let mut other = EditorTab::new(None, "other.txt".into());
    other.text = "old tail".into();
    other.saved_text = other.text.clone();
    app.projects[0].tabs.push(other);
    let mut screen = harness(app);
    text(&mut screen, "y2l");
    assert_eq!(screen.state().yank_register.as_ref().unwrap().text, "α🦀");
    screen.state_mut().activate_tab(0, 1);
    screen.run_steps(3);
    text(&mut screen, "diw\"0p");
    assert_eq!(screen.state().tab().unwrap().text, " α🦀tail");
    assert_unnamed(screen.state(), "old", false);
    text(&mut screen, "ggysfx)");
    // A failed find consumes the unfinished surround without replacing either register.
    assert_eq!(screen.state().yank_register.as_ref().unwrap().text, "α🦀");
    assert_unnamed(screen.state(), "old", false);
    text(&mut screen, "ys2l]");
    assert_eq!(screen.state().tab().unwrap().text, " [α🦀]tail");
    assert_unnamed(screen.state(), "old", false);
    assert_eq!(screen.state().yank_register.as_ref().unwrap().text, "α🦀");
    assert_no_clipboard_write(&screen);
}

#[test]
fn vim_search_operator_updates_only_the_unnamed_register() {
    let mut screen = harness(fixture("start 测试 next 测试\n"));
    events(
        &mut screen,
        [
            egui::Event::Text("y/测试".into()),
            key(Key::Enter, egui::Modifiers::NONE),
        ],
    );
    assert_unnamed(screen.state(), "start ", false);
    assert_no_clipboard_write(&screen);
    assert_eq!(screen.state().tab().unwrap().text, "start 测试 next 测试\n");
}

#[test]
fn system_copy_preserves_unnamed_register_in_every_editor_mode() {
    for mode in [VimMode::Normal, VimMode::Visual, VimMode::Insert] {
        let mut screen = harness(fixture("αβ🦀tail\n"));
        if mode != VimMode::Normal {
            select_native(&mut screen, mode);
        }
        events(&mut screen, [egui::Event::Copy]);
        assert_system_copy(
            &screen,
            if mode == VimMode::Normal {
                "αβ🦀tail\n"
            } else {
                "β🦀"
            },
        );
        assert_eq!(screen.state().tab().unwrap().text, "αβ🦀tail\n");
        assert_eq!(screen.state().tab().unwrap().vim.mode, mode);
        assert_eq!(
            screen.state().tab().unwrap().selection,
            if mode == VimMode::Normal { 0..0 } else { 1..3 }
        );
    }
}

#[test]
fn copy_dispatch_preserves_an_explicit_native_selection_without_changing_mode() {
    let mut app = fixture("αβ🦀tail\n");
    let id = app.tab().unwrap().id;
    app.tab_mut().unwrap().set_cursor(1..3);
    let ctx = app.ctx.clone();
    ctx.memory_mut(|memory| memory.request_focus(app.document_widget_id(id)));
    let mut output = ctx.run_ui(
        egui::RawInput {
            events: vec![egui::Event::Copy],
            ..Default::default()
        },
        |_| app.handle_vim(),
    );
    output.textures_delta.clear();
    assert!(output.platform_output.commands.iter().any(|command| {
        matches!(command, egui::OutputCommand::CopyText(text) if text == "β🦀")
    }));
    assert_eq!(app.tab().unwrap().vim.mode, VimMode::Normal);
    assert_eq!(app.tab().unwrap().selection, 1..3);
    assert_unnamed(&app, UNNAMED, false);
}

#[test]
fn system_copy_without_a_selection_copies_the_current_line_and_keeps_mode() {
    for mode in [VimMode::Normal, VimMode::Insert] {
        let mut screen = harness(fixture("αβ🦀\nsecond\n"));
        if mode == VimMode::Insert {
            text(&mut screen, "i");
        }
        events(&mut screen, [key(Key::C, egui::Modifiers::CTRL)]);
        assert_system_copy(&screen, "αβ🦀\n");
        assert_eq!(screen.state().tab().unwrap().vim.mode, mode);
        assert!(screen.state().tab().unwrap().selection.is_empty());
        assert_eq!(screen.state().tab().unwrap().text, "αβ🦀\nsecond\n");
    }
}

#[test]
fn system_cut_preserves_unnamed_register_and_unicode_undo_in_every_mode() {
    for mode in [VimMode::Normal, VimMode::Visual, VimMode::Insert] {
        let mut screen = harness(fixture("αβ🦀tail\n"));
        select_native(&mut screen, mode);
        events(&mut screen, [egui::Event::Cut]);
        assert_system_copy(&screen, "β🦀");
        assert_eq!(screen.state().tab().unwrap().text, "αtail\n");
        events(&mut screen, [key(Key::Escape, egui::Modifiers::NONE)]);
        text(&mut screen, "u");
        assert_eq!(screen.state().tab().unwrap().text, "αβ🦀tail\n");
        assert_unnamed(screen.state(), UNNAMED, false);
    }
}

#[test]
fn system_cut_without_a_selection_removes_the_current_line_and_is_undoable() {
    for mode in [VimMode::Normal, VimMode::Insert] {
        let mut screen = harness(fixture("αβ🦀\nsecond\n"));
        if mode == VimMode::Insert {
            text(&mut screen, "i");
        }
        events(&mut screen, [key(Key::X, egui::Modifiers::CTRL)]);
        assert_system_copy(&screen, "αβ🦀\n");
        assert_eq!(screen.state().tab().unwrap().text, "second\n");
        events(&mut screen, [key(Key::Escape, egui::Modifiers::NONE)]);
        text(&mut screen, "u");
        assert_eq!(screen.state().tab().unwrap().text, "αβ🦀\nsecond\n");
    }
}

#[test]
fn system_paste_replaces_unicode_selection_and_preserves_register_in_every_mode() {
    for mode in [VimMode::Normal, VimMode::Visual, VimMode::Insert] {
        let mut screen = harness(fixture("αβ🦀tail\n"));
        select_native(&mut screen, mode);
        events(&mut screen, [egui::Event::Paste("δ😀".into())]);
        assert_eq!(screen.state().tab().unwrap().text, "αδ😀tail\n");
        assert_unnamed(screen.state(), UNNAMED, false);
        assert_no_clipboard_write(&screen);
        events(&mut screen, [key(Key::Escape, egui::Modifiers::NONE)]);
        text(&mut screen, "u");
        assert_eq!(screen.state().tab().unwrap().text, "αβ🦀tail\n");
        assert_unnamed(screen.state(), UNNAMED, false);
    }
}

#[test]
fn vim_p_uses_the_unnamed_register_after_a_system_copy() {
    let mut screen = harness(fixture("αβ🦀tail\n"));
    text(&mut screen, "lvl");
    events(&mut screen, [key(Key::C, egui::Modifiers::CTRL)]);
    assert_system_copy(&screen, "β🦀");
    assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::Visual);
    events(&mut screen, [key(Key::Escape, egui::Modifiers::NONE)]);
    text(&mut screen, "0p");
    assert_eq!(screen.state().tab().unwrap().text, "αKEEP🦀β🦀tail\n");
    assert_unnamed(screen.state(), UNNAMED, false);
    assert_no_clipboard_write(&screen);
}

#[test]
fn system_copy_and_paste_do_not_create_a_previously_empty_vim_register() {
    for paste in [false, true] {
        let mut app = fixture("αβ🦀tail\n");
        app.register = None;
        app.tab_mut().unwrap().vim = Default::default();
        let mut screen = harness(app);
        text(&mut screen, "lvl");
        if paste {
            events(&mut screen, [egui::Event::Paste("δ😀".into())]);
            assert_eq!(screen.state().tab().unwrap().text, "αδ😀tail\n");
        } else {
            events(&mut screen, [egui::Event::Copy]);
            assert_eq!(screen.state().tab().unwrap().text, "αβ🦀tail\n");
        }
        assert!(screen.state().register.is_none());
        assert!(screen.state().tab().unwrap().vim.register().is_none());
        events(&mut screen, [key(Key::Escape, egui::Modifiers::NONE)]);
        let before = screen.state().tab().unwrap().text.clone();
        text(&mut screen, "p");
        assert_eq!(screen.state().tab().unwrap().text, before);
        assert_no_clipboard_write(&screen);
    }
}

#[test]
fn ctrl_x_uses_the_system_clipboard_without_replacing_unnamed_register() {
    let mut screen = harness(fixture("αβ🦀tail\n"));
    text(&mut screen, "lvl");
    events(&mut screen, [key(Key::X, egui::Modifiers::CTRL)]);
    assert_system_copy(&screen, "β🦀");
    assert_eq!(screen.state().tab().unwrap().text, "αtail\n");
    text(&mut screen, "u");
    assert_eq!(screen.state().tab().unwrap().text, "αβ🦀tail\n");
}

#[test]
fn context_copy_and_cut_preserve_unnamed_register() {
    for action in [EditorAction::Copy, EditorAction::Cut] {
        let mut app = fixture("αβ🦀tail\n");
        let id = app.tab().unwrap().id;
        app.tab_mut().unwrap().set_cursor(1..3);
        let ctx = app.ctx.clone();
        let mut output = ctx.run_ui(egui::RawInput::default(), |_| {
            app.apply_editor_context_action(id, action);
        });
        output.textures_delta.clear();
        assert!(output.platform_output.commands.iter().any(|command| {
            matches!(command, egui::OutputCommand::CopyText(text) if text == "β🦀")
        }));
        assert_unnamed(&app, UNNAMED, false);
        if action == EditorAction::Cut {
            assert_eq!(app.tab().unwrap().text, "αtail\n");
            app.apply_editor_context_action(id, EditorAction::Undo);
        }
        assert_eq!(app.tab().unwrap().text, "αβ🦀tail\n");
    }
}

#[test]
fn context_paste_preserves_register_and_replaces_selection_once() {
    let mut app = fixture("αβ🦀tail\n");
    let id = app.tab().unwrap().id;
    app.tab_mut().unwrap().set_cursor(1..3);
    app.apply_editor_context_action(id, EditorAction::Paste);
    let ctx = app.ctx.clone();
    let mut output = ctx.run_ui(
        egui::RawInput {
            events: vec![egui::Event::Paste("δ😀".into())],
            ..Default::default()
        },
        |_| {
            app.context_paste_poll();
            app.context_paste_poll();
        },
    );
    output.textures_delta.clear();
    assert_eq!(app.tab().unwrap().text, "αδ😀tail\n");
    assert_unnamed(&app, UNNAMED, false);
    assert!(app.context_paste.is_none());
    app.apply_editor_context_action(id, EditorAction::Undo);
    assert_eq!(app.tab().unwrap().text, "αβ🦀tail\n");
    assert_unnamed(&app, UNNAMED, false);
}

fn tap(screen: &mut Harness<'_, VEditApp>, key_code: Key, modifiers: egui::Modifiers) {
    events(
        screen,
        [
            key(
                key_code,
                egui::Modifiers {
                    command: modifiers.ctrl,
                    ..modifiers
                },
            ),
            release(key_code),
        ],
    );
    screen.run_steps(3);
}

#[test]
fn visual_block_windows_clipboard_and_context_actions_preserve_vim_registers() {
    for context in [false, true] {
        let mut screen = harness(fixture("abcd\nefgh\nijkl"));
        text(&mut screen, "l");
        tap(&mut screen, Key::Q, egui::Modifiers::CTRL);
        text(&mut screen, "jl");
        let tab = screen.state().tab().unwrap();
        assert_eq!(tab.vim.mode, VimMode::VisualBlock);
        assert_eq!(tab.vim.block_selection(&tab.text), Some(vec![1..3, 6..8]));
        assert!(!tab.vim.multi.active());
        if context {
            let app = screen.state_mut();
            let id = app.tab().unwrap().id;
            let ctx = app.ctx.clone();
            let output = ctx.run_ui(egui::RawInput::default(), |_| {
                app.apply_editor_context_action(id, EditorAction::Copy)
            });
            assert!(output.platform_output.commands.iter().any(
                |command| matches!(command, egui::OutputCommand::CopyText(text) if text == "bc\nfg")
            ));
            app.apply_editor_context_action(id, EditorAction::Cut);
        } else {
            events(
                &mut screen,
                [
                    key(Key::C, egui::Modifiers::CTRL),
                    egui::Event::Copy,
                    release(Key::C),
                ],
            );
            assert_system_copy(&screen, "bc\nfg");
            events(
                &mut screen,
                [
                    key(Key::X, egui::Modifiers::CTRL),
                    egui::Event::Cut,
                    release(Key::X),
                ],
            );
        }
        assert_eq!(
            screen.state().tab().unwrap().text,
            "ad\neh\nijkl",
            "context={context}"
        );
        assert_unnamed(screen.state(), UNNAMED, false);
        // A menu action returns focus during the editor's next frame.
        screen.run_steps(3);
        tap(&mut screen, Key::Z, egui::Modifiers::CTRL);
        assert_eq!(
            screen.state().tab().unwrap().text,
            "abcd\nefgh\nijkl",
            "context={context}"
        );
    }
}

#[test]
fn visual_block_editor_line_commands_cover_all_rows_and_keep_virtual_columns() {
    for palette in [false, true] {
        for ending in ["\n", "\r\n"] {
            let original = ["top", "abcde", "x", "bottom"].join(ending);
            let moved = ["abcde", "x", "top", "bottom"].join(ending);
            let duplicated = ["abcde", "x", "abcde", "x", "top", "bottom"].join(ending);
            let mut screen = harness(fixture(&original));
            text(&mut screen, "jlll");
            tap(&mut screen, Key::Q, egui::Modifiers::CTRL);
            text(&mut screen, "jll");
            assert_eq!(
                screen
                    .state()
                    .tab()
                    .unwrap()
                    .vim
                    .block_selection_columns(&original),
                Some(vec![3..6, 3..6])
            );
            if palette {
                screen
                    .state_mut()
                    .run_shortcut_action(keybindings::Action::MoveLineUp);
            } else {
                tap(&mut screen, Key::ArrowUp, egui::Modifiers::ALT);
            }
            screen.run_steps(3);
            let tab = screen.state().tab().unwrap();
            assert_eq!(tab.text, moved, "palette={palette}, ending={ending:?}");
            assert_eq!(tab.vim.mode, VimMode::VisualBlock);
            assert_eq!(
                tab.vim.block_selection_columns(&tab.text),
                Some(vec![3..6, 3..6])
            );
            if palette {
                screen
                    .state_mut()
                    .run_shortcut_action(keybindings::Action::DuplicateLine);
            } else {
                tap(
                    &mut screen,
                    Key::D,
                    egui::Modifiers::ALT.plus(egui::Modifiers::SHIFT),
                );
            }
            screen.run_steps(3);
            let tab = screen.state().tab().unwrap();
            assert_eq!(tab.text, duplicated, "palette={palette}, ending={ending:?}");
            assert_eq!(tab.vim.mode, VimMode::VisualBlock);
            assert_eq!(
                tab.vim.block_selection_columns(&tab.text),
                Some(vec![3..6, 3..6])
            );
            for expected in [&moved, &original] {
                tap(&mut screen, Key::Z, egui::Modifiers::CTRL);
                assert_eq!(&screen.state().tab().unwrap().text, expected);
            }
            // Delete line shares the same line-area contract after rebuilding
            // the rectangle; an empty range on its last row must still count.
            {
                let tab = screen.state_mut().tab_mut().unwrap();
                tab.vim.set_cursor(
                    &tab.text,
                    "top".chars().count() + ending.chars().count() + 3,
                );
                tab.vim.handle_key(&mut tab.text, VimKey::VisualBlock);
                tab.vim.handle_key(&mut tab.text, VimKey::Char('j'));
            }
            if palette {
                screen
                    .state_mut()
                    .run_shortcut_action(keybindings::Action::DeleteLine);
            } else {
                tap(
                    &mut screen,
                    Key::K,
                    egui::Modifiers::CTRL.plus(egui::Modifiers::SHIFT),
                );
            }
            screen.run_steps(3);
            assert_eq!(
                screen.state().tab().unwrap().text,
                ["top", "bottom"].join(ending)
            );
            tap(&mut screen, Key::Z, egui::Modifiers::CTRL);
            assert_eq!(screen.state().tab().unwrap().text, original);
            assert!(!screen.state().tab().unwrap().vim.can_undo());
        }
    }
}

#[test]
fn vim_forwarding_owns_clipboard_chords_once_and_falls_back_in_insert() {
    use keybindings::vim_keys::Target;
    for semantic in [false, true] {
        let mut app = fixture("abcd\nefgh");
        app.settings.vim_keys.set(Key::V, Target::Vim);
        app.settings.vim_keys.set(Key::C, Target::Vim);
        let mut screen = harness(app);
        let mut batch = vec![key(Key::V, egui::Modifiers::CTRL)];
        if semantic {
            batch.push(egui::Event::Paste("must not paste".into()));
        }
        batch.push(release(Key::V));
        events(&mut screen, batch);
        assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::VisualBlock);
        assert_eq!(screen.state().tab().unwrap().text, "abcd\nefgh");
        text(&mut screen, "jl");
        events(
            &mut screen,
            [
                key(Key::C, egui::Modifiers::CTRL),
                egui::Event::Copy,
                release(Key::C),
            ],
        );
        assert_no_clipboard_write(&screen);
        assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::Normal);
        text(&mut screen, "i");
        events(
            &mut screen,
            [
                key(Key::V, egui::Modifiers::CTRL),
                egui::Event::Paste("Z".into()),
                release(Key::V),
            ],
        );
        assert_eq!(screen.state().tab().unwrap().text.matches('Z').count(), 1);
        assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::Insert);
        assert_unnamed(screen.state(), UNNAMED, false);
    }
}

#[test]
fn vim_control_preferences_override_custom_commands_and_preserve_other_contexts() {
    use keybindings::vim_keys::Target;
    for target in [Target::Default, Target::Editor, Target::Vim] {
        let mut app = fixture("foo foo\nfoo foo");
        app.settings
            .keybindings
            .insert("toggle_wrap".into(), Some("Ctrl+Q".into()));
        app.settings.vim_keys.set(Key::Q, target);
        let initial_wrap = app.settings.word_wrap;
        let mut screen = harness(app);
        tap(&mut screen, Key::Q, egui::Modifiers::CTRL);
        assert_eq!(
            screen.state().tab().unwrap().vim.mode == VimMode::VisualBlock,
            target == Target::Vim
        );
        assert_eq!(
            screen.state().settings.word_wrap != initial_wrap,
            target != Target::Vim
        );
    }
    let mut app = fixture("one\ntwo");
    app.settings
        .keybindings
        .insert("visual_block".into(), Some("Ctrl+Shift+Q".into()));
    let mut screen = harness(app);
    text(&mut screen, "i");
    tap(
        &mut screen,
        Key::Q,
        egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
    );
    assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::VisualBlock);
    let mut app = fixture("foo foo");
    app.settings.vim_keys.set(Key::F, Target::Editor);
    let mut screen = harness(app);
    tap(&mut screen, Key::F, egui::Modifiers::CTRL);
    assert!(screen.state().find_visible);
    screen.state_mut().find_visible = false;
    screen.state_mut().tab_mut().unwrap().focus = true;
    screen.run_steps(3);
    tap(&mut screen, Key::D, egui::Modifiers::CTRL);
    tap(&mut screen, Key::D, egui::Modifiers::CTRL);
    assert!(screen.state().tab().unwrap().vim.multi.active());
    screen
        .state_mut()
        .settings
        .vim_keys
        .set(Key::V, Target::Vim);
    events(&mut screen, [egui::Event::Paste("bar".into())]);
    assert_eq!(screen.state().tab().unwrap().text, "bar bar");
    assert_ne!(screen.state().tab().unwrap().vim.mode, VimMode::VisualBlock);
}

#[test]
fn visual_block_context_paste_rejects_changed_selection_and_is_one_undo_step() {
    for stale in [false, true] {
        let mut screen = harness(fixture("abcd\nefgh\nijkl"));
        text(&mut screen, "l");
        tap(&mut screen, Key::Q, egui::Modifiers::CTRL);
        text(&mut screen, "jl");
        let app = screen.state_mut();
        let id = app.tab().unwrap().id;
        app.apply_editor_context_action(id, EditorAction::Paste);
        if stale {
            let tab = app.tab_mut().unwrap();
            tab.vim.handle_key(&mut tab.text, VimKey::Char('j'));
        }
        let ctx = app.ctx.clone();
        let _ = ctx.run_ui(
            egui::RawInput {
                events: vec![egui::Event::Paste("X".into())],
                ..Default::default()
            },
            |_| app.context_paste_poll(),
        );
        assert_eq!(
            app.tab().unwrap().text,
            if stale {
                "abcd\nefgh\nijkl"
            } else {
                "aXd\neXh\nijkl"
            }
        );
        assert_unnamed(app, UNNAMED, false);
        if !stale {
            app.apply_editor_context_action(id, EditorAction::Undo);
        }
        assert_eq!(app.tab().unwrap().text, "abcd\nefgh\nijkl");
    }
}

#[test]
fn visual_block_and_vim_key_settings_render() {
    let mut screen = visual_tests::harness(fixture(
        "name   count\nalpha  12\nbravo  34\ncharl  56\n\nBlock selections stay rectangular.\n",
    ));
    text(&mut screen, "j");
    tap(&mut screen, Key::Q, egui::Modifiers::CTRL);
    text(&mut screen, "2j4l");
    assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::VisualBlock);
    visual_tests::snapshot(&mut screen, "vedit_visual_block");
    screen.state_mut().open_settings_section("Vim");
    screen.run_steps(3);
    visual_tests::snapshot(&mut screen, "vedit_vim_key_settings");
}

#[test]
fn double_escape_hides_highlights_and_n_restores_the_retained_query() {
    let mut screen = harness(fixture("a 🦀 b 🦀 c 🦀\n"));
    events(
        &mut screen,
        [
            egui::Event::Text("/🦀".into()),
            key(Key::Enter, egui::Modifiers::NONE),
        ],
    );
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 2);
    assert!(screen.state().find.highlighted);
    events(&mut screen, [key(Key::Escape, egui::Modifiers::NONE)]);
    assert!(screen.state().find.highlighted);
    events(
        &mut screen,
        [
            release(Key::Escape),
            key(Key::Escape, egui::Modifiers::NONE),
        ],
    );
    assert!(!screen.state().find.highlighted);
    assert!(screen.state_mut().editor_search_matches().is_empty());
    text(&mut screen, "n");
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 6);
    assert!(screen.state().find.highlighted);
    assert_eq!(screen.state_mut().editor_search_matches().len(), 3);
    text(&mut screen, "N");
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 2);
}

#[test]
fn intervening_motion_breaks_the_double_escape_sequence() {
    let mut screen = harness(fixture("a 🦀 b 🦀\n"));
    events(
        &mut screen,
        [
            egui::Event::Text("/🦀".into()),
            key(Key::Enter, egui::Modifiers::NONE),
            key(Key::Escape, egui::Modifiers::NONE),
            egui::Event::Text("h".into()),
            release(Key::Escape),
            key(Key::Escape, egui::Modifiers::NONE),
        ],
    );
    assert!(screen.state().find.highlighted);
}

#[test]
fn swap_jk_applies_to_counted_motions_and_operators() {
    let mut app = fixture("a\nb\nc\nd\n");
    app.settings.swap_jk = true;
    let mut screen = harness(app);
    text(&mut screen, "2k");
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 4);
    text(&mut screen, "dj");
    assert_eq!(screen.state().tab().unwrap().text, "a\nd\n");
    assert_unnamed(screen.state(), "b\nc\n", true);
    assert_no_clipboard_write(&screen);
    text(&mut screen, "u");
    assert_eq!(screen.state().tab().unwrap().text, "a\nb\nc\nd\n");
}

#[test]
fn swap_jk_preserves_literal_find_replacement_and_insert_characters() {
    for (command, expected_text, expected_cursor) in [
        ("rj", "j jk\n", 0),
        ("fk", "a jk\n", 3),
        ("ijk", "jka jk\n", 2),
    ] {
        let mut app = fixture("a jk\n");
        app.settings.swap_jk = true;
        let mut screen = harness(app);
        text(&mut screen, command);
        assert_eq!(screen.state().tab().unwrap().text, expected_text);
        assert_eq!(screen.state().tab().unwrap().vim.cursor, expected_cursor);
    }
}

#[test]
fn alt_a_and_alt_x_change_numbers_and_unsupported_commands_report_in_the_status_bar() {
    let mut screen = harness(fixture("width 9 and -3\n"));
    screen.state_mut().settings.keybindings.extend([
        (
            keybindings::Action::ReviewNextFile.id().into(),
            Some("Alt+A".into()),
        ),
        (
            keybindings::Action::ReviewPreviousFile.id().into(),
            Some("Alt+X".into()),
        ),
    ]);
    events(
        &mut screen,
        [
            key(Key::A, egui::Modifiers::ALT),
            egui::Event::Text("a".into()),
        ],
    );
    assert_eq!(screen.state().tab().unwrap().text, "width 10 and -3\n");
    text(&mut screen, "w5");
    events(
        &mut screen,
        [
            key(Key::X, egui::Modifiers::ALT),
            egui::Event::Text("x".into()),
        ],
    );
    assert_eq!(screen.state().tab().unwrap().text, "width 10 and -8\n");
    // Ctrl+A keeps selecting all text.
    events(&mut screen, [key(Key::A, egui::Modifiers::CTRL)]);
    assert_eq!(screen.state().tab().unwrap().text, "width 10 and -8\n");

    text(&mut screen, "0qa");
    let status = screen
        .state()
        .status
        .as_ref()
        .map(|status| status.0.clone());
    assert_eq!(status.as_deref(), Some("Not supported in vEdit: q"));
    assert_eq!(screen.state().tab().unwrap().text, "width 10 and -8\n");
    text(&mut screen, "x");
    assert_eq!(
        screen.state().tab().unwrap().text,
        "idth 10 and -8\n",
        "the recorded register name is consumed, later keys work again"
    );
    text(&mut screen, "u");
    text(&mut screen, "u");
    text(&mut screen, "u");
    assert_eq!(screen.state().tab().unwrap().text, "width 9 and -3\n");
}

#[test]
fn swap_jk_is_persistent_and_defaults_to_standard_vim_directions() {
    let defaults: Settings = serde_json::from_str("{}").unwrap();
    assert!(defaults.vim);
    assert!(!defaults.swap_jk);
    let saved: Settings = serde_json::from_str(r#"{"swap_jk":true}"#).unwrap();
    assert!(saved.swap_jk);
    let roundtrip: Settings =
        serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
    assert!(roundtrip.swap_jk);
}

#[test]
fn clipboard_key_provenance_and_semantic_events_execute_once_in_every_editor_mode() {
    use keybindings::vim_keys::Target;
    for mode in [
        None,
        Some(VimMode::Normal),
        Some(VimMode::Insert),
        Some(VimMode::Visual),
    ] {
        for alternatives in [false, true] {
            let mut app = fixture("abcd");
            app.settings.vim = mode.is_some();
            if alternatives {
                for key in [Key::C, Key::X, Key::V] {
                    app.settings.vim_keys.set(key, Target::Vim);
                }
            }
            let mut screen = harness(app);
            select_native(&mut screen, mode.unwrap_or(VimMode::Normal));
            screen.run_steps(1);
            // Normal mode has a block cursor, so native clipboard commands
            // operate on its current line; the other cases retain "bc".
            let (copied, cut_text, pasted_text) = if mode == Some(VimMode::Normal) {
                ("abcd", "", "Z")
            } else {
                ("bc", "ad", "aZd")
            };
            let (copy_key, cut_key, paste_key, cut_modifiers, paste_modifiers) = if alternatives {
                (
                    Key::Insert,
                    Key::Delete,
                    Key::Insert,
                    egui::Modifiers::SHIFT,
                    egui::Modifiers::SHIFT,
                )
            } else {
                (
                    Key::C,
                    Key::X,
                    Key::V,
                    egui::Modifiers::CTRL,
                    egui::Modifiers::CTRL,
                )
            };
            events(
                &mut screen,
                [
                    key(copy_key, egui::Modifiers::CTRL),
                    egui::Event::Copy,
                    release(copy_key),
                ],
            );
            super::editor_input_tests::settle(&mut screen);
            let copies: Vec<_> = screen
                .output()
                .platform_output
                .commands
                .iter()
                .filter_map(|command| {
                    if let egui::OutputCommand::CopyText(value) = command {
                        Some(value.as_str())
                    } else {
                        None
                    }
                })
                .collect();
            assert_eq!(
                copies,
                vec![copied],
                "copy mode={mode:?}, alternatives={alternatives}"
            );
            assert_eq!(screen.state().tab().unwrap().text, "abcd");

            events(
                &mut screen,
                [
                    key(cut_key, cut_modifiers),
                    egui::Event::Cut,
                    release(cut_key),
                ],
            );
            super::editor_input_tests::settle(&mut screen);
            assert_eq!(
                screen.state().tab().unwrap().text,
                cut_text,
                "cut mode={mode:?}, alternatives={alternatives}"
            );
            let copies: Vec<_> = screen
                .output()
                .platform_output
                .commands
                .iter()
                .filter_map(|command| {
                    if let egui::OutputCommand::CopyText(value) = command {
                        Some(value.as_str())
                    } else {
                        None
                    }
                })
                .collect();
            assert_eq!(
                copies,
                vec![copied],
                "cut clipboard mode={mode:?}, alternatives={alternatives}"
            );

            events(
                &mut screen,
                [
                    key(paste_key, paste_modifiers),
                    egui::Event::Paste("Z".into()),
                    release(paste_key),
                ],
            );
            super::editor_input_tests::settle(&mut screen);
            assert_eq!(
                screen.state().tab().unwrap().text,
                pasted_text,
                "paste mode={mode:?}, alternatives={alternatives}"
            );
            assert_unnamed(screen.state(), UNNAMED, false);
        }
    }
}

//! Macros traverse the same native input, search, Ex and history boundaries.
use super::*;
use egui_kittest::Harness;

fn harness(text: &str) -> Harness<'static, VEditApp> {
    let mut app = VEditApp::empty(egui::Context::default());
    app.settings.vim = true;
    app.settings.autosave = false;
    app.settings.smooth_scroll = false;
    app.sidebar_visible = false;
    let mut project = Project::new(std::env::temp_dir().join("vedit-vim-macros"));
    let mut tab = EditorTab::new(None, "macro.txt".into());
    tab.text = text.into();
    tab.saved_text = text.into();
    project.tabs.push(tab);
    app.projects.push(project);
    app.bind_current_pane();
    let mut screen = Harness::builder()
        .with_size([1000.0, 700.0])
        .with_os(egui::os::OperatingSystem::Windows)
        .build_ui_state(
            |ui, app: &mut VEditApp| {
                app.ctx = ui.ctx().clone();
                app.prepare_vim_macros();
                app.shortcuts();
                app.handle_vim();
                app.apply_pending_jump();
                app.editor_ui(ui);
                app.observe_navigation();
                app.finish_vim_macros();
            },
            app,
        );
    set_style(&screen.ctx);
    screen.run_steps(3);
    screen
}

fn input(screen: &mut Harness<'_, VEditApp>, text: &str) {
    screen
        .input_mut()
        .events
        .push(egui::Event::Text(text.into()));
    screen.run_steps(1);
    settle(screen);
}

fn escape(screen: &mut Harness<'_, VEditApp>) {
    screen
        .input_mut()
        .events
        .push(vim_macros::key(Key::Escape, egui::Modifiers::NONE));
    screen.input_mut().events.push(egui::Event::Key {
        key: Key::Escape,
        physical_key: None,
        pressed: false,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    });
    screen.run_steps(1);
    settle(screen);
}

fn settle(screen: &mut Harness<'_, VEditApp>) {
    for _ in 0..1000 {
        let app = screen.state();
        if !app.vim_macros.active() && !app.editor_input_pending() && !app.vim_file_mark_pending() {
            return;
        }
        screen.run_steps(1);
    }
    panic!("macro input did not finish");
}

fn text(screen: &Harness<'_, VEditApp>) -> String {
    screen.state().tab().unwrap().text.clone()
}

#[test]
fn multicursor_recording_replay_counts_and_undo_use_the_shared_macro_owner() {
    for (recorded, after_recording, after_replay) in [
        ("x", "bcd bcd", "d d"),
        ("i!", "!abcd !abcd", "!!!abcd !!!abcd"),
    ] {
        let mut screen = harness("abcd abcd");
        {
            let tab = screen.state_mut().tab_mut().unwrap();
            tab.vim.multi.selections = vec![
                crate::multicursor::Selection::new(0, 0),
                crate::multicursor::Selection::new(5, 5),
            ];
            tab.sync_multicursor();
        }
        screen.run_steps(2);
        input(&mut screen, &format!("qa{recorded}"));
        if screen.state().tab().unwrap().vim.mode == VimMode::Insert {
            escape(&mut screen);
        }
        input(&mut screen, "q");
        assert_eq!(text(&screen), after_recording, "{recorded}");
        assert!(screen.state().vim_macros.label().is_none());
        assert!(
            screen
                .state()
                .vim_registers
                .get('a')
                .unwrap()
                .macro_events
                .is_some()
        );
        input(&mut screen, "2@a");
        assert_eq!(text(&screen), after_replay, "{recorded}");
        input(&mut screen, "u");
        assert_eq!(
            text(&screen),
            after_recording,
            "{recorded}: both repetitions form one Undo group"
        );
    }
}

#[test]
fn recording_can_activate_multicursor_and_stop_there_then_replay_the_activation() {
    let mut screen = harness("one one");
    input(&mut screen, "qa");
    screen.input_mut().events.push(vim_macros::key(
        Key::L,
        egui::Modifiers::CTRL | egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
    ));
    screen.run_steps(2);
    assert!(screen.state().tab().unwrap().vim.multi.active());
    escape(&mut screen);
    input(&mut screen, "brXq");
    assert_eq!(text(&screen), "Xne Xne");
    assert!(screen.state().vim_macros.label().is_none());
    let value = screen.state().vim_registers.get('a').unwrap();
    assert!(value.macro_events.as_ref().unwrap().iter().any(|event| matches!(event, egui::Event::Key { key: Key::L, modifiers, .. } if modifiers.ctrl && modifiers.shift)));
    input(&mut screen, "u");
    assert_eq!(text(&screen), "one one");
    escape(&mut screen);
    assert!(!screen.state().tab().unwrap().vim.multi.active());
    input(&mut screen, "@a");
    assert_eq!(text(&screen), "Xne Xne");
}

#[test]
fn multicursor_macro_records_clipboard_payload_ime_and_insert_register_once() {
    let mut screen = harness("a a");
    {
        let tab = screen.state_mut().tab_mut().unwrap();
        tab.vim.multi.selections = vec![
            crate::multicursor::Selection::new(0, 0),
            crate::multicursor::Selection::new(2, 2),
        ];
        tab.sync_multicursor();
    }
    screen.state_mut().vim_registers.set_text(
        'c',
        Yank {
            text: "Z".into(),
            linewise: false,
            block_width: None,
        },
    );
    screen.run_steps(2);
    input(&mut screen, "qbi");
    screen.input_mut().events.extend([
        vim_macros::key(Key::V, egui::Modifiers::CTRL | egui::Modifiers::COMMAND),
        egui::Event::Paste("P".into()),
        egui::Event::Ime(egui::ImeEvent::Commit("中".into())),
        vim_macros::key(Key::R, egui::Modifiers::CTRL | egui::Modifiers::COMMAND),
        egui::Event::Text("c!".into()),
        vim_macros::key(Key::Escape, egui::Modifiers::NONE),
    ]);
    screen.run_steps(1);
    settle(&mut screen);
    input(&mut screen, "q");
    assert_eq!(text(&screen), "P中Z!a P中Z!a");
    let value = screen.state().vim_registers.get('b').unwrap();
    assert!(
        !value
            .macro_events
            .as_ref()
            .unwrap()
            .iter()
            .any(|event| matches!(event, egui::Event::Key { key: Key::V, .. }))
    );
    input(&mut screen, "u@b");
    assert_eq!(text(&screen), "P中Z!a P中Z!a");
}

#[test]
fn recording_counts_repeat_append_and_undo_share_real_native_input() {
    let mut screen = harness("α\r\nβ\r\nγ\r\nδ");
    input(&mut screen, "qaA!");
    escape(&mut screen);
    input(&mut screen, "jq");
    assert_eq!(text(&screen), "α!\r\nβ\r\nγ\r\nδ");
    input(&mut screen, "2@a");
    assert_eq!(text(&screen), "α!\r\nβ!\r\nγ!\r\nδ");
    input(&mut screen, "u");
    assert_eq!(text(&screen), "α!\r\nβ\r\nγ\r\nδ");
    input(&mut screen, "@@");
    assert_eq!(text(&screen), "α!\r\nβ!\r\nγ\r\nδ");
    input(&mut screen, "qAA?");
    escape(&mut screen);
    input(&mut screen, "q");
    input(&mut screen, "gg@a");
    assert_eq!(text(&screen), "α!!\r\nβ!?\r\nγ?\r\nδ");
}

#[test]
fn recording_macro_calls_keeps_calls_and_can_execute_yanked_text() {
    let mut screen = harness("one two three four");
    input(&mut screen, "qaxq");
    input(&mut screen, "qb@aq");
    let value = screen.state().vim_registers.get('b').unwrap();
    assert_eq!(value.yank.text, "@a");
    input(&mut screen, "@b");
    assert_eq!(text(&screen), " two three four");
    let mut bank = screen.state().vim_registers.clone();
    bank.set_text(
        'c',
        Yank {
            text: "A!\u{1b}".into(),
            linewise: false,
            block_width: None,
        },
    );
    screen.state_mut().vim_registers = bank;
    input(&mut screen, "@c");
    assert_eq!(text(&screen), " two three four!");
    let before = text(&screen);
    let mut bank = screen.state().vim_registers.clone();
    bank.set_text(
        'd',
        Yank {
            text: "@zx".into(),
            linewise: false,
            block_width: None,
        },
    );
    screen.state_mut().vim_registers = bank;
    input(&mut screen, "@d");
    assert_eq!(
        text(&screen),
        before,
        "an empty nested register discards the internal suffix"
    );
}

#[test]
fn text_register_crlf_is_one_input_boundary_and_large_macros_are_rejected_before_decoding() {
    let mut screen = harness("abcd");
    screen.state_mut().vim_registers.set_text(
        'a',
        Yank {
            text: "iX\r\n\u{1b}".into(),
            linewise: false,
            block_width: None,
        },
    );
    input(&mut screen, "@a");
    assert_eq!(text(&screen), "X\nabcd");
    let before = text(&screen);
    screen.state_mut().vim_registers.set_text(
        'b',
        Yank {
            text: "x".repeat(100_001),
            linewise: false,
            block_width: None,
        },
    );
    input(&mut screen, "@b");
    assert_eq!(text(&screen), before);
    assert!(!screen.state().vim_macros.active());
    assert!(
        screen
            .state()
            .status
            .as_ref()
            .is_some_and(|(message, _, _)| message.contains("limit"))
    );
}

#[test]
fn macro_altgr_input_does_not_leave_modifier_keys_pressed() {
    let mut screen = harness("word");
    input(&mut screen, "qai");
    let modifiers = egui::Modifiers::CTRL | egui::Modifiers::ALT | egui::Modifiers::COMMAND;
    screen.input_mut().events.extend([
        vim_macros::key(Key::AltRight, modifiers),
        vim_macros::key(Key::E, modifiers),
        egui::Event::Text("€".into()),
        egui::Event::Key {
            key: Key::AltRight,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        },
    ]);
    screen.run_steps(1);
    escape(&mut screen);
    input(&mut screen, "q@a");
    assert_eq!(text(&screen), "€€word");
    assert!(!screen.ctx.input(|input| input.key_down(Key::AltRight)));
    assert!(!screen.ctx.input(|input| input.key_down(Key::Escape)));
}

#[test]
fn replay_waits_for_search_and_ex_and_replace_uses_the_same_history_group() {
    let mut screen = harness("one two\none two\none two");
    input(&mut screen, "qa/two");
    screen
        .input_mut()
        .events
        .push(vim_macros::key(Key::Enter, egui::Modifiers::NONE));
    screen.run_steps(2);
    input(&mut screen, "RXYZ");
    escape(&mut screen);
    input(&mut screen, "q");
    input(&mut screen, "@a");
    assert_eq!(text(&screen), "one XYZ\none XYZ\none two");
    input(&mut screen, "u");
    assert_eq!(text(&screen), "one XYZ\none two\none two");
    input(&mut screen, "j");
    input(&mut screen, "qb:s/one/ONE/");
    screen
        .input_mut()
        .events
        .push(vim_macros::key(Key::Enter, egui::Modifiers::NONE));
    screen.run_steps(2);
    input(&mut screen, "jq");
    input(&mut screen, "@b");
    assert_eq!(text(&screen), "one XYZ\nONE two\nONE two");
}

#[test]
fn unexpected_navigation_or_external_edits_cancel_macro_input_and_keep_external_history() {
    for (focus, finished_insert) in [(true, false), (false, false), (false, true)] {
        let mut screen = harness("source");
        let source = screen.state().tab().unwrap().id;
        screen.state_mut().vim_registers.set_macro(
            'a',
            Yank {
                text: "A!\u{1b}x".into(),
                linewise: false,
                block_width: None,
            },
            if finished_insert {
                vec![egui::Event::Text("A!".into())]
            } else {
                vec![
                    egui::Event::Text("A!".into()),
                    vim_macros::key(Key::Escape, egui::Modifiers::NONE),
                    egui::Event::Text("x".into()),
                ]
            },
        );
        screen.event(egui::Event::Text("@a".into()));
        screen.run_steps(2);
        if finished_insert {
            screen.run_steps(1);
        }
        assert_eq!(screen.state().vim_macros.active(), !finished_insert);
        assert_eq!(text(&screen), "source!");
        if focus {
            screen.state_mut().new_file();
            let tab = screen.state_mut().tab_mut().unwrap();
            tab.text = "target".into();
            tab.saved_text = tab.text.clone();
        } else {
            let tab = screen.state_mut().tab_mut().unwrap();
            tab.vim.finish_insert(&mut tab.text);
            tab.vim.begin_external_edit(&tab.text);
            tab.text.push_str("EXTERNAL");
            tab.changed();
        }
        screen.run_steps(4);
        assert!(!screen.state().vim_macros.active());
        if focus {
            assert_eq!(text(&screen), "target");
            assert_eq!(
                screen.state_mut().tab_by_id_mut(source).unwrap().text,
                "source!"
            );
        } else {
            assert_eq!(text(&screen), "source!EXTERNAL");
            input(&mut screen, "u");
            assert_eq!(text(&screen), "source!");
            input(&mut screen, "u");
            assert_eq!(text(&screen), "source");
        }
    }
}

#[test]
fn closing_the_last_document_finishes_playback_and_releases_later_input() {
    let mut screen = harness("clean");
    let mut bank = screen.state().vim_registers.clone();
    bank.set_text(
        'a',
        Yank {
            text: ":q\r".into(),
            linewise: false,
            block_width: None,
        },
    );
    screen.state_mut().vim_registers = bank;
    input(&mut screen, "@a");
    assert!(screen.state().tab().is_none());
    assert!(!screen.state().vim_macros.active());
    screen.state_mut().new_file();
    screen.run_steps(2);
    input(&mut screen, "iwork");
    escape(&mut screen);
    assert_eq!(text(&screen), "work");
}

#[test]
fn definition_navigation_in_macros_waits_and_drops_input_after_failure_or_staleness() {
    fn text_at(app: &VEditApp, id: u64) -> &str {
        &app.projects
            .iter()
            .flat_map(|project| &project.tabs)
            .find(|tab| tab.id == id)
            .unwrap()
            .text
    }
    for metadata in [false, true] {
        for outcome in ["found", "missing", "stale", "canceled", "unavailable"] {
            let mut screen = harness("source");
            let (server, events, _requests) =
                crate::lsp::LanguageServer::test_channel_with_requests();
            let (source, target, target_path) = {
                let app = screen.state_mut();
                let source = app.tab().unwrap().id;
                let target_path = app.project().unwrap().root.join("target.rs");
                let mut target = EditorTab::new(Some(target_path.clone()), "target.rs".into());
                target.text = "target".into();
                target.saved_text = target.text.clone();
                let target_id = target.id;
                app.projects[0].tabs.push(target);
                if outcome != "unavailable" {
                    let path = app.project().unwrap().root.join("source.rs");
                    app.tab_mut().unwrap().path = Some(path.clone());
                    app.projects[0].rust_project = true;
                    let session = app.projects[0].rust_language_mut();
                    session.server = Some(server);
                    session.ready = true;
                    session.opened.insert(path, (0, 1));
                    session.opened.insert(target_path.clone(), (0, 1));
                }
                let yank = Yank {
                    text: "gdx".into(),
                    linewise: false,
                    block_width: None,
                };
                if metadata {
                    app.vim_registers
                        .set_macro('a', yank, vec![egui::Event::Text("gdx".into())]);
                } else {
                    app.vim_registers.set_text('a', yank);
                }
                (source, target_id, target_path)
            };
            screen
                .input_mut()
                .events
                .push(egui::Event::Text("@a".into()));
            screen.run_steps(5);
            assert_eq!(
                text_at(screen.state(), source),
                "source",
                "{metadata} {outcome}"
            );
            assert_eq!(
                text_at(screen.state(), target),
                "target",
                "{metadata} {outcome}"
            );
            if outcome == "unavailable" {
                assert!(!screen.state().vim_macros.active());
                continue;
            }
            let request = screen.state().definition_request.unwrap().0;
            assert!(screen.state().vim_macros.active());
            match outcome {
                "found" => events
                    .send(crate::lsp::LspEvent::Definition {
                        request_id: request,
                        path: target_path,
                        line: 0,
                        column: 0,
                    })
                    .unwrap(),
                "missing" => events
                    .send(crate::lsp::LspEvent::DefinitionNotFound {
                        request_id: request,
                    })
                    .unwrap(),
                "stale" => screen.state_mut().tab_mut().unwrap().set_cursor(1..1),
                "canceled" => {
                    escape(&mut screen);
                    events
                        .send(crate::lsp::LspEvent::Definition {
                            request_id: request,
                            path: target_path,
                            line: 0,
                            column: 0,
                        })
                        .unwrap();
                }
                _ => unreachable!(),
            }
            screen.state_mut().poll_languages();
            settle(&mut screen);
            assert_eq!(
                text_at(screen.state(), source),
                "source",
                "{metadata} {outcome}"
            );
            assert_eq!(
                text_at(screen.state(), target),
                if outcome == "found" {
                    "arget"
                } else {
                    "target"
                },
                "{metadata} {outcome}"
            );
            assert_eq!(
                screen.state().tab().unwrap().id,
                if outcome == "found" { target } else { source },
                "{metadata} {outcome}"
            );
            if outcome == "found" {
                input(&mut screen, "u");
                assert_eq!(text_at(screen.state(), target), "target");
            }
        }
    }
}

#[test]
fn recursive_macro_is_bounded_and_escape_cancels_playback_without_losing_typing() {
    let mut screen = harness("abcd");
    let mut bank = screen.state().vim_registers.clone();
    bank.set_text(
        'a',
        Yank {
            text: "@a".into(),
            linewise: false,
            block_width: None,
        },
    );
    bank.set_text(
        'b',
        Yank {
            text: "x".into(),
            linewise: false,
            block_width: None,
        },
    );
    bank.set_text(
        'c',
        Yank {
            text: "jx".into(),
            linewise: false,
            block_width: None,
        },
    );
    screen.state_mut().vim_registers = bank;
    input(&mut screen, "@a");
    assert!(!screen.state().vim_macros.active());
    let mut bank = screen.state().vim_registers.clone();
    bank.set_text(
        'd',
        Yank {
            text: "'Zx".into(),
            linewise: false,
            block_width: None,
        },
    );
    screen.state_mut().vim_registers = bank;
    input(&mut screen, "@d");
    screen.run_steps(3);
    assert_eq!(
        text(&screen),
        "abcd",
        "a failed file mark drops the synthetic suffix"
    );
    input(&mut screen, "99@c");
    assert_eq!(
        text(&screen),
        "abcd",
        "a failed motion stops the remaining macro commands and repetitions"
    );
    screen
        .input_mut()
        .events
        .push(egui::Event::Text("999@b".into()));
    screen.run_steps(1);
    assert!(screen.state().vim_macros.active());
    escape(&mut screen);
    assert!(!screen.state().vim_macros.active());
    input(&mut screen, "iZ");
    escape(&mut screen);
    assert!(text(&screen).starts_with('Z'));
}

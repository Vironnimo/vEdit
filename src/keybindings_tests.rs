use super::*;

fn event(key: Key, modifiers: egui::Modifiers) -> egui::Event {
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

#[test]
fn defaults_are_valid_unique_and_use_native_safe_clipboard_keys() {
    assert!(validate(&Bindings::new()).is_ok());
    for &action in Action::ALL {
        for stroke in strokes(&Bindings::new(), action) {
            stroke.safe(action).unwrap();
        }
        assert_eq!(Action::from_id(action.id()), Some(action));
        assert_eq!(Action::from_label(action.label()), Some(action));
    }
    assert_eq!(
        label(&Bindings::new(), Action::MarkdownPreview),
        "Ctrl+Alt+M"
    );
}

#[test]
fn reopen_upgrade_preserves_an_existing_custom_shortcut() {
    for value in ["Ctrl+Shift+T", " shift + ctrl + t "] {
        let mut bindings = Bindings::from([(Action::ToggleWrap.id().into(), Some(value.into()))]);
        migrate_reopen_shortcut(&mut bindings);
        assert_eq!(bindings.get(Action::ReopenTab.id()), Some(&None));
        assert_eq!(bindings[Action::ToggleWrap.id()].as_deref(), Some(value));
        assert!(validate(&bindings).is_ok());
        let key = event(Key::T, egui::Modifiers::CTRL | egui::Modifiers::SHIFT);
        assert!(matches(&bindings, Action::ToggleWrap, &key));
        assert!(!matches(&bindings, Action::ReopenTab, &key));
        let persisted = serde_json::to_string(&bindings).unwrap();
        let mut restored: Bindings = serde_json::from_str(&persisted).unwrap();
        migrate_reopen_shortcut(&mut restored);
        assert_eq!(restored, bindings);
    }
}

#[test]
fn reopen_upgrade_leaves_explicit_choices_and_unrelated_bindings_unchanged() {
    let existing = (Action::ToggleWrap.id().into(), Some("Ctrl+Shift+T".into()));
    for reopen in [None, Some("Ctrl+Shift+T".into()), Some("Ctrl+Alt+T".into())] {
        let mut bindings =
            Bindings::from([existing.clone(), (Action::ReopenTab.id().into(), reopen)]);
        let original = bindings.clone();
        migrate_reopen_shortcut(&mut bindings);
        assert_eq!(bindings, original);
    }
    for mut bindings in [
        Bindings::new(),
        Bindings::from([("unknown_action".into(), Some("Ctrl+Shift+T".into()))]),
        Bindings::from([(Action::ToggleWrap.id().into(), None)]),
        Bindings::from([(Action::ToggleWrap.id().into(), Some("Ctrl+Alt+T".into()))]),
        Bindings::from([(Action::ToggleWrap.id().into(), Some("invalid".into()))]),
    ] {
        let original = bindings.clone();
        migrate_reopen_shortcut(&mut bindings);
        assert_eq!(bindings, original);
        assert!(matches(
            &bindings,
            Action::ReopenTab,
            &event(Key::T, egui::Modifiers::CTRL | egui::Modifiers::SHIFT)
        ));
    }
}

#[test]
fn shortcut_parser_accepts_case_insensitive_names_and_literal_plus() {
    for (value, expected) in [
        (" ctrl + shift + f8 ", "Ctrl+Shift+F8"),
        ("alt+arrowdown", "Alt+Down"),
        ("CTRL+BACKSLASH", "Ctrl+Backslash"),
        ("Ctrl++", "Ctrl+Plus"),
        ("Ctrl+Shift++", "Ctrl+Shift+Plus"),
    ] {
        assert_eq!(Stroke::parse(value).unwrap().label(), expected, "{value}");
    }
    for value in ["Ctrl+", "Ctrl+++", "Ctrl++F8", "Ctrl+Shift+"] {
        assert!(Stroke::parse(value).is_err(), "{value}");
    }
}

#[test]
fn every_supported_function_key_can_be_recorded_without_modifiers() {
    for &key in Key::ALL {
        if key.name().strip_prefix('F').is_some_and(|number| {
            number
                .parse::<usize>()
                .is_ok_and(|number| (1..=35).contains(&number))
        }) {
            Stroke {
                key,
                ctrl: false,
                alt: false,
                shift: false,
            }
            .safe(Action::Save)
            .unwrap();
        }
    }
    let bindings = Bindings::from([(Action::Save.id().into(), Some("f35".into()))]);
    assert!(validate(&bindings).is_ok());
    assert!(matches(
        &bindings,
        Action::Save,
        &event(Key::F35, egui::Modifiers::NONE)
    ));
}

#[test]
fn altgr_emitted_text_wins_without_disabling_actual_ctrl_alt_shortcuts() {
    let ctx = egui::Context::default();
    let chord = egui::Modifiers::CTRL.plus(egui::Modifiers::ALT);
    let real_shortcut = event(Key::E, chord);
    let normal_altgr = event(Key::E, egui::Modifiers::NONE);
    let right_alt = event(Key::AltRight, chord);
    let mut release_right_alt = event(Key::AltRight, egui::Modifiers::NONE);
    if let egui::Event::Key { pressed, .. } = &mut release_right_alt {
        *pressed = false;
    }
    ctx.input_mut(|input| {
        input.events = vec![
            right_alt.clone(),
            real_shortcut.clone(),
            egui::Event::Text("€".into()),
            event(Key::M, chord),
            egui::Event::Text("µ".into()),
            release_right_alt.clone(),
            normal_altgr.clone(),
            egui::Event::Text("€".into()),
            real_shortcut.clone(),
            egui::Event::Text("later typing".into()),
            event(Key::U, chord),
            egui::Event::Text("Y".into()),
            real_shortcut.clone(),
            egui::Event::Text("é".into()),
        ];
    });
    preserve_altgr_text(&ctx);
    assert_eq!(
        ctx.input(|input| input.events.clone()),
        vec![
            right_alt,
            egui::Event::Text("€".into()),
            egui::Event::Text("µ".into()),
            release_right_alt,
            normal_altgr,
            egui::Event::Text("€".into()),
            real_shortcut.clone(),
            egui::Event::Text("later typing".into()),
            event(Key::U, chord),
            egui::Event::Text("Y".into()),
            real_shortcut,
            egui::Event::Text("é".into()),
        ]
    );
}

#[test]
fn recording_altgr_text_does_not_create_a_shortcut_or_stop_recording() {
    let mut app = VEditApp::empty(egui::Context::default());
    app.keybinding_ui.capture = Some(Action::Save);
    let chord = egui::Modifiers::CTRL.plus(egui::Modifiers::ALT);
    app.ctx.input_mut(|input| {
        input.events = vec![
            event(Key::AltRight, chord),
            event(Key::E, chord),
            egui::Event::Text("€".into()),
        ];
    });
    app.capture_shortcut_input();
    assert_eq!(app.keybinding_ui.capture, Some(Action::Save));
    assert!(app.settings.keybindings.is_empty());
    assert!(app.ctx.input(|input| input.events.is_empty()));
}

#[test]
fn invalid_recorded_chord_can_be_corrected_by_pressing_another_without_reclicking() {
    let mut app = VEditApp::empty(egui::Context::default());
    app.keybinding_ui.capture = Some(Action::Save);
    app.ctx
        .input_mut(|input| input.events.push(event(Key::A, egui::Modifiers::NONE)));
    app.capture_shortcut_input();
    assert_eq!(app.keybinding_ui.capture, Some(Action::Save));
    assert!(app.keybinding_ui.feedback.as_ref().unwrap().1);
    assert!(app.settings.keybindings.is_empty());
    app.ctx
        .input_mut(|input| input.events.push(event(Key::F35, egui::Modifiers::NONE)));
    app.capture_shortcut_input();
    assert!(app.keybinding_ui.capture.is_none());
    assert_eq!(app.shortcut_label(Action::Save), "F35");
    assert!(!app.keybinding_ui.feedback.as_ref().unwrap().1);
}

#[test]
fn deferred_widget_shortcuts_are_cancelled_when_a_vim_command_prompt_opens() {
    let mut app = VEditApp::empty(egui::Context::default());
    app.ctx
        .run_ui(Default::default(), |_| {})
        .textures_delta
        .clear();
    app.defer_widget_shortcut_input(vec![event(Key::Z, egui::Modifiers::ALT)]);
    app.finish_shortcut_input();
    app.vim_command = Some("".into());
    app.ctx
        .run_ui(Default::default(), |_| {})
        .textures_delta
        .clear();
    app.resume_shortcut_input();
    assert!(app.ctx.input(|input| input.events.is_empty()));
    assert!(app.keybinding_ui.continuation.is_none());
}

#[test]
fn overrides_replace_all_aliases_disable_and_restore_defaults() {
    let mut bindings = Bindings::new();
    let old = event(Key::Y, egui::Modifiers::CTRL);
    let alternative = event(Key::Z, egui::Modifiers::CTRL.plus(egui::Modifiers::SHIFT));
    assert!(matches(&bindings, Action::Redo, &old));
    assert!(matches(&bindings, Action::Redo, &alternative));
    bindings.insert(Action::Redo.id().into(), Some("F8".into()));
    assert!(!matches(&bindings, Action::Redo, &old));
    assert!(!matches(&bindings, Action::Redo, &alternative));
    assert!(matches(
        &bindings,
        Action::Redo,
        &event(Key::F8, egui::Modifiers::NONE)
    ));
    bindings.insert(Action::Redo.id().into(), None);
    assert!(label(&bindings, Action::Redo).is_empty());
    bindings.remove(Action::Redo.id());
    assert!(matches(&bindings, Action::Redo, &old));
}

#[test]
fn exact_modifiers_conflicts_and_all_zoom_aliases_are_checked() {
    let mut bindings = Bindings::new();
    assert!(!matches(
        &bindings,
        Action::Save,
        &event(Key::S, egui::Modifiers::CTRL.plus(egui::Modifiers::SHIFT))
    ));
    bindings.insert(Action::Save.id().into(), Some("Ctrl+Shift+S".into()));
    let conflict = validate(&bindings).unwrap_err();
    assert!(conflict.contains("Save current file") && conflict.contains("Save all files"));
    bindings.insert(Action::SaveAll.id().into(), None);
    assert!(validate(&bindings).is_ok());
    bindings.insert(Action::Save.id().into(), Some("Ctrl+Shift+Equals".into()));
    assert!(validate(&bindings).unwrap_err().contains("Zoom in"));
    bindings.insert(Action::ZoomIn.id().into(), None);
    assert!(validate(&bindings).is_ok());
}

#[test]
fn native_editing_modifier_only_and_typing_keys_are_rejected() {
    for value in [
        "A",
        "Shift+A",
        "Escape",
        "Ctrl+C",
        "Ctrl+Shift+V",
        "Ctrl+Alt+X",
        "Ctrl+ControlLeft",
        "Alt+F4",
        "Ctrl+ArrowLeft",
    ] {
        let bindings = Bindings::from([(Action::Save.id().into(), Some(value.into()))]);
        assert!(validate(&bindings).is_err(), "{value}");
    }
    assert!(Stroke::event(&event(Key::ControlLeft, egui::Modifiers::CTRL)).is_none());
    assert!(
        Stroke::event(&event(
            Key::K,
            egui::Modifiers {
                mac_cmd: true,
                ..Default::default()
            }
        ))
        .is_none()
    );
}

#[test]
fn recording_ignores_modifier_press_cancels_without_escape_leak_and_does_not_zoom() {
    let mut app = VEditApp::empty(egui::Context::default());
    app.settings_open = true;
    app.keybinding_ui.capture = Some(Action::Save);
    app.ctx.input_mut(|input| {
        input
            .events
            .push(event(Key::ControlLeft, egui::Modifiers::CTRL))
    });
    app.capture_shortcut_input();
    assert_eq!(app.keybinding_ui.capture, Some(Action::Save));
    app.ctx
        .input_mut(|input| input.events.push(event(Key::Equals, egui::Modifiers::CTRL)));
    app.capture_shortcut_input();
    app.preference_shortcuts();
    assert_eq!(app.settings.ui_scale, 1.0);
    assert!(app.settings.keybindings.is_empty());
    assert!(app.keybinding_ui.feedback.as_ref().unwrap().1);
    app.keybinding_ui.capture = Some(Action::Save);
    app.ctx
        .input_mut(|input| input.events.push(event(Key::Escape, egui::Modifiers::NONE)));
    app.capture_shortcut_input();
    assert!(app.keybinding_ui.capture.is_none());
    assert!(app.ctx.input(|input| input.events.is_empty()));
    assert!(app.settings_open);
}

#[test]
fn custom_binding_executes_old_binding_stops_and_modals_own_their_input() {
    let mut app = VEditApp::empty(egui::Context::default());
    app.settings
        .keybindings
        .insert(Action::ToggleWrap.id().into(), Some("F8".into()));
    app.ctx
        .input_mut(|input| input.events.push(event(Key::Z, egui::Modifiers::ALT)));
    app.shortcuts();
    assert!(!app.settings.word_wrap);
    app.ctx
        .input_mut(|input| input.events = vec![event(Key::F8, egui::Modifiers::NONE)]);
    app.shortcuts();
    assert!(app.settings.word_wrap);
    app.settings_open = true;
    app.ctx
        .input_mut(|input| input.events = vec![event(Key::F8, egui::Modifiers::NONE)]);
    app.shortcuts();
    assert!(app.settings.word_wrap);
    assert_eq!(app.ctx.input(|input| input.events.len()), 1);
}

#[test]
fn shortcut_patch_is_atomic_and_session_round_trip_preserves_disabled_actions() {
    let mut app = VEditApp::empty(egui::Context::default());
    let response = app.control_request(crate::cli::Request::Settings {
        patch: Some(crate::cli::SettingsPatch {
            font_size: Some(22.0),
            keybindings: Some(Bindings::from([(
                Action::Save.id().into(),
                Some("Ctrl+P".into()),
            )])),
            ..Default::default()
        }),
    });
    assert!(!response.ok);
    assert_eq!(app.settings.font_size, 14.0);
    assert!(app.settings.keybindings.is_empty());
    app.settings.keybindings = Bindings::from([
        (Action::Save.id().into(), Some("F8".into())),
        (Action::ToggleLeftSidebar.id().into(), None),
    ]);
    let encoded = serde_json::to_string(&app.session()).unwrap();
    let restored: Session = serde_json::from_str(&encoded).unwrap();
    assert_eq!(
        restored.settings.unwrap().keybindings,
        app.settings.keybindings
    );
}

#[test]
fn alt_shortcut_consumes_only_its_immediately_generated_character() {
    let app = VEditApp::empty(egui::Context::default());
    app.ctx.input_mut(|input| {
        input.events = vec![
            event(Key::Z, egui::Modifiers::ALT),
            egui::Event::Text("z".into()),
            egui::Event::Text("next".into()),
        ]
    });
    assert!(app.take_shortcut(Action::ToggleWrap));
    assert_eq!(
        app.ctx.input(|input| input.events.clone()),
        vec![egui::Event::Text("next".into())]
    );
}

#[test]
fn native_clipboard_event_gives_recording_feedback_without_touching_a_document() {
    let mut app = VEditApp::empty(egui::Context::default());
    app.keybinding_ui.capture = Some(Action::Save);
    app.ctx.input_mut(|input| {
        input
            .events
            .push(egui::Event::Paste("private clipboard".into()))
    });
    app.capture_shortcut_input();
    assert!(app.keybinding_ui.capture.is_none());
    assert!(
        app.keybinding_ui
            .feedback
            .as_ref()
            .unwrap()
            .0
            .contains("clipboard")
    );
    assert!(app.ctx.input(|input| input.events.is_empty()));
    assert!(app.settings.keybindings.is_empty());
}

#[test]
fn settings_search_record_disable_and_reset_are_operable_through_widgets() {
    use egui_kittest::{Harness, kittest::Queryable};
    let mut app = VEditApp::empty(egui::Context::default());
    app.keybinding_ui.query = "Save current file".into();
    let mut screen = Harness::builder()
        .with_size([650.0, 600.0])
        .with_os(egui::os::OperatingSystem::Windows)
        .build_ui_state(
            |ui, app: &mut VEditApp| {
                app.ctx = ui.ctx().clone();
                app.capture_shortcut_input();
                app.keyboard_settings_ui(ui);
            },
            app,
        );
    screen.run_steps(2);
    screen.get_by_label("Ctrl+S").click();
    screen.run_steps(1);
    assert_eq!(screen.state().keybinding_ui.capture, Some(Action::Save));
    screen
        .input_mut()
        .events
        .push(event(Key::F8, egui::Modifiers::NONE));
    screen.run_steps(2);
    assert_eq!(screen.state().shortcut_label(Action::Save), "F8");
    screen.get_by_label("Disable").click();
    screen.run_steps(2);
    assert!(screen.state().shortcut_label(Action::Save).is_empty());
    screen.get_by_label("Reset").click();
    screen.run_steps(2);
    assert_eq!(screen.state().shortcut_label(Action::Save), "Ctrl+S");
    assert!(screen.state().settings.keybindings.is_empty());
}

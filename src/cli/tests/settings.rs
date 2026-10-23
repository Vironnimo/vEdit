//! Settings parsing and JSON validation share the same bounded contract.
use super::*;

#[test]
fn leader_key_cli_normalizes_only_exact_aliases_and_preserves_unicode() {
    for (input, expected) in [
        ("space", " "),
        ("comma", ","),
        ("backslash", "\\"),
        (" ", " "),
        ("ß", "ß"),
        ("🎯", "🎯"),
        ("x", "x"),
    ] {
        let Parsed::Command {
            request: Request::Settings { patch: Some(patch) },
            start,
        } = parse(&["settings", "leader-key", input]).unwrap()
        else {
            panic!()
        };
        assert_eq!(patch.leader_key.as_deref(), Some(expected));
        assert!(!start);
        assert!(patch.vim.is_none() && patch.autosave.is_none() && patch.smooth_scroll.is_none());
    }
    assert!(
        matches!(parse(&["settings", "leader-key", "--", "-"]).unwrap(), Parsed::Command { request: Request::Settings { patch: Some(patch) }, .. } if patch.leader_key.as_deref() == Some("-"))
    );
    for value in ["", "SPACE", "comma ", "Space", "ab", "\n", "\t", "\0"] {
        assert!(
            parse(&["settings", "leader-key", value]).is_err(),
            "{value:?}"
        );
    }
}

#[test]
fn leader_key_json_validation_matches_cli_without_changing_payload_fields() {
    for value in [" ", "space", "comma", "backslash", "ö"] {
        let request: Request =
            serde_json::from_value(json!({"op":"settings","patch":{"leader_key":value}})).unwrap();
        validate(&request).unwrap();
    }
    for value in ["", "spacebar", "a b", "\u{7f}", "\r"] {
        let request: Request =
            serde_json::from_value(json!({"op":"settings","patch":{"leader_key":value}})).unwrap();
        assert!(validate(&request).is_err());
    }
    assert!(
        serde_json::from_value::<Request>(
            json!({"op":"settings","patch":{"leader_key":" ","leader_timeout":500}})
        )
        .is_err()
    );
}
#[test]
fn search_exclusion_options_preserve_inheritance_and_explicit_empty_override() {
    let search = |args: &[&str]| {
        let Parsed::Command {
            request: Request::Search { excludes, .. },
            ..
        } = parse(args).unwrap()
        else {
            panic!("expected Search")
        };
        excludes
    };
    assert_eq!(search(&["search", "needle"]), None);
    for pattern in ["--help", "-h", "--no-start", "--"] {
        assert_eq!(
            search(&["search", "needle", "--exclude", pattern]),
            Some(vec![pattern.into()])
        );
    }
    assert_eq!(
        search(&["search", "needle", "--no-excludes"]),
        Some(Vec::new())
    );
    let rules = search(&[
        "search",
        "needle",
        "--exclude",
        "src\\generated",
        "--exclude",
        "**/*.min.js",
    ])
    .unwrap();
    assert_eq!(
        normalize_search_excludes(&rules).unwrap(),
        ["src/generated", "**/*.min.js"]
    );
    for args in [
        vec![
            "search",
            "needle",
            "--exclude",
            "generated",
            "--no-excludes",
        ],
        vec![
            "search",
            "needle",
            "--no-excludes",
            "--exclude",
            "generated",
        ],
        vec!["search", "needle", "--no-excludes", "--no-excludes"],
        vec!["search", "needle", "--exclude"],
        vec!["read", "1", "--exclude", "generated"],
        vec!["settings", "--no-excludes"],
    ] {
        assert!(parse(&args).is_err(), "{args:?}");
    }
    for json in [
        json!({"op":"search","query":"needle"}),
        json!({"op":"search","query":"needle","excludes":null}),
    ] {
        let request: Request = serde_json::from_value(json).unwrap();
        validate(&request).unwrap();
        assert!(matches!(request, Request::Search { excludes: None, .. }));
    }
}
#[test]
fn exclusion_rules_validate_globs_and_actual_payload_bounds_before_effects() {
    let normalized = normalize_search_excludes(&[
        "  src\\generated  ".into(),
        "".into(),
        " ".into(),
        "src/generated".into(),
        "#cache".into(),
        "**/*.min.js".into(),
    ])
    .unwrap();
    assert_eq!(normalized, ["src/generated", "#cache", "**/*.min.js"]);
    for rules in [
        vec!["!keep.rs".into()],
        vec!["[abc".into()],
        vec!["folder\nfile".into()],
        vec!["folder\tfile".into()],
        vec!["x".repeat(1025)],
        vec!["x".into(); 129],
        vec!["x".repeat(1024); 17],
    ] {
        assert!(normalize_search_excludes(&rules).is_err(), "{rules:?}");
        for json in [
            json!({"op":"search","query":"needle","excludes":rules}),
            json!({"op":"settings","patch":{"swap_jk":true,"search_excludes":rules}}),
        ] {
            let request: Request = serde_json::from_value(json).unwrap();
            assert!(validate(&request).is_err());
        }
    }
    assert!(normalize_search_excludes(&vec!["x".repeat(1024); 16]).is_ok());
    for json in [
        json!({"op":"settings","patch":{"swap_jk":"true"}}),
        json!({"op":"search","query":"needle","excludes":[42]}),
        json!({"op":"search","query":"needle","exclude":["target"]}),
    ] {
        assert!(serde_json::from_value::<Request>(json).is_err());
    }
}
#[test]
fn keybinding_settings_preserve_strings_disable_and_aliases_with_bounded_payloads() {
    for (bindings, expected) in [
        (json!({}), json!({})),
        (
            json!({"save":"F8","toggle_left_sidebar":null}),
            json!({"save":"F8","toggle_left_sidebar":null}),
        ),
        (json!({"save":["F8"]}), json!({"save":"F8"})),
        (
            json!({"save":["Ctrl+S","F8"]}),
            json!({"save":["Ctrl+S","F8"]}),
        ),
        (
            json!({"save":["F1","F2","F3","F4","F5","F6","F7","F8"]}),
            json!({"save":["F1","F2","F3","F4","F5","F6","F7","F8"]}),
        ),
    ] {
        let request: Request =
            serde_json::from_value(json!({"op":"settings","patch":{"keybindings":bindings}}))
                .unwrap();
        validate(&request).unwrap();
        assert_eq!(
            serde_json::to_value(&request).unwrap()["patch"]["keybindings"],
            expected
        );
    }
    for bindings in [
        json!({"save":[]}),
        json!({"save":vec!["F8"; MAX_SHORTCUT_ALIASES + 1]}),
        json!({"save":""}),
        json!({"save":["F8", ""]}),
        json!({"save":"x".repeat(MAX_SHORTCUT_BYTES + 1)}),
        json!({"save":["F8", "x".repeat(MAX_SHORTCUT_BYTES + 1)]}),
    ] {
        let request: Request = serde_json::from_value(
            json!({"op":"settings","patch":{"vim":false,"keybindings":bindings}}),
        )
        .unwrap();
        assert!(validate(&request).is_err(), "{bindings}");
    }
    for binding in [json!(42), json!(["F8", null]), json!({"keys":"F8"})] {
        assert!(
            serde_json::from_value::<Request>(
                json!({"op":"settings","patch":{"keybindings":{"save":binding}}})
            )
            .is_err()
        );
    }
    assert!(
        transport::from_unique_json::<Request>(
            br#"{"op":"settings","patch":{"keybindings":{"save":"F8","save":["F9"]}}}"#
        )
        .is_err()
    );
}

#[test]
fn vim_key_settings_accept_known_targets_and_per_key_reset() {
    for patch in [json!({}), json!({"vim_keys":null})] {
        let request: Request =
            serde_json::from_value(json!({"op":"settings","patch":patch})).unwrap();
        validate(&request).unwrap();
        assert!(matches!(
            request,
            Request::Settings {
                patch: Some(SettingsPatch { vim_keys: None, .. })
            }
        ));
    }
    for key in VIM_CONTROL_KEYS {
        for target in [json!("editor"), json!("vim"), Value::Null] {
            let keys = json!({(*key):target});
            let request: Request =
                serde_json::from_value(json!({"op":"settings","patch":{"vim_keys":keys}})).unwrap();
            validate(&request).unwrap();
            assert_eq!(
                serde_json::to_value(&request).unwrap()["patch"]["vim_keys"],
                keys
            );
        }
    }
    for key in ["Ctrl+Z", "Ctrl+Shift+C", "ctrl+c", "C", ""] {
        let request: Request = serde_json::from_value(
            json!({"op":"settings","patch":{"vim_keys":{key:"vim"},"word_wrap":true}}),
        )
        .unwrap();
        assert!(validate(&request).is_err(), "{key}");
    }
    for target in [json!("default"), json!("Editor"), json!(false), json!([])] {
        assert!(
            serde_json::from_value::<Request>(
                json!({"op":"settings","patch":{"vim_keys":{"Ctrl+C":target}}})
            )
            .is_err()
        );
    }
    assert!(
        transport::from_unique_json::<Request>(
            br#"{"op":"settings","patch":{"vim_keys":{"Ctrl+C":"vim","Ctrl+C":null}}}"#
        )
        .is_err()
    );
    assert_eq!(
        schema()["settings"]["vim_keys"]["keys"],
        json!(VIM_CONTROL_KEYS)
    );
}

#[test]
fn swap_jk_cli_is_explicit_boolean_and_leaves_unrelated_settings_omitted() {
    for (input, expected) in [("true", true), ("false", false)] {
        let Parsed::Command {
            request: Request::Settings { patch: Some(patch) },
            start,
        } = parse(&["settings", "swap-jk", input]).unwrap()
        else {
            panic!()
        };
        assert_eq!(patch.swap_jk, Some(expected));
        assert!(
            patch.vim.is_none() && patch.leader_key.is_none() && patch.search_excludes.is_none()
        );
        assert!(!start);
    }
    for input in ["", "on", "1", "TRUE", "false "] {
        assert!(parse(&["settings", "swap-jk", input]).is_err());
    }
    let request: Request =
        serde_json::from_value(json!({"op":"settings","patch":{"word_wrap":true}})).unwrap();
    assert!(matches!(
        request,
        Request::Settings {
            patch: Some(SettingsPatch {
                swap_jk: None,
                search_excludes: None,
                ..
            })
        }
    ));
}
#[test]
fn editor_font_cli_accepts_equivalent_spellings_and_changes_only_the_font() {
    for (input, expected, serialized) in [
        (
            "jetbrains-mono",
            EditorFont::JetBrainsMono,
            "jetbrains_mono",
        ),
        (
            "jetbrains_mono",
            EditorFont::JetBrainsMono,
            "jetbrains_mono",
        ),
        ("consolas", EditorFont::Consolas, "consolas"),
        ("cascadia-mono", EditorFont::CascadiaMono, "cascadia_mono"),
        ("cascadia_mono", EditorFont::CascadiaMono, "cascadia_mono"),
    ] {
        let Parsed::Command {
            request: Request::Settings { patch: Some(patch) },
            start,
        } = parse(&["settings", "font", input]).unwrap()
        else {
            panic!("expected a font settings patch")
        };
        assert_eq!(patch.editor_font, Some(expected));
        let mut fields = serde_json::to_value(patch).unwrap();
        assert_eq!(
            fields.as_object_mut().unwrap().remove("editor_font"),
            Some(json!(serialized))
        );
        assert!(fields.as_object().unwrap().values().all(Value::is_null));
        assert!(!start, "font selection must not start another app");
    }
    for args in [
        vec!["settings", "font", "JetBrainsMono"],
        vec!["settings", "font", "jetbrains"],
        vec!["settings", "font", "consolas-bold"],
        vec!["settings", "font", "cascadia-code"],
        vec!["settings", "font", ""],
        vec!["settings", "font", "consolas", "cascadia-mono"],
    ] {
        assert!(parse(&args).is_err(), "{args:?}");
    }
}
#[test]
fn editor_font_json_preserves_null_and_omission_and_refuses_unknown_choices() {
    assert_eq!(EditorFont::default(), EditorFont::JetBrainsMono);
    for patch in [
        json!({}),
        json!({"editor_font":null}),
        json!({"word_wrap":true}),
    ] {
        let request: Request =
            serde_json::from_value(json!({"op":"settings","patch":patch})).unwrap();
        validate(&request).unwrap();
        assert!(matches!(
            request,
            Request::Settings {
                patch: Some(SettingsPatch {
                    editor_font: None,
                    ..
                })
            }
        ));
    }
    for (input, expected) in [
        ("jetbrains_mono", EditorFont::JetBrainsMono),
        ("jetbrains-mono", EditorFont::JetBrainsMono),
        ("consolas", EditorFont::Consolas),
        ("cascadia_mono", EditorFont::CascadiaMono),
        ("cascadia-mono", EditorFont::CascadiaMono),
    ] {
        let request: Request =
            serde_json::from_value(json!({"op":"settings","patch":{"editor_font":input}})).unwrap();
        validate(&request).unwrap();
        assert!(
            matches!(request, Request::Settings { patch: Some(SettingsPatch { editor_font: Some(font), .. }) } if font == expected)
        );
    }
    for patch in [
        json!({"editor_font":"consola","autosave":false}),
        json!({"editor_font":"Consolas"}),
        json!({"editor_font":42}),
        json!({"editor_font":"consolas","font_weight":"bold"}),
    ] {
        assert!(serde_json::from_value::<Request>(json!({"op":"settings","patch":patch})).is_err());
    }
    assert!(transport::from_unique_json::<Request>(br#"{"op":"settings","patch":{"editor_font":"consolas","editor_font":"cascadia_mono"}}"#).is_err());
    let schema = schema();
    assert_eq!(
        schema["settings"]["editor_font"]["enum"],
        json!(["jetbrains_mono", "consolas", "cascadia_mono"])
    );
    assert_eq!(
        schema["settings"]["editor_font"]["default"],
        "jetbrains_mono"
    );
}

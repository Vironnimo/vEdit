//! CLI protocol and parsing contract tests.
#[path = "tests/marks.rs"]
mod marks;
#[path = "tests/settings.rs"]
mod settings;

#[test]
fn old_instance_variable_cannot_silently_target_the_default_editor() {
    assert!(
        super::validate_control_environment(false, true)
            .unwrap_err()
            .contains("VEDIT_CONTROL_DIR")
    );
    assert!(super::validate_control_environment(true, true).is_ok());
    assert!(super::validate_control_environment(false, false).is_ok());
}
use super::*;
fn parse(args: &[&str]) -> Result<Parsed, String> {
    parse_args(args.iter().map(OsString::from))
}
fn mark_request(args: &[&str]) -> Request {
    let Parsed::Command { request, start } = parse(args).unwrap() else {
        panic!("expected an application request")
    };
    assert!(!start, "agent marks must use the existing window");
    request
}
#[test]
fn open_preserves_preview_and_exact_location() {
    let Parsed::Command {
        request: Request::Open { files, .. },
        start,
    } = parse(&[
        "open",
        "plan.md",
        "--preview",
        "--line",
        "7",
        "--column",
        "3",
    ])
    .unwrap()
    else {
        panic!()
    };
    assert!(start);
    assert_eq!(files[0].view, DocumentView::Preview);
    assert_eq!(files[0].location, Some(Location { line: 7, column: 3 }));
    assert!(files[0].path.is_absolute());
}
#[test]
fn rejects_contradictory_and_unsupported_options() {
    for args in [
        &["focus", "1", "--preview", "--source"][..],
        &["status", "--line", "2"],
        &["open", "a.rs", "--preview"],
        &["open", "a.rs", "--column", "2"],
        &["tabs", "--limit", "0"],
        &["close", "1", "1"],
        &["--no-start"],
    ] {
        assert!(parse(args).is_err(), "{args:?}");
    }
}
#[test]
fn unknown_constraints_are_not_dropped() {
    assert!(
        serde_json::from_value::<Request>(
            json!({"op":"open","files":[{"path":"C:/x.md","split":true}]})
        )
        .is_err()
    );
    assert!(serde_json::from_value::<Request>(json!({"op":"status","force":true})).is_err());
}
#[test]
fn every_advertised_schema_example_is_a_complete_valid_typed_request() {
    for example in schema()["commands"].as_array().unwrap() {
        let request: Request = serde_json::from_value(example.clone())
            .unwrap_or_else(|error| panic!("{example}: {error}"));
        validate(&request).unwrap_or_else(|error| panic!("{example}: {error}"));
    }
}
#[test]
fn local_tab_close_is_typed_distinct_from_global_close_and_validates_ids() {
    let request: Request = serde_json::from_value(json!({
        "op": "layout",
        "change": { "action": "close_tab", "pane_id": 2, "tab_id": 42 }
    }))
    .unwrap();
    validate(&request).unwrap();
    assert!(matches!(
        request,
        Request::Layout {
            change: LayoutAction::CloseTab {
                pane_id: 2,
                tab_id: 42
            }
        }
    ));
    for (pane_id, tab_id) in [(0, 42), (2, 0)] {
        let request: Request = serde_json::from_value(json!({
            "op": "layout",
            "change": { "action": "close_tab", "pane_id": pane_id, "tab_id": tab_id }
        }))
        .unwrap();
        assert!(validate(&request).is_err());
    }
    for change in [
        json!({"action":"close_tab","pane_id":2}),
        json!({"action":"close_tab","tab_id":42}),
        json!({"action":"close_tab","pane_id":2,"tab_id":42,"force":true}),
    ] {
        assert!(serde_json::from_value::<LayoutAction>(change).is_err());
    }
    let Parsed::Command {
        request: Request::Close { tab_ids },
        ..
    } = parse(&["close", "42"]).unwrap()
    else {
        panic!()
    };
    assert_eq!(tab_ids, [42]);
}
#[test]
fn layout_change_file_parser_preserves_local_tab_close() {
    let path = std::env::temp_dir().join(format!(
        "vedit-layout-close-tab-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &path,
        b"\xef\xbb\xbf{\"action\":\"close_tab\",\"pane_id\":2,\"tab_id\":42}",
    )
    .unwrap();
    let result = parse_args([
        OsString::from("--no-start"),
        OsString::from("layout"),
        path.as_os_str().to_owned(),
    ]);
    std::fs::remove_file(&path).unwrap();
    let Parsed::Command {
        request:
            Request::Layout {
                change: LayoutAction::CloseTab { pane_id, tab_id },
            },
        start,
    } = result.unwrap()
    else {
        panic!()
    };
    assert_eq!((pane_id, tab_id), (2, 42));
    assert!(!start);
}
#[test]
fn tab_move_preserves_explicit_panes_position_and_requires_every_constraint() {
    for (source_pane_id, pane_id, index) in [(1, 2, 0), (2, 2, 3), (3, 1, 100_000)] {
        let request: Request = serde_json::from_value(json!({
            "op": "layout",
            "change": { "action": "move_tab", "source_pane_id": source_pane_id,
                "pane_id": pane_id, "tab_id": 42, "index": index }
        }))
        .unwrap();
        validate(&request).unwrap();
        let Request::Layout {
            change:
                LayoutAction::MoveTab {
                    source_pane_id: actual_source,
                    pane_id: actual_target,
                    tab_id,
                    index: actual_index,
                },
        } = request
        else {
            panic!()
        };
        assert_eq!(
            (actual_source, actual_target, tab_id, actual_index),
            (source_pane_id, pane_id, 42, index)
        );
    }
    for (source_pane_id, pane_id, tab_id, index) in [
        (0, 2, 42, 0),
        (1, 0, 42, 0),
        (1, 2, 0, 0),
        (1, 2, 42, 100_001),
    ] {
        let request: Request = serde_json::from_value(json!({
            "op": "layout",
            "change": { "action": "move_tab", "source_pane_id": source_pane_id,
                "pane_id": pane_id, "tab_id": tab_id, "index": index }
        }))
        .unwrap();
        assert!(validate(&request).is_err());
    }
    let valid = json!({"action":"move_tab","source_pane_id":1,
        "pane_id":2,"tab_id":42,"index":0});
    for missing in ["source_pane_id", "pane_id", "tab_id", "index"] {
        let mut change = valid.clone();
        change.as_object_mut().unwrap().remove(missing);
        assert!(serde_json::from_value::<LayoutAction>(change).is_err());
    }
    for invalid in [
        json!({"action":"move_tab","source_pane_id":1,"target_pane_id":2,"tab_id":42,"index":0}),
        json!({"action":"move_tab","source_pane_id":1,"pane_id":2,"tab_id":42,"index":-1}),
        json!({"action":"move_tab","source_pane_id":1,"pane_id":2,"tab_id":42,"index":1.5}),
        json!({"action":"move_tab","source_pane_id":1,"pane_id":2,"tab_id":42,"index":0,"view":"preview"}),
        json!({"action":"move_tab","source_pane_id":1,"pane_id":2,"tab_id":42,"index":0,"copy":true}),
    ] {
        assert!(serde_json::from_value::<LayoutAction>(invalid).is_err());
    }
}
#[test]
fn layout_change_file_parser_preserves_zero_based_tab_move() {
    let path = std::env::temp_dir().join(format!(
        "vedit-layout-move-tab-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(&path,
        b"\xef\xbb\xbf{\"action\":\"move_tab\",\"source_pane_id\":3,\"pane_id\":2,\"tab_id\":42,\"index\":0}")
        .unwrap();
    let result = parse_args([
        OsString::from("--no-start"),
        OsString::from("layout"),
        path.as_os_str().to_owned(),
    ]);
    std::fs::remove_file(&path).unwrap();
    let Parsed::Command {
        request:
            Request::Layout {
                change:
                    LayoutAction::MoveTab {
                        source_pane_id,
                        pane_id,
                        tab_id,
                        index,
                    },
            },
        start,
    } = result.unwrap()
    else {
        panic!()
    };
    assert_eq!((source_pane_id, pane_id, tab_id, index), (3, 2, 42, 0));
    assert!(!start);
}
#[test]
fn edits_require_revision_preserve_payload_and_reject_conflicting_shapes() {
    assert!(
        serde_json::from_value::<Request>(json!({"op":"edit","tab_id":1,"set_text":"erase"}))
            .is_err()
    );
    let payload = "literal {\"path\":\"C:/a\",\"op\":\"delete\"} 😀\n";
    let request: Request = serde_json::from_value(
        json!({"op":"edit","tab_id":1,"expected_revision":0,"set_text":payload}),
    )
    .unwrap();
    validate(&request).unwrap();
    let Request::Edit { set_text, .. } = request else {
        panic!()
    };
    assert_eq!(set_text.unwrap(), payload);
    let request:Request=serde_json::from_value(json!({"op":"edit","tab_id":1,"expected_revision":0,"set_text":"","edits":[{"range":{"start":{"line":1},"end":{"line":1}},"text":"x"}]})).unwrap();
    assert!(validate(&request).is_err());
    for value in [
        json!({"op":"edit","tab_id":1,"expected_revision":0,"set_text":"x","force":true}),
        json!({"op":"settings","patch":{"word_wrap":true,"word_wrap_width":80}}),
        json!({"op":"layout","change":{"action":"split","orientation":"vertical","tab_id":1,"clone_text":true}}),
    ] {
        assert!(serde_json::from_value::<Request>(value).is_err());
    }
}
#[test]
fn broad_cli_wrappers_require_revision_and_preserve_explicit_options() {
    assert!(parse(&["replace", "1", "64", "96"]).is_err());
    let Parsed::Command {
        request:
            Request::Replace {
                expected_revision,
                case_sensitive,
                whole_word,
                all,
                replacement,
                ..
            },
        start,
    } = parse(&[
        "replace",
        "1",
        "64",
        "",
        "--revision",
        "3",
        "--case-sensitive",
        "--whole-word",
        "--first",
    ])
    .unwrap()
    else {
        panic!()
    };
    assert_eq!(expected_revision, 3);
    assert!(case_sensitive);
    assert!(whole_word);
    assert!(!all);
    assert!(replacement.is_empty());
    assert!(!start);
    for args in [
        &["command", "1", "undo", "--revision", "3", "--revision", "4"][..],
        &["find", "1", "q", "--revision", "3"],
        &["settings", "--whole-word"],
        &["search", "q", "--offset", "1"],
    ] {
        assert!(parse(args).is_err(), "{args:?}");
    }
    let Parsed::Command {
        request: Request::Layout {
            change: LayoutAction::Status {},
        },
        ..
    } = parse(&["layout"]).unwrap()
    else {
        panic!()
    };
}
#[test]
fn delete_file_command_resolves_unicode_target_and_does_not_autostart() {
    let manifest = std::env::temp_dir().join(format!(
        "vedit-delete-action-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &manifest,
        serde_json::to_vec(&json!({"action":"delete", "path":"obsolete/Pläne 東京.txt"})).unwrap(),
    )
    .unwrap();
    let parsed = parse_args([OsString::from("file"), manifest.as_os_str().to_owned()]);
    std::fs::remove_file(&manifest).unwrap();
    let Parsed::Command {
        request: Request::File {
            action: FileAction::Delete { path },
        },
        start,
    } = parsed.unwrap()
    else {
        panic!()
    };
    assert!(!start);
    assert_eq!(path, absolute("obsolete/Pläne 東京.txt".into()));
}

#[test]
fn delete_action_requires_an_explicit_path_and_rejects_extra_payload() {
    let request: Request = serde_json::from_value(
        json!({"op":"file", "action":{"action":"delete", "path":absolute("obsolete.txt".into())}}),
    )
    .unwrap();
    validate(&request).unwrap();
    assert!(
        serde_json::from_value::<Request>(json!({"op":"file", "action":{"action":"delete"}}))
            .is_err()
    );
    assert!(serde_json::from_value::<Request>(json!({"op":"file", "action":{"action":"delete", "path":"obsolete.txt", "name":"different.txt"}})).is_err());
}

#[test]
fn selections_and_lifecycle_constraints_are_never_silently_ignored() {
    let request:Request=serde_json::from_value(json!({"op":"selection","tab_id":1,"range":{"start":{"line":1},"end":{"line":1,"column":2}}})).unwrap();
    assert!(validate(&request).is_err());
    let request = Request::File {
        action: FileAction::CreateFolder {
            parent: absolute(".".into()),
            name: "../escape".into(),
        },
    };
    assert!(validate(&request).is_err());
    for action in [
        Request::Search {
            query: "x".into(),
            workspace: None,
            excludes: None,
            case_sensitive: false,
            whole_word: false,
            limit: 100,
        },
        Request::File {
            action: FileAction::Rename {
                path: absolute("old.md".into()),
                name: "new.md".into(),
            },
        },
    ] {
        let request = Request::Batch {
            actions: vec![Request::Raise {}, action],
            stop_on_error: false,
        };
        assert!(validate(&request).is_err());
    }
}
#[test]
fn annotation_end_cannot_reverse_range() {
    let request = Request::Open {
        background: false,
        files: vec![OpenFile {
            path: absolute("x.rs".into()),
            location: None,
            view: DocumentView::Source,
            annotations: vec![Annotation {
                id: String::new(),
                start: Location { line: 4, column: 3 },
                end: Some(Location { line: 4, column: 2 }),
                note: "look".into(),
                severity: AnnotationSeverity::Info,
            }],
        }],
    };
    assert!(validate(&request).is_err());
}
#[test]
fn no_start_preserves_command_effect() {
    let Parsed::Command {
        request: Request::Open { files, .. },
        start,
    } = parse(&["--no-start", "open", "x.md", "--source"]).unwrap()
    else {
        panic!()
    };
    assert!(!start);
    assert_eq!(files[0].view, DocumentView::Source);
}
#[test]
fn source_default_and_one_based_column() {
    let item: OpenFile =
        serde_json::from_value(json!({"path":"C:/x.md","location":{"line":9}})).unwrap();
    assert_eq!(item.view, DocumentView::Source);
    assert_eq!(item.location.unwrap().column, 1);
}
#[test]
fn json_bom_and_literal_flag_filenames_are_preserved() {
    let request: Request =
        serde_json::from_slice(json_bytes(b"\xef\xbb\xbf{\"op\":\"status\"}")).unwrap();
    assert!(matches!(request, Request::Status {}));
    let Parsed::Command {
        request: Request::Open { files, .. },
        ..
    } = parse(&["open", "--", "--no-start", "--help"]).unwrap()
    else {
        panic!()
    };
    assert!(files[0].path.ends_with("--no-start"));
    assert!(files[1].path.ends_with("--help"));
}
#[test]
fn git_commands_parse_exact_refs_and_bounded_pages() {
    for command in ["git-branches", "git-history"] {
        let Parsed::Command { request, .. } =
            parse(&[command, "--limit", "1", "--offset", "2"]).unwrap()
        else {
            panic!()
        };
        let value = serde_json::to_value(request).unwrap();
        assert_eq!(value["limit"], 1);
        assert_eq!(value["offset"], 2);
    }
    let Parsed::Command { request, .. } =
        parse(&["git-switch-branch", "refs/heads/feature/Überarbeitung"]).unwrap()
    else {
        panic!()
    };
    assert_eq!(
        serde_json::to_value(request).unwrap()["target"],
        "refs/heads/feature/Überarbeitung"
    );
    for args in [
        vec!["git-switch-branch"],
        vec!["git-switch-branch", "main"],
        vec!["git-branches", "--limit", "0"],
    ] {
        assert!(parse(&args).is_err());
    }
}

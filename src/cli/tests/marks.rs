//! Agent marks and bookmarks preserve explicit intent and literal payloads.
use super::*;

#[test]
fn agent_marks_cli_preserves_collection_targets_and_action_defaults() {
    let cases = [
        (
            vec!["agent-marks"],
            json!({"action":"list","limit":100,"offset":0}),
        ),
        (
            vec![
                "agent-marks",
                "list",
                "--limit",
                "500",
                "--offset",
                "100001",
            ],
            json!({"action":"list","limit":500,"offset":100001}),
        ),
        (
            vec!["agent-marks", "take"],
            json!({"action":"take","request_id":null,"limit":100}),
        ),
        (
            vec![
                "agent-marks",
                "take",
                "--request-id",
                "collection-42",
                "--limit",
                "3",
            ],
            json!({"action":"take","request_id":"collection-42","limit":3}),
        ),
        (
            vec!["agent-marks", "batch"],
            json!({"action":"batch","batch_id":null,"limit":100,"offset":0}),
        ),
        (
            vec!["agent-marks", "batch", "42", "--offset", "101"],
            json!({"action":"batch","batch_id":42,"limit":100,"offset":101}),
        ),
        (
            vec!["agent-marks", "text", "9"],
            json!({"action":"text","id":9,"batch_id":null,"offset":0,"count":4096}),
        ),
        (
            vec![
                "agent-marks",
                "text",
                "9",
                "--batch",
                "42",
                "--offset",
                "4096",
                "--count",
                "65536",
            ],
            json!({"action":"text","id":9,"batch_id":42,"offset":4096,"count":65536}),
        ),
        (
            vec!["agent-marks", "ack", "42"],
            json!({"action":"ack","batch_id":42}),
        ),
        (
            vec!["agent-marks", "restore"],
            json!({"action":"restore","batch_id":null}),
        ),
        (
            vec!["agent-marks", "restore", "42"],
            json!({"action":"restore","batch_id":42}),
        ),
        (vec!["agent-marks", "clear"], json!({"action":"clear"})),
        (
            vec!["agent-marks", "remove", "9"],
            json!({"action":"remove","id":9}),
        ),
        (
            vec!["agent-marks", "note", "9", ""],
            json!({"action":"note","id":9,"note":""}),
        ),
        (
            vec!["agent-marks", "add"],
            json!({"action":"add","tab_id":null,"note":null}),
        ),
        (
            vec![
                "agent-marks",
                "add",
                "--tab",
                "7",
                "--note",
                " Prüfe Überlauf 🎯\nZeile zwei ",
            ],
            json!({"action":"add","tab_id":7,"note":" Prüfe Überlauf 🎯\nZeile zwei "}),
        ),
    ];
    for (args, action) in cases {
        let request = mark_request(&args);
        assert_eq!(
            serde_json::to_value(request).unwrap(),
            json!({"op":"agent_marks","action":action}),
            "{args:?}"
        );
    }
    assert!(matches!(
        mark_request(&["clear-marks"]),
        Request::ClearMarks { .. }
    ));
}

#[test]
fn agent_marks_parser_keeps_literal_notes_and_refuses_unrelated_constraints() {
    for payload in ["--help", "-h", "--no-start", "--", "{\"action\":\"clear\"}"] {
        let request = mark_request(&["agent-marks", "add", "--note", payload]);
        assert!(
            matches!(request, Request::AgentMarks { action: AgentMarksAction::Add { note: Some(note), .. } } if note == payload)
        );
        let request = mark_request(&["agent-marks", "note", "9", "--", payload]);
        assert!(
            matches!(request, Request::AgentMarks { action: AgentMarksAction::Note { id: 9, note } } if note == payload)
        );
    }
    let request = mark_request(&["agent-marks", "take", "--request-id", "--no-start"]);
    assert!(
        matches!(request, Request::AgentMarks { action: AgentMarksAction::Take { request_id: Some(token), .. } } if token == "--no-start")
    );
    for args in [
        &["agent-marks", "list", "--tab", "7"][..],
        &["agent-marks", "take", "--offset", "1"],
        &["agent-marks", "take", "--count", "1"],
        &["agent-marks", "take", "--batch", "42"],
        &["agent-marks", "text", "9", "--limit", "1"],
        &["agent-marks", "text", "9", "--request-id", "token"],
        &["agent-marks", "batch", "42", "--batch", "43"],
        &["agent-marks", "batch", "--note", "x"],
        &["agent-marks", "ack", "42", "--limit", "2"],
        &["agent-marks", "restore", "42", "--offset", "0"],
        &["agent-marks", "clear", "--tab", "7"],
        &["agent-marks", "remove", "9", "--note", "x"],
        &["agent-marks", "note", "9", "x", "--note", "y"],
        &["agent-marks", "add", "--offset", "0"],
        &["agent-marks", "add", "--line", "7"],
        &["agent-marks", "add", "--label", "x"],
        &[
            "agent-marks",
            "take",
            "--request-id",
            "a",
            "--request-id",
            "b",
        ],
        &["agent-marks", "text", "9", "--batch", "42", "--batch", "43"],
        &["agent-marks", "add", "--note", "a", "--note", "b"],
        &["agent-marks", "add", "--tab", "7", "--tab", "8"],
        &["agent-marks", "ack"],
        &["agent-marks", "remove", "0"],
        &["agent-marks", "batch", "-1"],
        &["agent-marks", "note", "9"],
        &["agent-marks", "add", "arbitrary-range"],
        &["status", "--request-id", "x"],
        &["bookmarks", "--batch", "42"],
    ] {
        assert!(parse(args).is_err(), "{args:?}");
    }
}

#[test]
fn agent_marks_json_has_readonly_omission_and_strict_explicit_intent() {
    let request: Request = serde_json::from_value(json!({"op":"agent_marks"})).unwrap();
    validate(&request).unwrap();
    assert!(matches!(
        request,
        Request::AgentMarks {
            action: AgentMarksAction::List {
                limit: 100,
                offset: 0
            }
        }
    ));
    let action = |value: Value| {
        serde_json::from_value::<Request>(json!({"op":"agent_marks","action":value}))
    };
    for invalid in [
        Value::Null,
        json!("take"),
        json!({}),
        json!({"action":"Take"}),
        json!({"action":"take","offset":1}),
        json!({"action":"take","request_id":42}),
        json!({"action":"remove","id":9,"force":true}),
        json!({"action":"note","id":9,"note":null}),
        json!({"action":"add","range":{"start":{"line":1},"end":{"line":2}}}),
        json!({"action":"text","id":9,"tab_id":7}),
        json!({"action":"batch","batch_id":-1}),
        json!({"action":"clear","workspace":"C:/other"}),
    ] {
        assert!(action(invalid.clone()).is_err(), "{invalid}");
    }
    for valid in [
        json!({"action":"batch"}),
        json!({"action":"batch","batch_id":null}),
        json!({"action":"text","id":9,"batch_id":null}),
        json!({"action":"restore","batch_id":null}),
        json!({"action":"take","request_id":null}),
        json!({"action":"add","tab_id":null,"note":null}),
    ] {
        validate(&action(valid.clone()).unwrap()).unwrap();
    }
    for bytes in [
        br#"{"op":"agent_marks","action":{"action":"take","request_id":"a","request_id":"b"}}"#
            .as_slice(),
        br#"{"op":"agent_marks","action":{"action":"remove","id":9,"id":10}}"#,
        br#"{"op":"agent_marks","action":{"action":"list"},"action":{"action":"clear"}}"#,
    ] {
        assert!(transport::from_unique_json::<Request>(bytes).is_err());
    }
}

#[test]
fn agent_marks_bounds_use_unicode_notes_positive_ids_and_bounded_payloads() {
    let request = |action: Value| {
        serde_json::from_value::<Request>(json!({"op":"agent_marks","action":action})).unwrap()
    };
    for note in [
        "".to_owned(),
        "🎯".repeat(4096),
        " literal {\"op\":\"clear\"}\n\t ".to_owned(),
    ] {
        let input = request(json!({"action":"note","id":9,"note":note}));
        validate(&input).unwrap();
        assert!(
            matches!(input, Request::AgentMarks { action: AgentMarksAction::Note { note: actual, .. } } if actual == note)
        );
    }
    for invalid in [
        json!({"action":"list","limit":0}),
        json!({"action":"take","limit":501}),
        json!({"action":"batch","batch_id":0}),
        json!({"action":"text","id":0}),
        json!({"action":"text","id":9,"batch_id":0}),
        json!({"action":"text","id":9,"count":0}),
        json!({"action":"text","id":9,"count":65537}),
        json!({"action":"ack","batch_id":0}),
        json!({"action":"restore","batch_id":0}),
        json!({"action":"add","tab_id":0}),
        json!({"action":"add","note":"x".repeat(4097)}),
        json!({"action":"note","id":9,"note":"🎯".repeat(4097)}),
        json!({"action":"take","request_id":""}),
        json!({"action":"take","request_id":"é".repeat(65)}),
        json!({"action":"take","request_id":"line\nbreak"}),
    ] {
        assert!(validate(&request(invalid.clone())).is_err(), "{invalid}");
    }
    for valid in [
        json!({"action":"list","limit":500,"offset":usize::MAX}),
        json!({"action":"take","request_id":"é".repeat(64)}),
        json!({"action":"text","id":u64::MAX,"offset":usize::MAX,"count":65536}),
    ] {
        validate(&request(valid)).unwrap();
    }
}
#[test]
fn bookmark_commands_preserve_targets_labels_and_existing_instance_scope() {
    for args in [&["bookmarks"][..], &["bookmarks", "list"]] {
        assert!(matches!(
            parse(args).unwrap(),
            Parsed::Command {
                request: Request::Bookmarks {
                    change: BookmarkAction::List {
                        limit: 100,
                        offset: 0
                    }
                },
                start: false
            }
        ));
    }
    let Parsed::Command {
        request:
            Request::Bookmarks {
                change: BookmarkAction::Toggle { tab_id, label },
            },
        start,
    } = parse(&[
        "bookmarks",
        "toggle",
        "--tab",
        "42",
        "--label",
        " Prüfe Überlauf 🎯 ",
    ])
    .unwrap()
    else {
        panic!()
    };
    assert_eq!(tab_id, Some(42));
    assert_eq!(label.as_deref(), Some(" Prüfe Überlauf 🎯 "));
    assert!(!start);
    assert!(matches!(
        parse(&["bookmarks", "toggle"]).unwrap(),
        Parsed::Command {
            request: Request::Bookmarks {
                change: BookmarkAction::Toggle {
                    tab_id: None,
                    label: None
                }
            },
            ..
        }
    ));
    assert!(matches!(
        parse(&["bookmarks", "remove", "7"]).unwrap(),
        Parsed::Command {
            request: Request::Bookmarks {
                change: BookmarkAction::Remove { id: 7 }
            },
            ..
        }
    ));
    assert!(matches!(
        parse(&["bookmarks", "jump", "9"]).unwrap(),
        Parsed::Command {
            request: Request::Bookmarks {
                change: BookmarkAction::Jump { id: 9 }
            },
            ..
        }
    ));
    assert!(
        matches!(parse(&["bookmarks", "rename", "7", "--label", ""]).unwrap(), Parsed::Command { request: Request::Bookmarks { change: BookmarkAction::Rename { id: 7, label } }, .. } if label.is_empty())
    );
}

#[test]
fn bookmark_parser_refuses_unrelated_or_contradictory_constraints() {
    for args in [
        &["bookmarks", "--tab", "42"][..],
        &["bookmarks", "list", "--label", "x"],
        &["bookmarks", "remove", "7", "--label", "x"],
        &["bookmarks", "rename", "7"],
        &["bookmarks", "rename", "7", "--label", "x", "--tab", "42"],
        &["bookmarks", "toggle", "--tab", "0"],
        &["bookmarks", "toggle", "--tab", "1", "--tab", "2"],
        &["bookmarks", "toggle", "--label", "a", "--label", "b"],
        &["bookmarks", "toggle", "--line", "7"],
        &["bookmarks", "toggle", "--revision", "3"],
        &["bookmarks", "toggle", "--limit", "3"],
        &["bookmarks", "rename", "7", "--label", "x", "--offset", "2"],
        &["bookmarks", "toggle", "extra"],
        &["bookmarks", "jump", "0"],
        &["bookmarks", "remove", "18446744073709551616"],
        &["status", "--label", "x"],
        &["focus", "1", "--tab", "2"],
    ] {
        assert!(parse(args).is_err(), "{args:?}");
    }
}

#[test]
fn bookmark_pages_are_bounded_and_legacy_list_json_keeps_defaults() {
    for args in [
        &["bookmarks", "--limit", "500", "--offset", "13"][..],
        &["bookmarks", "list", "--limit", "500", "--offset", "13"],
    ] {
        assert!(matches!(
            parse(args).unwrap(),
            Parsed::Command {
                request: Request::Bookmarks {
                    change: BookmarkAction::List {
                        limit: 500,
                        offset: 13
                    }
                },
                ..
            }
        ));
    }
    let request: Request =
        serde_json::from_value(json!({"op":"bookmarks","change":{"action":"list"}})).unwrap();
    validate(&request).unwrap();
    assert!(matches!(
        request,
        Request::Bookmarks {
            change: BookmarkAction::List {
                limit: 100,
                offset: 0
            }
        }
    ));
    for limit in [0, 501] {
        let request: Request = serde_json::from_value(
            json!({"op":"bookmarks","change":{"action":"list","limit":limit}}),
        )
        .unwrap();
        assert!(validate(&request).is_err());
        assert!(parse(&["bookmarks", "--limit", &limit.to_string()]).is_err());
    }
}

#[test]
fn labels_that_look_like_global_options_remain_literal_payload() {
    for label in ["--help", "-h", "--no-start", "--"] {
        let Parsed::Command {
            request:
                Request::Bookmarks {
                    change:
                        BookmarkAction::Toggle {
                            label: Some(actual),
                            ..
                        },
                },
            ..
        } = parse(&["bookmarks", "toggle", "--label", label]).unwrap()
        else {
            panic!()
        };
        assert_eq!(actual, label);
    }
    assert!(matches!(
        parse(&["bookmarks", "toggle", "--label", "x", "--help"]).unwrap(),
        Parsed::Help { .. }
    ));
    assert!(
        matches!(parse(&["open", "--no-start", "--", "--no-start"]).unwrap(), Parsed::Command { request: Request::Open { files, .. }, start: false } if files[0].path.file_name().unwrap() == "--no-start")
    );
}

#[test]
fn bookmark_json_keeps_explicit_targets_and_refuses_unknown_features() {
    let request: Request = serde_json::from_value(
        json!({"op":"bookmarks","change":{"action":"toggle","tab_id":42,"label":"source/plan.md"}}),
    )
    .unwrap();
    validate(&request).unwrap();
    assert!(
        matches!(request, Request::Bookmarks { change: BookmarkAction::Toggle { tab_id: Some(42), label: Some(label) } } if label == "source/plan.md")
    );
    for change in [
        json!({"action":"toggle","tab_id":42,"line":7}),
        json!({"action":"list","workspace":"C:/project"}),
        json!({"action":"jump","id":9,"force":true}),
        json!({"action":"remove","tab_id":42}),
        json!({"action":"rename","id":9}),
        json!({"action":"jump","id":-1}),
    ] {
        assert!(
            serde_json::from_value::<Request>(json!({"op":"bookmarks","change":change})).is_err()
        );
    }
    for change in [
        json!({"action":"jump","id":0}),
        json!({"action":"toggle","tab_id":0}),
    ] {
        let request: Request =
            serde_json::from_value(json!({"op":"bookmarks","change":change})).unwrap();
        assert!(validate(&request).is_err());
    }
}

#[test]
fn bookmark_labels_have_unicode_byte_boundaries_and_clear_semantics() {
    let allowed = "é".repeat(128);
    let refused = "é".repeat(129);
    for label in ["", allowed.as_str(), " leading and trailing "] {
        let request = Request::Bookmarks {
            change: BookmarkAction::Rename {
                id: 1,
                label: label.to_owned(),
            },
        };
        validate(&request).unwrap();
    }
    for label in [refused.as_str(), "line\nbreak", "tab\tlabel", "null\0label"] {
        let request = Request::Bookmarks {
            change: BookmarkAction::Toggle {
                tab_id: Some(42),
                label: Some(label.to_owned()),
            },
        };
        assert!(validate(&request).is_err());
    }
}

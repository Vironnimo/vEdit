use super::*;

#[test]
fn launch_cli_validates_profiles_options_and_unicode_file_input() {
    let json = r#"{"profiles":[{"id":"web","name":"Web 東京","program":"powershell.exe","args":["--help","café & test"]},{"id":"desktop","name":"Desktop","program":"app.exe"}],"default":"desktop"}"#;
    let mut bytes = vec![0xff, 0xfe];
    for unit in json.encode_utf16() {
        bytes.extend(unit.to_le_bytes());
    }
    let file = InputFile::new(&bytes);
    let Parsed::Command {
        request:
            Request::Launch {
                workspace,
                action:
                    launch::Action::Configure {
                        configuration,
                        revision,
                    },
            },
        start,
    } = parse(&[
        "launch",
        "configure",
        "--file",
        file.path(),
        "--revision",
        "7",
        "--workspace",
        ".",
    ])
    .unwrap()
    else {
        panic!("launch configure");
    };
    assert!(!start);
    assert!(workspace.unwrap().is_absolute());
    assert_eq!(revision, 7);
    assert_eq!(configuration.profiles[0].name, "Web 東京");
    assert_eq!(configuration.profiles[0].args[0], "--help");
    for args in [
        vec!["launch", "start", "a", "b"],
        vec!["launch", "start", "--revision", "1"],
        vec!["launch", "configure"],
        vec!["launch", "list", "--count", "2"],
        vec!["status", "--workspace", "."],
        vec!["launch", "output", "--count", "0"],
        vec!["launch", "select", "bad/id"],
    ] {
        assert!(parse(&args).is_err(), "{args:?}");
    }
    for action in ["list", "start", "stop", "output"] {
        assert!(parse(&["launch", action]).is_ok());
    }
    let duplicate = json.replace("\"desktop\"", "\"web\"");
    assert!(launch::decode(duplicate.as_bytes()).is_err());
    assert!(launch::decode(br#"{"profiles":[],"default":"missing"}"#).is_err());
    assert!(launch::decode(br#"{"profiles":[],"unknown":true}"#).is_err());
    assert!(
        schema_topic(Some("launch")).unwrap()["commands"]
            .as_array()
            .unwrap()
            .len()
            >= 6
    );
}

fn parse(values: &[&str]) -> Result<Parsed, String> {
    parse_args(values.iter().map(OsString::from))
}

struct InputFile(PathBuf);
impl InputFile {
    fn new(bytes: &[u8]) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "vedit-cli-workflow-{}-{}.txt",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::write(&path, bytes).unwrap();
        Self(path)
    }
    fn path(&self) -> &str {
        self.0.to_str().unwrap()
    }
}
impl Drop for InputFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[test]
fn powershell_utf16_requests_preserve_unicode_and_reject_bad_encoding() {
    let input = r#"{"op":"new","text":"Zürich 東京 😀\r\n--help"}"#;
    for big_endian in [false, true] {
        let mut bytes = if big_endian {
            vec![0xfe, 0xff]
        } else {
            vec![0xff, 0xfe]
        };
        for unit in input.encode_utf16() {
            bytes.extend(if big_endian {
                unit.to_be_bytes()
            } else {
                unit.to_le_bytes()
            });
        }
        let file = InputFile::new(&bytes);
        let Parsed::Command {
            request: Request::New { text, .. },
            start,
        } = parse(&["request", file.path()]).unwrap()
        else {
            panic!("new request")
        };
        assert!(!start);
        assert_eq!(text, "Zürich 東京 😀\r\n--help");
    }
    for bytes in [&[0xff, 0xfe, 0][..], &[0xff, 0xfe, 0, 0xd8], &[0xc3, 0x28]] {
        let file = InputFile::new(bytes);
        assert!(parse(&["request", file.path()]).is_err());
    }
}

#[test]
fn direct_draft_and_edit_file_preserve_text_and_require_revision() {
    let file = InputFile::new("Zürich 東京 😀\r\n--help\n".as_bytes());
    let Parsed::Command {
        request: Request::New { title, text, path },
        start,
    } = parse(&["new", "Proposal", "--file", file.path()]).unwrap()
    else {
        panic!("new")
    };
    assert_eq!(title.as_deref(), Some("Proposal"));
    assert_eq!(text, "Zürich 東京 😀\r\n--help\n");
    assert!(path.is_none() && !start);
    let Parsed::Command {
        request:
            Request::Edit {
                tab_id,
                expected_revision,
                edits,
                set_text,
            },
        start,
    } = parse(&["edit", "42", "--file", file.path(), "--revision", "7"]).unwrap()
    else {
        panic!("edit")
    };
    assert_eq!((tab_id, expected_revision), (42, 7));
    assert!(edits.is_empty() && !start);
    assert_eq!(set_text.as_deref(), Some(text.as_str()));
    assert!(parse(&["edit", "42", "--file", file.path()]).is_err());
    assert!(parse(&["save", "42", "--file", file.path()]).is_err());
    let Parsed::Command {
        request:
            Request::SaveAs {
                tab_id,
                expected_revision,
                path,
            },
        start,
    } = parse(&["save-as", "42", "proposal.md", "--revision", "8"]).unwrap()
    else {
        panic!("save-as")
    };
    assert_eq!((tab_id, expected_revision), (42, 8));
    assert!(path.is_absolute() && !start);
}

#[test]
fn scoped_schema_discovery_and_equals_options_are_offline_and_literal() {
    assert!(parse(&["schema", "edit"]).is_ok());
    assert!(parse(&["schema", "not-an-operation"]).is_err());
    let Parsed::Command {
        request:
            Request::Bookmarks {
                change: BookmarkAction::Rename { id, label },
            },
        ..
    } = parse(&["bookmarks", "rename", "3", "--label=--help"]).unwrap()
    else {
        panic!("rename")
    };
    assert_eq!(id, 3);
    assert_eq!(label, "--help");
    assert!(parse(&["tabs", "--limit=1", "--limit", "2"]).is_err());
}

#[test]
fn command_help_and_skill_index_cover_the_parser_catalog() {
    let skill = include_str!("../.agents/skills/vedit/SKILL.md");
    for entry in help::COMMANDS {
        let Parsed::Help { command } = parse(&[entry.name, "--help"]).unwrap() else {
            panic!("help must be offline")
        };
        assert_eq!(command.as_deref(), Some(entry.name));
        let rendered = help::render(command.as_deref());
        assert!(rendered.contains(&format!("Usage: vedit-cli {}", entry.name)));
        assert!(rendered.len() < 6500, "{} help too large", entry.name);
        assert!(
            skill.contains(&format!("- `{}`:", entry.name)),
            "missing skill command {}",
            entry.name
        );
        for flag in entry.flags {
            assert!(rendered.contains(flag));
        }
        if let Err(error) = parse(&[entry.name]) {
            assert!(
                !error.starts_with("Unknown command"),
                "{} is not parsed",
                entry.name
            );
        }
    }
    assert!(parse(&["missing", "--help"]).is_err());
    assert!(parse(&["read", "1", "--column", "2"]).is_err());
    for op in ["read", "output"] {
        let mut value = json!({"op":op,"start_column":0});
        if op == "read" {
            value["tab_id"] = json!(1);
        }
        assert!(validate(&serde_json::from_value(value).unwrap()).is_err());
    }
}

#[test]
fn schema_topics_keep_examples_typed_and_default_read_wire_compatible() {
    for example in schema()["commands"].as_array().unwrap() {
        let topic = example["op"].as_str().unwrap();
        let selected = schema_topic(Some(topic)).unwrap();
        for request in selected["commands"].as_array().unwrap() {
            assert_eq!(request["op"], topic);
            validate(&serde_json::from_value(request.clone()).unwrap()).unwrap();
        }
    }
    for args in [&["read", "1"][..], &["output"][..]] {
        let Parsed::Command { request, .. } = parse(args).unwrap() else {
            panic!("command")
        };
        assert!(
            serde_json::to_value(request)
                .unwrap()
                .get("start_column")
                .is_none()
        );
    }
}

#[test]
fn notify_and_project_flags_parse_without_starting_the_editor() {
    let Parsed::Command {
        request:
            Request::Notify {
                message,
                title,
                level,
                workspace,
                open,
                review,
            },
        start,
    } = parse(&[
        "notify",
        "Tests pass 東京",
        "--title",
        "Ready",
        "--level",
        "SUCCESS",
        "--open",
        "C:/project/src/lib.rs:42",
    ])
    .unwrap()
    else {
        panic!("notify")
    };
    assert!(!start, "notify never starts vEdit");
    assert_eq!(message, "Tests pass 東京");
    assert_eq!(title.as_deref(), Some("Ready"));
    assert_eq!(level, NotificationLevel::Success);
    assert!(workspace.is_none() && review.is_none());
    let open = open.unwrap();
    assert!(open.path.ends_with("lib.rs"));
    assert_eq!(open.location.unwrap().line, 42);
    // A drive letter is part of the path, not a line number.
    let Parsed::Command {
        request: Request::Notify { open, .. },
        ..
    } = parse(&["notify", "x", "--open", "C:/project/notes.md"]).unwrap()
    else {
        panic!("notify")
    };
    assert!(open.unwrap().location.is_none());
    assert!(parse(&["notify"]).is_err());
    assert!(parse(&["notify", "x", "--level", "loud"]).is_err());
    assert!(parse(&["notify", "x", "--review", "working", "--open", "a.rs"]).is_err());
    assert!(parse(&["notify", "x", "--title", "two\nlines"]).is_err());
    // Background presentation never starts vEdit; ordinary open does.
    let Parsed::Command { start, request } = parse(&["open", "x.md", "--background"]).unwrap()
    else {
        panic!("open")
    };
    assert!(!start);
    assert!(matches!(
        request,
        Request::Open {
            background: true,
            ..
        }
    ));
    assert!(matches!(
        parse(&["open", "x.md"]).unwrap(),
        Parsed::Command { start: true, .. }
    ));
    // Project commands take --workspace, or the legacy positional, not both.
    assert!(matches!(
        parse(&["task", "test", "--workspace", "C:/project"]).unwrap(),
        Parsed::Command {
            request: Request::Task {
                workspace: Some(_),
                ..
            },
            ..
        }
    ));
    assert!(matches!(
        parse(&["output", "--workspace", "C:/project"]).unwrap(),
        Parsed::Command {
            request: Request::Output {
                workspace: Some(_),
                ..
            },
            ..
        }
    ));
    assert!(parse(&["git-status", "C:/a", "--workspace", "C:/b"]).is_err());
    assert!(parse(&["focus", "1", "--background"]).is_err());
}

use super::*;

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

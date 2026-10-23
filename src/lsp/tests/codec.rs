//! Wire bounds, positions, URI and response decoding contracts.
use super::*;

#[test]
fn utf16_positions_round_trip_unicode_and_clamp_surrogate_pairs() {
    let text = "a🦀ü\n日本語😀z\n";
    for offset in 0..=text.chars().count() {
        assert_eq!(
            position_to_char(text, char_to_position(text, offset)),
            offset
        );
    }
    assert_eq!(
        char_to_position(text, 2),
        Position {
            line: 0,
            character: 3
        }
    );
    assert_eq!(
        position_to_char(
            text,
            Position {
                line: 0,
                character: 2
            }
        ),
        1
    );
    assert_eq!(
        position_to_char(
            text,
            Position {
                line: 0,
                character: 1000
            }
        ),
        3
    );
    assert_eq!(
        position_to_char(
            text,
            Position {
                line: 100,
                character: 10
            }
        ),
        text.chars().count()
    );
    assert_eq!(
        char_to_position(text, 1000),
        Position {
            line: 2,
            character: 0
        }
    );
}

#[test]
fn crlf_positions_exclude_line_breaks_from_columns() {
    let text = "🦀x\r\ny\r\n";
    assert_eq!(
        char_to_position(text, 2),
        Position {
            line: 0,
            character: 3
        }
    );
    assert_eq!(
        char_to_position(text, 3),
        Position {
            line: 0,
            character: 3
        }
    );
    assert_eq!(
        char_to_position(text, 4),
        Position {
            line: 1,
            character: 0
        }
    );
    assert_eq!(
        position_to_char(
            text,
            Position {
                line: 0,
                character: 99
            }
        ),
        2
    );
    assert_eq!(
        position_to_char(
            text,
            Position {
                line: 1,
                character: 1
            }
        ),
        5
    );
}

#[test]
fn file_uris_round_trip_unicode_spaces_and_reserved_characters() {
    #[cfg(windows)]
    let path = Path::new(r"C:\Code\über #1%\🦀.rs");
    #[cfg(not(windows))]
    let path = Path::new("/code/über #1%/🦀.rs");
    let uri = path_to_uri(path).unwrap();
    assert!(uri.contains("%C3%BCber%20%231%25/"));
    assert!(uri.ends_with("%F0%9F%A6%80.rs"));
    assert_eq!(uri_to_path(&uri).unwrap(), path);
    assert!(uri_to_path("https://example.com/code.rs").is_none());
    assert!(uri_to_path("file:///bad%XZ.rs").is_none());
    assert!(uri_to_path("file:///bad%00.rs").is_none());
    assert!(path_to_uri(Path::new("relative/file.rs")).is_err());
}

#[cfg(windows)]
#[test]
fn windows_file_uris_handle_extended_prefixes_and_unc_shares() {
    let extended = Path::new(r"\\?\C:\Code\main.rs");
    assert_eq!(path_to_uri(extended).unwrap(), "file:///C:/Code/main.rs");
    let unc = Path::new(r"\\?\UNC\server\share\hello world.rs");
    let uri = path_to_uri(unc).unwrap();
    assert_eq!(uri, "file://server/share/hello%20world.rs");
    assert_eq!(
        uri_to_path(&uri).unwrap(),
        Path::new(r"\\server\share\hello world.rs")
    );
    assert_eq!(
        uri_to_path("file://localhost/C:/Code/main.rs").unwrap(),
        Path::new(r"C:\Code\main.rs")
    );
    assert_eq!(
        uri_to_path("file:///C%3A/Code/main.rs").unwrap(),
        Path::new(r"C:\Code\main.rs")
    );
}

#[test]
fn framing_counts_utf8_bytes_and_reads_multiple_messages() {
    let first = json!({"jsonrpc":"2.0","method":"test","params":{"text":"🦀 ü"}});
    let second = json!({"jsonrpc":"2.0","id":7,"result":null});
    let mut bytes = Vec::new();
    write_message(&mut bytes, &first).unwrap();
    write_message(&mut bytes, &second).unwrap();
    let mut reader = Cursor::new(bytes);
    assert_eq!(read_message(&mut reader).unwrap(), Some(first));
    assert_eq!(read_message(&mut reader).unwrap(), Some(second));
    assert_eq!(read_message(&mut reader).unwrap(), None);
}

#[test]
fn malformed_or_oversized_frames_fail_before_allocating_the_body() {
    let cases = [
        b"Content-Length: nope\r\n\r\n".as_slice(),
        b"Content-Length: 17000000\r\n\r\n".as_slice(),
        b"Content-Length: 2\r\nContent-Length: 2\r\n\r\n{}".as_slice(),
        b"Content-Type: application/json\r\n\r\n{}".as_slice(),
        b"Content-Length: 10\r\n\r\n{}".as_slice(),
        b"Content-Length: 3\r\n\r\nx{}".as_slice(),
        b"Content-Length: 0\r\n\r\n".as_slice(),
    ];
    for bytes in cases {
        assert!(read_message(&mut Cursor::new(bytes)).is_err());
    }
    assert!(read_message(&mut Cursor::new(vec![b'a'; HEADER_LIMIT + 2])).is_err());
}

#[test]
fn completion_parsing_supports_lists_replace_edits_imports_and_plain_snippets() {
    let result = json!({"isIncomplete":false,"items":[
        {"label":"later","sortText":"2","insertText":"later()"},
        {"label":"first","sortText":"1","detail":"fn first(value: i32)","insertTextFormat":2,
            "textEdit":{"insert":range(2,3,3),"replace":range(2,3,8),"newText":"first(${1:value})$0"},
            "additionalTextEdits":[{"range":range(0,0,0),"newText":"use sample::first;\n"}]}
    ]});
    let items = parse_completions(&result);
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].label, "first");
    assert_eq!(items[0].insert_text, "first(value)");
    assert_eq!(items[0].text_edit.as_ref().unwrap().range.end.character, 8);
    assert_eq!(
        items[0].additional_text_edits[0].new_text,
        "use sample::first;\n"
    );
    assert_eq!(items[1].insert_text, "later()");
    assert!(parse_completions(&Value::Null).is_empty());
    assert_eq!(
        plain_snippet("${1|red,green,blue|} ${2:${3:nested}} \\$literal $0"),
        "red nested $literal "
    );
}

#[test]
fn snippet_fallback_preserves_escaped_braces_and_nested_placeholders() {
    assert_eq!(plain_snippet(r"${1:before\}after}"), "before}after");
    assert_eq!(
        plain_snippet(r"${1:before\}${2:nested}after} $0"),
        "before}nestedafter "
    );
    assert_eq!(plain_snippet(r"${1:before\\}after"), r"before\after");
}

#[test]
fn definition_parsing_prefers_target_selection_ranges() {
    let result = json!([{"targetUri":"file:///C:/Code/main.rs","targetRange":range(5,0,20),"targetSelectionRange":range(6,7,12)}]);
    let (_, position) = parse_definition(&result).unwrap();
    assert_eq!(
        position,
        Position {
            line: 6,
            character: 7
        }
    );
    let result = json!({"uri":"file:///C:/Code/main.rs","range":range(1,2,3)});
    assert_eq!(
        parse_definition(&result).unwrap().1,
        Position {
            line: 1,
            character: 2
        }
    );
    assert!(parse_definition(&json!([])).is_none());
    assert!(parse_definition(&Value::Null).is_none());
}

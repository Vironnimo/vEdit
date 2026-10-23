//! Hover, references and symbol requests of the language client.
use super::tests::{TestWorkspace, initialize_protocol, protocol};
use super::*;
use std::fs;

fn text_range(line: u32, start: u32, end: u32) -> TextRange {
    TextRange {
        start: Position {
            line,
            character: start,
        },
        end: Position {
            line,
            character: end,
        },
    }
}

fn range_json(line: u32, start: u32, end: u32) -> Value {
    json!({"start":{"line":line,"character":start},"end":{"line":line,"character":end}})
}

#[test]
fn hover_parsing_supports_markup_marked_strings_arrays_and_limits() {
    let parse = |contents: Value| parse_hover(&json!({ "contents": contents }));
    assert_eq!(
        parse(json!({"kind":"markdown","value":"  **x**  \n"})),
        Some(HoverContents {
            markdown: true,
            text: "**x**".into()
        })
    );
    assert_eq!(
        parse(json!({"kind":"plaintext","value":"fn f() -> 🙂"})),
        Some(HoverContents {
            markdown: false,
            text: "fn f() -> 🙂".into()
        })
    );
    assert_eq!(
        parse(json!({"language":"rust","value":"fn f()"}))
            .unwrap()
            .text,
        "```rust\nfn f()\n```"
    );
    assert_eq!(
        parse(json!([{"language":"rust","value":"fn f()"}, "", "Docs"]))
            .unwrap()
            .text,
        "```rust\nfn f()\n```\n\nDocs"
    );
    assert_eq!(parse(json!("plain")).unwrap().text, "plain");
    assert_eq!(parse(json!({"kind":"markdown","value":"   "})), None);
    assert_eq!(parse(json!([])), None);
    assert_eq!(parse_hover(&Value::Null), None);
    let long = parse(json!({"kind":"plaintext","value":"漢".repeat(HOVER_LIMIT)})).unwrap();
    assert!(long.text.len() <= HOVER_LIMIT + '…'.len_utf8());
    assert!(long.text.ends_with('…'));
}

#[test]
fn reference_previews_convert_utf16_columns_to_scalars_for_emoji_cjk_and_crlf() {
    let root = std::env::temp_dir().join("vedit-reference-previews");
    let open_path = root.join("open.rs");
    let disk_path = root.join("disk.rs");
    let missing_path = root.join("missing.rs");
    // CLI edits can leave raw CRLF in an open buffer.
    let open = HashMap::from([(
        open_path.clone(),
        "fn main() {\r\n    let 🙂 = 漢字(🦀name);\r\n}\r\n".to_owned(),
    )]);
    let read = |path: &Path| (path == disk_path).then(|| "a\nb 🙂 name\n".to_owned());
    let references = build_references(
        vec![
            (open_path.clone(), text_range(1, 18, 22)),
            (disk_path.clone(), text_range(1, 5, 9)),
            (missing_path.clone(), text_range(4, 3, 7)),
        ],
        &open,
        read,
    );
    assert_eq!(references.len(), 3);

    let first = &references[0];
    assert_eq!(first.path, canonical_path(&open_path));
    assert_eq!((first.line, first.column), (2, 17));
    assert_eq!(first.preview, "let 🙂 = 漢字(🦀name);");
    let highlighted: String = first
        .preview
        .chars()
        .skip(first.highlight.start)
        .take(first.highlight.len())
        .collect();
    assert_eq!(highlighted, "name");
    assert_eq!(
        first.position,
        Position {
            line: 1,
            character: 18
        }
    );

    let second = &references[1];
    assert_eq!((second.line, second.column), (2, 5));
    assert_eq!(second.preview, "b 🙂 name");
    assert_eq!(second.highlight, 4..8);

    let missing = &references[2];
    assert_eq!(missing.preview, "");
    assert_eq!((missing.line, missing.column), (5, 4));
}

#[test]
fn reference_previews_preserve_result_order_and_share_each_source() {
    let root = std::env::temp_dir().join("vedit-reference-order");
    let open_path = root.join("open.rs");
    let disk_path = root.join("disk.rs");
    let open = HashMap::from([(open_path.clone(), "zero\r\n🙂 first\r\nlast\r\n".to_owned())]);
    let reads = std::cell::Cell::new(0);
    let references = build_references(
        vec![
            (open_path.clone(), text_range(2, 0, 4)),
            (disk_path.clone(), text_range(1, 0, 3)),
            (open_path.clone(), text_range(1, 3, 8)),
            (open_path.clone(), text_range(1, 0, 2)),
            (disk_path.clone(), text_range(0, 0, 3)),
            (open_path.clone(), text_range(7, 0, 1)),
            (open_path, text_range(3, 0, 1)),
        ],
        &open,
        |path| {
            assert_eq!(path, disk_path);
            reads.set(reads.get() + 1);
            Some("one\ntwo".to_owned())
        },
    );
    assert_eq!(reads.get(), 1);
    let previews: Vec<_> = references
        .iter()
        .map(|reference| (reference.line, reference.column, reference.preview.as_str()))
        .collect();
    assert_eq!(
        previews,
        vec![
            (3, 1, "last"),
            (2, 1, "two"),
            (2, 3, "🙂 first"),
            (2, 1, "🙂 first"),
            (1, 1, "one"),
            (8, 1, ""),
            (4, 1, ""),
        ]
    );
}

#[test]
#[ignore = "manual reference-preview benchmark"]
fn benchmark_reference_preview_batches() {
    let path = std::env::temp_dir().join("vedit-reference-benchmark.rs");
    let text = "let value = target(); // benchmark source line\n".repeat(100_000);
    let locations = (0..REFERENCE_LIMIT)
        .rev()
        .map(|index| (path.clone(), text_range((index * 50) as u32, 12, 18)))
        .collect();
    let open = HashMap::from([(path, text)]);
    let started = Instant::now();
    let references = build_references(locations, &open, |_| panic!("open source is available"));
    eprintln!("Reference preview batch: {:?}", started.elapsed());
    assert_eq!(references.len(), REFERENCE_LIMIT);
    assert!(references.iter().all(|reference| {
        reference.preview == "let value = target(); // benchmark source line"
            && reference.highlight == (12..18)
    }));
}

#[test]
fn long_reference_lines_keep_the_match_visible() {
    let line = format!("{}target{}", "x".repeat(300), "y".repeat(300));
    let (column, preview, highlight) = line_preview(&line, text_range(0, 300, 306));
    assert_eq!(column, 300);
    assert!(preview.starts_with('…') && preview.ends_with('…'));
    assert!(preview.chars().count() <= PREVIEW_CHARS + 2);
    let highlighted: String = preview
        .chars()
        .skip(highlight.start)
        .take(highlight.len())
        .collect();
    assert_eq!(highlighted, "target");
}

#[test]
fn locations_accept_links_skip_duplicates_and_report_truncation() {
    let root = std::env::temp_dir();
    let uri = path_to_uri(&root.join("a.rs")).unwrap();
    let result = json!([
        {"uri":uri,"range":range_json(1,2,3)},
        {"uri":uri,"range":range_json(1,2,3)},
        {"targetUri":uri,"targetRange":range_json(5,0,9),"targetSelectionRange":range_json(5,4,7)},
        {"uri":uri},
    ]);
    let (locations, truncated) = parse_locations(&result);
    assert!(!truncated);
    assert_eq!(
        locations
            .iter()
            .map(|(_, range)| *range)
            .collect::<Vec<_>>(),
        vec![text_range(1, 2, 3), text_range(5, 4, 7)]
    );
    assert_eq!(parse_locations(&Value::Null), (Vec::new(), false));

    let many: Vec<Value> = (0..REFERENCE_LIMIT as u32 + 5)
        .map(|line| json!({"uri":uri,"range":range_json(line,0,1)}))
        .collect();
    let (locations, truncated) = parse_locations(&Value::Array(many));
    assert_eq!(locations.len(), REFERENCE_LIMIT);
    assert!(truncated);
}

#[test]
fn symbols_keep_nesting_containers_and_flat_order() {
    let hierarchical = json!([{
        "name":"Parser","kind":23,"range":range_json(2,0,40),"selectionRange":range_json(2,11,17),
        "children":[{"name":"parse","kind":6,"detail":"fn(&self)","range":range_json(3,4,30),"selectionRange":range_json(3,11,16)}]
    }]);
    let symbols = parse_symbols(&hierarchical, false);
    assert_eq!(symbols.len(), 2);
    assert_eq!(symbols[0].name, "Parser");
    assert_eq!(
        (symbols[0].depth, symbols[0].container.as_deref()),
        (0, None)
    );
    assert_eq!(symbols[1].name, "parse");
    assert_eq!(symbols[1].container.as_deref(), Some("Parser"));
    assert_eq!(symbols[1].detail.as_deref(), Some("fn(&self)"));
    assert_eq!(symbols[1].depth, 1);
    assert_eq!(
        symbols[1].position,
        Position {
            line: 3,
            character: 11
        }
    );
    assert_eq!(symbol_kind_label(symbols[0].kind), "struct");
    assert_eq!(symbol_kind_label(symbols[1].kind), "method");
    assert_eq!(symbol_kind_label(99), "symbol");

    let root = std::env::temp_dir();
    let uri = path_to_uri(&root.join("lib.rs")).unwrap();
    let flat = json!([
        {"name":"later","kind":12,"location":{"uri":uri,"range":range_json(9,3,8)},"containerName":"tools"},
        {"name":"first","kind":12,"location":{"uri":uri,"range":range_json(1,3,8)}},
        {"name":"lazy","kind":12,"location":{"uri":uri}},
        {"name":"nowhere","kind":12,"location":{}},
    ]);
    let document = parse_symbols(&flat, false);
    assert_eq!(
        document
            .iter()
            .map(|symbol| symbol.name.as_str())
            .collect::<Vec<_>>(),
        ["lazy", "nowhere", "first", "later"]
    );
    assert!(document.iter().all(|symbol| symbol.path.is_none()));
    let workspace = parse_symbols(&flat, true);
    assert_eq!(
        workspace
            .iter()
            .map(|symbol| symbol.name.as_str())
            .collect::<Vec<_>>(),
        ["later", "first", "lazy"]
    );
    assert_eq!(workspace[0].container.as_deref(), Some("tools"));
    assert_eq!(workspace[0].path, uri_to_path(&uri));
    assert_eq!(workspace[2].position, Position::default());
}

#[test]
fn client_advertises_exactly_the_implemented_language_features() {
    let capabilities = client_capabilities();
    let document = &capabilities["textDocument"];
    assert_eq!(
        document["hover"]["contentFormat"],
        json!(["markdown", "plaintext"])
    );
    assert_eq!(document["references"], json!({}));
    assert_eq!(
        document["documentSymbol"]["hierarchicalDocumentSymbolSupport"],
        true
    );
    assert!(capabilities["workspace"]["symbol"].is_object());
    for unimplemented in [
        "rename",
        "codeAction",
        "signatureHelp",
        "implementation",
        "typeDefinition",
        "documentHighlight",
        "formatting",
    ] {
        assert!(
            document.get(unimplemented).is_none(),
            "{unimplemented} must not be advertised"
        );
    }
}

#[test]
fn server_capabilities_decide_features_and_missing_ones_fail_without_a_request() {
    let (mut protocol, outgoing) = protocol();
    protocol.config.name = "Python analysis".into();
    initialize_protocol(
        &mut protocol,
        json!({"hoverProvider":true,"referencesProvider":{"workDoneProgress":true},"documentSymbolProvider":false}),
    );
    let features = protocol.features.load(Ordering::Acquire);
    assert_ne!(features & Feature::Hover.bit(), 0);
    assert_ne!(features & Feature::References.bit(), 0);
    assert_eq!(features & Feature::DocumentSymbols.bit(), 0);
    assert_eq!(features & Feature::WorkspaceSymbols.bit(), 0);
    outgoing.try_iter().for_each(drop);

    let path = protocol.root.join("features.py");
    protocol
        .command(ClientCommand::DocumentSymbols {
            id: 61,
            path: path.clone(),
        })
        .unwrap();
    protocol
        .command(ClientCommand::WorkspaceSymbols {
            id: 62,
            query: "x".into(),
        })
        .unwrap();
    assert!(outgoing.try_recv().is_err());
    assert!(protocol.pending.is_empty());
    assert!(matches!(
        protocol.failed_requests.pop_front(),
        Some(LspEvent::RequestFailed { request_id: 61, error: Some(error) })
            if error == "Python analysis does not provide document symbols."
    ));
    assert!(matches!(
        protocol.failed_requests.pop_front(),
        Some(LspEvent::RequestFailed { request_id: 62, error: Some(error) })
            if error.contains("workspace symbols")
    ));

    protocol
        .command(ClientCommand::Hover {
            id: 63,
            path,
            position: Position {
                line: 2,
                character: 5,
            },
        })
        .unwrap();
    let message = outgoing.try_recv().unwrap().message;
    assert_eq!(message["method"], "textDocument/hover");
    assert_eq!(message["id"], 63);
    assert_eq!(
        message["params"]["position"],
        json!({"line":2,"character":5})
    );
}

#[test]
fn hover_references_and_symbols_route_results_and_rpc_errors() {
    let (mut protocol, outgoing) = protocol();
    protocol.ready = true;
    let (sender, events) = mpsc::sync_channel(8);
    let emitter = EventEmitter::new(sender, None);
    let path = protocol.root.join("vedit-intel-routing-missing.rs");
    let position = Position {
        line: 1,
        character: 18,
    };

    protocol
        .command(ClientCommand::Hover {
            id: 81,
            path: path.clone(),
            position,
        })
        .unwrap();
    assert_eq!(
        outgoing.try_recv().unwrap().message["params"]["position"],
        json!({"line":1,"character":18})
    );
    protocol
        .message(
            json!({"jsonrpc":"2.0","id":81,"result":{"contents":{"kind":"markdown","value":"`name`"}}}),
            &emitter,
        )
        .unwrap();
    assert!(matches!(
        events.try_recv().unwrap(),
        LspEvent::Hover { request_id: 81, contents: Some(HoverContents { markdown: true, text }) }
            if text == "`name`"
    ));

    protocol
        .command(ClientCommand::References {
            id: 82,
            path: path.clone(),
            position,
        })
        .unwrap();
    let message = outgoing.try_recv().unwrap().message;
    assert_eq!(message["method"], "textDocument/references");
    assert_eq!(message["params"]["context"]["includeDeclaration"], true);
    let uri = path_to_uri(&path).unwrap();
    protocol
        .message(
            json!({"jsonrpc":"2.0","id":82,"result":[{"uri":uri,"range":range_json(3,4,8)}]}),
            &emitter,
        )
        .unwrap();
    // Previews are read off the protocol thread.
    match events.recv_timeout(Duration::from_secs(5)).unwrap() {
        LspEvent::References {
            request_id: 82,
            references,
            truncated: false,
        } => {
            assert_eq!(references.len(), 1);
            assert_eq!((references[0].line, references[0].column), (4, 5));
        }
        other => panic!("unexpected event {other:?}"),
    }

    protocol
        .command(ClientCommand::DocumentSymbols {
            id: 83,
            path: path.clone(),
        })
        .unwrap();
    assert_eq!(
        outgoing.try_recv().unwrap().message["method"],
        "textDocument/documentSymbol"
    );
    protocol
        .message(
            json!({"jsonrpc":"2.0","id":83,"result":[{"name":"main","kind":12,"range":range_json(0,0,9),"selectionRange":range_json(0,3,7)}]}),
            &emitter,
        )
        .unwrap();
    assert!(matches!(
        events.try_recv().unwrap(),
        LspEvent::Symbols { request_id: 83, symbols } if symbols.len() == 1 && symbols[0].name == "main"
    ));

    protocol
        .command(ClientCommand::WorkspaceSymbols {
            id: 84,
            query: "ma".into(),
        })
        .unwrap();
    let message = outgoing.try_recv().unwrap().message;
    assert_eq!(message["method"], "workspace/symbol");
    assert_eq!(message["params"]["query"], "ma");
    protocol
        .message(
            json!({"jsonrpc":"2.0","id":84,"error":{"code":-32603,"message":"index not ready"}}),
            &emitter,
        )
        .unwrap();
    assert!(matches!(
        events.try_recv().unwrap(),
        LspEvent::RequestFailed { request_id: 84, error: Some(error) } if error.contains("index not ready")
    ));
    assert!(events.try_recv().is_err());
    assert!(protocol.pending.is_empty());
}

#[test]
fn requests_before_initialization_wait_for_the_capabilities() {
    let (mut protocol, outgoing) = protocol();
    protocol.features.store(0, Ordering::Release);
    let path = protocol.root.join("early.rs");
    protocol
        .command(ClientCommand::Hover {
            id: 91,
            path,
            position: Position::default(),
        })
        .unwrap();
    assert!(outgoing.try_recv().is_err());
    initialize_protocol(&mut protocol, json!({"hoverProvider":true}));
    let methods: Vec<_> = outgoing
        .try_iter()
        .map(|out| out.message["method"].clone())
        .collect();
    assert!(
        methods.contains(&json!("textDocument/hover")),
        "{methods:?}"
    );
    assert!(protocol.failed_requests.is_empty());
}

#[test]
#[ignore = "requires the installed rust-analyzer and Rust toolchain; run explicitly"]
fn live_rust_analyzer_hover_references_and_symbols() {
    let workspace = TestWorkspace::new();
    let path = workspace.0.join("src/lib.rs");
    let text = "/// Doubles 🙂 a value.\npub fn double(value: i32) -> i32 { value * 2 }\npub fn run() -> i32 { double(4) + double(5) }\n";
    fs::write(&path, text).unwrap();
    let server = LanguageServer::start(workspace.0.clone(), None);
    assert!(server.open(&path, text));
    let usage = char_to_position(text, text.find("double(4)").unwrap() + 2);
    let definition = char_to_position(text, text.find("double(value").unwrap() + 1);
    let deadline = Instant::now() + Duration::from_secs(90);
    let (mut ready, mut hover, mut references, mut symbols) = (false, false, false, false);
    let mut next_request = Instant::now();
    while Instant::now() < deadline && !(hover && references && symbols) {
        if ready && Instant::now() >= next_request {
            assert!(server.supports(Feature::Hover));
            assert!(server.supports(Feature::References));
            assert!(server.supports(Feature::DocumentSymbols));
            assert!(server.supports(Feature::WorkspaceSymbols));
            if !hover {
                assert_ne!(server.hover(&path, usage), 0);
            }
            if !references {
                assert_ne!(server.references(&path, definition), 0);
            }
            if !symbols {
                assert_ne!(server.document_symbols(&path), 0);
            }
            next_request = Instant::now() + Duration::from_secs(1);
        }
        match server.receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(LspEvent::Ready) => ready = true,
            Ok(LspEvent::Hover {
                contents: Some(contents),
                ..
            }) => {
                if contents.text.contains("double") && contents.text.contains("Doubles 🙂") {
                    assert!(contents.markdown);
                    hover = true;
                }
            }
            Ok(LspEvent::References {
                references: found, ..
            }) => {
                // Declaration plus both calls, in this file.
                if found.len() == 3 {
                    assert!(
                        found
                            .iter()
                            .all(|reference| normalize_path(&reference.path)
                                == normalize_path(&path))
                    );
                    assert!(found.iter().any(|reference| reference.line == 3
                        && reference.preview.starts_with("pub fn run()")));
                    references = true;
                }
            }
            Ok(LspEvent::Symbols { symbols: found, .. }) => {
                if found.iter().any(|symbol| symbol.name == "double")
                    && found.iter().any(|symbol| symbol.name == "run")
                {
                    symbols = true;
                }
            }
            Ok(LspEvent::Error(error)) => panic!("live rust-analyzer error: {error}"),
            _ => {}
        }
    }
    assert!(ready, "rust-analyzer never initialized");
    assert!(hover, "rust-analyzer did not describe the hovered function");
    assert!(
        references,
        "rust-analyzer did not list the three references"
    );
    assert!(symbols, "rust-analyzer did not list the document symbols");
    server.shutdown();
    drop(server);
    thread::sleep(Duration::from_millis(150));
}

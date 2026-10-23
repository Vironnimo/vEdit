//! Negotiation, document synchronization and pending-request contracts.
use super::*;

#[test]
fn python_and_dart_open_documents_with_the_configured_language_id_and_settings() {
    for language in ["python", "dart"] {
        let (mut protocol, outgoing) = protocol();
        protocol.config.language_id = language.into();
        protocol.config.settings = json!({language: {"configured": true}});
        let path = protocol.root.join(format!("language.{language}"));
        protocol
            .command(ClientCommand::Open {
                path,
                text: "example".into(),
            })
            .unwrap();
        initialize_protocol(
            &mut protocol,
            json!({"textDocumentSync":{"openClose":true,"change":1}}),
        );
        let messages = outgoing
            .try_iter()
            .map(|out| out.message)
            .collect::<Vec<_>>();
        assert_eq!(messages[0]["method"], "initialized");
        assert_eq!(messages[1]["method"], "workspace/didChangeConfiguration");
        assert_eq!(
            messages[1]["params"]["settings"][language]["configured"],
            true
        );
        assert_eq!(messages[2]["method"], "textDocument/didOpen");
        assert_eq!(
            messages[2]["params"]["textDocument"]["languageId"],
            language
        );
    }
}

#[test]
fn generic_configuration_supports_whole_nested_and_dotted_sections() {
    let settings = json!({"python":{"pythonPath":"/venv/python", "analysis":{"typeCheckingMode":"basic"}}, "dart":{"lineLength":100}, "custom.flat":{"enabled":true}});
    assert_eq!(configuration_section(&settings, ""), settings);
    assert_eq!(
        configuration_section(&settings, "python.pythonPath"),
        "/venv/python"
    );
    assert_eq!(
        configuration_section(&settings, "python.analysis.typeCheckingMode"),
        "basic"
    );
    assert_eq!(
        configuration_section(&settings, "custom.flat")["enabled"],
        true
    );
    assert_eq!(configuration_section(&settings, "missing"), Value::Null);
}

#[test]
fn document_end_matches_scalar_positions_for_unicode_and_mixed_line_endings() {
    for text in [
        "",
        "plain",
        "a\r\nb\n",
        "\r",
        "🦀\r\nGrüße🦀\r",
        "a\r\r\nb\r🦀",
        "\n\n🦀",
    ] {
        assert_eq!(
            document_end_position(text),
            char_to_position(text, text.chars().count()),
            "{text:?}",
        );
    }
}

#[test]
fn full_sync_sends_unicode_text_without_calculating_incremental_ranges() {
    let (mut protocol, outgoing) = protocol();
    let path = protocol.root.join("full-sync.py");
    protocol
        .command(ClientCommand::Open {
            path: path.clone(),
            text: "🦀\r\nold".into(),
        })
        .unwrap();
    initialize_protocol(
        &mut protocol,
        json!({"textDocumentSync":{"openClose":true,"change":1}}),
    );
    outgoing.try_iter().for_each(drop);
    assert_eq!(protocol.documents[&path].synced_end, Position::default());
    protocol
        .command(ClientCommand::Change {
            path: path.clone(),
            text: "🦀\r\nnew".into(),
            version: 2,
        })
        .unwrap();
    protocol.flush_documents().unwrap();
    let change = outgoing.try_recv().unwrap().message;
    assert_eq!(
        change["params"]["contentChanges"][0],
        json!({"text":"🦀\r\nnew"})
    );
    assert_eq!(protocol.documents[&path].synced_end, Position::default());
}

#[test]
fn incremental_changes_use_previous_sent_utf16_range_and_save_includes_text_when_requested() {
    let (mut protocol, outgoing) = protocol();
    protocol.config.language_id = "python".into();
    let path = protocol.root.join("unicode.py");
    protocol
        .command(ClientCommand::Open {
            path: path.clone(),
            text: "a\u{1f600}\r\nz\u{1f600}".into(),
        })
        .unwrap();
    initialize_protocol(
        &mut protocol,
        json!({"textDocumentSync":{"openClose":true,"change":2,"save":{"includeText":true}}}),
    );
    outgoing.try_iter().for_each(drop);
    protocol
        .command(ClientCommand::Change {
            path: path.clone(),
            text: "intermediate".into(),
            version: 2,
        })
        .unwrap();
    protocol
        .command(ClientCommand::Change {
            path: path.clone(),
            text: "new\u{1f600}".into(),
            version: 3,
        })
        .unwrap();
    protocol.command(ClientCommand::Save(path.clone())).unwrap();
    protocol.flush_documents().unwrap();
    let change = outgoing.try_recv().unwrap().message;
    assert_eq!(change["method"], "textDocument/didChange");
    assert_eq!(change["params"]["textDocument"]["version"], 3);
    assert_eq!(
        change["params"]["contentChanges"][0]["range"],
        json!({"start":{"line":0,"character":0},"end":{"line":1,"character":3}})
    );
    assert_eq!(
        change["params"]["contentChanges"][0]["text"],
        "new\u{1f600}"
    );
    let save = outgoing.try_recv().unwrap().message;
    assert_eq!(save["method"], "textDocument/didSave");
    assert_eq!(save["params"]["text"], "new\u{1f600}");
    protocol
        .command(ClientCommand::Change {
            path,
            text: "last".into(),
            version: 4,
        })
        .unwrap();
    protocol.flush_documents().unwrap();
    assert_eq!(
        outgoing.try_recv().unwrap().message["params"]["contentChanges"][0]["range"]["end"],
        json!({"line":0,"character":5})
    );
}

#[test]
fn absent_features_report_unavailable_without_sending_requests_or_fake_success() {
    let (mut protocol, outgoing) = protocol();
    protocol.config.name = "Python analysis".into();
    initialize_protocol(&mut protocol, json!({}));
    outgoing.try_iter().for_each(drop);
    let path = protocol.root.join("features.py");
    protocol
        .command(ClientCommand::Completion {
            id: 71,
            path: path.clone(),
            position: Position::default(),
            version: 1,
        })
        .unwrap();
    protocol
        .command(ClientCommand::Definition {
            id: 72,
            path,
            position: Position::default(),
        })
        .unwrap();
    assert!(outgoing.try_recv().is_err());
    assert!(protocol.pending.is_empty());
    assert!(
        matches!(protocol.failed_requests.pop_front(), Some(LspEvent::RequestFailed { request_id:71,error:Some(error) }) if error.contains("Python analysis") && error.contains("completion"))
    );
    assert!(
        matches!(protocol.failed_requests.pop_front(), Some(LspEvent::RequestFailed { request_id:72,error:Some(error) }) if error.contains("go to definition"))
    );
}

#[test]
fn disabled_sync_and_save_capabilities_produce_no_notifications() {
    let (mut protocol, outgoing) = protocol();
    initialize_protocol(
        &mut protocol,
        json!({"textDocumentSync":{"openClose":false,"change":0,"save":false}}),
    );
    outgoing.try_iter().for_each(drop);
    let path = protocol.root.join("no-sync.py");
    protocol
        .command(ClientCommand::Open {
            path: path.clone(),
            text: "first".into(),
        })
        .unwrap();
    protocol.flush_documents().unwrap();
    protocol
        .command(ClientCommand::Change {
            path: path.clone(),
            text: "second".into(),
            version: 2,
        })
        .unwrap();
    protocol.command(ClientCommand::Save(path.clone())).unwrap();
    protocol.flush_documents().unwrap();
    protocol.command(ClientCommand::Close(path)).unwrap();
    assert!(outgoing.try_recv().is_err());
}

#[test]
fn document_updates_are_coalesced_until_initialized_and_stale_changes_are_ignored() {
    let (mut protocol, outgoing) = protocol();
    let path = protocol.root.join("vedit-lsp-document.rs");
    protocol
        .command(ClientCommand::Open {
            path: path.clone(),
            text: "old".into(),
        })
        .unwrap();
    protocol
        .command(ClientCommand::Change {
            path: path.clone(),
            text: "newest".into(),
            version: 3,
        })
        .unwrap();
    protocol
        .command(ClientCommand::Change {
            path: path.clone(),
            text: "stale".into(),
            version: 2,
        })
        .unwrap();
    assert!(outgoing.try_recv().is_err());
    protocol.ready = true;
    protocol.flush_documents().unwrap();
    let message = outgoing.try_recv().unwrap().message;
    assert_eq!(message["method"], "textDocument/didOpen");
    assert_eq!(message["params"]["textDocument"]["text"], "newest");
    assert_eq!(message["params"]["textDocument"]["version"], 3);
    protocol
        .command(ClientCommand::Change {
            path: path.clone(),
            text: "fourth".into(),
            version: 4,
        })
        .unwrap();
    protocol
        .command(ClientCommand::Completion {
            id: 7,
            path,
            position: Position::default(),
            version: 4,
        })
        .unwrap();
    assert_eq!(
        outgoing.try_recv().unwrap().message["method"],
        "textDocument/didChange"
    );
    assert_eq!(
        outgoing.try_recv().unwrap().message["method"],
        "textDocument/completion"
    );
}

#[test]
fn rpc_request_errors_emit_failure_only_and_later_success_still_works() {
    for completion in [false, true] {
        let (mut protocol, _outgoing) = protocol();
        protocol.ready = true;
        let path = protocol.root.join("vedit-lsp-error.rs");
        let (sender, events) = mpsc::sync_channel(8);
        let emitter = EventEmitter::new(sender, None);
        let request = |id| {
            if completion {
                ClientCommand::Completion {
                    id,
                    path: path.clone(),
                    position: Position::default(),
                    version: 1,
                }
            } else {
                ClientCommand::Definition {
                    id,
                    path: path.clone(),
                    position: Position::default(),
                }
            }
        };
        protocol.command(request(41)).unwrap();
        protocol.message(json!({"jsonrpc":"2.0","id":41,"error":{"code":-32603,"message":"temporary server failure"}}), &emitter).unwrap();
        assert!(
            matches!(events.try_recv().unwrap(), LspEvent::RequestFailed { request_id: 41, error: Some(error) } if error.contains("temporary server failure"))
        );
        assert!(
            events.try_recv().is_err(),
            "an RPC error must not synthesize a successful empty response"
        );
        assert!(!protocol.pending.contains_key(&41));

        protocol.command(request(42)).unwrap();
        let result = if completion {
            json!([{ "label": "recovered" }])
        } else {
            Value::Null
        };
        protocol
            .message(json!({"jsonrpc":"2.0","id":42,"result":result}), &emitter)
            .unwrap();
        let event = events.try_recv().unwrap();
        if completion {
            assert!(
                matches!(event, LspEvent::Completions { request_id: 42, items, .. } if items.len() == 1 && items[0].label == "recovered")
            );
        } else {
            assert!(matches!(
                event,
                LspEvent::DefinitionNotFound { request_id: 42 }
            ));
        }
        assert!(events.try_recv().is_err());
        assert!(!protocol.pending.contains_key(&42));
    }
}

#[test]
fn rpc_cancellation_clears_the_request_without_success_or_a_warning() {
    let (mut protocol, _outgoing) = protocol();
    protocol.ready = true;
    let path = protocol.root.join("vedit-lsp-cancel.rs");
    let (sender, events) = mpsc::sync_channel(8);
    let emitter = EventEmitter::new(sender, None);
    protocol
        .command(ClientCommand::Definition {
            id: 51,
            path,
            position: Position::default(),
        })
        .unwrap();
    protocol
        .message(
            json!({"jsonrpc":"2.0","id":51,"error":{"code":-32800,"message":"Request cancelled"}}),
            &emitter,
        )
        .unwrap();
    assert!(matches!(
        events.try_recv().unwrap(),
        LspEvent::RequestFailed {
            request_id: 51,
            error: None
        }
    ));
    assert!(events.try_recv().is_err());
    assert!(!protocol.pending.contains_key(&51));
}

#[test]
fn configuration_and_registration_requests_receive_responses() {
    let (mut protocol, outgoing) = protocol();
    let (sender, _events) = mpsc::sync_channel(8);
    let emitter = EventEmitter::new(sender, None);
    protocol.message(json!({"jsonrpc":"2.0","id":"config","method":"workspace/configuration","params":{"items":[{"section":"rust-analyzer"},{"section":"rust-analyzer.cargo"},{"section":"other"}]}}), &emitter).unwrap();
    let response = outgoing.try_recv().unwrap().message;
    assert_eq!(response["id"], "config");
    assert_eq!(response["result"][0]["checkOnSave"], true);
    assert_eq!(response["result"][1]["targetDir"], true);
    assert!(response["result"][2].is_null());
    protocol
        .message(
            json!({"id":9,"method":"client/registerCapability","params":{}}),
            &emitter,
        )
        .unwrap();
    assert!(outgoing.try_recv().unwrap().message["result"].is_null());
    protocol
        .message(
            json!({"id":10,"method":"workspace/applyEdit","params":{}}),
            &emitter,
        )
        .unwrap();
    assert_eq!(
        outgoing.try_recv().unwrap().message["result"]["applied"],
        false
    );
}

#[test]
fn writer_pressure_is_bounded_and_preserves_message_order() {
    let (mut protocol, outgoing) = protocol();
    for number in 0..100 {
        protocol.notify("test", json!({"number":number})).unwrap();
    }
    assert!(!protocol.queued_writes.is_empty());
    let mut received = Vec::new();
    while received.len() < 100 {
        while let Ok(outbound) = outgoing.try_recv() {
            received.push(outbound.message["params"]["number"].as_u64().unwrap());
        }
        protocol.flush_writes().unwrap();
    }
    assert_eq!(received, (0..100).collect::<Vec<_>>());
    assert!(protocol.queued_writes.is_empty());
    for number in 0..160 {
        protocol.notify("test", json!({"number":number})).unwrap();
    }
    assert!(protocol.notify("overflow", json!({})).is_err());
}

#[test]
fn versioned_diagnostics_do_not_replace_newer_document_state() {
    let (mut protocol, _outgoing) = protocol();
    let path = protocol.root.join("vedit-lsp-diagnostics.rs");
    protocol
        .command(ClientCommand::Change {
            path: path.clone(),
            text: "new".into(),
            version: 5,
        })
        .unwrap();
    let (sender, events) = mpsc::sync_channel(8);
    let emitter = EventEmitter::new(sender, None);
    let notification = |version| json!({"method":"textDocument/publishDiagnostics","params":{"uri":path_to_uri(&path).unwrap(),"version":version,"diagnostics":[{"range":range(0,0,1),"severity":2,"source":"rust-analyzer","message":"example warning"}]}});
    protocol.message(notification(4), &emitter).unwrap();
    assert!(events.try_recv().is_err());
    protocol.message(notification(5), &emitter).unwrap();
    let LspEvent::Diagnostics {
        diagnostics,
        version,
        ..
    } = events.try_recv().unwrap()
    else {
        panic!("expected diagnostics");
    };
    assert_eq!(version, Some(5));
    assert_eq!(diagnostics[0].severity, 2);
    assert_eq!(diagnostics[0].message, "example warning");
}

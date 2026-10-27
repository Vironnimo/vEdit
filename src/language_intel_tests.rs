//! Hover, references and symbol navigation against a scripted language server.
use super::language_intel::{
    HoverBlock, HoverBody, HoverSource, PointerTarget, ReferencesState, SymbolScope, SymbolState,
    filter_symbols, group_references, hover_document, symbol_scope, word_at,
};
use super::*;
use crate::lsp::{
    Diagnostic, Feature, HoverContents, LanguageServer, LspEvent, Position, Reference, Symbol,
    TestRequest, TestRequests, TextRange,
};
use egui_kittest::{Harness, kittest::Queryable};

struct Fixture(PathBuf);

impl Fixture {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let path = std::env::temp_dir().join(format!(
            "vedit-language-intel-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }

    fn app(&self, text: &str) -> (VEditApp, PathBuf) {
        let path = self.0.join("source.rs");
        fs::write(&path, text).unwrap();
        fs::write(
            self.0.join("other.rs"),
            "fn other() {\n    let 🙂 = name();\n}\n",
        )
        .unwrap();
        let mut app = VEditApp::empty(egui::Context::default());
        app.settings.autosave = false;
        app.recovery_session_path = self.0.join("recovery/session.json");
        app.add_project(self.0.clone());
        app.open_file(path.clone());
        wait_loaded(&mut app);
        app.tab_mut().unwrap().recovery_path = self.0.join("recovery/source.txt");
        (app, path)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let root = fs::canonicalize(std::env::temp_dir()).unwrap();
        assert!(self.0.starts_with(&root));
        assert!(
            self.0
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("vedit-language-intel-")
        );
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn wait_loaded(app: &mut VEditApp) {
    let deadline = Instant::now() + Duration::from_secs(8);
    let explorer_pending = |app: &VEditApp| {
        app.project()
            .unwrap()
            .explorer
            .as_ref()
            .is_some_and(|explorer| explorer.pending_count() > 0)
    };
    while app.project().unwrap().indexing
        || app.tab().is_none_or(|tab| tab.loading)
        || explorer_pending(app)
    {
        app.poll();
        assert!(Instant::now() < deadline, "fixture did not finish loading");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Connects a scripted Rust server that provides every feature.
fn connect(app: &mut VEditApp) -> (mpsc::SyncSender<LspEvent>, TestRequests) {
    let (server, events, requests) = LanguageServer::test_channel_with_requests();
    let tab = app.tab().unwrap();
    let path = tab.path.clone().unwrap();
    let revision = tab.revision;
    let session = app.projects[0].rust_language_mut();
    session.server = Some(server);
    session.ready = true;
    session.opened.insert(path, (revision, 7));
    (events, requests)
}

fn server(app: &VEditApp) -> &LanguageServer {
    app.projects[0].languages["rust"].server.as_ref().unwrap()
}

/// Places the cursor and the keyboard anchor as a rendered editor would.
fn cursor(app: &mut VEditApp, index: usize) {
    app.tab_mut().unwrap().set_cursor(index..index);
    app.intel.interactive = true;
    app.intel.caret = Some(egui::Rect::from_min_size(
        egui::pos2(100.0, 100.0),
        egui::vec2(2.0, 18.0),
    ));
}

fn markdown(text: &str) -> Option<HoverContents> {
    Some(HoverContents {
        markdown: true,
        text: text.into(),
    })
}

fn hover_request(requests: &TestRequests) -> (u64, Position) {
    match requests.next_request() {
        Some(TestRequest::Hover { id, position }) => (id, position),
        other => panic!("expected a hover request, got {other:?}"),
    }
}

fn reference(path: &Path, line: u32, character: u32) -> Reference {
    Reference {
        path: path.to_path_buf(),
        position: Position { line, character },
        line: line as usize + 1,
        column: character as usize + 1,
        preview: format!("line {line}"),
        highlight: 0..0,
    }
}

fn symbol(name: &str, kind: u32, container: Option<&str>, depth: usize, line: u32) -> Symbol {
    Symbol {
        name: name.into(),
        detail: None,
        kind,
        container: container.map(str::to_owned),
        path: None,
        position: Position { line, character: 4 },
        depth,
    }
}

#[test]
fn keyboard_hover_sends_utf16_positions_and_drops_results_after_an_edit() {
    let fixture = Fixture::new("stale");
    let (mut app, _) = fixture.app("let 🙂 = 漢字;\nname()\n");
    let (events, requests) = connect(&mut app);
    // Scalar 9 is 字: the emoji counts twice in UTF-16.
    cursor(&mut app, 9);
    app.show_hover();
    let (stale, position) = hover_request(&requests);
    assert_eq!(
        position,
        Position {
            line: 0,
            character: 10
        }
    );
    assert_eq!(
        app.intel.hover.as_ref().unwrap().body,
        HoverBody::Requested(stale)
    );

    let tab = app.tab_mut().unwrap();
    tab.text.insert(0, ' ');
    tab.changed();
    app.update_hover();
    assert!(
        app.intel.hover.is_none(),
        "an edit closes the explicit hover"
    );
    events
        .send(LspEvent::Hover {
            request_id: stale,
            contents: markdown("stale"),
        })
        .unwrap();
    app.poll_languages();
    assert!(app.intel.hover.is_none());
    assert_eq!(app.intel.hover_in_flight(), None);

    // A fresh request at the new revision is shown.
    cursor(&mut app, 10);
    app.show_hover();
    let (fresh, _) = hover_request(&requests);
    events
        .send(LspEvent::Hover {
            request_id: fresh,
            contents: markdown("```rust\nfn name()\n```\n---\nDocs"),
        })
        .unwrap();
    app.poll_languages();
    let popup = app.intel.hover.as_ref().unwrap();
    assert_eq!(popup.target.source, HoverSource::Keyboard);
    let HoverBody::Ready(Some(document)) = &popup.body else {
        panic!("hover was not applied: {:?}", popup.body);
    };
    assert!(
        matches!(&document.blocks[0], HoverBlock::Code { language, text } if language == "rust" && text == "fn name()")
    );
    assert!(app.intel.hover_visible());
    app.update_hover();
    assert!(app.intel.hover_visible());
    // The explicit hover closes when the cursor moves.
    cursor(&mut app, 11);
    app.update_hover();
    assert!(app.intel.hover.is_none());
}

#[test]
fn only_one_hover_request_is_in_flight_and_the_newest_target_follows() {
    let fixture = Fixture::new("in-flight");
    let (mut app, _) = fixture.app("alpha beta gamma\n");
    let (events, requests) = connect(&mut app);
    cursor(&mut app, 1);
    app.show_hover();
    let (first, _) = hover_request(&requests);
    cursor(&mut app, 12);
    app.show_hover();
    app.update_hover();
    assert!(requests.next_request().is_none(), "no request storm");
    assert_eq!(app.intel.hover.as_ref().unwrap().body, HoverBody::Pending);

    events
        .send(LspEvent::Hover {
            request_id: first,
            contents: markdown("alpha"),
        })
        .unwrap();
    app.poll_languages();
    assert_eq!(app.intel.hover.as_ref().unwrap().body, HoverBody::Pending);
    app.update_hover();
    let (second, position) = hover_request(&requests);
    assert_ne!(first, second);
    assert_eq!(
        position,
        Position {
            line: 0,
            character: 12
        }
    );
}

#[test]
fn resting_pointer_requests_hover_once_and_diagnostics_come_first() {
    let fixture = Fixture::new("pointer");
    let (mut app, path) = fixture.app("fn main() { value }\n");
    let (events, requests) = connect(&mut app);
    app.projects[0].diagnostics.insert(
        path,
        vec![Diagnostic {
            range: TextRange {
                start: Position {
                    line: 0,
                    character: 12,
                },
                end: Position {
                    line: 0,
                    character: 17,
                },
            },
            severity: 1,
            message: "cannot find value".into(),
            source: None,
        }],
    );
    let (tab_id, revision) = app.tab().map(|tab| (tab.id, tab.revision)).unwrap();
    let target = |x: f32| PointerTarget {
        tab_id,
        pane_id: 0,
        revision,
        index: 13,
        word: 12..17,
        rect: egui::Rect::from_min_size(egui::pos2(90.0, 40.0), egui::vec2(40.0, 18.0)),
        pointer: egui::pos2(x, 48.0),
    };
    let frame = |app: &mut VEditApp, x: f32| {
        app.intel.interactive = true;
        app.intel.pointer = Some(target(x));
        app.update_hover();
    };
    frame(&mut app, 100.0);
    assert!(app.intel.hover.is_none());
    app.intel.age_pointer_rest(language_intel::HOVER_DELAY);
    // Moving within the word before the delay cancels the pending hover.
    frame(&mut app, 104.0);
    assert!(app.intel.hover.is_none());
    assert!(requests.next_request().is_none());
    app.intel.age_pointer_rest(language_intel::HOVER_DELAY);
    frame(&mut app, 104.0);
    let (id, position) = hover_request(&requests);
    assert_eq!(
        position,
        Position {
            line: 0,
            character: 13
        }
    );
    let popup = app.intel.hover.as_ref().unwrap();
    assert_eq!(popup.target.source, HoverSource::Pointer);
    assert_eq!(popup.diagnostics, vec![(1, "cannot find value".to_owned())]);
    assert!(app.intel.hover_visible(), "diagnostics show while loading");

    events
        .send(LspEvent::Hover {
            request_id: id,
            contents: markdown("`value: i32`"),
        })
        .unwrap();
    app.poll_languages();
    assert!(matches!(
        app.intel.hover.as_ref().unwrap().body,
        HoverBody::Ready(Some(_))
    ));
    // Staying on the word neither closes nor repeats the request.
    frame(&mut app, 110.0);
    assert!(app.intel.hover.is_some());
    assert!(requests.next_request().is_none());
    // Leaving the editor without reaching the popup closes it.
    app.intel.interactive = true;
    app.update_hover();
    assert!(app.intel.hover.is_none());
}

#[test]
fn hover_markdown_keeps_code_inline_styles_lists_and_plain_text() {
    let document = hover_document(&HoverContents {
        markdown: true,
        text: "# Title\n\nUse **bold** and `code`\nnext.\n\n- one\n- two `x`\n\n---\n\n```python\nprint(1)\n```\n\n---".into(),
    })
    .unwrap();
    let blocks = &document.blocks;
    assert!(matches!(&blocks[0], HoverBlock::Text { heading: true, .. }));
    let HoverBlock::Text {
        spans,
        heading: false,
        bullet: None,
    } = &blocks[1]
    else {
        panic!("{blocks:?}");
    };
    let text: String = spans.iter().map(|span| span.text.as_str()).collect();
    assert_eq!(text, "Use bold and code next.");
    assert!(spans.iter().any(|span| span.strong && span.text == "bold"));
    assert!(spans.iter().any(|span| span.code && span.text == "code"));
    assert!(matches!(
        &blocks[2],
        HoverBlock::Text {
            bullet: Some(0),
            ..
        }
    ));
    assert!(matches!(
        &blocks[3],
        HoverBlock::Text {
            bullet: Some(0),
            ..
        }
    ));
    assert!(matches!(&blocks[4], HoverBlock::Rule));
    assert!(
        matches!(&blocks[5], HoverBlock::Code { language, text } if language == "python" && text == "print(1)")
    );
    assert_eq!(blocks.len(), 6, "a trailing rule is dropped");

    let plain = hover_document(&HoverContents {
        markdown: false,
        text: "**not markdown**".into(),
    })
    .unwrap();
    assert_eq!(
        plain.blocks,
        vec![HoverBlock::Plain("**not markdown**".into())]
    );
    assert!(
        hover_document(&HoverContents {
            markdown: true,
            text: "  ".into()
        })
        .is_none()
    );
}

#[test]
fn words_at_scalar_indices_handle_emoji_and_cjk() {
    let text = "let 🙂x = 漢字_y;";
    assert_eq!(word_at(text, 5), Some((5..6, "x".into())));
    assert_eq!(word_at(text, 9), Some((9..13, "漢字_y".into())));
    assert_eq!(word_at(text, 12), Some((9..13, "漢字_y".into())));
    assert_eq!(word_at(text, 7), Some((7..8, "=".into())));
    assert_eq!(word_at(text, 6), Some((5..6, "x".into())));
    assert_eq!(word_at(" ", 0), None);
}

#[test]
fn references_group_the_current_file_first_in_source_order() {
    let root = std::env::temp_dir().join("vedit-reference-groups");
    let current = root.join("src/main.rs");
    let groups = group_references(
        vec![
            reference(&root.join("src/z.rs"), 1, 0),
            reference(&current, 9, 2),
            reference(&root.join("src/A.rs"), 4, 0),
            reference(&current, 2, 8),
            reference(&current, 2, 1),
            reference(&root.join("src/z.rs"), 0, 3),
        ],
        &current,
        &root,
    );
    let order: Vec<_> = groups.iter().map(|group| group.label.as_str()).collect();
    assert_eq!(
        order,
        [
            crate::paths::display(Path::new("src/main.rs")),
            crate::paths::display(Path::new("src/A.rs")),
            crate::paths::display(Path::new("src/z.rs")),
        ]
    );
    let lines: Vec<_> = groups[0]
        .items
        .iter()
        .map(|item| (item.position.line, item.position.character))
        .collect();
    assert_eq!(lines, [(2, 1), (2, 8), (9, 2)]);
    assert_eq!(groups[2].items[0].position.line, 0);
}

#[test]
fn find_references_lists_results_and_opens_a_location_with_reveal() {
    let fixture = Fixture::new("references");
    let (mut app, path) = fixture.app("fn name() {}\nfn run() { name(); }\n");
    let (events, requests) = connect(&mut app);
    cursor(&mut app, 4);
    app.find_references();
    let id = match requests.next_request() {
        Some(TestRequest::References { id, position }) => {
            assert_eq!(
                position,
                Position {
                    line: 0,
                    character: 4
                }
            );
            id
        }
        other => panic!("expected references, got {other:?}"),
    };
    let view = app.intel.references.as_ref().unwrap();
    assert_eq!(view.symbol, "name");
    assert_eq!(view.state, ReferencesState::Loading);
    assert!(app.sidebar_view_visible(SidebarView::Search));

    let other = fixture.0.join("other.rs");
    events
        .send(LspEvent::References {
            request_id: id,
            references: vec![
                reference(&other, 1, 13),
                reference(&path, 1, 11),
                reference(&path, 0, 3),
            ],
            truncated: false,
        })
        .unwrap();
    app.poll_languages();
    let view = app.intel.references.as_ref().unwrap();
    let ReferencesState::Loaded { groups, truncated } = &view.state else {
        panic!("references were not applied");
    };
    assert!(!truncated);
    assert_eq!(groups[0].path, path);
    assert_eq!(groups[1].path, other);
    assert!(view.focus_selected, "the list takes focus at the origin");
    assert_eq!(view.locations().len(), 3);
    assert!(app.status.is_none());

    // 🙂 is two UTF-16 units: character 13 is scalar 12 on that line.
    let target = view.locations()[2].clone();
    app.open_language_location(target.path.clone(), target.position);
    wait_loaded(&mut app);
    app.poll_languages();
    let tab = app.tab().unwrap();
    assert_eq!(tab.path.as_ref(), Some(&other));
    let expected = "fn other() {\n    let 🙂 = ".chars().count();
    assert_eq!(tab.vim.cursor, expected);
    assert!(tab.pending_agent_location.is_none());

    // The jump is in the navigation history, so Back returns to the origin.
    app.navigate_history(true);
    let tab = app.tab().unwrap();
    assert_eq!(tab.path.as_ref(), Some(&path));
    assert_eq!(tab.vim.cursor, 4);
}

#[test]
fn references_list_moves_with_arrows_and_opens_with_enter() {
    let fixture = Fixture::new("references-keys");
    let (mut app, path) = fixture.app("fn name() {}\nname();\nname();\n");
    let (events, requests) = connect(&mut app);
    cursor(&mut app, 4);
    app.find_references();
    let Some(TestRequest::References { id, .. }) = requests.next_request() else {
        panic!("expected references");
    };
    events
        .send(LspEvent::References {
            request_id: id,
            references: vec![
                reference(&path, 0, 3),
                reference(&path, 1, 0),
                reference(&path, 2, 0),
            ],
            truncated: false,
        })
        .unwrap();
    app.poll_languages();
    let mut screen = Harness::builder().with_size([420.0, 700.0]).build_ui_state(
        |ui, app: &mut VEditApp| {
            app.ctx = ui.ctx().clone();
            app.poll_languages();
            egui::CentralPanel::default().show(ui, |ui| app.project_search_ui(ui));
        },
        app,
    );
    screen.run_steps(3);
    assert!(screen.ctx.memory(|memory| memory.focused().is_some()));
    screen.key_press(Key::ArrowDown);
    screen.run_steps(2);
    screen.key_press(Key::ArrowDown);
    screen.run_steps(2);
    assert_eq!(
        screen.state().intel.references.as_ref().unwrap().selected,
        2
    );
    screen.key_press(Key::ArrowUp);
    screen.run_steps(2);
    screen.key_press(Key::Enter);
    screen.run_steps(2);
    let tab = screen.state().tab().unwrap();
    assert_eq!(
        tab.vim.cursor,
        "fn name() {}\n".chars().count(),
        "Enter reveals the selected reference"
    );
}

#[test]
fn document_symbols_fill_the_at_palette_with_fuzzy_filtering_and_containers() {
    let fixture = Fixture::new("symbols");
    let (mut app, path) = fixture.app("struct Parser;\nimpl Parser {\n    fn parse() {}\n}\n");
    let (events, requests) = connect(&mut app);
    app.run_shortcut_action(keybindings::Action::DocumentSymbol);
    assert!(app.palette == Some(Palette::Files));
    assert_eq!(app.palette_query, "@");
    assert_eq!(symbol_scope("@ pa"), Some((SymbolScope::Document, "pa")));
    assert_eq!(symbol_scope("#pa"), Some((SymbolScope::Workspace, "pa")));
    assert_eq!(symbol_scope("main.rs"), None);

    let loading = app.symbol_palette(SymbolScope::Document, "");
    assert!(loading.loading && loading.rows.is_empty());
    let Some(TestRequest::DocumentSymbols { id }) = requests.next_request() else {
        panic!("expected a document symbol request");
    };
    // Rendering again while loading does not repeat the request.
    app.symbol_palette(SymbolScope::Document, "");
    assert!(requests.next_request().is_none());
    events
        .send(LspEvent::Symbols {
            request_id: id,
            symbols: vec![
                symbol("Parser", 23, None, 0, 0),
                symbol("Parser", 19, None, 0, 1),
                symbol("parse", 6, Some("Parser"), 1, 2),
                symbol("unrelated", 12, None, 0, 3),
            ],
        })
        .unwrap();
    app.poll_languages();

    let all = app.symbol_palette(SymbolScope::Document, "");
    assert_eq!(all.rows.len(), 4);
    assert_eq!(all.rows[2].0, "    parse", "nesting shows as indentation");
    assert_eq!(all.rows[2].1, "method · Parser");
    let filtered = app.symbol_palette(SymbolScope::Document, "prs");
    let names: Vec<_> = filtered.rows.iter().map(|row| row.0.as_str()).collect();
    assert_eq!(names, ["Parser", "Parser", "parse"]);
    assert_eq!(filtered.rows[0].1, "struct");
    assert_eq!(filtered.rows[2].2.as_ref(), Some(&path));
    assert_eq!(
        filtered.rows[2].3,
        Some(Position {
            line: 2,
            character: 4
        })
    );
    let none = app.symbol_palette(SymbolScope::Document, "zzz");
    assert_eq!(none.message.as_deref(), Some("No matching symbols"));
    // Container names also match.
    let symbols = vec![symbol("run", 6, Some("Runner"), 1, 0)];
    assert_eq!(filter_symbols(&symbols, "runner run").len(), 1);

    // An edit refreshes the list once for the new revision.
    let tab = app.tab_mut().unwrap();
    tab.text.push('\n');
    tab.changed();
    let refreshing = app.symbol_palette(SymbolScope::Document, "");
    assert_eq!(
        refreshing.rows.len(),
        4,
        "old symbols stay while refreshing"
    );
    assert!(matches!(
        requests.next_request(),
        Some(TestRequest::DocumentSymbols { .. })
    ));
}

#[test]
fn workspace_symbols_search_per_query_and_show_paths() {
    let fixture = Fixture::new("workspace-symbols");
    let (mut app, _) = fixture.app("fn main() {}\n");
    let (events, requests) = connect(&mut app);
    let empty = app.symbol_palette(SymbolScope::Workspace, "");
    assert!(empty.message.is_some());
    assert!(requests.next_request().is_none());
    app.symbol_palette(SymbolScope::Workspace, "oth");
    let Some(TestRequest::WorkspaceSymbols { id, query }) = requests.next_request() else {
        panic!("expected a workspace symbol request");
    };
    assert_eq!(query, "oth");
    let other = fixture.0.join("other.rs");
    let mut found = symbol("other", 12, None, 0, 0);
    found.path = Some(other.clone());
    events
        .send(LspEvent::Symbols {
            request_id: id,
            symbols: vec![found],
        })
        .unwrap();
    app.poll_languages();
    let rows = app.symbol_palette(SymbolScope::Workspace, "oth").rows;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].1, "function · other.rs");
    assert!(requests.next_request().is_none());
    app.symbol_palette(SymbolScope::Workspace, "othe");
    assert!(matches!(
        requests.next_request(),
        Some(TestRequest::WorkspaceSymbols { query, .. }) if query == "othe"
    ));
}

#[test]
fn missing_capabilities_explain_themselves_without_language_errors() {
    let fixture = Fixture::new("capabilities");
    let (mut app, _) = fixture.app("fn main() {}\n");
    let (_events, requests) = connect(&mut app);
    for feature in [
        Feature::Hover,
        Feature::References,
        Feature::DocumentSymbols,
        Feature::WorkspaceSymbols,
    ] {
        server(&app).set_test_feature(feature, false);
    }
    cursor(&mut app, 3);
    app.find_references();
    assert!(app.intel.references.is_none());
    assert_eq!(
        app.status
            .as_ref()
            .map(|(text, error, _)| (text.as_str(), *error)),
        Some(("Rust analysis does not provide references.", false))
    );
    app.show_hover();
    assert!(app.intel.hover.is_none());
    assert_eq!(
        app.status.as_ref().map(|(text, _, _)| text.as_str()),
        Some("Rust analysis does not provide hover information.")
    );
    let symbols = app.symbol_palette(SymbolScope::Document, "");
    assert_eq!(
        symbols.message.as_deref(),
        Some("Rust analysis does not provide document symbols.")
    );
    let workspace = app.symbol_palette(SymbolScope::Workspace, "x");
    assert!(workspace.message.is_some());
    assert!(requests.next_request().is_none());
    assert!(!app.pointer_hover_available(app.tab().unwrap()));
    assert!(app.projects[0].languages["rust"].error.is_none());
    assert!(app.projects[0].diagnostics.is_empty());
}

#[test]
fn server_errors_stay_in_their_views_and_restarts_cancel_pending_requests() {
    let fixture = Fixture::new("restart");
    let (mut app, _) = fixture.app("fn main() {}\n");
    let (events, requests) = connect(&mut app);
    cursor(&mut app, 3);
    app.find_references();
    let Some(TestRequest::References { id, .. }) = requests.next_request() else {
        panic!("expected references");
    };
    events
        .send(LspEvent::RequestFailed {
            request_id: id,
            error: Some("content modified".into()),
        })
        .unwrap();
    app.poll_languages();
    assert_eq!(
        app.intel.references.as_ref().unwrap().state,
        ReferencesState::Failed("content modified".into())
    );
    assert!(app.projects[0].languages["rust"].error.is_none());

    app.find_references();
    app.show_hover();
    app.symbol_palette(SymbolScope::Document, "");
    assert!(app.intel.hover_in_flight().is_some());
    // A restarted server is a different instance; its predecessor's
    // requests can never answer.
    let (stopped_events, stopped_requests) = connect(&mut app);
    app.poll_languages();
    assert!(matches!(
        &app.intel.references.as_ref().unwrap().state,
        ReferencesState::Failed(message) if message.contains("cancelled")
    ));
    assert_eq!(app.intel.hover_in_flight(), None);
    assert!(matches!(
        &app.intel.document_symbols.as_ref().unwrap().state,
        SymbolState::Failed(message) if message.contains("cancelled")
    ));
    assert!(app.projects[0].languages["rust"].error.is_none());

    // A result already queued when the server exits must not defeat the same
    // cancellation check during the next poll.
    app.find_references();
    let Some(TestRequest::References { id, .. }) = stopped_requests.next_request() else {
        panic!("expected references");
    };
    stopped_events
        .send(LspEvent::References {
            request_id: id,
            references: vec![reference(&fixture.0.join("source.rs"), 0, 0)],
            truncated: false,
        })
        .unwrap();
    server(&app).shutdown();
    app.poll_languages();
    assert!(matches!(
        &app.intel.references.as_ref().unwrap().state,
        ReferencesState::Failed(message) if message.contains("cancelled")
    ));
}

#[test]
fn vim_k_and_gr_request_hover_and_references_and_esc_closes_the_hover() {
    let fixture = Fixture::new("vim");
    let (mut app, _) = fixture.app("fn name() {}\nname();\n");
    let (events, requests) = connect(&mut app);
    app.settings.vim = true;
    app.tab_mut().unwrap().set_cursor(4..4);
    let mut screen = Harness::builder()
        .with_size([1000.0, 700.0])
        .build_ui_state(
            |ui, app: &mut VEditApp| {
                app.ctx = ui.ctx().clone();
                app.poll_languages();
                design::begin_hover_frame(&app.ctx);
                app.hover_escape();
                app.shortcuts();
                app.handle_vim();
                app.intel.begin_frame();
                app.editor_ui(ui);
                app.overlays();
                app.hover_ui();
                design::end_hover_frame(&app.ctx);
            },
            app,
        );
    screen.run_steps(3);
    screen.event(egui::Event::Text("K".into()));
    screen.run_steps(2);
    let (id, position) = hover_request(&requests);
    assert_eq!(
        position,
        Position {
            line: 0,
            character: 4
        }
    );
    events
        .send(LspEvent::Hover {
            request_id: id,
            contents: markdown("`fn name()`"),
        })
        .unwrap();
    screen.run_steps(2);
    assert!(screen.state().intel.hover_visible());
    assert!(
        screen.state().intel.hover_drawn(),
        "the popup is drawn at the caret"
    );
    let text = screen.state().tab().unwrap().text.clone();
    screen.get_by_label("fn name()").click();
    screen.run_steps(2);
    screen.event(egui::Event::Text("dd".into()));
    screen.event(egui::Event::Paste("do not replace the source".into()));
    screen.run_steps(2);
    assert_eq!(screen.state().tab().unwrap().text, text);
    assert!(screen.state().intel.hover_visible());
    screen.key_press(Key::Escape);
    screen.run_steps(2);
    assert!(screen.state().intel.hover.is_none());
    assert_eq!(screen.state().tab().unwrap().text, text);
    assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::Normal);

    screen.event(egui::Event::Text("gr".into()));
    screen.run_steps(2);
    assert!(matches!(
        requests.next_request(),
        Some(TestRequest::References { .. })
    ));
    assert!(screen.state().intel.references.is_some());
    assert_eq!(screen.state().tab().unwrap().text, text, "gr does not edit");
}

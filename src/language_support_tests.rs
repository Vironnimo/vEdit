use super::*;
use crate::lsp::{Diagnostic, LanguageServer, LspEvent, TextRange};

fn rust_app() -> VEditApp {
    let mut app = VEditApp::empty(egui::Context::default());
    let mut project = Project::new(PathBuf::from("C:/vedit-language-tests"));
    project.rust_project = true;
    project.indexing = false;
    let mut tab = EditorTab::new(Some(project.root.join("main.rs")), "main.rs".into());
    tab.text = "fn main() { unsaved(); }".into();
    tab.dirty = true;
    project.tabs.push(tab);
    app.projects.push(project);
    app
}

fn diagnostic() -> Diagnostic {
    Diagnostic {
        range: TextRange::default(),
        severity: 1,
        message: "stale diagnostic".into(),
        source: Some("rust-analyzer".into()),
    }
}

#[test]
fn restarting_analysis_clears_stale_state_and_keeps_unsaved_documents() {
    let mut app = rust_app();
    let (server, events) = LanguageServer::test_channel();
    let path = app.projects[0].tabs[0].path.clone().unwrap();
    let text = app.projects[0].tabs[0].text.clone();
    let id = app.projects[0].tabs[0].id;
    app.projects[0].rust_language_mut().server = Some(server);
    app.projects[0].rust_language_mut().ready = true;
    app.projects[0].rust_language_mut().error = Some("server died".into());
    app.projects[0]
        .rust_language_mut()
        .opened
        .insert(path.clone(), (3, 9));
    app.projects[0].diagnostics.insert(path, vec![diagnostic()]);
    app.completion_request = Some(7);
    app.definition_request = Some((8, id));
    app.completions = Some(CompletionPopup {
        tab_id: id,
        version: 9,
        cursor: 0,
        items: Vec::new(),
        selected: 0,
    });
    events.send(LspEvent::Ready).unwrap();

    app.command_action("Restart Rust analysis");
    app.poll_languages();

    let project = &app.projects[0];
    assert!(project.rust_language().server.is_none());
    assert!(!project.rust_language().ready);
    assert!(project.rust_language().error.is_none());
    assert!(project.rust_language().opened.is_empty());
    assert!(project.diagnostics.is_empty());
    assert!(app.completions.is_none());
    assert!(app.completion_request.is_none());
    assert!(app.definition_request.is_none());
    assert_eq!(project.tabs[0].text, text);
    assert!(project.tabs[0].dirty);
    assert_eq!(app.rust_analysis_status().unwrap().label, "Rust: loading");
    assert!(
        events.send(LspEvent::Ready).is_err(),
        "old events must not reach a restarted session"
    );
}

#[test]
fn stopped_server_is_visible_and_waits_for_explicit_restart() {
    let mut app = rust_app();
    let (server, _events) = LanguageServer::test_channel();
    server.shutdown();
    let path = app.projects[0].tabs[0].path.clone().unwrap();
    app.projects[0].rust_language_mut().server = Some(server);
    app.projects[0].rust_language_mut().ready = true;
    app.projects[0].diagnostics.insert(path, vec![diagnostic()]);

    for _ in 0..3 {
        app.sync_languages(false);
        assert!(
            app.projects[0]
                .rust_language_mut()
                .server
                .as_ref()
                .unwrap()
                .is_stopped()
        );
        assert!(!app.projects[0].rust_language_mut().ready);
        assert!(app.projects[0].diagnostics.is_empty());
    }
    let status = app.rust_analysis_status().unwrap();
    assert_eq!(status.label, "Rust: unavailable");
    assert!(status.hint.contains("Restart language analysis"));
}

#[test]
fn analysis_error_is_visible_even_without_a_server_handle() {
    let mut app = rust_app();
    app.projects[0].rust_language_mut().error = Some("rust-analyzer could not start".into());
    let status = app.rust_analysis_status().unwrap();
    assert_eq!(status.label, "Rust: unavailable");
    assert!(status.hint.contains("could not start"));
}

#[test]
fn unsupported_project_has_actionable_feedback_without_starting_analysis() {
    let mut app = rust_app();
    app.projects[0].rust_project = false;
    app.command_action("Restart Rust analysis");
    assert!(app.projects[0].rust_language_mut().server.is_none());
    assert!(app.status.as_ref().unwrap().0.contains("Cargo.toml"));
    let status = app.rust_analysis_status().unwrap();
    assert_eq!(status.label, "Rust: unavailable");
    assert!(status.hint.contains("project folder"));
}

#[test]
fn successful_diagnostics_clear_a_transient_analysis_error() {
    let mut app = rust_app();
    let (server, events) = LanguageServer::test_channel();
    app.projects[0].rust_language_mut().server = Some(server);
    app.projects[0].rust_language_mut().ready = true;
    let path = app.projects[0].tabs[0].path.clone().unwrap();
    events
        .send(LspEvent::Error("Rust analysis request timed out".into()))
        .unwrap();
    app.poll_languages();
    assert_eq!(app.rust_analysis_status().unwrap().label, "Rust: warning");
    events
        .send(LspEvent::Diagnostics {
            path: path.clone(),
            version: None,
            diagnostics: vec![diagnostic()],
        })
        .unwrap();
    app.poll_languages();
    assert!(app.projects[0].rust_language_mut().error.is_none());
    assert_eq!(app.rust_analysis_status().unwrap().label, "Rust: ready");
    assert!(
        app.status.is_none(),
        "the recovered error must not remain in the status bar"
    );
    assert_eq!(app.projects[0].diagnostics[&path].len(), 1);
    assert!(
        app.projects[0]
            .rust_language_mut()
            .server
            .as_ref()
            .is_some_and(|server| !server.is_stopped())
    );
}

#[test]
fn successful_completions_clear_a_timeout_even_when_no_items_match() {
    for empty in [false, true] {
        let mut app = rust_app();
        let (server, events) = LanguageServer::test_channel();
        app.projects[0].rust_language_mut().server = Some(server);
        app.projects[0].rust_language_mut().ready = true;
        let tab = &app.projects[0].tabs[0];
        let path = tab.path.clone().unwrap();
        let position = crate::lsp::char_to_position(&tab.text, tab.vim.cursor);
        let revision = tab.revision;
        app.projects[0]
            .rust_language_mut()
            .opened
            .insert(path.clone(), (revision, 1));
        app.completion_request = Some(42);
        events
            .send(LspEvent::Error("Completion request timed out".into()))
            .unwrap();
        app.poll_languages();
        assert_eq!(app.rust_analysis_status().unwrap().label, "Rust: warning");
        app.status("Unrelated file save error", true);
        events
            .send(LspEvent::Completions {
                request_id: 42,
                path,
                version: 1,
                position,
                items: if empty {
                    Vec::new()
                } else {
                    vec![crate::lsp::CompletionItem {
                        label: "main".into(),
                        detail: None,
                        insert_text: "main".into(),
                        text_edit: None,
                        additional_text_edits: Vec::new(),
                    }]
                },
            })
            .unwrap();
        app.poll_languages();
        assert_eq!(app.rust_analysis_status().unwrap().label, "Rust: ready");
        assert!(app.projects[0].rust_language_mut().error.is_none());
        assert_eq!(app.completions.is_none(), empty);
        assert_eq!(app.status.as_ref().unwrap().0, "Unrelated file save error");
        assert!(
            app.projects[0]
                .rust_language_mut()
                .server
                .as_ref()
                .is_some_and(|server| !server.is_stopped())
        );
    }
}

#[test]
fn late_success_does_not_hide_a_stopped_server_error() {
    let mut app = rust_app();
    let (server, events) = LanguageServer::test_channel();
    server.shutdown();
    app.projects[0].rust_language_mut().server = Some(server);
    app.projects[0].rust_language_mut().ready = true;
    events
        .send(LspEvent::Error("Rust analysis process exited".into()))
        .unwrap();
    app.poll_languages();
    events
        .send(LspEvent::Diagnostics {
            path: app.projects[0].tabs[0].path.clone().unwrap(),
            version: None,
            diagnostics: Vec::new(),
        })
        .unwrap();
    app.poll_languages();
    assert_eq!(
        app.rust_analysis_status().unwrap().label,
        "Rust: unavailable"
    );
    assert_eq!(
        app.projects[0].rust_language_mut().error.as_deref(),
        Some("Rust analysis process exited")
    );
    assert!(!app.projects[0].rust_language_mut().ready);
    assert!(
        app.projects[0]
            .rust_language_mut()
            .server
            .as_ref()
            .unwrap()
            .is_stopped()
    );
}

#[test]
fn failed_rpc_request_remains_visible_until_a_real_success() {
    for completion in [false, true] {
        let mut app = rust_app();
        let (server, events) = LanguageServer::test_channel();
        app.projects[0].rust_language_mut().server = Some(server);
        app.projects[0].rust_language_mut().ready = true;
        let tab = &app.projects[0].tabs[0];
        let path = tab.path.clone().unwrap();
        let id = tab.id;
        let position = crate::lsp::char_to_position(&tab.text, tab.vim.cursor);
        let revision = tab.revision;
        app.projects[0]
            .rust_language_mut()
            .opened
            .insert(path.clone(), (revision, 1));
        if completion {
            app.completion_request = Some(41);
        } else {
            app.definition_request = Some((41, id));
            app.status("Finding definition…", false);
        }
        events
            .send(LspEvent::RequestFailed {
                request_id: 41,
                error: Some("Rust analysis: temporary server failure".into()),
            })
            .unwrap();
        app.poll_languages();
        assert_eq!(app.rust_analysis_status().unwrap().label, "Rust: warning");
        assert!(app.completion_request.is_none());
        assert!(app.definition_request.is_none());
        assert!(app.completions.is_none());
        assert_eq!(
            app.status.as_ref().unwrap().0,
            "Rust analysis: temporary server failure"
        );

        if completion {
            app.completion_request = Some(42);
            events
                .send(LspEvent::Completions {
                    request_id: 42,
                    path,
                    version: 1,
                    position,
                    items: Vec::new(),
                })
                .unwrap();
        } else {
            app.definition_request = Some((42, id));
            events
                .send(LspEvent::DefinitionNotFound { request_id: 42 })
                .unwrap();
        }
        app.poll_languages();
        assert_eq!(app.rust_analysis_status().unwrap().label, "Rust: ready");
        assert!(app.projects[0].rust_language_mut().error.is_none());
        assert!(
            app.projects[0]
                .rust_language_mut()
                .server
                .as_ref()
                .is_some_and(|server| !server.is_stopped())
        );
    }
}

#[test]
fn cancelled_request_clears_pending_state_without_reporting_success() {
    let mut app = rust_app();
    let (server, events) = LanguageServer::test_channel();
    app.projects[0].rust_language_mut().server = Some(server);
    app.projects[0].rust_language_mut().ready = true;
    app.projects[0].rust_language_mut().error = Some("Previous request failed".into());
    app.definition_request = Some((51, app.projects[0].tabs[0].id));
    app.status("Finding definition…", false);
    events
        .send(LspEvent::RequestFailed {
            request_id: 51,
            error: None,
        })
        .unwrap();
    app.poll_languages();
    assert!(app.definition_request.is_none());
    assert!(app.status.is_none());
    assert_eq!(
        app.projects[0].rust_language_mut().error.as_deref(),
        Some("Previous request failed")
    );
    assert_eq!(app.rust_analysis_status().unwrap().label, "Rust: warning");
}

#[test]
fn mixed_language_sessions_keep_diagnostics_and_restart_independent() {
    let mut app = rust_app();
    let root = app.projects[0].root.clone();
    let mut python = EditorTab::new(Some(root.join("main.py")), "main.py".into());
    python.text = "unsaved_name\n".into();
    python.dirty = true;
    let python_path = python.path.clone().unwrap();
    app.projects[0].tabs.push(python);
    let mut dart = EditorTab::new(Some(root.join("main.dart")), "main.dart".into());
    dart.text = "void main() { missing(); }".into();
    dart.dirty = true;
    let dart_path = dart.path.clone().unwrap();
    app.projects[0].tabs.push(dart);
    let (rust_server, rust_events) = LanguageServer::test_channel();
    app.projects[0].rust_language_mut().server = Some(rust_server);
    app.projects[0].rust_language_mut().ready = true;
    let (python_server, python_events) = LanguageServer::test_channel();
    let (dart_server, dart_events) = LanguageServer::test_channel();
    for (id, server) in [("python", python_server), ("dart", dart_server)] {
        let config = language_settings::defaults()
            .into_iter()
            .find(|config| config.language_id == id)
            .unwrap();
        let mut session = LanguageSession::new(config);
        session.server = Some(server);
        session.ready = true;
        app.projects[0].languages.insert(id.into(), session);
    }
    let rust_path = app.projects[0].tabs[0].path.clone().unwrap();
    for (events, path) in [
        (&rust_events, &rust_path),
        (&python_events, &python_path),
        (&dart_events, &dart_path),
    ] {
        events
            .send(LspEvent::Diagnostics {
                path: path.clone(),
                version: None,
                diagnostics: vec![diagnostic()],
            })
            .unwrap();
    }
    app.poll_languages();
    assert_eq!(app.projects[0].diagnostics.len(), 3);
    // A server cannot overwrite another language's diagnostics.
    python_events
        .send(LspEvent::Diagnostics {
            path: rust_path.clone(),
            version: None,
            diagnostics: vec![],
        })
        .unwrap();
    app.poll_languages();
    assert_eq!(app.projects[0].diagnostics[&rust_path].len(), 1);
    app.projects[0].active_tab = 1;
    app.restart_active_language();
    assert!(app.projects[0].languages["python"].server.is_none());
    assert!(app.projects[0].languages["dart"].active());
    assert!(app.projects[0].languages["rust"].active());
    assert!(!app.projects[0].diagnostics.contains_key(&python_path));
    assert!(app.projects[0].diagnostics.contains_key(&dart_path));
    assert_eq!(app.projects[0].tabs[1].text, "unsaved_name\n");
    assert!(app.projects[0].tabs[1].dirty);
    assert!(python_events.send(LspEvent::Ready).is_err());
}

#[test]
fn definition_not_found_from_another_project_cannot_clear_pending_navigation() {
    let mut app = rust_app();
    let mut other = Project::new(PathBuf::from("C:/vedit-other-language-tests"));
    let (server, events) = LanguageServer::test_channel();
    other.rust_language_mut().server = Some(server);
    other.rust_language_mut().ready = true;
    app.projects.push(other);
    let tab = app.projects[0].tabs[0].id;
    app.definition_request = Some((999, tab));
    events
        .send(LspEvent::DefinitionNotFound { request_id: 999 })
        .unwrap();
    app.poll_languages();
    assert_eq!(app.definition_request, Some((999, tab)));
}

#[test]
fn python_and_dart_selection_does_not_require_a_cargo_manifest() {
    let mut app = rust_app();
    app.projects[0].rust_project = false;
    for (file, language) in [("scratch.PY", "Python"), ("scratch.dart", "Dart")] {
        app.projects[0].tabs[0].path = Some(app.projects[0].root.join(file));
        let status = app.rust_analysis_status().unwrap();
        assert_eq!(status.label, format!("{language}: loading"));
        assert!(!status.hint.contains("Cargo"));
    }
}

#[test]
fn disabled_language_is_visible_and_does_not_spawn_a_server() {
    let mut app = rust_app();
    app.projects[0].rust_project = false;
    app.projects[0].tabs[0].path = Some(app.projects[0].root.join("test.py"));
    app.settings
        .language_servers
        .iter_mut()
        .find(|config| config.language_id == "python")
        .unwrap()
        .enabled = false;
    app.sync_languages(false);
    assert!(!app.projects[0].languages.contains_key("python"));
    assert_eq!(
        app.rust_analysis_status().unwrap().label,
        "Python: disabled"
    );
}

#[test]
fn custom_enabled_owner_wins_over_disabled_builtin_in_status_and_restart() {
    let mut app = rust_app();
    app.projects[0].rust_project = false;
    app.projects[0].tabs[0].path = Some(app.projects[0].root.join("example.py"));
    app.settings
        .language_servers
        .iter_mut()
        .find(|config| config.language_id == "python")
        .unwrap()
        .enabled = false;
    let mut custom = language_settings::defaults().remove(1);
    custom.language_id = "custom-python".into();
    custom.name = "Custom Python".into();
    custom.program = "custom-lsp".into();
    app.settings.language_servers.push(custom);
    assert_eq!(
        app.rust_analysis_status().unwrap().label,
        "Custom Python: loading"
    );
    app.restart_active_language();
    assert!(app.projects[0].languages.contains_key("custom-python"));
    assert!(!app.status.as_ref().unwrap().0.contains("disabled"));
}

#[test]
fn support_file_diagnostics_are_preserved_and_owned_by_their_session() {
    let mut app = rust_app();
    let root = app.projects[0].root.clone();
    let config_path = root.join("pyproject.toml");
    let (rust_server, rust_events) = LanguageServer::test_channel();
    app.projects[0].rust_language_mut().server = Some(rust_server);
    app.projects[0].rust_language_mut().ready = true;
    let (python_server, python_events) = LanguageServer::test_channel();
    let mut python = LanguageSession::new(language_settings::defaults().remove(1));
    python.server = Some(python_server);
    python.ready = true;
    app.projects[0].languages.insert("python".into(), python);
    for events in [&rust_events, &python_events] {
        events
            .send(LspEvent::Diagnostics {
                path: config_path.clone(),
                version: None,
                diagnostics: vec![diagnostic()],
            })
            .unwrap();
    }
    app.poll_languages();
    assert_eq!(app.projects[0].diagnostics[&config_path].len(), 2);
    app.restart_project_language(0, "python");
    assert_eq!(app.projects[0].diagnostics[&config_path].len(), 1);
    assert!(app.projects[0].languages["rust"].active());
}

#[test]
fn late_definition_does_not_navigate_after_editing_or_moving_the_cursor() {
    for edit in [false, true] {
        let mut app = rust_app();
        let (server, events) = LanguageServer::test_channel();
        app.projects[0].rust_language_mut().server = Some(server);
        app.projects[0].rust_language_mut().ready = true;
        let tab = app.tab().unwrap();
        app.definition_origin = Some((
            800,
            tab.revision,
            tab.vim.cursor,
            app.document_widget_id(tab.id),
        ));
        app.definition_request = Some((800, app.tab().unwrap().id));
        app.status("Finding definition…", false);
        if edit {
            app.projects[0].tabs[0].text.push('x');
            app.projects[0].tabs[0].changed();
        } else {
            app.projects[0].tabs[0].set_cursor(1..1);
        }
        events
            .send(LspEvent::Definition {
                request_id: 800,
                path: app.projects[0].root.join("different.rs"),
                line: 0,
                column: 0,
            })
            .unwrap();
        app.poll_languages();
        assert_eq!(app.projects[0].tabs.len(), 1);
        assert!(app.definition_request.is_none());
        assert!(app.pending_location.is_none());
        assert!(app.status.is_none());
    }
}

#[test]
fn renamed_paths_do_not_reappear_from_cached_or_late_diagnostics() {
    let mut app = rust_app();
    let root = app.projects[0].root.clone();
    let old = root.join("old/pyproject.toml");
    let (server, events) = LanguageServer::test_channel();
    let mut session = LanguageSession::new(language_settings::defaults().remove(1));
    session.server = Some(server);
    session.ready = true;
    app.projects[0].languages.insert("python".into(), session);
    events
        .send(LspEvent::Diagnostics {
            path: old.clone(),
            version: None,
            diagnostics: vec![diagnostic()],
        })
        .unwrap();
    app.poll_languages();
    assert!(app.projects[0].diagnostics.contains_key(&old));
    app.projects[0].close_language_paths(&root.join("old"));
    app.projects[0].clear_language_diagnostics("rust");
    assert!(!app.projects[0].diagnostics.contains_key(&old));
    events
        .send(LspEvent::Diagnostics {
            path: old.clone(),
            version: None,
            diagnostics: vec![diagnostic()],
        })
        .unwrap();
    app.poll_languages();
    assert!(!app.projects[0].diagnostics.contains_key(&old));
    assert!(
        !app.projects[0].languages["python"]
            .diagnostics
            .contains_key(&old)
    );
}

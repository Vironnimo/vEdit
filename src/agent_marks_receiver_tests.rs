//! Regressions at the production GUI-thread control boundary. All persistence
//! stays inside process-owned fixture directories, never the user's session.
use super::*;
use crate::cli::{
    AgentMarksAction, Annotation, AnnotationSeverity, EditorCommand, LayoutAction, Location,
    Request, Response, SplitOrientation,
};
use serde_json::{Value, json};

struct Fixture {
    app: VEditApp,
    root: PathBuf,
    first: u64,
    second: u64,
    other: u64,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-state/agent-marks-receiver")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(&root).unwrap();
        let mut app = VEditApp::empty(egui::Context::default());
        app.settings.autosave = false;
        app.recovery_session_path = root.join("recovery/session.json");
        let mut first_project = Project::new(root.join("one"));
        let mut other_project = Project::new(root.join("two"));
        for project in [&mut first_project, &mut other_project] {
            fs::create_dir_all(&project.root).unwrap();
            project.index = Some(Arc::new(workspace::WorkspaceIndex::empty(
                project.root.clone(),
            )));
            project.last_git_refresh = Instant::now();
        }
        let first = fixture_tab(
            &root,
            &first_project.root,
            "a.md",
            "first block\nsecond block\n",
        );
        let second = fixture_tab(&root, &first_project.root, "b.rs", "fn important() {}\n");
        let other = fixture_tab(&root, &other_project.root, "c.md", "another project\n");
        let ids = (first.id, second.id, other.id);
        first_project.tabs.extend([first, second]);
        other_project.tabs.push(other);
        app.projects.extend([first_project, other_project]);
        Self {
            app,
            root,
            first: ids.0,
            second: ids.1,
            other: ids.2,
        }
    }

    fn capture(&mut self, tab_id: u64, range: Range<usize>, note: &str) -> u64 {
        let vim = self.app.settings.vim;
        let tab = self.app.tab_by_id_mut(tab_id).unwrap();
        tab.set_cursor(range.clone());
        if vim {
            tab.vim.set_selection(&tab.text, range);
        }
        let before = editor_state(&self.app);
        let response = request(
            &mut self.app,
            AgentMarksAction::Add {
                tab_id: Some(tab_id),
                note: Some(note.into()),
            },
        );
        assert!(response.ok, "{response:?}");
        assert_eq!(
            editor_state(&self.app),
            before,
            "Capturing a mark must not edit or navigate"
        );
        response.result["id"].as_u64().unwrap()
    }

    fn take(&mut self, request_id: &str, limit: usize) -> Response {
        request(
            &mut self.app,
            AgentMarksAction::Take {
                request_id: Some(request_id.into()),
                limit,
            },
        )
    }
}

fn fixture_tab(root: &Path, workspace: &Path, title: &str, text: &str) -> EditorTab {
    let path = workspace.join(title);
    fs::write(&path, text).unwrap();
    let mut tab = EditorTab::new(Some(path), title.into());
    tab.text = text.into();
    tab.saved_text = text.into();
    tab.recovery_path = root.join("recovery").join(format!("{}.txt", tab.id));
    tab.vim.set_register(Yank {
        text: "private vim register".into(),
        block_width: None,
        linewise: false,
    });
    tab
}

fn request(app: &mut VEditApp, action: AgentMarksAction) -> Response {
    app.control_request(Request::AgentMarks { action })
}

fn list(app: &mut VEditApp, limit: usize, offset: usize) -> Value {
    let response = request(app, AgentMarksAction::List { limit, offset });
    assert!(response.ok, "{response:?}");
    response.result
}

/// Compare editor-owned state rather than merely checking the receiver's JSON.
/// Marks and the session write flag are deliberately outside this invariant.
fn editor_state(app: &VEditApp) -> Value {
    let projects: Vec<_> = app
        .projects
        .iter()
        .map(|project| {
            let tabs: Vec<_> = project.tabs.iter().map(|tab| {
            json!({
                "id": tab.id, "path": tab.path, "text": tab.text,
                "saved_text": tab.saved_text, "revision": tab.revision,
                "dirty": tab.dirty, "agent_unsaved": tab.agent_unsaved,
                "selection": tab.selection, "pending_selection": tab.pending_selection,
                "vim": format!("{:?}", tab.vim),
                "native_history": app.context_native_history(tab.id),
                "annotations": tab.annotations, "annotations_revision": tab.annotations_revision,
                "view": tab.agent_view, "focus": tab.focus,
                "scroll_to_cursor": tab.scroll_to_cursor,
                "center_on_reveal": tab.center_on_reveal,
            })
        }).collect();
            json!({"root": project.root, "active_tab": project.active_tab, "tabs": tabs})
        })
        .collect();
    json!({
        "projects": projects, "active_project": app.active_project,
        "layout": app.layout_status(), "register": format!("{:?}", app.register),
        "settings": app.settings, "review": app.review,
        "editor_focused": app.editor_focused, "sidebar_visible": app.sidebar_visible,
        "sidebar": app.sidebar,
    })
}

fn batch_id(response: &Response) -> u64 {
    assert!(response.ok, "{response:?}");
    response.result["batch_id"].as_u64().unwrap()
}

fn mark_ids(app: &VEditApp) -> Vec<u64> {
    app.agent_marks.items.iter().map(|mark| mark.id).collect()
}

fn saved_marks(app: &VEditApp) -> Value {
    serde_json::to_value(&app.agent_marks).unwrap()
}

#[test]
fn marks_across_projects_and_split_views_are_observed_without_touching_editor_state() {
    let mut fixture = Fixture::new();
    let (first, second, other) = (fixture.first, fixture.second, fixture.other);
    let split = fixture.app.control_request(Request::Layout {
        change: LayoutAction::Split {
            orientation: SplitOrientation::Vertical,
            pane_id: None,
            tab_id: Some(second),
            view: None,
        },
    });
    assert!(split.ok, "{split:?}");
    let ids = [
        fixture.capture(first, 0..5, "first concern"),
        fixture.capture(first, 12..18, "another block"),
        fixture.capture(second, 3..12, "implementation"),
        fixture.capture(other, 0..7, "other workspace"),
    ];
    let tab = fixture.app.tab_by_id_mut(first).unwrap();
    tab.annotations.push(Annotation {
        id: "agent-authored-review".into(),
        start: Location { line: 1, column: 1 },
        end: None,
        note: "This annotation belongs to the review API".into(),
        severity: AnnotationSeverity::Warning,
    });
    tab.annotations_revision = Some(tab.revision);
    fixture.app.register = Some(Yank {
        text: "shared vim value".into(),
        block_width: None,
        linewise: true,
    });
    // An unrelated insert transaction must survive observing or handing off marks.
    let tab = fixture.app.tab_by_id_mut(other).unwrap();
    tab.vim.handle_key(&mut tab.text, VimKey::Escape);
    tab.vim.handle_key(&mut tab.text, VimKey::Char('i'));
    let before = editor_state(&fixture.app);
    fixture.app.settings_open = true;
    let page = list(&mut fixture.app, 2, 0);
    assert!(
        fixture.app.settings_open,
        "Read-only polling must not dismiss the user's dialog"
    );
    assert_eq!(page["total"], 4);
    assert_eq!(page["next_offset"], 2);
    assert_eq!(page["items"][0]["text"], "first");
    assert_eq!(page["items"][1]["text"], "second");
    assert_eq!(
        page["items"][1]["range"]["start"],
        json!({"line": 2, "column": 1})
    );
    assert_eq!(
        page["items"][1]["range"]["end"],
        json!({"line": 2, "column": 7})
    );
    assert_eq!(page["items"][0]["tab_id"], first);
    assert_eq!(
        page["items"][0]["path"],
        crate::paths::display(&fixture.root.join("one").join("a.md"))
    );
    assert_eq!(
        page["items"][0]["workspace"],
        crate::paths::display(&fixture.root.join("one"))
    );
    let tail = list(&mut fixture.app, 2, 2);
    assert_eq!(tail["items"][0]["id"], ids[2]);
    assert_eq!(tail["items"][1]["id"], ids[3]);
    assert_eq!(
        tail["items"][1]["workspace"],
        crate::paths::display(&fixture.root.join("two"))
    );
    assert!(tail["next_offset"].is_null());
    assert_eq!(editor_state(&fixture.app), before);
    fixture.app.settings_open = false;
    let taken = fixture.take("cross-project-review", 2);
    assert!(taken.ok, "{taken:?}");
    assert_eq!(taken.result["total"], 4);
    assert!(fixture.app.agent_marks.items.is_empty());
    assert_eq!(editor_state(&fixture.app), before);
    assert_eq!(
        fixture.app.projects[0].tabs[0].annotations[0].id,
        "agent-authored-review"
    );
    for project in &fixture.app.projects {
        for tab in &project.tabs {
            assert_eq!(
                fs::read_to_string(tab.path.as_ref().unwrap()).unwrap(),
                tab.saved_text
            );
        }
    }
}

#[test]
fn take_retries_preserve_new_marks_until_ack_and_expired_receipts_never_consume_again() {
    let mut fixture = Fixture::new();
    let first = fixture.first;
    let old = fixture.capture(first, 0..5, "old");
    let original = fixture.take("agent-turn-1", 1);
    let first_batch = batch_id(&original);
    assert_eq!(original.result["request_id"], "agent-turn-1");
    assert_eq!(original.result["acknowledged"], false);
    let new = fixture.capture(first, 12..18, "created after handoff");
    let replay = fixture.take("agent-turn-1", 1);
    assert_eq!(batch_id(&replay), first_batch);
    assert_eq!(replay.result["replayed"], true);
    assert_eq!(replay.result["items"][0]["id"], old);
    assert_eq!(mark_ids(&fixture.app), [new]);
    let pending = fixture.take("agent-turn-2", 1);
    assert!(!pending.ok);
    assert_eq!(pending.error.unwrap().code, "batch_pending");
    assert_eq!(pending.result["batch_id"], first_batch);
    assert_eq!(mark_ids(&fixture.app), [new]);
    let ack = request(
        &mut fixture.app,
        AgentMarksAction::Ack {
            batch_id: first_batch,
        },
    );
    assert!(ack.ok, "{ack:?}");
    let next = fixture.take("agent-turn-2", 1);
    let next_batch = batch_id(&next);
    assert_ne!(next_batch, first_batch);
    assert_eq!(next.result["items"][0]["id"], new);
    let newest = fixture.capture(first, 0..5, "do not consume on expired retry");
    let before = saved_marks(&fixture.app);
    let expired = fixture.take("agent-turn-1", 1);
    assert!(!expired.ok);
    assert_eq!(expired.error.unwrap().code, "receipt_expired");
    assert_eq!(expired.result["batch_id"], first_batch);
    assert_eq!(saved_marks(&fixture.app), before);
    assert_eq!(mark_ids(&fixture.app), [newest]);
}

#[test]
fn restore_merges_consumed_or_removed_marks_without_duplicates_or_native_history_changes() {
    let mut fixture = Fixture::new();
    fixture.app.settings.vim = false;
    let first = fixture.first;
    let edit = fixture.app.control_request(Request::Edit {
        tab_id: first,
        expected_revision: 0,
        edits: Vec::new(),
        set_text: Some("first block\nsecond block\nextra\n".into()),
    });
    assert!(edit.ok, "{edit:?}");
    assert_eq!(fixture.app.context_native_history(first), (true, false));
    let old = fixture.capture(first, 0..5, "saved block");
    let taken = fixture.take("native-handoff", 1);
    let batch = batch_id(&taken);
    let newer = fixture.capture(first, 12..18, "new block");
    let before = editor_state(&fixture.app);
    let restored = request(
        &mut fixture.app,
        AgentMarksAction::Restore {
            batch_id: Some(batch),
        },
    );
    assert!(restored.ok, "{restored:?}");
    assert_eq!(mark_ids(&fixture.app), [newer, old]);
    assert!(
        request(
            &mut fixture.app,
            AgentMarksAction::Restore {
                batch_id: Some(batch)
            }
        )
        .ok
    );
    assert_eq!(mark_ids(&fixture.app), [newer, old]);
    assert_eq!(editor_state(&fixture.app), before);
    let cleared = request(&mut fixture.app, AgentMarksAction::Clear {});
    assert!(cleared.ok);
    assert!(fixture.app.agent_marks.items.is_empty());
    assert!(
        request(
            &mut fixture.app,
            AgentMarksAction::Restore { batch_id: None }
        )
        .ok
    );
    assert_eq!(mark_ids(&fixture.app), [newer, old]);
    assert_eq!(editor_state(&fixture.app), before);
    // Verify the retained native history actually works, rather than only flags.
    let undo = fixture.app.control_request(Request::Command {
        tab_id: first,
        command: EditorCommand::Undo,
        expected_revision: 1,
    });
    assert!(undo.ok, "{undo:?}");
    assert_eq!(
        fixture.app.tab_by_id_mut(first).unwrap().text,
        "first block\nsecond block\n"
    );
}

#[test]
fn bounded_previews_and_frozen_text_continuations_preserve_unicode_crlf_and_exact_offsets() {
    let mut fixture = Fixture::new();
    let first = fixture.first;
    let source = format!("prefix{}suffix", "🦊é\r\n".repeat(1800));
    let tab = fixture.app.tab_by_id_mut(first).unwrap();
    tab.text = source.clone();
    tab.saved_text = source;
    let selected = "🦊é\r\n".repeat(1800);
    let id = fixture.capture(
        first,
        6..6 + selected.chars().count(),
        "exact Unicode selection",
    );
    let page = list(&mut fixture.app, 1, 0);
    let preview = &page["items"][0];
    assert_eq!(preview["text_chars"], selected.chars().count());
    assert!(preview["text"].as_str().unwrap().chars().count() <= 4096);
    assert_eq!(
        preview["text_next_offset"].as_u64().unwrap() as usize,
        preview["text"].as_str().unwrap().chars().count()
    );
    let frozen = fixture.take("unicode-batch", 1);
    let batch = batch_id(&frozen);
    let mut offset = 0;
    let mut received = String::new();
    loop {
        let response = request(
            &mut fixture.app,
            AgentMarksAction::Text {
                id,
                batch_id: Some(batch),
                offset,
                count: 997,
            },
        );
        assert!(response.ok, "{response:?}");
        assert_eq!(response.result["offset"], offset);
        assert_eq!(response.result["total_chars"], selected.chars().count());
        let text = response.result["text"].as_str().unwrap();
        assert!(text.chars().count() <= 997);
        received.push_str(text);
        let Some(next) = response.result["next_offset"].as_u64() else {
            break;
        };
        assert_eq!(next as usize, offset + text.chars().count());
        offset = next as usize;
    }
    assert_eq!(received.as_bytes(), selected.as_bytes());
    let final_chunk = request(
        &mut fixture.app,
        AgentMarksAction::Text {
            id,
            batch_id: Some(batch),
            offset: selected.chars().count(),
            count: 1,
        },
    );
    assert!(final_chunk.ok);
    assert_eq!(final_chunk.result["text"], "");
    assert!(final_chunk.result["next_offset"].is_null());
    let invalid = request(
        &mut fixture.app,
        AgentMarksAction::Text {
            id,
            batch_id: Some(batch),
            offset: selected.chars().count() + 1,
            count: 1,
        },
    );
    assert!(!invalid.ok);
    assert_eq!(invalid.error.unwrap().code, "invalid_request");
}

#[test]
fn add_requires_a_real_selection_in_vim_and_native_modes_instead_of_marking_the_current_line() {
    for vim in [true, false] {
        let mut fixture = Fixture::new();
        fixture.app.settings.vim = vim;
        let first = fixture.first;
        let tab = fixture.app.tab_by_id_mut(first).unwrap();
        tab.set_cursor(3..3);
        let before = editor_state(&fixture.app);
        let empty = request(
            &mut fixture.app,
            AgentMarksAction::Add {
                tab_id: Some(first),
                note: None,
            },
        );
        assert!(
            !empty.ok,
            "An empty selection must not create a mark in mode {vim}"
        );
        assert!(fixture.app.agent_marks.items.is_empty());
        assert_eq!(editor_state(&fixture.app), before);
        let id = fixture.capture(first, 2..7, "only selected text");
        let marked = list(&mut fixture.app, 1, 0);
        assert_eq!(marked["items"][0]["id"], id);
        assert_eq!(marked["items"][0]["text"], "rst b");
        assert_eq!(
            marked["items"][0]["range"]["start"],
            json!({"line": 1, "column": 3})
        );
        assert_eq!(
            marked["items"][0]["range"]["end"],
            json!({"line": 1, "column": 8})
        );
    }
}

#[test]
fn edited_selection_is_reported_stale_while_its_captured_text_remains_available_after_tab_close() {
    let mut fixture = Fixture::new();
    let first = fixture.first;
    let id = fixture.capture(first, 6..11, "review this original block");
    let edit = fixture.app.control_request(Request::Edit {
        tab_id: first,
        expected_revision: 0,
        edits: Vec::new(),
        set_text: Some("first edited\nsecond block\n".into()),
    });
    assert!(edit.ok, "{edit:?}");
    let page = list(&mut fixture.app, 1, 0);
    assert_eq!(page["items"][0]["id"], id);
    assert_eq!(page["items"][0]["text"], "block");
    assert_eq!(page["items"][0]["stale"], true);
    assert_eq!(page["items"][0]["range_current"], false);
    assert_eq!(page["items"][0]["revision"], 1);
    assert_eq!(fixture.app.agent_marks.visible_marks(first).count(), 0);
    // A closed source remains a frozen user handoff, without reopening it.
    fixture.app.perform_close(CloseAction::Tab(first));
    let closed = list(&mut fixture.app, 1, 0);
    assert_eq!(closed["items"][0]["text"], "block");
    assert_eq!(closed["items"][0]["source_open"], false);
    assert!(closed["items"][0]["tab_id"].is_null());
    assert_eq!(fixture.app.projects[0].tabs.len(), 1);
}

#[test]
fn retained_batch_keeps_captured_text_without_claiming_its_range_matches_the_edited_live_buffer() {
    let mut fixture = Fixture::new();
    let first = fixture.first;
    let captured = fixture.capture(first, 6..11, "Original block for the agent");
    let taken = fixture.take("frozen-range-review", 1);
    let batch = batch_id(&taken);
    assert_eq!(taken.result["items"][0]["text"], "block");
    assert_eq!(
        taken.result["items"][0]["range_current"], false,
        "A retained receipt is a snapshot, even while the source remains unchanged"
    );

    let edited = fixture.app.control_request(Request::Edit {
        tab_id: first,
        expected_revision: 0,
        edits: Vec::new(),
        set_text: Some("first changed\nsecond block\n".into()),
    });
    assert!(edited.ok, "{edited:?}");
    let current = fixture.capture(first, 6..13, "New verified live selection");
    let editor = editor_state(&fixture.app);
    let retained = request(
        &mut fixture.app,
        AgentMarksAction::Batch {
            batch_id: Some(batch),
            limit: 1,
            offset: 0,
        },
    );
    assert!(retained.ok, "{retained:?}");
    let snapshot = &retained.result["items"][0];
    assert_eq!(snapshot["id"], captured);
    assert_eq!(snapshot["text"], "block");
    assert_eq!(
        snapshot["range"],
        json!({
            "start": {"line": 1, "column": 7},
            "end": {"line": 1, "column": 12},
        })
    );
    assert_eq!(snapshot["source_open"], true);
    assert_eq!(snapshot["tab_id"], first);
    assert_eq!(snapshot["revision"], 1);
    assert_eq!(
        snapshot["range_current"], false,
        "An open tab and its current revision do not verify the captured batch range"
    );
    let verified = list(&mut fixture.app, 1, 0);
    assert_eq!(verified["items"][0]["id"], current);
    assert_eq!(verified["items"][0]["text"], "changed");
    assert_eq!(
        verified["items"][0]["range_current"], true,
        "Current collection marks must retain their verified live-range status"
    );
    assert_eq!(editor_state(&fixture.app), editor);
}

#[test]
fn invalid_mutations_and_user_dialog_gates_leave_marks_and_editor_state_unchanged() {
    let mut fixture = Fixture::new();
    let first = fixture.first;
    let id = fixture.capture(first, 0..5, "retain this");
    for action in [
        AgentMarksAction::Take {
            request_id: Some("invalid\nrequest".into()),
            limit: 1,
        },
        AgentMarksAction::Take {
            request_id: Some("invalid-limit".into()),
            limit: 0,
        },
        AgentMarksAction::Note {
            id,
            note: "x".repeat(4097),
        },
        AgentMarksAction::Remove { id: 0 },
        AgentMarksAction::Ack { batch_id: u64::MAX },
        AgentMarksAction::Restore {
            batch_id: Some(u64::MAX),
        },
    ] {
        let marks = saved_marks(&fixture.app);
        let editor = editor_state(&fixture.app);
        let invalid = request(&mut fixture.app, action);
        assert!(!invalid.ok, "{invalid:?}");
        assert_eq!(saved_marks(&fixture.app), marks);
        assert_eq!(editor_state(&fixture.app), editor);
    }
    fixture.app.settings_open = true;
    let marks = saved_marks(&fixture.app);
    let editor = editor_state(&fixture.app);
    let rejected = fixture.take("dialog-open", 1);
    assert!(!rejected.ok);
    assert_eq!(rejected.error.unwrap().code, "user_dialog_open");
    assert_eq!(saved_marks(&fixture.app), marks);
    assert_eq!(editor_state(&fixture.app), editor);
    assert!(fixture.app.settings_open);
}

#[test]
fn production_session_preserves_frozen_batches_retry_receipts_and_legacy_empty_defaults() {
    let mut fixture = Fixture::new();
    let first = fixture.first;
    let old = fixture.capture(first, 0..5, "resume after restart");
    let taken = fixture.take("persisted-request", 1);
    let batch = batch_id(&taken);
    let newer = fixture.capture(first, 12..18, "still waiting");
    let encoded = serde_json::to_vec(&fixture.app.session()).unwrap();
    let restored: Session = serde_json::from_slice(&encoded).unwrap();
    let mut next_app = VEditApp::empty(egui::Context::default());
    next_app.recovery_session_path = fixture.root.join("restart/session.json");
    next_app.agent_marks = restored.agent_marks;
    let retry = request(
        &mut next_app,
        AgentMarksAction::Take {
            request_id: Some("persisted-request".into()),
            limit: 1,
        },
    );
    assert_eq!(batch_id(&retry), batch);
    assert_eq!(retry.result["replayed"], true);
    assert_eq!(retry.result["items"][0]["id"], old);
    assert_eq!(retry.result["items"][0]["text"], "first");
    assert_eq!(mark_ids(&next_app), [newer]);
    let pending = request(
        &mut next_app,
        AgentMarksAction::Take {
            request_id: Some("after-restart".into()),
            limit: 1,
        },
    );
    assert!(!pending.ok);
    assert_eq!(pending.error.unwrap().code, "batch_pending");
    assert_eq!(pending.result["batch_id"], batch);
    let mut legacy = serde_json::to_value(fixture.app.session()).unwrap();
    legacy.as_object_mut().unwrap().remove("agent_marks");
    let restored: Session = serde_json::from_value(legacy).unwrap();
    assert!(restored.agent_marks.items.is_empty());
    assert!(restored.agent_marks.last_batch().is_none());
    assert_eq!(restored.projects.len(), 2);
    assert_eq!(restored.projects[0].tabs.len(), 2);
    assert!(!restored.settings.unwrap().autosave);
}

#[test]
fn recovered_untitled_snapshot_cannot_claim_or_navigate_a_tab_with_a_reused_serialized_id() {
    let mut fixture = Fixture::new();
    let first = fixture.first;
    let original_tab = fixture.app.tab_by_id_mut(first).unwrap();
    original_tab.path = None;
    original_tab.title = "Untitled plan".into();
    let id = fixture.capture(first, 0..5, "Retain the exact original excerpt");
    let encoded = serde_json::to_vec(&fixture.app.session()).unwrap();
    let recovered: Session = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(fixture.app.agent_marks.visible_marks(first).count(), 1);
    fixture.app.perform_close(CloseAction::Tab(first));
    let closed = fixture.app.agent_marks.get(id, None).unwrap();
    assert!(!closed.is_attached());
    assert!(
        !closed.has_runtime_identity(),
        "Closing the actual document must revoke live authority"
    );
    assert_eq!(closed.snippet, "first");
    assert_eq!(fixture.app.agent_marks.visible_marks(first).count(), 0);
    assert_eq!(
        list(&mut fixture.app, 1, 0)["items"][0]["source_open"],
        false
    );
    let workspace = recovered.projects[0].root.clone();
    let mut next_app = VEditApp::empty(egui::Context::default());
    next_app.recovery_session_path = fixture.root.join("untitled-restart/session.json");
    next_app.agent_marks = recovered.agent_marks;
    let mut project = Project::new(workspace);
    let mut unrelated = EditorTab::new(None, "Unrelated unsaved work".into());
    // Deliberately reuse the persisted number in the same workspace. Serialized
    // runtime hints must never establish authority over this unrelated buffer.
    unrelated.id = first;
    unrelated.text = "entirely different and valuable work".into();
    unrelated.saved_text = unrelated.text.clone();
    unrelated.recovery_path = fixture.root.join("untitled-restart/unrelated.txt");
    unrelated.set_cursor(8..8);
    project.tabs.push(unrelated);
    next_app.projects.push(project);
    let editor = editor_state(&next_app);
    let observed = list(&mut next_app, 1, 0);
    assert_eq!(observed["items"][0]["id"], id);
    assert_eq!(observed["items"][0]["text"], "first");
    assert_eq!(observed["items"][0]["source_open"], false);
    assert_eq!(observed["items"][0]["range_current"], false);
    assert!(observed["items"][0]["tab_id"].is_null());
    assert!(observed["items"][0]["revision"].is_null());
    assert!(observed["items"][0]["dirty"].is_null());
    assert_eq!(next_app.agent_marks.visible_marks(first).count(), 0);
    assert_eq!(editor_state(&next_app), editor);
    let navigation = next_app.navigate_agent_mark(id);
    assert!(
        navigation.is_err(),
        "A persisted untitled snapshot has no trusted live document"
    );
    assert_eq!(
        editor_state(&next_app),
        editor,
        "An unavailable snapshot must not alter the unrelated tab's cursor, selection, view or text"
    );
    assert_eq!(next_app.projects[0].tabs.len(), 1);
    assert_eq!(list(&mut next_app, 1, 0)["items"][0]["text"], "first");
}

#[test]
fn failed_durable_take_and_ack_roll_back_receipts_current_marks_and_live_untitled_highlights() {
    let mut fixture = Fixture::new();
    let (first, other) = (fixture.first, fixture.other);
    let saved_id = fixture.capture(first, 0..5, "saved source");
    let untitled = fixture.app.tab_by_id_mut(other).unwrap();
    untitled.path = None;
    untitled.title = "Untitled discussion".into();
    let untitled_id = fixture.capture(other, 0..7, "unsaved source identity");
    assert_eq!(fixture.app.agent_marks.visible_marks(first).count(), 1);
    assert_eq!(fixture.app.agent_marks.visible_marks(other).count(), 1);

    let good_path = fixture.app.recovery_session_path.clone();
    let blocker = fixture.root.join("recovery-parent-is-a-file");
    fs::write(&blocker, "keep this fixture file").unwrap();
    fixture.app.recovery_session_path = blocker.join("session.json");
    let marks = saved_marks(&fixture.app);
    let editor = editor_state(&fixture.app);
    let rejected_take = fixture.take("retry-after-storage-recovers", 1);
    assert!(
        !rejected_take.ok,
        "Consumption must wait for a successful durable write"
    );
    assert_eq!(rejected_take.error.unwrap().code, "persistence_failed");
    assert_eq!(saved_marks(&fixture.app), marks);
    assert_eq!(editor_state(&fixture.app), editor);
    assert_eq!(mark_ids(&fixture.app), [saved_id, untitled_id]);
    assert_eq!(fixture.app.agent_marks.visible_marks(first).count(), 1);
    assert_eq!(
        fixture.app.agent_marks.visible_marks(other).count(),
        1,
        "A failed write must retain the live identity of untitled selections"
    );
    assert_eq!(
        fs::read_to_string(&blocker).unwrap(),
        "keep this fixture file"
    );

    fixture.app.recovery_session_path = good_path.clone();
    let successful_take = fixture.take("retry-after-storage-recovers", 1);
    let batch = batch_id(&successful_take);
    assert_eq!(
        successful_take.result["replayed"], false,
        "The failed request must not have reserved its idempotency receipt"
    );
    assert_eq!(successful_take.result["persistence"], "saved");
    assert_eq!(successful_take.result["total"], 2);
    let durable: Session = serde_json::from_slice(&fs::read(&good_path).unwrap()).unwrap();
    let retained = durable.agent_marks.last_batch().unwrap();
    assert_eq!(retained.id, batch);
    assert_eq!(retained.request_id, "retry-after-storage-recovers");
    assert!(!retained.acknowledged);
    assert!(durable.agent_marks.items.is_empty());

    let newer = fixture.capture(first, 12..18, "created while agent reads the batch");
    fixture.app.recovery_session_path = blocker.join("session.json");
    let marks = saved_marks(&fixture.app);
    let editor = editor_state(&fixture.app);
    let rejected_ack = request(&mut fixture.app, AgentMarksAction::Ack { batch_id: batch });
    assert!(
        !rejected_ack.ok,
        "Acknowledgement must not outrun its durable write"
    );
    assert_eq!(rejected_ack.error.unwrap().code, "persistence_failed");
    assert_eq!(saved_marks(&fixture.app), marks);
    assert_eq!(editor_state(&fixture.app), editor);
    assert_eq!(mark_ids(&fixture.app), [newer]);
    assert!(!fixture.app.agent_marks.last_batch().unwrap().acknowledged);
    let still_pending = fixture.take("do-not-overwrite-unacknowledged-batch", 1);
    assert!(!still_pending.ok);
    assert_eq!(still_pending.error.unwrap().code, "batch_pending");
    assert_eq!(still_pending.result["batch_id"], batch);
    assert_eq!(mark_ids(&fixture.app), [newer]);

    fixture.app.recovery_session_path = good_path.clone();
    let successful_ack = request(&mut fixture.app, AgentMarksAction::Ack { batch_id: batch });
    assert!(successful_ack.ok, "{successful_ack:?}");
    assert_eq!(successful_ack.result["persistence"], "saved");
    let durable: Session = serde_json::from_slice(&fs::read(&good_path).unwrap()).unwrap();
    assert!(durable.agent_marks.last_batch().unwrap().acknowledged);
    assert_eq!(durable.agent_marks.items[0].id, newer);
    let next_take = fixture.take("do-not-overwrite-unacknowledged-batch", 1);
    assert!(next_take.ok, "{next_take:?}");
    assert_eq!(next_take.result["items"][0]["id"], newer);
    assert_ne!(batch_id(&next_take), batch);
}

//! Project switches and startup must preserve each root's complete editor arrangement.

use super::*;
use crate::cli::{DocumentView, LayoutAction, OpenFile, Request, SplitOrientation};
use egui_kittest::Harness;
use serde_json::{Value, json};

struct WorkspaceFiles {
    root: PathBuf,
}

impl WorkspaceFiles {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "morrow-project-layout-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        for project in ["A", "B", "C"] {
            fs::create_dir(root.join(project)).unwrap();
            if project != "C" {
                for name in ["first.md", "second.md", "third.md", "agent.md"] {
                    fs::write(root.join(project).join(name), document(project, name)).unwrap();
                }
            }
        }
        Self { root }
    }

    fn project(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    fn remove_project(&self, name: &str) {
        let project = self.project(name);
        for file in [
            "first.md",
            "second.md",
            "third.md",
            "agent.md",
            "recovery-first.md",
            "recovery-second.md",
            "recovery-third.md",
            "recovery-untitled.txt",
        ] {
            let _ = fs::remove_file(project.join(file));
        }
        let _ = fs::remove_dir(project);
    }
}

impl Drop for WorkspaceFiles {
    fn drop(&mut self) {
        for name in ["A", "B", "C"] {
            self.remove_project(name);
        }
        let _ = fs::remove_dir(&self.root);
    }
}

fn document(project: &str, name: &str) -> String {
    (0..45)
        .map(|line| {
            format!("# {project} {name} section {line}\n\nUnicode 🦀 café detail {line}.\n\n")
        })
        .collect()
}

fn fixture(files: &WorkspaceFiles) -> MorrowApp {
    let mut app = MorrowApp::empty(egui::Context::default());
    app.settings.autosave = false;
    app.settings.smooth_scroll = false;
    app.sidebar_visible = false;
    for name in ["A", "B", "C"] {
        let root = files.project(name);
        let mut project = Project::new(root.clone());
        project.index = Some(Arc::new(workspace::WorkspaceIndex::empty(root.clone())));
        project.last_git_refresh = Instant::now();
        if name != "C" {
            for file in ["first.md", "second.md", "third.md"] {
                let mut tab = EditorTab::new(Some(root.join(file)), file.into());
                tab.text = document(name, file);
                tab.saved_text = tab.text.clone();
                tab.fingerprint =
                    Some(workspace::fingerprint(&root.join(file), MAX_DOCUMENT).unwrap());
                tab.recovery_path = root.join(format!("recovery-{file}"));
                project.tabs.push(tab);
            }
        }
        app.projects.push(project);
    }
    app.bind_current_pane();
    app
}

fn layout(app: &MorrowApp) -> Value {
    serde_json::to_value(app.editor_session_layout()).unwrap()
}

fn panes(app: &MorrowApp) -> Vec<Value> {
    app.layout_status()["panes"].as_array().unwrap().clone()
}

fn assert_own_documents(app: &MorrowApp, project: usize) {
    let own: std::collections::HashSet<_> = app.projects[project]
        .tabs
        .iter()
        .map(|tab| tab.id)
        .collect();
    for pane in panes(app) {
        for id in pane["tab_ids"].as_array().unwrap() {
            assert!(
                own.contains(&id.as_u64().unwrap()),
                "foreign document leaked: {pane}"
            );
        }
        if let Some(id) = pane["tab_id"].as_u64() {
            assert!(own.contains(&id));
        }
    }
}

fn change(app: &mut MorrowApp, action: LayoutAction) {
    app.control_layout(action).unwrap();
}

fn cursor(app: &mut MorrowApp, offset: usize) {
    app.tab_mut().unwrap().set_cursor(offset..offset);
    app.capture_pane_navigation();
}

fn arrange_a(app: &mut MorrowApp) -> (u64, u64, u64) {
    app.switch_project(0);
    let first = app.current_pane_id();
    let first_tab = app.projects[0].tabs[0].id;
    let second_tab = app.projects[0].tabs[1].id;
    let third_tab = app.projects[0].tabs[2].id;
    cursor(app, 8);
    let right = app.split_active(SplitOrientation::Vertical).unwrap();
    change(
        app,
        LayoutAction::Assign {
            pane_id: right,
            tab_id: second_tab,
            view: None,
        },
    );
    cursor(app, 13);
    let bottom = app.split_active(SplitOrientation::Horizontal).unwrap();
    change(
        app,
        LayoutAction::Assign {
            pane_id: bottom,
            tab_id: third_tab,
            view: Some(DocumentView::Preview),
        },
    );
    cursor(app, 21);
    change(
        app,
        LayoutAction::MoveTab {
            source_pane_id: first,
            pane_id: right,
            tab_id: second_tab,
            index: 0,
        },
    );
    change(
        app,
        LayoutAction::MoveTab {
            source_pane_id: first,
            pane_id: first,
            tab_id: third_tab,
            index: 0,
        },
    );
    change(
        app,
        LayoutAction::Resize {
            pane_id: first,
            ratio: 0.37,
        },
    );
    change(
        app,
        LayoutAction::Resize {
            pane_id: bottom,
            ratio: 0.64,
        },
    );
    change(app, LayoutAction::Focus { pane_id: first });
    // Return to the source document without changing the group's reordered tabs.
    app.activate_tab(0, 0);
    assert_eq!(app.tab().unwrap().id, first_tab);
    cursor(app, 8);
    change(app, LayoutAction::Focus { pane_id: bottom });
    (first, right, bottom)
}

fn arrange_b(app: &mut MorrowApp) -> (u64, u64) {
    app.switch_project(1);
    let upper = app.current_pane_id();
    let third = app.projects[1].tabs[2].id;
    let second = app.projects[1].tabs[1].id;
    change(
        app,
        LayoutAction::MoveTab {
            source_pane_id: upper,
            pane_id: upper,
            tab_id: third,
            index: 0,
        },
    );
    cursor(app, 5);
    let lower = app.split_active(SplitOrientation::Horizontal).unwrap();
    change(
        app,
        LayoutAction::Assign {
            pane_id: lower,
            tab_id: second,
            view: Some(DocumentView::Source),
        },
    );
    cursor(app, 34);
    change(
        app,
        LayoutAction::Resize {
            pane_id: lower,
            ratio: 0.71,
        },
    );
    (upper, lower)
}

#[test]
fn project_switch_restores_nested_splits_groups_views_and_dirty_unicode_history() {
    let files = WorkspaceFiles::new();
    let mut app = fixture(&files);
    let (left, _, preview) = arrange_a(&mut app);
    change(&mut app, LayoutAction::Focus { pane_id: left });
    app.activate_tab(0, 0);
    let original = app.tab().unwrap().text.clone();
    app.tab_mut().unwrap().apply(editor::replace_selection(
        &original,
        0..0,
        "unsaved 🦀 café\n",
    ));
    let dirty_text = app.tab().unwrap().text.clone();
    cursor(&mut app, 16);
    change(&mut app, LayoutAction::Focus { pane_id: preview });
    let a = layout(&app);
    assert_own_documents(&app, 0);
    arrange_b(&mut app);
    let b = layout(&app);
    assert_own_documents(&app, 1);
    assert_ne!(a["root"], b["root"]);
    assert_ne!(
        a["panes"].as_array().unwrap().len(),
        b["panes"].as_array().unwrap().len()
    );

    for _ in 0..3 {
        app.switch_project(0);
        assert_eq!(layout(&app), a);
        assert_own_documents(&app, 0);
        assert_eq!(app.projects[0].tabs[0].text, dirty_text);
        assert!(app.projects[0].tabs[0].dirty);
        app.switch_project(1);
        assert_eq!(layout(&app), b);
        assert_own_documents(&app, 1);
    }
    app.switch_project(0);
    change(&mut app, LayoutAction::Focus { pane_id: left });
    app.activate_tab(0, 0);
    assert_eq!(app.tab().unwrap().vim.cursor, 16);
    let tab = app.tab_mut().unwrap();
    assert!(
        tab.vim
            .handle_key(&mut tab.text, crate::vim::VimKey::Char('u'))
            .text_changed
    );
    assert_eq!(
        tab.text, original,
        "switching projects must retain shared Vim undo history"
    );
    assert!(
        tab.vim
            .handle_key(&mut tab.text, crate::vim::VimKey::Redo)
            .text_changed
    );
    assert_eq!(tab.text, dirty_text);
    assert_eq!(app.projects[1].tabs[0].text, document("B", "first.md"));
}

#[test]
fn empty_project_owns_its_own_blank_layout_and_new_buffer() {
    let files = WorkspaceFiles::new();
    let mut app = fixture(&files);
    arrange_a(&mut app);
    let a = layout(&app);
    app.switch_project(2);
    assert_eq!(app.active_project, 2);
    assert!(app.tab().is_none());
    assert_eq!(panes(&app).len(), 1);
    assert!(panes(&app)[0]["tab_id"].is_null());
    assert!(panes(&app)[0]["tab_ids"].as_array().unwrap().is_empty());
    let blank_pane = app.current_pane_id();
    app.new_file();
    let new_id = app.tab().unwrap().id;
    app.tab_mut()
        .unwrap()
        .apply(editor::replace_selection("", 0..0, "C's unsaved 🦀"));
    assert_eq!(app.current_pane_id(), blank_pane);
    assert_own_documents(&app, 2);
    app.switch_project(0);
    assert_eq!(layout(&app), a);
    app.switch_project(2);
    assert_eq!(app.tab().unwrap().id, new_id);
    assert_eq!(app.tab().unwrap().text, "C's unsaved 🦀");
    assert!(app.tab().unwrap().dirty);
}

#[test]
fn closing_inactive_project_or_document_keeps_the_active_arrangement() {
    let files = WorkspaceFiles::new();
    let mut app = fixture(&files);
    arrange_a(&mut app);
    let inactive = app.projects[0].tabs[1].id;
    arrange_b(&mut app);
    let b = layout(&app);
    let response = app.control_request(Request::Close {
        tab_ids: vec![inactive],
    });
    assert!(response.ok);
    assert_eq!(response.result["complete"], true);
    assert_eq!(layout(&app), b);
    app.switch_project(0);
    assert!(panes(&app).iter().all(|pane| {
        !pane["tab_ids"]
            .as_array()
            .unwrap()
            .iter()
            .any(|id| id == inactive)
    }));
    assert_own_documents(&app, 0);
    app.switch_project(1);
    app.request_close(CloseAction::Project(0));
    assert_eq!(
        app.active_project, 0,
        "surviving B shifts index without losing its layout"
    );
    assert_eq!(app.project().unwrap().root, files.project("B"));
    // Serialized document indices shift with the project list; the actual layout is unchanged.
    let mut expected = b;
    for pane in expected["panes"].as_array_mut().unwrap() {
        if !pane["project"].is_null() {
            pane["project"] = json!(0);
        }
        for pair in pane["documents"].as_array_mut().unwrap() {
            pair[0] = json!(0);
        }
        for state in pane["states"].as_array_mut().unwrap() {
            state["project"] = json!(0);
        }
    }
    assert_eq!(layout(&app), expected);
    assert_own_documents(&app, 0);
}

#[test]
fn cli_layout_rejects_inactive_tabs_and_panes_without_switching_or_mutating() {
    let files = WorkspaceFiles::new();
    let mut app = fixture(&files);
    let (_, _, inactive_pane) = arrange_a(&mut app);
    let inactive_tab = app.projects[0].tabs[2].id;
    arrange_b(&mut app);
    let active_pane = app.current_pane_id();
    let before = layout(&app);
    let a_ids: std::collections::HashSet<_> = app.session().projects[0]
        .layout
        .as_ref()
        .map(|layout| {
            serde_json::to_value(layout).unwrap()["panes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|pane| pane["id"].as_u64().unwrap())
                .collect()
        })
        .unwrap();
    assert!(
        panes(&app)
            .iter()
            .all(|pane| !a_ids.contains(&pane["id"].as_u64().unwrap())),
        "pane handles must be unique across active and parked layouts"
    );
    for action in [
        LayoutAction::Focus {
            pane_id: inactive_pane,
        },
        LayoutAction::Close {
            pane_id: inactive_pane,
        },
        LayoutAction::Assign {
            pane_id: active_pane,
            tab_id: inactive_tab,
            view: None,
        },
        LayoutAction::Split {
            orientation: SplitOrientation::Vertical,
            pane_id: Some(active_pane),
            tab_id: Some(inactive_tab),
            view: None,
        },
        LayoutAction::MoveTab {
            source_pane_id: inactive_pane,
            pane_id: active_pane,
            tab_id: inactive_tab,
            index: 0,
        },
    ] {
        let response = app.control_request(Request::Layout { change: action });
        assert!(
            !response.ok,
            "inactive IDs must not accidentally address B's layout"
        );
        assert_eq!(app.active_project, 1);
        assert_eq!(layout(&app), before);
    }
    let active_text = app.tab().unwrap().text.clone();
    let tab = app.tab_mut().unwrap();
    tab.vim
        .handle_key(&mut tab.text, crate::vim::VimKey::Char('i'));
    let response = app.control_request(Request::Layout {
        change: LayoutAction::Assign {
            pane_id: active_pane,
            tab_id: inactive_tab,
            view: None,
        },
    });
    assert!(!response.ok);
    assert_eq!(app.tab().unwrap().vim.mode, VimMode::Insert);
    assert_eq!(app.tab().unwrap().text, active_text);
    let tab = app.tab_mut().unwrap();
    tab.vim
        .handle_key(&mut tab.text, crate::vim::VimKey::Escape);
    // The app applies Escape's resulting caret to the native selection too.
    let normal_cursor = tab.vim.cursor;
    tab.set_cursor(normal_cursor..normal_cursor);
    // Leaving Insert deliberately moves the caret back one character. This user
    // action establishes the state that cross-project navigation must preserve.
    let after_insert = layout(&app);
    let response = app.control_request(Request::Focus {
        tab_id: inactive_tab,
        location: None,
        view: Some(DocumentView::Preview),
    });
    assert!(
        response.ok,
        "explicit document focus is the cross-project navigation seam"
    );
    assert_eq!(app.active_project, 0);
    assert_eq!(panes(&app).len(), 3);
    assert_eq!(app.tab().unwrap().id, inactive_tab);
    assert_own_documents(&app, 0);
    app.switch_project(1);
    assert_eq!(layout(&app), after_insert);
}

fn wait_loaded(app: &mut MorrowApp) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while app
        .projects
        .iter()
        .flat_map(|project| &project.tabs)
        .any(|tab| tab.loading)
    {
        assert!(
            Instant::now() < deadline,
            "project documents did not finish loading"
        );
        std::thread::sleep(Duration::from_millis(2));
        app.poll();
        app.apply_pending_agent_locations();
    }
    app.apply_pending_agent_locations();
}

#[test]
fn cross_project_agent_open_and_review_keep_each_root_own_groups() {
    let files = WorkspaceFiles::new();
    let mut app = fixture(&files);
    arrange_a(&mut app);
    let a_tree = layout(&app)["root"].clone();
    arrange_b(&mut app);
    let b_tree = layout(&app)["root"].clone();
    let a_path = files.project("A").join("agent.md");
    let b_path = files.project("B").join("agent.md");
    let response = app.control_request(Request::Open {
        files: vec![
            OpenFile::source(a_path.clone()),
            OpenFile::source(b_path.clone()),
        ],
    });
    assert!(response.ok);
    assert_eq!(response.result["complete"], true);
    wait_loaded(&mut app);
    assert_eq!(app.active_project, 1);
    assert_eq!(layout(&app)["root"], b_tree);
    assert_own_documents(&app, 1);
    app.switch_project(0);
    assert_eq!(layout(&app)["root"], a_tree);
    assert_own_documents(&app, 0);

    let response = app.control_request(Request::Review {
        title: "Two roots".into(),
        note: "Keep the workspaces separate".into(),
        files: vec![
            OpenFile::source(a_path),
            OpenFile {
                view: DocumentView::Preview,
                location: Some(crate::cli::Location { line: 7, column: 4 }),
                ..OpenFile::source(b_path)
            },
        ],
    });
    assert!(response.ok);
    wait_loaded(&mut app);
    assert_eq!(
        app.active_project, 0,
        "review presents its first successful file"
    );
    assert_eq!(app.review.as_ref().unwrap().files.len(), 2);
    assert_eq!(layout(&app)["root"], a_tree);
    assert_own_documents(&app, 0);
    let b_review = app.review.as_ref().unwrap().files[1];
    let b_index = app.projects[1]
        .tabs
        .iter()
        .position(|tab| tab.id == b_review)
        .unwrap();
    let expected_cursor = editor::go_to_line(&app.projects[1].tabs[b_index].text, 7) + 3;
    assert!(
        app.projects[1].tabs[b_index]
            .pending_agent_location
            .is_none()
    );
    assert!(app.projects[1].tabs[b_index].agent_location_error.is_none());
    let parked_b =
        serde_json::to_value(app.project_session_layout(&files.project("B")).unwrap()).unwrap();
    let parked_state = parked_b["panes"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|pane| pane["states"].as_array().unwrap())
        .find(|state| state["tab"] == b_index)
        .unwrap();
    assert_eq!(
        parked_state["cursor"], expected_cursor,
        "the hidden review location must update B's parked pane before requesting focus"
    );
    let response = app.control_request(Request::Focus {
        tab_id: b_review,
        location: None,
        view: Some(DocumentView::Preview),
    });
    assert!(response.ok);
    assert_eq!(app.active_project, 1);
    assert_eq!(layout(&app)["root"], b_tree);
    assert_eq!(app.tab().unwrap().agent_view, DocumentView::Preview);
    assert_eq!(
        app.tab().unwrap().vim.cursor,
        expected_cursor,
        "a loading review document's location must update its parked group"
    );
    assert_own_documents(&app, 1);
}

#[test]
fn inactive_document_read_edit_and_save_preserve_active_project_and_other_local_views() {
    let files = WorkspaceFiles::new();
    let mut app = fixture(&files);
    arrange_a(&mut app);
    let id = app.projects[0].tabs[0].id;
    let a_before = layout(&app);
    arrange_b(&mut app);
    let b = layout(&app);
    let response = app.control_request(Request::Read {
        tab_id: id,
        start_line: 1,
        line_count: 3,
    });
    assert!(response.ok);
    assert_eq!(app.active_project, 1);
    assert_eq!(layout(&app), b);
    let revision = app.projects[0].tabs[0].revision;
    let replacement = format!("Agent change 🦀\n{}", document("A", "first.md"));
    let response = app.control_request(Request::Edit {
        tab_id: id,
        expected_revision: revision,
        edits: Vec::new(),
        set_text: Some(replacement.clone()),
    });
    assert!(response.ok);
    assert_eq!(app.active_project, 1);
    assert_eq!(layout(&app), b);
    assert!(app.projects[0].tabs[0].dirty);
    let dirty_close = app.control_request(Request::Close { tab_ids: vec![id] });
    assert!(dirty_close.ok);
    assert_eq!(dirty_close.result["complete"], false);
    assert_eq!(app.active_project, 1);
    assert_eq!(layout(&app), b);
    let response = app.control_request(Request::Save { tab_ids: vec![id] });
    assert!(response.ok);
    assert_eq!(response.result["items"][0]["ok"], true, "{response:?}");
    assert_eq!(response.result["items"][0]["saving"], true, "{response:?}");
    assert!(app.projects[0].tabs[0].error.is_none());
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.projects[0].tabs[0].saving {
        assert!(Instant::now() < deadline, "inactive save did not finish");
        std::thread::sleep(Duration::from_millis(2));
        app.poll();
    }
    assert!(!app.projects[0].tabs[0].dirty);
    assert!(app.projects[0].tabs[0].error.is_none());
    assert_eq!(
        fs::read_to_string(files.project("A").join("first.md")).unwrap(),
        replacement
    );
    assert_eq!(app.active_project, 1);
    assert_eq!(layout(&app), b);
    app.switch_project(0);
    let a_after = layout(&app);
    assert_eq!(a_after["root"], a_before["root"]);
    assert_eq!(a_after["focused"], a_before["focused"]);
    for (before, after) in a_before["panes"]
        .as_array()
        .unwrap()
        .iter()
        .zip(a_after["panes"].as_array().unwrap())
    {
        assert_eq!(before["documents"], after["documents"]);
        for state in before["states"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|state| state["tab"] != 0)
        {
            let actual = after["states"]
                .as_array()
                .unwrap()
                .iter()
                .find(|candidate| candidate["tab"] == state["tab"])
                .unwrap();
            assert_eq!(
                actual, state,
                "unrelated local cursors and views must stay unchanged"
            );
        }
    }
    assert_own_documents(&app, 0);
}

fn save_recoveries(app: &MorrowApp) {
    for tab in app
        .projects
        .iter()
        .flat_map(|project| &project.tabs)
        .filter(|tab| tab.dirty)
    {
        fs::write(&tab.recovery_path, &tab.text).unwrap();
    }
}

fn restore(saved: Session) -> MorrowApp {
    let mut app = MorrowApp::empty(egui::Context::default());
    app.settings.autosave = false;
    app.restore_session_workspace(saved.projects, saved.active_project, saved.layout);
    wait_loaded(&mut app);
    app
}

#[test]
fn session_roundtrip_restores_all_project_layouts_and_dirty_named_and_untitled_buffers() {
    let files = WorkspaceFiles::new();
    let mut app = fixture(&files);
    arrange_a(&mut app);
    let tab = &mut app.projects[0].tabs[0];
    tab.apply(editor::replace_selection(
        &tab.text,
        0..0,
        "Recovered 🦀 café\n",
    ));
    arrange_b(&mut app);
    let a = app.session().projects[0].layout.clone();
    let b = layout(&app);
    app.switch_project(2);
    app.new_file();
    let tab = app.tab_mut().unwrap();
    tab.recovery_path = files.project("C").join("recovery-untitled.txt");
    tab.apply(editor::replace_selection("", 0..0, "Unsaved C 🦀\n"));
    cursor(&mut app, 9);
    let c = layout(&app);
    save_recoveries(&app);
    let serialized = serde_json::to_vec(&app.session()).unwrap();
    let mut restarted = restore(serde_json::from_slice(&serialized).unwrap());
    assert_eq!(restarted.active_project, 2);
    assert_eq!(layout(&restarted), c);
    assert_eq!(restarted.tab().unwrap().text, "Unsaved C 🦀\n");
    assert!(restarted.tab().unwrap().dirty);
    restarted.switch_project(0);
    assert_eq!(
        layout(&restarted),
        serde_json::to_value(a.unwrap()).unwrap()
    );
    assert!(restarted.projects[0].tabs[0].dirty);
    assert!(
        restarted.projects[0].tabs[0]
            .text
            .starts_with("Recovered 🦀 café\n")
    );
    assert_eq!(
        restarted.projects[0].tabs[0].saved_text,
        document("A", "first.md")
    );
    assert_own_documents(&restarted, 0);
    restarted.switch_project(1);
    assert_eq!(layout(&restarted), b);
    assert_eq!(restarted.tab().unwrap().vim.cursor, 34);
    assert_own_documents(&restarted, 1);
}

#[test]
fn project_session_remaps_layout_documents_after_a_missing_first_root() {
    let files = WorkspaceFiles::new();
    let mut app = fixture(&files);
    arrange_a(&mut app);
    arrange_b(&mut app);
    let b = layout(&app);
    let serialized = serde_json::to_vec(&app.session()).unwrap();
    files.remove_project("A");
    let mut restarted = restore(serde_json::from_slice(&serialized).unwrap());
    assert_eq!(restarted.projects.len(), 2);
    assert_eq!(restarted.active_project, 0);
    assert_eq!(restarted.project().unwrap().root, files.project("B"));
    assert_eq!(layout(&restarted)["root"], b["root"]);
    assert_eq!(layout(&restarted)["focused"], b["focused"]);
    assert_eq!(restarted.tab().unwrap().vim.cursor, 34);
    assert_own_documents(&restarted, 0);
    for pane in layout(&restarted)["panes"].as_array().unwrap() {
        assert_eq!(pane["project"], 0);
        assert!(
            pane["documents"]
                .as_array()
                .unwrap()
                .iter()
                .all(|pair| pair[0] == 0)
        );
        assert!(
            pane["states"]
                .as_array()
                .unwrap()
                .iter()
                .all(|state| state["project"] == 0)
        );
    }
    restarted.switch_project(1);
    assert_eq!(restarted.project().unwrap().root, files.project("C"));
    assert!(restarted.tab().is_none());
    assert_eq!(panes(&restarted).len(), 1);
    assert_own_documents(&restarted, 1);
}

#[test]
fn legacy_mixed_global_layout_migrates_document_groups_and_preview_to_own_projects() {
    let files = WorkspaceFiles::new();
    let app = fixture(&files);
    let mut saved = app.session();
    for project in &mut saved.projects {
        project.layout = None;
    }
    saved.active_project = 0;
    saved.layout = Some(serde_json::from_value(json!({
        "root": {"kind":"split","orientation":"vertical","ratio":0.42,
            "first":{"kind":"pane","id":2},"second":{"kind":"pane","id":3}},
        "focused":2,
        "panes":[
            {"id":2,"project":0,"tab":0,"view":"source","cursor":7,
             "documents":[[0,1],[1,0],[0,0]],"states":[
                {"project":0,"tab":0,"view":"source","cursor":7,"selection":{"start":7,"end":7}},
                {"project":0,"tab":1,"view":"source","cursor":11,"selection":{"start":11,"end":11}},
                {"project":1,"tab":0,"view":"source","cursor":3,"selection":{"start":3,"end":3}}]},
            {"id":3,"project":1,"tab":0,"view":"preview","cursor":22,
             "documents":[[1,0],[0,2]],"states":[
                {"project":1,"tab":0,"view":"preview","cursor":22,"selection":{"start":22,"end":22}},
                {"project":0,"tab":2,"view":"source","cursor":15,"selection":{"start":15,"end":15}}]}
        ]
    })).unwrap());
    let mut restarted = restore(saved);
    assert_eq!(restarted.active_project, 0);
    assert_own_documents(&restarted, 0);
    assert_eq!(panes(&restarted).len(), 2);
    assert_eq!(restarted.tab().unwrap().vim.cursor, 7);
    let first = &layout(&restarted)["panes"][0];
    assert_eq!(first["documents"], json!([[0, 1], [0, 0]]));
    restarted.switch_project(1);
    assert_own_documents(&restarted, 1);
    assert_eq!(panes(&restarted).len(), 2);
    let preview = layout(&restarted)["panes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|pane| pane["view"] == "preview")
        .cloned()
        .unwrap();
    assert_eq!(preview["cursor"], 22);
    let preview_id = preview["id"].as_u64().unwrap();
    change(
        &mut restarted,
        LayoutAction::Focus {
            pane_id: preview_id,
        },
    );
    assert_eq!(restarted.tab().unwrap().agent_view, DocumentView::Preview);
    assert_eq!(restarted.tab().unwrap().vim.cursor, 22);
    restarted.switch_project(2);
    assert!(restarted.tab().is_none());
    assert_eq!(panes(&restarted).len(), 1);
}

#[test]
fn legacy_shared_strip_keeps_each_projects_own_file_order_and_selected_view() {
    let files = WorkspaceFiles::new();
    let app = fixture(&files);
    let mut saved = app.session();
    for project in &mut saved.projects {
        project.layout = None;
    }
    saved.active_project = 1;
    saved.layout = Some(
        serde_json::from_value(json!({
            "root": {"kind":"split","orientation":"vertical","ratio":0.42,
                "first":{"kind":"pane","id":2},"second":{"kind":"pane","id":3}},
            "focused":3,
            "panes":[
                {"id":2,"project":0,"tab":1,"view":"source","cursor":11},
                {"id":3,"project":1,"tab":2,"view":"preview","cursor":22}
            ]
        }))
        .unwrap(),
    );
    let mut restarted = restore(saved);
    assert_eq!(restarted.active_project, 1);
    assert_eq!(restarted.tab().unwrap().title, "third.md");
    assert_eq!(restarted.tab().unwrap().vim.cursor, 22);
    assert_eq!(restarted.tab().unwrap().agent_view, DocumentView::Preview);
    assert_eq!(panes(&restarted).len(), 1);
    assert_eq!(
        layout(&restarted)["panes"][0]["documents"],
        json!([[1, 0], [1, 1], [1, 2]])
    );
    assert_own_documents(&restarted, 1);
    restarted.switch_project(0);
    assert_eq!(restarted.tab().unwrap().title, "second.md");
    assert_eq!(restarted.tab().unwrap().vim.cursor, 11);
    assert_eq!(restarted.tab().unwrap().agent_view, DocumentView::Source);
    assert_eq!(panes(&restarted).len(), 1);
    assert_eq!(
        layout(&restarted)["panes"][0]["documents"],
        json!([[0, 0], [0, 1], [0, 2]])
    );
    assert_own_documents(&restarted, 0);
    restarted.switch_project(1);
    assert_eq!(restarted.tab().unwrap().title, "third.md");
    assert_eq!(restarted.tab().unwrap().vim.cursor, 22);
}

fn harness(app: MorrowApp) -> Harness<'static, MorrowApp> {
    let mut screen = Harness::builder()
        .with_size([1280.0, 820.0])
        .with_os(egui::os::OperatingSystem::Windows)
        .build_ui_state(
            |ui, app: &mut MorrowApp| {
                app.ctx = ui.ctx().clone();
                app.last_disk_check = Instant::now();
                app.last_session_save = Instant::now();
                app.session_dirty = false;
                for project in &mut app.projects {
                    project.last_git_refresh = Instant::now();
                    for tab in &mut project.tabs {
                        tab.last_edit = Instant::now();
                    }
                }
                app.draw_ui(ui);
            },
            app,
        );
    set_style(&screen.ctx);
    screen.ctx.all_styles_mut(|style| {
        style.visuals.text_cursor.blink = false;
        style.animation_time = 0.0;
        style.scroll_animation = egui::style::ScrollAnimation::none();
    });
    screen.run_steps(3);
    screen
}

fn wheel(screen: &mut Harness<'_, MorrowApp>, x: f32, amount: f32) {
    screen.event(egui::Event::PointerMoved(egui::pos2(x, 450.0)));
    screen.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: Vec2::new(0.0, -amount),
        modifiers: egui::Modifiers::NONE,
        phase: egui::TouchPhase::Move,
    });
    screen.run_steps(3);
}

fn preview_offset(app: &MorrowApp, pane: u64) -> f32 {
    let snapshot = layout(app);
    let pane = snapshot["panes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|candidate| candidate["id"] == pane)
        .unwrap();
    pane["states"]
        .as_array()
        .unwrap()
        .iter()
        .find(|state| state["tab"] == 0)
        .unwrap()["preview_scroll"]
        .as_f64()
        .unwrap() as f32
}

#[test]
fn actual_source_and_markdown_wheels_keep_separate_positions_across_project_switches() {
    let files = WorkspaceFiles::new();
    let mut app = fixture(&files);
    let document = app.projects[0].tabs[0].id;
    let source = app.current_pane_id();
    let preview = app.split_active(SplitOrientation::Vertical).unwrap();
    app.set_document_view(DocumentView::Preview);
    let mut screen = harness(app);
    wheel(&mut screen, 300.0, 180.0);
    let source_offset = screen.state().find.view_scroll[&(source, document)].1;
    assert!(
        source_offset.y > 100.0,
        "the actual source viewport must move"
    );
    assert_eq!(preview_offset(screen.state(), preview), 0.0);
    wheel(&mut screen, 900.0, 290.0);
    let preview_scroll = preview_offset(screen.state(), preview);
    assert!(
        preview_scroll > 200.0,
        "the actual Markdown viewport must move"
    );
    assert_eq!(
        screen.state().find.view_scroll[&(source, document)].1,
        source_offset
    );
    assert_eq!(
        screen.state().current_pane_id(),
        preview,
        "wheel hover must preserve keyboard focus"
    );
    for project in [1, 2, 0] {
        screen.state_mut().switch_project(project);
        screen.run_steps(3);
        assert_own_documents(screen.state(), project);
    }
    assert_eq!(screen.state().current_pane_id(), preview);
    let restored_source = screen.state().find.view_scroll[&(source, document)].1;
    let restored_preview = preview_offset(screen.state(), preview);
    assert!((restored_source.y - source_offset.y).abs() < 0.5);
    assert!((restored_preview - preview_scroll).abs() < 0.5);
    assert_eq!(
        screen.state().projects[0].tabs[0].text,
        document_text(&files)
    );
    assert!(!screen.state().projects[0].tabs[0].dirty);

    // Save while A is inactive: startup must restore its parked view positions,
    // including source/preview copies of the same shared Markdown document.
    screen.state_mut().switch_project(1);
    screen.run_steps(3);
    let saved: Session =
        serde_json::from_slice(&serde_json::to_vec(&screen.state().session()).unwrap()).unwrap();
    let mut restarted = restore(saved);
    restarted.switch_project(0);
    let restarted_document = restarted.projects[0].tabs[0].id;
    let restarted = harness(restarted);
    let actual = restarted.state().find.view_scroll[&(source, restarted_document)].1;
    assert!((actual.y - source_offset.y).abs() < 0.5);
    assert!((preview_offset(restarted.state(), preview) - preview_scroll).abs() < 0.5);
}

#[test]
fn restart_restores_inactive_preview_tabs_after_another_preview_renders() {
    let files = WorkspaceFiles::new();
    let mut app = fixture(&files);
    let pane = app.current_pane_id();
    app.set_document_view(DocumentView::Preview);
    let mut screen = harness(app);
    wheel(&mut screen, 600.0, 230.0);
    let first_id = screen.state().projects[0].tabs[0].id;
    let first_scroll = screen.state().tab_preview_scroll(pane, first_id).unwrap();
    screen.state_mut().activate_tab(0, 1);
    screen.state_mut().set_document_view(DocumentView::Preview);
    screen.run_steps(3);
    wheel(&mut screen, 600.0, 420.0);
    let second_id = screen.state().projects[0].tabs[1].id;
    let second_scroll = screen.state().tab_preview_scroll(pane, second_id).unwrap();
    assert!(first_scroll > 100.0 && second_scroll > 300.0);
    screen.state_mut().activate_tab(0, 0);
    screen.run_steps(3);
    let saved =
        serde_json::from_slice(&serde_json::to_vec(&screen.state().session()).unwrap()).unwrap();
    let mut restarted = harness(restore(saved));
    let first_id = restarted.state().projects[0].tabs[0].id;
    assert!(
        (restarted
            .state()
            .tab_preview_scroll(pane, first_id)
            .unwrap()
            - first_scroll)
            .abs()
            < 0.5
    );
    restarted.state_mut().activate_tab(0, 1);
    restarted.run_steps(3);
    let second_id = restarted.state().projects[0].tabs[1].id;
    assert!(
        (restarted
            .state()
            .tab_preview_scroll(pane, second_id)
            .unwrap()
            - second_scroll)
            .abs()
            < 0.5
    );
}

fn document_text(files: &WorkspaceFiles) -> String {
    fs::read_to_string(files.project("A").join("first.md")).unwrap()
}

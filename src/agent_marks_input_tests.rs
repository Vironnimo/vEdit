//! Marking and opening the collection must respect chronological source input.
use super::*;
use egui_kittest::{Harness, kittest::Queryable};

const DOCUMENT: &str = "abcd target\nsecond line\n";

#[test]
fn same_batch_search_prefix_keeps_capture_shortcut_out_of_the_search_prompt() {
    let mut screen = harness(fixture(VimMode::Visual));
    screen.input_mut().events.extend([
        egui::Event::Text("/".into()),
        capture_shortcut(),
        egui::Event::Text("target".into()),
    ]);
    screen.run_steps(4);
    let app = screen.state();
    assert!(app.vim_search_active());
    assert!(app.agent_marks.items.is_empty());
    assert!(!app.agent_marks_modal_open());
    assert_eq!(app.tab().unwrap().text, DOCUMENT);
    assert_eq!(app.tab().unwrap().vim.mode, VimMode::Visual);
    assert!(!app.tab().unwrap().dirty);
    assert!(app.register.is_none());
}

fn fixture(mode: VimMode) -> MorrowApp {
    let mut app = MorrowApp::empty(egui::Context::default());
    app.settings.autosave = false;
    app.settings.smooth_scroll = false;
    app.settings.vim = true;
    app.settings.leader_key = " ".into();
    app.sidebar_visible = false;
    let root = std::env::temp_dir().join("morrow-agent-marks-input-render");
    let mut project = Project::new(root.clone());
    project.index = Some(Arc::new(workspace::WorkspaceIndex::empty(root)));
    project.last_git_refresh = Instant::now();
    let mut tab = EditorTab::new(None, "selection.txt".into());
    tab.text = DOCUMENT.into();
    tab.saved_text = DOCUMENT.into();
    tab.set_cursor(0..0);
    match mode {
        VimMode::Normal => {}
        VimMode::Visual | VimMode::VisualLine | VimMode::Insert => {
            let key = match mode {
                VimMode::Visual => 'v',
                VimMode::VisualLine => 'V',
                VimMode::Insert => 'i',
                _ => unreachable!(),
            };
            let action = tab.vim.handle_key(&mut tab.text, VimKey::Char(key));
            assert!(!action.text_changed);
            tab.selection = tab.vim.selection(&tab.text).unwrap_or(0..0);
            tab.pending_selection = Some(tab.selection.clone());
        }
    }
    project.tabs.push(tab);
    app.projects.push(project);
    app.bind_current_pane();
    let id = app.tab().unwrap().id;
    app.sync_external_document_navigation(id);
    app
}

fn harness(app: MorrowApp) -> Harness<'static, MorrowApp> {
    let mut screen = Harness::builder()
        .with_size([1100.0, 740.0])
        .with_os(egui::os::OperatingSystem::Windows)
        .build_ui_state(
            |ui, app: &mut MorrowApp| {
                app.ctx = ui.ctx().clone();
                app.last_disk_check = Instant::now();
                app.last_session_save = Instant::now();
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
        style.animation_time = 0.0;
        style.scroll_animation = egui::style::ScrollAnimation::none();
        style.visuals.text_cursor.blink = false;
    });
    screen.run_steps(3);
    let widget = screen
        .state()
        .document_widget_id(screen.state().tab().unwrap().id);
    assert!(screen.ctx.memory(|memory| memory.has_focus(widget)));
    screen
}

fn pressed(key: Key, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}

fn capture_shortcut() -> egui::Event {
    pressed(
        Key::A,
        egui::Modifiers::COMMAND.plus(egui::Modifiers::SHIFT),
    )
}

fn assert_prefix_mark_and_suffix_motion(screen: &Harness<'_, MorrowApp>) {
    let app = screen.state();
    assert_eq!(app.agent_marks.items.len(), 1);
    let mark = &app.agent_marks.items[0];
    assert_eq!(
        mark.range,
        (0..2).into(),
        "Capture must include the earlier l motion"
    );
    assert_eq!(mark.snippet, "ab");
    let tab = app.tab().unwrap();
    assert_eq!(tab.text, DOCUMENT);
    assert_eq!(tab.vim.mode, VimMode::Visual);
    assert_eq!(
        tab.vim.cursor, 0,
        "The later h belongs to the source editor"
    );
    assert_eq!(tab.selection, 0..1);
    assert_eq!(tab.vim.pending_command(), "");
    assert!(!tab.dirty);
    assert!(
        !tab.vim.can_undo(),
        "Capturing and moving must not add undo edits"
    );
    assert!(app.register.is_none());
    assert!(!app.agent_marks_modal_open());
    assert!(!app.navigation_pending());
}

#[test]
fn one_batch_visual_entry_then_capture_marks_the_new_selection() {
    let mut screen = harness(fixture(VimMode::Normal));
    // Harness::event queues separate frames; these events must share RawInput.
    screen.input_mut().events.extend([
        egui::Event::Text("vl".into()),
        capture_shortcut(),
        egui::Event::Text("h".into()),
    ]);
    screen.run_steps(4);
    assert_prefix_mark_and_suffix_motion(&screen);
}

#[test]
fn one_batch_visual_motion_then_capture_excludes_the_later_motion() {
    let mut screen = harness(fixture(VimMode::Visual));
    screen.input_mut().events.extend([
        egui::Event::Text("l".into()),
        capture_shortcut(),
        egui::Event::Text("h".into()),
    ]);
    screen.run_steps(4);
    assert_prefix_mark_and_suffix_motion(&screen);
}

#[test]
fn one_batch_arrow_motion_then_leader_capture_marks_the_updated_selection() {
    let mut screen = harness(fixture(VimMode::Visual));
    screen.input_mut().events.extend([
        pressed(Key::ArrowRight, egui::Modifiers::NONE),
        egui::Event::Text(" a".into()),
        egui::Event::Text("h".into()),
    ]);
    screen.run_steps(4);
    assert_prefix_mark_and_suffix_motion(&screen);
}

#[test]
fn one_text_event_visual_motion_then_leader_capture_keeps_all_input_in_order() {
    let mut screen = harness(fixture(VimMode::Visual));
    screen
        .input_mut()
        .events
        .push(egui::Event::Text("l ah".into()));
    screen.run_steps(4);
    assert_prefix_mark_and_suffix_motion(&screen);
}

#[test]
fn one_batch_collection_shortcut_keeps_insert_prefix_in_source_and_suffix_in_filter() {
    let mut screen = harness(fixture(VimMode::Insert));
    screen.input_mut().events.extend([
        egui::Event::Text("OLD".into()),
        pressed(
            Key::A,
            egui::Modifiers::COMMAND
                .plus(egui::Modifiers::SHIFT)
                .plus(egui::Modifiers::ALT),
        ),
        egui::Event::Text("NEW".into()),
    ]);
    screen.run_steps(4);
    let app = screen.state();
    assert!(app.agent_marks_modal_open());
    assert!(app.agent_marks.items.is_empty());
    let tab = app.tab().unwrap();
    assert_eq!(tab.text, format!("OLD{DOCUMENT}"));
    assert_eq!(tab.vim.mode, VimMode::Insert);
    assert_eq!(tab.vim.cursor, 3);
    assert_eq!(tab.selection, 3..3);
    assert!(tab.dirty);
    assert!(app.register.is_none());
    assert_eq!(
        screen.ctx.memory(|memory| memory.focused()),
        Some(egui::Id::new("agent_marks_filter")),
    );
    let filter = screen
        .get_all_by_role(egui::accesskit::Role::TextInput)
        .find(|node| node.is_focused())
        .expect("The collection filter must own the suffix input");
    assert_eq!(filter.value(), Some("NEW".into()));

    screen.get_by_label("Close marked blocks").click();
    screen.run_steps(4);
    assert!(!screen.state().agent_marks_modal_open());
    assert_eq!(screen.state().tab().unwrap().text, format!("OLD{DOCUMENT}"));
    assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::Insert);
    screen.input_mut().events.extend([
        pressed(Key::Escape, egui::Modifiers::NONE),
        egui::Event::Text("u".into()),
    ]);
    screen.run_steps(3);
    assert_eq!(screen.state().tab().unwrap().text, DOCUMENT);
    assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::Normal);
    assert!(!screen.state().tab().unwrap().dirty);
    assert!(screen.state().agent_marks.items.is_empty());
    assert!(screen.state().register.is_none());
}

#[test]
fn one_batch_native_selection_capture_excludes_the_later_selection_extension() {
    let mut app = fixture(VimMode::Normal);
    app.settings.vim = false;
    let mut screen = harness(app);
    screen.input_mut().events.extend([
        pressed(Key::ArrowRight, egui::Modifiers::SHIFT),
        capture_shortcut(),
        pressed(Key::ArrowRight, egui::Modifiers::SHIFT),
    ]);
    screen.run_steps(4);
    let app = screen.state();
    assert_eq!(app.agent_marks.items.len(), 1);
    assert_eq!(app.agent_marks.items[0].range, (0..1).into());
    assert_eq!(app.agent_marks.items[0].snippet, "a");
    let tab = app.tab().unwrap();
    assert_eq!(tab.text, DOCUMENT);
    assert_eq!(tab.selection, 0..2);
    assert!(!tab.dirty);
    assert!(!tab.vim.can_undo());
    assert!(app.register.is_none());
    assert!(!app.agent_marks_modal_open());
}

fn split_fixture() -> (MorrowApp, u64, u64, u64, u64) {
    let mut app = fixture(VimMode::Visual);
    let left_pane = app.current_pane_id();
    let left_id = app.tab().unwrap().id;
    let mut right = EditorTab::new(None, "right.txt".into());
    right.text = "RIGHT target\n".into();
    right.saved_text = right.text.clone();
    let right_id = right.id;
    app.projects[0].tabs.push(right);
    let right_pane = app
        .split_active(crate::cli::SplitOrientation::Vertical)
        .unwrap();
    for change in [
        crate::cli::LayoutAction::Assign {
            pane_id: right_pane,
            tab_id: right_id,
            view: Some(crate::cli::DocumentView::Source),
        },
        crate::cli::LayoutAction::Focus { pane_id: left_pane },
    ] {
        let response = app.control_request(crate::cli::Request::Layout { change });
        assert!(response.ok, "{response:?}");
    }
    {
        let tab = app.tab_by_id_mut(right_id).unwrap();
        tab.vim.set_selection(&tab.text, 0..2);
        tab.selection = 0..2;
        tab.pending_selection = Some(0..2);
        tab.focus = false;
    }
    app.sync_external_document_navigation(right_id);
    (app, left_pane, right_pane, left_id, right_id)
}

#[test]
fn one_batch_capture_and_pane_focus_shortcuts_use_their_chronological_document_owner() {
    for capture_first in [true, false] {
        let (app, left_pane, right_pane, left_id, right_id) = split_fixture();
        let mut screen = harness(app);
        assert_eq!(screen.state().current_pane_id(), left_pane);
        let focus = pressed(Key::F6, egui::Modifiers::NONE);
        let capture = capture_shortcut();
        let events = if capture_first {
            [capture, focus]
        } else {
            [focus, capture]
        };
        screen.input_mut().events.extend(events);
        screen.run_steps(6);
        let app = screen.state();
        assert_eq!(app.current_pane_id(), right_pane);
        assert_eq!(app.agent_marks.items.len(), 1);
        let mark = &app.agent_marks.items[0];
        if capture_first {
            assert_eq!(mark.document_id, left_id);
            assert_eq!(mark.range, (0..1).into());
            assert_eq!(mark.snippet, "a");
        } else {
            assert_eq!(mark.document_id, right_id);
            assert_eq!(mark.range, (0..2).into());
            assert_eq!(mark.snippet, "RI");
        }
        assert_eq!(
            app.projects[0]
                .tabs
                .iter()
                .find(|tab| tab.id == left_id)
                .unwrap()
                .text,
            DOCUMENT,
        );
        assert_eq!(
            app.projects[0]
                .tabs
                .iter()
                .find(|tab| tab.id == right_id)
                .unwrap()
                .text,
            "RIGHT target\n",
        );
        assert!(app.projects[0].tabs.iter().all(|tab| !tab.dirty));
        assert!(app.register.is_none());
    }
}

#[test]
fn pending_leader_capture_precedes_a_later_escape_in_the_same_batch() {
    let mut screen = harness(fixture(VimMode::Visual));
    screen
        .input_mut()
        .events
        .push(egui::Event::Text(" ".into()));
    screen.run_steps(1);
    assert!(screen.state().navigation_pending());
    assert_eq!(screen.state().tab().unwrap().selection, 0..1);
    screen.input_mut().events.extend([
        egui::Event::Text("a".into()),
        pressed(Key::Escape, egui::Modifiers::NONE),
    ]);
    screen.run_steps(4);
    let app = screen.state();
    assert_eq!(app.agent_marks.items.len(), 1);
    assert_eq!(app.agent_marks.items[0].range, (0..1).into());
    assert_eq!(app.agent_marks.items[0].snippet, "a");
    let tab = app.tab().unwrap();
    assert_eq!(tab.text, DOCUMENT);
    assert_eq!(tab.vim.mode, VimMode::Normal);
    assert_eq!(tab.vim.cursor, 0);
    assert_eq!(tab.selection, 0..0);
    assert_eq!(tab.vim.pending_command(), "");
    assert!(!tab.dirty);
    assert!(!tab.vim.can_undo());
    assert!(!app.navigation_pending());
    assert!(!app.agent_marks_modal_open());
    assert!(app.register.is_none());
}

#[test]
fn one_batch_leader_capture_before_f6_marks_the_original_pane() {
    let (app, left_pane, right_pane, left_id, _) = split_fixture();
    let mut screen = harness(app);
    assert_eq!(screen.state().current_pane_id(), left_pane);
    screen.input_mut().events.extend([
        egui::Event::Text(" a".into()),
        pressed(Key::F6, egui::Modifiers::NONE),
    ]);
    screen.run_steps(6);
    let app = screen.state();
    assert_eq!(app.current_pane_id(), right_pane);
    assert_eq!(app.agent_marks.items.len(), 1);
    let mark = &app.agent_marks.items[0];
    assert_eq!(mark.document_id, left_id);
    assert_eq!(mark.range, (0..1).into());
    assert_eq!(mark.snippet, "a");
    assert_eq!(app.tab().unwrap().selection, 0..2);
    assert!(app.projects[0].tabs.iter().all(|tab| !tab.dirty));
    assert!(!app.navigation_pending());
    assert!(!app.agent_marks_modal_open());
    assert!(app.register.is_none());
}

#[test]
fn deferred_capture_motion_is_discarded_when_cli_disables_vim_before_replay() {
    let mut screen = harness(fixture(VimMode::Visual));
    screen.input_mut().events.extend([
        egui::Event::Text("l".into()),
        capture_shortcut(),
        egui::Event::Text("h".into()),
    ]);
    screen.run_steps(1);
    assert_eq!(screen.state().agent_marks.items.len(), 1);
    assert_eq!(screen.state().agent_marks.items[0].snippet, "ab");
    assert_eq!(screen.state().tab().unwrap().text, DOCUMENT);
    assert_eq!(screen.state().tab().unwrap().selection, 0..2);
    let response = screen
        .state_mut()
        .control_request(crate::cli::Request::Settings {
            patch: Some(crate::cli::SettingsPatch {
                vim: Some(false),
                ..Default::default()
            }),
        });
    assert!(response.ok, "{response:?}");
    assert!(!screen.state().settings.vim);
    let selection = screen.state().tab().unwrap().selection.clone();
    let revision = screen.state().tab().unwrap().revision;
    screen.run_steps(4);
    let app = screen.state();
    let tab = app.tab().unwrap();
    assert_eq!(
        tab.text, DOCUMENT,
        "The queued Vim h must never become native text"
    );
    assert_eq!(tab.selection, selection);
    assert_eq!(tab.revision, revision);
    assert!(!tab.dirty);
    assert!(!tab.vim.can_undo());
    assert_eq!(app.agent_marks.items.len(), 1);
    assert_eq!(app.agent_marks.items[0].snippet, "ab");
    assert!(!app.agent_marks_modal_open());
    assert!(app.register.is_none());
}

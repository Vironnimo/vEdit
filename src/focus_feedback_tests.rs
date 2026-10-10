//! Keyboard focus and external navigation must preserve source state.
use super::*;
use egui_kittest::{Harness, kittest::Queryable};

fn fixture() -> VEditApp {
    let mut app = VEditApp::empty(egui::Context::default());
    app.settings.autosave = false;
    app.settings.smooth_scroll = false;
    let root = std::env::temp_dir().join("vedit-focus-feedback-render");
    let mut project = Project::new(root.clone());
    project.last_git_refresh = Instant::now();
    project.index = Some(Arc::new(workspace::WorkspaceIndex::empty(root.clone())));
    let mut tab = EditorTab::new(Some(root.join("notes.md")), "notes.md".into());
    tab.text = (1..=160)
        .map(|line| format!("Line {line} · Unicode 🦀 text\n"))
        .collect();
    tab.saved_text = tab.text.clone();
    project.tabs.push(tab);
    app.projects.push(project);
    app.bind_current_pane();
    app
}

fn harness(app: VEditApp) -> Harness<'static, VEditApp> {
    let mut screen = Harness::builder()
        .with_size([1200.0, 760.0])
        .with_os(egui::os::OperatingSystem::Windows)
        .build_ui_state(
            |ui, app: &mut VEditApp| {
                app.ctx = ui.ctx().clone();
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
    screen
}

fn shortcut(screen: &mut Harness<'_, VEditApp>, key: Key, modifiers: egui::Modifiers) {
    let modifiers = egui::Modifiers {
        command: modifiers.ctrl,
        ..modifiers
    };
    screen.event(egui::Event::ModifiersChanged(modifiers));
    screen.event(egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    });
    screen.run_steps(1);
    screen.event(egui::Event::Key {
        key,
        physical_key: None,
        pressed: false,
        repeat: false,
        modifiers,
    });
    screen.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    screen.run_steps(2);
}

fn distinct_region_buffers(left_insert: bool) -> (VEditApp, u64, u64, u64, u64) {
    let mut app = fixture();
    let left_pane = app.current_pane_id();
    let left_tab = app.tab_mut().unwrap();
    left_tab.path = None;
    left_tab.title = "left.txt".into();
    left_tab.text = "a\nsecond\n".into();
    left_tab.saved_text = left_tab.text.clone();
    let left_id = left_tab.id;
    let mut right_tab = EditorTab::new(None, "right.txt".into());
    right_tab.text = "b\nsecond\n".into();
    right_tab.saved_text = right_tab.text.clone();
    let right_id = right_tab.id;
    app.projects[0].tabs.push(right_tab);
    let right_pane = app
        .split_active(crate::cli::SplitOrientation::Vertical)
        .unwrap();
    for action in [
        crate::cli::LayoutAction::Assign {
            pane_id: right_pane,
            tab_id: right_id,
            view: Some(crate::cli::DocumentView::Source),
        },
        crate::cli::LayoutAction::Focus { pane_id: left_pane },
    ] {
        let response = app.control_request(crate::cli::Request::Layout { change: action });
        assert!(response.ok, "{response:?}");
    }
    for (id, insert) in [(left_id, left_insert), (right_id, true)] {
        let tab = app.tab_by_id_mut(id).unwrap();
        tab.set_cursor(0..0);
        if insert {
            let action = tab.vim.handle_key(&mut tab.text, VimKey::Char('i'));
            assert!(!action.text_changed);
            assert_eq!(tab.vim.mode, VimMode::Insert);
        }
        tab.focus = id == left_id;
        app.sync_external_document_navigation(id);
    }
    (app, left_pane, right_pane, left_id, right_id)
}

fn pressed(key: Key) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}

fn undo_focused_insert(screen: &mut Harness<'_, VEditApp>) {
    screen
        .input_mut()
        .events
        .extend([pressed(Key::Escape), egui::Event::Text("u".into())]);
    screen.run_steps(1);
}

#[test]
fn one_batch_f6_keeps_insert_prefix_and_suffix_in_their_own_panes_and_undo_histories() {
    let (app, left_pane, right_pane, left_id, right_id) = distinct_region_buffers(true);
    let mut screen = harness(app);
    assert_eq!(screen.state().current_pane_id(), left_pane);
    assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::Insert);
    screen.input_mut().events.extend([
        egui::Event::Text("OLD".into()),
        pressed(Key::F6),
        egui::Event::Text("NEW".into()),
    ]);
    screen.run_steps(1);
    assert_eq!(screen.state().projects[0].tabs[0].text, "OLDa\nsecond\n");
    assert_eq!(screen.state().projects[0].tabs[1].text, "b\nsecond\n");
    assert_eq!(screen.state().current_pane_id(), right_pane);
    screen.run_steps(1);
    assert_eq!(screen.state().projects[0].tabs[0].text, "OLDa\nsecond\n");
    assert_eq!(screen.state().projects[0].tabs[1].text, "NEWb\nsecond\n");
    assert_eq!(screen.state().tab().unwrap().id, right_id);
    assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::Insert);
    undo_focused_insert(&mut screen);
    assert_eq!(screen.state().projects[0].tabs[1].text, "b\nsecond\n");
    assert_eq!(screen.state().projects[0].tabs[0].text, "OLDa\nsecond\n");
    let response = screen
        .state_mut()
        .control_request(crate::cli::Request::Layout {
            change: crate::cli::LayoutAction::Focus { pane_id: left_pane },
        });
    assert!(response.ok, "{response:?}");
    screen.run_steps(1);
    assert_eq!(screen.state().tab().unwrap().id, left_id);
    screen.event(egui::Event::Text("u".into()));
    screen.run_steps(1);
    assert_eq!(screen.state().projects[0].tabs[0].text, "a\nsecond\n");
    assert_eq!(screen.state().projects[0].tabs[1].text, "b\nsecond\n");
    assert!(screen.state().projects[0].tabs.iter().all(|tab| !tab.dirty));
}

#[test]
fn one_batch_f6_keeps_search_query_prefix_out_of_the_destination_insert_buffer() {
    let (app, left_pane, right_pane, _, right_id) = distinct_region_buffers(false);
    let mut screen = harness(app);
    assert_eq!(screen.state().current_pane_id(), left_pane);
    screen.event(egui::Event::Text("/".into()));
    screen.run_steps(1);
    assert!(screen.state().vim_search_active());
    screen.input_mut().events.extend([
        egui::Event::Text("dd".into()),
        pressed(Key::F6),
        egui::Event::Text("X".into()),
    ]);
    screen.run_steps(1);
    assert!(!screen.state().vim_search_active());
    assert_eq!(screen.state().current_pane_id(), right_pane);
    assert_eq!(screen.state().projects[0].tabs[0].text, "a\nsecond\n");
    assert_eq!(screen.state().projects[0].tabs[1].text, "b\nsecond\n");
    assert!(!screen.state().projects[0].tabs[0].vim.can_undo());
    screen.run_steps(1);
    assert_eq!(screen.state().tab().unwrap().id, right_id);
    assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::Insert);
    assert_eq!(screen.state().projects[0].tabs[0].text, "a\nsecond\n");
    assert_eq!(screen.state().projects[0].tabs[1].text, "Xb\nsecond\n");
    assert!(screen.state().register.is_none());
    undo_focused_insert(&mut screen);
    assert_eq!(screen.state().projects[0].tabs[1].text, "b\nsecond\n");
    assert!(screen.state().projects[0].tabs.iter().all(|tab| !tab.dirty));
}

#[test]
fn deferred_f6_suffix_is_discarded_when_cli_focus_changes_its_destination() {
    let (app, left_pane, right_pane, left_id, _) = distinct_region_buffers(true);
    let mut screen = harness(app);
    screen.input_mut().events.extend([
        egui::Event::Text("OLD".into()),
        pressed(Key::F6),
        egui::Event::Text("NEW".into()),
    ]);
    screen.run_steps(1);
    assert_eq!(screen.state().current_pane_id(), right_pane);
    assert_eq!(screen.state().projects[0].tabs[0].text, "OLDa\nsecond\n");
    assert_eq!(screen.state().projects[0].tabs[1].text, "b\nsecond\n");
    let response = screen
        .state_mut()
        .control_request(crate::cli::Request::Layout {
            change: crate::cli::LayoutAction::Focus { pane_id: left_pane },
        });
    assert!(response.ok, "{response:?}");
    screen.run_steps(2);
    assert_eq!(screen.state().current_pane_id(), left_pane);
    assert_eq!(screen.state().tab().unwrap().id, left_id);
    assert_eq!(screen.state().projects[0].tabs[0].text, "OLDa\nsecond\n");
    assert_eq!(screen.state().projects[0].tabs[1].text, "b\nsecond\n");
    assert!(!screen.state().projects[0].tabs[1].dirty);
    assert!(screen.state().projects[0].tabs[0].vim.can_undo());
    screen.event(egui::Event::Text("u".into()));
    screen.run_steps(1);
    assert_eq!(screen.state().projects[0].tabs[0].text, "a\nsecond\n");
    assert_eq!(screen.state().projects[0].tabs[1].text, "b\nsecond\n");
    assert!(screen.state().projects[0].tabs.iter().all(|tab| !tab.dirty));
}

#[test]
fn focus_shortcuts_cycle_explorer_and_all_panes_without_touching_visual_draft() {
    let mut app = fixture();
    let left = app.current_pane_id();
    let right = app
        .split_active(crate::cli::SplitOrientation::Vertical)
        .unwrap();
    let tab = app.tab_mut().unwrap();
    tab.vim.set_selection(&tab.text, 8..15);
    tab.selection = 8..15;
    tab.pending_selection = Some(8..15);
    tab.dirty = true;
    let mut screen = harness(app);
    let text = screen.state().tab().unwrap().text.clone();
    shortcut(
        &mut screen,
        Key::E,
        egui::Modifiers::CTRL.plus(egui::Modifiers::SHIFT),
    );
    assert!(screen.state().explorer_keyboard_focused());
    shortcut(&mut screen, Key::F6, egui::Modifiers::NONE);
    assert!(!screen.state().explorer_keyboard_focused());
    assert_eq!(screen.state().current_pane_id(), left);
    shortcut(&mut screen, Key::F6, egui::Modifiers::NONE);
    assert_eq!(screen.state().current_pane_id(), right);
    shortcut(&mut screen, Key::F6, egui::Modifiers::NONE);
    assert!(screen.state().explorer_keyboard_focused());
    shortcut(&mut screen, Key::F6, egui::Modifiers::SHIFT);
    assert_eq!(screen.state().current_pane_id(), right);
    assert!(!screen.state().explorer_keyboard_focused());
    let tab = screen.state().tab().unwrap();
    assert_eq!(tab.text, text);
    assert_eq!(tab.vim.mode, VimMode::Visual);
    assert_eq!(tab.selection, 8..15);
    assert!(tab.dirty);
}

#[test]
fn explorer_focus_still_allows_global_file_palette_and_editor_focus_shortcut() {
    let mut screen = harness(fixture());
    shortcut(
        &mut screen,
        Key::E,
        egui::Modifiers::CTRL.plus(egui::Modifiers::SHIFT),
    );
    assert!(screen.state().explorer_keyboard_focused());
    shortcut(&mut screen, Key::P, egui::Modifiers::CTRL);
    assert!(matches!(screen.state().palette, Some(Palette::Files)));
    shortcut(&mut screen, Key::Escape, egui::Modifiers::NONE);
    assert!(screen.state().palette.is_none());
    shortcut(
        &mut screen,
        Key::E,
        egui::Modifiers::CTRL.plus(egui::Modifiers::SHIFT),
    );
    shortcut(&mut screen, Key::Num1, egui::Modifiers::CTRL);
    let tab = screen.state().tab().unwrap();
    assert!(
        screen
            .ctx
            .memory(|memory| memory.has_focus(screen.state().document_widget_id(tab.id)))
    );
}

#[test]
fn directional_focus_uses_horizontal_and_vertical_geometry_without_wrapping() {
    let mut app = fixture();
    let left = app.current_pane_id();
    let upper_right = app
        .split_active(crate::cli::SplitOrientation::Vertical)
        .unwrap();
    let lower_right = app
        .split_active(crate::cli::SplitOrientation::Horizontal)
        .unwrap();
    let mut screen = harness(app);
    let modifiers = egui::Modifiers::CTRL.plus(egui::Modifiers::ALT);
    shortcut(&mut screen, Key::ArrowUp, modifiers);
    assert_eq!(screen.state().current_pane_id(), upper_right);
    shortcut(&mut screen, Key::ArrowDown, modifiers);
    assert_eq!(screen.state().current_pane_id(), lower_right);
    shortcut(&mut screen, Key::ArrowLeft, modifiers);
    assert_eq!(screen.state().current_pane_id(), left);
    shortcut(&mut screen, Key::ArrowUp, modifiers);
    assert_eq!(screen.state().current_pane_id(), left);
    shortcut(&mut screen, Key::ArrowLeft, modifiers);
    assert!(screen.state().explorer_keyboard_focused());
    for key in [Key::ArrowLeft, Key::ArrowUp, Key::ArrowDown] {
        shortcut(&mut screen, key, modifiers);
        assert!(screen.state().explorer_keyboard_focused());
    }
    shortcut(&mut screen, Key::ArrowRight, modifiers);
    assert!(!screen.state().explorer_keyboard_focused());
    assert_eq!(screen.state().current_pane_id(), left);
}

fn yank_feedback_rects(screen: &Harness<'_, VEditApp>) -> Vec<egui::Rect> {
    screen
        .output()
        .shapes
        .iter()
        .filter_map(|shape| {
            let egui::Shape::Rect(rect) = &shape.shape else {
                return None;
            };
            let [red, green, blue, alpha] = rect.fill.to_srgba_unmultiplied();
            (rect.corner_radius == egui::CornerRadius::same(2)
                && alpha > 0
                && alpha <= 48
                && red.abs_diff(ACCENT.r()) < 10
                && green.abs_diff(ACCENT.g()) < 10
                && blue.abs_diff(ACCENT.b()) < 10)
                .then_some(rect.rect)
        })
        .collect()
}

#[test]
fn yy_feedback_covers_the_whole_final_and_empty_line_without_modifying_text() {
    for text in ["final 🦀 line", ""] {
        let mut app = fixture();
        let tab = app.tab_mut().unwrap();
        tab.text = text.into();
        tab.saved_text = text.into();
        let mut screen = harness(app);
        screen.event(egui::Event::Text("yy".into()));
        screen.run_steps(1);
        let viewport = screen.state().editor_rect.unwrap();
        let highlights = yank_feedback_rects(&screen);
        assert!(
            highlights
                .iter()
                .any(|rect| rect.width() > viewport.width() * 0.85),
            "{text:?}: {highlights:?}, viewport={viewport:?}"
        );
        assert_eq!(screen.state().tab().unwrap().text, text);
        assert!(!screen.state().tab().unwrap().dirty);
        assert!(!screen.state().tab().unwrap().vim.can_undo());
    }
}

#[test]
fn character_yank_feedback_is_exact_and_disappears_after_a_new_revision() {
    let mut app = fixture();
    let tab = app.tab_mut().unwrap();
    tab.text = "αβ🦀 long source line".into();
    tab.saved_text = tab.text.clone();
    let mut screen = harness(app);
    screen.event(egui::Event::Text("vlly".into()));
    screen.run_steps(1);
    let highlights = yank_feedback_rects(&screen);
    assert_eq!(highlights.len(), 1, "{highlights:?}");
    assert!(highlights[0].width() > 15.0 && highlights[0].width() < 80.0);
    assert_eq!(screen.state().register.as_ref().unwrap().text, "αβ🦀");
    screen.event(egui::Event::Text("iX".into()));
    screen.run_steps(1);
    assert!(yank_feedback_rects(&screen).is_empty());
    assert!(screen.state().tab().unwrap().text.contains('X'));
}

#[test]
fn search_result_reveals_unicode_location_in_the_middle_and_emits_feedback() {
    let mut screen = harness(fixture());
    let path = screen.state().tab().unwrap().path.clone().unwrap();
    screen.state_mut().reveal_search_result(path, 80, 15);
    screen.run_steps(4);
    let app = screen.state();
    let tab = app.tab().unwrap();
    assert_eq!(
        tab.lines.line_col(&tab.text, tab.revision, tab.vim.cursor),
        (80, 15)
    );
    assert!(tab.feedback.is_some());
    assert!(!tab.pending_navigation_feedback);
    assert!(!tab.dirty);
    assert!(!tab.vim.can_undo());
    let viewport = app.editor_rect.unwrap();
    let caret = app.editor_cursor_rect.unwrap();
    assert!(
        (caret.center().y - viewport.center().y).abs() < 28.0,
        "viewport={viewport:?}, caret={caret:?}"
    );
}

#[test]
fn source_line_totals_count_crlf_empty_lines_and_trailing_newline_without_wrap_rows() {
    let lines = crate::render::LineIndex::default();
    for (text, expected) in [
        ("", 1),
        ("one", 1),
        ("one\n", 2),
        ("one\r\n\r\nlast", 3),
        ("🦀\n\n", 3),
    ] {
        assert_eq!(lines.line_count(text, text.len() as u64), expected);
    }
    let mut app = fixture();
    app.settings.word_wrap = true;
    let mut screen = harness(app);
    assert!(screen.get_by_label("Ln 1/161, Col 1").rect().width() > 0.0);
    screen.state_mut().tab_mut().unwrap().text.push_str("last");
    screen.state_mut().tab_mut().unwrap().changed();
    screen.run_steps(2);
    assert_eq!(
        screen.state().tab().unwrap().lines.line_count(
            &screen.state().tab().unwrap().text,
            screen.state().tab().unwrap().revision
        ),
        161
    );
}

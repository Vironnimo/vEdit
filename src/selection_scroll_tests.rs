//! Native pointer selections must follow the viewport without replacing their anchor.

use super::*;
use egui_kittest::{Harness, kittest::Queryable};

#[derive(Clone, Copy, Debug)]
enum Editing {
    Normal,
    Insert,
    Native,
}

fn fixture(editing: Editing, smooth: bool, wrap: bool) -> VEditApp {
    let mut app = VEditApp::empty(egui::Context::default());
    app.settings.autosave = false;
    app.settings.smooth_scroll = smooth;
    app.settings.word_wrap = wrap;
    app.settings.vim = !matches!(editing, Editing::Native);
    app.sidebar_visible = false;
    let root = std::env::temp_dir().join("vedit-selection-scroll-in-memory");
    let mut project = Project::new(root.clone());
    project.last_git_refresh = Instant::now();
    project.index = Some(Arc::new(workspace::WorkspaceIndex::empty(root.clone())));
    let mut tab = EditorTab::new(Some(root.join("wide.txt")), "wide.txt".into());
    tab.text = (1..=250)
        .map(|line| format!("Line {line:03} · café Ω 🦀 {}\n", "wide words ".repeat(24)))
        .collect();
    tab.saved_text = tab.text.clone();
    if matches!(editing, Editing::Insert) {
        tab.vim.handle_key(&mut tab.text, VimKey::Char('i'));
    }
    project.tabs.push(tab);
    app.projects.push(project);
    app.bind_current_pane();
    app
}

fn harness(app: VEditApp) -> Harness<'static, VEditApp> {
    let mut screen = Harness::builder()
        .with_size([1200.0, 760.0])
        .with_step_dt(1.0 / 60.0)
        .with_os(egui::os::OperatingSystem::Windows)
        .build_ui_state(
            |ui, app: &mut VEditApp| {
                app.ctx = ui.ctx().clone();
                app.last_disk_check = Instant::now();
                app.last_session_save = Instant::now();
                app.session_dirty = false;
                for project in &mut app.projects {
                    project.last_git_refresh = Instant::now();
                }
                app.draw_ui(ui);
            },
            app,
        );
    set_style(&screen.ctx);
    screen.ctx.all_styles_mut(|style| {
        style.animation_time = 0.0;
        style.visuals.text_cursor.blink = false;
        // Leave scroll_animation intact: smooth scrolling must not delay a drag.
    });
    screen.run_steps(3);
    let tab = screen.state().tab().unwrap();
    screen.ctx.data_mut(|data| {
        data.insert_temp(
            egui::Id::new(("selection_scroll_initial_history", tab.id)),
            (tab.vim.can_undo(), tab.vim.can_redo()),
        );
    });
    screen
}

fn point_at_character(screen: &Harness<'_, VEditApp>, character: usize) -> egui::Pos2 {
    let viewport = screen.state().editor_rect.unwrap();
    let text = &screen.state().tab().unwrap().text;
    let document = screen
        .output()
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::epaint::Shape::Text(text_shape)
                if text_shape.galley.text() == text
                    && viewport.contains(shape.clip_rect.center()) =>
            {
                Some(text_shape)
            }
            _ => None,
        })
        .expect("the focused source document is painted");
    let caret = document
        .galley
        .pos_from_cursor(egui::text::CCursor::new(character));
    document.pos + egui::vec2(caret.left() + 1.0, caret.center().y)
}

fn character_at_line(screen: &Harness<'_, VEditApp>, line: usize, column: usize) -> usize {
    editor::go_to_line(&screen.state().tab().unwrap().text, line) + column
}

fn pointer_button(screen: &mut Harness<'_, VEditApp>, point: egui::Pos2, pressed: bool) {
    screen.input_mut().events.extend([
        egui::Event::PointerMoved(point),
        egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        },
    ]);
    screen.run_steps(1);
}

fn move_pointer(screen: &mut Harness<'_, VEditApp>, point: egui::Pos2) {
    screen
        .input_mut()
        .events
        .push(egui::Event::PointerMoved(point));
    screen.run_steps(1);
}

fn scroll_offset(screen: &Harness<'_, VEditApp>) -> egui::Vec2 {
    let app = screen.state();
    app.find.view_scroll[&(app.current_pane_id(), app.tab().unwrap().id)].1
}

fn native_range(screen: &Harness<'_, VEditApp>) -> egui::text::CCursorRange {
    let app = screen.state();
    let id = app.document_widget_id(app.tab().unwrap().id);
    egui::TextEdit::load_state(&screen.ctx, id)
        .unwrap()
        .cursor
        .char_range()
        .unwrap()
}

fn begin_selection(screen: &mut Harness<'_, VEditApp>, character: usize) -> usize {
    let point = point_at_character(screen, character);
    assert!(screen.state().editor_rect.unwrap().contains(point));
    pointer_button(screen, point, true);
    let anchor = native_range(screen).secondary.index.0;
    assert_eq!(anchor, character);
    anchor
}

fn assert_unchanged_document(screen: &Harness<'_, VEditApp>) {
    let tab = screen.state().tab().unwrap();
    assert_eq!(tab.text, tab.saved_text);
    assert!(!tab.dirty);
    let initial_history = screen.ctx.data(|data| {
        data.get_temp::<(bool, bool)>(egui::Id::new(("selection_scroll_initial_history", tab.id)))
            .unwrap()
    });
    assert_eq!((tab.vim.can_undo(), tab.vim.can_redo()), initial_history);
    let (id, offset) = screen.state().find.view_scroll[&(screen.state().current_pane_id(), tab.id)];
    let maximum = screen.ctx.data(|data| {
        data.get_temp::<egui::Vec2>(id.with("document_scroll_limit"))
            .unwrap()
    });
    assert!(offset.x >= 0.0 && offset.y >= 0.0);
    assert!(offset.x <= maximum.x + 0.5 && offset.y <= maximum.y + 0.5);
}

#[test]
fn stationary_bottom_drag_scrolls_and_extends_selection_in_every_editing_mode() {
    for smooth in [false, true] {
        for editing in [Editing::Normal, Editing::Insert, Editing::Native] {
            let mut screen = harness(fixture(editing, smooth, false));
            let start = character_at_line(&screen, 8, 24);
            let anchor = begin_selection(&mut screen, start);
            let viewport = screen.state().editor_rect.unwrap();
            let target = egui::pos2(viewport.center().x, viewport.bottom() + 18.0);
            move_pointer(&mut screen, target);
            let first_offset = scroll_offset(&screen);
            let first_end = native_range(&screen).primary.index.0;
            screen.run_steps(25);
            let later_offset = scroll_offset(&screen);
            let later_range = native_range(&screen);
            assert!(
                later_offset.y > first_offset.y + 50.0,
                "{editing:?}, smooth={smooth}"
            );
            assert_eq!(later_offset.x, 0.0);
            assert!(later_range.primary.index.0 > first_end);
            assert_eq!(later_range.secondary.index.0, anchor);
            assert_eq!(screen.state().tab().unwrap().selection.start, anchor);
            assert_unchanged_document(&screen);
            pointer_button(&mut screen, target, false);
        }
    }
}

#[test]
fn stationary_right_drag_scrolls_and_extends_an_unwrapped_unicode_selection() {
    for smooth in [false, true] {
        let mut screen = harness(fixture(Editing::Normal, smooth, false));
        let start = character_at_line(&screen, 6, 20);
        let anchor = begin_selection(&mut screen, start);
        let viewport = screen.state().editor_rect.unwrap();
        let start_point = point_at_character(&screen, start);
        let target = egui::pos2(viewport.right() + 18.0, start_point.y);
        move_pointer(&mut screen, target);
        let first_offset = scroll_offset(&screen);
        let first_end = native_range(&screen).primary.index.0;
        screen.run_steps(25);
        let later = native_range(&screen);
        assert!(
            scroll_offset(&screen).x > first_offset.x + 50.0,
            "smooth={smooth}"
        );
        assert_eq!(scroll_offset(&screen).y, 0.0);
        assert!(later.primary.index.0 > first_end);
        assert_eq!(later.secondary.index.0, anchor);
        let selected: String = screen
            .state()
            .tab()
            .unwrap()
            .text
            .chars()
            .skip(anchor)
            .take(later.primary.index.0 - anchor)
            .collect();
        assert!(selected.starts_with("wide words"));
        assert_unchanged_document(&screen);
        pointer_button(&mut screen, target, false);
    }
}

#[test]
fn reversing_up_and_left_returns_through_the_original_anchor_without_resetting_it() {
    for smooth in [false, true] {
        let mut screen = harness(fixture(Editing::Normal, smooth, false));
        let start = character_at_line(&screen, 8, 24);
        let anchor = begin_selection(&mut screen, start);
        let viewport = screen.state().editor_rect.unwrap();
        let forward = viewport.right_bottom() + egui::vec2(18.0, 18.0);
        move_pointer(&mut screen, forward);
        screen.run_steps(30);
        assert!(scroll_offset(&screen).x > 100.0 && scroll_offset(&screen).y > 100.0);
        assert!(native_range(&screen).primary.index.0 > anchor);
        let reverse = viewport.left_top() - egui::vec2(18.0, 18.0);
        move_pointer(&mut screen, reverse);
        screen.run_steps(65);
        let range = native_range(&screen);
        assert_eq!(scroll_offset(&screen), egui::Vec2::ZERO, "smooth={smooth}");
        assert_eq!(range.secondary.index.0, anchor);
        assert!(range.primary.index.0 < anchor);
        assert_eq!(screen.state().tab().unwrap().selection.end, anchor);
        assert_unchanged_document(&screen);
        pointer_button(&mut screen, reverse, false);
        screen.run_steps(2);
        assert_eq!(screen.state().tab().unwrap().selection.end, anchor);
    }
}

#[test]
fn moving_inside_and_releasing_stop_autoscroll_without_smooth_residual_motion() {
    for smooth in [false, true] {
        let mut screen = harness(fixture(Editing::Normal, smooth, false));
        let start = character_at_line(&screen, 8, 24);
        let anchor = begin_selection(&mut screen, start);
        let viewport = screen.state().editor_rect.unwrap();
        let outside = viewport.right_bottom() + egui::vec2(18.0, 18.0);
        move_pointer(&mut screen, outside);
        screen.run_steps(16);
        move_pointer(&mut screen, viewport.center());
        screen.run_steps(2);
        let inside_offset = scroll_offset(&screen);
        assert!(inside_offset.x > 0.0 && inside_offset.y > 0.0);
        screen.run_steps(10);
        assert_eq!(
            scroll_offset(&screen),
            inside_offset,
            "inside, smooth={smooth}"
        );
        assert_eq!(native_range(&screen).secondary.index.0, anchor);
        move_pointer(&mut screen, outside);
        screen.run_steps(10);
        assert!(scroll_offset(&screen).x > inside_offset.x);
        assert!(scroll_offset(&screen).y > inside_offset.y);
        pointer_button(&mut screen, outside, false);
        screen.run_steps(2);
        let released_offset = scroll_offset(&screen);
        let released_range = native_range(&screen);
        screen.run_steps(10);
        assert_eq!(
            scroll_offset(&screen),
            released_offset,
            "released, smooth={smooth}"
        );
        assert_eq!(native_range(&screen), released_range);
        assert_unchanged_document(&screen);
    }
}

#[test]
fn hovering_and_dragging_a_scrollbar_do_not_create_or_extend_a_source_selection() {
    let mut screen = harness(fixture(Editing::Normal, true, false));
    let viewport = screen.state().editor_rect.unwrap();
    let outside = viewport.right_bottom() + egui::vec2(18.0, 18.0);
    move_pointer(&mut screen, outside);
    screen.run_steps(20);
    assert_eq!(scroll_offset(&screen), egui::Vec2::ZERO);
    assert_eq!(screen.state().tab().unwrap().selection, 0..0);
    let bar = screen
        .query_all_by_role(egui::accesskit::Role::ScrollBar)
        .map(|node| node.rect())
        .find(|rect| rect.width() <= 20.0 && rect.height() > 100.0)
        .expect("the source view exposes its vertical scrollbar");
    let press = egui::pos2(bar.center().x, bar.top() + 12.0);
    pointer_button(&mut screen, press, true);
    let bottom = egui::pos2(bar.center().x, bar.bottom() - 12.0);
    move_pointer(&mut screen, bottom);
    screen.run_steps(2);
    let offset = scroll_offset(&screen);
    screen.run_steps(15);
    assert_eq!(scroll_offset(&screen), offset);
    assert_eq!(screen.state().tab().unwrap().selection, 0..0);
    assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::Normal);
    assert_unchanged_document(&screen);
    pointer_button(&mut screen, bottom, false);
}

#[test]
fn wrapped_source_autoscrolls_vertically_and_never_horizontally() {
    for smooth in [false, true] {
        let mut screen = harness(fixture(Editing::Normal, smooth, true));
        let anchor = begin_selection(&mut screen, 24);
        let viewport = screen.state().editor_rect.unwrap();
        let target = viewport.right_bottom() + egui::vec2(18.0, 18.0);
        move_pointer(&mut screen, target);
        let first = native_range(&screen).primary.index.0;
        screen.run_steps(25);
        assert!(scroll_offset(&screen).y > 50.0);
        assert_eq!(scroll_offset(&screen).x, 0.0);
        let range = native_range(&screen);
        assert!(range.primary.index.0 > first);
        assert_eq!(range.secondary.index.0, anchor);
        assert_unchanged_document(&screen);
        pointer_button(&mut screen, target, false);
    }
}

#[test]
fn dragging_out_of_one_split_scrolls_only_its_owned_view_of_the_shared_document() {
    let mut app = fixture(Editing::Normal, true, false);
    let left = app.current_pane_id();
    let document = app.tab().unwrap().id;
    let right = app
        .split_active(crate::cli::SplitOrientation::Vertical)
        .unwrap();
    let response = app.control_request(crate::cli::Request::Layout {
        change: crate::cli::LayoutAction::Focus { pane_id: left },
    });
    assert!(response.ok, "{response:?}");
    let mut screen = harness(app);
    let initial_right = screen.state().find.view_scroll[&(right, document)].1;
    let right_widget = egui::Id::new(("document", document, right));
    let right_range = egui::TextEdit::load_state(&screen.ctx, right_widget)
        .unwrap()
        .cursor
        .char_range()
        .unwrap();
    let start = character_at_line(&screen, 8, 24);
    let anchor = begin_selection(&mut screen, start);
    let viewport = screen.state().editor_rect.unwrap();
    let target = viewport.right_bottom() + egui::vec2(18.0, 18.0);
    move_pointer(&mut screen, target);
    screen.run_steps(25);
    assert_eq!(screen.state().current_pane_id(), left);
    assert!(scroll_offset(&screen).x > 50.0 && scroll_offset(&screen).y > 50.0);
    assert_eq!(
        screen.state().find.view_scroll[&(right, document)].1,
        initial_right
    );
    assert_eq!(
        egui::TextEdit::load_state(&screen.ctx, right_widget)
            .unwrap()
            .cursor
            .char_range()
            .unwrap(),
        right_range
    );
    assert_eq!(native_range(&screen).secondary.index.0, anchor);
    assert_unchanged_document(&screen);
    pointer_button(&mut screen, target, false);
}

#[test]
fn a_modal_opened_during_a_drag_suspends_source_autoscroll() {
    let mut screen = harness(fixture(Editing::Normal, true, false));
    let start = character_at_line(&screen, 8, 24);
    begin_selection(&mut screen, start);
    let viewport = screen.state().editor_rect.unwrap();
    let target = viewport.right_bottom() + egui::vec2(18.0, 18.0);
    move_pointer(&mut screen, target);
    screen.run_steps(12);
    screen.state_mut().open_palette(Palette::Commands);
    screen.run_steps(2);
    let modal_offset = scroll_offset(&screen);
    screen.run_steps(15);
    assert!(screen.state().palette.is_some());
    assert_eq!(scroll_offset(&screen), modal_offset);
    assert_unchanged_document(&screen);
    pointer_button(&mut screen, target, false);
}

#[test]
fn capturing_a_selection_drag_cancels_an_existing_animated_cursor_reveal() {
    let mut screen = harness(fixture(Editing::Normal, true, false));
    let destination = character_at_line(&screen, 180, 20);
    screen
        .state_mut()
        .tab_mut()
        .unwrap()
        .set_cursor(destination..destination);
    screen.run_steps(1);
    let app = screen.state();
    let scroll_id = app.find.view_scroll[&(app.current_pane_id(), app.tab().unwrap().id)].0;
    let animated_state =
        serde_json::to_value(egui::scroll_area::State::load(&screen.ctx, scroll_id).unwrap())
            .unwrap();
    assert!(
        animated_state["offset_target"]
            .as_array()
            .unwrap()
            .iter()
            .any(|target| !target.is_null()),
        "the fixture must start with a real animated reveal, not a settled scroll"
    );
    let viewport = screen.state().editor_rect.unwrap();
    let press = viewport.center();
    pointer_button(&mut screen, press, true);
    let inside = press + egui::vec2(30.0, 0.0);
    move_pointer(&mut screen, inside);
    screen.run_steps(2);
    let captured_offset = scroll_offset(&screen);
    let captured_anchor = native_range(&screen).secondary.index.0;
    screen.run_steps(10);
    assert_eq!(scroll_offset(&screen), captured_offset);
    assert_eq!(native_range(&screen).secondary.index.0, captured_anchor);
    pointer_button(&mut screen, inside, false);
    screen.run_steps(2);
    let released_offset = scroll_offset(&screen);
    screen.run_steps(25);
    assert_eq!(
        scroll_offset(&screen),
        released_offset,
        "the reveal queued before the drag must not resume after release"
    );
    assert_unchanged_document(&screen);
}

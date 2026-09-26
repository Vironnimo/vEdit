//! Actual pointer and keyboard input must keep user selections separate from marks.
use super::*;
use egui_kittest::{Harness, kittest::Queryable};

const DOCUMENT: &str = "before: target after\nSecond block · β🦀\nThird block for the agent\n";

fn fixture(vim: bool) -> MorrowApp {
    let mut app = MorrowApp::empty(egui::Context::default());
    app.settings.autosave = false;
    app.settings.vim = vim;
    app.sidebar_visible = false;
    let root = std::env::temp_dir().join("morrow-agent-marks-render-only");
    let mut project = Project::new(root.clone());
    project.index = Some(Arc::new(workspace::WorkspaceIndex::empty(root.clone())));
    project.last_git_refresh = Instant::now();
    let mut tab = EditorTab::new(Some(root.join("plan.md")), "plan.md".into());
    tab.text = DOCUMENT.into();
    tab.saved_text = DOCUMENT.into();
    project.tabs.push(tab);
    app.projects.push(project);
    app.bind_current_pane();
    app
}

fn harness(app: MorrowApp, size: [f32; 2]) -> Harness<'static, MorrowApp> {
    let mut setup = egui_wgpu::WgpuSetup::without_display_handle();
    if let egui_wgpu::WgpuSetup::CreateNew(options) = &mut setup {
        options.instance_descriptor.backends = egui_wgpu::wgpu::Backends::GL;
    }
    let mut styled = false;
    let mut screen = Harness::builder()
        .with_size(size)
        .with_os(egui::os::OperatingSystem::Windows)
        .wgpu_setup(setup)
        .build_ui_state(
            move |ui, app: &mut MorrowApp| {
                app.ctx = ui.ctx().clone();
                if !styled {
                    set_style(&app.ctx);
                    app.ctx.all_styles_mut(|style| {
                        style.visuals.text_cursor.blink = false;
                        style.animation_time = 0.0;
                        style.scroll_animation = egui::style::ScrollAnimation::none();
                    });
                    styled = true;
                }
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
    screen.run_steps(4);
    screen
}

fn source_shape(screen: &Harness<'_, MorrowApp>) -> egui::epaint::TextShape {
    let current_text = &screen.state().tab().unwrap().text;
    screen
        .output()
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::epaint::Shape::Text(shape) if shape.galley.text() == current_text.as_str() => {
                Some(shape.clone())
            }
            _ => None,
        })
        .expect("The actual source document must be painted")
}

fn character_point(screen: &Harness<'_, MorrowApp>, character: usize) -> egui::Pos2 {
    let shape = source_shape(screen);
    let cursor = shape
        .galley
        .pos_from_cursor(egui::text::CCursor::new(character));
    shape.pos + egui::vec2(cursor.left() + 1.0, cursor.center().y)
}

fn select(screen: &mut Harness<'_, MorrowApp>, start: usize, end: usize) {
    let from = character_point(screen, start);
    let to = character_point(screen, end);
    screen.hover_at(from);
    screen.drag_at(from);
    screen.run_steps(1);
    screen.hover_at(to);
    screen.run_steps(1);
    screen.drop_at(to);
    screen.run_steps(3);
    assert_eq!(
        screen.state().tab().unwrap().selection,
        start.min(end)..start.max(end)
    );
}

fn key(screen: &mut Harness<'_, MorrowApp>, key: Key, modifiers: egui::Modifiers) {
    screen.input_mut().events.extend([
        egui::Event::ModifiersChanged(modifiers),
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        },
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers,
        },
        egui::Event::ModifiersChanged(egui::Modifiers::NONE),
    ]);
    screen.run_steps(3);
}

fn add_selection(screen: &mut Harness<'_, MorrowApp>) {
    key(
        screen,
        Key::A,
        egui::Modifiers::COMMAND.plus(egui::Modifiers::SHIFT),
    );
}

fn tab_to(screen: &mut Harness<'_, MorrowApp>, label: &str) {
    for _ in 0..40 {
        let node = if label.ends_with('·') {
            screen.query_by_label_contains(label)
        } else {
            screen.query_by_label(label)
        };
        if node.is_some_and(|node| node.is_focused()) {
            return;
        }
        key(screen, Key::Tab, egui::Modifiers::NONE);
    }
    panic!("Keyboard Tab must reach the enabled action: {label}");
}

fn original_selection(
    screen: &Harness<'_, MorrowApp>,
) -> (Range<usize>, usize, VimMode, String, u64) {
    let tab = screen.state().tab().unwrap();
    (
        tab.selection.clone(),
        tab.vim.cursor,
        tab.vim.mode,
        tab.text.clone(),
        tab.revision,
    )
}

#[test]
fn capture_and_inline_remove_preserve_native_and_vim_drag_selection() {
    for vim in [false, true] {
        let mut screen = harness(fixture(vim), [1200.0, 900.0]);
        select(&mut screen, 14, 8);
        let original = original_selection(&screen);
        add_selection(&mut screen);
        assert_eq!(screen.state().agent_marks.items.len(), 1);
        assert_eq!(screen.state().agent_marks.items[0].snippet, "target");
        assert_eq!(original_selection(&screen), original);
        let id = screen.state().agent_marks.items[0].id;
        screen
            .get_by_label(&format!("Remove marked block {id}"))
            .click();
        screen.run_steps(3);
        assert!(screen.state().agent_marks.items.is_empty());
        assert_eq!(
            original_selection(&screen),
            original,
            "Deleting a mark must not collapse the native or Vim selection"
        );
        assert!(!screen.state().tab().unwrap().dirty);
        let widget = screen
            .state()
            .document_widget_id(screen.state().tab().unwrap().id);
        assert_eq!(screen.ctx.memory(|memory| memory.focused()), Some(widget));
    }
}

#[test]
fn inline_remove_stays_visible_and_keeps_selection_after_horizontal_scroll() {
    let mut app = fixture(true);
    let tab = app.tab_mut().unwrap();
    tab.text.push_str(&"long_unwrapped_content_".repeat(150));
    tab.saved_text = tab.text.clone();
    let mut screen = harness(app, [680.0, 650.0]);
    select(&mut screen, 8, 14);
    add_selection(&mut screen);
    let original = original_selection(&screen);
    let id = screen.state().agent_marks.items[0].id;
    let label = format!("Remove marked block {id}");
    let before = screen.get_by_label(&label).rect();
    let viewport = screen.state().editor_rect.unwrap();
    screen.hover_at(viewport.left_top() + Vec2::new(250.0, 20.0));
    screen.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: Vec2::new(-420.0, 0.0),
        modifiers: egui::Modifiers::NONE,
        phase: egui::TouchPhase::Move,
    });
    screen.run_steps(4);
    let app = screen.state();
    let offset = app.find.view_scroll[&(app.current_pane_id(), app.tab().unwrap().id)].1;
    assert!(
        offset.x > 300.0,
        "The fixture must actually scroll horizontally"
    );
    assert_eq!(
        screen.get_by_label(&label).rect(),
        before,
        "The gutter action must stay visible even when the marked text is offscreen horizontally"
    );
    screen.get_by_label(&label).click();
    screen.run_steps(4);
    assert!(screen.state().agent_marks.items.is_empty());
    assert_eq!(original_selection(&screen), original);
}

#[test]
fn collection_clear_restore_and_notes_are_real_user_actions() {
    let mut screen = harness(fixture(true), [1200.0, 900.0]);
    select(&mut screen, 8, 14);
    add_selection(&mut screen);
    select(&mut screen, 21, 33);
    add_selection(&mut screen);
    assert_eq!(screen.state().agent_marks.items.len(), 2);
    let original = original_selection(&screen);
    let id = screen.state().agent_marks.items[0].id;
    screen.get_by_label("For agent").click();
    screen.run_steps(4);
    assert!(screen.state().agent_marks_modal_open());
    screen
        .get_by_label_contains(&format!("Edit note for mark {id} ·"))
        .click();
    screen.run_steps(4);
    screen.event(egui::Event::Text("Please simplify this block".into()));
    screen.run_steps(2);
    key(&mut screen, Key::Enter, egui::Modifiers::NONE);
    assert_eq!(
        screen
            .state()
            .agent_marks
            .items
            .iter()
            .find(|mark| mark.id == id)
            .unwrap()
            .note,
        "Please simplify this block"
    );
    assert!(screen.state().agent_marks_modal_open());
    screen.get_by_label("Clear all").click();
    screen.run_steps(4);
    assert!(screen.state().agent_marks.items.is_empty());
    assert_eq!(original_selection(&screen), original);
    screen.get_by_label("Restore last").click();
    screen.run_steps(4);
    assert_eq!(screen.state().agent_marks.items.len(), 2);
    assert_eq!(
        screen
            .state()
            .agent_marks
            .items
            .iter()
            .find(|mark| mark.id == id)
            .unwrap()
            .note,
        "Please simplify this block"
    );
    key(&mut screen, Key::Escape, egui::Modifiers::NONE);
    assert!(!screen.state().agent_marks_modal_open());
    assert_eq!(original_selection(&screen), original);
}

#[test]
fn keyboard_tab_and_enter_activate_collection_actions_without_opening_a_mark() {
    let mut screen = harness(fixture(true), [1200.0, 900.0]);
    select(&mut screen, 8, 14);
    add_selection(&mut screen);
    select(&mut screen, 21, 37);
    add_selection(&mut screen);
    let original = original_selection(&screen);
    let id = screen.state().agent_marks.items[0].id;
    screen.get_by_label("For agent").click();
    screen.run_steps(4);
    tab_to(&mut screen, "Clear all");
    key(&mut screen, Key::Enter, egui::Modifiers::NONE);
    assert!(screen.state().agent_marks_modal_open());
    assert!(screen.state().agent_marks.items.is_empty());
    tab_to(&mut screen, "Restore last");
    key(&mut screen, Key::Enter, egui::Modifiers::NONE);
    assert!(screen.state().agent_marks_modal_open());
    assert_eq!(screen.state().agent_marks.items.len(), 2);
    tab_to(&mut screen, &format!("Edit note for mark {id} ·"));
    key(&mut screen, Key::Enter, egui::Modifiers::NONE);
    assert!(screen.state().agent_marks_ui.editing.is_some());
    screen.event(egui::Event::Text("Unsaved draft note".into()));
    screen.run_steps(2);
    tab_to(&mut screen, "Cancel");
    key(&mut screen, Key::Enter, egui::Modifiers::NONE);
    assert!(screen.state().agent_marks_modal_open());
    assert!(screen.state().agent_marks_ui.editing.is_none());
    assert!(
        screen
            .state()
            .agent_marks
            .items
            .iter()
            .find(|mark| mark.id == id)
            .unwrap()
            .note
            .is_empty(),
        "Enter on Cancel must cancel the draft, rather than save it"
    );
    tab_to(&mut screen, &format!("Remove mark {id} ·"));
    key(&mut screen, Key::Enter, egui::Modifiers::NONE);
    assert!(screen.state().agent_marks_modal_open());
    assert_eq!(screen.state().agent_marks.items.len(), 1);
    assert_eq!(original_selection(&screen), original);
}

#[test]
fn collection_filter_searches_later_lines_and_long_notes_without_changing_excerpt() {
    let mut app = fixture(false);
    let text = format!("{}\nLATER_LINE_NEEDLE · β🦀\n", "prefix".repeat(70));
    let tab = app.tab_mut().unwrap();
    tab.text = text.clone();
    tab.saved_text = text.clone();
    let mut screen = harness(app, [1200.0, 900.0]);
    key(&mut screen, Key::A, egui::Modifiers::COMMAND);
    add_selection(&mut screen);
    assert_eq!(screen.state().agent_marks.items.len(), 1);
    let id = screen.state().agent_marks.items[0].id;
    let note = format!("{} LONG_NOTE_NEEDLE", "purpose ".repeat(35));
    screen.state_mut().agent_marks_set_note(id, &note);
    screen.get_by_label("For agent").click();
    screen.run_steps(4);
    screen.event(egui::Event::Text("later_line_needle".into()));
    screen.run_steps(4);
    assert_eq!(screen.state().agent_marks_ui.filter.matches, vec![0]);
    assert!(screen.query_by_label_contains("Marked block ").is_some());
    assert_eq!(screen.state().agent_marks.items[0].snippet, text);
    key(&mut screen, Key::A, egui::Modifiers::COMMAND);
    screen.event(egui::Event::Text("long_note_needle".into()));
    screen.run_steps(4);
    assert_eq!(screen.state().agent_marks_ui.filter.matches, vec![0]);
    assert!(screen.query_by_label_contains("Marked block ").is_some());
    assert_eq!(screen.state().agent_marks.items[0].note, note);
}

#[test]
fn collection_outside_click_closes_without_moving_or_editing_source() {
    for vim in [false, true] {
        let mut screen = harness(fixture(vim), [1200.0, 900.0]);
        select(&mut screen, 8, 14);
        add_selection(&mut screen);
        let original = original_selection(&screen);
        screen.get_by_label("For agent").click();
        screen.run_steps(4);
        let point = egui::pos2(15.0, 120.0);
        screen.hover_at(point);
        screen.event(egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        });
        screen.event(egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        screen.run_steps(4);
        assert!(!screen.state().agent_marks_modal_open());
        assert_eq!(original_selection(&screen), original);
        assert_eq!(screen.state().agent_marks.items.len(), 1);
    }
}

#[test]
fn purple_marks_follow_actual_wrapped_unicode_source_geometry() {
    let mut app = fixture(false);
    app.settings.word_wrap = true;
    app.settings.font_size = 28.0;
    let mut screen = harness(app, [510.0, 600.0]);
    select(&mut screen, 21, 64);
    add_selection(&mut screen);
    let shape = source_shape(&screen);
    assert!(
        shape.galley.rows.len() > DOCUMENT.split('\n').count(),
        "The fixture must actually wrap source rows"
    );
    let fill = Color32::from_rgba_unmultiplied(
        AGENT_MARK_COLOR.r(),
        AGENT_MARK_COLOR.g(),
        AGENT_MARK_COLOR.b(),
        28,
    );
    let actual: Vec<_> = screen
        .output()
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::epaint::Shape::Rect(rect) if rect.fill == fill => Some(rect.rect),
            _ => None,
        })
        .collect();
    let expected: Vec<_> = crate::render::visual_rows(&shape.galley)
        .enumerate()
        .filter_map(|(index, mapped)| {
            crate::render::range_on_visual_row(&shape.galley, index, mapped, 21, 64)
                .map(|rect| rect.translate(shape.pos.to_vec2()))
        })
        .collect();
    assert_eq!(actual, expected);
    assert_ne!(
        fill,
        screen
            .ctx
            .style_of(egui::Theme::Dark)
            .visuals
            .selection
            .bg_fill
    );
    assert_eq!(screen.state().tab().unwrap().selection, 21..64);
}

#[test]
fn collection_groups_files_and_opens_the_chosen_mark_with_real_clicks() {
    let mut app = fixture(true);
    let root = app.projects[0].root.clone();
    let mut other = EditorTab::new(Some(root.join("other.md")), "other.md".into());
    other.text = DOCUMENT.into();
    other.saved_text = DOCUMENT.into();
    app.projects[0].tabs.push(other);
    app.projects[0].active_tab = 1;
    app.bind_current_pane();
    app.projects[0].active_tab = 0;
    app.bind_current_pane();
    let mut screen = harness(app, [1200.0, 900.0]);
    select(&mut screen, 8, 14);
    add_selection(&mut screen);
    let original_id = screen.state().tab().unwrap().id;
    screen.get_by_label("other.md").click();
    screen.run_steps(4);
    assert_ne!(screen.state().tab().unwrap().id, original_id);
    select(&mut screen, 21, 37);
    add_selection(&mut screen);
    assert_eq!(screen.state().agent_marks.items.len(), 2);
    screen.get_by_label("For agent").click();
    screen.run_steps(4);
    screen
        .get_by_label_contains("Marked block 1:9–1:15: target ·")
        .click();
    screen.run_steps(4);
    assert!(!screen.state().agent_marks_modal_open());
    assert_eq!(screen.state().tab().unwrap().id, original_id);
    assert_eq!(screen.state().agent_marks.items.len(), 2);
    assert_eq!(screen.state().tab().unwrap().text, DOCUMENT);
}

#[test]
fn marked_source_and_collection_screenshots() {
    let mut screen = harness(fixture(true), [1100.0, 760.0]);
    select(&mut screen, 8, 14);
    add_selection(&mut screen);
    select(&mut screen, 21, 37);
    add_selection(&mut screen);
    let id = screen.state().agent_marks.items[0].id;
    screen
        .state_mut()
        .agent_marks_set_note(id, "Simplify this condition without changing behavior");
    key(&mut screen, Key::Escape, egui::Modifiers::NONE);
    super::super::visual_tests::snapshot(&mut screen, "morrow_agent_marks_source");
    screen.state_mut().open_agent_marks();
    screen.run_steps(4);
    super::super::visual_tests::snapshot(&mut screen, "morrow_agent_marks_collection");
}

#[test]
fn narrow_collection_screenshot() {
    let mut narrow = harness(fixture(true), [510.0, 620.0]);
    select(&mut narrow, 8, 14);
    add_selection(&mut narrow);
    narrow.state_mut().open_agent_marks();
    narrow.run_steps(4);
    super::super::visual_tests::snapshot(&mut narrow, "morrow_narrow_agent_marks_collection");
}

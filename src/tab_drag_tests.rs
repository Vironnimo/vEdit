//! Real pointer drags and model moves must preserve buffers and pane-local state.
use super::*;
use egui_kittest::Harness;

fn fixture() -> VEditApp {
    let mut app = VEditApp::empty(egui::Context::default());
    app.settings.autosave = false;
    app.settings.smooth_scroll = false;
    app.sidebar_visible = false;
    let root = std::env::temp_dir().join("vedit-tab-drag-in-memory");
    let mut project = Project::new(root.clone());
    project.last_git_refresh = Instant::now();
    for (name, text) in [
        ("first.txt", "first document\nsecond line\n"),
        ("second.txt", "another document\nits second line\n"),
        ("plan.md", "# Plan\n\nFirst step\n\nSecond step\n"),
    ] {
        let mut tab = EditorTab::new(Some(root.join(name)), name.into());
        tab.text = text.into();
        tab.saved_text = text.into();
        project.tabs.push(tab);
    }
    app.projects.push(project);
    app.bind_current_pane();
    app
}
fn harness(app: VEditApp) -> Harness<'static, VEditApp> {
    let mut setup = egui_wgpu::WgpuSetup::without_display_handle();
    if let egui_wgpu::WgpuSetup::CreateNew(options) = &mut setup {
        options.instance_descriptor.backends = egui_wgpu::wgpu::Backends::GL;
    }
    let mut screen = Harness::builder()
        .with_size([1440.0, 920.0])
        .with_os(egui::os::OperatingSystem::Windows)
        .wgpu_setup(setup)
        .build_ui_state(
            |ui, app: &mut VEditApp| {
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
fn documents(app: &VEditApp, pane: u64) -> Vec<u64> {
    app.editor_layout
        .panes
        .iter()
        .find(|p| p.id == pane)
        .unwrap()
        .documents
        .clone()
}
fn tab_center(screen: &Harness<'_, VEditApp>, pane: u64, title: &str) -> egui::Pos2 {
    fn find(shape: &egui::Shape, rect: egui::Rect, title: &str) -> Option<egui::Pos2> {
        match shape {
            egui::Shape::Text(text)
                if text.galley.job.text == title
                    && rect.contains(text.pos)
                    && text.pos.y < rect.top() + 37.0 =>
            {
                Some(text.pos + text.galley.size() * 0.5)
            }
            egui::Shape::Vec(shapes) => shapes.iter().find_map(|s| find(s, rect, title)),
            _ => None,
        }
    }
    let rect = screen.state().editor_layout.rects[&pane];
    screen
        .output()
        .shapes
        .iter()
        .find_map(|s| find(&s.shape, rect, title))
        .unwrap_or_else(|| panic!("tab {title} not painted in pane {pane}"))
}
fn pointer(
    screen: &mut Harness<'_, VEditApp>,
    position: egui::Pos2,
    pressed: bool,
    button: egui::PointerButton,
) {
    screen.event(egui::Event::PointerMoved(position));
    screen.event(egui::Event::PointerButton {
        pos: position,
        button,
        pressed,
        modifiers: egui::Modifiers::NONE,
    });
    screen.run_steps(1);
}
fn start_drag(screen: &mut Harness<'_, VEditApp>, pane: u64, title: &str, target: egui::Pos2) {
    let start = tab_center(screen, pane, title);
    pointer(screen, start, true, egui::PointerButton::Primary);
    screen.event(egui::Event::PointerMoved(start + Vec2::new(15.0, 0.0)));
    screen.run_steps(1);
    screen.event(egui::Event::PointerMoved(target));
    screen.run_steps(2);
    assert!(
        screen.state().tab_drag_active(),
        "pointer drag must be active before release"
    );
}
fn release(screen: &mut Harness<'_, VEditApp>, target: egui::Pos2) {
    pointer(screen, target, false, egui::PointerButton::Primary);
    screen.run_steps(3);
    assert!(!screen.state().tab_drag_active());
}
fn drag_snapshot(
    screen: &mut Harness<'_, VEditApp>,
    name: &str,
    pointer: egui::Pos2,
    insertion: bool,
) {
    // Ordinary editor snapshots intentionally move the pointer out of the UI.
    // A drag snapshot must retain its actual target and feedback instead.
    screen.run_steps(3);
    let deadline = Instant::now() + Duration::from_secs(8);
    while screen.state().editor_layout.panes.iter().any(|pane| {
        pane.view == DocumentView::Source
            && pane.document.is_some_and(|id| {
                screen
                    .state()
                    .projects
                    .iter()
                    .flat_map(|project| &project.tabs)
                    .find(|tab| tab.id == id)
                    .is_some_and(|tab| {
                        !screen.state().syntax.is_ready(
                            tab.id,
                            &tab.text,
                            syntax_language(tab.path.as_deref()),
                            screen.state().settings.font_size,
                        )
                    })
            })
    }) {
        assert!(
            Instant::now() < deadline,
            "drag snapshot syntax worker timed out"
        );
        std::thread::sleep(Duration::from_millis(2));
        screen.run_steps(1);
    }
    screen.run_steps(2);
    fn ghost_at(shape: &egui::Shape, expected: egui::Pos2) -> bool {
        match shape {
            egui::Shape::Text(text) => {
                text.galley.job.text == "plan.md" && text.pos.distance(expected) < 1.0
            }
            egui::Shape::Vec(shapes) => shapes.iter().any(|s| ghost_at(s, expected)),
            _ => false,
        }
    }
    assert!(
        screen
            .output()
            .shapes
            .iter()
            .any(|s| ghost_at(&s.shape, pointer + Vec2::new(23.0, 24.0))),
        "drag title must be painted beside the pointer"
    );
    if insertion {
        assert!(
            screen.output().shapes.iter().any(|s| matches!(&s.shape,
            egui::Shape::LineSegment { points, stroke }
                if stroke.color == ACCENT && stroke.width > 2.0
                && (points[0].x-points[1].x).abs() < 0.1
                && (points[1].y-points[0].y).abs() > 20.0)),
            "a visible vertical insertion marker must be painted"
        );
    }
    if let Some(directory) = std::env::var_os("VEDIT_SNAPSHOT_DIR") {
        screen.snapshot_options(
            name,
            &egui_kittest::SnapshotOptions::new().output_path(PathBuf::from(directory)),
        );
    } else {
        screen.snapshot(name);
    }
}
fn exclusive_right(app: &mut VEditApp) -> u64 {
    let first = app.projects[0].tabs[0].id;
    let second = app.projects[0].tabs[1].id;
    let right = app.split_active(SplitOrientation::Vertical).unwrap();
    app.assign_editor_pane(right, second, None).unwrap();
    assert!(!app.detach_pane_tab(right, first).unwrap());
    app.focus_editor_pane(1).unwrap();
    right
}

#[test]
fn model_reorders_final_positions_and_session_remembers_order() {
    let mut app = fixture();
    let ids = documents(&app, 1);
    app.control_layout(LayoutAction::MoveTab {
        source_pane_id: 1,
        pane_id: 1,
        tab_id: ids[0],
        index: 2,
    })
    .unwrap();
    assert_eq!(documents(&app, 1), vec![ids[1], ids[2], ids[0]]);
    assert_eq!(app.tab().unwrap().id, ids[0]);
    let saved = app.editor_session_layout();
    let mut restored = fixture();
    let new_ids = documents(&restored, 1);
    restored.restore_editor_layout(Some(saved));
    assert_eq!(
        documents(&restored, 1),
        vec![new_ids[1], new_ids[2], new_ids[0]]
    );
    assert_eq!(restored.tab().unwrap().id, new_ids[0]);
}

#[test]
fn model_deduplicates_shared_tab_and_keeps_destination_view_and_cursor() {
    let mut app = fixture();
    app.activate_tab(0, 2);
    let id = app.tab().unwrap().id;
    let right = app.split_active(SplitOrientation::Vertical).unwrap();
    app.set_document_view(DocumentView::Preview);
    app.tab_mut().unwrap().vim.cursor = 9;
    app.capture_pane_navigation();
    app.focus_editor_pane(1).unwrap();
    app.tab_mut().unwrap().vim.cursor = 2;
    app.capture_pane_navigation();
    app.control_layout(LayoutAction::MoveTab {
        source_pane_id: 1,
        pane_id: right,
        tab_id: id,
        index: 0,
    })
    .unwrap();
    assert_eq!(documents(&app, right), vec![id]);
    assert!(!documents(&app, 1).contains(&id));
    assert_eq!(app.tab().unwrap().vim.cursor, 9);
    assert_eq!(app.tab().unwrap().agent_view, DocumentView::Preview);
    assert_eq!(app.projects[0].tabs.len(), 3);
}

#[test]
fn reordering_the_active_tab_keeps_insert_mode_and_one_undo_transaction() {
    let mut app = fixture();
    let id = app.tab().unwrap().id;
    let original = app.tab().unwrap().text.clone();
    let tab = app.tab_mut().unwrap();
    tab.vim.handle_key(&mut tab.text, VimKey::Char('i'));
    tab.vim.insert_text(&mut tab.text, "before ");
    tab.changed();
    let cursor = tab.vim.cursor;
    app.capture_pane_navigation();
    app.control_layout(LayoutAction::MoveTab {
        source_pane_id: 1,
        pane_id: 1,
        tab_id: id,
        index: 2,
    })
    .unwrap();
    assert_eq!(app.tab().unwrap().vim.mode, VimMode::Insert);
    assert_eq!(app.tab().unwrap().vim.cursor, cursor);
    let tab = app.tab_mut().unwrap();
    tab.vim.insert_text(&mut tab.text, "after ");
    tab.vim.handle_key(&mut tab.text, VimKey::Escape);
    tab.vim.handle_key(&mut tab.text, VimKey::Char('u'));
    assert_eq!(
        tab.text, original,
        "reordering must not split an unfinished Insert transaction"
    );
}

#[test]
fn rejected_model_moves_leave_insert_history_navigation_and_layout_untouched() {
    let mut app = fixture();
    let right = exclusive_right(&mut app);
    app.tab_mut()
        .unwrap()
        .vim
        .handle_key(&mut String::new(), VimKey::Char('i'));
    let id = app.tab().unwrap().id;
    let before = app.layout_status();
    let text = app.tab().unwrap().text.clone();
    for (source, target, tab, index) in [
        (999, right, id, 0),
        (1, 999, id, 0),
        (right, 1, id, 0),
        (1, right, id, 99),
    ] {
        assert!(
            app.control_layout(LayoutAction::MoveTab {
                source_pane_id: source,
                pane_id: target,
                tab_id: tab,
                index
            })
            .is_err()
        );
        assert_eq!(app.layout_status(), before);
        assert_eq!(app.tab().unwrap().vim.mode, VimMode::Insert);
        assert_eq!(app.tab().unwrap().text, text);
    }
}

#[test]
fn pointer_reorder_shows_insertion_and_does_not_switch_tabs_until_drop() {
    let app = fixture();
    let ids = documents(&app, 1);
    let mut screen = harness(app);
    let first = tab_center(&screen, 1, "first.txt");
    let target = first - Vec2::new(35.0, 0.0);
    start_drag(&mut screen, 1, "plan.md", target);
    assert_eq!(documents(screen.state(), 1), ids);
    assert_eq!(screen.state().tab().unwrap().id, ids[0]);
    drag_snapshot(&mut screen, "vedit_tab_drag_insert", target, true);
    release(&mut screen, target);
    assert_eq!(documents(screen.state(), 1), vec![ids[2], ids[0], ids[1]]);
    assert_eq!(screen.state().tab().unwrap().id, ids[2]);
}

#[test]
fn pointer_move_to_other_strip_preserves_dirty_vim_undo_and_visual_selection() {
    let mut app = fixture();
    let right = exclusive_right(&mut app);
    app.activate_tab(0, 2);
    let id = app.tab().unwrap().id;
    let original = app.tab().unwrap().text.clone();
    let tab = app.tab_mut().unwrap();
    tab.vim.handle_key(&mut tab.text, VimKey::Char('i'));
    for character in "added ".chars() {
        tab.vim.handle_key(&mut tab.text, VimKey::Char(character));
    }
    tab.vim.handle_key(&mut tab.text, VimKey::Escape);
    tab.vim.handle_key(&mut tab.text, VimKey::Char('V'));
    tab.vim.handle_key(&mut tab.text, VimKey::Char('j'));
    tab.changed();
    let selection = tab.vim.selection(&tab.text).unwrap();
    let cursor = tab.vim.cursor;
    app.capture_pane_navigation();
    let revision = app.tab().unwrap().revision;
    let destination_second = documents(&app, right)[0];
    let mut screen = harness(app);
    let target = tab_center(&screen, right, "second.txt") - Vec2::new(35.0, 0.0);
    start_drag(&mut screen, 1, "plan.md", target);
    release(&mut screen, target);
    assert_eq!(
        documents(screen.state(), right),
        vec![id, destination_second]
    );
    assert!(!documents(screen.state(), 1).contains(&id));
    let tab = screen.state().tab().unwrap();
    assert_eq!(tab.id, id);
    assert_eq!(tab.vim.cursor, cursor);
    assert_eq!(tab.vim.mode, VimMode::VisualLine);
    assert_eq!(tab.vim.selection(&tab.text), Some(selection));
    assert_eq!(tab.revision, revision);
    assert!(tab.dirty);
    assert_eq!(screen.state().editor_layout.focused, right);
    screen.key_press(Key::Escape);
    screen.run_steps(1);
    screen.event(egui::Event::Text("u".into()));
    screen.run_steps(2);
    assert_eq!(screen.state().tab().unwrap().text, original);
    assert_eq!(screen.state().projects[0].tabs.len(), 3);
}

#[test]
fn pointer_drop_on_editor_body_moves_and_closes_empty_source() {
    let mut app = fixture();
    let right = exclusive_right(&mut app);
    let second = app.projects[0].tabs[1].id;
    let first = app.projects[0].tabs[0].id;
    app.move_pane_tab(1, right, first, 1).unwrap();
    app.move_pane_tab(1, right, second, 0).unwrap();
    assert_eq!(documents(&app, 1).len(), 1);
    let id = documents(&app, 1)[0];
    let mut screen = harness(app);
    let target = screen.state().editor_layout.rects[&right].center();
    start_drag(&mut screen, 1, "plan.md", target);
    drag_snapshot(&mut screen, "vedit_tab_drag_pane", target, false);
    release(&mut screen, target);
    assert_eq!(screen.state().editor_layout.panes.len(), 1);
    assert_eq!(screen.state().editor_layout.root.first_id(), right);
    assert_eq!(documents(screen.state(), right), vec![second, first, id]);
    assert_eq!(screen.state().tab().unwrap().id, id);
    assert_eq!(screen.state().projects[0].tabs.len(), 3);
}

#[test]
fn pointer_drop_shared_tab_deduplicates_destination() {
    let mut app = fixture();
    let id = app.tab().unwrap().id;
    let right = app.split_active(SplitOrientation::Vertical).unwrap();
    app.focus_editor_pane(1).unwrap();
    let mut screen = harness(app);
    let target = screen.state().editor_layout.rects[&right].center();
    start_drag(&mut screen, 1, "first.txt", target);
    release(&mut screen, target);
    assert_eq!(documents(screen.state(), right), vec![id]);
    assert!(!documents(screen.state(), 1).contains(&id));
    assert_eq!(screen.state().projects[0].tabs.len(), 3);
}

fn wheel(screen: &mut Harness<'_, VEditApp>, pane: u64, delta: Vec2) {
    let position = screen.state().editor_layout.rects[&pane].center();
    screen.event(egui::Event::PointerMoved(position));
    screen.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta,
        modifiers: egui::Modifiers::NONE,
        phase: egui::TouchPhase::Move,
    });
    screen.run_steps(2);
}

#[test]
fn moving_a_scrolled_source_carries_both_offsets_to_a_fresh_target() {
    let mut app = fixture();
    let right = exclusive_right(&mut app);
    let text = (0..350)
        .map(|line| format!("Line {line}: {}\n", "long content ".repeat(40)))
        .collect::<String>();
    let tab = &mut app.projects[0].tabs[2];
    tab.text = text.clone();
    tab.saved_text = text.clone();
    app.activate_tab(0, 2);
    let id = app.tab().unwrap().id;
    let mut screen = harness(app);
    wheel(&mut screen, 1, Vec2::new(-140.0, -520.0));
    let original = screen.state().find.view_scroll[&(1, id)].1;
    assert!(
        original.x > 100.0 && original.y > 400.0,
        "actual source scroll must move both axes: {original:?}"
    );
    let target = screen.state().editor_layout.rects[&right].center();
    start_drag(&mut screen, 1, "plan.md", target);
    release(&mut screen, target);
    let moved = screen.state().find.view_scroll[&(right, id)].1;
    assert!(
        (moved - original).length() < 1.0,
        "source offsets must travel to the fresh destination: {original:?} -> {moved:?}"
    );
    assert_eq!(screen.state().tab().unwrap().text, text);
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 0);
}

#[test]
fn moving_a_shared_source_keeps_the_existing_destination_scroll() {
    let mut app = fixture();
    let text = (0..350)
        .map(|line| format!("Line {line}\n"))
        .collect::<String>();
    let tab = app.tab_mut().unwrap();
    tab.text = text.clone();
    tab.saved_text = text;
    let id = tab.id;
    let right = app.split_active(SplitOrientation::Vertical).unwrap();
    app.focus_editor_pane(1).unwrap();
    let mut screen = harness(app);
    wheel(&mut screen, 1, Vec2::new(0.0, -520.0));
    wheel(&mut screen, right, Vec2::new(0.0, -180.0));
    let target_offset = screen.state().find.view_scroll[&(right, id)].1;
    assert!(target_offset.y > 100.0 && target_offset.y < 300.0);
    let target = screen.state().editor_layout.rects[&right].center();
    start_drag(&mut screen, 1, "first.txt", target);
    release(&mut screen, target);
    let actual = screen.state().find.view_scroll[&(right, id)].1;
    assert!(
        (actual - target_offset).length() < 1.0,
        "an existing target view must retain its own reading position"
    );
}

#[test]
fn dropping_a_shared_markdown_tab_keeps_target_preview_and_releases_editor_focus() {
    let mut app = fixture();
    app.activate_tab(0, 2);
    let id = app.tab().unwrap().id;
    let right = app.split_active(SplitOrientation::Vertical).unwrap();
    app.set_document_view(DocumentView::Preview);
    app.focus_editor_pane(1).unwrap();
    let mut screen = harness(app);
    let target = screen.state().editor_layout.rects[&right].center();
    start_drag(&mut screen, 1, "plan.md", target);
    release(&mut screen, target);
    assert_eq!(documents(screen.state(), right), vec![id]);
    assert!(!documents(screen.state(), 1).contains(&id));
    assert_eq!(
        screen.state().tab().unwrap().agent_view,
        DocumentView::Preview
    );
    assert!(screen.ctx.memory(|memory| memory.focused().is_none()));
    assert_eq!(screen.state().projects[0].tabs.len(), 3);
}

fn wide_preview_tables() -> (String, Vec<usize>) {
    let mut text = String::from("# Wide review tables\n\n");
    let mut tables = Vec::new();
    for name in ["Primary", "Secondary"] {
        tables.push(text.len());
        text.push('|');
        for column in 0..12 {
            text.push_str(&format!(" {name} {column} |"));
        }
        text.push_str("\n|");
        for _ in 0..12 {
            text.push_str(" --- |");
        }
        text.push_str("\n|");
        for column in 0..12 {
            text.push_str(&format!(" Value {column} |"));
        }
        text.push_str("\n\n");
    }
    (text, tables)
}

fn preview_label_position(screen: &Harness<'_, VEditApp>, pane: u64, label: &str) -> egui::Pos2 {
    fn find(shape: &egui::Shape, pane: egui::Rect, label: &str) -> Option<egui::Pos2> {
        match shape {
            egui::Shape::Text(text) if text.galley.job.text == label && pane.contains(text.pos) => {
                Some(text.pos)
            }
            egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| find(shape, pane, label)),
            _ => None,
        }
    }
    let rect = screen.state().editor_layout.rects[&pane];
    screen
        .output()
        .shapes
        .iter()
        .find_map(|shape| find(&shape.shape, rect, label))
        .unwrap_or_else(|| panic!("Preview label {label} must be visible in pane {pane}"))
}

fn table_wheel(screen: &mut Harness<'_, VEditApp>, pane: u64, label: &str, delta: f32) {
    let position = preview_label_position(screen, pane, label) + Vec2::splat(5.0);
    screen.event(egui::Event::PointerMoved(position));
    screen.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: Vec2::new(delta, 0.0),
        modifiers: egui::Modifiers::NONE,
        phase: egui::TouchPhase::Move,
    });
    screen.run_steps(2);
}

#[test]
fn moving_a_preview_keeps_each_wide_table_horizontal_position_and_accepts_the_next_wheel() {
    let mut app = fixture();
    let right = exclusive_right(&mut app);
    let (text, tables) = wide_preview_tables();
    app.projects[0].tabs[2].text = text.clone();
    app.projects[0].tabs[2].saved_text = text;
    app.activate_tab(0, 2);
    app.set_document_view(DocumentView::Preview);
    let id = app.tab().unwrap().id;
    let mut screen = harness(app);
    table_wheel(&mut screen, 1, "Primary 2", -140.0);
    table_wheel(&mut screen, 1, "Secondary 2", -80.0);
    let offsets: Vec<_> = tables
        .iter()
        .map(|source| {
            screen
                .state()
                .tab_preview_table_scroll(1, id, *source)
                .unwrap()
        })
        .collect();
    assert!(
        (offsets[0] - 140.0).abs() < 1.0 && (offsets[1] - 80.0).abs() < 1.0,
        "actual independent table wheel scrolling is required: {offsets:?}"
    );
    let relative_x = preview_label_position(&screen, 1, "Primary 2").x
        - screen.state().editor_layout.rects[&1].left();
    let target = screen.state().editor_layout.rects[&right].center();
    start_drag(&mut screen, 1, "plan.md", target);
    release(&mut screen, target);
    for (source, expected) in tables.iter().zip(&offsets) {
        let actual = screen
            .state()
            .tab_preview_table_scroll(right, id, *source)
            .unwrap();
        assert!(
            (actual - expected).abs() < 1.0,
            "each table must retain its horizontal position: {expected} -> {actual}"
        );
    }
    let moved_x = preview_label_position(&screen, right, "Primary 2").x
        - screen.state().editor_layout.rects[&right].left();
    assert!(
        (moved_x - relative_x).abs() < 1.0,
        "the actual painted table content must keep its position"
    );
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 0);
    table_wheel(&mut screen, right, "Primary 2", -50.0);
    assert!(
        (screen
            .state()
            .tab_preview_table_scroll(right, id, tables[0])
            .unwrap()
            - 190.0)
            .abs()
            < 1.0,
        "the first wheel in the destination must continue from the transferred position"
    );
}

#[test]
fn moving_a_shared_preview_keeps_the_destination_table_horizontal_position() {
    let mut app = fixture();
    let (text, tables) = wide_preview_tables();
    app.projects[0].tabs[2].text = text.clone();
    app.projects[0].tabs[2].saved_text = text;
    app.activate_tab(0, 2);
    let id = app.tab().unwrap().id;
    let right = app.split_active(SplitOrientation::Vertical).unwrap();
    app.set_document_view(DocumentView::Preview);
    app.focus_editor_pane(1).unwrap();
    app.set_document_view(DocumentView::Preview);
    let mut screen = harness(app);
    table_wheel(&mut screen, 1, "Primary 2", -140.0);
    table_wheel(&mut screen, right, "Primary 2", -80.0);
    let original = screen
        .state()
        .tab_preview_table_scroll(right, id, tables[0])
        .unwrap();
    assert!((original - 80.0).abs() < 1.0);
    let painted_x = preview_label_position(&screen, right, "Primary 2").x;
    let target = screen.state().editor_layout.rects[&right].center();
    start_drag(&mut screen, 1, "plan.md", target);
    release(&mut screen, target);
    let actual = screen
        .state()
        .tab_preview_table_scroll(right, id, tables[0])
        .unwrap();
    assert!(
        (actual - original).abs() < 1.0,
        "dedupe must retain the target's own table scroll"
    );
    assert!(
        (preview_label_position(&screen, right, "Primary 2").x - painted_x).abs() < 1.0,
        "dedupe must retain the target's painted horizontal reading position"
    );
    assert_eq!(documents(screen.state(), right), vec![id]);
}

fn long_preview() -> String {
    (0..160).map(|section| format!("## Section {section}\n\nParagraph {section} explains the next step in this plan. {}\n\n",
        "Keep this useful detail visible while reviewing the document. ".repeat(3))).collect()
}

#[test]
fn moving_a_scrolled_markdown_preview_carries_its_reading_position() {
    let mut app = fixture();
    let right = exclusive_right(&mut app);
    let text = long_preview();
    app.projects[0].tabs[2].text = text.clone();
    app.projects[0].tabs[2].saved_text = text.clone();
    app.activate_tab(0, 2);
    app.set_document_view(DocumentView::Preview);
    let id = app.tab().unwrap().id;
    let mut screen = harness(app);
    wheel(&mut screen, 1, Vec2::new(0.0, -640.0));
    let original = screen.state().tab_preview_scroll(1, id).unwrap();
    assert!(
        original > 500.0,
        "actual preview must be scrolled before moving"
    );
    let target = screen.state().editor_layout.rects[&right].center();
    start_drag(&mut screen, 1, "plan.md", target);
    release(&mut screen, target);
    let moved = screen.state().tab_preview_scroll(right, id).unwrap();
    assert!(
        (moved - original).abs() < 1.0,
        "preview reading position must travel to the fresh destination: {original} -> {moved}"
    );
    assert_eq!(
        screen.state().tab().unwrap().agent_view,
        DocumentView::Preview
    );
    assert_eq!(screen.state().tab().unwrap().text, text);
}

#[test]
fn moving_a_shared_preview_keeps_the_destination_reading_position() {
    let mut app = fixture();
    let text = long_preview();
    app.projects[0].tabs[2].text = text.clone();
    app.projects[0].tabs[2].saved_text = text;
    app.activate_tab(0, 2);
    let id = app.tab().unwrap().id;
    let right = app.split_active(SplitOrientation::Vertical).unwrap();
    app.set_document_view(DocumentView::Preview);
    app.focus_editor_pane(1).unwrap();
    app.set_document_view(DocumentView::Preview);
    let mut screen = harness(app);
    wheel(&mut screen, 1, Vec2::new(0.0, -640.0));
    wheel(&mut screen, right, Vec2::new(0.0, -180.0));
    let original = screen.state().tab_preview_scroll(right, id).unwrap();
    assert!(original > 100.0 && original < 300.0);
    let target = screen.state().editor_layout.rects[&right].center();
    start_drag(&mut screen, 1, "plan.md", target);
    release(&mut screen, target);
    let actual = screen.state().tab_preview_scroll(right, id).unwrap();
    assert!(
        (actual - original).abs() < 1.0,
        "the existing target preview must keep its own scroll"
    );
}

#[test]
fn cli_batch_carries_preview_scroll_through_panes_that_have_not_rendered() {
    let mut app = fixture();
    let (mut text, tables) = wide_preview_tables();
    text.push_str(&long_preview());
    app.projects[0].tabs[2].text = text.clone();
    app.projects[0].tabs[2].saved_text = text;
    app.activate_tab(0, 2);
    app.set_document_view(DocumentView::Preview);
    let id = app.tab().unwrap().id;
    let second = app.projects[0].tabs[1].id;
    let mut screen = harness(app);
    table_wheel(&mut screen, 1, "Primary 2", -140.0);
    wheel(&mut screen, 1, Vec2::new(0.0, -640.0));
    let original = screen.state().tab_preview_scroll(1, id).unwrap();
    assert!(original > 500.0);
    let app = screen.state_mut();
    for source in [1, 2] {
        app.control_layout(LayoutAction::Split {
            orientation: SplitOrientation::Vertical,
            pane_id: Some(source),
            tab_id: Some(second),
            view: None,
        })
        .unwrap();
    }
    for (source, target) in [(1, 2), (2, 3)] {
        app.control_layout(LayoutAction::MoveTab {
            source_pane_id: source,
            pane_id: target,
            tab_id: id,
            index: 1,
        })
        .unwrap();
    }
    assert_eq!(documents(app, 2), vec![second]);
    assert_eq!(documents(app, 3), vec![second, id]);
    // Render only after the entire batch, as the live CLI receiver does.
    screen.run_steps(3);
    let moved = screen.state().tab_preview_scroll(3, id).unwrap();
    assert!(
        (moved - original).abs() < 1.0,
        "reading position must follow a chained pre-render batch: {original} -> {moved}"
    );
    assert_eq!(
        screen.state().tab().unwrap().agent_view,
        DocumentView::Preview
    );
    assert!(
        (screen
            .state()
            .tab_preview_table_scroll(3, id, tables[0])
            .unwrap()
            - 140.0)
            .abs()
            < 1.0,
        "the table's horizontal reading position must follow the pre-render batch too"
    );
}

#[test]
fn outside_drop_and_escape_cancel_without_reordering_or_selecting_dragged_tab() {
    let mut screen = harness(fixture());
    let before = screen.state().layout_status();
    let outside = egui::pos2(10.0, 10.0);
    start_drag(&mut screen, 1, "plan.md", outside);
    release(&mut screen, outside);
    assert_eq!(screen.state().layout_status(), before);
    let target = tab_center(&screen, 1, "first.txt");
    start_drag(&mut screen, 1, "plan.md", target);
    assert_eq!(
        screen.state().layout_status(),
        before,
        "dragging must preserve the original Visual cursor before Escape"
    );
    screen.key_press(Key::Escape);
    screen.run_steps(1);
    assert!(!screen.state().tab_drag_active());
    release(&mut screen, target);
    assert_eq!(screen.state().layout_status(), before);
}

#[test]
fn tiny_pointer_motion_clicks_normally_without_starting_a_drag() {
    let mut screen = harness(fixture());
    let before = documents(screen.state(), 1);
    let start = tab_center(&screen, 1, "plan.md");
    pointer(&mut screen, start, true, egui::PointerButton::Primary);
    screen.event(egui::Event::PointerMoved(start + Vec2::new(1.0, 0.0)));
    screen.run_steps(1);
    assert!(!screen.state().tab_drag_active());
    release(&mut screen, start + Vec2::new(1.0, 0.0));
    assert_eq!(documents(screen.state(), 1), before);
    assert_eq!(screen.state().tab().unwrap().title, "plan.md");
}

#[test]
fn holding_a_drag_at_the_strip_edge_reaches_initially_hidden_tabs() {
    let mut app = fixture();
    let first = app.tab().unwrap().id;
    let root = app.projects[0].root.clone();
    for index in 0..26 {
        let title = format!("extra-{index:02}.txt");
        let mut tab = EditorTab::new(Some(root.join(&title)), title);
        tab.text = "extra document\n".into();
        tab.saved_text = tab.text.clone();
        app.projects[0].tabs.push(tab);
    }
    app.adopt_orphan_documents();
    let mut screen = harness(app);
    let strip = screen
        .state()
        .tab_strip_rect(1)
        .expect("the tabs expose a drop strip");
    let target = egui::pos2(strip.right() - 8.0, strip.center().y);
    start_drag(&mut screen, 1, "first.txt", target);
    screen.run_steps(160);
    release(&mut screen, target);
    let position = documents(screen.state(), 1)
        .iter()
        .position(|id| *id == first)
        .unwrap();
    assert!(
        position > 12,
        "edge scrolling must reveal drop positions beyond the original viewport: {position}"
    );
    assert_eq!(screen.state().projects[0].tabs.len(), 29);
    assert_eq!(screen.state().tab().unwrap().id, first);
}

#[test]
fn escape_and_trailing_typing_cancel_drag_without_changing_visual_selection() {
    let mut app = fixture();
    let tab = app.tab_mut().unwrap();
    tab.vim.handle_key(&mut tab.text, VimKey::Char('V'));
    tab.vim.handle_key(&mut tab.text, VimKey::Char('j'));
    app.capture_pane_navigation();
    let mut screen = harness(app);
    let before = screen.state().layout_status();
    let selection = screen
        .state()
        .tab()
        .unwrap()
        .vim
        .selection(&screen.state().tab().unwrap().text);
    let target = tab_center(&screen, 1, "first.txt");
    start_drag(&mut screen, 1, "plan.md", target);
    assert_eq!(
        screen.state().layout_status(),
        before,
        "Visual cursor during held drag"
    );
    screen.event(egui::Event::Key {
        key: Key::Escape,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    });
    screen.event(egui::Event::Text("idd".into()));
    screen.run_steps(1);
    release(&mut screen, target);
    assert_eq!(screen.state().layout_status(), before);
    assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::VisualLine);
    assert_eq!(
        screen
            .state()
            .tab()
            .unwrap()
            .vim
            .selection(&screen.state().tab().unwrap().text),
        selection
    );
    assert!(screen.state().projects[0].tabs.iter().all(|tab| !tab.dirty));
}

#[test]
fn pointer_middle_click_and_close_button_keep_working_without_dragging() {
    let mut screen = harness(fixture());
    let id = screen.state().projects[0].tabs[2].id;
    let position = tab_center(&screen, 1, "plan.md");
    pointer(&mut screen, position, true, egui::PointerButton::Middle);
    pointer(&mut screen, position, false, egui::PointerButton::Middle);
    screen.run_steps(2);
    assert!(!screen.state().pane_contains_tab(1, id));
    assert!(!screen.state().tab_drag_active());
    let rect = screen.state().editor_layout.rects[&1];
    let close = rect.left_top() + Vec2::new(125.0 - 16.0, 18.5);
    pointer(&mut screen, close, true, egui::PointerButton::Primary);
    pointer(&mut screen, close, false, egui::PointerButton::Primary);
    screen.run_steps(2);
    assert_eq!(documents(screen.state(), 1).len(), 1);
    assert_eq!(screen.state().tab().unwrap().title, "second.txt");
}

#[test]
fn agent_observations_remain_available_but_mutations_wait_during_pointer_drag() {
    let mut screen = harness(fixture());
    let target = tab_center(&screen, 1, "first.txt");
    start_drag(&mut screen, 1, "plan.md", target);
    let before = screen.state().layout_status();
    let id = screen.state().tab().unwrap().id;
    let revision = screen.state().tab().unwrap().revision;
    let app = screen.state_mut();
    let status = app.control_request(crate::cli::Request::Status {});
    assert!(status.ok);
    assert_eq!(status.result["dialog"], "tab_drag");
    let read: crate::cli::Request =
        serde_json::from_value(json!({"op":"read","tab_id":id})).unwrap();
    assert!(app.control_request(read).ok);
    let blocked = app.control_request(crate::cli::Request::Layout {
        change: LayoutAction::MoveTab {
            source_pane_id: 1,
            pane_id: 1,
            tab_id: id,
            index: 2,
        },
    });
    assert!(!blocked.ok);
    assert_eq!(blocked.error.unwrap().code, "user_dialog_open");
    let blocked = app.control_request(crate::cli::Request::Edit {
        tab_id: id,
        expected_revision: revision,
        edits: Vec::new(),
        set_text: Some("must not replace".into()),
    });
    assert!(!blocked.ok);
    assert_eq!(app.layout_status(), before);
    assert!(app.tab_drag_active());
    screen.key_press(Key::Escape);
    screen.run_steps(1);
    release(&mut screen, target);
    let response = screen
        .state_mut()
        .control_request(crate::cli::Request::Layout {
            change: LayoutAction::MoveTab {
                source_pane_id: 1,
                pane_id: 1,
                tab_id: id,
                index: 2,
            },
        });
    assert!(response.ok, "{}", response.result);
}

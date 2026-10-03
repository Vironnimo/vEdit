use super::*;
use egui_kittest::Harness;

fn fixture() -> MorrowApp {
    let mut app = MorrowApp::empty(egui::Context::default());
    app.settings.autosave = false;
    app.settings.smooth_scroll = false;
    app.settings.vim = false;
    let root = std::env::temp_dir().join("morrow-spatial-navigation-ui");
    let mut project = Project::new(root.clone());
    project.last_git_refresh = Instant::now();
    project.index = Some(Arc::new(workspace::WorkspaceIndex::empty(root.clone())));
    for name in ["one.txt", "two.txt"] {
        let mut tab = EditorTab::new(Some(root.join(name)), name.into());
        tab.text = "alpha\nbeta\ngamma\n".into();
        tab.saved_text = tab.text.clone();
        project.tabs.push(tab);
    }
    app.projects.push(project);
    app.bind_current_pane();
    app.explorer_git_layout();
    app
}

fn harness(app: MorrowApp) -> Harness<'static, MorrowApp> {
    harness_size(app, [1560.0, 1000.0])
}

fn harness_size(app: MorrowApp, size: [f32; 2]) -> Harness<'static, MorrowApp> {
    let mut screen = Harness::builder()
        .with_size(size)
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
        style.animation_time = 0.0;
        style.scroll_animation = egui::style::ScrollAnimation::none();
        style.visuals.text_cursor.blink = false;
    });
    screen.run_steps(3);
    screen
}

fn key(key: Key, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}

#[test]
fn sidebar_neighbor_uses_nearest_editor_edge_and_remembered_row() {
    let sidebar = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(200.0, 800.0));
    let regions = vec![
        (
            Region::Pane(1),
            egui::Rect::from_min_max(egui::pos2(205.0, 0.0), egui::pos2(500.0, 395.0)),
        ),
        (
            Region::Pane(2),
            egui::Rect::from_min_max(egui::pos2(505.0, 0.0), egui::pos2(800.0, 395.0)),
        ),
        (
            Region::Pane(3),
            egui::Rect::from_min_max(egui::pos2(205.0, 400.0), egui::pos2(500.0, 800.0)),
        ),
        (
            Region::Pane(4),
            egui::Rect::from_min_max(egui::pos2(505.0, 400.0), egui::pos2(800.0, 800.0)),
        ),
    ];
    assert_eq!(
        neighbor(
            sidebar,
            egui::pos2(650.0, 650.0),
            Vec2::new(1.0, 0.0),
            regions.clone()
        ),
        Some(Region::Pane(3))
    );
    assert_eq!(
        neighbor(
            sidebar,
            egui::pos2(650.0, 100.0),
            Vec2::new(1.0, 0.0),
            regions
        ),
        Some(Region::Pane(1))
    );
}

#[test]
fn directional_focus_traverses_two_rows_and_restores_sidebar_row() {
    let mut app = fixture();
    let left_top = app.current_pane_id();
    let right_top = app.split_active(SplitOrientation::Vertical).unwrap();
    let right_bottom = app.split_active(SplitOrientation::Horizontal).unwrap();
    app.focus_editor_pane(left_top).unwrap();
    let left_bottom = app.split_active(SplitOrientation::Horizontal).unwrap();
    let mut screen = harness(app);
    screen.state_mut().focus_editor();
    screen.state_mut().tab_mut().unwrap().set_cursor(7..7);
    screen.run_steps(2);
    screen.state_mut().split_command("focus_left");
    screen.run_steps(2);
    assert_eq!(
        screen.state().focused_sidebar(),
        Some(sidebar_layout::Side::Left)
    );
    screen.state_mut().split_command("focus_right");
    screen.run_steps(2);
    assert_eq!(screen.state().current_pane_id(), left_bottom);
    assert_eq!(screen.state().tab().unwrap().selection, 7..7);
    screen.state_mut().split_command("focus_right");
    screen.run_steps(2);
    assert_eq!(screen.state().current_pane_id(), right_bottom);
    screen.state_mut().split_command("focus_up");
    screen.run_steps(2);
    assert_eq!(screen.state().current_pane_id(), right_top);
    screen.state_mut().split_command("focus_left");
    screen.run_steps(2);
    assert_eq!(screen.state().current_pane_id(), left_top);
}

#[test]
fn f6_includes_output_and_skips_hidden_sidebars() {
    let mut app = fixture();
    app.output_visible = true;
    app.sidebar_visible = false;
    app.sidebar_layout.right_visible = false;
    let pane = app.current_pane_id();
    let mut screen = harness(app);
    screen.state_mut().focus_editor();
    screen.state_mut().cycle_focus_region(false);
    screen.run_steps(2);
    assert!(screen.state().output_keyboard_focused());
    assert_eq!(
        screen.ctx.memory(|memory| memory.focused()),
        Some(output_command_id())
    );
    screen.state_mut().cycle_focus_region(false);
    screen.run_steps(2);
    assert_eq!(screen.state().navigation_region(), Region::Pane(pane));
    assert!(!screen.state().sidebar_visible);
    assert!(!screen.state().sidebar_layout.right_visible);
}

#[test]
fn output_up_returns_to_the_same_editor_column() {
    let mut app = fixture();
    app.output_visible = true;
    let right = app.split_active(SplitOrientation::Vertical).unwrap();
    let mut screen = harness(app);
    screen.state_mut().focus_editor();
    screen.state_mut().split_command("focus_down");
    screen.run_steps(2);
    assert!(screen.state().output_keyboard_focused());
    screen.state_mut().split_command("focus_up");
    screen.run_steps(2);
    assert_eq!(screen.state().navigation_region(), Region::Pane(right));
}

#[test]
fn output_focus_shortcut_preserves_text_on_each_side_of_event_boundary() {
    let mut app = fixture();
    app.output_visible = true;
    app.command.clear();
    let mut screen = harness(app);
    screen.state_mut().focus_editor();
    screen.state_mut().tab_mut().unwrap().set_cursor(0..0);
    screen.run_steps(2);
    screen.input_mut().events.extend([
        egui::Event::Text("source ".into()),
        key(
            Key::ArrowDown,
            egui::Modifiers::COMMAND.plus(egui::Modifiers::ALT),
        ),
        egui::Event::Text("destination".into()),
    ]);
    screen.run_steps(4);
    assert_eq!(
        screen.state().tab().unwrap().text,
        "source alpha\nbeta\ngamma\n"
    );
    assert_eq!(screen.state().command, "destination");
    assert!(screen.state().output_keyboard_focused());
}

#[test]
fn custom_spatial_shortcut_replaces_default_and_preserves_source() {
    let mut app = fixture();
    app.settings
        .keybindings
        .insert("focus_left".into(), Some("Ctrl+Alt+H".into()));
    let text = app.tab().unwrap().text.clone();
    let mut screen = harness(app);
    screen.state_mut().focus_editor();
    screen.run_steps(2);
    screen.input_mut().events.push(key(
        Key::ArrowLeft,
        egui::Modifiers::COMMAND.plus(egui::Modifiers::ALT),
    ));
    screen.run_steps(2);
    assert_eq!(screen.state().focused_sidebar(), None);
    screen.input_mut().events.push(key(
        Key::H,
        egui::Modifiers::COMMAND.plus(egui::Modifiers::ALT),
    ));
    screen.run_steps(3);
    assert_eq!(
        screen.state().focused_sidebar(),
        Some(sidebar_layout::Side::Left)
    );
    assert_eq!(screen.state().tab().unwrap().text, text);
}

#[test]
fn explicit_focus_switches_compact_sidebars_before_dispatching_queued_text() {
    let mut app = fixture();
    app.settings.ui_scale = 2.0;
    app.projects[0].git_root = Some(app.projects[0].root.clone());
    app.projects[0].git_loading = false;
    let source_text = app.tab().unwrap().text.clone();
    let composer = egui::Id::new(("git-content", &app.projects[0].root, "commit-message"));
    let mut screen = harness_size(app, [1200.0, 1000.0]);
    assert!(screen.state().sidebar_view_visible(SidebarView::Explorer));
    assert!(
        !screen
            .state()
            .sidebar_view_visible(SidebarView::SourceControl)
    );
    screen.state_mut().show_sidebar(SidebarView::SourceControl);
    screen
        .state_mut()
        .focus_sidebar_region(sidebar_layout::Side::Right);
    screen.run_steps(3);
    screen
        .ctx
        .memory_mut(|memory| memory.request_focus(composer));
    screen.run_steps(2);
    screen.state_mut().focus_explorer();
    screen.run_steps(3);
    screen.state_mut().focus_editor();
    screen.run_steps(2);
    assert!(
        !screen
            .state()
            .sidebar_view_visible(SidebarView::SourceControl)
    );
    screen.input_mut().events.extend([
        key(
            Key::G,
            egui::Modifiers::COMMAND.plus(egui::Modifiers::SHIFT),
        ),
        egui::Event::Text("commit suffix".into()),
    ]);
    screen.run_steps(4);
    assert_eq!(
        screen.state().focused_sidebar(),
        Some(sidebar_layout::Side::Right)
    );
    assert_eq!(screen.ctx.memory(|memory| memory.focused()), Some(composer));
    assert_eq!(screen.state().projects[0].commit_message, "commit suffix");
    assert_eq!(screen.state().tab().unwrap().text, source_text);

    screen.input_mut().events.extend([
        key(
            Key::E,
            egui::Modifiers::COMMAND.plus(egui::Modifiers::SHIFT),
        ),
        egui::Event::Text("explorer suffix".into()),
    ]);
    screen.run_steps(4);
    assert_eq!(
        screen.state().focused_sidebar(),
        Some(sidebar_layout::Side::Left)
    );
    assert!(screen.state().explorer_keyboard_focused());
    assert!(
        !screen
            .state()
            .sidebar_view_visible(SidebarView::SourceControl)
    );
    assert_eq!(screen.state().tab().unwrap().text, source_text);
    assert_eq!(screen.state().projects[0].commit_message, "commit suffix");
}

#[test]
fn offscreen_git_row_still_owns_region_navigation() {
    let mut app = fixture();
    app.projects[0].git_root = Some(app.projects[0].root.clone());
    app.projects[0].git_loading = false;
    app.projects[0].git_changes = (0..100)
        .map(|index| git::FileStatus {
            path: format!("file{index:03}.py"),
            original_path: None,
            index: git::FileChange::None,
            worktree: git::FileChange::Modified,
        })
        .collect();
    let row = egui::Id::new((
        "git-content",
        &app.projects[0].root,
        "working:changes:file:file099.py",
    ));
    let mut screen = harness(app);
    let sidebar = screen
        .state()
        .sidebar_region_rect(sidebar_layout::Side::Right)
        .unwrap();
    let row_response = screen.ctx.read_response(row).unwrap();
    assert!(
        !sidebar.contains(row_response.rect.center()),
        "The test must use a row outside the visible sidebar"
    );
    screen.ctx.memory_mut(|memory| memory.request_focus(row));
    assert_eq!(
        screen.state().focused_sidebar(),
        Some(sidebar_layout::Side::Right)
    );
    screen.state_mut().cycle_focus_region(false);
    screen.run_steps(2);
    assert!(
        screen.state().explorer_keyboard_focused(),
        "F6 must start from Git and wrap to Explorer"
    );
    screen.ctx.memory_mut(|memory| memory.request_focus(row));
    screen.state_mut().split_command("focus_left");
    screen.run_steps(2);
    assert_eq!(screen.state().focused_sidebar(), None);
    assert!(matches!(
        screen.state().navigation_region(),
        Region::Pane(_)
    ));
}

#[test]
fn printable_alt_focus_chord_does_not_type_its_character_into_output() {
    let mut app = fixture();
    app.output_visible = true;
    app.command.clear();
    app.settings
        .keybindings
        .insert("focus_output".into(), Some("Alt+O".into()));
    let text = app.tab().unwrap().text.clone();
    let mut screen = harness(app);
    screen.state_mut().focus_editor();
    screen.run_steps(2);
    screen.input_mut().events.extend([
        key(Key::O, egui::Modifiers::ALT),
        egui::Event::Text("o".into()),
        egui::Event::Text("destination".into()),
    ]);
    screen.run_steps(4);
    assert_eq!(screen.state().tab().unwrap().text, text);
    assert_eq!(screen.state().command, "destination");
}

#[test]
fn move_tab_to_adjacent_pane_preserves_dirty_buffer_and_selection() {
    let mut app = fixture();
    let source = app.current_pane_id();
    let tab = app.tab().unwrap().id;
    let destination = app.split_active(SplitOrientation::Vertical).unwrap();
    app.focus_editor_pane(source).unwrap();
    let mut screen = harness(app);
    screen.state_mut().focus_editor();
    {
        let document = screen.state_mut().tab_mut().unwrap();
        document.text.push_str("unsaved\n");
        document.dirty = true;
        document.set_cursor(6..9);
    }
    screen.run_steps(2);
    screen
        .state_mut()
        .move_tab_direction(Vec2::new(1.0, 0.0))
        .unwrap();
    screen.run_steps(2);
    assert_eq!(screen.state().current_pane_id(), destination);
    assert_eq!(screen.state().tab().unwrap().id, tab);
    assert!(screen.state().tab().unwrap().dirty);
    assert!(screen.state().tab().unwrap().text.ends_with("unsaved\n"));
    assert_eq!(screen.state().tab().unwrap().selection, 6..9);
    assert!(
        !screen
            .state()
            .editor_layout
            .panes
            .iter()
            .find(|pane| pane.id == source)
            .unwrap()
            .documents
            .contains(&tab)
    );
    assert!(
        screen
            .state_mut()
            .move_tab_direction(Vec2::new(1.0, 0.0))
            .unwrap_err()
            .contains("No editor pane")
    );
}

#[test]
fn directional_move_keeps_source_view_when_destination_shows_markdown_preview() {
    let mut app = fixture();
    app.projects[0].tabs[0].path = Some(app.projects[0].root.join("notes.md"));
    app.projects[0].tabs[0].title = "notes.md".into();
    let source = app.current_pane_id();
    let destination = app.split_active(SplitOrientation::Vertical).unwrap();
    app.set_document_view(DocumentView::Preview);
    app.focus_editor_pane(source).unwrap();
    let mut screen = harness(app);
    screen.state_mut().focus_editor();
    assert_eq!(
        screen.state().tab().unwrap().agent_view,
        DocumentView::Source
    );
    screen
        .state_mut()
        .move_tab_direction(Vec2::new(1.0, 0.0))
        .unwrap();
    screen.run_steps(2);
    assert_eq!(screen.state().current_pane_id(), destination);
    assert_eq!(
        screen.state().tab().unwrap().agent_view,
        DocumentView::Source
    );
}

#[test]
fn hiding_focused_output_restores_editor_keyboard_input() {
    let mut app = fixture();
    app.output_visible = true;
    let mut screen = harness(app);
    screen.state_mut().focus_output_region();
    screen.run_steps(2);
    screen.state_mut().output_visible = false;
    screen.run_steps(2);
    let tab = screen.state().tab().unwrap();
    assert_eq!(
        screen.ctx.memory(|memory| memory.focused()),
        Some(screen.state().document_widget_id(tab.id))
    );
    assert!(!screen.state().output_keyboard_focused());
}

#[test]
fn output_navigation_also_works_for_an_empty_workspace() {
    let mut app = fixture();
    app.projects[0].tabs.clear();
    app.editor_layout = EditorLayout::default();
    app.bind_current_pane();
    app.output_visible = true;
    app.sidebar_visible = false;
    app.sidebar_layout.right_visible = false;
    let mut screen = harness(app);
    screen.state_mut().focus_editor();
    screen.state_mut().cycle_focus_region(false);
    screen.run_steps(2);
    assert!(screen.state().output_keyboard_focused());
    screen.state_mut().split_command("focus_up");
    screen.run_steps(2);
    assert!(!screen.state().output_keyboard_focused());
    assert!(screen.state().tab().is_none());
}

fn focused_git_composer() -> Harness<'static, MorrowApp> {
    let mut app = fixture();
    app.projects[0].git_root = Some(app.projects[0].root.clone());
    app.projects[0].git_loading = false;
    app.tab_mut().unwrap().set_cursor(0..0);
    let composer = egui::Id::new(("git-content", &app.projects[0].root, "commit-message"));
    let mut screen = harness(app);
    screen
        .state_mut()
        .focus_sidebar_region(sidebar_layout::Side::Right);
    screen.run_steps(2);
    screen
        .ctx
        .memory_mut(|memory| memory.request_focus(composer));
    screen.run_steps(2);
    assert_eq!(screen.ctx.memory(|memory| memory.focused()), Some(composer));
    screen
}

#[test]
fn git_composer_prefix_stays_in_git_when_hide_shortcut_focuses_source() {
    let mut screen = focused_git_composer();
    screen.input_mut().events.extend([
        egui::Event::Text("commit prefix".into()),
        key(Key::B, egui::Modifiers::COMMAND.plus(egui::Modifiers::ALT)),
        egui::Event::Text("source suffix".into()),
    ]);
    screen.run_steps(6);
    assert_eq!(screen.state().projects[0].commit_message, "commit prefix");
    assert!(!screen.state().sidebar_layout.right_visible);
    assert_eq!(
        screen.state().tab().unwrap().text,
        "source suffixalpha\nbeta\ngamma\n"
    );
    let editor = screen
        .state()
        .document_widget_id(screen.state().tab().unwrap().id);
    assert_eq!(screen.ctx.memory(|memory| memory.focused()), Some(editor));
}

#[test]
fn git_composer_save_then_focus_editor_replays_source_suffix_in_order() {
    let mut screen = focused_git_composer();
    // An existing save keeps this shortcut test independent of filesystem workers.
    screen.state_mut().tab_mut().unwrap().saving = true;
    screen.input_mut().events.extend([
        key(Key::S, egui::Modifiers::COMMAND),
        key(Key::Num1, egui::Modifiers::COMMAND),
        egui::Event::Text("source suffix".into()),
    ]);
    screen.run_steps(6);
    assert!(screen.state().projects[0].commit_message.is_empty());
    assert_eq!(
        screen.state().tab().unwrap().text,
        "source suffixalpha\nbeta\ngamma\n"
    );
    let editor = screen
        .state()
        .document_widget_id(screen.state().tab().unwrap().id);
    assert_eq!(screen.ctx.memory(|memory| memory.focused()), Some(editor));
}

#[test]
fn output_prefix_stays_in_command_when_hide_shortcut_focuses_source() {
    let mut app = fixture();
    app.output_visible = true;
    app.command.clear();
    app.tab_mut().unwrap().set_cursor(0..0);
    let mut screen = harness(app);
    screen.state_mut().focus_output_region();
    screen.run_steps(2);
    screen.input_mut().events.extend([
        egui::Event::Text("command prefix".into()),
        key(Key::Backtick, egui::Modifiers::COMMAND),
        egui::Event::Text("source suffix".into()),
    ]);
    screen.run_steps(6);
    assert_eq!(screen.state().command, "command prefix");
    assert!(!screen.state().output_visible);
    assert_eq!(
        screen.state().tab().unwrap().text,
        "source suffixalpha\nbeta\ngamma\n"
    );
    let editor = screen
        .state()
        .document_widget_id(screen.state().tab().unwrap().id);
    assert_eq!(screen.ctx.memory(|memory| memory.focused()), Some(editor));
}

#[test]
fn output_save_then_focus_editor_replays_source_suffix_in_order() {
    let mut app = fixture();
    app.output_visible = true;
    app.command.clear();
    app.tab_mut().unwrap().set_cursor(0..0);
    let mut screen = harness(app);
    screen.state_mut().focus_output_region();
    screen.run_steps(2);
    // An existing save keeps this shortcut test independent of filesystem workers.
    screen.state_mut().tab_mut().unwrap().saving = true;
    screen.input_mut().events.extend([
        key(Key::S, egui::Modifiers::COMMAND),
        key(Key::Num1, egui::Modifiers::COMMAND),
        egui::Event::Text("source suffix".into()),
    ]);
    screen.run_steps(6);
    assert!(screen.state().command.is_empty());
    assert_eq!(
        screen.state().tab().unwrap().text,
        "source suffixalpha\nbeta\ngamma\n"
    );
    let editor = screen
        .state()
        .document_widget_id(screen.state().tab().unwrap().id);
    assert_eq!(screen.ctx.memory(|memory| memory.focused()), Some(editor));
}

#[test]
fn output_hide_dispatches_suffix_through_vim_after_restoring_source_focus() {
    let mut app = fixture();
    app.settings.vim = true;
    app.output_visible = true;
    app.command.clear();
    app.tab_mut().unwrap().set_cursor(0..0);
    let mut screen = harness(app);
    screen.state_mut().focus_output_region();
    screen.run_steps(2);
    screen.input_mut().events.extend([
        egui::Event::Text("command prefix".into()),
        key(Key::Backtick, egui::Modifiers::COMMAND),
        egui::Event::Text("iX".into()),
    ]);
    screen.run_steps(6);
    assert_eq!(screen.state().command, "command prefix");
    assert!(!screen.state().output_visible);
    assert_eq!(screen.state().tab().unwrap().text, "Xalpha\nbeta\ngamma\n");
    assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::Insert);
}

#[test]
fn find_and_replace_from_git_preserve_field_prefix_and_deliver_query_suffix() {
    for key_code in [Key::F, Key::H] {
        let mut screen = focused_git_composer();
        screen.input_mut().events.extend([
            egui::Event::Text("commit prefix".into()),
            key(key_code, egui::Modifiers::COMMAND),
            egui::Event::Text("needle".into()),
        ]);
        screen.run_steps(6);
        assert_eq!(screen.state().projects[0].commit_message, "commit prefix");
        assert_eq!(screen.state().find_text, "needle");
        assert!(screen.state().find_visible);
        assert_eq!(screen.state().replace_visible, key_code == Key::H);
        assert_eq!(screen.state().tab().unwrap().text, "alpha\nbeta\ngamma\n");
    }
}

#[test]
fn find_from_output_preserves_command_prefix_and_delivers_query_suffix() {
    let mut app = fixture();
    app.output_visible = true;
    app.command.clear();
    let mut screen = harness(app);
    screen.state_mut().focus_output_region();
    screen.run_steps(2);
    screen.input_mut().events.extend([
        egui::Event::Text("command prefix".into()),
        key(Key::F, egui::Modifiers::COMMAND),
        egui::Event::Text("needle".into()),
    ]);
    screen.run_steps(6);
    assert_eq!(screen.state().command, "command prefix");
    assert_eq!(screen.state().find_text, "needle");
    assert!(screen.state().find_visible);
    assert_eq!(screen.state().tab().unwrap().text, "alpha\nbeta\ngamma\n");
}

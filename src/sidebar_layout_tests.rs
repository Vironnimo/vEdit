use super::*;
use crate::cli::{Panel, Request};
use egui_kittest::{Harness, kittest::Queryable};

fn fixture() -> MorrowApp {
    let mut app = MorrowApp::empty(egui::Context::default());
    app.settings.autosave = false;
    let root = std::env::temp_dir().join("morrow-sidebar-layout-in-memory");
    let mut project = Project::new(root.clone());
    project.last_git_refresh = Instant::now() + Duration::from_secs(3600);
    let mut tab = EditorTab::new(Some(root.join("source.rs")), "source.rs".into());
    tab.text = "first\nsecond\nthird\n".into();
    tab.saved_text = tab.text.clone();
    project.tabs.push(tab);
    app.projects.push(project);
    app.bind_current_pane();
    app
}

fn harness(app: MorrowApp, width: f32) -> Harness<'static, MorrowApp> {
    let mut screen = Harness::builder()
        .with_size([width, 720.0])
        .with_os(egui::os::OperatingSystem::Windows)
        .build_ui_state(
            |ui, app: &mut MorrowApp| {
                app.ctx = ui.ctx().clone();
                if !app.sidebar_drag_keys() {
                    app.shortcuts();
                }
                app.activity(ui);
                app.sidebar_ui(ui);
                app.editor_ui(ui);
            },
            app,
        );
    screen.run_steps(3);
    screen
}

#[test]
fn git_can_open_alongside_explorer_and_hide_independently() {
    let mut app = fixture();
    app.show_sidebar(SidebarView::SourceControl);
    assert!(app.sidebar_view_visible(SidebarView::Explorer));
    assert!(app.sidebar_view_visible(SidebarView::SourceControl));
    app.toggle_sidebar(Side::Right);
    assert!(app.sidebar_view_visible(SidebarView::Explorer));
    assert!(!app.sidebar_view_visible(SidebarView::SourceControl));
    app.toggle_sidebar(Side::Right);
    app.toggle_sidebar(Side::Left);
    assert!(!app.sidebar_view_visible(SidebarView::Explorer));
    assert!(app.sidebar_view_visible(SidebarView::SourceControl));
}

#[test]
fn source_control_can_return_to_single_left_sidebar() {
    let mut app = fixture();
    app.move_sidebar(SidebarView::SourceControl, Side::Left);
    app.set_sidebar_visible(Side::Right, false);
    assert_eq!(app.sidebar, SidebarView::SourceControl);
    assert_eq!(app.sidebar_side(SidebarView::SourceControl), Side::Left);
    app.show_sidebar(SidebarView::SourceControl);
    assert!(!app.sidebar_layout.right_visible);
    app.move_sidebar(SidebarView::Search, Side::Right);
    assert!(app.sidebar_view_visible(SidebarView::Search));
    assert!(app.sidebar_view_visible(SidebarView::SourceControl));
    assert_ne!(app.sidebar, app.sidebar_layout.right);
}

#[test]
fn right_panel_assignments_survive_switching_and_session_restore() {
    let mut app = fixture();
    app.explorer_git_layout();
    app.move_sidebar(SidebarView::Search, Side::Right);
    assert_eq!(app.sidebar_layout.right, SidebarView::Search);
    app.show_sidebar(SidebarView::SourceControl);
    assert_eq!(app.sidebar, SidebarView::Explorer);
    assert_eq!(app.sidebar_layout.right, SidebarView::SourceControl);
    let session = app.sidebar_session();
    let mut restored = fixture();
    restored.restore_sidebars(Some(session), None);
    assert_eq!(restored.sidebar_side(SidebarView::Search), Side::Right);
    assert_eq!(
        restored.sidebar_side(SidebarView::SourceControl),
        Side::Right
    );
    restored.show_sidebar(SidebarView::Search);
    assert_eq!(restored.sidebar, SidebarView::Explorer);
    assert_eq!(restored.sidebar_layout.right, SidebarView::Search);
}

#[test]
fn showing_an_empty_side_does_not_move_or_restore_any_panel() {
    let mut app = fixture();
    app.move_sidebar(SidebarView::SourceControl, Side::Left);
    app.toggle_sidebar(Side::Right);
    assert!(app.sidebar_layout.right_visible);
    assert!(app.sidebar_views(Side::Right).is_empty());
    assert_eq!(app.sidebar_side(SidebarView::SourceControl), Side::Left);
    assert!(app.sidebar_view_visible(SidebarView::SourceControl));
    assert!(app.sidebar_view_visible(SidebarView::Explorer));
}

#[test]
fn sidebar_selection_visibility_and_widths_round_trip_through_session() {
    let mut app = fixture();
    app.move_sidebar(SidebarView::Explorer, Side::Right);
    app.sidebar_layout.left_width = 231.0;
    app.sidebar_layout.right_width = 377.0;
    app.set_sidebar_visible(Side::Left, false);
    let json = serde_json::to_string(&app.session()).unwrap();
    let saved: Session = serde_json::from_str(&json).unwrap();
    let mut restored = fixture();
    restored.restore_sidebars(saved.sidebars, saved.sidebar);
    assert!(restored.sidebar_views(Side::Left).is_empty());
    assert!(!restored.sidebar_visible);
    assert_eq!(restored.sidebar_layout.right, SidebarView::Explorer);
    assert!(restored.sidebar_layout.right_visible);
    assert_eq!(restored.sidebar_layout.left_width, 231.0);
    assert_eq!(restored.sidebar_layout.right_width, 377.0);
}

#[test]
fn old_sessions_keep_their_primary_panel_and_invalid_widths_are_safe() {
    let mut app = fixture();
    app.restore_sidebars(None, Some(SidebarView::SourceControl));
    assert_eq!(app.sidebar, SidebarView::SourceControl);
    assert!(app.sidebar_visible);
    assert!(!app.sidebar_layout.right_visible);
    assert_ne!(app.sidebar, app.sidebar_layout.right);
    let mut session = app.sidebar_session();
    session.left_width = f32::NAN;
    session.right_width = -10.0;
    app.restore_sidebars(Some(session), None);
    assert_eq!(app.sidebar_layout.left_width, 272.0);
    assert_eq!(app.sidebar_layout.right_width, MIN_SIDEBAR);
}

#[test]
fn hiding_or_replacing_focused_explorer_returns_input_to_editor() {
    let mut app = fixture();
    for right in [false, true] {
        if right {
            app.move_sidebar(SidebarView::Explorer, Side::Right);
        }
        app.focus_explorer();
        assert!(app.explorer_keyboard_focused());
        app.set_sidebar_view_visible(SidebarView::Explorer, false);
        assert!(!app.explorer_keyboard_focused());
        assert!(app.tab().unwrap().focus);
        app.cycle_focus_region(false);
        assert!(!app.sidebar_view_visible(SidebarView::Explorer));
        assert!(app.tab().unwrap().focus);
    }
}

#[test]
fn panel_cli_uses_assigned_side_and_hiding_absent_panel_is_a_noop() {
    let mut app = fixture();
    app.control_request(Request::Panel {
        panel: Panel::SourceControl,
        visible: true,
    });
    assert!(app.sidebar_visible);
    assert!(app.sidebar_layout.right_visible);
    app.control_request(Request::Panel {
        panel: Panel::Search,
        visible: false,
    });
    assert!(app.sidebar_visible);
    assert_eq!(app.sidebar, SidebarView::Explorer);
    app.control_request(Request::Panel {
        panel: Panel::RightSidebar,
        visible: false,
    });
    assert!(app.sidebar_visible);
    assert!(!app.sidebar_layout.right_visible);
    let status = app.sidebars_status();
    assert_eq!(status["left"]["panel"], "explorer");
    assert_eq!(status["right"]["panel"], "source_control");
    assert_eq!(status["right"]["visible"], false);
}

#[test]
fn both_large_sidebars_leave_room_for_editor_at_minimum_window_width() {
    let mut app = fixture();
    app.explorer_git_layout();
    app.sidebar_layout.left_width = MAX_SIDEBAR;
    app.sidebar_layout.right_width = MAX_SIDEBAR;
    let screen = harness(app, 960.0);
    let left = egui::containers::panel::PanelState::load(&screen.ctx, egui::Id::new("sidebar"))
        .unwrap()
        .size()
        .x;
    let right =
        egui::containers::panel::PanelState::load(&screen.ctx, egui::Id::new("right_sidebar"))
            .unwrap()
            .size()
            .x;
    assert!(
        left >= MIN_SIDEBAR && right >= MIN_SIDEBAR,
        "left={left}, right={right}"
    );
    assert!(
        left + right <= 960.0 - 49.0 - MIN_EDITOR + 2.0,
        "left={left}, right={right}"
    );
    assert_eq!(screen.state().sidebar_layout.left_width, MAX_SIDEBAR);
    assert_eq!(screen.state().sidebar_layout.right_width, MAX_SIDEBAR);
}

#[test]
fn narrow_window_collapses_one_side_and_can_reveal_the_other() {
    let mut app = fixture();
    app.explorer_git_layout();
    let mut screen = harness(app, 480.0);
    assert!(screen.state().sidebar_view_visible(SidebarView::Explorer));
    assert!(
        !screen
            .state()
            .sidebar_view_visible(SidebarView::SourceControl)
    );
    assert!(screen.state().sidebar_layout.right_visible);
    screen.get_by_label("Show right").click();
    screen.run_steps(3);
    assert!(!screen.state().sidebar_view_visible(SidebarView::Explorer));
    assert!(
        screen
            .state()
            .sidebar_view_visible(SidebarView::SourceControl)
    );
    let width =
        egui::containers::panel::PanelState::load(&screen.ctx, egui::Id::new("right_sidebar"))
            .unwrap()
            .size()
            .x;
    assert!(width >= MIN_SIDEBAR);
}

#[test]
fn region_cycle_reaches_both_visible_sidebars_without_unhiding_them() {
    let mut app = fixture();
    app.explorer_git_layout();
    let mut screen = harness(app, 1200.0);
    screen.state_mut().focus_editor();
    screen.state_mut().cycle_focus_region(false);
    screen.run_steps(2);
    assert_eq!(screen.state().focused_sidebar(), Some(Side::Right));
    screen.state_mut().cycle_focus_region(false);
    screen.run_steps(2);
    assert!(screen.state().explorer_keyboard_focused());
    screen.state_mut().set_sidebar_visible(Side::Left, false);
    screen.state_mut().set_sidebar_visible(Side::Right, false);
    screen.state_mut().cycle_focus_region(false);
    assert!(!screen.state().sidebar_visible);
    assert!(!screen.state().sidebar_layout.right_visible);
}

#[test]
fn search_added_to_sidebar_keeps_other_panels_and_query_focus() {
    let mut app = fixture();
    app.explorer_git_layout();
    let mut screen = harness(app, 1200.0);
    screen.get_by_label("Add panel to right sidebar").click();
    screen.run_steps(2);
    screen.get_by_label("Search").click();
    screen.run_steps(3);
    assert_eq!(screen.state().sidebar_layout.right, SidebarView::Search);
    assert!(
        screen
            .state()
            .sidebar_view_visible(SidebarView::SourceControl)
    );
    assert_eq!(
        screen.ctx.memory(|memory| memory.focused()),
        Some(egui::Id::new("project_search_query"))
    );
    screen
        .input_mut()
        .events
        .push(egui::Event::Text("needle".into()));
    screen.run_steps(2);
    assert_eq!(screen.state().project_query, "needle");
    assert_eq!(screen.state().tab().unwrap().text, "first\nsecond\nthird\n");
}

#[test]
fn safe_sidebar_shortcuts_work_while_vim_normal_mode_has_focus() {
    let mut screen = harness(fixture(), 1200.0);
    screen.state_mut().focus_editor();
    screen.run_steps(2);
    let modifiers = egui::Modifiers::COMMAND.plus(egui::Modifiers::ALT);
    for (key, side) in [(Key::B, Side::Right), (Key::E, Side::Left)] {
        let before = match side {
            Side::Left => screen.state().sidebar_visible,
            Side::Right => screen.state().sidebar_layout.right_visible,
        };
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
        screen.run_steps(2);
        let after = match side {
            Side::Left => screen.state().sidebar_visible,
            Side::Right => screen.state().sidebar_layout.right_visible,
        };
        assert_ne!(before, after);
        assert_eq!(screen.state().tab().unwrap().text, "first\nsecond\nthird\n");
    }
}

#[test]
fn toggle_reveals_a_temporarily_collapsed_sidebar_in_one_action() {
    let mut app = fixture();
    app.explorer_git_layout();
    let mut screen = harness(app, 480.0);
    assert!(
        !screen.state().sidebars_status()["right"]["effective_visible"]
            .as_bool()
            .unwrap()
    );
    screen.state_mut().toggle_sidebar(Side::Right);
    screen.run_steps(3);
    assert!(
        screen.state().sidebars_status()["right"]["effective_visible"]
            .as_bool()
            .unwrap()
    );
    assert!(
        screen.state().sidebar_visible,
        "compact switching must retain the desired left-side visibility"
    );
    screen.state_mut().toggle_sidebar(Side::Right);
    screen.run_steps(3);
    assert!(!screen.state().sidebar_layout.right_visible);
    assert!(
        screen.state().sidebars_status()["left"]["effective_visible"]
            .as_bool()
            .unwrap()
    );
}

#[test]
fn moving_panels_never_replaces_neighbors_and_search_can_be_alone_on_right() {
    let mut app = fixture();
    app.move_sidebar(SidebarView::SourceControl, Side::Left);
    app.move_sidebar(SidebarView::Search, Side::Right);
    assert_eq!(
        app.sidebar_views(Side::Left),
        vec![SidebarView::Explorer, SidebarView::SourceControl]
    );
    assert_eq!(app.sidebar_views(Side::Right), vec![SidebarView::Search]);
    for view in [
        SidebarView::Explorer,
        SidebarView::SourceControl,
        SidebarView::Search,
    ] {
        assert!(app.sidebar_view_visible(view));
    }
    app.move_sidebar(SidebarView::Explorer, Side::Right);
    assert!(app.sidebar_view_visible(SidebarView::SourceControl));
    assert!(app.sidebar_view_visible(SidebarView::Search));
    assert!(app.sidebar_view_visible(SidebarView::Explorer));
    app.set_sidebar_view_visible(SidebarView::Search, false);
    assert!(app.sidebar_view_visible(SidebarView::Explorer));
    assert!(app.sidebar_layout.right_visible);
    app.set_sidebar_view_visible(SidebarView::Explorer, false);
    assert!(!app.sidebar_layout.right_visible);
    assert!(app.sidebar_view_visible(SidebarView::SourceControl));
}

#[test]
fn all_assignments_and_panel_visibility_combinations_round_trip() {
    for assignments in 0..16 {
        for opened in 0..16 {
            let mut app = fixture();
            for (index, view) in VIEWS.into_iter().enumerate() {
                app.move_sidebar(
                    view,
                    if assignments & (1 << index) == 0 {
                        Side::Left
                    } else {
                        Side::Right
                    },
                );
            }
            for (index, view) in VIEWS.into_iter().enumerate() {
                app.set_sidebar_view_visible(view, opened & (1 << index) != 0);
            }
            for (left, right) in [(false, false), (true, false), (false, true), (true, true)] {
                app.set_sidebar_visible(Side::Left, left);
                app.set_sidebar_visible(Side::Right, right);
                let session =
                    serde_json::from_str(&serde_json::to_string(&app.sidebar_session()).unwrap())
                        .unwrap();
                let mut restored = fixture();
                restored.restore_sidebars(Some(session), None);
                for (index, view) in VIEWS.into_iter().enumerate() {
                    let side = if assignments & (1 << index) == 0 {
                        Side::Left
                    } else {
                        Side::Right
                    };
                    assert_eq!(restored.sidebar_side(view), side);
                    assert_eq!(
                        restored.sidebar_panel_open(view),
                        opened & (1 << index) != 0
                    );
                    assert_eq!(
                        restored.sidebar_view_visible(view),
                        opened & (1 << index) != 0 && if side == Side::Left { left } else { right }
                    );
                }
            }
        }
    }
}

#[test]
fn closing_the_last_panel_on_each_side_keeps_both_sides_empty_after_restore() {
    let mut app = fixture();
    app.move_sidebar(SidebarView::Search, Side::Right);
    app.set_sidebar_view_visible(SidebarView::Explorer, false);
    app.set_sidebar_view_visible(SidebarView::Search, false);
    assert!(app.visible_sidebar_sides().is_empty());
    let mut restored = fixture();
    restored.restore_sidebars(Some(app.sidebar_session()), None);
    assert!(restored.visible_sidebar_sides().is_empty());
    assert!(restored.sidebar_layout.open_views.is_empty());
    restored.toggle_sidebar(Side::Left);
    restored.toggle_sidebar(Side::Right);
    assert!(restored.sidebar_views(Side::Left).is_empty());
    assert!(restored.sidebar_views(Side::Right).is_empty());
}

#[test]
fn old_two_panel_sessions_migrate_without_changing_their_layout() {
    let old = r#"{"left":"source_control","left_visible":true,"right":"search","right_visible":true,"right_views":["search"],"left_width":260,"right_width":300}"#;
    let mut app = fixture();
    app.restore_sidebars(Some(serde_json::from_str(old).unwrap()), None);
    assert_eq!(
        app.sidebar_views(Side::Left),
        vec![SidebarView::SourceControl]
    );
    assert_eq!(app.sidebar_views(Side::Right), vec![SidebarView::Search]);
    assert!(!app.sidebar_panel_open(SidebarView::Explorer));
}

#[test]
fn panels_share_one_sidebar_without_overlapping_and_collapse_independently() {
    let mut app = fixture();
    app.move_sidebar(SidebarView::SourceControl, Side::Left);
    app.move_sidebar(SidebarView::Search, Side::Right);
    let mut screen = harness(app, 1200.0);
    let explorer = screen.state().sidebar_layout.view_rects[SidebarView::Explorer.index()].unwrap();
    let git = screen.state().sidebar_layout.view_rects[SidebarView::SourceControl.index()].unwrap();
    assert!(explorer.bottom() < git.top());
    assert!(
        git.bottom()
            <= screen
                .state()
                .sidebar_region_rect(Side::Left)
                .unwrap()
                .bottom()
    );
    screen.get_by_label("Collapse Explorer panel").click();
    screen.run_steps(3);
    assert!(!screen.state().sidebar_view_visible(SidebarView::Explorer));
    assert!(
        screen
            .state()
            .sidebar_view_visible(SidebarView::SourceControl)
    );
    assert!(screen.state().sidebar_view_visible(SidebarView::Search));
    let session = screen.state().sidebar_session();
    assert!(session.collapsed_views.contains(&SidebarView::Explorer));
    screen.get_by_label("Expand Explorer panel").click();
    screen.run_steps(3);
    assert!(screen.state().sidebar_view_visible(SidebarView::Explorer));
}

fn sidebar_pointer(screen: &mut Harness<'_, MorrowApp>, point: egui::Pos2, pressed: bool) {
    screen.event(egui::Event::PointerMoved(point));
    screen.event(egui::Event::PointerButton {
        pos: point,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    });
    screen.run_steps(1);
}

fn start_panel_drag(screen: &mut Harness<'_, MorrowApp>, view: SidebarView, target: egui::Pos2) {
    let start = screen.state().sidebar_layout.title_rects[view.index()]
        .unwrap()
        .center();
    sidebar_pointer(screen, start, true);
    screen.event(egui::Event::PointerMoved(start + Vec2::new(15.0, 0.0)));
    screen.run_steps(2);
    screen.event(egui::Event::PointerMoved(target));
    screen.run_steps(2);
    assert_eq!(screen.state().sidebar_layout.dragged, Some(view));
}

#[test]
fn dragging_a_panel_to_a_hidden_sidebar_and_back_preserves_its_neighbors() {
    let mut app = fixture();
    app.move_sidebar(SidebarView::SourceControl, Side::Left);
    let mut screen = harness(app, 1200.0);
    let target = egui::pos2(1170.0, 300.0);
    start_panel_drag(&mut screen, SidebarView::Explorer, target);
    sidebar_pointer(&mut screen, target, false);
    screen.run_steps(3);
    assert_eq!(
        screen.state().sidebar_views(Side::Left),
        vec![SidebarView::SourceControl]
    );
    assert_eq!(
        screen.state().sidebar_views(Side::Right),
        vec![SidebarView::Explorer]
    );
    let target = screen
        .state()
        .sidebar_region_rect(Side::Left)
        .unwrap()
        .center();
    start_panel_drag(&mut screen, SidebarView::Explorer, target);
    sidebar_pointer(&mut screen, target, false);
    screen.run_steps(3);
    assert!(screen.state().sidebar_view_visible(SidebarView::Explorer));
    assert!(
        screen
            .state()
            .sidebar_view_visible(SidebarView::SourceControl)
    );
    assert!(!screen.state().sidebar_layout.right_visible);
    assert_eq!(screen.state().tab().unwrap().text, "first\nsecond\nthird\n");
}

#[test]
fn panel_drag_can_reorder_and_cancel_without_changing_buffers_or_assignments() {
    let mut app = fixture();
    app.move_sidebar(SidebarView::SourceControl, Side::Left);
    let mut screen = harness(app, 1200.0);
    let target = screen.state().sidebar_layout.title_rects[SidebarView::Explorer.index()]
        .unwrap()
        .center();
    start_panel_drag(&mut screen, SidebarView::SourceControl, target);
    sidebar_pointer(&mut screen, target, false);
    screen.run_steps(3);
    assert_eq!(
        screen.state().sidebar_views(Side::Left),
        vec![SidebarView::SourceControl, SidebarView::Explorer]
    );
    let target = egui::pos2(1170.0, 300.0);
    start_panel_drag(&mut screen, SidebarView::SourceControl, target);
    screen.event(egui::Event::Text("must not be inserted".into()));
    screen.event(egui::Event::Key {
        key: Key::Escape,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    });
    screen.run_steps(2);
    sidebar_pointer(&mut screen, target, false);
    screen.run_steps(3);
    assert_eq!(
        screen.state().sidebar_side(SidebarView::SourceControl),
        Side::Left
    );
    assert!(!screen.state().sidebar_layout.right_visible);
    assert_eq!(screen.state().tab().unwrap().text, "first\nsecond\nthird\n");
}

#[test]
fn hiding_search_before_it_renders_cancels_pending_query_focus() {
    let mut screen = harness(fixture(), 1200.0);
    screen
        .state_mut()
        .move_sidebar(SidebarView::Search, Side::Right);
    screen.state_mut().open_project_search();
    screen
        .state_mut()
        .set_sidebar_view_visible(SidebarView::Search, false);
    assert!(!screen.state().project_search_focus);
    screen.run_steps(3);
    assert!(screen.ctx.memory(|memory| {
        memory.has_focus(
            screen
                .state()
                .document_widget_id(screen.state().tab().unwrap().id),
        )
    }));
    assert!(screen.state().sidebar_view_visible(SidebarView::Explorer));
}

#[test]
fn moving_search_preserves_expanded_filters_and_query() {
    let mut app = fixture();
    app.move_sidebar(SidebarView::Search, Side::Right);
    app.project_query = "keep this query".into();
    let mut screen = harness(app, 1200.0);
    screen.get_by_label_contains("Exclusions ·").click();
    screen.run_steps(3);
    assert!(
        screen
            .query_by_label("Exclude folders or patterns")
            .is_some()
    );
    screen
        .state_mut()
        .move_sidebar(SidebarView::Search, Side::Left);
    screen.run_steps(3);
    assert!(
        screen
            .query_by_label("Exclude folders or patterns")
            .is_some()
    );
    assert_eq!(screen.state().project_query, "keep this query");
    assert!(screen.state().sidebar_view_visible(SidebarView::Explorer));
    assert!(screen.state().sidebar_view_visible(SidebarView::Search));
    assert!(!screen.state().sidebar_layout.right_visible);
}

#[test]
fn panel_menu_moves_one_panel_without_removing_the_others() {
    let mut app = fixture();
    app.move_sidebar(SidebarView::SourceControl, Side::Left);
    let mut screen = harness(app, 1200.0);
    screen.get_by_label("Explorer panel actions").click();
    screen.run_steps(2);
    screen.get_by_label("Move panel to right").click();
    screen.run_steps(3);
    assert_eq!(
        screen.state().sidebar_views(Side::Left),
        vec![SidebarView::SourceControl]
    );
    assert_eq!(
        screen.state().sidebar_views(Side::Right),
        vec![SidebarView::Explorer]
    );
    screen.get_by_label("Source control panel actions").click();
    screen.run_steps(2);
    screen.get_by_label("Hide panel").click();
    screen.run_steps(3);
    assert!(!screen.state().sidebar_visible);
    assert!(screen.state().sidebar_view_visible(SidebarView::Explorer));
}

#[test]
fn opening_another_project_preserves_empty_and_hidden_sides() {
    for empty in [false, true] {
        let mut app = fixture();
        app.move_sidebar(SidebarView::Search, Side::Right);
        if empty {
            app.set_sidebar_view_visible(SidebarView::Explorer, false);
            app.set_sidebar_view_visible(SidebarView::Search, false);
        } else {
            app.set_sidebar_visible(Side::Left, false);
            app.set_sidebar_visible(Side::Right, false);
        }
        let before = app.sidebars_status();
        app.add_project(std::env::temp_dir().join("morrow-sidebar-preserve-project-in-memory"));
        assert_eq!(app.sidebars_status(), before);
        assert!(app.visible_sidebar_sides().is_empty());
    }
}

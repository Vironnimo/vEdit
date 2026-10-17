use super::*;
use crate::cli::{Panel, Request};
use egui_kittest::{Harness, kittest::Queryable};

fn fixture() -> VEditApp {
    let mut app = VEditApp::empty(egui::Context::default());
    app.settings.autosave = false;
    let root = std::env::temp_dir().join("vedit-sidebar-layout-in-memory");
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

fn harness(app: VEditApp, width: f32) -> Harness<'static, VEditApp> {
    let mut screen = Harness::builder()
        .with_size([width, 720.0])
        .with_os(egui::os::OperatingSystem::Windows)
        .build_ui_state(
            |ui, app: &mut VEditApp| {
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
    assert!(!app.sidebar_view_visible(SidebarView::Explorer));
    assert!(app.sidebar_panel_open(SidebarView::Explorer));
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
fn hiding_or_moving_an_offscreen_git_row_restores_editor_focus() {
    let mut app = fixture();
    let root = app.projects[0].root.clone();
    app.projects[0].git_root = Some(root.clone());
    app.projects[0].git_changes = (0..100)
        .map(|index| git::FileStatus {
            path: format!("file{index:03}.py"),
            original_path: None,
            index: git::FileChange::None,
            worktree: git::FileChange::Modified,
        })
        .collect();
    app.move_sidebar(SidebarView::SourceControl, Side::Left);
    let row = egui::Id::new(("git-content", &root, "working:changes:file:file099.py"));
    let mut screen = harness(app, 1400.0);
    let row_rect = screen.ctx.read_response(row).unwrap().rect;
    assert!(
        !screen.state().sidebar_layout.view_rects[SidebarView::SourceControl.index()]
            .unwrap()
            .contains(row_rect.center())
    );
    for moving in [false, true] {
        screen.state_mut().show_sidebar(SidebarView::SourceControl);
        screen.run_steps(2);
        screen.ctx.memory_mut(|memory| memory.request_focus(row));
        screen.state_mut().tab_mut().unwrap().focus = false;
        assert!(
            screen
                .state()
                .sidebar_view_has_focus(SidebarView::SourceControl)
        );
        if moving {
            screen
                .state_mut()
                .move_sidebar(SidebarView::SourceControl, Side::Right);
        } else {
            screen
                .state_mut()
                .set_sidebar_view_visible(SidebarView::SourceControl, false);
        }
        assert!(screen.state().tab().unwrap().focus);
        assert!(screen.state().sidebar_view_visible(SidebarView::Explorer));
        screen.run_steps(2);
        assert!(!screen.state().git_content_has_focus());
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
fn panel_menu_switching_to_search_keeps_git_available_and_focuses_query() {
    let mut app = fixture();
    app.explorer_git_layout();
    app.move_sidebar(SidebarView::Search, Side::Right);
    app.show_sidebar(SidebarView::SourceControl);
    let mut screen = harness(app, 1200.0);
    screen.get_by_label("Source control panel actions").click();
    screen.run_steps(2);
    screen.get_by_label("Show Search").click();
    screen.run_steps(3);
    assert_eq!(screen.state().sidebar_layout.right, SidebarView::Search);
    assert!(
        !screen
            .state()
            .sidebar_view_visible(SidebarView::SourceControl)
    );
    assert!(
        screen
            .state()
            .sidebar_panel_open(SidebarView::SourceControl)
    );
    assert!(screen.state().sidebar_view_visible(SidebarView::Search));
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
fn moving_panels_preserves_assignments_and_one_active_panel_per_side() {
    let mut app = fixture();
    app.move_sidebar(SidebarView::SourceControl, Side::Left);
    app.move_sidebar(SidebarView::Search, Side::Right);
    assert_eq!(
        app.sidebar_views(Side::Left),
        vec![SidebarView::Explorer, SidebarView::SourceControl]
    );
    assert_eq!(app.sidebar_views(Side::Right), vec![SidebarView::Search]);
    assert!(!app.sidebar_view_visible(SidebarView::Explorer));
    assert!(app.sidebar_view_visible(SidebarView::SourceControl));
    assert!(app.sidebar_view_visible(SidebarView::Search));
    app.move_sidebar(SidebarView::Explorer, Side::Right);
    assert!(app.sidebar_view_visible(SidebarView::SourceControl));
    assert!(!app.sidebar_view_visible(SidebarView::Search));
    assert!(app.sidebar_panel_open(SidebarView::Search));
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
                        opened & (1 << index) != 0
                            && restored.sidebar_selected_view(side) == Some(view)
                            && if side == Side::Left { left } else { right }
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
fn same_side_panel_menu_switches_content_and_keeps_full_sidebar_height() {
    let mut app = fixture();
    app.move_sidebar(SidebarView::SourceControl, Side::Left);
    app.move_sidebar(SidebarView::Search, Side::Right);
    let mut screen = harness(app, 1200.0);
    assert!(screen.state().sidebar_layout.view_rects[SidebarView::Explorer.index()].is_none());
    let git = screen.state().sidebar_layout.view_rects[SidebarView::SourceControl.index()].unwrap();
    assert!(git.height() > 500.0);
    screen.get_by_label("Source control panel actions").click();
    screen.run_steps(2);
    screen.get_by_label("Show Explorer").click();
    screen.run_steps(3);
    assert!(screen.state().sidebar_view_visible(SidebarView::Explorer));
    assert!(
        !screen
            .state()
            .sidebar_view_visible(SidebarView::SourceControl)
    );
    assert!(screen.state().sidebar_layout.view_rects[SidebarView::SourceControl.index()].is_none());
    let explorer = screen.state().sidebar_layout.view_rects[SidebarView::Explorer.index()].unwrap();
    assert!(explorer.height() > 500.0);
    assert!(screen.state().sidebar_view_visible(SidebarView::Search));
    assert!(screen.query_by_label("Collapse Explorer panel").is_none());
    assert!(screen.query_by_label("LEFT SIDEBAR").is_none());
    screen.get_by_label("Explorer panel actions").click();
    screen.run_steps(2);
    screen.get_by_label("Show Source control").click();
    screen.run_steps(3);
    assert!(
        screen
            .state()
            .sidebar_view_visible(SidebarView::SourceControl)
    );
    assert!(!screen.state().sidebar_view_visible(SidebarView::Explorer));
}

#[test]
fn old_stacked_sessions_restore_assignments_with_one_active_panel() {
    let old = r#"{"left":"source_control","left_visible":true,"right":"search","right_visible":true,"right_views":["search"],"open_views":["explorer","source_control","search"],"collapsed_views":["source_control"]}"#;
    let mut app = fixture();
    app.restore_sidebars(Some(serde_json::from_str(old).unwrap()), None);
    assert_eq!(
        app.sidebar_views(Side::Left),
        vec![SidebarView::Explorer, SidebarView::SourceControl]
    );
    assert!(app.sidebar_view_visible(SidebarView::SourceControl));
    assert!(!app.sidebar_view_visible(SidebarView::Explorer));
    assert!(app.sidebar_view_visible(SidebarView::Search));
    assert!(app.sidebar_session().collapsed_views.is_empty());
    app.show_sidebar(SidebarView::Explorer);
    assert!(app.sidebar_view_visible(SidebarView::Explorer));
    assert!(!app.sidebar_view_visible(SidebarView::SourceControl));
}

#[test]
fn switching_away_from_focused_search_restores_editor_without_losing_query() {
    let mut app = fixture();
    app.open_project_search();
    app.project_query = "retained query".into();
    let mut screen = harness(app, 1200.0);
    assert!(
        screen.state().project_search_focus
            || screen
                .ctx
                .memory(|memory| memory.has_focus(egui::Id::new("project_search_query")))
    );
    screen.state_mut().show_sidebar(SidebarView::Explorer);
    assert!(screen.state().tab().unwrap().focus);
    screen.run_steps(3);
    assert!(!screen.state().sidebar_view_visible(SidebarView::Search));
    assert!(!screen.state().project_search_focus);
    assert_eq!(screen.state().project_query, "retained query");
    assert!(screen.ctx.memory(|memory| {
        memory.has_focus(
            screen
                .state()
                .document_widget_id(screen.state().tab().unwrap().id),
        )
    }));
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
fn hiding_search_sidebar_before_render_restores_source_focus() {
    let mut screen = harness(fixture(), 1200.0);
    screen.state_mut().open_project_search();
    assert!(screen.state().project_search_focus);
    screen.state_mut().set_sidebar_visible(Side::Left, false);
    assert!(!screen.state().project_search_focus);
    assert!(screen.state().tab().unwrap().focus);
    screen.run_steps(3);
    assert!(screen.ctx.memory(|memory| {
        memory.has_focus(
            screen
                .state()
                .document_widget_id(screen.state().tab().unwrap().id),
        )
    }));
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
    assert!(!screen.state().sidebar_view_visible(SidebarView::Explorer));
    assert!(screen.state().sidebar_panel_open(SidebarView::Explorer));
    assert!(screen.state().sidebar_view_visible(SidebarView::Search));
    assert!(!screen.state().sidebar_layout.right_visible);
}

#[test]
fn panel_menu_moves_one_panel_without_removing_the_others() {
    let mut app = fixture();
    app.move_sidebar(SidebarView::SourceControl, Side::Left);
    app.show_sidebar(SidebarView::Explorer);
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
        app.add_project(std::env::temp_dir().join("vedit-sidebar-preserve-project-in-memory"));
        assert_eq!(app.sidebars_status(), before);
        assert!(app.visible_sidebar_sides().is_empty());
    }
}

fn overflowing_explorer_fixture(side: Side, width: f32) -> VEditApp {
    let mut app = fixture();
    let root = app.projects[0].root.clone();
    let mut tree = workspace::ExplorerTree::new(root.clone());
    let generation = tree.begin(&root).unwrap();
    let mut entries: Vec<_> = (0..160)
        .map(|index| workspace::WorkspaceEntry {
            path: root.join(format!("file{index:03}.rs")),
            name: format!("file{index:03}.rs"),
            is_dir: false,
            ignored: false,
        })
        .collect();
    entries.push(workspace::WorkspaceEntry {
        path: root.join("source.rs"),
        name: "source.rs".into(),
        is_dir: false,
        ignored: false,
    });
    assert!(tree.complete(
        &root,
        generation,
        Ok(workspace::ExplorerDirectory {
            root: root.clone(),
            path: root.clone(),
            entries,
            warnings: Vec::new(),
        })
    ));
    app.projects[0].index = Some(tree.snapshot());
    app.settings.smooth_scroll = false;
    if side == Side::Right {
        app.move_sidebar(SidebarView::Explorer, Side::Right);
        app.set_sidebar_visible(Side::Left, false);
        app.sidebar_layout.right_width = width;
    } else {
        app.set_sidebar_visible(Side::Right, false);
        app.sidebar_layout.left_width = width;
    }
    app
}

fn assert_sidebar_edge_painted(screen: &mut Harness<'_, VEditApp>, side: Side, width: f32) {
    let rect = screen.state().sidebar_region_rect(side).unwrap();
    assert!(
        (rect.width() - width).abs() <= 1.0,
        "header contents must not expand the reserved panel width: {rect:?}, expected {width}"
    );
    let inner_edge = match side {
        Side::Left => rect.right() - 4.0,
        Side::Right => rect.left() + 4.0,
    };
    let scale = screen.ctx.pixels_per_point();
    let image = screen.render().expect("render sidebar edge pixels");
    // Check above the title, beside scrolled rows and below the scrollbar. These
    // points stay clear of the one-pixel separator and pointer affordance.
    for y in [rect.top() + 4.0, rect.center().y, rect.bottom() - 4.0] {
        let x = (inner_edge * scale).floor() as u32;
        let y = (y * scale).floor() as u32;
        assert_eq!(
            image.get_pixel(x, y).0,
            PANEL.to_array(),
            "the sidebar frame must paint its entire reserved edge on {side:?} at {scale}x ({x},{y})"
        );
    }
    let bar = screen
        .query_all_by_role(egui::accesskit::Role::ScrollBar)
        .map(|node| node.rect())
        .find(|bar| {
            rect.contains_rect(*bar)
                && bar.width() > 0.0
                && bar.width() <= 20.0
                && bar.height() > 100.0
        })
        .expect("overflowing Explorer keeps its vertical scrollbar");
    assert!(bar.top() > rect.top() && bar.bottom() < rect.bottom());
}

#[test]
fn sidebar_frame_has_no_unpainted_gutter_at_native_dpi_or_after_resize() {
    for side in [Side::Left, Side::Right] {
        let mut screen = visual_tests::harness(overflowing_explorer_fixture(side, 292.0));
        screen.hover_at(egui::pos2(-20.0, -20.0));
        screen.run_steps(4);
        assert_sidebar_edge_painted(&mut screen, side, 292.0);
        screen
            .input_mut()
            .viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .native_pixels_per_point = Some(1.25);
        screen.run_steps(4);
        assert!((screen.ctx.pixels_per_point() - 1.25).abs() < 0.001);
        assert_sidebar_edge_painted(&mut screen, side, 292.0);
        let rect = screen.state().sidebar_region_rect(side).unwrap();
        let resize_id = egui::Id::new(if side == Side::Left {
            "sidebar"
        } else {
            "right_sidebar"
        })
        .with("__resize");
        let handle = screen
            .ctx
            .read_response(resize_id)
            .expect("sidebar resize handle");
        let own_handle = handle.interact_rect.intersect(rect);
        assert!(own_handle.width() > 0.0 && own_handle.height() > 100.0);
        // The mathematical boundary also belongs to the later editor panel.
        // Hover the visible resize strip inside this sidebar's actual hit rect.
        let edge = egui::pos2(handle.rect.center().x, rect.center().y - 90.0);
        let press = egui::pos2(own_handle.center().x, edge.y);
        screen.hover_at(press);
        screen.run_steps(2);
        let handle = screen.ctx.read_response(resize_id).unwrap();
        assert!(
            handle.hovered(),
            "side={side:?}, press={press:?}, panel={rect:?}, resize={handle:?}"
        );
        assert_eq!(
            screen.output().platform_output.cursor_icon,
            egui::CursorIcon::ResizeHorizontal,
            "side={side:?}, press={press:?}, panel={rect:?}, resize={handle:?}"
        );
        assert_sidebar_edge_painted(&mut screen, side, 292.0);
        screen.drag_at(press);
        screen.run_steps(1);
        let resized = edge + Vec2::new(if side == Side::Left { 40.0 } else { -40.0 }, 0.0);
        screen.hover_at(resized);
        screen.run_steps(2);
        assert_sidebar_edge_painted(&mut screen, side, 332.0);
        screen.drop_at(resized);
        screen.hover_at(egui::pos2(-20.0, -20.0));
        screen.run_steps(3);
        assert_sidebar_edge_painted(&mut screen, side, 332.0);
        // A compact panel must keep actions and the actual scrollbar inside it.
        match side {
            Side::Left => screen.state_mut().sidebar_layout.left_width = MIN_SIDEBAR,
            Side::Right => screen.state_mut().sidebar_layout.right_width = MIN_SIDEBAR,
        }
        screen.run_steps(3);
        assert_sidebar_edge_painted(&mut screen, side, MIN_SIDEBAR);
    }
}

#[test]
fn panel_actions_hover_never_moves_the_header_or_content() {
    for side in [Side::Left, Side::Right] {
        for view in VIEWS {
            let mut app = fixture();
            app.move_sidebar(view, side);
            app.show_sidebar(view);
            let mut screen = harness(app, 1200.0);
            set_style(&screen.ctx);
            screen.hover_at(egui::pos2(-20.0, -20.0));
            screen.run_steps(3);
            let label = format!("{} panel actions", view.title());
            let menu = screen.get_by_label(&label).rect();
            let content = screen.state().sidebar_layout.view_rects[view.index()].unwrap();
            let panel = screen.state().sidebar_region_rect(side).unwrap();
            assert!(menu.height() <= 24.0);
            assert!(
                content.top() - panel.top() <= 44.0,
                "compact header: {panel:?} {content:?}"
            );
            screen.hover_at(menu.center());
            screen.run_steps(3);
            assert_eq!(
                screen.get_by_label(&label).rect(),
                menu,
                "{side:?} {} button resized on hover",
                view.title()
            );
            assert_eq!(
                screen.state().sidebar_layout.view_rects[view.index()],
                Some(content),
                "{side:?} {} content moved on hover",
                view.title()
            );
            screen.hover_at(egui::pos2(-20.0, -20.0));
            screen.run_steps(2);
            assert_eq!(
                screen.state().sidebar_layout.view_rects[view.index()],
                Some(content)
            );
        }
    }
}

#[test]
fn sidebar_header_shows_only_active_title_and_keeps_other_panels_in_menu() {
    let mut app = fixture();
    app.move_sidebar(SidebarView::SourceControl, Side::Left);
    app.move_sidebar(SidebarView::Search, Side::Left);
    app.show_sidebar(SidebarView::Explorer);
    let mut screen = harness(app, 1200.0);
    for active in [
        SidebarView::Explorer,
        SidebarView::SourceControl,
        SidebarView::Search,
    ] {
        let side = screen.state().sidebar_region_rect(Side::Left).unwrap();
        for view in VIEWS {
            assert!(
                screen
                    .query_by_label(&format!("{} panel tab", view.title()))
                    .is_none()
            );
            let titles: Vec<_> = screen
                .query_all_by_label(&if view == SidebarView::Explorer {
                    file_title(&screen.state().project().unwrap().root)
                } else {
                    format!("{} panel title", view.title())
                })
                .map(|node| node.rect())
                .collect();
            if view == active {
                assert_eq!(titles.len(), 1, "one quiet title for the active panel");
                assert!(side.contains_rect(titles[0]));
            } else {
                assert!(
                    titles.is_empty(),
                    "inactive panels never duplicate the rail in the header"
                );
            }
        }
        let next = match active {
            SidebarView::Explorer => SidebarView::SourceControl,
            SidebarView::SourceControl => SidebarView::Search,
            _ => SidebarView::Explorer,
        };
        screen
            .get_by_label(&format!("{} panel actions", active.title()))
            .click();
        screen.run_steps(2);
        screen
            .get_by_label(&format!("Show {}", next.title()))
            .click();
        screen.run_steps(3);
        assert!(screen.state().sidebar_view_visible(next));
        assert_eq!(screen.state().sidebar_views(Side::Left).len(), 3);
    }
}

#[test]
fn title_actions_still_move_hide_and_restore_panel_assignments() {
    let mut app = fixture();
    app.move_sidebar(SidebarView::SourceControl, Side::Left);
    app.show_sidebar(SidebarView::Explorer);
    let mut screen = harness(app, 1200.0);
    screen.get_by_label("Explorer panel actions").click();
    screen.run_steps(2);
    screen.get_by_label("Move panel to right").click();
    screen.run_steps(3);
    assert_eq!(
        screen.state().sidebar_side(SidebarView::Explorer),
        Side::Right
    );
    assert!(
        screen
            .state()
            .sidebar_view_visible(SidebarView::SourceControl)
    );
    assert!(screen.state().sidebar_view_visible(SidebarView::Explorer));
    assert!(
        screen
            .query_by_label(&file_title(&screen.state().project().unwrap().root))
            .is_some()
    );
    assert!(
        screen
            .query_by_label("Source control panel title")
            .is_some()
    );
    // The header has no close button; hiding the sidebar is a menu action.
    assert!(screen.query_by_label("Hide right sidebar").is_none());
    screen.get_by_label("Explorer panel actions").click();
    screen.run_steps(2);
    screen.get_by_label("Hide right sidebar").click();
    screen.run_steps(3);
    assert!(!screen.state().sidebar_view_visible(SidebarView::Explorer));
    assert!(screen.state().sidebar_panel_open(SidebarView::Explorer));
    let saved =
        serde_json::from_str(&serde_json::to_string(&screen.state().sidebar_session()).unwrap())
            .unwrap();
    let mut restored = fixture();
    restored.restore_sidebars(Some(saved), None);
    assert_eq!(restored.sidebar_side(SidebarView::Explorer), Side::Right);
    assert!(!restored.sidebar_layout.right_visible);
    assert!(restored.sidebar_view_visible(SidebarView::SourceControl));
    restored.show_sidebar(SidebarView::Explorer);
    assert!(restored.sidebar_view_visible(SidebarView::Explorer));
}

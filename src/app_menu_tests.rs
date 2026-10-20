use super::*;
use egui_kittest::{Harness, kittest::Queryable};

fn fixture() -> VEditApp {
    let mut app = VEditApp::empty(egui::Context::default());
    visual_tests::workspace(&mut app);
    app.move_sidebar(SidebarView::SourceControl, sidebar_layout::Side::Left);
    app.move_sidebar(SidebarView::Search, sidebar_layout::Side::Left);
    app.show_sidebar(SidebarView::Explorer);
    app
}

fn harness(app: VEditApp, width: f32, height: f32) -> Harness<'static, VEditApp> {
    let mut screen = Harness::builder()
        .with_size([width, height])
        .with_theme(egui::Theme::Dark)
        .with_os(egui::os::OperatingSystem::Windows)
        .build_ui_state(
            |ui, app| {
                app.ctx = ui.ctx().clone();
                app.draw_ui(ui);
            },
            app,
        );
    set_style(&screen.ctx);
    screen.run_steps(3);
    screen
}

fn open_menu(screen: &mut Harness<'_, VEditApp>, section: &str) {
    screen.get_by_label("Main menu").click();
    screen.run_steps(2);
    screen.get_by_label(section).click();
    screen.run_steps(3);
}

#[test]
fn rail_menu_is_the_only_menu_entry_and_panel_navigation_remains_accessible() {
    let mut screen = harness(fixture(), 1000.0, 640.0);
    for label in ["File", "View", "Run"] {
        assert!(screen.query_by_label(label).is_none());
    }
    let main = screen.get_by_label("Main menu").rect();
    assert!(main.bottom() < screen.get_by_label("Explorer").rect().top());
    for label in ["Explorer", "Search project", "Source control"] {
        assert_eq!(screen.query_all_by_label(label).count(), 1);
    }
    screen.get_by_label("Source control").click();
    screen.run_steps(3);
    assert!(
        screen
            .state()
            .sidebar_view_visible(SidebarView::SourceControl)
    );
    assert!(
        screen
            .query_by_label("Source control panel title")
            .is_some()
    );
    assert!(screen.query_by_label("Explorer panel title").is_none());
    screen.get_by_label("Explorer").click();
    screen.run_steps(3);
    assert!(screen.state().sidebar_view_visible(SidebarView::Explorer));
    assert!(screen.query_by_label("Explorer panel tab").is_none());
}

#[test]
fn welcome_menu_and_file_view_run_actions_work_at_the_native_minimum_size() {
    let mut welcome = harness(VEditApp::empty(egui::Context::default()), 500.0, 360.0);
    open_menu(&mut welcome, "File");
    welcome.get_by_label_contains("Settings Ctrl").click();
    welcome.run_steps(3);
    assert!(welcome.state().settings_open);

    for (section, action) in [
        ("File", "New file Ctrl+N"),
        ("View", "Search project Ctrl+Shift+F"),
        ("Run", "Configure project tasks"),
    ] {
        let mut screen = harness(fixture(), 500.0, 360.0);
        open_menu(&mut screen, section);
        let response = screen.get_by_label_contains(action);
        assert!(
            screen.ctx.content_rect().contains_rect(response.rect()),
            "{section}/{action} must remain reachable in a snapped window"
        );
        response.click();
        screen.run_steps(3);
        match section {
            "File" => assert_eq!(screen.state().projects[0].tabs.len(), 3),
            "View" => assert!(screen.state().sidebar_view_visible(SidebarView::Search)),
            "Run" => assert!(screen.state().settings_open),
            _ => unreachable!(),
        }
    }
}

#[test]
fn custom_close_uses_the_unsaved_document_guard_and_cancel_preserves_text() {
    let mut app = fixture();
    let tab = app.tab_mut().unwrap();
    tab.text.push_str("unsaved change\n");
    tab.dirty = true;
    let original = tab.text.clone();
    let mut screen = harness(app, 500.0, 360.0);
    screen.get_by_label("Close window").click();
    screen.run_steps(3);
    assert!(matches!(
        screen.state().close_action,
        Some(CloseAction::Exit)
    ));
    assert!(!screen.state().exit_ready);
    assert_eq!(screen.state().tab().unwrap().text, original);
    assert!(screen.state().chrome_blocked());
    screen.get_by_label("Cancel").click();
    screen.run_steps(3);
    assert!(screen.state().close_action.is_none());
    assert!(!screen.state().exit_ready);
    assert!(screen.state().tab().unwrap().dirty);
    assert_eq!(screen.state().tab().unwrap().text, original);
}

#[test]
fn file_rename_dialog_blocks_titlebar_drag_until_it_is_cancelled() {
    let mut app = fixture();
    let path = app.tab().unwrap().path.clone().unwrap();
    let original = app.tab().unwrap().text.clone();
    app.context_begin_rename(&path, false);
    let mut screen = harness(app, 700.0, 500.0);
    let geometry = || {
        screen
            .ctx
            .data(|data| {
                data.get_temp::<crate::window_chrome::ChromeGeometry>(egui::Id::new(
                    "vedit_window_chrome",
                ))
            })
            .unwrap()
    };
    assert!(screen.state().chrome_blocked());
    assert!(geometry().drag_regions.is_empty());
    screen.get_by_label("Cancel").click();
    screen.run_steps(4);
    assert!(screen.state().context_file_dialog.is_none());
    assert!(!screen.state().chrome_blocked());
    let geometry = screen.ctx.data(|data| {
        data.get_temp::<crate::window_chrome::ChromeGeometry>(egui::Id::new("vedit_window_chrome"))
    });
    assert!(!geometry.unwrap().drag_regions.is_empty());
    assert_eq!(screen.state().tab().unwrap().text, original);
}

#[test]
fn compact_status_bar_keeps_mode_save_state_and_cursor_position_separate() {
    for width in [500.0, 250.0] {
        let mut app = fixture();
        app.projects[0].git_branch = Some("long-project-branch-name".into());
        let mut screen = harness(app, width, 360.0);
        let mode = screen.get_by_label(" NORMAL ").rect();
        let saved = screen.get_by_label("Saved").rect();
        let position = if width >= 500.0 {
            screen.get_by_label_contains("Ln 1/").rect()
        } else {
            screen.get_by_label("1:1").rect()
        };
        assert!(mode.right() < saved.left(), "mode/save overlap at {width}");
        assert!(
            saved.right() < position.left(),
            "save/cursor overlap at {width}"
        );
        for rect in [mode, saved, position] {
            assert!(screen.ctx.content_rect().contains_rect(rect));
        }
        screen.state_mut().tab_mut().unwrap().dirty = true;
        screen.run_steps(3);
        let unsaved = screen.get_by_label("Unsaved").rect();
        assert!(mode.right() < unsaved.left());
    }
}

#[test]
fn status_bar_explains_why_large_files_are_plain_text() {
    const HINT: &str = "Plain text: file too large for highlighting";
    let mut app = fixture();
    let tab = app.tab_mut().unwrap();
    tab.path = Some(PathBuf::from("C:/vedit-status/large.rs"));
    tab.text = "fn value() -> u32 { 42 }
"
    .repeat(9_000);
    assert!(tab.text.len() > crate::render::MAX_DOCUMENT_BYTES);
    tab.saved_text = tab.text.clone();
    tab.changed();
    let mut screen = harness(app, 1200.0, 500.0);
    let hint = screen.get_by_label(HINT);
    hint.hover();
    screen.run_steps(3);
    assert!(
        screen
            .query_by_label_contains("larger than 192 KiB")
            .is_some()
    );

    let tab = screen.state_mut().tab_mut().unwrap();
    tab.text = "fn value() -> u32 { 42 }
"
    .into();
    tab.changed();
    screen.run_steps(3);
    assert!(screen.query_by_label(HINT).is_none());
}

#[test]
fn main_menu_and_nested_file_menu_screenshots() {
    let mut screen = visual_tests::harness(fixture());
    visual_tests::settle_render(&mut screen);
    screen.get_by_label("Main menu").click();
    screen.run_steps(3);
    screen.hover_at(screen.get_by_label("Main menu").rect().center());
    screen.run_steps(2);
    if let Some(directory) = std::env::var_os("VEDIT_SNAPSHOT_DIR") {
        screen.snapshot_options(
            "vedit_main_menu",
            &egui_kittest::SnapshotOptions::new().output_path(PathBuf::from(&directory)),
        );
    } else {
        screen.snapshot("vedit_main_menu");
    }
    screen.get_by_label("File").hover();
    screen.run_steps(4);
    if let Some(directory) = std::env::var_os("VEDIT_SNAPSHOT_DIR") {
        screen.snapshot_options(
            "vedit_file_menu",
            &egui_kittest::SnapshotOptions::new().output_path(PathBuf::from(directory)),
        );
    } else {
        screen.snapshot("vedit_file_menu");
    }
}

#[test]
fn status_bar_aligns_mode_branch_and_save_text_with_room_for_labels() {
    for width in [500.0, 960.0, 1440.0] {
        let screen = harness(fixture(), width, 640.0);
        let text_rect = |label: &str| {
            screen
                .output()
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == label => {
                        Some(text.galley.rect.translate(text.pos.to_vec2()))
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("missing status text {label}"))
        };
        let mode = text_rect(" NORMAL ");
        let branch = text_rect("main");
        let saved = text_rect("Saved");
        for (label, rect) in [("branch", branch), ("save state", saved)] {
            assert!(
                (mode.center().y - rect.center().y).abs() <= 1.0,
                "{label} does not align at {width}: mode={mode:?}, text={rect:?}"
            );
        }
        assert!(branch.left() - mode.right() >= 8.0);
        assert!(screen.ctx.content_rect().bottom() - mode.bottom() >= 6.0);
    }
}

#[test]
fn text_statistics_follow_unsaved_text_and_selection_without_overlapping_status() {
    for width in [250.0, 500.0, 700.0, 960.0, 1140.0, 1440.0] {
        let mut app = fixture();
        let tab = app.tab_mut().unwrap();
        tab.text = "ä👩‍💻\r\n".into();
        tab.changed();
        let mut screen = harness(app, width, 640.0);
        let position = screen
            .get_by_label(if width < 360.0 {
                "1:1"
            } else {
                "Ln 1/2, Col 1"
            })
            .rect();
        let saved = screen.get_by_label("Unsaved").rect();
        let mode = screen.get_by_label(" NORMAL ").rect();
        if width >= 700.0 {
            let chars = screen
                .get_by_label_contains(if width < 1100.0 {
                    "6 ch ·"
                } else {
                    "6 chars ·"
                })
                .rect();
            assert!(
                saved.right() < chars.left(),
                "save/statistics overlap at {width}"
            );
            assert!(
                chars.right() < position.left(),
                "statistics/position overlap at {width}"
            );
            assert!(screen.ctx.content_rect().contains_rect(chars));
            let tab = screen.state_mut().tab_mut().unwrap();
            tab.set_cursor(1..1);
            tab.vim.handle_key(&mut tab.text, VimKey::Char('v'));
            tab.vim.cursor = 3;
            tab.pending_selection = tab.vim.selection(&tab.text);
            screen.run_steps(3);
            assert!(
                screen
                    .query_by_label_contains(if width < 1100.0 {
                        "Sel: 3 ch"
                    } else {
                        "Selection: 3 chars"
                    })
                    .is_some()
            );
        } else {
            assert!(screen.query_by_label_contains(" chars ·").is_none());
            screen.ctx.all_styles_mut(|style| {
                style.interaction.tooltip_delay = 0.0;
                style.interaction.tooltip_grace_time = 0.0;
                style.interaction.show_tooltips_only_when_still = false;
            });
            screen.hover_at(position.center());
            screen.run_steps(3);
            assert!(
                screen
                    .query_by_label_contains("File: 6 characters")
                    .is_some()
            );
        }
        assert!(mode.right() < saved.left(), "mode/save overlap at {width}");
        for rect in [mode, saved, position] {
            assert!(screen.ctx.content_rect().contains_rect(rect));
        }
    }
}

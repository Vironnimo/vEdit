//! Exercise the editor's dialog and toolbar flows through real pointer/key input.

use super::*;
use egui_kittest::{Harness, kittest::Queryable};

fn fixture(text: &str, markdown: bool) -> VEditApp {
    let mut app = VEditApp::empty(egui::Context::default());
    app.settings.autosave = false;
    app.settings.smooth_scroll = false;
    app.sidebar_visible = false;
    let root = std::env::temp_dir().join("vedit-editor-ux-render-only");
    let mut project = Project::new(root.clone());
    let name = if markdown { "plan.md" } else { "notes.txt" };
    let mut tab = EditorTab::new(Some(root.join(name)), name.into());
    tab.text = text.into();
    tab.saved_text = text.into();
    project.tabs.push(tab);
    app.projects.push(project);
    app.bind_current_pane();
    app
}

fn harness(app: VEditApp, size: [f32; 2]) -> Harness<'static, VEditApp> {
    harness_with(app, size, Harness::<VEditApp>::builder())
}

/// Frames 1/60 s apart, so edits stay within the text widget's Undo grouping.
fn fast_harness(app: VEditApp, size: [f32; 2]) -> Harness<'static, VEditApp> {
    harness_with(
        app,
        size,
        Harness::<VEditApp>::builder().with_step_dt(1.0 / 60.0),
    )
}

fn harness_with(
    app: VEditApp,
    size: [f32; 2],
    builder: egui_kittest::HarnessBuilder<VEditApp>,
) -> Harness<'static, VEditApp> {
    let mut screen = builder
        .with_size(size)
        .with_os(egui::os::OperatingSystem::Windows)
        .build_ui_state(
            |ui, app: &mut VEditApp| {
                app.ctx = ui.ctx().clone();
                // These fixtures never read/write documents or user recovery data.
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

fn text(screen: &mut Harness<'_, VEditApp>, value: &str) {
    screen.event(egui::Event::Text(value.into()));
    screen.run_steps(2);
}

fn key(screen: &mut Harness<'_, VEditApp>, key: Key, modifiers: egui::Modifiers) {
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
}

#[test]
fn settings_escape_isolates_normal_insert_and_native_editing() {
    for (vim, insert) in [(true, false), (true, true), (false, false)] {
        let original = "first line\nsecond line\n";
        let mut app = fixture(original, false);
        app.settings.vim = vim;
        let mut screen = harness(app, [960.0, 640.0]);
        if insert {
            text(&mut screen, "i");
        }
        let widget = screen
            .state()
            .document_widget_id(screen.state().tab().unwrap().id);
        assert!(screen.ctx.memory(|memory| memory.has_focus(widget)));

        key(&mut screen, Key::Comma, egui::Modifiers::COMMAND);
        assert!(screen.state().settings_open);
        assert!(screen.query_by_label("Done").is_some());
        text(&mut screen, "unwanted edit");
        assert_eq!(screen.state().tab().unwrap().text, original);
        assert!(!screen.state().tab().unwrap().dirty);

        key(&mut screen, Key::Escape, egui::Modifiers::NONE);
        screen.run_steps(2);
        assert!(!screen.state().settings_open);
        assert!(screen.ctx.memory(|memory| memory.has_focus(widget)));
        assert_eq!(screen.state().tab().unwrap().text, original);
        if vim && !insert {
            text(&mut screen, "j");
            assert_eq!(
                screen.state().tab().unwrap().vim.cursor,
                "first line\n".chars().count()
            );
            assert_eq!(screen.state().tab().unwrap().text, original);
        } else {
            text(&mut screen, "ok");
            assert!(screen.state().tab().unwrap().text.starts_with("ok"));
        }
    }
}

#[test]
fn settings_done_fits_short_window_and_restores_preview_reading() {
    let mut app = fixture("# Reading start\n\nA plan to read.\n", true);
    app.set_document_view(crate::cli::DocumentView::Preview);
    let mut screen = harness(app, [960.0, 640.0]);
    key(&mut screen, Key::Comma, egui::Modifiers::COMMAND);
    // Let the centered modal settle after its first measured layout.
    screen.run_steps(3);
    let done = screen.get_by_label("Done").rect();
    assert!(done.bottom() < 640.0);
    assert!(done.top() > 0.0);
    screen.get_by_label("Wrap long lines").click();
    screen.run_steps(2);
    assert!(
        screen.state().settings.word_wrap,
        "Wrap control did not toggle: settings_open={}, rect={:?}",
        screen.state().settings_open,
        screen.get_by_label("Wrap long lines").rect()
    );
    screen.get_by_label("Done").click();
    screen.run_steps(3);
    assert!(!screen.state().settings_open);
    assert_eq!(
        screen.state().tab().unwrap().agent_view,
        crate::cli::DocumentView::Preview
    );
    assert!(screen.ctx.memory(|memory| memory.focused().is_none()));
    assert!(!screen.state().tab().unwrap().dirty);
}

#[test]
fn find_replace_controls_fit_a_narrow_split_and_escape_restores_that_pane() {
    let original = "foo Foo food foo\nsecond line\n";
    let mut app = fixture(original, false);
    app.sidebar_visible = true;
    app.split_active(split::SplitOrientation::Vertical).unwrap();
    app.find_visible = true;
    app.replace_visible = true;
    app.find_focus = true;
    app.find_text = "foo".into();
    app.find_next(false);
    let mut screen = harness(app, [960.0, 640.0]);
    let pane_id = screen.state().current_pane_id();
    let pane = screen
        .state()
        .tab_drag_pane_rects()
        .into_iter()
        .find(|(id, _)| *id == pane_id)
        .unwrap()
        .1;
    assert!(
        pane.width() < 400.0,
        "fixture must exercise a narrow real split: {pane:?}"
    );
    for label in [
        "Find in file",
        "Replace with",
        "Close find",
        "Match case",
        "Whole word",
        "Previous match",
        "Next match",
        "Replace",
        "Replace all",
    ] {
        let rect = screen.get_by_label(label).rect();
        assert!(
            rect.left() >= pane.left() && rect.right() <= pane.right() + 1.0,
            "{label} escapes pane: {rect:?} vs {pane:?}"
        );
    }
    screen.get_by_label("Replace with").click();
    screen.run_steps(2);
    text(&mut screen, "replacement");
    assert_eq!(screen.state().replace_text, "replacement");
    assert_eq!(screen.state().tab().unwrap().text, original);
    key(&mut screen, Key::Escape, egui::Modifiers::NONE);
    screen.run_steps(2);
    assert!(!screen.state().find_visible);
    let widget = screen
        .state()
        .document_widget_id(screen.state().tab().unwrap().id);
    assert!(screen.ctx.memory(|memory| memory.has_focus(widget)));
    assert_eq!(screen.state().tab().unwrap().text, original);
}

#[test]
fn find_options_update_results_without_editing_and_close_button_restores_focus() {
    let original = "foo Foo food foo\n";
    let mut app = fixture(original, false);
    app.find_visible = true;
    app.find_focus = true;
    app.find_text = "foo".into();
    app.find_next(false);
    let mut screen = harness(app, [1200.0, 760.0]);
    assert_eq!(screen.state_mut().editor_search_matches().len(), 4);
    screen.get_by_label("Match case").click();
    screen.run_steps(2);
    assert_eq!(screen.state_mut().editor_search_matches().len(), 3);
    screen.get_by_label("Whole word").click();
    screen.run_steps(2);
    assert_eq!(screen.state_mut().editor_search_matches().len(), 2);
    assert_eq!(screen.state().tab().unwrap().text, original);
    screen.get_by_label("Next match").click();
    screen.run_steps(2);
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 13);
    screen.get_by_label("Close find").click();
    screen.run_steps(3);
    assert!(!screen.state().find_visible);
    let widget = screen
        .state()
        .document_widget_id(screen.state().tab().unwrap().id);
    assert!(screen.ctx.memory(|memory| memory.has_focus(widget)));
    assert!(!screen.state().tab().unwrap().dirty);
}

#[test]
fn replacement_enter_does_not_skip_adjacent_matches_and_updates_count() {
    let mut app = fixture("foofoo 🦀foo\n", false);
    app.find_visible = true;
    app.replace_visible = true;
    app.find_focus = true;
    app.find_text = "foo".into();
    app.find_next(false);
    let mut screen = harness(app, [1200.0, 760.0]);
    screen.get_by_label("Replace with").click();
    screen.run_steps(2);
    for (expected, remaining) in [("foo 🦀foo\n", 2), (" 🦀foo\n", 1), (" 🦀\n", 0)] {
        key(&mut screen, Key::Enter, egui::Modifiers::NONE);
        assert_eq!(screen.state().tab().unwrap().text, expected);
        assert_eq!(screen.state_mut().editor_search_matches().len(), remaining);
    }
    assert!(
        screen
            .state()
            .status
            .as_ref()
            .is_some_and(|(_, error, _)| !error)
    );
    key(&mut screen, Key::Escape, egui::Modifiers::NONE);
    assert!(!screen.state().find_visible);
}

#[test]
fn native_undo_restores_the_exact_text_before_replace_and_replace_all() {
    let mut app = fixture("alpha foo beta foo\n", false);
    app.settings.vim = false;
    app.find_visible = true;
    app.replace_visible = true;
    app.find_text = "foo".into();
    app.replace_text = "bar".into();
    let mut screen = fast_harness(app, [1200.0, 760.0]);
    screen.state_mut().tab_mut().unwrap().set_cursor(0..0);
    // Typing and replacing quickly after each other would otherwise share
    // one native Undo step, and Undo would also remove the typing.
    let widget = screen
        .state()
        .document_widget_id(screen.state().tab().unwrap().id);
    screen.ctx.memory_mut(|memory| memory.request_focus(widget));
    screen.run_steps(1);
    text(&mut screen, "typed ");
    let typed = "typed alpha foo beta foo\n";
    assert_eq!(screen.state().tab().unwrap().text, typed);
    screen.get_by_label("Replace all").click();
    screen.run_steps(2);
    assert_eq!(
        screen.state().tab().unwrap().text,
        "typed alpha bar beta bar\n"
    );
    key(&mut screen, Key::Escape, egui::Modifiers::NONE);
    key(&mut screen, Key::Z, egui::Modifiers::COMMAND);
    assert_eq!(
        screen.state().tab().unwrap().text,
        typed,
        "one Undo reverts Replace all and keeps the typing before it"
    );

    screen.state_mut().find_visible = true;
    screen.state_mut().tab_mut().unwrap().set_cursor(0..0);
    screen.ctx.memory_mut(|memory| memory.request_focus(widget));
    screen.run_steps(1);
    text(&mut screen, "more ");
    let typed = "more typed alpha foo beta foo\n";
    assert_eq!(screen.state().tab().unwrap().text, typed);
    screen.get_by_label("Replace").click();
    screen.run_steps(2);
    assert_eq!(
        screen.state().tab().unwrap().text,
        "more typed alpha bar beta foo\n"
    );
    key(&mut screen, Key::Escape, egui::Modifiers::NONE);
    key(&mut screen, Key::Z, egui::Modifiers::COMMAND);
    assert_eq!(
        screen.state().tab().unwrap().text,
        typed,
        "one Undo reverts Replace and keeps the typing before it"
    );
}

#[test]
fn find_toolbar_regex_toggle_replaces_groups_reports_invalid_patterns_and_persists() {
    let original = "user@example and admin@host\n";
    let mut app = fixture(original, false);
    app.settings.vim = false;
    app.find_visible = true;
    app.replace_visible = true;
    app.find_text = r"(\w+)@(\w+)".into();
    app.replace_text = "$2 at $1".into();
    let mut screen = harness(app, [1200.0, 760.0]);
    assert!(screen.state_mut().editor_search_matches().is_empty());
    screen.get_by_label("Use regular expression").click();
    screen.run_steps(2);
    assert!(screen.state().find_regex);
    assert_eq!(screen.state_mut().editor_search_matches().len(), 2);
    let session: Session =
        serde_json::from_slice(&serde_json::to_vec(&screen.state().session()).unwrap()).unwrap();
    assert!(session.find_regex, "the toggle is kept in the session");

    screen.state_mut().tab_mut().unwrap().set_cursor(0..0);
    screen.get_by_label("Replace").click();
    screen.run_steps(2);
    assert_eq!(
        screen.state().tab().unwrap().text,
        "example at user and admin@host\n"
    );
    screen.get_by_label("Replace all").click();
    screen.run_steps(2);
    assert_eq!(
        screen.state().tab().unwrap().text,
        "example at user and host at admin\n"
    );

    screen.state_mut().find_text = "(unclosed".into();
    screen.run_steps(2);
    assert!(screen.query_by_label("Invalid pattern").is_some());
    assert!(screen.state_mut().editor_search_matches().is_empty());
    screen.get_by_label("Use regular expression").click();
    screen.run_steps(2);
    assert!(screen.query_by_label("Invalid pattern").is_none());
    assert_eq!(
        screen.state().tab().unwrap().text,
        "example at user and host at admin\n"
    );
}

fn bracket_outlines(screen: &Harness<'_, VEditApp>) -> Vec<egui::Rect> {
    let stroke = crate::palette::MUTED.linear_multiply(0.45);
    let mut rects: Vec<_> = screen
        .output()
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Rect(rect) if rect.stroke.color == stroke => Some(rect.rect),
            _ => None,
        })
        .collect();
    rects.sort_by(|a, b| a.left().total_cmp(&b.left()));
    rects
}

#[test]
fn the_bracket_at_the_caret_and_its_partner_are_outlined_in_the_focused_pane() {
    let mut screen = harness(fixture("call(a, [b])\n", false), [1200.0, 760.0]);
    let text = |screen: &mut Harness<'_, VEditApp>, value: &str| {
        screen.event(egui::Event::Text(value.into()));
        screen.run_steps(2);
    };
    assert!(bracket_outlines(&screen).is_empty());
    text(&mut screen, "4l");
    let outlines = bracket_outlines(&screen);
    assert_eq!(outlines.len(), 2, "( and ) are outlined: {outlines:?}");
    assert!((outlines[0].center().y - outlines[1].center().y).abs() < 1.0);
    let width = outlines[0].width();
    assert!(width > 4.0 && (outlines[1].width() - width).abs() < 0.5);
    let column = (outlines[1].left() - outlines[0].left()) / 7.0;
    assert!(
        (0.85..1.15).contains(&(column / width)),
        "the outlines cover columns 5 and 12: {outlines:?}"
    );

    // A block cursor just after the bracket outlines nothing; the Insert
    // caret there belongs to the bracket before it.
    text(&mut screen, "l");
    assert!(bracket_outlines(&screen).is_empty());
    text(&mut screen, "i");
    assert_eq!(bracket_outlines(&screen).len(), 2);
    assert!(!screen.state().tab().unwrap().dirty);
}

#[test]
fn large_find_shows_pending_progress_and_accepts_query_refinement() {
    let original = "needle here\n".repeat(25_000);
    assert!(original.len() > 256 * 1024);
    let mut app = fixture(&original, false);
    app.find_visible = true;
    app.find_focus = true;
    app.find_text = "needle".into();
    // Draw the actual toolbar against a large buffer, without laying out its
    // source galley. Its first count lookup starts the real debounced worker.
    let mut screen = Harness::builder().with_size([380.0, 220.0]).build_ui_state(
        |ui, app: &mut VEditApp| {
            app.ctx = ui.ctx().clone();
            app.validate_vim_search();
            app.find_ui(ui);
        },
        app,
    );
    assert!(screen.state().find_search_running());
    assert!(screen.query_by_label("Searching…").is_some());
    screen.get_by_label("Find in file").click();
    screen.run_steps(1);
    key(&mut screen, Key::A, egui::Modifiers::COMMAND);
    text(&mut screen, "here");
    assert_eq!(screen.state().find_text, "here");
    assert_eq!(screen.state().tab().unwrap().text, original);

    let deadline = Instant::now() + Duration::from_secs(3);
    while screen.state().find_search_running() {
        assert!(
            Instant::now() < deadline,
            "actual file-search worker did not complete"
        );
        std::thread::sleep(Duration::from_millis(5));
        screen.run_steps(1);
    }
    screen.run_steps(2);
    assert!(screen.query_by_label("Searching…").is_none());
    assert_eq!(screen.state_mut().editor_search_matches().len(), 25_000);
    assert!(screen.query_by_label_contains("of 25000").is_some());
    assert!(!screen.state().tab().unwrap().dirty);

    // Clearing a new in-flight query must cancel its pending navigation too.
    key(&mut screen, Key::A, egui::Modifiers::COMMAND);
    text(&mut screen, "eed");
    assert!(screen.state().find_search_running());
    key(&mut screen, Key::A, egui::Modifiers::COMMAND);
    key(&mut screen, Key::Backspace, egui::Modifiers::NONE);
    assert!(screen.state().find_text.is_empty());
    screen.run_steps(2);
    assert!(!screen.state().find_search_running());
    assert!(!screen.state().vim_search_active());
    assert!(screen.state_mut().editor_search_matches().is_empty());
    assert_eq!(screen.state().tab().unwrap().text, original);
}

fn suggestions(screen: &mut Harness<'_, VEditApp>, count: usize) {
    text(screen, "i");
    screen.state_mut().tab_mut().unwrap().set_cursor(2..2);
    screen.run_steps(2);
    let tab = screen.state().tab().unwrap();
    let id = tab.id;
    let revision = tab.revision;
    let cursor = tab.vim.cursor;
    let path = tab.path.clone().unwrap();
    // The generic editing fixture uses notes.txt. Its simulated language
    // service must explicitly own that extension, just like a configured one.
    let extension = path.extension().unwrap().to_string_lossy().into_owned();
    {
        let app = screen.state_mut();
        app.settings
            .language_servers
            .iter_mut()
            .find(|config| config.language_id == "rust")
            .unwrap()
            .extensions = vec![extension.clone()];
        app.projects[0].rust_language_mut().config.extensions = vec![extension];
    }
    screen.state_mut().projects[0]
        .rust_language_mut()
        .opened
        .insert(path, (revision, 7));
    screen.state_mut().completions = Some(CompletionPopup {
        tab_id: id,
        version: 7,
        cursor,
        selected: 0,
        items: (0..count)
            .map(|index| crate::lsp::CompletionItem {
                label: format!("candidate{index:03}"),
                detail: Some(format!("Completion detail {index}")),
                insert_text: format!("candidate{index:03}"),
                text_edit: None,
                additional_text_edits: Vec::new(),
            })
            .collect(),
    });
    screen.run_steps(2);
}

#[test]
fn completion_popup_preserves_remapped_modified_keys_and_command_event_order() {
    let mut screen = harness(fixture("fo", false), [960.0, 640.0]);
    suggestions(&mut screen, 3);
    screen.state_mut().settings.keybindings.insert(
        keybindings::Action::ToggleWrap.id().into(),
        Some("Alt+Enter".into()),
    );
    key(&mut screen, Key::Enter, egui::Modifiers::ALT);
    assert!(screen.state().settings.word_wrap);
    assert_eq!(screen.state().tab().unwrap().text, "fo");
    assert!(screen.state().completions.is_some());
    screen.state_mut().settings.keybindings.insert(
        keybindings::Action::ToggleWrap.id().into(),
        Some("F9".into()),
    );
    screen
        .input_mut()
        .events
        .extend([Key::F9, Key::Enter].map(|key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }));
    screen.run_steps(6);
    assert!(!screen.state().settings.word_wrap);
    assert_eq!(screen.state().tab().unwrap().text, "candidate000");
}

#[test]
fn completion_navigation_reveals_results_beyond_one_hundred_and_enter_accepts_visible_item() {
    let mut screen = harness(fixture("fo", false), [960.0, 640.0]);
    suggestions(&mut screen, 180);
    for _ in 0..110 {
        key(&mut screen, Key::ArrowDown, egui::Modifiers::NONE);
    }
    assert_eq!(screen.state().completions.as_ref().unwrap().selected, 110);
    let row = screen.get_by_label_contains("candidate110").rect();
    assert!(
        row.top() >= 0.0 && row.bottom() <= 640.0,
        "selected suggestion must be visible: {row:?}"
    );
    assert!(screen.query_by_label_contains("111 of 180").is_some());
    key(&mut screen, Key::Enter, egui::Modifiers::NONE);
    assert!(screen.state().completions.is_none());
    assert_eq!(screen.state().tab().unwrap().text, "candidate110");
}

#[test]
fn completion_pointer_can_accept_an_item_beyond_one_hundred() {
    let mut screen = harness(fixture("fo", false), [1200.0, 760.0]);
    suggestions(&mut screen, 180);
    for _ in 0..110 {
        key(&mut screen, Key::ArrowDown, egui::Modifiers::NONE);
    }
    screen.get_by_label_contains("candidate109").click();
    screen.run_steps(2);
    assert!(screen.state().completions.is_none());
    assert_eq!(screen.state().tab().unwrap().text, "candidate109");
}

#[test]
fn completion_enter_reveals_an_offscreen_selection_before_accepting_it() {
    let mut screen = harness(fixture("fo", false), [1200.0, 760.0]);
    suggestions(&mut screen, 180);
    screen.get_by_label_contains("candidate003").hover();
    screen.run_steps(2);
    screen.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: Vec2::new(0.0, -1500.0),
        modifiers: egui::Modifiers::NONE,
        phase: egui::TouchPhase::Move,
    });
    screen.run_steps(20);
    assert!(screen.query_by_label_contains("candidate000").is_none());
    assert_eq!(screen.state().completions.as_ref().unwrap().selected, 0);
    key(&mut screen, Key::Enter, egui::Modifiers::NONE);
    assert!(screen.state().completions.is_some());
    assert_eq!(screen.state().tab().unwrap().text, "fo");
    let selected = screen.get_by_label_contains("candidate000").rect();
    assert!(selected.top() >= 0.0 && selected.bottom() <= 760.0);
    key(&mut screen, Key::Enter, egui::Modifiers::NONE);
    assert!(screen.state().completions.is_none());
    assert_eq!(screen.state().tab().unwrap().text, "candidate000");
}

#[test]
fn completion_reopening_identical_results_reveals_the_first_selection_after_mouse_scroll() {
    let mut screen = harness(fixture("fo", false), [1200.0, 760.0]);
    suggestions(&mut screen, 180);
    screen.get_by_label_contains("candidate003").hover();
    screen.run_steps(2);
    screen.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: Vec2::new(0.0, -1500.0),
        modifiers: egui::Modifiers::NONE,
        phase: egui::TouchPhase::Move,
    });
    screen.run_steps(20);
    assert!(screen.query_by_label_contains("candidate000").is_none());
    assert_eq!(screen.state().completions.as_ref().unwrap().selected, 0);
    assert_eq!(screen.state().tab().unwrap().text, "fo");
    // A new identical server result must reveal its selected row even though
    // the popup's document, version, cursor and selection have not changed.
    let popup = screen.state_mut().completions.take().unwrap();
    screen.run_steps(3);
    screen.state_mut().completions = Some(popup);
    screen.run_steps(3);
    let row = screen.get_by_label_contains("candidate000").rect();
    assert!(row.top() >= 0.0 && row.bottom() <= 760.0);
    key(&mut screen, Key::Enter, egui::Modifiers::NONE);
    assert_eq!(screen.state().tab().unwrap().text, "candidate000");
}

#[test]
fn narrow_find_replace_screenshot() {
    let mut app = fixture("", false);
    visual_tests::workspace(&mut app);
    app.bind_current_pane();
    app.split_active(split::SplitOrientation::Vertical).unwrap();
    app.find_visible = true;
    app.replace_visible = true;
    app.find_focus = true;
    app.find_text = "path".into();
    app.replace_text = "document_path".into();
    app.find_next(false);
    let mut narrow = visual_tests::harness(app);
    narrow.set_size(Vec2::new(960.0, 720.0));
    visual_tests::snapshot(&mut narrow, "vedit_narrow_find_replace");
}

#[test]
fn long_suggestions_screenshot() {
    let mut app = fixture("fo", false);
    visual_tests::workspace(&mut app);
    app.bind_current_pane();
    let mut completions = visual_tests::harness(app);
    completions.run_steps(3);
    suggestions(&mut completions, 180);
    completions
        .state_mut()
        .completions
        .as_mut()
        .unwrap()
        .selected = 110;
    visual_tests::snapshot(&mut completions, "vedit_long_suggestions");
}

#[test]
fn small_settings_screenshot() {
    let mut app = fixture("", false);
    visual_tests::workspace(&mut app);
    app.bind_current_pane();
    app.settings_open = true;
    let mut settings = visual_tests::harness(app);
    settings.set_size(Vec2::new(960.0, 640.0));
    visual_tests::snapshot(&mut settings, "vedit_small_settings");
}

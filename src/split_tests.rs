//! Panes must isolate navigation while sharing the actual document and history.

use super::*;
use egui_kittest::Harness;

fn fixture() -> MorrowApp {
    let mut app = MorrowApp::empty(egui::Context::default());
    app.settings.autosave = false;
    app.settings.smooth_scroll = false;
    app.sidebar_visible = false;
    let root = std::env::temp_dir().join("morrow-split-in-memory");
    let mut project = Project::new(root.clone());
    project.last_git_refresh = Instant::now();
    for (name, text) in [
        ("first.txt", "first document\nsecond line\nthird line\n"),
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

fn harness(app: MorrowApp) -> Harness<'static, MorrowApp> {
    let mut screen = Harness::builder()
        .with_size([1360.0, 900.0])
        .with_os(egui::os::OperatingSystem::Windows)
        .build_ui_state(
            |ui, app: &mut MorrowApp| {
                app.ctx = ui.ctx().clone();
                // UI-only fixtures must never run disk polling, Git, autosave,
                // recovery writes, or session writes into the user's app data.
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

fn click_source(screen: &mut Harness<'_, MorrowApp>, pane: u64) {
    let rect = screen.state().editor_layout.rects[&pane];
    // Tabs occupy the first 37 points; the first source line starts below it.
    let position = rect.left_top() + Vec2::new(70.0, 60.0);
    screen.event(egui::Event::PointerMoved(position));
    screen.event(egui::Event::PointerButton {
        pos: position,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    screen.run_steps(1);
    screen.event(egui::Event::PointerButton {
        pos: position,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    screen.run_steps(2);
    assert_eq!(screen.state().editor_layout.focused, pane);
    let id = screen
        .state()
        .document_widget_id(screen.state().tab().unwrap().id);
    assert!(screen.ctx.memory(|memory| memory.has_focus(id)));
}

fn type_text(screen: &mut Harness<'_, MorrowApp>, text: &str) {
    screen.event(egui::Event::Text(text.into()));
    screen.run_steps(2);
}

fn painted_text_y(screen: &Harness<'_, MorrowApp>, pane: u64, text: &str) -> f32 {
    fn find(shape: &egui::Shape, rect: egui::Rect, text: &str) -> Option<f32> {
        match shape {
            egui::Shape::Text(shape)
                if shape.galley.job.text == text
                    && shape.pos.x >= rect.left()
                    && shape.pos.x < rect.right() =>
            {
                Some(shape.pos.y)
            }
            egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| find(shape, rect, text)),
            _ => None,
        }
    }
    let rect = screen.state().editor_layout.rects[&pane];
    screen
        .output()
        .shapes
        .iter()
        .find_map(|shape| find(&shape.shape, rect, text))
        .unwrap_or_else(|| {
            panic!(
                "Expected painted text in pane {pane}: {}",
                text.chars().take(40).collect::<String>()
            )
        })
}

fn painted_tab_center(
    screen: &Harness<'_, MorrowApp>,
    pane: u64,
    title: &str,
) -> Option<egui::Pos2> {
    fn find(shape: &egui::Shape, rect: egui::Rect, title: &str) -> Option<egui::Pos2> {
        match shape {
            egui::Shape::Text(shape)
                if shape.galley.job.text == title
                    && shape.pos.x >= rect.left()
                    && shape.pos.x < rect.right()
                    && shape.pos.y >= rect.top()
                    && shape.pos.y < rect.top() + 37.0 =>
            {
                Some(shape.pos + shape.galley.size() * 0.5)
            }
            egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| find(shape, rect, title)),
            _ => None,
        }
    }
    let rect = screen.state().editor_layout.rects[&pane];
    screen
        .output()
        .shapes
        .iter()
        .find_map(|shape| find(&shape.shape, rect, title))
}

fn click_tab(
    screen: &mut Harness<'_, MorrowApp>,
    pane: u64,
    title: &str,
    button: egui::PointerButton,
) {
    let position = painted_tab_center(screen, pane, title)
        .unwrap_or_else(|| panic!("Expected tab {title} in pane {pane}"));
    screen.event(egui::Event::PointerMoved(position));
    screen.event(egui::Event::PointerButton {
        pos: position,
        button,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    screen.run_steps(1);
    screen.event(egui::Event::PointerButton {
        pos: position,
        button,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    screen.run_steps(2);
}

fn pane_documents(app: &MorrowApp, pane: u64) -> Vec<u64> {
    app.editor_layout
        .panes
        .iter()
        .find(|candidate| candidate.id == pane)
        .unwrap_or_else(|| panic!("Expected pane {pane}"))
        .documents
        .clone()
}

#[test]
fn pointer_focus_routes_native_selection_and_typing_to_only_one_document() {
    let mut app = fixture();
    app.settings.vim = false;
    let first = app.projects[0].tabs[0].id;
    let second = app.projects[0].tabs[1].id;
    let right = app.split_active(SplitOrientation::Vertical).unwrap();
    app.assign_editor_pane(right, second, Some(DocumentView::Source))
        .unwrap();
    let mut screen = harness(app);

    click_source(&mut screen, 1);
    screen.key_press_modifiers(egui::Modifiers::COMMAND, Key::A);
    screen.run_steps(3);
    assert_eq!(
        screen.state().tab().unwrap().selection,
        0..screen.state().tab().unwrap().text.chars().count()
    );
    type_text(&mut screen, "LEFT");
    assert_eq!(screen.state().projects[0].tabs[0].text, "LEFT");
    assert_eq!(
        screen.state().projects[0].tabs[1].text,
        "another document\nits second line\n"
    );

    click_source(&mut screen, right);
    screen.key_press_modifiers(egui::Modifiers::COMMAND, Key::A);
    screen.run_steps(3);
    assert_eq!(
        screen.state().tab().unwrap().selection,
        0..screen.state().tab().unwrap().text.chars().count()
    );
    type_text(&mut screen, "RIGHT");
    assert_eq!(screen.state().projects[0].tabs[0].id, first);
    assert_eq!(screen.state().projects[0].tabs[1].id, second);
    assert_eq!(screen.state().projects[0].tabs[0].text, "LEFT");
    assert_eq!(screen.state().projects[0].tabs[1].text, "RIGHT");
    assert!(
        screen.state().projects[0].tabs[..2]
            .iter()
            .all(|tab| tab.dirty)
    );
}

#[test]
fn one_markdown_document_has_independent_source_preview_and_scroll() {
    let mut app = fixture();
    let markdown = (0..40)
        .map(|index| format!("# Section {index}\n\nParagraph {index} has a useful detail.\n\n"))
        .collect::<String>();
    app.projects[0].tabs[2].text = markdown.clone();
    app.projects[0].tabs[2].saved_text = markdown.clone();
    app.activate_tab(0, 2);
    let right = app.split_active(SplitOrientation::Vertical).unwrap();
    app.set_document_view(DocumentView::Preview);
    let mut screen = harness(app);
    assert_eq!(
        screen.state().editor_layout.panes[0].view,
        DocumentView::Source
    );
    assert_eq!(
        screen.state().editor_layout.panes[1].view,
        DocumentView::Preview
    );
    let source_before = painted_text_y(&screen, 1, &markdown);
    let preview_before = painted_text_y(&screen, right, "Section 0");
    let position = screen.state().editor_layout.rects[&1].center();
    screen.event(egui::Event::PointerMoved(position));
    screen.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: Vec2::new(0.0, -120.0),
        modifiers: egui::Modifiers::NONE,
        phase: egui::TouchPhase::Move,
    });
    screen.run_steps(2);
    let source_scrolled = painted_text_y(&screen, 1, &markdown);
    assert!(source_scrolled < source_before - 100.0);
    assert!((painted_text_y(&screen, right, "Section 0") - preview_before).abs() < 0.5);
    assert_eq!(
        screen.state().editor_layout.focused,
        right,
        "Hovering/wheeling another pane must not move keyboard focus"
    );

    screen.key_press(Key::J);
    screen.run_steps(2);
    assert!(painted_text_y(&screen, right, "Section 0") < preview_before - 20.0);
    assert!((painted_text_y(&screen, 1, &markdown) - source_scrolled).abs() < 0.5);
    assert_eq!(screen.state().projects[0].tabs[2].text, markdown);
    assert!(!screen.state().projects[0].tabs[2].dirty);
}

#[test]
fn vim_undo_and_redo_are_shared_after_pointer_switching_panes() {
    let mut app = fixture();
    let original = app.tab().unwrap().text.clone();
    let right = app.split_active(SplitOrientation::Vertical).unwrap();
    let mut screen = harness(app);
    click_source(&mut screen, right);
    type_text(&mut screen, "ggi");
    type_text(&mut screen, "added ");
    screen.key_press(Key::Escape);
    screen.run_steps(2);
    assert_eq!(
        screen.state().tab().unwrap().text,
        format!("added {original}")
    );

    click_source(&mut screen, 1);
    type_text(&mut screen, "u");
    assert_eq!(screen.state().tab().unwrap().text, original);
    click_source(&mut screen, right);
    screen.key_press_modifiers(egui::Modifiers::CTRL, Key::R);
    screen.run_steps(2);
    assert_eq!(
        screen.state().tab().unwrap().text,
        format!("added {original}")
    );
    assert_eq!(
        screen.state().projects[0].tabs.len(),
        3,
        "Splitting never duplicates the document buffer"
    );
}

#[test]
fn inactive_pane_navigation_is_clamped_after_shared_document_shrinks() {
    let mut app = fixture();
    let right = app.split_active(SplitOrientation::Vertical).unwrap();
    app.focus_editor_pane(1).unwrap();
    let end = app.tab().unwrap().text.chars().count();
    app.tab_mut().unwrap().set_cursor(end..end);
    app.capture_pane_navigation();
    app.focus_editor_pane(right).unwrap();
    let tab = app.tab_mut().unwrap();
    tab.apply(editor::replace_selection(&tab.text, 0..end, "x"));
    app.capture_pane_navigation();
    let before_render = app.layout_status();
    let inactive = before_render["panes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|pane| pane["id"] == 1)
        .unwrap();
    assert_eq!(inactive["location"], json!({"line":1,"column":2}));
    let mut screen = harness(app);
    screen.state_mut().focus_editor_pane(1).unwrap();
    screen.run_steps(2);
    let tab = screen.state().tab().unwrap();
    assert!(tab.vim.cursor <= 1);
    assert!(tab.selection.start <= 1 && tab.selection.end <= 1);
    for pane in &screen.state().editor_layout.panes {
        let state = pane.states.get(&tab.id).unwrap();
        assert!(state.navigation.cursor <= 1);
    }
}

#[test]
fn closing_a_pane_preserves_dirty_documents_and_other_pane_views() {
    let mut app = fixture();
    let right = app.split_active(SplitOrientation::Vertical).unwrap();
    let tab = app.tab_mut().unwrap();
    tab.apply(editor::replace_selection(&tab.text, 0..0, "unsaved "));
    let id = tab.id;
    let text = tab.text.clone();
    app.close_editor_pane(right).unwrap();
    assert_eq!(app.editor_layout.panes.len(), 1);
    assert_eq!(app.projects[0].tabs.len(), 3);
    assert_eq!(app.tab().unwrap().id, id);
    assert_eq!(app.tab().unwrap().text, text);
    assert!(app.tab().unwrap().dirty);
    assert!(
        app.close_action.is_none(),
        "Closing a pane is not closing its shared documents"
    );
    assert!(app.close_editor_pane(1).is_err());
    assert_eq!(app.tab().unwrap().text, text);
}

#[test]
fn session_round_trip_restores_nested_layout_using_new_document_ids() {
    let mut app = fixture();
    app.activate_tab(0, 2);
    let right = app.split_active(SplitOrientation::Vertical).unwrap();
    app.set_document_view(DocumentView::Preview);
    app.tab_mut().unwrap().set_cursor(12..12);
    app.capture_pane_navigation();
    let bottom = app.split_active(SplitOrientation::Horizontal).unwrap();
    let second = app.projects[0].tabs[1].id;
    app.assign_editor_pane(bottom, second, Some(DocumentView::Source))
        .unwrap();
    app.tab_mut().unwrap().set_cursor(8..8);
    app.capture_pane_navigation();
    app.control_layout(LayoutAction::Resize {
        pane_id: bottom,
        ratio: 0.65,
    })
    .unwrap();
    let stored = serde_json::to_string(&app.editor_session_layout()).unwrap();
    let previous_ids: Vec<_> = app.projects[0].tabs.iter().map(|tab| tab.id).collect();

    let mut restarted = fixture();
    let restored: SessionLayout = serde_json::from_str(&stored).unwrap();
    restarted.restore_editor_layout(Some(restored));
    assert_eq!(restarted.editor_layout.panes.len(), 3);
    assert_eq!(restarted.editor_layout.focused, bottom);
    assert_eq!(restarted.tab().unwrap().title, "second.txt");
    assert_eq!(restarted.tab().unwrap().vim.cursor, 8);
    assert!(
        restarted.projects[0]
            .tabs
            .iter()
            .all(|tab| !previous_ids.contains(&tab.id))
    );
    let preview = restarted
        .editor_layout
        .panes
        .iter()
        .find(|pane| pane.id == right)
        .unwrap();
    assert_eq!(preview.view, DocumentView::Preview);
    assert_eq!(preview.document, Some(restarted.projects[0].tabs[2].id));
    let mut screen = harness(restarted);
    screen.state_mut().focus_editor_pane(right).unwrap();
    screen.run_steps(2);
    assert_eq!(
        screen.state().tab().unwrap().agent_view,
        DocumentView::Preview
    );
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 12);
}

#[test]
fn invalid_preview_layout_requests_preserve_focus_document_and_tree() {
    let mut app = fixture();
    let right = app.split_active(SplitOrientation::Vertical).unwrap();
    let tab = app.tab_mut().unwrap();
    tab.vim.handle_key(&mut tab.text, VimKey::Char('V'));
    app.capture_pane_navigation();
    let before = app.layout_status();
    let document = app.tab().unwrap().id;
    assert!(
        app.control_layout(LayoutAction::Split {
            orientation: SplitOrientation::Horizontal,
            pane_id: Some(1),
            tab_id: Some(document),
            view: Some(DocumentView::Preview),
        })
        .is_err()
    );
    assert_eq!(app.layout_status(), before);
    assert_eq!(app.editor_layout.focused, right);
    assert!(
        app.control_layout(LayoutAction::Assign {
            pane_id: 1,
            tab_id: document,
            view: Some(DocumentView::Preview)
        })
        .is_err()
    );
    assert_eq!(app.layout_status(), before);
    assert_eq!(app.tab().unwrap().vim.mode, VimMode::VisualLine);
}

#[test]
fn a_rejected_ninth_split_does_not_focus_or_commit_another_pane() {
    let mut app = fixture();
    for _ in 1..MAX_PANES {
        app.split_active(SplitOrientation::Vertical).unwrap();
    }
    let focused = app.editor_layout.focused;
    let tab = app.tab_mut().unwrap();
    tab.vim.handle_key(&mut tab.text, VimKey::Char('i'));
    tab.apply(editor::replace_selection(&tab.text, 0..0, "editing "));
    let text = tab.text.clone();
    app.capture_pane_navigation();
    let before = app.layout_status();
    assert!(
        app.control_layout(LayoutAction::Split {
            orientation: SplitOrientation::Horizontal,
            pane_id: Some(1),
            tab_id: None,
            view: None,
        })
        .is_err()
    );
    assert_eq!(app.layout_status(), before);
    assert_eq!(app.editor_layout.focused, focused);
    assert_eq!(app.tab().unwrap().vim.mode, VimMode::Insert);
    assert_eq!(app.tab().unwrap().text, text);
}

#[test]
fn find_from_the_right_pane_targets_its_document_and_keeps_the_left_cursor() {
    let mut app = fixture();
    app.settings.vim = false;
    for (tab, text) in app.projects[0].tabs[..2].iter_mut().zip([
        "left needle\nleft second line\n",
        "right first line\nneedle target\nright third line\n",
    ]) {
        tab.text = text.into();
        tab.saved_text = text.into();
    }
    let right = app.split_active(SplitOrientation::Vertical).unwrap();
    let second = app.projects[0].tabs[1].id;
    app.assign_editor_pane(right, second, Some(DocumentView::Source))
        .unwrap();
    let mut screen = harness(app);
    click_source(&mut screen, right);
    screen.key_press_modifiers(egui::Modifiers::COMMAND, Key::F);
    screen.run_steps(2);
    type_text(&mut screen, "needle");
    assert!(screen.state().find_visible);
    assert_eq!(screen.state().find_text, "needle");
    let editor_id = screen.state().document_widget_id(second);
    assert!(!screen.ctx.memory(|memory| memory.has_focus(editor_id)));
    let find_id = screen.ctx.memory(|memory| memory.focused()).unwrap();
    let match_start = "right first line\n".chars().count();
    let expected_selection = match_start..match_start + "needle".chars().count();
    // Enter keeps the find field active so another Enter searches again,
    // without inserting a newline into the document.
    for _ in 0..2 {
        screen.key_press(Key::Enter);
        screen.run_steps(2);
        let tab = screen.state().tab().unwrap();
        assert_eq!(tab.id, second);
        assert_eq!(tab.selection, expected_selection);
        // Native selection may have its primary caret at either endpoint;
        // the exact selected match is what identifies successful navigation.
        assert!([expected_selection.start, expected_selection.end].contains(&tab.vim.cursor));
        assert_eq!(screen.state().find_match, Some(match_start));
        let native = egui::TextEdit::load_state(&screen.ctx, editor_id)
            .unwrap()
            .cursor
            .char_range()
            .unwrap()
            .as_sorted_char_range();
        assert_eq!(native.start.0..native.end.0, expected_selection);
        assert_eq!(screen.ctx.memory(|memory| memory.focused()), Some(find_id));
    }
    assert_eq!(screen.state().editor_layout.focused, right);
    let first_pane = screen
        .state()
        .editor_layout
        .panes
        .iter()
        .find(|pane| pane.id == 1)
        .unwrap();
    let first_tab = screen.state().projects[0].tabs[0].id;
    assert_eq!(first_pane.states[&first_tab].navigation.cursor, 0);
    assert_eq!(first_pane.states[&first_tab].selection, 0..0);
    assert!(screen.state().projects[0].tabs.iter().all(|tab| !tab.dirty));
}

#[test]
fn ex_line_command_from_the_right_pane_does_not_jump_the_left_document() {
    let mut app = fixture();
    let right = app.split_active(SplitOrientation::Vertical).unwrap();
    let second = app.projects[0].tabs[1].id;
    app.assign_editor_pane(right, second, Some(DocumentView::Source))
        .unwrap();
    let mut screen = harness(app);
    click_source(&mut screen, right);
    type_text(&mut screen, ":");
    assert!(screen.state().vim_command.is_some());
    type_text(&mut screen, "2");
    screen.key_press(Key::Enter);
    screen.run_steps(2);
    let tab = screen.state().tab().unwrap();
    assert_eq!(tab.id, second);
    assert_eq!(tab.vim.cursor, "another document\n".chars().count());
    assert!(screen.state().vim_command.is_none());
    assert_eq!(screen.state().editor_layout.focused, right);
    let first_pane = screen
        .state()
        .editor_layout
        .panes
        .iter()
        .find(|pane| pane.id == 1)
        .unwrap();
    let first_tab = screen.state().projects[0].tabs[0].id;
    assert_eq!(first_pane.states[&first_tab].navigation.cursor, 0);
    assert!(screen.state().projects[0].tabs.iter().all(|tab| !tab.dirty));
}

#[test]
fn focused_layout_status_reads_live_cli_edits_and_selection_before_capture() {
    let mut app = fixture();
    let right = app.split_active(SplitOrientation::Vertical).unwrap();
    let tab_id = app.tab().unwrap().id;
    let revision = app.tab().unwrap().revision;
    let response = app.control_request(crate::cli::Request::Edit {
        tab_id,
        expected_revision: revision,
        edits: Vec::new(),
        set_text: Some("new\nβ🦀".into()),
    });
    assert!(response.ok, "{}", response.result);
    let status = app.layout_status();
    let focused = status["panes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|pane| pane["id"] == right)
        .unwrap();
    assert_eq!(focused["location"], json!({"line":2,"column":3}));
    assert_eq!(focused["focused"], true);
    // Capturing for a later frame must not alter the reported live state.
    app.capture_pane_navigation();
    assert_eq!(app.layout_status(), status);

    let revision = app.tab().unwrap().revision;
    let response = app.control_request(crate::cli::Request::Selection {
        tab_id,
        expected_revision: Some(revision),
        range: Some(crate::cli::TextRange {
            start: crate::cli::Location { line: 2, column: 1 },
            end: crate::cli::Location { line: 2, column: 2 },
        }),
    });
    assert!(response.ok, "{}", response.result);
    let status = app.layout_status();
    let focused = status["panes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|pane| pane["id"] == right)
        .unwrap();
    assert_eq!(focused["location"], json!({"line":2,"column":1}));
    assert_eq!(app.tab().unwrap().vim.mode, VimMode::Visual);
}

#[test]
fn restored_pane_cursors_survive_blank_frames_until_document_load_finishes() {
    let mut original = fixture();
    let text = original.tab().unwrap().text.clone();
    original.tab_mut().unwrap().set_cursor(10..10);
    original.capture_pane_navigation();
    let right = original.split_active(SplitOrientation::Vertical).unwrap();
    original.tab_mut().unwrap().set_cursor(20..20);
    original.capture_pane_navigation();
    let saved = original.editor_session_layout();
    let saved = serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();

    let mut restarted = fixture();
    let tab_id = restarted.tab().unwrap().id;
    let tab = restarted.tab_mut().unwrap();
    tab.text.clear();
    tab.saved_text.clear();
    tab.loading = true;
    tab.vim.cursor = 20;
    tab.selection = 20..20;
    tab.pending_selection = Some(20..20);
    restarted.restore_editor_layout(saved);
    let mut screen = harness(restarted);
    screen.run_steps(5);
    assert!(screen.state().tab().unwrap().loading);
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 20);
    for (pane, cursor) in [(1, 10), (right, 20)] {
        let pane = screen
            .state()
            .editor_layout
            .panes
            .iter()
            .find(|p| p.id == pane)
            .unwrap();
        assert_eq!(pane.states[&tab_id].navigation.cursor, cursor);
    }

    screen
        .state_mut()
        .sender
        .send(WorkerResult::Loaded {
            id: tab_id,
            result: Ok(workspace::LoadedDocument {
                fingerprint: workspace::FileFingerprint {
                    len: text.len() as u64,
                    modified: None,
                    hash: 7,
                },
                text: text.clone(),
                line_ending: workspace::LineEnding::Lf,
                bom: false,
                mixed_line_endings: false,
            }),
            recovery: None,
        })
        .unwrap();
    screen.run_steps(3);
    assert!(!screen.state().tab().unwrap().loading);
    assert_eq!(screen.state().tab().unwrap().text, text);
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 20);
    screen.state_mut().focus_editor_pane(1).unwrap();
    screen.run_steps(2);
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 10);
    screen.state_mut().focus_editor_pane(right).unwrap();
    screen.run_steps(2);
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 20);
    assert!(!screen.state().tab().unwrap().dirty);
}

#[test]
fn dragging_outer_divider_resizes_that_split_when_its_right_child_is_split() {
    let mut app = fixture();
    let right = app.split_active(SplitOrientation::Vertical).unwrap();
    app.split_active(SplitOrientation::Horizontal).unwrap();
    let mut screen = harness(app);
    let left_rect = screen.state().editor_layout.rects[&1];
    let right_rect = screen.state().editor_layout.rects[&right];
    let start = egui::pos2(left_rect.right() + 2.5, left_rect.center().y);
    let end = start + Vec2::new((right_rect.right() - left_rect.left()) * 0.14, 0.0);
    screen.event(egui::Event::PointerMoved(start));
    screen.event(egui::Event::PointerButton {
        pos: start,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    screen.run_steps(1);
    screen.event(egui::Event::PointerMoved(end));
    screen.run_steps(2);
    screen.event(egui::Event::PointerButton {
        pos: end,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    screen.run_steps(2);
    let PaneNode::Split { ratio, second, .. } = &screen.state().editor_layout.root else {
        panic!("Expected outer split");
    };
    assert!((*ratio - 0.64).abs() < 0.02, "Outer ratio was {ratio}");
    let PaneNode::Split { ratio, .. } = second.as_ref() else {
        panic!("Expected inner split");
    };
    assert_eq!(*ratio, 0.5);
}

#[test]
fn split_tab_strip_shows_only_its_own_documents() {
    let mut app = fixture();
    let original = app.projects[0]
        .tabs
        .iter()
        .map(|tab| tab.id)
        .collect::<Vec<_>>();
    let right = app.split_active(SplitOrientation::Vertical).unwrap();
    assert_eq!(pane_documents(&app, 1), original);
    assert_eq!(pane_documents(&app, right), vec![original[0]]);
    let screen = harness(app);
    for title in ["first.txt", "second.txt", "plan.md"] {
        assert!(painted_tab_center(&screen, 1, title).is_some());
    }
    assert!(painted_tab_center(&screen, right, "first.txt").is_some());
    assert!(painted_tab_center(&screen, right, "second.txt").is_none());
    assert!(painted_tab_center(&screen, right, "plan.md").is_none());
}

#[test]
fn new_documents_and_reopened_buffers_join_only_the_focused_group() {
    let mut app = fixture();
    let original = pane_documents(&app, 1);
    let right = app.split_active(SplitOrientation::Vertical).unwrap();
    app.new_file();
    let native_new = app.tab().unwrap().id;
    let response = app.control_request(crate::cli::Request::New {
        title: Some("agent draft.rs".into()),
        text: "// unsaved agent draft\n".into(),
        path: None,
    });
    assert!(response.ok, "{}", response.result);
    let agent_new = app.tab().unwrap().id;
    // Opening an already loaded buffer adds a reference rather than a second buffer.
    app.activate_tab(0, 1);
    assert_eq!(pane_documents(&app, 1), original);
    assert_eq!(
        pane_documents(&app, right),
        vec![original[0], native_new, agent_new, original[1]]
    );
    assert_eq!(app.projects[0].tabs.len(), 5);
    assert_eq!(app.editor_layout.focused, right);
}

#[test]
fn ctrl_tab_and_shift_ctrl_tab_cycle_only_the_focused_group() {
    let mut app = fixture();
    let first = app.projects[0].tabs[0].id;
    let second = app.projects[0].tabs[1].id;
    let right = app.split_active(SplitOrientation::Vertical).unwrap();
    app.activate_tab(0, 1);
    let mut screen = harness(app);
    screen.key_press_modifiers(egui::Modifiers::CTRL, Key::Tab);
    screen.run_steps(2);
    assert_eq!(screen.state().tab().unwrap().id, first);
    assert_eq!(screen.state().editor_layout.focused, right);
    screen.key_press_modifiers(egui::Modifiers::CTRL, Key::Tab);
    screen.run_steps(2);
    assert_eq!(screen.state().tab().unwrap().id, second);
    screen.key_press_modifiers(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, Key::Tab);
    screen.run_steps(2);
    assert_eq!(screen.state().tab().unwrap().id, first);
    click_tab(&mut screen, 1, "plan.md", egui::PointerButton::Primary);
    assert_eq!(screen.state().tab().unwrap().title, "plan.md");
    screen.key_press_modifiers(egui::Modifiers::CTRL, Key::Tab);
    screen.run_steps(2);
    assert_eq!(screen.state().tab().unwrap().id, first);
    assert_eq!(screen.state().editor_layout.focused, 1);
    let other = screen
        .state()
        .editor_layout
        .panes
        .iter()
        .find(|pane| pane.id == right)
        .unwrap();
    assert_eq!(other.document, Some(first));
}

#[test]
fn middle_clicking_a_shared_dirty_tab_closes_only_its_pane_and_preserves_undo() {
    let mut app = fixture();
    let original = app.tab().unwrap().text.clone();
    let shared_id = app.tab().unwrap().id;
    let right = app.split_active(SplitOrientation::Vertical).unwrap();
    let mut screen = harness(app);
    click_source(&mut screen, right);
    type_text(&mut screen, "ggi");
    type_text(&mut screen, "draft ");
    screen.key_press(Key::Escape);
    screen.run_steps(2);
    assert_eq!(
        screen.state().tab().unwrap().text,
        format!("draft {original}")
    );
    assert!(screen.state().tab().unwrap().dirty);
    click_tab(&mut screen, right, "first.txt", egui::PointerButton::Middle);
    assert_eq!(screen.state().editor_layout.panes.len(), 1);
    assert_eq!(screen.state().editor_layout.focused, 1);
    assert_eq!(screen.state().tab().unwrap().id, shared_id);
    assert_eq!(screen.state().projects[0].tabs.len(), 3);
    assert!(screen.state().tab().unwrap().dirty);
    assert!(screen.state().close_action.is_none());
    assert!(screen.state().tab().unwrap().vim.can_undo());
    click_source(&mut screen, 1);
    type_text(&mut screen, "u");
    assert_eq!(screen.state().tab().unwrap().text, original);
    assert!(!screen.state().tab().unwrap().dirty);
}

#[test]
fn ctrl_w_closes_local_tabs_then_collapses_the_empty_split() {
    let mut app = fixture();
    let original = pane_documents(&app, 1);
    let right = app.split_active(SplitOrientation::Vertical).unwrap();
    app.activate_tab(0, 1);
    let mut screen = harness(app);
    screen.key_press_modifiers(egui::Modifiers::COMMAND, Key::W);
    screen.run_steps(2);
    assert_eq!(screen.state().editor_layout.panes.len(), 2);
    assert_eq!(pane_documents(screen.state(), right), vec![original[0]]);
    assert_eq!(pane_documents(screen.state(), 1), original);
    assert_eq!(screen.state().projects[0].tabs.len(), 3);
    screen.key_press_modifiers(egui::Modifiers::COMMAND, Key::W);
    screen.run_steps(2);
    assert_eq!(screen.state().editor_layout.panes.len(), 1);
    assert_eq!(screen.state().editor_layout.focused, 1);
    assert_eq!(pane_documents(screen.state(), 1), original);
    assert_eq!(screen.state().projects[0].tabs.len(), 3);
    assert!(matches!(
        screen.state().editor_layout.root,
        PaneNode::Pane { id: 1 }
    ));
}

#[test]
fn closing_a_shared_tab_finishes_counted_insert_as_one_shared_undo_transaction() {
    let mut app = fixture();
    let original = app.tab().unwrap().text.clone();
    let shared_id = app.tab().unwrap().id;
    let right = app.split_active(SplitOrientation::Vertical).unwrap();
    let mut screen = harness(app);
    click_source(&mut screen, right);
    type_text(&mut screen, "gg3i");
    assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::Insert);
    type_text(&mut screen, "draft ");
    assert_eq!(
        screen.state().tab().unwrap().text,
        format!("draft {original}")
    );
    // Closing the view must finish the counted insertion before restoring
    // its sibling's independent cursor; no Escape is sent here.
    screen.key_press_modifiers(egui::Modifiers::COMMAND, Key::W);
    screen.run_steps(2);
    assert_eq!(screen.state().editor_layout.panes.len(), 1);
    assert_eq!(screen.state().editor_layout.focused, 1);
    assert_eq!(screen.state().tab().unwrap().id, shared_id);
    assert_eq!(
        screen.state().tab().unwrap().text,
        format!("draft draft draft {original}")
    );
    assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::Normal);
    assert!(screen.state().close_action.is_none());
    click_source(&mut screen, 1);
    type_text(&mut screen, "u");
    assert_eq!(screen.state().tab().unwrap().text, original);
    assert!(!screen.state().tab().unwrap().dirty);
    screen.key_press_modifiers(egui::Modifiers::CTRL, Key::R);
    screen.run_steps(2);
    assert_eq!(
        screen.state().tab().unwrap().text,
        format!("draft draft draft {original}")
    );
    assert!(screen.state().tab().unwrap().dirty);
}

#[test]
fn close_other_saved_tabs_is_local_and_keeps_local_unsaved_documents() {
    let mut app = fixture();
    let original = pane_documents(&app, 1);
    let right = app.split_active(SplitOrientation::Vertical).unwrap();
    app.activate_tab(0, 1);
    app.new_file();
    let dirty = app.tab().unwrap().id;
    app.tab_mut()
        .unwrap()
        .apply(editor::replace_selection("", 0..0, "dirty draft"));
    app.apply_tab_context_action(original[0], context_actions::TabAction::CloseOthers);
    assert_eq!(pane_documents(&app, 1), original);
    assert_eq!(pane_documents(&app, right), vec![original[0], dirty]);
    assert_eq!(app.projects[0].tabs.len(), 4);
    assert_eq!(app.tab_by_id_mut(dirty).unwrap().text, "dirty draft");
    assert!(app.tab_by_id_mut(dirty).unwrap().dirty);
    assert!(app.close_action.is_none());
}

#[test]
fn close_saved_tabs_collapses_only_the_local_empty_split() {
    let mut app = fixture();
    let original = pane_documents(&app, 1);
    let right = app.split_active(SplitOrientation::Vertical).unwrap();
    app.activate_tab(0, 1);
    app.apply_tab_context_action(original[1], context_actions::TabAction::CloseSaved);
    assert_eq!(app.editor_layout.panes.len(), 1);
    assert!(!app.editor_layout.panes.iter().any(|pane| pane.id == right));
    assert_eq!(pane_documents(&app, 1), original);
    assert_eq!(app.projects[0].tabs.len(), 3);
    assert_eq!(app.editor_layout.focused, 1);
}

#[test]
fn closing_the_last_reference_to_a_dirty_document_waits_for_confirmation() {
    let mut app = fixture();
    let original = pane_documents(&app, 1);
    let right = app.split_active(SplitOrientation::Vertical).unwrap();
    app.new_file();
    let draft = app.tab().unwrap().id;
    app.tab_mut()
        .unwrap()
        .apply(editor::replace_selection("", 0..0, "unique draft"));
    app.request_close(CloseAction::PaneTab {
        pane_id: right,
        id: original[0],
    });
    assert_eq!(pane_documents(&app, right), vec![draft]);
    app.request_close(CloseAction::PaneTab {
        pane_id: right,
        id: draft,
    });
    assert!(
        matches!(app.close_action, Some(CloseAction::PaneTab { pane_id, id }) if pane_id == right && id == draft)
    );
    assert_eq!(app.editor_layout.panes.len(), 2);
    assert_eq!(pane_documents(&app, right), vec![draft]);
    assert_eq!(pane_documents(&app, 1), original);
    assert_eq!(app.tab().unwrap().text, "unique draft");
    assert!(app.tab().unwrap().dirty);
    // Cancel leaves the group and draft intact. Only an explicit confirmed
    // close removes the final buffer reference and collapses its empty pane.
    app.close_action = None;
    app.perform_close(CloseAction::PaneTab {
        pane_id: right,
        id: draft,
    });
    assert_eq!(app.editor_layout.panes.len(), 1);
    assert_eq!(pane_documents(&app, 1), original);
    assert!(app.tab_by_id_mut(draft).is_none());
}

#[test]
fn closing_the_last_tab_in_a_nested_pane_keeps_its_siblings() {
    let mut app = fixture();
    let first = app.tab().unwrap().id;
    let right = app.split_active(SplitOrientation::Vertical).unwrap();
    let bottom = app.split_active(SplitOrientation::Horizontal).unwrap();
    app.request_close(CloseAction::PaneTab {
        pane_id: bottom,
        id: first,
    });
    assert_eq!(app.editor_layout.panes.len(), 2);
    assert!(!app.editor_layout.panes.iter().any(|pane| pane.id == bottom));
    assert_eq!(pane_documents(&app, right), vec![first]);
    assert_eq!(app.projects[0].tabs.len(), 3);
    assert_eq!(
        app.editor_layout.focused, right,
        "Closing a nested split returns focus to its adjacent sibling"
    );
    let PaneNode::Split {
        first: left_node,
        second: right_node,
        ..
    } = &app.editor_layout.root
    else {
        panic!("Expected surviving vertical split");
    };
    assert!(matches!(left_node.as_ref(), PaneNode::Pane { id: 1 }));
    assert!(matches!(right_node.as_ref(), PaneNode::Pane { id } if *id == right));
    assert!(
        app.editor_layout
            .panes
            .iter()
            .any(|pane| pane.id == app.editor_layout.focused)
    );
}

#[test]
fn closing_a_whole_pane_keeps_its_unique_dirty_tab_reachable_in_the_survivor() {
    let mut app = fixture();
    let original = pane_documents(&app, 1);
    let right = app.split_active(SplitOrientation::Vertical).unwrap();
    app.new_file();
    let draft = app.tab().unwrap().id;
    app.tab_mut()
        .unwrap()
        .apply(editor::replace_selection("", 0..0, "unique draft"));
    app.close_editor_pane(right).unwrap();
    assert_eq!(app.editor_layout.panes.len(), 1);
    assert_eq!(pane_documents(&app, 1), [original, vec![draft]].concat());
    assert_eq!(app.projects[0].tabs.len(), 4);
    let (project, index) = app.tab_location(draft).unwrap();
    app.activate_tab(project, index);
    assert_eq!(app.tab().unwrap().id, draft);
    assert_eq!(app.tab().unwrap().text, "unique draft");
    assert!(app.tab().unwrap().dirty);
    assert!(app.close_action.is_none());
}

#[test]
fn closing_all_documents_keeps_one_empty_pane_without_reopening_closed_tabs() {
    let mut app = fixture();
    let original = pane_documents(&app, 1);
    for id in original {
        app.request_close(CloseAction::PaneTab { pane_id: 1, id });
        assert!(app.close_action.is_none());
    }
    assert_eq!(app.editor_layout.panes.len(), 1);
    assert!(pane_documents(&app, 1).is_empty());
    assert!(app.editor_layout.panes[0].document.is_none());
    assert!(app.projects[0].tabs.is_empty());
    assert!(app.tab().is_none());
    let mut screen = harness(app);
    screen.run_steps(3);
    assert!(screen.state().tab().is_none());
    assert!(pane_documents(screen.state(), 1).is_empty());
    screen.state_mut().new_file();
    screen.run_steps(2);
    assert_eq!(
        pane_documents(screen.state(), 1),
        vec![screen.state().tab().unwrap().id]
    );
}

#[test]
fn session_round_trip_restores_group_membership_and_order_without_duplicate_buffers() {
    let mut app = fixture();
    let original = pane_documents(&app, 1);
    app.activate_tab(0, 2);
    let right = app.split_active(SplitOrientation::Vertical).unwrap();
    app.set_document_view(DocumentView::Preview);
    app.activate_tab(0, 0);
    let stored = serde_json::to_string(&app.editor_session_layout()).unwrap();
    let persisted_documents = app.session().projects.remove(0).tabs;
    let mut restarted = fixture();
    // The document portion of session restore provides each buffer's default
    // view; layout restore supplies group order and active per-pane navigation.
    for (tab, saved) in restarted.projects[0]
        .tabs
        .iter_mut()
        .zip(persisted_documents)
    {
        tab.agent_view = saved.view;
    }
    restarted.restore_editor_layout(serde_json::from_str(&stored).unwrap());
    let fresh = restarted.projects[0]
        .tabs
        .iter()
        .map(|tab| tab.id)
        .collect::<Vec<_>>();
    assert!(fresh.iter().all(|id| !original.contains(id)));
    assert_eq!(pane_documents(&restarted, 1), fresh);
    assert_eq!(pane_documents(&restarted, right), vec![fresh[2], fresh[0]]);
    assert_eq!(restarted.editor_layout.focused, right);
    assert_eq!(restarted.tab().unwrap().id, fresh[0]);
    assert_eq!(restarted.projects[0].tabs.len(), 3);
    restarted.activate_tab(0, 2);
    assert_eq!(restarted.tab().unwrap().agent_view, DocumentView::Preview);
}

#[test]
fn session_restores_inactive_markdown_tabs_with_each_groups_view_and_cursor() {
    let mut app = fixture();
    app.activate_tab(0, 2);
    app.tab_mut().unwrap().set_cursor(6..6);
    app.capture_pane_navigation();
    let right = app.split_active(SplitOrientation::Vertical).unwrap();
    app.set_document_view(DocumentView::Preview);
    app.tab_mut().unwrap().set_cursor(12..12);
    app.capture_pane_navigation();
    app.activate_tab(0, 1);
    app.focus_editor_pane(1).unwrap();
    app.activate_tab(0, 0);
    let session = app.session();
    let stored = serde_json::to_string(&session.projects[0].layout).unwrap();
    let mut restarted = fixture();
    for (tab, saved) in restarted.projects[0]
        .tabs
        .iter_mut()
        .zip(&session.projects[0].tabs)
    {
        tab.agent_view = saved.view;
        tab.set_cursor(saved.cursor..saved.cursor);
    }
    restarted.restore_editor_layout(serde_json::from_str(&stored).unwrap());
    restarted.activate_tab(0, 2);
    assert_eq!(restarted.tab().unwrap().agent_view, DocumentView::Source);
    assert_eq!(restarted.tab().unwrap().vim.cursor, 6);
    restarted.focus_editor_pane(right).unwrap();
    restarted.activate_tab(0, 2);
    assert_eq!(restarted.tab().unwrap().agent_view, DocumentView::Preview);
    assert_eq!(restarted.tab().unwrap().vim.cursor, 12);
    assert_eq!(restarted.projects[0].tabs.len(), 3);
}

#[test]
fn cli_local_close_retains_shared_dirty_text_and_refuses_its_last_view_without_effects() {
    let mut app = fixture();
    let first = app.tab().unwrap().id;
    let original = app.tab().unwrap().text.clone();
    let right = app.split_active(SplitOrientation::Vertical).unwrap();
    let tab = app.tab_mut().unwrap();
    tab.vim.begin_external_edit(&tab.text);
    tab.text.insert_str(0, "draft ");
    tab.changed();
    let revision = tab.revision;
    let closed = app.control_request(crate::cli::Request::Layout {
        change: crate::cli::LayoutAction::CloseTab {
            pane_id: right,
            tab_id: first,
        },
    });
    assert!(closed.ok, "{:?}", closed.error);
    assert_eq!(app.editor_layout.panes.len(), 1);
    assert_eq!(app.tab().unwrap().text, format!("draft {original}"));
    assert_eq!(app.tab().unwrap().revision, revision);
    assert!(app.tab().unwrap().dirty);
    assert!(app.tab().unwrap().vim.can_undo());
    let layout = app.layout_status();
    let rejected = app.control_request(crate::cli::Request::Layout {
        change: crate::cli::LayoutAction::CloseTab {
            pane_id: 1,
            tab_id: first,
        },
    });
    assert!(!rejected.ok);
    assert_eq!(app.layout_status(), layout);
    assert_eq!(app.tab().unwrap().revision, revision);
    assert_eq!(app.tab().unwrap().text, format!("draft {original}"));
    assert!(app.close_action.is_none());
}

#[test]
fn legacy_shared_strip_session_migrates_all_documents_without_losing_selected_tabs() {
    let mut app = fixture();
    app.activate_tab(0, 2);
    let right = app.split_active(SplitOrientation::Vertical).unwrap();
    app.activate_tab(0, 1);
    let mut legacy = serde_json::to_value(app.editor_session_layout()).unwrap();
    for pane in legacy["panes"].as_array_mut().unwrap() {
        pane.as_object_mut().unwrap().remove("documents");
        pane.as_object_mut().unwrap().remove("states");
    }
    let mut restarted = fixture();
    restarted.restore_editor_layout(Some(serde_json::from_value(legacy).unwrap()));
    let ids: Vec<_> = restarted.projects[0]
        .tabs
        .iter()
        .map(|tab| tab.id)
        .collect();
    assert_eq!(pane_documents(&restarted, 1), vec![ids[2]]);
    assert_eq!(pane_documents(&restarted, right), ids);
    assert_eq!(restarted.editor_layout.focused, right);
    assert_eq!(restarted.tab().unwrap().id, ids[1]);
    assert_eq!(restarted.projects[0].tabs.len(), 3);
}

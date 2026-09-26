//! Cross-feature regressions: reading plans must not disrupt editing state.

use super::*;
use crate::cli::DocumentView;
use egui_kittest::{Harness, kittest::Queryable};

fn fixture() -> MorrowApp {
    let mut app = MorrowApp::empty(egui::Context::default());
    app.settings.autosave = false;
    let root = std::env::temp_dir().join("morrow-interaction-in-memory");
    let mut project = Project::new(root.clone());
    project.last_git_refresh = Instant::now();
    for (name, text) in [
        ("plan.md", "# Plan\n\nFirst line\nSecond line\nThird line\n"),
        ("source.txt", "first\nsecond\nthird\n"),
    ] {
        let mut tab = EditorTab::new(Some(root.join(name)), name.into());
        tab.text = text.into();
        tab.saved_text = text.into();
        project.tabs.push(tab);
    }
    app.projects.push(project);
    app
}

fn harness(app: MorrowApp) -> Harness<'static, MorrowApp> {
    let mut screen = Harness::builder()
        .with_size([1200.0, 760.0])
        .with_step_dt(1.0 / 60.0)
        .with_os(egui::os::OperatingSystem::Windows)
        .build_ui_state(
            |ui, app: &mut MorrowApp| {
                app.ctx = ui.ctx().clone();
                let reading_key = app.ctx.input(|input| input.key_pressed(Key::J));
                app.shortcuts();
                app.handle_vim();
                app.editor_ui(ui);
                app.overlays();
                if reading_key {
                    let consumed = !app.ctx.input(|input| input.key_pressed(Key::J));
                    app.ctx.data_mut(|data| {
                        data.insert_temp(
                            egui::Id::new("interaction_reading_key_consumed"),
                            consumed,
                        )
                    });
                }
            },
            app,
        );
    screen.run_steps(2);
    screen
}

#[test]
fn preview_round_trip_preserves_visual_lines_and_cursor() {
    let mut screen = harness(fixture());
    let tab = screen.state_mut().tab_mut().unwrap();
    tab.vim.cursor = editor::go_to_line(&tab.text, 3);
    tab.vim.handle_key(&mut tab.text, VimKey::Char('V'));
    tab.vim.handle_key(&mut tab.text, VimKey::Char('j'));
    let cursor = tab.vim.cursor;
    let selection = tab.vim.selection(&tab.text).unwrap();
    tab.pending_selection = Some(selection.clone());
    screen.run_steps(2);

    screen.state_mut().set_document_view(DocumentView::Preview);
    screen.run_steps(3);
    let tab = screen.state().tab().unwrap();
    assert_eq!(tab.vim.mode, VimMode::VisualLine);
    assert_eq!(tab.vim.cursor, cursor);
    assert_eq!(tab.vim.selection(&tab.text), Some(selection.clone()));
    assert!(
        !screen
            .ctx
            .memory(|memory| memory.has_focus(egui::Id::new(("document", tab.id))))
    );

    screen.state_mut().set_document_view(DocumentView::Source);
    screen.run_steps(2);
    let tab = screen.state().tab().unwrap();
    assert_eq!(tab.vim.mode, VimMode::VisualLine);
    assert_eq!(tab.vim.cursor, cursor);
    assert_eq!(tab.selection, selection);
    assert!(
        screen
            .ctx
            .memory(|memory| memory.has_focus(egui::Id::new(("document", tab.id))))
    );
}

#[test]
fn switching_to_preview_commits_insert_and_keeps_it_undoable() {
    let mut screen = harness(fixture());
    let tab = screen.state_mut().tab_mut().unwrap();
    let original = tab.text.clone();
    tab.vim.handle_key(&mut tab.text, VimKey::Char('i'));
    let edit = editor::replace_selection(&tab.text, 0..0, "Inserted ");
    tab.apply(edit);
    let edited = tab.text.clone();

    screen.state_mut().set_document_view(DocumentView::Preview);
    screen.run_steps(2);
    let tab = screen.state().tab().unwrap();
    assert_eq!(tab.text, edited);
    assert_eq!(tab.vim.mode, VimMode::Normal);
    assert!(tab.dirty);

    screen.state_mut().set_document_view(DocumentView::Source);
    screen.run_steps(2);
    let tab = screen.state_mut().tab_mut().unwrap();
    let action = tab.vim.handle_key(&mut tab.text, VimKey::Char('u'));
    assert!(action.text_changed);
    assert_eq!(tab.text, original);
}

#[test]
fn activating_a_preview_tab_releases_the_previous_editor_focus() {
    let mut app = fixture();
    app.projects[0].tabs[0].agent_view = DocumentView::Preview;
    app.projects[0].active_tab = 1;
    let mut screen = harness(app);
    let old_id = screen.state().tab().unwrap().id;
    assert!(
        screen
            .ctx
            .memory(|memory| memory.has_focus(egui::Id::new(("document", old_id))))
    );

    screen.state_mut().activate_tab(0, 0);
    assert!(!screen.state().tab().unwrap().focus);
    assert!(
        !screen
            .ctx
            .memory(|memory| memory.has_focus(egui::Id::new(("document", old_id))))
    );
    screen.run_steps(2);
    assert_eq!(
        screen.state().tab().unwrap().agent_view,
        DocumentView::Preview
    );
    assert!(!screen.state().editor_focused);
}

#[test]
fn source_activation_preserves_another_tabs_visual_selection() {
    let mut screen = harness(fixture());
    let tab = screen.state_mut().tab_mut().unwrap();
    tab.vim.handle_key(&mut tab.text, VimKey::Char('V'));
    tab.vim.handle_key(&mut tab.text, VimKey::Char('j'));
    let selection = tab.vim.selection(&tab.text).unwrap();
    tab.pending_selection = Some(selection.clone());
    screen.run_steps(1);
    screen.state_mut().activate_tab(0, 1);
    screen.run_steps(2);
    screen.state_mut().activate_tab(0, 0);
    screen.run_steps(2);
    let tab = screen.state().tab().unwrap();
    assert_eq!(tab.vim.mode, VimMode::VisualLine);
    assert_eq!(tab.selection, selection);
}

#[test]
fn opening_file_palette_from_preview_keeps_its_input_focus() {
    let mut screen = harness(fixture());
    screen.state_mut().set_document_view(DocumentView::Preview);
    screen.run_steps(2);
    screen.state_mut().open_palette(Palette::Files);
    screen.run_steps(2);
    let focused = screen.ctx.memory(|memory| memory.focused());
    assert!(focused.is_some());
    let document_id = egui::Id::new(("document", screen.state().tab().unwrap().id));
    assert_ne!(focused, Some(document_id));
    screen.run_steps(2);
    assert_eq!(screen.ctx.memory(|memory| memory.focused()), focused);
}

#[test]
fn clicking_preview_toolbar_releases_focus_for_reading_motions() {
    let mut app = fixture();
    let tab = app.tab_mut().unwrap();
    tab.text =
        "# Reading plan\n\nSeveral paragraphs to move through with Vim motions.\n\n".repeat(40);
    tab.saved_text = tab.text.clone();
    let original = tab.text.clone();
    let mut screen = harness(app);
    screen.get_by_label("Toggle Markdown preview").click();
    screen.run_steps(2);
    assert_eq!(
        screen.state().tab().unwrap().agent_view,
        DocumentView::Preview
    );
    assert_eq!(screen.ctx.memory(|memory| memory.focused()), None);
    screen.key_press(Key::J);
    screen.run_steps(1);
    assert_eq!(screen.state().tab().unwrap().text, original);
    assert!(!screen.state().tab().unwrap().dirty);
    assert_eq!(
        screen
            .ctx
            .data(|data| data.get_temp::<bool>(egui::Id::new("interaction_reading_key_consumed"))),
        Some(true),
        "Preview should consume its reading motion instead of leaving it blocked by toolbar focus"
    );
}

#[test]
fn explicit_navigation_disarms_delete_and_keeps_exact_end_boundaries() {
    let mut app = fixture();
    let tab = app.tab_mut().unwrap();
    let original = tab.text.clone();
    tab.vim.handle_key(&mut tab.text, VimKey::Char('d'));
    assert_eq!(tab.vim.pending_command(), "d");
    let target = editor::go_to_line(&tab.text, 3);
    tab.set_cursor(target..target);
    assert!(tab.vim.pending_command().is_empty());
    tab.vim.handle_key(&mut tab.text, VimKey::Char('j'));
    assert_eq!(
        tab.text, original,
        "A jump must not carry an armed delete into its destination"
    );

    let end_of_line = tab.lines.line_end(&tab.text, tab.revision, 3).unwrap();
    tab.set_cursor(end_of_line..end_of_line);
    assert_eq!(tab.vim.cursor, end_of_line);
    assert_eq!(tab.selection, end_of_line..end_of_line);
    let eof = tab.text.chars().count();
    tab.set_cursor(eof..eof);
    assert_eq!(tab.vim.cursor, eof);
}

#[test]
fn loaded_disk_replacement_invalidates_review_marks_and_old_undo_history() {
    let mut app = fixture();
    let tab = app.tab_mut().unwrap();
    tab.vim.handle_key(&mut tab.text, VimKey::Char('x'));
    tab.saved_text = tab.text.clone();
    assert!(tab.vim.can_undo());
    tab.annotations.push(crate::cli::Annotation {
        id: "important".into(),
        start: crate::cli::Location { line: 3, column: 1 },
        end: None,
        note: "Original plan detail".into(),
        severity: crate::cli::AnnotationSeverity::Info,
    });
    tab.annotations_revision = Some(tab.revision);
    let old_revision = tab.revision;
    let id = tab.id;
    tab.loading = true;
    app.sender
        .send(WorkerResult::Loaded {
            id,
            result: Ok(workspace::LoadedDocument {
                text: "# Updated plan\n\nNew introduction\nImportant detail moved here\n".into(),
                line_ending: workspace::LineEnding::Lf,
                bom: false,
                mixed_line_endings: false,
                fingerprint: workspace::FileFingerprint {
                    len: 61,
                    modified: None,
                    hash: 42,
                },
            }),
            recovery: None,
        })
        .unwrap();
    app.poll();
    let tab = app.tab().unwrap();
    assert!(!tab.loading);
    assert!(!tab.dirty);
    assert!(tab.revision > old_revision);
    assert_ne!(tab.annotations_revision, Some(tab.revision));
    assert_eq!(tab.vim.mode, VimMode::Normal);
    assert!(
        !tab.vim.can_undo(),
        "Old buffer snapshots must not overwrite the newly loaded disk version"
    );
    assert_eq!(tab.lines.line_col(&tab.text, tab.revision, 0), (1, 1));
}

fn document_shape(screen: &Harness<'static, MorrowApp>) -> egui::epaint::TextShape {
    let text = &screen.state().tab().unwrap().text;
    screen
        .output()
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::epaint::Shape::Text(shape) if shape.galley.text() == text => Some(shape.clone()),
            _ => None,
        })
        .expect("Editor should paint its document")
}

fn point_at_character(screen: &Harness<'static, MorrowApp>, character: usize) -> egui::Pos2 {
    let document = document_shape(screen);
    let rect = document
        .galley
        .pos_from_cursor(egui::text::CCursor::new(character));
    document.pos + egui::vec2(rect.left() + 1.0, rect.center().y)
}

#[test]
fn real_mouse_click_moves_vim_cursor_on_press_and_keeps_it_after_release() {
    let mut app = fixture();
    app.projects[0].active_tab = 1;
    let mut screen = harness(app);
    let point = point_at_character(&screen, 8);
    screen.hover_at(point);
    screen.drag_at(point);
    screen.run_steps(1);
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 8);
    screen.drop_at(point);
    screen.run_steps(2);
    let tab = screen.state().tab().unwrap();
    assert_eq!(tab.vim.cursor, 8);
    assert_eq!(tab.vim.mode, VimMode::Normal);
    assert_eq!(tab.selection, 8..8);
    let caret = screen.state().editor_cursor_rect.unwrap();
    assert!((caret.center().y - point.y).abs() < 1.0);
    assert!(
        screen
            .output()
            .shapes
            .iter()
            .any(|shape| matches!(&shape.shape,
                egui::epaint::Shape::Rect(rect)
                    if rect.fill == ACCENT.linear_multiply(0.65)
                        && rect.rect.min.distance(caret.min) < 1.0
                        && rect.rect.width() > 0.0
            )),
        "The focused Normal-mode caret must be painted at the mouse destination"
    );
}

#[test]
fn wrapped_document_keeps_logical_gutter_numbers_and_font() {
    let mut app = fixture();
    let tab = app.tab_mut().unwrap();
    tab.text = format!("{}\nsecond\nthird", "long words ".repeat(45));
    tab.saved_text = tab.text.clone();
    let mut screen = harness(app);
    let unwrapped = document_shape(&screen);
    screen.state_mut().settings.word_wrap = true;
    screen.run_steps(2);
    let document = document_shape(&screen);
    assert!(document.galley.rows.len() > 3);
    assert_eq!(
        document.galley.job.sections[0].format.font_id,
        unwrapped.galley.job.sections[0].format.font_id
    );
    let numbers: Vec<_> = screen
        .output()
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::epaint::Shape::Text(shape)
                if shape.pos.x < document.pos.x
                    && matches!(shape.galley.text(), "1" | "2" | "3") =>
            {
                Some(shape.galley.text().to_owned())
            }
            _ => None,
        })
        .collect();
    assert_eq!(numbers, ["1", "2", "3"]);
}

fn pointer_click(screen: &mut Harness<'static, MorrowApp>, point: egui::Pos2) {
    screen.hover_at(point);
    screen.drag_at(point);
    screen.run_steps(1);
    screen.event(egui::Event::PointerButton {
        pos: point,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    screen.run_steps(1);
}

#[test]
fn reverse_mouse_drag_keeps_original_anchor_and_selection_for_vim_yank() {
    let mut app = fixture();
    app.projects[0].active_tab = 1;
    let mut screen = harness(app);
    let start = point_at_character(&screen, 10);
    let middle = point_at_character(&screen, 8);
    let end = point_at_character(&screen, 6);
    screen.hover_at(start);
    screen.drag_at(start);
    screen.run_steps(1);
    screen.hover_at(middle);
    screen.run_steps(1);
    assert_eq!(
        screen
            .state()
            .tab()
            .unwrap()
            .vim
            .selection(&screen.state().tab().unwrap().text),
        Some(8..10)
    );
    screen.hover_at(end);
    screen.run_steps(1);
    screen.drop_at(end);
    screen.run_steps(2);
    let tab = screen.state().tab().unwrap();
    assert_eq!(tab.vim.mode, VimMode::Visual);
    assert_eq!(tab.selection, 6..10);
    assert_eq!(tab.vim.cursor, 6);
    screen.event(egui::Event::Text("y".into()));
    screen.run_steps(1);
    assert_eq!(screen.state().register.as_ref().unwrap().text, "seco");
    assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::Normal);
}

#[test]
fn forward_mouse_drag_including_newline_can_be_deleted_and_undone_with_vim() {
    let mut app = fixture();
    app.projects[0].active_tab = 1;
    let original = app.tab().unwrap().text.clone();
    let mut screen = harness(app);
    let start = point_at_character(&screen, 6);
    let end = point_at_character(&screen, 13);
    screen.hover_at(start);
    screen.drag_at(start);
    screen.run_steps(1);
    screen.hover_at(end);
    screen.run_steps(1);
    screen.drop_at(end);
    screen.run_steps(1);
    assert_eq!(screen.state().tab().unwrap().selection, 6..13);
    screen.event(egui::Event::Text("d".into()));
    screen.run_steps(1);
    assert_eq!(screen.state().tab().unwrap().text, "first\nthird\n");
    screen.event(egui::Event::Text("u".into()));
    screen.run_steps(1);
    assert_eq!(screen.state().tab().unwrap().text, original);
}

#[test]
fn double_click_selects_word_and_triple_click_selects_logical_line() {
    let mut app = fixture();
    app.projects[0].active_tab = 1;
    let mut screen = harness(app);
    let point = point_at_character(&screen, 8);
    pointer_click(&mut screen, point);
    pointer_click(&mut screen, point);
    let tab = screen.state().tab().unwrap();
    assert_eq!(tab.vim.mode, VimMode::Visual);
    assert_eq!(tab.selection, 6..12);
    pointer_click(&mut screen, point);
    screen.run_steps(2);
    let tab = screen.state().tab().unwrap();
    assert_eq!(tab.vim.mode, VimMode::VisualLine);
    assert_eq!(tab.selection, 6..13);
}

#[test]
fn clicking_in_insert_mode_preserves_transaction_and_inserts_at_mouse_position() {
    let mut app = fixture();
    app.projects[0].active_tab = 1;
    let tab = app.tab_mut().unwrap();
    let original = tab.text.clone();
    tab.vim.handle_key(&mut tab.text, VimKey::Char('i'));
    let mut screen = harness(app);
    let point = point_at_character(&screen, 8);
    pointer_click(&mut screen, point);
    assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::Insert);
    screen.event(egui::Event::Text("Q".into()));
    screen.run_steps(1);
    assert_eq!(
        screen.state().tab().unwrap().text,
        "first\nseQcond\nthird\n"
    );
    screen.key_press(Key::Escape);
    screen.run_steps(1);
    screen.event(egui::Event::Text("u".into()));
    screen.run_steps(1);
    assert_eq!(screen.state().tab().unwrap().text, original);
}

#[test]
fn mouse_can_position_caret_on_wrapped_continuation() {
    let mut app = fixture();
    app.settings.word_wrap = true;
    let tab = app.tab_mut().unwrap();
    tab.text = format!("{}\nsecond line", "long words ".repeat(45));
    tab.saved_text = tab.text.clone();
    let mut screen = harness(app);
    let point = point_at_character(&screen, 240);
    pointer_click(&mut screen, point);
    let tab = screen.state().tab().unwrap();
    assert_eq!(tab.vim.cursor, 240);
    assert_eq!(tab.selection, 240..240);
    assert_eq!(
        tab.lines.line_col(&tab.text, tab.revision, tab.vim.cursor),
        (1, 241)
    );
}

#[test]
fn wrapped_diagnostics_and_annotations_follow_visual_rows() {
    let mut app = fixture();
    app.settings.word_wrap = true;
    let tab = app.tab_mut().unwrap();
    tab.text = format!("{}\nsecond line", "long words ".repeat(45));
    tab.saved_text = tab.text.clone();
    tab.annotations = vec![crate::cli::Annotation {
        id: "wrapped-range".into(),
        start: crate::cli::Location {
            line: 1,
            column: 151,
        },
        end: Some(crate::cli::Location {
            line: 1,
            column: 380,
        }),
        note: "A range across wrapped rows".into(),
        severity: crate::cli::AnnotationSeverity::Info,
    }];
    tab.annotations_revision = Some(tab.revision);
    let path = tab.path.clone().unwrap();
    app.projects[0].diagnostics.insert(
        path,
        vec![crate::lsp::Diagnostic {
            range: crate::lsp::TextRange {
                start: crate::lsp::Position {
                    line: 0,
                    character: 150,
                },
                end: crate::lsp::Position {
                    line: 0,
                    character: 380,
                },
            },
            severity: 1,
            message: "Wrapped diagnostic".into(),
            source: None,
        }],
    );
    let screen = harness(app);
    let document = document_shape(&screen);
    let expected_rows: Vec<_> = crate::render::visual_rows(&document.galley)
        .enumerate()
        .filter_map(|(index, row)| {
            crate::render::range_on_visual_row(&document.galley, index, row, 150, 380)
        })
        .collect();
    assert!(expected_rows.len() > 1);
    let underlines: Vec<_> = screen
        .output()
        .shapes
        .iter()
        .filter_map(|shape| match shape.shape {
            egui::epaint::Shape::LineSegment { points, stroke } if stroke.color == RED => {
                Some(points)
            }
            _ => None,
        })
        .collect();
    assert_eq!(underlines.len(), expected_rows.len());
    for (line, expected) in underlines.iter().zip(expected_rows) {
        assert!((line[0].y - document.pos.y - expected.bottom() + 1.0).abs() < 1.0);
    }
    let color = annotation_color(crate::cli::AnnotationSeverity::Info);
    let annotation_rects = screen.output().shapes.iter().filter(|shape| matches!(&shape.shape,
        egui::epaint::Shape::Rect(rect) if rect.fill == Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 25)
    )).count();
    assert_eq!(annotation_rects, underlines.len());
}

#[test]
fn default_editor_wheel_scroll_is_immediate_and_has_no_smoothing_tail() {
    let mut app = fixture();
    assert!(!app.settings.smooth_scroll);
    let tab = app.tab_mut().unwrap();
    tab.text = "a readable source line\n".repeat(150);
    tab.saved_text = tab.text.clone();
    let mut screen = harness(app);
    let before = document_shape(&screen).pos.y;
    let pointer = screen.state().editor_rect.unwrap().center();
    screen.hover_at(pointer);
    screen.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, -120.0),
        modifiers: egui::Modifiers::NONE,
        phase: egui::TouchPhase::Move,
    });
    screen.run_steps(1);
    let after = document_shape(&screen).pos.y;
    assert!(
        (before - after - 120.0).abs() < 1.0,
        "One wheel event must move the entire requested distance in its first frame"
    );
    screen.run_steps(5);
    assert!((document_shape(&screen).pos.y - after).abs() < 1.0);
}

#[test]
fn editor_wheel_frames_accumulate_reverse_and_clamp_at_document_boundaries() {
    let mut app = fixture();
    let tab = app.tab_mut().unwrap();
    tab.text = (0..200)
        .map(|line| format!("Readable source line {line}\n"))
        .collect();
    tab.saved_text = tab.text.clone();
    let original = tab.text.clone();
    let mut screen = harness(app);
    let initial = document_shape(&screen).pos.y;
    let pointer = screen.state().editor_rect.unwrap().center();
    screen.hover_at(pointer);
    let wheel = |screen: &mut Harness<'static, MorrowApp>, delta| {
        screen.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, delta),
            modifiers: egui::Modifiers::NONE,
            phase: egui::TouchPhase::Move,
        });
        screen.run_steps(1);
    };
    for (delta, expected_offset) in [(-96.0, 96.0), (-64.0, 160.0), (40.0, 120.0)] {
        wheel(&mut screen, delta);
        assert!(
            (initial - document_shape(&screen).pos.y - expected_offset).abs() < 1.0,
            "Each wheel frame must continue from the existing document position"
        );
    }
    let position = document_shape(&screen).pos.y;
    screen.run_steps(4);
    assert!((document_shape(&screen).pos.y - position).abs() < 1.0);

    wheel(&mut screen, -1_000_000.0);
    let bottom = document_shape(&screen);
    let clip = screen
        .output()
        .shapes
        .iter()
        .find(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == original))
        .unwrap()
        .clip_rect;
    let last_line = bottom
        .galley
        .pos_from_cursor(egui::text::CCursor::new(original.chars().count() - 1))
        .translate(bottom.pos.to_vec2());
    assert!(last_line.center().y >= clip.top() && last_line.center().y <= clip.bottom());
    wheel(&mut screen, -1_000_000.0);
    assert!((document_shape(&screen).pos.y - bottom.pos.y).abs() < 1.0);
    wheel(&mut screen, 80.0);
    assert!((document_shape(&screen).pos.y - bottom.pos.y - 80.0).abs() < 1.0);
    wheel(&mut screen, 1_000_000.0);
    assert!((document_shape(&screen).pos.y - initial).abs() < 1.0);
    wheel(&mut screen, 1_000_000.0);
    assert!((document_shape(&screen).pos.y - initial).abs() < 1.0);
    let tab = screen.state().tab().unwrap();
    assert_eq!(tab.text, original);
    assert!(!tab.dirty);
    assert_eq!(tab.vim.cursor, 0);
    assert_eq!(tab.selection, 0..0);
}

#[test]
fn editor_horizontal_wheel_frames_accumulate_reverse_and_clamp_at_line_boundaries() {
    let mut app = fixture();
    let tab = app.tab_mut().unwrap();
    tab.text = format!("{}LAST_WORD\n", "long_identifier ".repeat(300));
    tab.saved_text = tab.text.clone();
    let original = tab.text.clone();
    let mut screen = harness(app);
    let initial = document_shape(&screen).pos.x;
    let pointer = screen.state().editor_rect.unwrap().center();
    screen.hover_at(pointer);
    let wheel = |screen: &mut Harness<'static, MorrowApp>, delta| {
        screen.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(delta, 0.0),
            modifiers: egui::Modifiers::NONE,
            phase: egui::TouchPhase::Move,
        });
        screen.run_steps(1);
    };
    for (delta, expected_offset) in [(-120.0, 120.0), (-60.0, 180.0), (45.0, 135.0)] {
        wheel(&mut screen, delta);
        assert!(
            (initial - document_shape(&screen).pos.x - expected_offset).abs() < 1.0,
            "Horizontal wheel frames must accumulate and reverse immediately"
        );
    }
    let position = document_shape(&screen).pos.x;
    screen.run_steps(4);
    assert!((document_shape(&screen).pos.x - position).abs() < 1.0);
    wheel(&mut screen, -1_000_000.0);
    let right = document_shape(&screen);
    let clip = screen
        .output()
        .shapes
        .iter()
        .find(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == original))
        .unwrap()
        .clip_rect;
    let end = right
        .galley
        .pos_from_cursor(egui::text::CCursor::new(original.chars().count() - 1))
        .translate(right.pos.to_vec2());
    assert!(end.left() >= clip.left() && end.left() <= clip.right());
    wheel(&mut screen, -1_000_000.0);
    assert!((document_shape(&screen).pos.x - right.pos.x).abs() < 1.0);
    wheel(&mut screen, 80.0);
    assert!((document_shape(&screen).pos.x - right.pos.x - 80.0).abs() < 1.0);
    wheel(&mut screen, 1_000_000.0);
    assert!((document_shape(&screen).pos.x - initial).abs() < 1.0);
    wheel(&mut screen, 1_000_000.0);
    assert!((document_shape(&screen).pos.x - initial).abs() < 1.0);
    let tab = screen.state().tab().unwrap();
    assert_eq!(tab.text, original);
    assert!(!tab.dirty);
    assert_eq!(tab.vim.cursor, 0);
}

#[test]
fn opening_editor_context_menu_preserves_visual_selection() {
    let mut app = fixture();
    app.projects[0].active_tab = 1;
    let tab = app.tab_mut().unwrap();
    tab.vim.set_selection(&tab.text, 6..12);
    tab.selection = 6..12;
    tab.pending_selection = Some(6..12);
    let mut screen = harness(app);
    let point = point_at_character(&screen, 8);
    screen.hover_at(point);
    screen.event(egui::Event::PointerButton {
        pos: point,
        button: egui::PointerButton::Secondary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    screen.run_steps(1);
    screen.event(egui::Event::PointerButton {
        pos: point,
        button: egui::PointerButton::Secondary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    screen.run_steps(2);
    assert_eq!(screen.state().tab().unwrap().selection, 6..12);
    assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::Visual);
    assert!(egui::Popup::is_any_open(&screen.ctx));
    let copy_point = screen.get_by_label("Copy Ctrl+C").rect().center();
    let mut copied = false;
    for event in [
        egui::Event::PointerMoved(copy_point),
        egui::Event::PointerButton {
            pos: copy_point,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        },
        egui::Event::PointerButton {
            pos: copy_point,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        },
    ] {
        screen.input_mut().events.push(event);
        screen.run_steps(1);
        copied |= screen.output().platform_output.commands.iter().any(
            |command| matches!(command, egui::OutputCommand::CopyText(text) if text == "second"),
        );
    }
    assert!(
        copied,
        "Copy must emit its clipboard command during the actual click frames"
    );
    assert!(screen.state().register.is_none());
    let tab = screen.state().tab().unwrap();
    assert!(tab.vim.register().is_none());
    assert_eq!(tab.selection, 6..12);
    assert_eq!(tab.vim.mode, VimMode::Visual);
}

#[test]
fn editor_context_cut_repeats_and_menu_undo_remains_live_during_save_errors() {
    let mut app = fixture();
    app.projects[0].active_tab = 1;
    let preserved = Yank {
        text: "Vim register 🦀\n".into(),
        linewise: true,
    };
    app.register = Some(preserved.clone());
    let tab = app.tab_mut().unwrap();
    tab.vim.set_register(preserved.clone());
    tab.text = "abcdefghij\n".into();
    tab.saved_text = tab.text.clone();
    tab.vim.set_selection(&tab.text, 0..3);
    tab.vim.handle_key(&mut tab.text, VimKey::Char('r'));
    tab.selection = 0..3;
    tab.pending_selection = Some(0..3);
    tab.saving = true;
    tab.error = Some("Save failed; use Save As".into());
    let mut screen = harness(app);

    let choose = |screen: &mut Harness<'static, MorrowApp>, label: &str| {
        let point = point_at_character(screen, 1);
        screen
            .input_mut()
            .events
            .push(egui::Event::PointerMoved(point));
        screen.run_steps(1);
        for pressed in [true, false] {
            screen.input_mut().events.push(egui::Event::PointerButton {
                pos: point,
                button: egui::PointerButton::Secondary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
            screen.run_steps(1);
        }
        screen.run_steps(2);
        assert!(egui::Popup::is_any_open(&screen.ctx));
        let choice = screen.get_by_label(label).rect().center();
        let mut copied = Vec::new();
        for event in [
            egui::Event::PointerMoved(choice),
            egui::Event::PointerButton {
                pos: choice,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: choice,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ] {
            screen.input_mut().events.push(event);
            screen.run_steps(1);
            for command in &screen.output().platform_output.commands {
                if let egui::OutputCommand::CopyText(text) = command {
                    copied.push(text.clone());
                }
            }
        }
        screen.run_steps(2);
        assert!(!egui::Popup::is_any_open(&screen.ctx));
        copied
    };

    assert_eq!(choose(&mut screen, "Cut Ctrl+X"), ["abc"]);
    assert_eq!(screen.state().tab().unwrap().text, "defghij\n");
    assert_eq!(screen.state().register.as_ref(), Some(&preserved));
    assert_eq!(
        screen.state().tab().unwrap().vim.register(),
        Some(&preserved)
    );
    assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::Normal);
    assert!(
        screen
            .state()
            .tab()
            .unwrap()
            .vim
            .pending_command()
            .is_empty()
    );
    screen.event(egui::Event::Text(".".into()));
    screen.run_steps(1);
    assert_eq!(screen.state().tab().unwrap().text, "ghij\n");
    assert_eq!(screen.state().register.as_ref().unwrap().text, "def");
    assert!(!screen.state().register.as_ref().unwrap().linewise);
    assert!(
        screen
            .output()
            .platform_output
            .commands
            .iter()
            .all(|command| { !matches!(command, egui::OutputCommand::CopyText(_)) })
    );
    screen.event(egui::Event::Text("u".into()));
    screen.run_steps(1);
    assert_eq!(screen.state().tab().unwrap().text, "defghij\n");
    choose(&mut screen, "Undo u / Ctrl+Z");
    let tab = screen.state().tab().unwrap();
    assert_eq!(tab.text, "abcdefghij\n");
    assert!(tab.saving);
    assert_eq!(tab.error.as_deref(), Some("Save failed; use Save As"));
}

#[test]
fn native_undo_history_is_shared_between_editor_panes() {
    let mut app = fixture();
    app.settings.vim = false;
    app.projects[0].active_tab = 1;
    let tab = app.tab_mut().unwrap();
    tab.text = "base".into();
    tab.saved_text = tab.text.clone();
    let mut screen = harness(app);
    screen.event(egui::Event::Text("A".into()));
    screen.run_steps(1);
    assert_eq!(screen.state().tab().unwrap().text, "Abase");
    let right = screen
        .state_mut()
        .split_active(crate::cli::SplitOrientation::Vertical)
        .unwrap();
    screen.run_steps(1);
    screen.event(egui::Event::Text("B".into()));
    screen.run_steps(1);
    assert_eq!(screen.state().tab().unwrap().text, "ABbase");
    screen
        .state_mut()
        .control_layout(crate::cli::LayoutAction::Focus { pane_id: 1 })
        .unwrap();
    screen.run_steps(1);
    screen.key_press_modifiers(egui::Modifiers::COMMAND, Key::Z);
    screen.run_steps(1);
    assert_eq!(
        screen.state().tab().unwrap().text,
        "Abase",
        "Undo in the left pane must undo the most recent edit made in the right pane"
    );
    screen
        .state_mut()
        .control_layout(crate::cli::LayoutAction::Focus { pane_id: right })
        .unwrap();
    screen.run_steps(1);
    screen.key_press_modifiers(egui::Modifiers::COMMAND, Key::Z);
    screen.run_steps(1);
    assert_eq!(screen.state().tab().unwrap().text, "base");
    screen.key_press_modifiers(egui::Modifiers::COMMAND, Key::Y);
    screen.run_steps(1);
    assert_eq!(screen.state().tab().unwrap().text, "Abase");
    screen
        .state_mut()
        .control_layout(crate::cli::LayoutAction::Focus { pane_id: 1 })
        .unwrap();
    screen.run_steps(1);
    screen.key_press_modifiers(egui::Modifiers::COMMAND, Key::Y);
    screen.run_steps(1);
    assert_eq!(
        screen.state().tab().unwrap().text,
        "ABbase",
        "Redo must also survive changing editor panes"
    );
}

#[test]
fn native_keyboard_cli_and_context_actions_use_one_document_history() {
    let mut app = fixture();
    app.settings.vim = false;
    app.projects[0].active_tab = 1;
    let original = app.tab().unwrap().text.clone();
    let mut screen = harness(app);
    let tab = screen.state().tab().unwrap();
    let (id, revision) = (tab.id, tab.revision);
    let response = screen
        .state_mut()
        .control_request(crate::cli::Request::Edit {
            tab_id: id,
            expected_revision: revision,
            edits: Vec::new(),
            set_text: Some("Agent replacement".into()),
        });
    assert!(response.ok);
    screen.run_steps(1);
    screen.key_press_modifiers(egui::Modifiers::COMMAND, Key::Z);
    screen.run_steps(1);
    assert_eq!(screen.state().tab().unwrap().text, original);
    screen.key_press_modifiers(egui::Modifiers::COMMAND, Key::Y);
    screen.run_steps(1);
    assert_eq!(screen.state().tab().unwrap().text, "Agent replacement");
    let start = point_at_character(&screen, 0);
    pointer_click(&mut screen, start);
    screen.event(egui::Event::Text("Q".into()));
    screen.run_steps(1);
    assert_eq!(screen.state().tab().unwrap().text, "QAgent replacement");
    let revision = screen.state().tab().unwrap().revision;
    let response = screen
        .state_mut()
        .control_request(crate::cli::Request::Command {
            tab_id: id,
            command: crate::cli::EditorCommand::Undo,
            expected_revision: revision,
        });
    assert!(response.ok);
    screen.run_steps(1);
    assert_eq!(screen.state().tab().unwrap().text, "Agent replacement");
    screen
        .state_mut()
        .apply_editor_context_action(id, context_actions::EditorAction::Redo);
    screen.run_steps(1);
    assert_eq!(screen.state().tab().unwrap().text, "QAgent replacement");
}

#[test]
fn native_history_survives_markdown_preview_and_cli_edit() {
    let mut app = fixture();
    app.settings.vim = false;
    let original = app.tab().unwrap().text.clone();
    let mut screen = harness(app);
    screen.event(egui::Event::Text("Title: ".into()));
    screen.run_steps(1);
    let typed = screen.state().tab().unwrap().text.clone();
    screen.state_mut().set_document_view(DocumentView::Preview);
    screen.run_steps(1);
    let tab = screen.state().tab().unwrap();
    let (id, revision) = (tab.id, tab.revision);
    let response = screen
        .state_mut()
        .control_request(crate::cli::Request::Edit {
            tab_id: id,
            expected_revision: revision,
            edits: Vec::new(),
            set_text: Some("# Agent plan\n".into()),
        });
    assert!(response.ok);
    screen.state_mut().set_document_view(DocumentView::Source);
    screen.run_steps(1);
    screen.key_press_modifiers(egui::Modifiers::COMMAND, Key::Z);
    screen.run_steps(1);
    assert_eq!(screen.state().tab().unwrap().text, typed);
    screen.key_press_modifiers(egui::Modifiers::COMMAND, Key::Z);
    screen.run_steps(1);
    assert_eq!(screen.state().tab().unwrap().text, original);
}

#[test]
fn native_disk_reload_starts_fresh_undo_history() {
    let mut app = fixture();
    app.settings.vim = false;
    let mut screen = harness(app);
    screen.event(egui::Event::Text("Old edit ".into()));
    screen.run_steps(1);
    let app = screen.state_mut();
    let tab = app.tab_mut().unwrap();
    let id = tab.id;
    tab.loading = true;
    let replacement = "# Reloaded disk plan\n";
    app.sender
        .send(WorkerResult::Loaded {
            id,
            result: Ok(workspace::LoadedDocument {
                text: replacement.into(),
                line_ending: workspace::LineEnding::Lf,
                bom: false,
                mixed_line_endings: false,
                fingerprint: workspace::FileFingerprint {
                    len: replacement.len() as u64,
                    modified: None,
                    hash: 42,
                },
            }),
            recovery: None,
        })
        .unwrap();
    app.poll();
    screen.run_steps(1);
    screen.key_press_modifiers(egui::Modifiers::COMMAND, Key::Z);
    screen.run_steps(1);
    assert_eq!(screen.state().tab().unwrap().text, replacement);
    screen.key_press_modifiers(egui::Modifiers::COMMAND, Key::Y);
    screen.run_steps(1);
    assert_eq!(screen.state().tab().unwrap().text, replacement);
}

#[test]
fn native_cli_selection_survives_first_source_frame_after_preview() {
    let mut app = fixture();
    app.settings.vim = false;
    let mut screen = harness(app);
    screen.state_mut().set_document_view(DocumentView::Preview);
    screen.run_steps(2);
    let tab = screen.state().tab().unwrap();
    let (id, revision) = (tab.id, tab.revision);
    let response = screen
        .state_mut()
        .control_request(crate::cli::Request::Selection {
            tab_id: id,
            range: Some(crate::cli::TextRange {
                start: crate::cli::Location { line: 3, column: 1 },
                end: crate::cli::Location { line: 3, column: 6 },
            }),
            expected_revision: Some(revision),
        });
    assert!(response.ok);
    screen.state_mut().set_document_view(DocumentView::Source);
    let expected = editor::go_to_line(&screen.state().tab().unwrap().text, 3);
    for _ in 0..3 {
        screen.run_steps(1);
        assert_eq!(
            screen.state().tab().unwrap().selection,
            expected..expected + 5
        );
        let native = egui::TextEdit::load_state(&screen.ctx, egui::Id::new(("document", id)))
            .unwrap()
            .cursor
            .char_range()
            .unwrap()
            .as_sorted_char_range();
        assert_eq!(native.start.0..native.end.0, expected..expected + 5);
    }
    screen.event(egui::Event::Text("New".into()));
    screen.run_steps(1);
    assert!(screen.state().tab().unwrap().text.contains("New line"));
}

#[test]
fn native_selection_survives_another_pane_having_keyboard_focus() {
    let mut app = fixture();
    app.settings.vim = false;
    app.projects[0].active_tab = 1;
    let mut screen = harness(app);
    screen.state_mut().tab_mut().unwrap().set_cursor(6..12);
    screen.run_steps(2);
    let id = screen.state().tab().unwrap().id;
    let right = screen
        .state_mut()
        .split_active(crate::cli::SplitOrientation::Vertical)
        .unwrap();
    screen.run_steps(3);
    screen.state_mut().tab_mut().unwrap().set_cursor(0..0);
    screen.run_steps(3);
    let left = egui::TextEdit::load_state(&screen.ctx, egui::Id::new(("document", id)))
        .unwrap()
        .cursor
        .char_range()
        .unwrap()
        .as_sorted_char_range();
    assert_eq!(left.start.0..left.end.0, 6..12);
    assert_eq!(screen.state().layout_status()["focused_pane"], right);
    screen
        .state_mut()
        .control_layout(crate::cli::LayoutAction::Focus { pane_id: 1 })
        .unwrap();
    screen.run_steps(2);
    assert_eq!(screen.state().tab().unwrap().selection, 6..12);
}

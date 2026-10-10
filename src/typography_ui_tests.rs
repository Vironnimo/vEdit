//! Font preferences must change painted text without disturbing editor state.
use super::*;
use egui_kittest::{Harness, kittest::Queryable};

const DOCUMENT: &str = "before: target after\nSecond line · β🦀\n";

fn fixture() -> VEditApp {
    let mut app = VEditApp::empty(egui::Context::default());
    app.settings.autosave = false;
    app.settings.smooth_scroll = false;
    app.sidebar_visible = false;
    let root = std::env::temp_dir().join("vedit-typography-render-only");
    let mut project = Project::new(root.clone());
    project.index = Some(Arc::new(workspace::WorkspaceIndex::empty(root.clone())));
    project.last_git_refresh = Instant::now();
    let mut tab = EditorTab::new(Some(root.join("notes.txt")), "notes.txt".into());
    tab.text = DOCUMENT.into();
    tab.saved_text = DOCUMENT.into();
    project.tabs.push(tab);
    app.projects.push(project);
    app.bind_current_pane();
    app
}

fn harness(app: VEditApp) -> Harness<'static, VEditApp> {
    let mut styled = false;
    let mut screen = Harness::builder()
        .with_size([1200.0, 900.0])
        .with_os(egui::os::OperatingSystem::Windows)
        .build_ui_state(
            move |ui, app: &mut VEditApp| {
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

fn source_shape(screen: &Harness<'_, VEditApp>, text: &str) -> egui::epaint::TextShape {
    screen
        .output()
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::epaint::Shape::Text(shape) if shape.galley.text() == text => Some(shape.clone()),
            _ => None,
        })
        .expect("The editor must paint the requested source document")
}

fn character_point(screen: &Harness<'_, VEditApp>, character: usize) -> egui::Pos2 {
    let document = source_shape(screen, DOCUMENT);
    let cursor = document
        .galley
        .pos_from_cursor(egui::text::CCursor::new(character));
    document.pos + egui::vec2(cursor.left() + 1.0, cursor.center().y)
}

fn select_target(screen: &mut Harness<'_, VEditApp>) {
    let start = character_point(screen, 8);
    let end = character_point(screen, 14);
    screen.hover_at(start);
    screen.drag_at(start);
    screen.run_steps(1);
    screen.hover_at(end);
    screen.run_steps(1);
    screen.drop_at(end);
    screen.run_steps(2);
    assert_eq!(screen.state().tab().unwrap().selection, 8..14);
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
    screen.run_steps(3);
}

#[test]
fn interface_zoom_is_bounded_saved_and_resettable_while_settings_is_open() {
    let mut app = fixture();
    app.settings.ui_scale = 1.4;
    let mut screen = harness(app);
    assert!((screen.ctx.zoom_factor() - 1.4).abs() < 0.001);
    key(&mut screen, Key::Comma, egui::Modifiers::COMMAND);
    key(&mut screen, Key::Plus, egui::Modifiers::COMMAND);
    assert!((screen.state().settings.ui_scale - 1.5).abs() < 0.001);
    assert!((screen.ctx.zoom_factor() - 1.5).abs() < 0.001);
    let saved: Session =
        serde_json::from_slice(&serde_json::to_vec(&screen.state().session()).unwrap()).unwrap();
    assert_eq!(saved.settings.unwrap().ui_scale, 1.5);
    screen.state_mut().settings.ui_scale = 2.0;
    screen.set_size(Vec2::new(960.0, 640.0));
    screen.run_steps(5);
    key(&mut screen, Key::Plus, egui::Modifiers::COMMAND);
    assert_eq!(screen.state().settings.ui_scale, 2.0);
    let done = screen.get_by_label("Done").rect();
    assert!(
        screen.ctx.content_rect().contains_rect(done),
        "Done must remain accessible at 200%: {done:?}"
    );
    key(&mut screen, Key::Num0, egui::Modifiers::COMMAND);
    assert_eq!(screen.state().settings.ui_scale, 1.0);
    assert_eq!(screen.ctx.zoom_factor(), 1.0);
    assert!(screen.state().settings_open);
    assert_eq!(screen.state().tab().unwrap().text, DOCUMENT);
    assert_eq!(screen.state().tab().unwrap().revision, 0);
}

#[test]
fn source_reading_preferences_change_geometry_numbers_and_highlight_without_editing() {
    let mut app = fixture();
    app.tab_mut().unwrap().set_cursor(21..21);
    let mut screen = harness(app);
    let before = source_shape(&screen, DOCUMENT);
    screen.state_mut().settings.font_size = 32.0;
    screen.state_mut().settings.line_height = 2.0;
    screen.state_mut().settings.line_numbers = crate::cli::LineNumbers::Relative;
    screen.run_steps(3);
    let larger = source_shape(&screen, DOCUMENT);
    assert!(larger.galley.rows[1].pos.y > before.galley.rows[1].pos.y * 2.0);
    assert_eq!(larger.galley.job.sections[0].format.line_height, Some(64.0));
    let numbers = |screen: &Harness<'_, VEditApp>| -> Vec<String> {
        let source = source_shape(screen, DOCUMENT);
        screen
            .output()
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(shape)
                    if shape.pos.x < source.pos.x
                        && shape.pos.y >= source.pos.y
                        && shape.pos.y < source.pos.y + source.galley.size().y
                        && !shape.galley.text().is_empty()
                        && shape.galley.text().chars().all(|ch| ch.is_ascii_digit()) =>
                {
                    Some(shape.galley.text().to_owned())
                }
                _ => None,
            })
            .collect()
    };
    assert_eq!(numbers(&screen), ["1", "2", "1"]);
    let has_highlight = |screen: &Harness<'_, VEditApp>| {
        screen.output().shapes.iter().any(|shape| {
            matches!(&shape.shape,
            egui::epaint::Shape::Rect(rect) if rect.fill == ACCENT.linear_multiply(0.055))
        })
    };
    assert!(has_highlight(&screen));
    screen.state_mut().settings.line_numbers = crate::cli::LineNumbers::Off;
    screen.state_mut().settings.highlight_current_line = false;
    screen.run_steps(2);
    assert!(numbers(&screen).is_empty());
    assert!(!has_highlight(&screen));
    assert_eq!(screen.state().tab().unwrap().text, DOCUMENT);
    assert_eq!(screen.state().tab().unwrap().revision, 0);
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 21);
}

#[test]
fn settings_font_combo_preserves_mouse_selection_mode_and_document() {
    for vim in [true, false] {
        let mut app = fixture();
        app.settings.vim = vim;
        let mut screen = harness(app);
        select_target(&mut screen);
        let tab = screen.state().tab().unwrap();
        let original = (tab.text.clone(), tab.revision, tab.vim.cursor, tab.vim.mode);
        if vim {
            assert_eq!(tab.vim.mode, VimMode::Visual);
        }
        let original_shape = source_shape(&screen, DOCUMENT);

        key(&mut screen, Key::Comma, egui::Modifiers::COMMAND);
        assert!(screen.state().settings_open);
        // Allow the centered modal to settle before recording a pointer target.
        screen.run_steps(3);
        let combo_rect = screen.get_by_label("Editor font").rect();
        screen.hover_at(combo_rect.center());
        screen.run_steps(2);
        assert_eq!(
            screen.get_by_label("Editor font").rect(),
            combo_rect,
            "A settled Settings control must not move when the pointer approaches it"
        );
        screen.get_by_label("Editor font").click();
        screen.run_steps(3);
        assert_eq!(
            screen.state().settings.font_size,
            14.0,
            "Choosing a code face must not activate the adjacent font-size slider"
        );
        assert!(
            egui::Popup::is_any_open(&screen.ctx),
            "The actual font chooser must open"
        );
        screen.get_by_label("Consolas").click();
        screen.run_steps(3);
        assert_eq!(
            screen.state().settings.editor_font,
            crate::cli::EditorFont::Consolas
        );
        assert_eq!(
            screen.state().applied_editor_font,
            crate::cli::EditorFont::Consolas
        );
        screen.get_by_label("Done").click();
        screen.run_steps(3);

        let tab = screen.state().tab().unwrap();
        assert_eq!(tab.selection, 8..14);
        assert_eq!(
            (tab.text.clone(), tab.revision, tab.vim.cursor, tab.vim.mode),
            original
        );
        assert!(!tab.dirty);
        assert!(!screen.state().settings_open);
        let changed = source_shape(&screen, DOCUMENT);
        assert!(
            !Arc::ptr_eq(&original_shape.galley, &changed.galley),
            "A font switch must replace the painted source galley"
        );
        let widget = screen.state().document_widget_id(tab.id);
        assert!(screen.ctx.memory(|memory| memory.has_focus(widget)));
    }
}

#[test]
fn receiver_font_change_relayouts_split_sources_and_keeps_search_marks_on_glyphs() {
    let mut app = fixture();
    app.settings.vim = false;
    let left_pane = app.current_pane_id();
    let left_id = app.tab().unwrap().id;
    let peer_text = "peer: target retained\nOther source\n";
    let mut peer = EditorTab::new(None, "peer.txt".into());
    peer.text = peer_text.into();
    peer.saved_text = peer.text.clone();
    let peer_id = peer.id;
    app.projects[0].tabs.push(peer);
    let right_pane = app
        .split_active(crate::cli::SplitOrientation::Vertical)
        .unwrap();
    for change in [
        crate::cli::LayoutAction::Assign {
            pane_id: right_pane,
            tab_id: peer_id,
            view: Some(crate::cli::DocumentView::Source),
        },
        crate::cli::LayoutAction::Focus {
            pane_id: right_pane,
        },
    ] {
        assert!(
            app.control_request(crate::cli::Request::Layout { change })
                .ok
        );
    }
    app.tab_mut().unwrap().set_cursor(18..18);
    app.sync_external_document_navigation(peer_id);
    assert!(
        app.control_request(crate::cli::Request::Layout {
            change: crate::cli::LayoutAction::Focus { pane_id: left_pane }
        })
        .ok
    );
    let mut screen = harness(app);
    select_target(&mut screen);
    screen.state_mut().find_visible = true;
    screen.state_mut().find_text = "target".into();
    screen.run_steps(3);
    let original_layout = serde_json::to_value(screen.state().editor_session_layout()).unwrap();
    let original_left = source_shape(&screen, DOCUMENT);
    let original_right = source_shape(&screen, peer_text);
    assert_eq!(screen.state().projects[0].tabs[0].id, left_id);
    let original_revision = screen.state().projects[0].tabs[0].revision;

    // Exercise the production receiver used by agent commands, not direct font installation.
    let recovery = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/test-state")
        .join(format!("typography-ui-{}-{peer_id}", std::process::id()));
    screen.state_mut().recovery_session_path = recovery.join("session.json");
    let response = screen
        .state_mut()
        .control_request(crate::cli::Request::Settings {
            patch: Some(crate::cli::SettingsPatch {
                editor_font: Some(crate::cli::EditorFont::Consolas),
                ..Default::default()
            }),
        });
    assert!(response.ok, "{response:?}");
    screen.run_steps(4);

    assert_eq!(
        serde_json::to_value(screen.state().editor_session_layout()).unwrap(),
        original_layout
    );
    assert_eq!(
        screen.state().projects[0].tabs[0].revision,
        original_revision
    );
    assert!(screen.state().projects[0].tabs.iter().all(|tab| !tab.dirty));
    assert_eq!(screen.state().projects[0].tabs[0].selection, 8..14);
    for (text, old_shape, start, end) in [
        (DOCUMENT, original_left, 8, 14),
        (peer_text, original_right, 6, 12),
    ] {
        let shape = source_shape(&screen, text);
        assert!(!Arc::ptr_eq(&old_shape.galley, &shape.galley));
        let left = shape
            .galley
            .pos_from_cursor(egui::text::CCursor::new(start))
            .left()
            + shape.pos.x;
        let right = shape
            .galley
            .pos_from_cursor(egui::text::CCursor::new(end))
            .left()
            + shape.pos.x;
        let top = shape.pos.y + shape.galley.rows[0].pos.y;
        let bottom = top + shape.galley.rows[0].size.y;
        assert!(
            screen
                .output()
                .shapes
                .iter()
                .any(|paint| matches!(&paint.shape,
                    egui::epaint::Shape::Rect(rect)
                        if rect.fill == Color32::from_rgba_unmultiplied(BLUE.r(), BLUE.g(), BLUE.b(), 38)
                            && (rect.rect.left() - left).abs() < 0.1
                            && (rect.rect.right() - right).abs() < 0.1
                            && (rect.rect.top() - top).abs() < 0.1
                            && (rect.rect.bottom() - bottom).abs() < 0.1
                )),
            "Search marks must follow the newly shaped glyphs in {text:?}"
        );
    }
}

#[test]
fn legacy_session_adds_bundled_font_without_resetting_existing_preferences() {
    let old_settings = serde_json::json!({
        "vim": false, "swap_jk": true, "autosave": false, "font_size": 18.0,
        "tab_width": 2, "word_wrap": true, "smooth_scroll": true,
        "leader_key": ",", "search_excludes": ["generated", "vendor"]
    });
    let old_session = serde_json::json!({
        "projects": [], "active_project": 0, "sidebar": "search", "settings": old_settings
    });
    let session: Session = serde_json::from_value(old_session).unwrap();
    assert_eq!(session.sidebar, Some(SidebarView::Search));
    let restored = session.settings.unwrap();
    assert_eq!(restored.editor_font, crate::cli::EditorFont::JetBrainsMono);
    assert_eq!(restored.ui_scale, 1.0);
    assert_eq!(restored.line_height, 1.35);
    assert_eq!(restored.line_numbers, crate::cli::LineNumbers::Absolute);
    assert!(restored.highlight_current_line);
    assert!(restored.show_ignored_files);
    assert_eq!(restored.autosave_delay_ms, 850);
    let updated = serde_json::to_value(&restored).unwrap();
    for (key, value) in old_settings.as_object().unwrap() {
        assert_eq!(
            &updated[key], value,
            "Existing preference {key} must survive migration"
        );
    }
}

#[test]
fn gutter_numbers_share_source_baselines_across_faces_sizes_and_wrapping() {
    for font in [
        crate::cli::EditorFont::JetBrainsMono,
        crate::cli::EditorFont::Consolas,
        crate::cli::EditorFont::CascadiaMono,
    ] {
        for size in [14.0, 20.0] {
            for wrap in [false, true] {
                let mut app = fixture();
                app.settings.editor_font = font;
                app.settings.font_size = size;
                app.settings.word_wrap = wrap;
                let text = format!(
                    "{}\n\nsecond line\nthird line\n",
                    "wide source words ".repeat(15)
                );
                let tab = app.tab_mut().unwrap();
                tab.text = text.clone();
                tab.saved_text = text.clone();
                let screen = harness(app);
                let source = source_shape(&screen, &text);
                if wrap {
                    assert!(source.galley.rows.len() > 5);
                }
                // Blank rows use the same font baseline within their line box
                // as the neighboring ASCII rows, including the final newline.
                let relative_baseline = source
                    .galley
                    .rows
                    .iter()
                    .find_map(|row| row.glyphs.first().map(|glyph| glyph.pos.y))
                    .expect("The source fixture contains ASCII glyphs");
                let mut empty_lines = Vec::new();
                for (row_index, mapped) in crate::render::visual_rows(&source.galley)
                    .enumerate()
                    .filter(|(_, row)| row.first_in_line)
                {
                    let number = mapped.line.to_string();
                    let gutter = screen
                        .output()
                        .shapes
                        .iter()
                        .find_map(|paint| match &paint.shape {
                            egui::epaint::Shape::Text(shape)
                                if shape.galley.text() == number && shape.pos.x < source.pos.x =>
                            {
                                Some(shape)
                            }
                            _ => None,
                        })
                        .unwrap_or_else(|| panic!("Missing logical gutter number {number}"));
                    let row = &source.galley.rows[row_index];
                    let baseline = row.glyphs.first().map_or_else(
                        || {
                            empty_lines.push(mapped.line);
                            relative_baseline
                        },
                        |glyph| glyph.pos.y,
                    );
                    let source_baseline = source.pos.y + row.pos.y + baseline;
                    let gutter_row = &gutter.galley.rows[0];
                    let gutter_baseline =
                        gutter.pos.y + gutter_row.pos.y + gutter_row.glyphs[0].pos.y;
                    assert!(
                        (source_baseline - gutter_baseline).abs() < 0.1,
                        "{font:?}, size={size}, wrap={wrap}, line={number}: source baseline {source_baseline}, gutter {gutter_baseline}"
                    );
                }
                assert_eq!(
                    empty_lines,
                    [2, 5],
                    "The interior and trailing blank rows must both be checked"
                );
            }
        }
    }
}

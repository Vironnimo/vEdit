//! Verify the accepted user-perspective review findings in the actual app UI.
use super::context_actions::EditorAction;
use super::*;
use egui_kittest::{Harness, kittest::Queryable};

fn fixture() -> VEditApp {
    let mut app = VEditApp::empty(egui::Context::default());
    visual_tests::workspace(&mut app);
    app.bind_current_pane();
    app
}

fn harness(app: VEditApp) -> Harness<'static, VEditApp> {
    let mut screen = Harness::builder()
        .with_size([960.0, 640.0])
        .with_os(egui::os::OperatingSystem::Windows)
        .build_ui_state(
            |ui, app: &mut VEditApp| {
                app.ctx = ui.ctx().clone();
                app.last_disk_check = Instant::now();
                app.last_session_save = Instant::now();
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
    screen.hover_at(egui::pos2(-20.0, -20.0));
    screen.run_steps(3);
    screen
}

fn painted_text(screen: &Harness<'_, VEditApp>, text: &str) -> Option<egui::Rect> {
    screen
        .output()
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::epaint::Shape::Text(shape) if shape.galley.text() == text => {
                Some(shape.galley.rect.translate(shape.pos.to_vec2()))
            }
            _ => None,
        })
}

#[test]
fn status_persistently_explains_save_state_and_controls_autosave_without_editing() {
    let mut screen = harness(fixture());
    let id = screen.state().tab().unwrap().id;
    let original = screen.state().tab().unwrap().text.clone();
    let saved = painted_text(&screen, "Saved").expect("save state visible without a tooltip");
    let off = screen.get_by_label("Auto save off").rect();
    assert!(
        saved.top() > screen.ctx.content_rect().bottom() - 48.0 && !saved.intersects(off),
        "save text={saved:?}, autosave={off:?}"
    );
    assert!(off.right() < 960.0);

    screen.get_by_label("Auto save off").click();
    screen.run_steps(3);
    assert!(screen.state().settings.autosave);
    assert!(screen.state().session_dirty);
    assert!(
        screen
            .ctx
            .memory(|memory| memory.has_focus(screen.state().document_widget_id(id)))
    );
    assert_eq!(screen.state().tab().unwrap().text, original);
    assert!(!screen.state().tab().unwrap().vim.can_undo());
    screen.get_by_label("Auto save on").click();
    screen.run_steps(3);
    assert!(!screen.state().settings.autosave);

    {
        let tab = screen.state_mut().tab_mut().unwrap();
        tab.text.push_str("pending\n");
        tab.changed();
    }
    screen.run_steps(2);
    assert!(painted_text(&screen, "Unsaved").is_some());
    screen.state_mut().tab_mut().unwrap().agent_unsaved = true;
    screen.run_steps(2);
    assert!(painted_text(&screen, "Agent draft").is_some());
    screen.get_by_label("Auto save off").click();
    screen.run_steps(2);
    assert!(screen.state().tab().unwrap().agent_unsaved);
    assert!(screen.state().tab().unwrap().dirty);
    assert!(painted_text(&screen, "Agent draft").is_some());
    assert!(screen.query_by_label_contains("Ln 1/").is_some());

    screen.state_mut().tab_mut().unwrap().saving = true;
    screen.run_steps(2);
    assert!(painted_text(&screen, "Saving…").is_some());
    screen.state_mut().tab_mut().unwrap().saving = false;
    screen.state_mut().tab_mut().unwrap().error = Some("File changed on disk".into());
    screen.run_steps(2);
    assert!(painted_text(&screen, "Save blocked").is_some());
    screen.state_mut().tab_mut().unwrap().loading = true;
    screen.run_steps(2);
    assert!(painted_text(&screen, "Loading…").is_some());
    let state = screen.state().session();
    assert!(state.settings.unwrap().autosave);
}

#[test]
fn file_conflict_banner_offers_save_as_without_reloading_unsaved_text() {
    let mut app = fixture();
    let tab = app.tab_mut().unwrap();
    tab.text.push_str("// preserved local work\n");
    tab.changed();
    tab.error = Some("This file changed outside vEdit.".into());
    let id = tab.id;
    let original = tab.text.clone();
    let revision = tab.revision;
    let mut screen = harness(app);
    assert!(
        screen
            .query_by_label("Your edits are kept. Save as… preserves them in another file.")
            .is_some()
    );
    // A pointer click on the disabled reload action must not dispatch a load.
    screen.get_by_label("Reload from disk").click();
    screen.run_steps(2);
    let tab = screen.state().tab().unwrap();
    assert!(!tab.loading);
    assert_eq!(tab.text, original);
    assert_eq!(tab.revision, revision);
    screen.get_by_label("Save as…").click();
    screen.run_steps(2);
    assert_eq!(screen.state().save_as, Some(id));
    assert_eq!(screen.state().tab().unwrap().text, original);
}

#[test]
fn colliding_file_tabs_are_distinguishable_and_recompute_in_each_split() {
    let mut app = fixture();
    app.sidebar_visible = false;
    let root = app.projects[0].root.clone();
    app.projects[0].tabs.clear();
    for relative in ["client/src/lib.rs", "server/src/lib.rs"] {
        let mut tab = EditorTab::new(Some(root.join(relative)), "lib.rs".into());
        tab.text = format!("// {relative}\n");
        tab.saved_text = tab.text.clone();
        app.projects[0].tabs.push(tab);
    }
    app.editor_layout = split::EditorLayout::default();
    app.layout_project = None;
    app.projects[0].layout = None;
    app.bind_current_pane();
    let mut screen = harness(app);
    for label in [r"lib.rs — client\src", r"lib.rs — server\src"] {
        let node = screen.get_by_label(label);
        assert!(node.rect().top() < 90.0);
        assert!(
            painted_text(&screen, label).is_some(),
            "label must be painted, not merely in accessibility metadata"
        );
    }
    screen.get_by_label(r"lib.rs — server\src").click();
    screen.run_steps(2);
    assert!(
        screen
            .state()
            .tab()
            .unwrap()
            .path
            .as_ref()
            .unwrap()
            .ends_with("server/src/lib.rs")
    );
    let right = screen
        .state_mut()
        .split_active(split::SplitOrientation::Vertical)
        .unwrap();
    screen.run_steps(3);
    let plain = screen.get_by_label("lib.rs").rect();
    let pane = screen
        .state()
        .tab_drag_pane_rects()
        .into_iter()
        .find(|(id, _)| *id == right)
        .unwrap()
        .1;
    assert!(
        pane.contains(plain.center()),
        "a single-file pane keeps the short basename"
    );
    assert!(screen.query_by_label(r"lib.rs — client\src").is_some());
    assert!(
        screen.state().projects[0]
            .tabs
            .iter()
            .all(|tab| tab.title == "lib.rs")
    );
}

#[test]
fn same_named_projects_have_distinct_visible_header_labels_after_switching() {
    let mut app = VEditApp::empty(egui::Context::default());
    app.settings.autosave = false;
    for parent in ["client", "server"] {
        let root = PathBuf::from(r"C:\work").join(parent).join("project");
        let mut project = Project::new(root.clone());
        project.index = Some(Arc::new(workspace::WorkspaceIndex::empty(root)));
        project.tabs.push(EditorTab::new(None, "Untitled".into()));
        app.projects.push(project);
    }
    app.bind_current_pane();
    let mut screen = harness(app);
    assert!(painted_text(&screen, "project — client").is_some());
    assert!(painted_text(&screen, "project — server").is_some());
    screen.state_mut().activate_project_layout(1);
    screen.run_steps(3);
    assert!(painted_text(&screen, "project — server").is_some());
    assert!(screen.query_by_label("Switch project").is_some());
}

#[test]
fn long_colliding_tab_names_keep_the_distinguishing_folder_inside_the_visible_tab() {
    let mut app = fixture();
    app.sidebar_visible = false;
    let root = app.projects[0].root.clone();
    let title = format!("{}.rs", "long_shared_filename_".repeat(8));
    for (tab, folder) in app.projects[0].tabs.iter_mut().zip(["client", "server"]) {
        tab.title = title.clone();
        tab.path = Some(root.join(folder).join(&title));
    }
    let screen = harness(app);
    for folder in ["client", "server"] {
        let label = format!("{title} — {folder}");
        let tab = screen.get_by_label(&label).rect();
        let suffix = painted_text(&screen, &format!(" — {folder}"))
            .expect("the parent must be painted separately from the truncated filename");
        assert!(tab.contains_rect(suffix));
        assert!(
            suffix.right() < tab.right() - 28.0,
            "keep the close button clear"
        );
    }
}

#[test]
fn windows_clipboard_newlines_are_normalized_for_native_vim_and_menu_paste() {
    for (vim, insert) in [(false, false), (true, false), (true, true)] {
        for context_menu in [false, true] {
            let mut app = fixture();
            app.sidebar_visible = false;
            app.settings.vim = vim;
            let tab = app.tab_mut().unwrap();
            tab.text.clear();
            tab.saved_text.clear();
            tab.line_ending = workspace::LineEnding::CrLf;
            tab.set_cursor(0..0);
            if insert {
                tab.vim.handle_key(&mut tab.text, VimKey::Char('i'));
            }
            let id = tab.id;
            app.sync_external_document_navigation(id);
            let mut screen = harness(app);
            if context_menu {
                screen
                    .state_mut()
                    .apply_editor_context_action(id, EditorAction::Paste);
            }
            screen.event(egui::Event::Paste("α🦀\r\nnext\rfinal\r\n".into()));
            screen.run_steps(3);
            let tab = screen.state().tab().unwrap();
            assert_eq!(
                tab.text, "α🦀\nnext\nfinal\n",
                "vim={vim}, insert={insert}, context={context_menu}"
            );
            assert_eq!(tab.line_ending, workspace::LineEnding::CrLf);
            assert_eq!(tab.selection.start, tab.text.chars().count());
            assert_eq!(tab.selection.start, tab.selection.end);
            assert!(tab.dirty);
        }
    }
}

fn explorer_row(screen: &Harness<'_, VEditApp>, name: &str) -> egui::Rect {
    screen
        .query_all_by_label(name)
        .find(|node| node.rect().top() > 90.0)
        .unwrap()
        .rect()
}

fn filled_selection(screen: &Harness<'_, VEditApp>, row: egui::Rect) -> bool {
    let visuals = screen.ctx.style_of(egui::Theme::Dark).visuals.clone();
    // Explorer deliberately softens the theme's selection fill. Keep testing
    // the exact filled row so the active file and keyboard selection remain
    // distinguishable from a mere hover or the thin active-file marker.
    let selected_fill = visuals
        .selection
        .bg_fill
        .lerp_to_gamma(visuals.panel_fill, 0.24);
    screen.output().shapes.iter().any(|shape| {
        matches!(&shape.shape,
        egui::epaint::Shape::Rect(rect) if rect.fill == selected_fill
            && rect.rect.contains(row.center()) && rect.rect.width() > row.width() * 0.8)
    })
}

#[test]
fn explorer_emphasizes_active_file_but_retains_its_keyboard_selection() {
    let mut screen = harness(fixture());
    let main = screen.state().projects[0].tabs[0].path.clone().unwrap();
    assert!(screen.state().explorer_selected(&main));
    screen.state_mut().activate_tab(0, 1);
    screen.run_steps(3);
    let readme = explorer_row(&screen, "README.md");
    assert!(filled_selection(&screen, readme));
    assert!(!filled_selection(&screen, explorer_row(&screen, "main.rs")));
    assert!(screen.state().explorer_selected(&main));

    screen.state_mut().focus_explorer();
    screen.run_steps(2);
    assert!(filled_selection(&screen, explorer_row(&screen, "main.rs")));
    assert!(!filled_selection(
        &screen,
        explorer_row(&screen, "README.md")
    ));
    screen.key_press(Key::ArrowUp);
    screen.run_steps(2);
    let folder = screen.state().project().unwrap().root.join("src");
    assert!(screen.state().explorer_selected(&folder));
    assert!(filled_selection(&screen, explorer_row(&screen, "src")));
    screen.state_mut().focus_editor();
    screen.run_steps(2);
    assert!(screen.state().explorer_selected(&folder));
    assert!(filled_selection(
        &screen,
        explorer_row(&screen, "README.md")
    ));
    assert!(!filled_selection(&screen, explorer_row(&screen, "src")));
}

#[test]
fn review_severity_is_readable_without_interpreting_colors() {
    let mut app = fixture();
    let tab = app.tab_mut().unwrap();
    tab.annotations = [
        crate::cli::AnnotationSeverity::Info,
        crate::cli::AnnotationSeverity::Warning,
        crate::cli::AnnotationSeverity::Critical,
    ]
    .into_iter()
    .enumerate()
    .map(|(index, severity)| crate::cli::Annotation {
        id: index.to_string(),
        start: crate::cli::Location {
            line: index + 1,
            column: 1,
        },
        end: None,
        note: format!("Review point {}", index + 1),
        severity,
    })
    .collect();
    app.review = Some(crate::cli::ReviewSummary {
        title: "Review".into(),
        note: String::new(),
        files: vec![tab.id],
    });
    app.set_sidebar_view_visible(SidebarView::Explorer, false);
    app.show_sidebar(SidebarView::Review);
    let screen = harness(app);
    for detail in ["Info · Line 1", "Warning · Line 2", "Critical · Line 3"] {
        assert!(painted_text(&screen, detail).is_some());
        assert!(screen.query_by_label_contains(detail).is_some());
    }
}

#[test]
fn markdown_scroll_tracks_are_visible_without_hover_in_actual_narrow_split() {
    let mut app = fixture();
    app.activate_tab(0, 1);
    let tab = app.tab_mut().unwrap();
    tab.text = "| Risk | Countermeasure | Done |\n| --- | --- | --- |\n| A compact risk. | A compact countermeasure. | H1 |\n\n".to_owned()
        + &"More reading below the table.\n\n".repeat(50);
    tab.saved_text = tab.text.clone();
    app.set_document_view(crate::cli::DocumentView::Preview);
    let preview_pane = app.current_pane_id();
    app.split_active(split::SplitOrientation::Vertical).unwrap();
    app.set_document_view(crate::cli::DocumentView::Source);
    let screen = harness(app);
    let pane = screen
        .state()
        .tab_drag_pane_rects()
        .into_iter()
        .find(|(id, _)| *id == preview_pane)
        .unwrap()
        .1;
    let tracks: Vec<_> = screen
        .output()
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Rect(rect) if rect.fill == BORDER && pane.contains(rect.rect.center()) => {
                Some(rect.rect)
            }
            _ => None,
        })
        .collect();
    assert!(
        tracks
            .iter()
            .any(|rect| (5.0..=7.0).contains(&rect.width()) && rect.height() > 300.0),
        "The real app preview needs a vertical reading track without hover: {tracks:?}; pane={pane:?}"
    );
    assert!(
        tracks
            .iter()
            .any(|rect| (5.0..=7.0).contains(&rect.height()) && rect.width() > 200.0),
        "The visible overflowing table needs a horizontal track without hover: {tracks:?}"
    );
}

#[test]
fn welcome_explains_how_to_type_and_find_vim_settings_at_minimum_height() {
    let screen = harness(VEditApp::empty(egui::Context::default()));
    for label in ["i / Esc", "v / V", "Ctrl+Comma"] {
        let rect = screen.get_by_label(label).rect();
        assert!(
            rect.top() > 0.0 && rect.bottom() < 610.0,
            "{label} must fit above status: {rect:?}"
        );
    }
}

#[test]
fn file_identity_and_agent_draft_screenshot() {
    let mut app = fixture();
    app.sidebar_visible = false;
    let root = app.projects[0].root.clone();
    app.projects[0].tabs[0].path = Some(root.join("client/src/lib.rs"));
    app.projects[0].tabs[0].title = "lib.rs".into();
    app.projects[0].tabs[1].path = Some(root.join("server/src/lib.rs"));
    app.projects[0].tabs[1].title = "lib.rs".into();
    app.projects[0].tabs[0]
        .text
        .push_str("\n// Unsaved agent suggestion\n");
    app.projects[0].tabs[0].changed();
    app.projects[0].tabs[0].agent_unsaved = true;
    let mut screen = visual_tests::harness(app);
    visual_tests::snapshot(&mut screen, "vedit_file_identity");
}

//! Review hierarchy and density with a representative tree, without disk or
//! background Explorer work. Native DPI and a narrow sidebar share the fixture.
use super::sidebar_layout::Side;
use super::*;
use egui_kittest::{Harness, kittest::Queryable};

const DOCUMENT: &str = r#"use crate::editor::Document;

/// Keep the anchor stable while the cursor moves.
#[derive(Clone, Copy, Debug, Default)]
pub struct Selection {
    pub anchor: usize,
    pub cursor: usize,
}

impl Selection {
    pub fn range(self) -> std::ops::Range<usize> {
        self.anchor.min(self.cursor)..self.anchor.max(self.cursor)
    }

    pub fn select_line(&mut self, document: &Document) {
        self.anchor = document.line_start(self.cursor);
        self.cursor = document.line_end(self.cursor);
    }
}
"#;

fn fixture(sidebar_width: f32) -> VEditApp {
    let root = PathBuf::from("C:/workspaces/vedit");
    let mut tree = workspace::ExplorerTree::new(root.clone());
    for (directory, children) in [
        (
            "",
            vec![
                (".github", true),
                ("assets", true),
                ("docs", true),
                ("src", true),
                ("target", true),
                ("tests", true),
                (".gitignore", false),
                ("Cargo.lock", false),
                ("Cargo.toml", false),
                ("LICENSE", false),
                ("README.md", false),
            ],
        ),
        (
            "src",
            vec![
                ("editor", true),
                ("language", true),
                ("platform", true),
                ("app.rs", false),
                ("main.rs", false),
            ],
        ),
        (
            "src/editor",
            vec![
                ("view", true),
                ("commands.rs", false),
                ("document.rs", false),
                ("history.rs", false),
                ("input.rs", false),
            ],
        ),
        (
            "src/editor/view",
            vec![
                ("selection.rs", false),
                ("selection_drag_unicode_and_wrapped_lines.rs", false),
                ("viewport.rs", false),
            ],
        ),
    ] {
        let path = root.join(directory);
        let entries = children
            .into_iter()
            .map(|(name, is_dir)| workspace::WorkspaceEntry {
                path: path.join(name),
                name: name.into(),
                is_dir,
                ignored: name == "target" || name == "Cargo.lock",
            })
            .collect();
        let generation = tree.begin(&path).expect("reserve fixture listing");
        assert!(tree.complete(
            &path,
            generation,
            Ok(workspace::ExplorerDirectory {
                root: root.clone(),
                path: path.clone(),
                entries,
                warnings: Vec::new(),
            })
        ));
    }
    let mut app = VEditApp::empty(egui::Context::default());
    app.settings.autosave = false;
    app.settings.smooth_scroll = false;
    let mut project = Project::new(root.clone());
    // Only inject the immutable snapshot. Do not attach the runtime tree, which
    // would schedule actual folder reads for this intentionally fictional root.
    project.index = Some(tree.snapshot());
    project.git_branch = Some("main".into());
    project.last_git_refresh = Instant::now();
    let mut tab = EditorTab::new(
        Some(root.join("src/editor/view/selection.rs")),
        "selection.rs".into(),
    );
    tab.text = DOCUMENT.into();
    tab.saved_text = DOCUMENT.into();
    project.tabs.push(tab);
    app.projects.push(project);
    app.bind_current_pane();
    app.restore_sidebars(
        Some(
            serde_json::from_value(serde_json::json!({
                "left_width": sidebar_width,
            }))
            .expect("visual sidebar layout"),
        ),
        None,
    );
    app
}

fn row_rect(screen: &Harness<'_, VEditApp>, label: &str) -> egui::Rect {
    screen
        .query_all_by_label(label)
        .max_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
        .unwrap_or_else(|| panic!("visible Explorer row {label}"))
        .rect()
}

fn assert_readable_hierarchy(screen: &Harness<'_, VEditApp>) {
    let sidebar = screen
        .state()
        .sidebar_region_rect(sidebar_layout::Side::Left)
        .expect("visible Explorer sidebar");
    let root_row = row_rect(screen, "src");
    for label in [
        "editor",
        "view",
        "selection.rs",
        "commands.rs",
        "Cargo.toml",
    ] {
        let row = row_rect(screen, label);
        assert!(
            sidebar.contains_rect(row),
            "{label} must remain inside its sidebar: {row:?}"
        );
        assert!(
            (row.left() - root_row.left()).abs() < 0.1,
            "selection and hover must span every depth equally: {label}"
        );
        assert!(
            (row.right() - root_row.right()).abs() < 0.1,
            "every depth must share the sidebar's right edge: {label}"
        );
        assert!(
            (row.height() - design::EXPLORER_ROW_HEIGHT).abs() < 0.1,
            "painting and virtual scrolling must share row geometry: {label}"
        );
    }
    assert!(screen.query_by_label("Show ignored files").is_none());
    assert!(
        screen
            .query_by_label_contains("Index limit reached")
            .is_none()
    );
    assert!(
        screen.query_by_label("target").is_some(),
        "generated directories belong in the Explorer"
    );
}

#[test]
fn explorer_selection_stays_on_the_row_without_an_accent_bar() {
    for side in [Side::Left, Side::Right] {
        let mut app = fixture(292.0);
        app.move_sidebar(SidebarView::Explorer, side);
        let root = app.projects[0].root.clone();
        let mut screen = visual_tests::harness(app);
        visual_tests::settle_render(&mut screen);
        for (label, relative) in [
            ("selection.rs", "src/editor/view/selection.rs"),
            ("assets", "assets"),
        ] {
            screen
                .query_all_by_label(label)
                .max_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
                .unwrap()
                .click();
            visual_tests::settle_render(&mut screen);
            assert!(screen.state().explorer_keyboard_focused());
            assert!(screen.state().explorer_selected(&root.join(relative)));
            let row = row_rect(&screen, label);
            let sidebar = screen.state().sidebar_region_rect(side).unwrap();
            let mut selected_fill = false;
            for shape in &screen.output().shapes {
                if let egui::Shape::Rect(rect) = &shape.shape {
                    assert!(
                        rect.stroke.color != ACCENT || rect.rect.height() < sidebar.height() * 0.9,
                        "Explorer focus must not outline the entire sidebar"
                    );
                    if row.contains_rect(rect.rect) && rect.fill.a() > 0 {
                        assert!(
                            rect.rect.width() > 3.0 || rect.rect.height() <= 6.0,
                            "Explorer selection must not have a narrow accent bar"
                        );
                        selected_fill |= rect.rect.width() > row.width() * 0.8;
                    }
                }
            }
            assert!(selected_fill, "the selected row must remain visible");
        }
    }
}

#[test]
fn explorer_tooltip_stays_beside_the_pointer_and_inside_the_window() {
    for side in [Side::Left, Side::Right] {
        let mut app = fixture(292.0);
        app.move_sidebar(SidebarView::Explorer, side);
        app.settings.vim = side == Side::Right;
        let tooltip = crate::paths::display(&app.projects[0].root.join("assets"));
        let mut screen = visual_tests::harness(app);
        visual_tests::settle_render(&mut screen);
        screen.ctx.all_styles_mut(|style| {
            style.interaction.tooltip_delay = 0.0;
            style.interaction.tooltip_grace_time = 0.0;
            style.interaction.show_tooltips_only_when_still = false;
        });
        let pointer = row_rect(&screen, "assets").center();
        screen.hover_at(pointer);
        screen.run_steps(4);
        assert!(screen.query_by_label("Copy all").is_none());
        let tooltip_rect = screen.get_by_label(&tooltip).rect();
        assert!(tooltip_rect.y_range().contains(pointer.y));
        match side {
            Side::Left => assert!(tooltip_rect.left() > pointer.x + 8.0),
            Side::Right => assert!(tooltip_rect.right() < pointer.x - 8.0),
        }
        assert!(screen.ctx.content_rect().contains_rect(tooltip_rect));
        // Pointer input passes through the hint, even where it covers the same row.
        assert!(
            screen
                .ctx
                .layer_id_at(tooltip_rect.center())
                .is_none_or(|layer| layer.order != egui::Order::Tooltip)
        );
        screen.hover_at(egui::pos2(-20.0, -20.0));
        screen.run_steps(3);
        assert!(screen.query_by_label(&tooltip).is_none());
    }
}

#[test]
fn explorer_preview_double_click_keeps_the_file_from_tree_or_tab() {
    for tree in [true, false] {
        let mut app = fixture(292.0);
        app.tab_mut().unwrap().preview_tab = true;
        let mut screen = visual_tests::harness(app);
        visual_tests::settle_render(&mut screen);
        let pointer = if tree {
            row_rect(&screen, "selection.rs").center()
        } else {
            screen
                .get_all_by_label("selection.rs")
                .find(|node| node.rect().height() > design::EXPLORER_ROW_HEIGHT)
                .unwrap()
                .rect()
                .center()
        };
        screen.hover_at(pointer);
        screen.run_steps(1);
        let started = screen.ctx.input(|input| input.time);
        for click in 0..2 {
            // The snapshot harness advances 250 ms per queued event. Supply a
            // real double-click interval, with press/release in one input frame.
            screen.input_mut().time = Some(started + 0.1 * (click + 1) as f64);
            for pressed in [true, false] {
                screen.input_mut().events.push(egui::Event::PointerButton {
                    pos: pointer,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::default(),
                });
            }
            screen.step();
            assert_eq!(
                screen.state().tab().unwrap().preview_tab,
                click == 0,
                "only the double-click keeps the file (tree={tree})"
            );
        }
    }
}

#[test]
fn explorer_preview_tab_and_option_screenshots() {
    let mut app = fixture(292.0);
    app.tab_mut().unwrap().preview_tab = true;
    let mut screen = visual_tests::harness(app);
    visual_tests::snapshot(&mut screen, "vedit_explorer_preview_tab");
    screen.state_mut().open_settings_section("Layout");
    screen.run_steps(3);
    visual_tests::snapshot(&mut screen, "vedit_explorer_preview_settings");
    screen.get_by_label("Preview files from Explorer").click();
    screen.run_steps(2);
    assert!(!screen.state().settings.explorer_preview_tabs);
    assert!(!screen.state().tab().unwrap().preview_tab);
}

#[test]
fn dense_explorer_hierarchy_screenshots() {
    let mut screen = visual_tests::harness(fixture(292.0));
    visual_tests::snapshot(&mut screen, "vedit_dense_explorer");
    assert_readable_hierarchy(&screen);

    // Windows native DPI is independent of the editor's own zoom preference.
    screen
        .input_mut()
        .viewports
        .get_mut(&egui::ViewportId::ROOT)
        .expect("root viewport")
        .native_pixels_per_point = Some(1.25);
    visual_tests::snapshot(&mut screen, "vedit_dense_explorer_125");
    assert!((screen.ctx.pixels_per_point() - 1.25).abs() < 0.001);
    assert_readable_hierarchy(&screen);

    let mut compact = visual_tests::harness(fixture(200.0));
    compact.set_size(Vec2::new(840.0, 840.0));
    visual_tests::snapshot(&mut compact, "vedit_dense_explorer_compact");
    assert_readable_hierarchy(&compact);

    screen
        .input_mut()
        .viewports
        .get_mut(&egui::ViewportId::ROOT)
        .unwrap()
        .native_pixels_per_point = Some(1.0);
    screen.get_by_label("assets").click();
    visual_tests::snapshot(&mut screen, "vedit_explorer_selection");

    // The snapshot runner requires one aggregate per test, even when multiple
    // independent window-size fixtures are intentionally compared.
    let mut results = screen.take_snapshot_results();
    results.extend(compact.take_snapshot_results());
}

#[test]
fn explorer_colors_changed_files_and_marks_their_folders() {
    let mut app = fixture(292.0);
    let project = &mut app.projects[0];
    project.git_root = Some(project.root.clone());
    let change = |path: &str, index, worktree| git::FileStatus {
        path: path.into(),
        original_path: None,
        index,
        worktree,
    };
    project.git_changes = vec![
        change(
            "src/editor/document.rs",
            git::FileChange::None,
            git::FileChange::Modified,
        ),
        change(
            "src/editor/view/viewport.rs",
            git::FileChange::None,
            git::FileChange::Untracked,
        ),
        change(
            "docs/guide.md",
            git::FileChange::Added,
            git::FileChange::None,
        ),
        change(
            "Cargo.toml",
            git::FileChange::Modified,
            git::FileChange::None,
        ),
        change(
            "tests/smoke.rs",
            git::FileChange::Unmerged,
            git::FileChange::Unmerged,
        ),
    ];
    let mut screen = visual_tests::harness(app);
    visual_tests::snapshot(&mut screen, "vedit_explorer_git_status");
    assert_readable_hierarchy(&screen);
    let marks = screen.state().projects[0]
        .explorer_git
        .as_ref()
        .expect("Explorer derives Git marks while painting");
    assert_eq!(
        marks.get(Path::new("src"), true),
        Some(explorer_git::ExplorerGitMark::Modified)
    );
    assert_eq!(
        marks.get(Path::new("tests"), true),
        Some(explorer_git::ExplorerGitMark::Conflicted)
    );
    assert!(marks.get(Path::new("README.md"), false).is_none());
}

use super::*;
use egui_kittest::{Harness, kittest::Queryable};

fn fixture() -> MorrowApp {
    let mut app = MorrowApp::empty(egui::Context::default());
    app.sidebar = SidebarView::SourceControl;
    let mut project = Project::new(PathBuf::from("C:/morrow-git-render-only"));
    project.git_root = Some(project.root.clone());
    project.git_branch = Some("main".into());
    project.git_view.section = GitSection::History;
    project.last_git_refresh = Instant::now() + Duration::from_secs(3600);
    for (index, references) in [
        Vec::new(),
        vec![("main", git::RefKind::Local)],
        vec![
            ("origin/topic", git::RefKind::Remote),
            ("v0.4.5", git::RefKind::Tag),
        ],
        vec![
            (
                "feature/Überarbeitung-der-sehr-langen-Branch-Beschreibung",
                git::RefKind::Local,
            ),
            ("origin/feature/Überarbeitung", git::RefKind::Remote),
            ("release", git::RefKind::Local),
            ("v0.4.5-rc1", git::RefKind::Tag),
        ],
        Vec::new(),
    ]
    .into_iter()
    .enumerate()
    {
        let hash = format!("{:040x}", index + 1);
        let parents = if index == 0 {
            vec![format!("{:040x}", 2), format!("{:040x}", 3)]
        } else if index < 3 {
            vec![format!("{:040x}", 4)]
        } else if index == 3 {
            vec![format!("{:040x}", 5)]
        } else {
            Vec::new()
        };
        project.recent_commits.push(git::CommitSummary {
            hash: hash.clone(),
            parents,
            refs: references
                .into_iter()
                .map(|(name, kind)| git::CommitRef {
                    hash: hash.clone(),
                    name: name.into(),
                    kind,
                    current: name == "main",
                })
                .collect(),
            author: "Alex Überarbeitung".into(),
            date: "2026-09-26".into(),
            subject: format!(
                "Commit {index}: readable history with a long Unicode subject Überarbeitung"
            ),
        });
        project.git_view.detail_cache.insert(
            hash.clone(),
            git::CommitDetails {
                hash,
                parents: Vec::new(),
                author: "Alex".into(),
                author_email: "alex@example.invalid".into(),
                date: "2026-09-26T00:00:00Z".into(),
                message: "Cached render-only commit".into(),
                files: Vec::new(),
            },
        );
    }
    app.projects.push(project);
    app
}

fn harness(app: MorrowApp, width: f32, full_sidebar: bool) -> Harness<'static, MorrowApp> {
    let screen = Harness::builder()
        .with_size([width, 600.0])
        .with_os(egui::os::OperatingSystem::Windows)
        .build_ui_state(
            move |ui, app: &mut MorrowApp| {
                app.ctx = ui.ctx().clone();
                if full_sidebar {
                    app.sidebar_ui(ui);
                } else {
                    app.git_ui(ui);
                }
            },
            app,
        );
    set_style(&screen.ctx);
    screen.ctx.all_styles_mut(|style| {
        style.animation_time = 0.0;
        style.scroll_animation = egui::style::ScrollAnimation::none();
    });
    screen
}

fn row_rects(screen: &Harness<'static, MorrowApp>) -> Vec<egui::Rect> {
    screen.state().projects[0]
        .recent_commits
        .iter()
        .map(|commit| screen.get_by_label(&commit.subject).rect())
        .collect()
}

#[test]
fn compact_history_keeps_all_ref_variants_in_two_fixed_lines() {
    for width in [220.0, 360.0] {
        let mut screen = harness(fixture(), width, false);
        screen.run_steps(3);
        let rows = row_rects(&screen);
        for row in &rows {
            assert_eq!(row.height(), 46.0, "refs must not reserve a third line");
            for shape in &screen.output().shapes {
                if let egui::Shape::Text(text) = &shape.shape {
                    // Inline Labels use centered galleys: their position is an
                    // anchor, not the painted rectangle's left edge.
                    let text_rect = text.visual_bounding_rect();
                    if row.contains(text_rect.center()) {
                        assert!(text_rect.left() >= row.left() + 24.0);
                        assert!(
                            text_rect.right() <= row.right() + 0.5,
                            "width={width}, text={:?}, text_rect={text_rect:?}, row={row:?}, clip={:?}",
                            text.galley.text(),
                            shape.clip_rect
                        );
                        assert!(text_rect.top() >= row.top());
                        assert!(text_rect.bottom() <= row.bottom());
                    }
                }
            }
        }
        for pair in rows.windows(2) {
            assert!((pair[1].top() - pair[0].bottom()).abs() < 0.5);
        }
        let branch = screen.get_by_label("HEAD · main").rect();
        assert!(rows[1].contains_rect(branch));
        assert!(branch.top() >= rows[1].top() + 24.0);
        assert!(
            screen.query_all_by_label_contains("+").next().is_some(),
            "omitted refs need a visible count"
        );
    }
}

#[test]
fn compact_graph_edges_meet_at_each_row_boundary() {
    let mut screen = harness(fixture(), 360.0, false);
    screen.run_steps(3);
    let rows = row_rects(&screen);
    for pair in rows.windows(2) {
        let boundary = pair[0].bottom();
        let mut below = Vec::new();
        let mut above = Vec::new();
        for shape in &screen.output().shapes {
            if let egui::Shape::CubicBezier(edge) = &shape.shape {
                let points = edge.points;
                let egui::epaint::ColorMode::Solid(color) = &edge.stroke.color else {
                    continue;
                };
                if (points[3].y - boundary).abs() < 0.5 {
                    above.push((points[3].x.to_bits(), *color));
                }
                if (points[0].y - boundary).abs() < 0.5 {
                    below.push((points[0].x.to_bits(), *color));
                }
            }
        }
        above.sort_by_key(|(x, color)| (*x, color.to_array()));
        above.dedup();
        below.sort_by_key(|(x, color)| (*x, color.to_array()));
        below.dedup();
        assert!(!above.is_empty());
        assert_eq!(
            above, below,
            "shorter rows must keep branch edges continuous"
        );
    }
}

#[test]
fn graph_subject_metadata_and_inline_ref_all_select_the_same_commit() {
    let mut screen = harness(fixture(), 360.0, false);
    screen.run_steps(3);
    let subject = screen.state().projects[0].recent_commits[1].subject.clone();
    let hash = screen.state().projects[0].recent_commits[1].hash.clone();
    let row = screen.get_by_label(&subject).rect();
    let chip = screen.get_by_label("HEAD · main").rect().center();
    for point in [
        egui::pos2(row.left() + 9.0, row.center().y),
        egui::pos2(row.left() + 45.0, row.top() + 11.0),
        egui::pos2(row.left() + 45.0, row.top() + 33.0),
        chip,
    ] {
        screen.state_mut().projects[0].git_view.selected_commit = None;
        screen
            .input_mut()
            .events
            .push(egui::Event::PointerMoved(point));
        for pressed in [true, false] {
            screen.input_mut().events.push(egui::Event::PointerButton {
                pos: point,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        screen.run_steps(2);
        assert_eq!(
            screen.state().projects[0].git_view.selected_commit.as_ref(),
            Some(&hash),
            "point={point:?}, row={row:?}, chip={chip:?}"
        );
        assert!(!screen.state().projects[0].git_view.detail_loading);
    }
}

#[test]
fn source_control_refresh_indicator_does_not_move_header_or_branch_controls() {
    let mut app = fixture();
    let branch = "feature/header-Überarbeitung-der-sehr-langen-Branch-Beschreibung";
    app.projects[0].git_branch = Some(branch.into());
    let mut screen = harness(app, 1000.0, true);
    screen.run_steps(3);
    let idle = [
        screen.get_by_label("SOURCE CONTROL").rect(),
        screen.get_by_label(branch).rect(),
        screen.get_by_label("···").rect(),
    ];
    screen.state_mut().projects[0].git_loading = true;
    screen.run_steps(3);
    assert_eq!(
        idle,
        [
            screen.get_by_label("SOURCE CONTROL").rect(),
            screen.get_by_label(branch).rect(),
            screen.get_by_label("···").rect()
        ]
    );
    let spinner = screen.get_by_label("Refreshing repository").rect();
    assert_eq!(spinner.size(), Vec2::splat(14.0));
    assert!(spinner.left() > idle[0].right());
    screen.state_mut().projects[0].git_loading = false;
    screen.run_steps(3);
    assert_eq!(
        idle,
        [
            screen.get_by_label("SOURCE CONTROL").rect(),
            screen.get_by_label(branch).rect(),
            screen.get_by_label("···").rect()
        ]
    );
    assert!(screen.query_by_label("Refreshing repository").is_none());
}

#[test]
fn segmented_sections_have_a_visible_active_marker_and_support_keyboard_switching() {
    let mut app = fixture();
    app.projects[0].git_view.section = GitSection::Changes;
    let mut screen = harness(app, 360.0, false);
    screen.run_steps(3);
    let changes = screen.get_by_label("Changes  0").rect();
    let history = screen.get_by_label("History").rect();
    assert_eq!(changes.size(), history.size());
    let active_marker = |screen: &Harness<'static, MorrowApp>, rect: egui::Rect| {
        screen.output().shapes.iter().any(|shape| {
            matches!(&shape.shape,
            egui::Shape::LineSegment { points, stroke }
                if stroke.color == ACCENT && rect.contains(points[0]) && rect.contains(points[1]))
        })
    };
    assert!(active_marker(&screen, changes));
    assert!(!active_marker(&screen, history));
    screen.get_by_label("History").hover();
    screen.run_steps(1);
    assert_eq!(
        screen.output().platform_output.cursor_icon,
        egui::CursorIcon::PointingHand
    );
    screen.get_by_label("History").focus();
    screen.run_steps(1);
    screen.key_press(Key::Enter);
    screen.run_steps(2);
    assert_eq!(
        screen.state().projects[0].git_view.section,
        GitSection::History
    );
    assert!(active_marker(&screen, history));
    assert!(!active_marker(&screen, changes));
    screen.get_by_label("Changes  0").click();
    screen.run_steps(2);
    assert_eq!(
        screen.state().projects[0].git_view.section,
        GitSection::Changes
    );
}

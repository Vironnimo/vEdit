use super::*;
use egui_kittest::{Harness, kittest::Queryable};

fn fixture() -> VEditApp {
    let mut app = VEditApp::empty(egui::Context::default());
    app.set_sidebar_view_visible(SidebarView::Explorer, false);
    app.move_sidebar(SidebarView::SourceControl, sidebar_layout::Side::Left);
    let mut project = Project::new(PathBuf::from("C:/vedit-git-render-only"));
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
            timestamp: 0,
            on_remote: false,
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

fn harness(app: VEditApp, width: f32, full_sidebar: bool) -> Harness<'static, VEditApp> {
    let screen = Harness::builder()
        .with_size([width, 800.0])
        .with_os(egui::os::OperatingSystem::Windows)
        .build_ui_state(
            move |ui, app: &mut VEditApp| {
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

fn row_rects(screen: &Harness<'static, VEditApp>) -> Vec<egui::Rect> {
    screen.state().projects[0]
        .recent_commits
        .iter()
        .filter(|commit| screen.output().shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text().starts_with(&commit.subject))))
        .filter_map(|commit| screen.query_by_label(&commit.subject).map(|row| row.rect()))
        .collect()
}

#[test]
fn git_explains_unsaved_documents_and_save_action_preserves_agent_draft_until_saved() {
    let mut app = fixture();
    app.projects[0].git_view.section = GitSection::Changes;
    let mut draft = EditorTab::new(None, "Untitled".into());
    draft.text = "unsaved agent work".into();
    draft.changed();
    draft.agent_unsaved = true;
    let id = draft.id;
    app.projects[0].tabs.push(draft);
    let mut screen = harness(app, 280.0, false);
    screen.run_steps(3);
    assert!(
        screen
            .query_by_label("1 unsaved document. Git uses files on disk.")
            .is_some()
    );
    assert!(screen.query_by_label("Working tree is clean").is_none());
    assert!(screen.query_by_label("No changes on disk").is_some());
    screen.get_by_label("Save all files").click();
    screen.run_steps(2);
    assert_eq!(screen.state().save_as, Some(id));
    let draft = &screen.state().projects[0].tabs[0];
    assert_eq!(draft.text, "unsaved agent work");
    assert!(draft.agent_unsaved && draft.dirty);
    assert!(
        screen.state().git_busy.is_none(),
        "Saving never stages or commits automatically"
    );
}

#[test]
fn git_unsaved_count_includes_other_open_folders_of_the_repository() {
    let mut app = fixture();
    let mut peer = Project::new(PathBuf::from("C:/vedit-git-render-only/sub"));
    for (path, dirty, saving) in [
        ("C:/vedit-git-render-only/sub/a.rs", true, false),
        ("C:/vedit-git-render-only/sub/b.rs", false, true),
        ("C:/vedit-git-render-only/sub/clean.rs", false, false),
        ("C:/vedit-git-render-only-other/no.rs", true, false),
    ] {
        let mut tab = EditorTab::new(Some(PathBuf::from(path)), "file".into());
        tab.dirty = dirty;
        tab.saving = saving;
        peer.tabs.push(tab);
    }
    app.projects.push(peer);
    assert_eq!(app.git_unsaved_document_count(), 2);
}

#[test]
fn history_keeps_subject_author_and_colored_branch_metadata_on_separate_lines() {
    for width in [220.0, 360.0] {
        let mut screen = harness(fixture(), width, false);
        screen.run_steps(3);
        let rows = row_rects(&screen);
        for (row, commit) in rows.iter().zip(&screen.state().projects[0].recent_commits) {
            assert_eq!(row.height(), 44.0);
            let history_clip = screen
                .output()
                .shapes
                .iter()
                .find_map(|shape| {
                    let egui::Shape::Text(text) = &shape.shape else {
                        return None;
                    };
                    text.galley
                        .text()
                        .starts_with(&commit.subject)
                        .then_some(shape.clip_rect)
                })
                .unwrap();
            let texts: Vec<_> = screen
                .output()
                .shapes
                .iter()
                .filter_map(|shape| {
                    let egui::Shape::Text(text) = &shape.shape else {
                        return None;
                    };
                    let center_y = text.pos.y + text.galley.size().y * 0.5;
                    // Virtual rows can extend under the separately clipped footer.
                    (shape.clip_rect == history_clip
                        && center_y >= row.top()
                        && center_y < row.bottom())
                    .then_some((text, shape.clip_rect))
                })
                .collect();
            assert_eq!(
                texts.len(),
                2,
                "subject and branch metadata have separate lines: {:?}",
                texts
                    .iter()
                    .map(|(text, _)| text.galley.text())
                    .collect::<Vec<_>>()
            );
            assert!(texts[1].0.galley.text().starts_with("Local  "));
            for reference in &commit.refs {
                assert!(texts[1].0.galley.text().contains(&reference.name));
            }
            let (text, clip) = texts[0];
            assert_eq!(
                text.galley.text(),
                format!("{}  {}", commit.subject, commit.author)
            );
            assert_eq!(text.galley.rows.len(), 1);
            assert_eq!(text.galley.job.sections[0].format.color, TEXT);
            assert_eq!(text.galley.job.sections[1].format.color, MUTED);
            assert!(text.pos.x >= row.left() + 24.0);
            assert!(text.pos.y >= row.top());
            assert!(text.pos.y + text.galley.size().y <= row.bottom());
            assert!(clip.right() <= row.right() + 0.5);
            let author_glyph = &text.galley.rows[0].glyphs[commit.subject.chars().count() + 2];
            assert!(
                text.pos.x + author_glyph.pos.x > clip.right(),
                "long subjects must use the row width and push the author outside its clip"
            );
        }
        for pair in rows.windows(2) {
            assert!((pair[1].top() - pair[0].bottom()).abs() < 0.5);
        }
        assert!(screen.query_by_label("HEAD · main").is_none());
        assert!(screen.query_by_label("origin/topic").is_none());
        assert!(screen.query_by_label("v0.4.5").is_none());
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
fn graph_subject_author_and_row_background_select_the_same_commit() {
    let mut app = fixture();
    app.projects[0].recent_commits[1].subject = "Short subject".into();
    let mut screen = harness(app, 360.0, false);
    screen.run_steps(3);
    let subject = screen.state().projects[0].recent_commits[1].subject.clone();
    let hash = screen.state().projects[0].recent_commits[1].hash.clone();
    let row = screen.get_by_label(&subject).rect();
    let author_x = screen
        .output()
        .shapes
        .iter()
        .find_map(|shape| {
            let egui::Shape::Text(text) = &shape.shape else {
                return None;
            };
            text.galley.text().starts_with(&subject).then(|| {
                text.pos.x
                    + text.galley.rows[0].glyphs[subject.chars().count() + 2]
                        .pos
                        .x
                    + 2.0
            })
        })
        .expect("the short subject leaves space for its author");
    assert!(author_x < row.right());
    for point in [
        egui::pos2(row.left() + 9.0, row.center().y),
        egui::pos2(row.left() + 70.0, row.center().y),
        egui::pos2(author_x, row.center().y),
        egui::pos2(row.right() - 8.0, row.center().y),
    ] {
        screen.state_mut().projects[0].git_view.selected_commit = None;
        screen.state_mut().projects[0].git_view.detail_open = false;
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
            "point={point:?}, row={row:?}"
        );
        assert!(!screen.state().projects[0].git_view.detail_loading);
    }
}

#[test]
fn background_repository_refresh_does_not_keep_repainting_the_window() {
    let mut app = fixture();
    app.projects[0].git_loading = true;
    let mut screen = harness(app, 1000.0, true);
    screen.run_steps(4);
    assert!(screen.query_by_label("Refreshing repository").is_some());
    let settled = screen
        .try_run()
        .expect("automatic Git refresh must not animate the entire editor while waiting");
    assert!(settled <= 4);
    assert!(screen.state().projects[0].git_loading);
}

#[test]
fn background_refresh_of_empty_history_also_settles() {
    let mut app = fixture();
    app.projects[0].git_loading = true;
    app.projects[0].recent_commits.clear();
    let mut screen = harness(app, 1000.0, true);
    screen.run_steps(4);
    assert!(screen.query_by_label("Loading commits…").is_some());
    screen
        .try_run()
        .expect("a repository without commits must not animate on each automatic refresh");
    assert!(screen.state().projects[0].git_loading);
}

#[test]
fn source_control_refresh_indicator_does_not_move_header_or_branch_controls() {
    let mut app = fixture();
    let branch = "feature/header-Überarbeitung-der-sehr-langen-Branch-Beschreibung";
    app.projects[0].git_branch = Some(branch.into());
    let mut screen = harness(app, 1000.0, true);
    screen.run_steps(3);
    let idle = [
        screen.get_by_label("Source control panel title").rect(),
        screen.get_by_label(&format!("{branch} ▾")).rect(),
        screen.get_by_label("···").rect(),
    ];
    screen.state_mut().projects[0].git_loading = true;
    screen.run_steps(3);
    assert_eq!(
        idle,
        [
            screen.get_by_label("Source control panel title").rect(),
            screen.get_by_label(&format!("{branch} ▾")).rect(),
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
            screen.get_by_label("Source control panel title").rect(),
            screen.get_by_label(&format!("{branch} ▾")).rect(),
            screen.get_by_label("···").rect()
        ]
    );
    assert!(screen.query_by_label("Refreshing repository").is_none());
}

#[test]
fn stacked_git_regions_resize_and_keep_both_contents_visible() {
    let mut app = fixture();
    app.projects[0].git_view.section = GitSection::Changes;
    let mut screen = harness(app, 360.0, false);
    screen.run_steps(3);
    assert!(
        screen.get_by_label("Changes  0").rect().bottom()
            < screen.get_by_label("History").rect().top()
    );
    assert!(screen.query_by_label("Working tree is clean").is_some());
    assert!(screen.query_by_label_contains("Commit 0:").is_some());
    let before = screen.get_by_label("History").rect().top();
    let start = screen
        .get_by_label("Resize Changes and History")
        .rect()
        .center();
    screen.event(egui::Event::PointerMoved(start));
    screen.event(egui::Event::PointerButton {
        pos: start,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    screen.run_steps(1);
    let end = start - egui::vec2(0.0, 80.0);
    screen.event(egui::Event::PointerMoved(end));
    screen.run_steps(2);
    screen.event(egui::Event::PointerButton {
        pos: end,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    screen.run_steps(3);
    assert!(screen.get_by_label("History").rect().top() < before - 40.0);
    assert!(
        screen.state().session().projects[0]
            .git_split_ratio
            .unwrap()
            < 0.5
    );
    screen.get_by_label("History").focus();
    screen.run_steps(1);
    screen.key_press(Key::Enter);
    screen.run_steps(2);
    assert_eq!(
        screen.state().projects[0].git_view.section,
        GitSection::History
    );
    assert!(screen.query_by_label("Working tree is clean").is_some());
    screen.get_by_label("Changes  0").click();
    screen.run_steps(2);
    assert_eq!(
        screen.state().projects[0].git_view.section,
        GitSection::Changes
    );
    assert!(screen.query_by_label_contains("Commit 0:").is_some());
}

fn keyboard_changes_fixture() -> VEditApp {
    let mut app = fixture();
    let project = &mut app.projects[0];
    project.git_view.section = GitSection::Changes;
    project.git_changes = ["src/first.py", "src/second.dart", "README.md"]
        .into_iter()
        .map(|path| git::FileStatus {
            path: path.into(),
            original_path: None,
            index: git::FileChange::None,
            worktree: git::FileChange::Modified,
        })
        .collect();
    app
}

#[test]
fn git_keyboard_moves_rows_expands_folders_and_opens_diff_without_staging() {
    let mut screen = harness(keyboard_changes_fixture(), 360.0, false);
    screen.run_steps(3);
    screen.state_mut().focus_git_content();
    screen.run_steps(2);
    let root = screen.state().projects[0].root.clone();
    assert_eq!(
        screen.ctx.memory(|m| m.focused()),
        Some(git_row_id(&root, "working:changes"))
    );
    screen.key_press(Key::ArrowDown);
    screen.run_steps(2);
    assert_eq!(
        screen.ctx.memory(|m| m.focused()),
        Some(git_row_id(&root, "working:changes:folder:src"))
    );
    screen.key_press(Key::ArrowLeft);
    screen.run_steps(2);
    assert!(!screen.state().projects[0].git_view.expanded["working:changes:folder:src"]);
    screen.key_press(Key::ArrowDown);
    screen.run_steps(2);
    assert_eq!(
        screen.ctx.memory(|m| m.focused()),
        Some(git_row_id(&root, "working:changes:file:README.md"))
    );
    screen.key_press(Key::ArrowUp);
    screen.run_steps(2);
    screen.key_press(Key::ArrowRight);
    screen.run_steps(2);
    screen.key_press(Key::ArrowDown);
    screen.run_steps(2);
    assert_eq!(
        screen.ctx.memory(|m| m.focused()),
        Some(git_row_id(&root, "working:changes:file:src/first.py"))
    );
    screen.key_press(Key::Enter);
    screen.run_steps(2);
    assert_eq!(
        screen.state().projects[0].git_view.selected_file.as_deref(),
        Some("working:changes:file:src/first.py")
    );
    assert!(screen.state().diff.is_some());
    assert!(
        screen.state().git_busy.is_none(),
        "Opening a diff must never stage changes"
    );
}

#[test]
fn git_keyboard_tab_reaches_stage_action_and_preserves_next_row_after_refresh() {
    let mut screen = harness(keyboard_changes_fixture(), 360.0, false);
    screen.run_steps(3);
    let root = screen.state().projects[0].root.clone();
    let first = git_row_id(&root, "working:changes:file:src/first.py");
    screen.ctx.memory_mut(|m| m.request_focus(first));
    screen.run_steps(2);
    screen.key_press(Key::Tab);
    screen.run_steps(2);
    assert_eq!(
        screen.ctx.memory(|m| m.focused()),
        Some(first.with("action"))
    );
    screen.key_press(Key::Enter);
    screen.run_steps(2);
    assert!(
        screen.state().git_busy.is_some(),
        "Enter on the labeled Stage control stages deliberately"
    );
    let app = screen.state_mut();
    app.git_busy = None;
    let project = &mut app.projects[0];
    let mut files = project.git_changes.clone();
    files.remove(0);
    project.git_status_request = Some(u64::MAX);
    let snapshot = GitSnapshot {
        root: project.git_root.clone(),
        parent_root: None,
        branch: project.git_branch.clone(),
        tracking: project.git_tracking.clone(),
        main_comparison: project.git_main_comparison.clone(),
        files,
        stats: project.git_stats.clone(),
        stats_error: None,
        history: Ok(GitHistoryRefresh {
            snapshot: project.git_history_snapshot.clone().unwrap_or_default(),
            page: None,
        }),
    };
    app.sender
        .send(WorkerResult::GitStatus {
            root: root.clone(),
            request_id: u64::MAX,
            result: Ok(Box::new(snapshot)),
        })
        .unwrap();
    app.poll();
    screen.run_steps(3);
    assert_eq!(
        screen.ctx.memory(|m| m.focused()),
        Some(git_row_id(&root, "working:changes:file:src/second.dart"))
    );
}

#[test]
fn git_keyboard_leaves_composer_arrows_and_enter_for_text_entry() {
    let mut app = keyboard_changes_fixture();
    app.projects[0].commit_message = "First line\nSecond line".into();
    let mut screen = harness(app, 360.0, false);
    screen.run_steps(3);
    let composer = git_row_id(&screen.state().projects[0].root, "commit-message");
    screen.ctx.memory_mut(|m| m.request_focus(composer));
    screen.run_steps(2);
    for key in [
        Key::End,
        Key::Home,
        Key::ArrowUp,
        Key::ArrowDown,
        Key::ArrowLeft,
        Key::ArrowRight,
        Key::PageUp,
        Key::PageDown,
        Key::Enter,
    ] {
        screen.key_press(key);
        screen.run_steps(2);
        assert_eq!(screen.ctx.memory(|m| m.focused()), Some(composer));
    }
    assert_eq!(
        screen.state().projects[0]
            .commit_message
            .matches('\n')
            .count(),
        2
    );
    assert!(screen.state().git_busy.is_none());
    screen.event(egui::Event::Text("jk".into()));
    screen.run_steps(2);
    assert!(screen.state().projects[0].commit_message.contains("jk"));
    assert_eq!(screen.ctx.memory(|memory| memory.focused()), Some(composer));
    screen.ctx.memory_mut(|m| m.surrender_focus(composer));
    screen.state_mut().focus_git_content();
    screen.run_steps(2);
    assert_eq!(
        screen.ctx.memory(|m| m.focused()),
        Some(composer),
        "Returning to Git restores the composer"
    );
}

#[test]
fn git_vim_navigation_handles_batched_keys_once_and_respects_swap() {
    let mut screen = harness(keyboard_changes_fixture(), 360.0, false);
    screen.run_steps(3);
    screen.state_mut().focus_git_content();
    screen.run_steps(2);
    let root = screen.state().projects[0].root.clone();
    for repeat in [false, true] {
        screen.input_mut().events.extend([
            egui::Event::Key {
                key: Key::J,
                physical_key: None,
                pressed: true,
                repeat,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::Text("j".into()),
        ]);
    }
    screen.run_steps(2);
    assert_eq!(
        screen.ctx.memory(|memory| memory.focused()),
        Some(git_row_id(&root, "working:changes:file:src/first.py"))
    );
    screen.state_mut().settings.swap_jk = true;
    screen.key_press(Key::K);
    screen.run_steps(2);
    assert_eq!(
        screen.ctx.memory(|memory| memory.focused()),
        Some(git_row_id(&root, "working:changes:file:src/second.dart"))
    );
    screen.event(egui::Event::Text("j".into()));
    screen.run_steps(2);
    assert_eq!(
        screen.ctx.memory(|memory| memory.focused()),
        Some(git_row_id(&root, "working:changes:file:src/first.py"))
    );
    screen.ctx.memory_mut(|memory| {
        memory.request_focus(git_row_id(&root, "working:changes:folder:src"));
    });
    screen.run_steps(2);
    screen.key_press(Key::H);
    screen.run_steps(2);
    assert!(!screen.state().projects[0].git_view.expanded["working:changes:folder:src"]);
    screen.event(egui::Event::Text("l".into()));
    screen.run_steps(2);
    assert!(screen.state().projects[0].git_view.expanded["working:changes:folder:src"]);
    assert!(screen.state().git_busy.is_none() && screen.state().diff.is_none());
    assert!(screen.state().projects[0].commit_message.is_empty());
}

#[test]
fn git_page_and_half_page_motions_reveal_rows_and_clamp_at_ends() {
    let mut app = keyboard_changes_fixture();
    app.projects[0].git_changes = (0..100)
        .map(|index| git::FileStatus {
            path: format!("file{index:03}.py"),
            original_path: None,
            index: git::FileChange::None,
            worktree: git::FileChange::Modified,
        })
        .collect();
    let root = app.projects[0].root.clone();
    let mut screen = harness(app, 360.0, false);
    screen.run_steps(3);
    screen.state_mut().focus_git_content();
    screen.run_steps(2);
    let page = screen.state().projects[0].git_view.keyboard_page_rows;
    assert!(page > 2);
    screen.key_press(Key::PageDown);
    screen.run_steps(3);
    let page_row = git_row_id(
        &root,
        &format!("working:changes:file:file{:03}.py", page - 1),
    );
    assert_eq!(screen.ctx.memory(|memory| memory.focused()), Some(page_row));
    screen.event(egui::Event::Key {
        key: Key::D,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::CTRL,
    });
    screen.run_steps(3);
    let half_page_index = page - 1 + (page / 2).max(1);
    let half_page_row = git_row_id(
        &root,
        &format!("working:changes:file:file{half_page_index:03}.py"),
    );
    assert_eq!(
        screen.ctx.memory(|memory| memory.focused()),
        Some(half_page_row)
    );
    assert!(
        screen.ctx.content_rect().contains(
            screen
                .ctx
                .read_response(half_page_row)
                .unwrap()
                .rect
                .center()
        )
    );
    screen.key_press(Key::Home);
    screen.run_steps(2);
    screen.key_press(Key::PageUp);
    screen.run_steps(2);
    assert_eq!(
        screen.ctx.memory(|memory| memory.focused()),
        Some(git_row_id(&root, "working:changes"))
    );
    screen.key_press(Key::End);
    screen.run_steps(2);
    screen.key_press(Key::PageDown);
    screen.run_steps(2);
    assert_eq!(
        screen.ctx.memory(|memory| memory.focused()),
        Some(git_row_id(&root, "working:changes:file:file099.py"))
    );
    assert!(screen.state().git_busy.is_none());
}

#[test]
fn git_keyboard_history_end_reveals_offscreen_commit_and_enter_opens_it() {
    let mut app = fixture();
    let project = &mut app.projects[0];
    let mut final_commit = project.recent_commits.last().unwrap().clone();
    for index in 6..=100 {
        final_commit.hash = format!("{index:040x}");
        final_commit.subject = format!("Earlier commit {index}");
        project.recent_commits.push(final_commit.clone());
    }
    let mut detail = project
        .git_view
        .detail_cache
        .values()
        .next()
        .unwrap()
        .clone();
    detail.hash = final_commit.hash.clone();
    project
        .git_view
        .detail_cache
        .insert(detail.hash.clone(), detail);
    let root = project.root.clone();
    let mut screen = harness(app, 360.0, false);
    screen.run_steps(3);
    screen.state_mut().focus_git_content();
    screen.run_steps(2);
    screen.key_press(Key::End);
    screen.run_steps(3);
    assert_eq!(
        screen.ctx.memory(|m| m.focused()),
        Some(git_row_id(&root, &final_commit.hash))
    );
    let row = screen.get_by_label(&final_commit.subject).rect();
    assert!(
        row.intersects(screen.ctx.content_rect()),
        "The destination must be scrolled into view"
    );
    screen.key_press(Key::Enter);
    screen.run_steps(2);
    let state = &screen.state().projects[0].git_view;
    assert_eq!(
        state.selected_commit.as_deref(),
        Some(final_commit.hash.as_str())
    );
    assert!(state.detail_open && !state.detail_loading);
}

#[test]
fn git_keyboard_tab_scrolls_actions_into_view_and_keeps_region_ownership() {
    let mut app = keyboard_changes_fixture();
    app.projects[0].git_changes = (0..60)
        .map(|index| git::FileStatus {
            path: format!("file{index:02}.py"),
            original_path: None,
            index: git::FileChange::None,
            worktree: git::FileChange::Modified,
        })
        .collect();
    let root = app.projects[0].root.clone();
    let mut screen = harness(app, 360.0, false);
    screen.run_steps(3);
    screen.ctx.memory_mut(|memory| {
        memory.request_focus(git_row_id(&root, "working:changes:file:file00.py"))
    });
    screen.run_steps(2);
    for _ in 0..43 {
        screen.key_press(Key::Tab);
        screen.run_steps(2);
    }
    let focused = screen.ctx.memory(|memory| memory.focused()).unwrap();
    assert_eq!(
        focused,
        git_row_id(&root, "working:changes:file:file21.py").with("action")
    );
    let response = screen.ctx.read_response(focused).unwrap();
    assert!(
        screen.ctx.content_rect().contains(response.rect.center()),
        "Tabbed actions must stay visible"
    );
    assert!(screen.state().git_content_has_focus());
}

#[test]
fn git_mouse_context_menus_open_after_idle_and_restore_the_invoking_row() {
    let mut screen = harness(keyboard_changes_fixture(), 360.0, false);
    screen.run_steps(5);
    let root = screen.state().projects[0].root.clone();
    for file in ["first.py", "second.dart"] {
        assert!(!egui::Popup::is_any_open(&screen.ctx));
        screen.get_by_label(file).click_secondary();
        screen.run_steps(3);
        assert!(egui::Popup::is_any_open(&screen.ctx));
        assert!(screen.query_by_label("View diff").is_some());
        assert!(screen.query_by_label("Copy relative path").is_some());
        screen.key_press(Key::Escape);
        screen.run_steps(3);
        assert!(!egui::Popup::is_any_open(&screen.ctx));
        assert_eq!(
            screen.ctx.memory(|memory| memory.focused()),
            Some(git_row_id(
                &root,
                &format!("working:changes:file:src/{file}")
            ))
        );
        assert!(screen.state().git_content_has_focus());
        screen.run_steps(3);
    }
    assert!(screen.state().git_busy.is_none() && screen.state().diff.is_none());
}

#[test]
fn git_keyboard_shift_f10_opens_file_menu_and_escape_returns_to_row() {
    let mut screen = harness(keyboard_changes_fixture(), 360.0, false);
    screen.run_steps(3);
    let root = screen.state().projects[0].root.clone();
    let first = git_row_id(&root, "working:changes:file:src/first.py");
    screen.ctx.memory_mut(|memory| memory.request_focus(first));
    screen.run_steps(2);
    screen.event(egui::Event::Key {
        key: Key::F10,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::SHIFT,
    });
    screen.run_steps(2);
    assert!(screen.query_by_label("Open file").is_some());
    assert!(screen.query_by_label("Copy relative path").is_some());
    assert!(screen.get_by_label("View diff").is_focused());
    screen.key_press(Key::Escape);
    screen.run_steps(2);
    assert!(!egui::Popup::is_any_open(&screen.ctx));
    assert_eq!(screen.ctx.memory(|memory| memory.focused()), Some(first));
    assert!(screen.state().git_content_has_focus());
    assert!(screen.state().git_busy.is_none() && screen.state().diff.is_none());
    screen.event(egui::Event::Key {
        key: Key::F10,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::SHIFT,
    });
    screen.run_steps(3);
    for _ in 0..3 {
        screen.key_press(Key::ArrowDown);
        screen.run_steps(2);
    }
    assert!(screen.get_by_label("Copy relative path").is_focused());
    // Harness::step renders each queued event separately; inspect CopyText on
    // the key-down frame before the key-up frame clears platform commands.
    screen.key_down(Key::Enter);
    screen.run_steps(1);
    assert!(screen.output().platform_output.commands.iter().any(
        |command| matches!(command, egui::OutputCommand::CopyText(text) if text == "src/first.py")
    ));
    screen.key_up(Key::Enter);
    screen.run_steps(2);
    assert_eq!(screen.ctx.memory(|memory| memory.focused()), Some(first));
}

#[test]
fn git_keyboard_no_repository_focus_has_safe_initialize_target() {
    let mut app = fixture();
    app.projects[0].git_root = None;
    let mut screen = harness(app, 360.0, false);
    screen.state_mut().focus_git_content();
    screen.run_steps(3);
    let focused = screen.ctx.memory(|memory| memory.focused()).unwrap();
    assert_eq!(
        screen.ctx.read_response(focused).unwrap().rect,
        screen.get_by_label("Initialize repository").rect()
    );
    assert!(screen.state().git_busy.is_none());
}

#[test]
fn repository_overview_keeps_push_visible_and_separates_files_tracking_and_main() {
    let mut app = fixture();
    app.projects[0].git_tracking = Some(git::BranchInfo {
        name: Some("feature".into()),
        upstream: Some("origin/feature".into()),
        ahead: 3,
        behind: 2,
        divergence_known: true,
    });
    app.projects[0].git_main_comparison = Ok(Some(git::BranchComparison {
        base: "main".into(),
        ahead: 12,
        behind: 1,
    }));
    let mut screen = harness(app, 300.0, false);
    screen.run_steps(2);
    for label in [
        "Repository status",
        "Working tree clean",
        "Tracking · origin/feature",
        "3 ahead · 2 behind",
        "Local base · main",
        "12 ahead · 1 behind",
        "Fetch",
        "Pull",
        "Push",
        "Remote status as of last fetch",
    ] {
        assert!(screen.query_by_label(label).is_some(), "missing {label}");
    }
    screen.state_mut().projects[0]
        .git_changes
        .push(git::FileStatus {
            path: "file.rs".into(),
            original_path: None,
            index: git::FileChange::Modified,
            worktree: git::FileChange::Modified,
        });
    let project = &mut screen.state_mut().projects[0];
    project
        .git_view
        .update_working_cache(&project.git_changes, &project.git_stats);
    screen.run_steps(2);
    assert!(screen.query_by_label("1 changed file").is_some());
    assert!(
        screen
            .query_by_label("1 staged · 1 unstaged · 0 untracked")
            .is_some()
    );
    assert!(screen.query_by_label("Push").is_some());
    assert!(screen.query_by_label("3 ahead · 2 behind").is_some());
    screen.state_mut().projects[0]
        .git_tracking
        .as_mut()
        .unwrap()
        .divergence_known = false;
    screen.run_steps(2);
    assert!(
        screen
            .query_by_label("Tracking status unavailable · Fetch to refresh")
            .is_some()
    );
    assert!(screen.query_by_label("3 ahead · 2 behind").is_none());
    screen.state_mut().projects[0].git_tracking = None;
    screen.run_steps(2);
    assert!(
        screen
            .query_by_label("No tracking branch configured")
            .is_some()
    );
    assert!(screen.query_by_label("Push").is_some());
    assert!(screen.query_by_label("12 ahead · 1 behind").is_some());
}

#[test]
fn git_keyboard_short_viewport_scrolls_composer_into_view() {
    let mut screen = harness(keyboard_changes_fixture(), 360.0, false);
    screen.set_size(Vec2::new(360.0, 240.0));
    screen.run_steps(3);
    let composer = git_row_id(&screen.state().projects[0].root, "commit-message");
    screen
        .ctx
        .memory_mut(|memory| memory.request_focus(composer));
    screen.run_steps(3);
    let rect = screen
        .get_by_role(egui::accesskit::Role::MultilineTextInput)
        .rect();
    assert!(
        screen.ctx.content_rect().contains(rect.center()),
        "Commit entry must be reachable in a short/zoomed sidebar: rect={rect:?}, viewport={:?}",
        screen.ctx.content_rect()
    );
    assert!(screen.state().git_content_has_focus());
}

#[test]
fn git_keyboard_nested_diff_returns_to_invoking_commit_file() {
    let mut app = fixture();
    let hash = app.projects[0].recent_commits[0].hash.clone();
    let mut detail = app.projects[0].git_view.detail_cache[&hash].clone();
    detail.files = vec![git::CommitFile {
        path: "notes.txt".into(),
        original_path: None,
        status: git::FileChange::Modified,
        stats: git::LineStats {
            added: 1,
            removed: 1,
            binary: false,
        },
    }];
    app.projects[0].git_view.detail = Some(detail);
    app.projects[0].git_view.selected_commit = Some(hash.clone());
    app.projects[0].git_view.detail_open = true;
    let root = app.projects[0].root.clone();
    let mut screen = Harness::builder()
        .with_size([1000.0, 760.0])
        .with_os(egui::os::OperatingSystem::Windows)
        .build_ui_state(
            |ui, app: &mut VEditApp| {
                app.ctx = ui.ctx().clone();
                app.git_ui(ui);
                app.overlays();
            },
            app,
        );
    set_style(&screen.ctx);
    screen.run_steps(4);
    screen.get_by_label("notes.txt").focus();
    screen.run_steps(2);
    let row = git_row_id(&root, &format!("commit:{hash}:file:notes.txt"));
    assert_eq!(screen.ctx.memory(|memory| memory.focused()), Some(row));
    screen.key_press(Key::Enter);
    screen.run_steps(3);
    assert!(screen.state().diff.is_some());
    screen.key_press(Key::Escape);
    screen.run_steps(3);
    assert!(screen.state().diff.is_none() && screen.state().git_detail_open());
    assert_eq!(screen.ctx.memory(|memory| memory.focused()), Some(row));
}

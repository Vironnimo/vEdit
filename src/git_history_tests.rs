use super::*;
use egui_kittest::{Harness, kittest::Queryable};

fn commit(index: usize) -> git::CommitSummary {
    git::CommitSummary {
        hash: format!("{index:040x}"),
        parents: vec![format!("{:040x}", index + 1)],
        refs: Vec::new(),
        author: "History Author".into(),
        date: "2026-09-26".into(),
        subject: format!("Commit {index:02}"),
    }
}

fn history_workspace(count: usize) -> MorrowApp {
    let mut app = MorrowApp::empty(egui::Context::default());
    visual_tests::workspace(&mut app);
    app.set_sidebar_view_visible(SidebarView::Explorer, false);
    app.move_sidebar(SidebarView::SourceControl, sidebar_layout::Side::Left);
    let project = &mut app.projects[0];
    project.git_root = Some(project.root.clone());
    project.last_git_refresh = Instant::now() + Duration::from_secs(3600);
    project.git_view.section = git_panel::GitSection::History;
    project.git_history_generation = u64::MAX - 7;
    project.git_history_has_more = true;
    // Scroll tests dispatch real workers. Empty traversal roots return a page
    // immediately without launching Git against the read-only UI fixture.
    project.git_history_snapshot = Some(git::HistorySnapshot::default());
    project.recent_commits = (0..count).map(commit).collect();
    project.git_view.graph.append(&project.recent_commits);
    app
}

fn deliver_page(
    app: &mut MorrowApp,
    generation: u64,
    offset: usize,
    result: Result<git::HistoryPage, String>,
) {
    app.sender
        .send(WorkerResult::GitHistory {
            root: app.projects[0].root.clone(),
            generation,
            offset,
            result,
        })
        .unwrap();
    app.poll();
}

fn deliver_refresh(
    app: &mut MorrowApp,
    snapshot: git::HistorySnapshot,
    page: Option<git::HistoryPage>,
) {
    app.projects[0].git_status_request = Some(u64::MAX);
    app.sender
        .send(WorkerResult::GitStatus {
            root: app.projects[0].root.clone(),
            request_id: u64::MAX,
            result: Ok(GitSnapshot {
                root: app.projects[0].git_root.clone(),
                branch: Some("main".into()),
                tracking: None,
                files: Vec::new(),
                stats: git::WorkingTreeStats::default(),
                stats_error: None,
                history: Ok(GitHistoryRefresh { snapshot, page }),
            }),
        })
        .unwrap();
    app.poll();
}

fn history_screen(app: MorrowApp) -> Harness<'static, MorrowApp> {
    // Paint the actual Git panel without draw_ui's background-result polling.
    // A dispatched page stays pending across frames so its request guard can
    // be tested independently of worker scheduling and the rest of the editor.
    let screen = Harness::builder()
        .with_size([360.0, 600.0])
        .with_theme(egui::Theme::Dark)
        .with_os(egui::os::OperatingSystem::Windows)
        .build_ui_state(
            |ui, app: &mut MorrowApp| {
                app.ctx = ui.ctx().clone();
                app.git_ui(ui);
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

fn immediate_history_tooltips(screen: &mut Harness<'_, MorrowApp>) {
    screen.ctx.all_styles_mut(|style| {
        style.interaction.tooltip_delay = 0.0;
        style.interaction.tooltip_grace_time = 0.0;
        style.interaction.show_tooltips_only_when_still = false;
    });
}

struct HoverRepository(PathBuf);

impl HoverRepository {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "morrow-git-hover-ui-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        git::initialize(&root).unwrap();
        for (key, value) in [
            ("user.name", "Hover Author"),
            ("user.email", "hover@example.invalid"),
            ("core.autocrlf", "false"),
            ("commit.gpgsign", "false"),
        ] {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["config", key, value])
                .output()
                .unwrap();
            assert!(output.status.success());
        }
        fs::write(root.join("first.txt"), "one\n").unwrap();
        git::stage_all(&root).unwrap();
        git::commit(&root, "Initial").unwrap();
        fs::write(root.join("first.txt"), "one edited\nsecond\n").unwrap();
        fs::write(root.join("second.txt"), "new\n").unwrap();
        git::stage_all(&root).unwrap();
        git::commit(&root, "Short subject").unwrap();
        Self(root)
    }

    fn app(&self) -> MorrowApp {
        let mut app = history_workspace(0);
        let snapshot = git::history_snapshot(&self.0).unwrap();
        let page = git::history_page(&self.0, &snapshot, 0, 10).unwrap();
        let project = &mut app.projects[0];
        project.root = self.0.clone();
        project.git_root = Some(self.0.clone());
        project.git_history_snapshot = Some(snapshot);
        project.git_history_has_more = false;
        project.recent_commits = page.commits;
        project.git_view.graph.append(&project.recent_commits);
        app
    }
}

impl Drop for HoverRepository {
    fn drop(&mut self) {
        let temporary = fs::canonicalize(std::env::temp_dir()).unwrap();
        let root = fs::canonicalize(&self.0).unwrap();
        assert!(root.starts_with(&temporary));
        assert!(
            root.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("morrow-git-hover-ui-")
        );
        let _ = fs::remove_dir_all(&root);
    }
}

#[test]
fn history_hover_loads_statistics_once_then_reuses_cached_colored_totals() {
    let repository = HoverRepository::new();
    let mut screen = history_screen(repository.app());
    immediate_history_tooltips(&mut screen);
    screen.run_steps(3);
    let commit = screen.state().projects[0].recent_commits[0].clone();
    let key = (repository.0.clone(), commit.hash.clone());
    screen.hover_at(screen.get_by_label(&commit.subject).rect().center());
    screen.run_steps(4);
    assert!(screen.query_by_label("Loading changes…").is_some());
    let request = screen.state().projects[0].git_view.hover_stats_pending[&key];
    screen.run_steps(3);
    assert_eq!(
        screen.state().projects[0].git_view.hover_stats_pending[&key],
        request
    );

    let deadline = Instant::now() + Duration::from_secs(5);
    while !screen.state().projects[0]
        .git_view
        .hover_stats
        .contains_key(&key)
    {
        assert!(
            Instant::now() < deadline,
            "commit hover statistics did not arrive"
        );
        screen.state_mut().poll();
        std::thread::sleep(Duration::from_millis(5));
    }
    let stats = screen.state().projects[0].git_view.hover_stats[&key]
        .as_ref()
        .unwrap();
    assert_eq!(stats.files_changed, 2);
    assert_eq!((stats.lines.added, stats.lines.removed), (3, 1));
    screen.run_steps(4);
    assert!(screen.query_by_label("2 files changed").is_some());
    assert!(screen.query_by_label("+3").is_some());
    assert!(screen.query_by_label("−1").is_some());
    assert!(screen.query_by_label(&commit.hash).is_some());
    for (label, expected) in [("+3", ACCENT), ("−1", RED)] {
        assert!(screen.output().shapes.iter().any(|shape| {
            let egui::Shape::Text(text) = &shape.shape else {
                return false;
            };
            text.galley.text() == label && text.galley.job.sections[0].format.color == expected
        }));
    }
    assert!(
        screen.state().projects[0]
            .git_view
            .hover_stats_pending
            .is_empty()
    );
    screen.run_steps(4);
    assert!(
        screen.state().projects[0]
            .git_view
            .hover_stats_pending
            .is_empty()
    );
    assert!(matches!(
        screen.state_mut().receiver.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
}

#[test]
fn older_merge_commit_hover_keeps_metadata_and_shows_its_cached_change_summary() {
    let mut app = history_workspace(5);
    let project = &mut app.projects[0];
    project.git_history_has_more = false;
    project.recent_commits[2].parents = vec![commit(3).hash, commit(4).hash];
    let hash = project.recent_commits[2].hash.clone();
    project.recent_commits[2].refs.push(git::CommitRef {
        hash: hash.clone(),
        name: "topic/older-merge".into(),
        kind: git::RefKind::Local,
        current: false,
    });
    project.git_view.graph = crate::git_graph::Graph::default();
    project.git_view.graph.append(&project.recent_commits);
    project.git_view.hover_stats.insert(
        (project.root.clone(), hash.clone()),
        Ok(git::CommitStats {
            files_changed: 3,
            lines: git::LineStats {
                added: 7,
                removed: 4,
                binary: true,
            },
        }),
    );
    let mut screen = history_screen(app);
    immediate_history_tooltips(&mut screen);
    screen.run_steps(3);
    screen.hover_at(screen.get_by_label("Commit 02").rect().center());
    screen.run_steps(4);
    for label in [
        "3 files changed",
        "+7",
        "−4",
        "Includes binary changes",
        "topic/older-merge",
        "History Author · 2026-09-26",
        &hash,
    ] {
        assert!(
            screen.query_by_label(label).is_some(),
            "missing hover metadata: {label}"
        );
    }
    assert!(
        screen.state().projects[0]
            .git_view
            .hover_stats_pending
            .is_empty()
    );
    assert!(matches!(
        screen.state_mut().receiver.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
}

#[test]
fn successful_git_refresh_retries_hover_errors_and_retains_immutable_totals() {
    let mut app = history_workspace(2);
    let repository = app.projects[0].root.clone();
    let successful = (repository.clone(), commit(0).hash);
    let failed = (repository, commit(1).hash);
    let stats = git::CommitStats {
        files_changed: 1,
        lines: git::LineStats {
            added: 3,
            removed: 2,
            binary: false,
        },
    };
    let view = &mut app.projects[0].git_view;
    view.hover_stats.insert(successful.clone(), Ok(stats));
    view.hover_stats
        .insert(failed.clone(), Err("temporary read failure".into()));
    view.hover_stats_order
        .extend([successful.clone(), failed.clone()]);
    deliver_refresh(&mut app, git::HistorySnapshot::default(), None);
    let view = &app.projects[0].git_view;
    assert_eq!(view.hover_stats[&successful], Ok(stats));
    assert!(!view.hover_stats.contains_key(&failed));
    assert_eq!(
        view.hover_stats_order.iter().collect::<Vec<_>>(),
        [&successful]
    );
}

#[test]
fn repository_relocation_releases_hover_slots_and_discards_old_repository_results() {
    let repository = HoverRepository::new();
    let mut app = repository.app();
    let root = app.projects[0].root.clone();
    let old_repository = root.join("previous-repository");
    let hash = app.projects[0].recent_commits[0].hash.clone();
    let current_key = (repository.0.clone(), hash.clone());
    let old_requests: Vec<_> = (0..4)
        .map(|index| {
            (
                (old_repository.clone(), commit(index + 30).hash),
                100 + index as u64,
            )
        })
        .collect();
    app.projects[0].git_root = Some(old_repository.clone());
    app.projects[0]
        .git_view
        .hover_stats_pending
        .extend(old_requests.iter().cloned());
    app.request_git_hover_stats(0, hash.clone());
    assert_eq!(app.projects[0].git_view.hover_stats_pending.len(), 4);
    assert!(
        !app.projects[0]
            .git_view
            .hover_stats_pending
            .contains_key(&current_key)
    );

    app.projects[0].git_status_request = Some(u64::MAX);
    app.sender
        .send(WorkerResult::GitStatus {
            root: root.clone(),
            request_id: u64::MAX,
            result: Ok(GitSnapshot {
                root: Some(repository.0.clone()),
                branch: Some("main".into()),
                tracking: None,
                files: Vec::new(),
                stats: git::WorkingTreeStats::default(),
                stats_error: None,
                history: Ok(GitHistoryRefresh {
                    snapshot: app.projects[0].git_history_snapshot.clone().unwrap(),
                    page: None,
                }),
            }),
        })
        .unwrap();
    app.poll();
    assert!(app.projects[0].git_view.hover_stats_pending.is_empty());

    app.request_git_hover_stats(0, hash);
    let current_request = app.projects[0].git_view.hover_stats_pending[&current_key];
    assert_eq!(app.projects[0].git_view.hover_stats_pending.len(), 1);
    for ((repository, hash), request_id) in &old_requests {
        app.sender
            .send(WorkerResult::GitCommitStats {
                root: root.clone(),
                repository: repository.clone(),
                hash: hash.clone(),
                request_id: *request_id,
                result: Ok(git::CommitStats {
                    files_changed: 999,
                    lines: git::LineStats {
                        added: 999,
                        removed: 999,
                        binary: false,
                    },
                }),
            })
            .unwrap();
    }
    app.poll();
    let view = &app.projects[0].git_view;
    assert!(old_requests.iter().all(|(key, _)| {
        !view.hover_stats.contains_key(key) && !view.hover_stats_pending.contains_key(key)
    }));
    assert!(
        view.hover_stats.contains_key(&current_key)
            || view.hover_stats_pending.get(&current_key) == Some(&current_request)
    );

    let deadline = Instant::now() + Duration::from_secs(5);
    while !app.projects[0]
        .git_view
        .hover_stats
        .contains_key(&current_key)
    {
        assert!(
            Instant::now() < deadline,
            "relocated repository hover request did not complete"
        );
        app.poll();
        std::thread::sleep(Duration::from_millis(5));
    }
    let stats = app.projects[0].git_view.hover_stats[&current_key]
        .as_ref()
        .unwrap();
    assert_eq!(stats.files_changed, 2);
    assert_eq!((stats.lines.added, stats.lines.removed), (3, 1));
    assert!(app.projects[0].git_view.hover_stats_pending.is_empty());
}

#[test]
fn switching_projects_discards_late_hover_results_without_canceling_a_new_request() {
    let mut app = history_workspace(2);
    let old_root = app.projects[0].root.clone();
    let repository = app.projects[0].git_root.clone().unwrap();
    let hash = commit(0).hash;
    let key = (repository.clone(), hash.clone());
    app.projects[0]
        .git_view
        .hover_stats_pending
        .insert(key.clone(), 41);
    let new_root = old_root.join("another-open-folder");
    let mut project = Project::new(new_root.clone());
    project.git_root = Some(repository.clone());
    app.projects.push(project);
    app.switch_project(1);
    assert!(app.projects[0].git_view.hover_stats_pending.is_empty());
    app.projects[1]
        .git_view
        .hover_stats_pending
        .insert(key.clone(), 42);
    let stats = git::CommitStats {
        files_changed: 1,
        lines: git::LineStats {
            added: 3,
            removed: 2,
            binary: false,
        },
    };
    app.sender
        .send(WorkerResult::GitCommitStats {
            root: old_root,
            repository: repository.clone(),
            hash: hash.clone(),
            request_id: 41,
            result: Ok(stats),
        })
        .unwrap();
    app.poll();
    assert!(app.projects[0].git_view.hover_stats.is_empty());
    assert!(app.projects[1].git_view.hover_stats.is_empty());
    assert_eq!(app.projects[1].git_view.hover_stats_pending[&key], 42);
    app.sender
        .send(WorkerResult::GitCommitStats {
            root: new_root,
            repository,
            hash,
            request_id: 42,
            result: Ok(stats),
        })
        .unwrap();
    app.poll();
    assert_eq!(app.projects[1].git_view.hover_stats[&key], Ok(stats));
    assert!(app.projects[1].git_view.hover_stats_pending.is_empty());
}

#[test]
fn only_the_expected_history_page_extends_the_existing_graph() {
    let mut app = history_workspace(2);
    let generation = app.projects[0].git_history_generation;
    app.projects[0].git_history_loading = true;
    let prefix = app.projects[0].recent_commits.clone();
    let graph = app.projects[0].git_view.graph.clone();
    let page = git::HistoryPage {
        commits: vec![commit(2), commit(3)],
        has_more: true,
    };

    deliver_page(&mut app, generation - 1, 2, Ok(page.clone()));
    deliver_page(&mut app, generation, 0, Ok(page.clone()));
    assert_eq!(app.projects[0].recent_commits, prefix);
    assert_eq!(app.projects[0].git_view.graph, graph);
    assert!(app.projects[0].git_history_loading);

    deliver_page(&mut app, generation, 2, Ok(page.clone()));
    assert_eq!(app.projects[0].recent_commits.len(), 4);
    assert_eq!(app.projects[0].recent_commits[..2], prefix);
    assert_eq!(app.projects[0].git_view.graph.rows[..2], graph.rows);
    assert!(!app.projects[0].git_history_loading);
    assert!(app.projects[0].git_history_has_more);
    let complete_graph = app.projects[0].git_view.graph.clone();

    // A duplicated success and a late failure from the same offset are stale
    // once the first copy has appended, so neither can alter the current page.
    deliver_page(&mut app, generation, 2, Ok(page));
    deliver_page(&mut app, generation, 2, Err("late failure".into()));
    assert_eq!(app.projects[0].recent_commits.len(), 4);
    assert_eq!(app.projects[0].git_view.graph, complete_graph);
    assert!(app.projects[0].git_history_error.is_none());
}

#[test]
fn unchanged_automatic_refresh_preserves_loaded_pages_and_pending_request() {
    let mut app = history_workspace(git::HISTORY_PAGE_SIZE * 2);
    let snapshot = git::HistorySnapshot {
        tips: vec![commit(0).hash],
        refs: vec![git::CommitRef {
            hash: commit(0).hash,
            name: "main".into(),
            kind: git::RefKind::Local,
            current: true,
        }],
    };
    let project = &mut app.projects[0];
    project.git_history_snapshot = Some(snapshot.clone());
    project.git_history_loading = true;
    let commits = project.recent_commits.clone();
    let graph = project.git_view.graph.clone();
    let generation = project.git_history_generation;

    deliver_refresh(&mut app, snapshot.clone(), None);
    let project = &app.projects[0];
    assert_eq!(project.recent_commits, commits);
    assert_eq!(project.git_view.graph, graph);
    assert_eq!(project.git_history_snapshot.as_ref(), Some(&snapshot));
    assert_eq!(project.git_history_generation, generation);
    assert!(project.git_history_loading && project.git_history_has_more);
    assert!(!project.git_history_update_available);
}

#[test]
fn moved_refs_leave_the_original_graph_and_snapshot_pageable() {
    let mut app = history_workspace(2);
    let original = git::HistorySnapshot {
        tips: vec![commit(0).hash],
        refs: vec![git::CommitRef {
            hash: commit(0).hash,
            name: "main".into(),
            kind: git::RefKind::Local,
            current: true,
        }],
    };
    app.projects[0].git_history_snapshot = Some(original.clone());
    app.projects[0].git_history_loading = true;
    let generation = app.projects[0].git_history_generation;
    let prefix = app.projects[0].recent_commits.clone();
    let graph = app.projects[0].git_view.graph.clone();
    let changed = git::HistorySnapshot {
        tips: vec![commit(99).hash],
        refs: vec![git::CommitRef {
            hash: commit(99).hash,
            name: "main".into(),
            kind: git::RefKind::Local,
            current: true,
        }],
    };

    deliver_refresh(&mut app, changed, None);
    assert!(app.projects[0].git_history_update_available);
    assert_eq!(app.projects[0].recent_commits, prefix);
    assert_eq!(app.projects[0].git_view.graph, graph);
    assert_eq!(
        app.projects[0].git_history_snapshot.as_ref(),
        Some(&original)
    );
    assert_eq!(app.projects[0].git_history_generation, generation);
    assert!(app.projects[0].git_history_loading);

    deliver_page(
        &mut app,
        generation,
        2,
        Ok(git::HistoryPage {
            commits: vec![commit(2)],
            has_more: false,
        }),
    );
    assert_eq!(app.projects[0].recent_commits[..2], prefix);
    assert_eq!(app.projects[0].git_view.graph.rows[..2], graph.rows);
    assert_eq!(app.projects[0].recent_commits[2].hash, commit(2).hash);
    assert_eq!(
        app.projects[0].git_history_snapshot.as_ref(),
        Some(&original)
    );
    assert!(app.projects[0].git_history_update_available);
    assert!(!app.projects[0].git_history_has_more);
}

#[test]
fn replacing_history_invalidates_a_pending_page_even_at_the_same_offset() {
    let mut app = history_workspace(2);
    app.projects[0].git_history_loading = true;
    let old_generation = app.projects[0].git_history_generation;
    let replacement = vec![commit(100), commit(101)];
    let snapshot = git::HistorySnapshot {
        tips: vec![replacement[0].hash.clone()],
        refs: Vec::new(),
    };
    deliver_refresh(
        &mut app,
        snapshot.clone(),
        Some(git::HistoryPage {
            commits: replacement.clone(),
            has_more: true,
        }),
    );
    let generation = app.projects[0].git_history_generation;
    assert_ne!(generation, old_generation);
    assert!(!app.projects[0].git_history_loading);
    assert_eq!(
        app.projects[0].git_history_snapshot.as_ref(),
        Some(&snapshot)
    );

    deliver_page(
        &mut app,
        old_generation,
        2,
        Ok(git::HistoryPage {
            commits: vec![commit(2)],
            has_more: false,
        }),
    );
    assert_eq!(app.projects[0].recent_commits, replacement);
    assert_eq!(app.projects[0].git_view.graph.rows.len(), 2);
    assert_eq!(app.projects[0].git_history_generation, generation);
    assert!(app.projects[0].git_history_has_more);
}

#[test]
fn scrolling_near_the_end_requests_one_page_without_a_load_more_button() {
    let mut screen = history_screen(history_workspace(git::HISTORY_PAGE_SIZE));
    screen.run_steps(3);
    assert!(!screen.state().projects[0].git_history_loading);
    assert!(screen.query_by_label("Load more").is_none());
    let first = screen.get_by_label("Commit 00").rect().center();
    screen.hover_at(first);
    screen.input_mut().events.push(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: Vec2::new(0.0, -100_000.0),
        phase: egui::TouchPhase::Move,
        modifiers: egui::Modifiers::NONE,
    });
    screen.run_steps(3);
    assert!(screen.state().projects[0].git_history_loading);
    assert!(screen.query_by_label("Loading commits…").is_some());

    let request = screen
        .state_mut()
        .receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("the scroll dispatched a history worker");
    let WorkerResult::GitHistory {
        generation,
        offset,
        result,
        ..
    } = request
    else {
        panic!("the isolated Git panel only starts a history worker");
    };
    assert_eq!(
        generation,
        screen.state().projects[0].git_history_generation
    );
    assert_eq!(offset, git::HISTORY_PAGE_SIZE);
    assert_eq!(result.unwrap(), git::HistoryPage::default());
    screen.run_steps(3);
    assert!(matches!(
        screen
            .state_mut()
            .receiver
            .recv_timeout(Duration::from_millis(50)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    ));
    assert_eq!(
        screen.state().projects[0].recent_commits.len(),
        git::HISTORY_PAGE_SIZE
    );

    screen.state_mut().projects[0].git_history_loading = false;
    screen.state_mut().projects[0].git_history_has_more = false;
    screen.run_steps(3);
    assert!(screen.query_by_label("End of available history").is_some());
    assert!(screen.query_by_label("Loading commits…").is_none());
}

#[test]
fn a_history_error_stops_automatic_requests_until_explicit_retry() {
    let mut app = history_workspace(2);
    app.projects[0].git_history_loading = true;
    let generation = app.projects[0].git_history_generation;
    let graph = app.projects[0].git_view.graph.clone();
    deliver_page(&mut app, generation, 2, Err("history unavailable".into()));
    assert!(!app.projects[0].git_history_loading);
    assert_eq!(app.projects[0].git_view.graph, graph);
    app.load_more_git_history();
    assert!(!app.projects[0].git_history_loading);

    let mut screen = history_screen(app);
    screen.run_steps(3);
    assert!(!screen.state().projects[0].git_history_loading);
    assert_eq!(screen.state().projects[0].recent_commits.len(), 2);
    assert!(screen.query_by_label("Retry history").is_some());
    assert!(screen.state_mut().receiver.try_recv().is_err());

    screen.get_by_label("Retry history").click();
    screen.run_steps(3);
    assert!(screen.state().projects[0].git_history_error.is_none());
    assert!(screen.state().projects[0].git_history_loading);
    assert_eq!(screen.state().projects[0].git_view.graph, graph);
}

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
    app.sender
        .send(WorkerResult::GitStatus {
            root: app.projects[0].root.clone(),
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

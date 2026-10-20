use super::*;
use egui_kittest::{Harness, kittest::Queryable};

fn commit(index: usize) -> git::CommitSummary {
    git::CommitSummary {
        hash: format!("{index:040x}"),
        parents: vec![format!("{:040x}", index + 1)],
        refs: Vec::new(),
        author: "History Author".into(),
        date: "2026-09-26".into(),
        timestamp: 0,
        on_remote: false,
        subject: format!("Commit {index:02}"),
    }
}

fn history_workspace(count: usize) -> VEditApp {
    let mut app = VEditApp::empty(egui::Context::default());
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
    app: &mut VEditApp,
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
    app: &mut VEditApp,
    snapshot: git::HistorySnapshot,
    page: Option<git::HistoryPage>,
) {
    app.projects[0].git_status_request = Some(u64::MAX);
    app.sender
        .send(WorkerResult::GitStatus {
            root: app.projects[0].root.clone(),
            request_id: u64::MAX,
            result: Ok(Box::new(GitSnapshot {
                root: app.projects[0].git_root.clone(),
                parent_root: None,
                branch: Some("main".into()),
                tracking: None,
                main_comparison: Ok(None),
                untracked_collapsed: false,
                files: Vec::new(),
                stats: git::WorkingTreeStats::default(),
                stats_error: None,
                history: Ok(GitHistoryRefresh { snapshot, page }),
            })),
        })
        .unwrap();
    app.poll();
}

fn history_screen(app: VEditApp) -> Harness<'static, VEditApp> {
    history_screen_with(app, false)
}

fn history_screen_with(app: VEditApp, panel_menu: bool) -> Harness<'static, VEditApp> {
    // Paint the actual Git panel without draw_ui's background-result polling.
    // A dispatched page stays pending across frames so its request guard can
    // be tested independently of worker scheduling and the rest of the editor.
    let screen = Harness::builder()
        .with_size([360.0, 800.0])
        .with_theme(egui::Theme::Dark)
        .with_os(egui::os::OperatingSystem::Windows)
        .build_ui_state(
            move |ui, app: &mut VEditApp| {
                app.ctx = ui.ctx().clone();
                if panel_menu {
                    // Repository actions live in the sidebar header's menu.
                    ui.menu_button("Repository actions", |ui| {
                        app.git_repository_menu(ui);
                    });
                }
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

fn immediate_history_tooltips(screen: &mut Harness<'_, VEditApp>) {
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
            "vedit-git-hover-ui-{}-{}",
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

    fn app(&self) -> VEditApp {
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
                .starts_with("vedit-git-hover-ui-")
        );
        let _ = fs::remove_dir_all(&root);
    }
}

#[test]
fn unchanged_repository_refresh_on_main_launches_three_git_processes() {
    let repository = HoverRepository::new();
    for args in [
        ["branch", "-M", "main"].as_slice(),
        &["update-ref", "refs/remotes/origin/main", "HEAD"],
    ] {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(&repository.0)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success());
    }
    let first = read_git_snapshot(&repository.0, None, None, None, false, None, None).unwrap();
    let repo = first.root.clone().unwrap();
    let previous = first.history.unwrap().snapshot;
    git::take_command_count();
    // Repository detection, status with HEAD, and the ref listing.
    let refreshed = read_git_snapshot(
        &repository.0,
        None,
        Some(&repo),
        Some(previous.clone()),
        false,
        None,
        Some(git::HISTORY_PAGE_SIZE),
    )
    .unwrap();
    assert_eq!(git::take_command_count(), 3);
    assert!(refreshed.files.is_empty());
    assert_eq!(refreshed.branch.as_deref(), Some("main"));
    assert_eq!(
        refreshed.main_comparison,
        Ok(Some(git::BranchComparison {
            base: "main".into(),
            ahead: 0,
            behind: 0,
        }))
    );
    let history = refreshed.history.unwrap();
    assert_eq!(history.snapshot, previous);
    assert!(history.page.is_none());
}

fn wait_for_git_refresh(app: &mut VEditApp, index: usize) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while app.projects[index].git_loading {
        assert!(Instant::now() < deadline, "Git refresh timed out");
        app.poll();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(app.projects[index].git_error.is_none());
}

fn history_window_state(app: &mut VEditApp, focused: bool, minimized: bool) {
    let mut input = egui::RawInput {
        focused,
        ..Default::default()
    };
    input
        .viewports
        .get_mut(&egui::ViewportId::ROOT)
        .unwrap()
        .minimized = Some(minimized);
    app.ctx.run_ui(input, |_| {}).textures_delta.clear();
}

#[test]
fn visible_history_automatically_shows_external_commits() {
    let repository = HoverRepository::new();
    let mut app = repository.app();
    history_window_state(&mut app, true, false);
    let previous = app.projects[0].recent_commits.clone();
    app.projects[0].git_view.selected_commit = Some(previous[1].hash.clone());
    fs::write(repository.0.join("external.txt"), "from another process\n").unwrap();
    git::stage_all(&repository.0).unwrap();
    git::commit(&repository.0, "External commit").unwrap();
    app.projects[0].last_git_refresh = Instant::now() - Duration::from_secs(4);

    app.refresh_git(0, false);
    let request = app.projects[0].git_status_request;
    app.refresh_git(0, false);
    assert_eq!(app.projects[0].git_status_request, request);
    assert!(!app.projects[0].git_refresh_pending);
    wait_for_git_refresh(&mut app, 0);
    assert_eq!(app.projects[0].recent_commits[0].subject, "External commit");
    assert_eq!(
        app.projects[0].recent_commits[1..]
            .iter()
            .map(|commit| &commit.hash)
            .collect::<Vec<_>>(),
        previous
            .iter()
            .map(|commit| &commit.hash)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        app.projects[0].git_view.selected_commit.as_ref(),
        Some(&previous[1].hash)
    );
    app.refresh_git(0, false);
    assert!(
        !app.projects[0].git_loading,
        "completed polls must be throttled"
    );
}

#[test]
fn automatic_repository_refresh_pauses_in_background_and_when_minimized() {
    let repository = HoverRepository::new();
    let mut app = repository.app();
    for (focused, minimized) in [(false, false), (true, true)] {
        history_window_state(&mut app, focused, minimized);
        app.projects[0].last_git_refresh = Instant::now() - Duration::from_secs(30);
        app.refresh_git(0, false);
        assert!(!app.projects[0].git_loading);
        assert!(app.projects[0].git_status_request.is_none());
    }
    history_window_state(&mut app, true, false);
    app.refresh_git(0, false);
    assert!(app.projects[0].git_loading);
    wait_for_git_refresh(&mut app, 0);
}

#[test]
fn restoring_the_window_refreshes_immediately_even_after_a_recent_read() {
    for (focused, minimized) in [(false, false), (true, true)] {
        let repository = HoverRepository::new();
        let mut app = repository.app();
        history_window_state(&mut app, focused, minimized);
        app.refresh_visible_git();
        fs::write(repository.0.join("external.txt"), "new while away\n").unwrap();
        git::stage_all(&repository.0).unwrap();
        git::commit(&repository.0, "While away").unwrap();
        // No polling interval has elapsed. Returning still must start a read.
        app.projects[0].last_git_refresh = Instant::now();
        history_window_state(&mut app, true, false);
        app.refresh_visible_git();
        assert!(app.projects[0].git_loading);
        wait_for_git_refresh(&mut app, 0);
        assert_eq!(app.projects[0].recent_commits[0].subject, "While away");
        app.refresh_visible_git();
        assert!(!app.projects[0].git_loading);
    }
}

#[test]
fn primary_push_button_pushes_to_an_isolated_local_remote() {
    let repository = HoverRepository::new();
    let remote = repository.0.join(".git/push-test-remote.git");
    for args in [
        vec!["init", "--bare", remote.to_str().unwrap()],
        vec!["remote", "add", "origin", remote.to_str().unwrap()],
        vec!["push", "-u", "origin", "HEAD"],
    ] {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(&repository.0)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fs::write(repository.0.join("push.txt"), "local test\n").unwrap();
    git::stage_all(&repository.0).unwrap();
    git::commit(&repository.0, "Ready to push").unwrap();
    let mut app = repository.app();
    history_window_state(&mut app, true, false);
    app.refresh_git(0, true);
    wait_for_git_refresh(&mut app, 0);
    assert_eq!(app.projects[0].git_tracking.as_ref().unwrap().ahead, 1);
    let head = app.projects[0].recent_commits[0].hash.clone();
    let branch = app.projects[0].git_branch.clone().unwrap();
    // Hold an old read across the click and deliver it after the push. The
    // action must stay available and an obsolete snapshot must not undo it.
    let stale = read_git_snapshot(&repository.0, None, None, None, false, None, None).unwrap();
    app.projects[0].git_loading = true;
    app.projects[0].git_status_request = Some(u64::MAX);
    let mut screen = history_screen(app);
    screen.run_steps(3);
    screen.get_by_label("Push 1 commit").click();
    screen.run_steps(2);
    assert!(screen.state().git_busy.is_some());
    assert!(!screen.state().projects[0].git_loading);
    let deadline = Instant::now() + Duration::from_secs(15);
    while screen.state().git_busy.is_some() || screen.state().projects[0].git_loading {
        assert!(Instant::now() < deadline, "local push did not finish");
        screen.state_mut().poll();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        screen.state().projects[0]
            .git_tracking
            .as_ref()
            .unwrap()
            .ahead,
        0
    );
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(&remote)
        .args(["rev-parse", &branch])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), head);
    screen
        .state()
        .sender
        .send(WorkerResult::GitStatus {
            root: repository.0.clone(),
            request_id: u64::MAX,
            result: Ok(Box::new(stale)),
        })
        .unwrap();
    screen.state_mut().poll();
    assert_eq!(
        screen.state().projects[0]
            .git_tracking
            .as_ref()
            .unwrap()
            .ahead,
        0
    );
    screen.run_steps(3);
    assert!(screen.query_by_label("Push 1 commit").is_none());
    assert!(screen.query_by_label("Commit staged changes").is_some());
}

#[test]
fn resume_coalesces_an_in_flight_read_and_waits_for_a_git_action() {
    let repository = HoverRepository::new();
    let mut app = repository.app();
    history_window_state(&mut app, false, false);
    app.refresh_visible_git();
    app.projects[0].git_loading = true;
    app.projects[0].git_status_request = Some(u64::MAX);
    history_window_state(&mut app, true, false);
    app.refresh_visible_git();
    app.refresh_visible_git();
    assert!(app.projects[0].git_refresh_pending);
    assert_eq!(app.projects[0].git_status_request, Some(u64::MAX));
    let snapshot = app.projects[0].git_history_snapshot.clone().unwrap();
    deliver_refresh(&mut app, snapshot, None);
    assert!(app.projects[0].git_loading);
    assert!(!app.projects[0].git_refresh_pending);
    wait_for_git_refresh(&mut app, 0);

    history_window_state(&mut app, false, false);
    app.refresh_visible_git();
    app.git_busy = Some(repository.0.clone());
    history_window_state(&mut app, true, false);
    app.refresh_visible_git();
    assert!(!app.projects[0].git_loading);
    app.git_busy = None;
    app.refresh_visible_git();
    assert!(app.projects[0].git_loading);
    wait_for_git_refresh(&mut app, 0);

    // Some native minimize/restore cycles render no inactive frame. The
    // restored focus event must still bypass the just-completed poll's timer.
    let input = egui::RawInput {
        focused: true,
        events: vec![
            egui::Event::WindowFocused(false),
            egui::Event::WindowFocused(true),
        ],
        ..Default::default()
    };
    app.ctx.run_ui(input, |_| {}).textures_delta.clear();
    app.refresh_visible_git();
    assert!(app.projects[0].git_loading);
    wait_for_git_refresh(&mut app, 0);
}

#[test]
fn hidden_history_catches_up_when_source_control_is_shown() {
    let repository = HoverRepository::new();
    let mut app = repository.app();
    history_window_state(&mut app, true, false);
    app.set_sidebar_view_visible(SidebarView::SourceControl, false);
    let previous = app.projects[0].recent_commits.clone();
    fs::write(repository.0.join("external.txt"), "new\n").unwrap();
    git::stage_all(&repository.0).unwrap();
    git::commit(&repository.0, "While hidden").unwrap();
    app.projects[0].last_git_refresh = Instant::now() - Duration::from_secs(30);
    app.refresh_git(0, false);
    wait_for_git_refresh(&mut app, 0);
    assert_eq!(app.projects[0].recent_commits, previous);

    app.set_sidebar_view_visible(SidebarView::SourceControl, true);
    app.projects[0].last_git_refresh = Instant::now() - Duration::from_secs(4);
    app.refresh_git(0, false);
    wait_for_git_refresh(&mut app, 0);
    assert_eq!(app.projects[0].recent_commits[0].subject, "While hidden");
}

#[test]
fn remote_ref_updates_refresh_history_decorations_without_a_new_local_commit() {
    let repository = HoverRepository::new();
    let mut app = repository.app();
    history_window_state(&mut app, true, false);
    assert!(
        app.projects[0]
            .recent_commits
            .iter()
            .all(|commit| !commit.on_remote)
    );
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(&repository.0)
        .args(["update-ref", "refs/remotes/origin/main", "HEAD"])
        .output()
        .unwrap();
    assert!(output.status.success());
    app.projects[0].last_git_refresh = Instant::now() - Duration::from_secs(4);
    app.refresh_git(0, false);
    wait_for_git_refresh(&mut app, 0);
    assert!(
        app.projects[0]
            .recent_commits
            .iter()
            .all(|commit| commit.on_remote)
    );
    assert!(
        app.projects[0].recent_commits[0]
            .refs
            .iter()
            .any(|reference| reference.name == "origin/main")
    );
}

#[test]
fn automatic_history_refresh_recovers_from_a_transient_error() {
    let repository = HoverRepository::new();
    let mut app = repository.app();
    history_window_state(&mut app, true, false);
    app.projects[0].git_history_error = Some("temporary failure".into());
    app.projects[0].last_git_refresh = Instant::now() - Duration::from_secs(4);
    app.refresh_git(0, false);
    wait_for_git_refresh(&mut app, 0);
    assert!(app.projects[0].git_history_error.is_none());
    assert_eq!(app.projects[0].recent_commits.len(), 2);
}

#[test]
fn automatic_history_refresh_retains_loaded_depth_and_can_continue_paging() {
    use std::io::Write;
    let repository = HoverRepository::new();
    let mut input = String::new();
    for mark in 1..=137 {
        let message = format!("History {mark}\n");
        input.push_str(&format!(
            "commit refs/heads/history\nmark :{mark}\ncommitter History Test <test@example.invalid> {} +0000\ndata {}\n{}",
            1_700_000_000 + mark, message.len(), message
        ));
        if mark > 1 {
            input.push_str(&format!("from :{}\n", mark - 1));
        }
        input.push('\n');
    }
    input.push_str("done\n");
    let mut child = std::process::Command::new("git")
        .arg("-C")
        .arg(&repository.0)
        .args(["fast-import", "--quiet", "--done"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let mut app = repository.app();
    history_window_state(&mut app, true, false);
    let snapshot = app.projects[0].git_history_snapshot.clone().unwrap();
    let page = git::history_page(&repository.0, &snapshot, 0, 120).unwrap();
    deliver_refresh(&mut app, snapshot, Some(page));
    let selected = app.projects[0].recent_commits[90].hash.clone();
    app.projects[0].git_view.selected_commit = Some(selected.clone());
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(&repository.0)
        .args([
            "update-ref",
            "refs/remotes/origin/history",
            "refs/heads/history",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    app.projects[0].last_git_refresh = Instant::now() - Duration::from_secs(4);
    app.refresh_git(0, false);
    wait_for_git_refresh(&mut app, 0);
    assert_eq!(app.projects[0].recent_commits.len(), 120);
    assert!(app.projects[0].git_history_has_more);
    assert_eq!(
        app.projects[0].git_view.selected_commit.as_ref(),
        Some(&selected)
    );
    app.load_more_git_history();
    let deadline = Instant::now() + Duration::from_secs(15);
    while app.projects[0].git_history_loading {
        assert!(Instant::now() < deadline);
        app.poll();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(app.projects[0].recent_commits.len(), 139);
    assert!(!app.projects[0].git_history_has_more);
    assert!(app.projects[0].git_history_error.is_none());
}

#[test]
fn replacing_visible_history_preserves_the_reading_position_and_top_follows_head() {
    for scrolled in [false, true] {
        let mut app = history_workspace(120);
        app.projects[0].git_history_has_more = false;
        let mut screen = history_screen(app);
        screen.run_steps(3);
        if scrolled {
            let first = screen.get_by_label("Commit 00").rect().center();
            screen.hover_at(first);
            screen.input_mut().events.push(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: Vec2::new(0.0, -1500.0),
                phase: egui::TouchPhase::Move,
                modifiers: egui::Modifiers::NONE,
            });
            // Let wheel smoothing finish before measuring a stationary reader.
            screen.run_steps(20);
            screen.input_mut().events.push(egui::Event::PointerGone);
            screen.run_steps(3);
        }
        let label = if scrolled { "Commit 65" } else { "Commit 00" };
        let before = screen.get_by_label(label).rect().top()
            - screen.get_by_label("History").rect().bottom();
        let mut replacement = vec![commit(500)];
        replacement.extend(screen.state().projects[0].recent_commits.clone());
        deliver_refresh(
            screen.state_mut(),
            git::HistorySnapshot::default(),
            Some(git::HistoryPage {
                commits: replacement,
                has_more: false,
            }),
        );
        screen.run_steps(3);
        assert!(screen.query_by_label("History changed · Refresh").is_none());
        let label = if scrolled { label } else { "Commit 500" };
        let after = screen.get_by_label(label).rect().top()
            - screen.get_by_label("History").rect().bottom();
        assert_eq!(after, before, "scrolled={scrolled}");
    }
}

#[test]
fn parent_repository_is_not_used_without_selection() {
    let repository = HoverRepository::new();
    let nested = repository.0.join("examples").join("tiny-project");
    fs::create_dir_all(&nested).unwrap();
    fs::write(repository.0.join("outside.txt"), "outside the project\n").unwrap();
    let mut app = VEditApp::empty(egui::Context::default());
    app.projects.push(Project::new(nested));
    app.refresh_git(0, true);
    wait_for_git_refresh(&mut app, 0);
    let project = &app.projects[0];
    assert!(project.git_root.is_none());
    assert!(project.git_branch.is_none());
    assert!(project.git_changes.is_empty());
    assert!(project.recent_commits.is_empty());
    assert_eq!(project.uncommitted_change_count(), None);
    assert!(project.git_parent_root.is_some());
    assert!(project.git_approved_parent.is_none());
}

#[test]
fn parent_repository_choice_is_persisted_and_can_be_revoked() {
    let repository = HoverRepository::new();
    let nested = repository.0.join("examples");
    fs::create_dir(&nested).unwrap();
    let mut app = VEditApp::empty(egui::Context::default());
    app.recovery_session_path = repository.0.join(".git/vedit-test-session.json");
    app.projects.push(Project::new(nested));
    app.refresh_git(0, true);
    wait_for_git_refresh(&mut app, 0);
    let mut screen = history_screen_with(app, true);
    screen.run_steps(2);
    assert!(screen.query_by_label("Initialize repository").is_none());
    assert!(screen.query_by_label("History").is_none());
    screen.state_mut().focus_git_content();
    screen.run_steps(2);
    let select = screen.get_by_label("Use parent repository");
    assert_eq!(
        screen
            .ctx
            .read_response(screen.ctx.memory(|m| m.focused()).unwrap())
            .unwrap()
            .rect,
        select.rect()
    );
    select.click();
    screen.run_steps(2);
    wait_for_git_refresh(screen.state_mut(), 0);
    assert!(screen.state().projects[0].git_root.is_some());
    assert_eq!(screen.state().projects[0].recent_commits.len(), 2);
    screen.run_steps(2);
    assert!(screen.query_by_label("Using parent repository").is_some());
    assert!(screen.query_by_label("History").is_some());
    let saved: Session =
        serde_json::from_slice(&serde_json::to_vec(&screen.state().session()).unwrap()).unwrap();
    let mut restored = VEditApp::empty(egui::Context::default());
    restored.restore_session_projects(saved.projects);
    restored.refresh_git(0, true);
    wait_for_git_refresh(&mut restored, 0);
    assert!(restored.projects[0].git_root.is_some());
    let generation = screen.state().projects[0].git_history_generation;
    screen.get_by_label("Repository actions").click();
    screen.run_steps(2);
    screen.get_by_label("Stop using parent repository").click();
    screen.run_steps(2);
    deliver_page(
        screen.state_mut(),
        generation,
        0,
        Ok(git::HistoryPage {
            commits: vec![commit(80)],
            has_more: false,
        }),
    );
    wait_for_git_refresh(screen.state_mut(), 0);
    assert!(screen.state().projects[0].git_root.is_none());
    assert!(screen.state().projects[0].recent_commits.is_empty());
    assert!(
        screen.state().session().projects[0]
            .git_approved_parent
            .is_none()
    );
    screen.run_steps(2);
    assert!(screen.query_by_label("Use parent repository").is_some());
    assert!(screen.query_by_label("History").is_none());
    // Wait for the final background session write before removing the fixture.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let saved = fs::read(&screen.state().recovery_session_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Session>(&bytes).ok());
        if saved.is_some_and(|s| s.projects[0].git_approved_parent.is_none()) {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn new_project_repository_replaces_the_selected_parents_history() {
    let repository = HoverRepository::new();
    let nested = repository.0.join("examples");
    fs::create_dir(&nested).unwrap();
    let mut app = VEditApp::empty(egui::Context::default());
    let mut project = Project::new(nested.clone());
    project.git_approved_parent = Some(repository.0.clone());
    app.projects.push(project);
    app.refresh_git(0, true);
    wait_for_git_refresh(&mut app, 0);
    assert_eq!(app.projects[0].recent_commits.len(), 2);
    git::initialize(&nested).unwrap();
    app.refresh_git(0, true);
    wait_for_git_refresh(&mut app, 0);
    let project = &app.projects[0];
    assert_eq!(
        fs::canonicalize(project.git_root.as_ref().unwrap()).unwrap(),
        fs::canonicalize(nested).unwrap()
    );
    assert!(project.git_parent_root.is_none());
    assert!(project.recent_commits.is_empty());
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
    assert!(screen.query_by_label(&commit.hash[..7]).is_some());
    for (label, expected) in [("+3", GREEN), ("−1", RED)] {
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
        "History Author · time unavailable",
        "2026-09-26",
        &hash[..7],
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
            result: Ok(Box::new(GitSnapshot {
                root: Some(repository.0.clone()),
                parent_root: None,
                branch: Some("main".into()),
                tracking: None,
                main_comparison: Ok(None),
                untracked_collapsed: false,
                files: Vec::new(),
                stats: git::WorkingTreeStats::default(),
                stats_error: None,
                history: Ok(GitHistoryRefresh {
                    snapshot: app.projects[0].git_history_snapshot.clone().unwrap(),
                    page: None,
                }),
            })),
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
        local_only: Default::default(),
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
}

#[test]
fn hidden_history_keeps_a_consistent_pageable_snapshot_until_reloaded() {
    let mut app = history_workspace(2);
    let original = git::HistorySnapshot {
        local_only: Default::default(),
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
        local_only: Default::default(),
        tips: vec![commit(99).hash],
        refs: vec![git::CommitRef {
            hash: commit(99).hash,
            name: "main".into(),
            kind: git::RefKind::Local,
            current: true,
        }],
    };

    deliver_refresh(&mut app, changed, None);
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
    assert!(!app.projects[0].git_history_has_more);
}

#[test]
fn replacing_history_invalidates_a_pending_page_even_at_the_same_offset() {
    let mut app = history_workspace(2);
    app.projects[0].git_history_loading = true;
    let old_generation = app.projects[0].git_history_generation;
    let replacement = vec![commit(100), commit(101)];
    let snapshot = git::HistorySnapshot {
        local_only: Default::default(),
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
    assert!(screen.query_by_label("Loading commits…").is_none());

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
    // The end of history needs no footer; only loading is announced.
    assert!(screen.query_by_label("Loading commits…").is_none());
}

#[test]
fn a_history_error_stops_scroll_requests_until_retry() {
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

fn git_cli_reply(app: &mut VEditApp, request: crate::cli::Request) -> crate::cli::Response {
    let (reply, receiver) = mpsc::channel();
    assert!(app.control_git_async(&request, reply));
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Ok(response) = receiver.try_recv() {
            return response;
        }
        assert!(Instant::now() < deadline, "Git CLI response timed out");
        app.poll();
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn git_cli_pages_branches_and_history_and_waits_for_safe_switch_completion() {
    use crate::cli::Request;
    let repo = HoverRepository::new();
    let result = std::process::Command::new("git")
        .arg("-C")
        .arg(&repo.0)
        .args(["branch", "feature/Überarbeitung"])
        .output()
        .unwrap();
    assert!(result.status.success());
    let mut app = repo.app();
    let first = git_cli_reply(
        &mut app,
        Request::GitBranches {
            workspace: None,
            limit: 1,
            offset: 0,
        },
    );
    assert!(first.ok, "{first:?}");
    assert_eq!(first.result["total"], 2);
    assert_eq!(first.result["next_offset"], 1);
    let second = git_cli_reply(
        &mut app,
        Request::GitBranches {
            workspace: None,
            limit: 1,
            offset: 1,
        },
    );
    assert!(second.ok && second.result["has_more"] == false);
    assert_ne!(
        first.result["items"][0]["ref"],
        second.result["items"][0]["ref"]
    );
    let target = "refs/heads/feature/Überarbeitung".to_owned();
    let history = git_cli_reply(
        &mut app,
        Request::GitHistory {
            workspace: None,
            limit: 1,
            offset: 0,
        },
    );
    assert!(history.ok && history.result["has_more"] == true);
    assert!(
        history.result["items"][0]["date"]
            .as_str()
            .unwrap()
            .contains('T')
    );
    assert_eq!(history.result["items"][0]["on_remote"], false);
    let rest = git_cli_reply(
        &mut app,
        Request::GitHistory {
            workspace: None,
            limit: 1,
            offset: 1,
        },
    );
    assert!(rest.ok && rest.result["has_more"] == false);
    let mut draft = EditorTab::new(None, "Draft".into());
    draft.text = "keep agent work".into();
    draft.changed();
    draft.agent_unsaved = true;
    app.projects[0].tabs.push(draft);
    let blocked = git_cli_reply(
        &mut app,
        Request::GitSwitchBranch {
            workspace: None,
            target: target.clone(),
        },
    );
    assert!(!blocked.ok);
    assert_eq!(blocked.error.unwrap().code, "git_switch_blocked");
    assert_eq!(app.projects[0].tabs.last().unwrap().text, "keep agent work");
    app.projects[0].tabs.pop();
    let switched = git_cli_reply(
        &mut app,
        Request::GitSwitchBranch {
            workspace: None,
            target,
        },
    );
    assert!(switched.ok, "{switched:?}");
    assert_eq!(switched.result["completed"], true);
    assert_eq!(switched.result["branch"], "feature/Überarbeitung");
    assert_eq!(
        git::status(&repo.0).unwrap().branch.as_deref(),
        Some("feature/Überarbeitung")
    );
    let missing = git_cli_reply(
        &mut app,
        Request::GitSwitchBranch {
            workspace: None,
            target: "refs/heads/missing".into(),
        },
    );
    assert!(!missing.ok);
    let invalid = git_cli_reply(
        &mut app,
        Request::GitSwitchBranch {
            workspace: None,
            target: "--force".into(),
        },
    );
    assert_eq!(invalid.error.unwrap().code, "invalid_request");
}

#[test]
fn history_tooltip_shows_exact_zoned_time_and_relative_age() {
    let mut app = history_workspace(1);
    let commit = &mut app.projects[0].recent_commits[0];
    commit.date = "2026-10-10T21:05:12+02:00".into();
    commit.timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
        - 720;
    let mut screen = history_screen(app);
    immediate_history_tooltips(&mut screen);
    screen.run_steps(3);
    screen.hover_at(screen.get_by_label("Commit 00").rect().center());
    screen.run_steps(4);
    assert!(screen.query_by_label("2026-10-10 21:05:12+02:00").is_some());
    assert!(
        screen
            .query_by_label_contains(" · 12 minutes ago")
            .is_some()
    );
}

#[test]
#[ignore = "requires VEDIT_CLI_TEST_BINARY pointing to the built vedit-cli executable"]
fn git_cli_executable_round_trip_in_isolated_editor() {
    let binary = std::env::var_os("VEDIT_CLI_TEST_BINARY").expect("set VEDIT_CLI_TEST_BINARY");
    let repo = HoverRepository::new();
    let control_directory = repo.0.join("isolated-control");
    let mut app = repo.app();
    app.control =
        Some(crate::cli::Server::start_test(app.ctx.clone(), control_directory.clone()).unwrap());
    let mut run = |args: &[&str]| {
        let mut child = std::process::Command::new(&binary)
            .args(args)
            .env("VEDIT_CONTROL_DIR", &control_directory)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(55);
        while child.try_wait().unwrap().is_none() {
            assert!(Instant::now() < deadline, "CLI process timed out");
            app.poll_control();
            app.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
        let output = child.wait_with_output().unwrap();
        assert!(output.stderr.is_empty());
        let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        println!(
            "{}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout).trim()
        );
        (output.status.success(), result)
    };
    let (ok, branches) = run(&["git-branches", "--limit", "1"]);
    assert!(ok);
    let target = branches["result"]["items"][0]["ref"].as_str().unwrap();
    let (ok, switched) = run(&["git-switch-branch", target]);
    assert!(ok && switched["result"]["completed"] == true);
    let (ok, first) = run(&["git-history", "--limit", "1"]);
    assert!(ok && first["result"]["has_more"] == true);
    let (ok, last) = run(&["git-history", "--limit", "1", "--offset", "1"]);
    assert!(ok && last["result"]["has_more"] == false);
    let (ok, missing) = run(&["git-switch-branch", "refs/heads/missing"]);
    assert!(!ok && missing["error"]["code"] == "git_switch_failed");
    let (ok, invalid) = run(&["git-switch-branch"]);
    assert!(!ok && invalid["error"]["code"] == "invalid_arguments");
}

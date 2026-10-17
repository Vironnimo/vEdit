use super::*;
use egui_kittest::{Harness, kittest::Queryable};

fn hash(character: char) -> String {
    character.to_string().repeat(40)
}

fn entry(path: &Path, branch: Option<&str>, head: Option<char>) -> git::WorktreeEntry {
    git::WorktreeEntry {
        path: path.to_path_buf(),
        head: head.map(hash),
        branch: branch.map(str::to_owned),
        bare: false,
        detached: branch.is_none(),
        locked: None,
        prunable: None,
    }
}

fn status(
    branch: Option<&str>,
    head: char,
    changes: usize,
    conflicts: usize,
    divergence: Option<(usize, usize)>,
) -> git::WorktreeStatus {
    git::WorktreeStatus {
        head: Some(hash(head)),
        tracking: git::BranchInfo {
            name: branch.map(str::to_owned),
            upstream: divergence.map(|_| format!("origin/{}", branch.unwrap_or("main"))),
            ahead: divergence.map_or(0, |(ahead, _)| ahead),
            behind: divergence.map_or(0, |(_, behind)| behind),
            divergence_known: divergence.is_some(),
        },
        changes,
        conflicts,
    }
}

fn headline(character: char, subject: &str, timestamp: i64) -> git::CommitHeadline {
    git::CommitHeadline {
        hash: hash(character),
        subject: subject.into(),
        author: "Alex Taylor".into(),
        timestamp,
    }
}

/// Apply a listing through the same request path a worker result takes.
fn list(
    overview: &mut RepositoryOverview,
    seed: &Path,
    entries: Vec<git::WorktreeEntry>,
    headlines: Vec<git::CommitHeadline>,
) -> u64 {
    let request = overview.start(Target::Listing(path_key(seed)), true, Instant::now());
    overview.finish(OverviewResult::Listing {
        request,
        result: Ok(Listing { entries, headlines }),
    });
    request
}

fn read(
    overview: &mut RepositoryOverview,
    path: &Path,
    result: Result<git::WorktreeStatus, String>,
) {
    let request = overview.start(Target::Status(path_key(path)), false, Instant::now());
    overview.finish(OverviewResult::Status {
        request,
        result: result.map(|status| StatusRead {
            status,
            headline: None,
        }),
    });
}

fn open<'a>(
    index: usize,
    current: bool,
    root: &Path,
    summary: Option<(&'a Path, &'a git::WorktreeStatus)>,
) -> OpenProject<'a> {
    OpenProject {
        index,
        current,
        root_key: path_key(root),
        repository_key: summary.map(|(repository, _)| path_key(repository)),
        repository: summary.map(|(repository, _)| repository),
        status: summary.map(|(_, status)| status),
        error: None,
    }
}

fn tooltip_screen(row: &Row, now: i64) -> Harness<'static, Row> {
    let mut setup = egui_wgpu::WgpuSetup::without_display_handle();
    if let egui_wgpu::WgpuSetup::CreateNew(options) = &mut setup {
        options.instance_descriptor.backends = egui_wgpu::wgpu::Backends::GL;
    }
    let mut screen = Harness::builder()
        .with_size([460.0, 560.0])
        .with_theme(egui::Theme::Dark)
        .with_os(egui::os::OperatingSystem::Windows)
        .wgpu_setup(setup)
        .build_ui_state(
            move |ui, row: &mut Row| {
                egui::Frame::popup(ui.style()).show(ui, |ui| repository_tooltip(ui, row, now));
            },
            row.clone(),
        );
    set_style(&screen.ctx);
    screen.run_steps(3);
    screen
}

fn assert_tooltip_labels(row: &Row, now: i64, labels: &[&str]) {
    let screen = tooltip_screen(row, now);
    for label in labels {
        assert!(
            screen.query_by_label(label).is_some(),
            "missing tooltip label: {label}"
        );
    }
}

#[test]
fn worktree_tooltip_screenshots() {
    let row = Row {
        key: "tooltip".into(),
        path: PathBuf::from("C:/Code/GTSystem/worktrees/agent-review"),
        name: "agent-review".into(),
        linked: true,
        project: None,
        current: false,
        kind: RowKind::Worktree,
        branch: Some("feature/review-panel".into()),
        head: Some(hash('a')),
        locked: None,
        status: Some(Ok(status(
            Some("feature/review-panel"),
            'a',
            7,
            0,
            Some((4, 1)),
        ))),
        details: Some(Ok(ChangeDetails {
            files: 7,
            added: 123,
            removed: 45,
            binary: 0,
            unknown: 0,
        })),
        headline: Some(headline(
            'a',
            "Make the review panel easier to scan",
            1_790_426_000,
        )),
        listing_error: None,
    };
    let mut screen = tooltip_screen(&row, 1_790_426_840);
    for (name, compact) in [
        ("vedit_worktree_tooltip", false),
        ("vedit_worktree_tooltip_compact", true),
    ] {
        if compact {
            screen.set_size(Vec2::new(330.0, 720.0));
            let row = screen.state_mut();
            row.path = PathBuf::from(
                "C:/Development/projects/GTSystem/worktrees/long-agent-review-folder",
            );
            row.locked = Some("Agent is applying changes".into());
            row.status.as_mut().unwrap().as_mut().unwrap().conflicts = 2;
            let details = row.details.as_mut().unwrap().as_mut().unwrap();
            details.binary = 1;
            details.unknown = 1;
        }
        screen.run_steps(3);
        if let Some(directory) = std::env::var_os("VEDIT_SNAPSHOT_DIR") {
            screen.snapshot_options(
                name,
                &egui_kittest::SnapshotOptions::new().output_path(PathBuf::from(directory)),
            );
        } else {
            screen.snapshot(name);
        }
    }
}

#[test]
fn path_keys_match_verbatim_mixed_separator_and_case_variants() {
    if cfg!(windows) {
        assert_eq!(
            path_key(Path::new(r"\\?\C:\Code\vEdit Überarbeitung\")),
            path_key(Path::new("c:/code/VEDIT ÜBERARBEITUNG"))
        );
        assert_eq!(path_key(Path::new(r"C:\")), r"c:\");
        assert_ne!(
            path_key(Path::new("C:/Code/vEdit")),
            path_key(Path::new("C:/Code/vEdit-agent"))
        );
    } else {
        assert_eq!(path_key(Path::new("/code/vedit/")), "/code/vedit");
        assert_eq!(path_key(Path::new("/")), "/");
    }
}

#[test]
fn compact_ages_stay_short_and_tolerate_clock_skew() {
    let now = 1_800_000_000;
    for (age, expected) in [
        (0, "now"),
        (59, "now"),
        (60, "1m"),
        (3_599, "59m"),
        (3_600, "1h"),
        (86_400 * 2, "2d"),
        (2_592_000 * 4, "4mo"),
        (31_536_000, "1y"),
        (-500, "now"),
    ] {
        assert_eq!(compact_age(now - age, now), expected, "{age}");
    }
    assert_eq!(compact_age(0, now), "");
}

#[test]
fn rows_group_worktrees_under_their_repository_in_project_order() {
    let base = Path::new("C:/Code");
    let main = base.join("vEdit");
    let agent = base.join("vEdit worktrees/agent Überarbeitung");
    let review = base.join("vEdit worktrees/Review");
    let missing = base.join("vEdit worktrees/gone");
    let notes = base.join("notes");
    let notes_examples = notes.join("examples");
    let mut overview = RepositoryOverview::default();
    let mut gone = entry(&missing, Some("old"), Some('9'));
    gone.prunable = Some("gitdir file points to non-existent location".into());
    let mut locked = entry(&review, None, Some('3'));
    locked.locked = Some("agent busy".into());
    list(
        &mut overview,
        &main,
        vec![
            entry(&main, Some("main"), Some('1')),
            entry(&agent, Some("agent/work"), Some('2')),
            locked,
            gone,
        ],
        vec![headline('2', "Teach the agent", 1_700_000_000)],
    );
    read(
        &mut overview,
        &agent,
        Ok(status(Some("agent/work"), '2', 3, 0, Some((2, 0)))),
    );
    read(&mut overview, &review, Err("index is corrupt".into()));
    // The main worktree is open through a project inside it; the second
    // project's repository has not been listed yet.
    let main_status = status(Some("main"), '1', 0, 1, Some((0, 0)));
    let notes_status = status(Some("trunk"), '4', 1, 0, None);
    let projects = [
        open(0, false, &notes_examples, Some((&notes, &notes_status))),
        open(1, true, &main.join("src"), Some((&main, &main_status))),
    ];
    let rows = overview.rows(&projects);
    let names: Vec<_> = rows.iter().map(|row| row.name.as_str()).collect();
    assert_eq!(
        names,
        ["notes", "vEdit", "agent Überarbeitung", "gone", "Review"],
        "groups follow the project tabs; worktrees sort beneath their repository"
    );
    assert_eq!(
        rows.iter().map(|row| row.linked).collect::<Vec<_>>(),
        [false, false, true, true, true]
    );
    let notes_row = &rows[0];
    assert_eq!(notes_row.path, notes);
    assert_eq!(notes_row.project, Some(0));
    assert!(!notes_row.current);
    let main_row = &rows[1];
    assert!(main_row.current);
    assert_eq!(main_row.project, Some(1));
    assert_eq!(
        main_row
            .status
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap()
            .conflicts,
        1,
        "open projects' own refreshes win over porcelain data"
    );
    let agent_row = &rows[2];
    assert_eq!(agent_row.project, None);
    assert_eq!(agent_row.branch.as_deref(), Some("agent/work"));
    assert_eq!(
        agent_row.headline.as_ref().unwrap().subject,
        "Teach the agent"
    );
    assert_eq!(
        sync_marker(agent_row.status.as_ref().unwrap().as_ref().unwrap()),
        Some(("↑2".to_owned(), MUTED))
    );
    assert_eq!(
        rows[3].kind,
        RowKind::Missing("gitdir file points to non-existent location".into())
    );
    assert!(rows[3].status.is_none() && !rows[3].openable());
    let review_row = &rows[4];
    assert_eq!(review_row.branch, None);
    assert_eq!(review_row.head, Some(hash('3')));
    assert_eq!(review_row.locked.as_deref(), Some("agent busy"));
    assert!(matches!(review_row.status, Some(Err(_))));
    assert_tooltip_labels(
        review_row,
        1_700_000_100,
        &[
            "Detached HEAD",
            "3333333",
            "Status unavailable",
            "index is corrupt",
            "Locked: agent busy",
            "Click to open as project",
        ],
    );
    assert_tooltip_labels(
        agent_row,
        1_700_000_100,
        &[
            "agent/work",
            "Tracking origin/agent/work",
            "↑ 2 to push",
            "↓ 0 to pull",
            "3 files changed",
            "Last commit",
            "2222222",
            "Teach the agent",
            "Alex Taylor · 1 minute ago",
        ],
    );
}

#[test]
fn bare_repositories_group_their_worktrees_without_status_reads() {
    let bare_path = Path::new("C:/Repos/project.git");
    let linked = Path::new("C:/Repos/project-main");
    let mut overview = RepositoryOverview::default();
    let mut bare = entry(bare_path, None, None);
    bare.bare = true;
    bare.detached = false;
    list(
        &mut overview,
        linked,
        vec![bare, entry(linked, Some("main"), Some('1'))],
        vec![headline('1', "Base", 1)],
    );
    let linked_status = status(Some("main"), '1', 0, 0, None);
    let rows = overview.rows(&[open(0, true, linked, Some((linked, &linked_status)))]);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].kind, RowKind::Bare);
    assert!(!rows[0].openable() && rows[0].project.is_none());
    assert!(rows[1].current && rows[1].linked);
    let seeds = vec![(path_key(linked), linked.to_path_buf())];
    let open_keys = HashMap::from([(path_key(linked), Some(hash('1')))]);
    let work = overview.plan(&seeds, &open_keys, Instant::now());
    assert!(
        work.is_empty(),
        "neither a bare nor an open worktree is read"
    );
}

#[test]
fn planning_reads_only_due_worktrees_that_are_not_open_within_the_budget() {
    let main = Path::new("C:/Code/vEdit");
    let linked: Vec<PathBuf> = (1..=4)
        .map(|index| PathBuf::from(format!("C:/Code/vEdit-{index}")))
        .collect();
    let mut overview = RepositoryOverview::default();
    let mut entries = vec![entry(main, Some("main"), Some('1'))];
    entries.extend(
        linked
            .iter()
            .map(|path| entry(path, Some("topic"), Some('2'))),
    );
    let mut gone = entry(Path::new("C:/Code/vEdit-gone"), Some("gone"), Some('3'));
    gone.prunable = Some(String::new());
    entries.push(gone);
    list(
        &mut overview,
        main,
        entries,
        vec![headline('1', "Base", 1), headline('2', "Topic", 1)],
    );
    let seeds = vec![(path_key(main), main.to_path_buf())];
    let open_keys = HashMap::from([(path_key(main), Some(hash('1')))]);
    let now = Instant::now();
    let mut reads = Vec::new();
    for _ in 0..4 {
        let work = overview.plan(&seeds, &open_keys, now);
        assert!(work.len() <= MAX_JOBS);
        for work in work {
            let Work::Status {
                request,
                path,
                cached_head,
            } = work
            else {
                panic!("a fresh listing is not due again");
            };
            assert_eq!(
                cached_head,
                Some(hash('2')),
                "known headlines are not re-read"
            );
            reads.push(path.clone());
            overview.finish(OverviewResult::Status {
                request,
                result: Ok(StatusRead {
                    status: status(Some("topic"), '2', 0, 0, None),
                    headline: None,
                }),
            });
        }
    }
    reads.sort();
    assert_eq!(reads, linked, "each worktree that is not open is read once");
    assert!(overview.plan(&seeds, &open_keys, now).is_empty(), "idle");
    // Folding pauses status reads; listings slow down.
    overview.collapsed = true;
    assert!(
        overview
            .plan(&seeds, &open_keys, now + STATUS_INTERVAL + LISTING_INTERVAL)
            .is_empty()
    );
    let work = overview.plan(&seeds, &open_keys, now + COLLAPSED_LISTING_INTERVAL);
    assert!(matches!(work.as_slice(), [Work::Listing { .. }]));
    overview.jobs.clear();
    overview.collapsed = false;
    // An action such as a commit or fetch makes everything due once.
    let later = Instant::now();
    overview.request_refresh();
    let open_keys = HashMap::from([(path_key(main), Some(hash('5')))]);
    let work = overview.plan(&seeds, &open_keys, later);
    assert_eq!(work.len(), MAX_JOBS);
    let Work::Listing { cached, extra, .. } = &work[0] else {
        panic!("listing first");
    };
    assert!(cached.contains(&hash('1')) && cached.contains(&hash('2')));
    assert_eq!(
        extra,
        &[hash('5')],
        "the open project's new head lacks a headline"
    );
}

#[test]
fn discovery_lists_one_uncovered_repository_at_a_time_and_backs_off_after_failure() {
    let first = Path::new("C:/Code/first");
    let second = Path::new("C:/Code/second");
    let seeds = vec![
        (path_key(first), first.to_path_buf()),
        (path_key(second), second.to_path_buf()),
    ];
    let open_keys: HashMap<String, Option<String>> =
        seeds.iter().map(|(key, _)| (key.clone(), None)).collect();
    let mut overview = RepositoryOverview::default();
    let now = Instant::now();
    let work = overview.plan(&seeds, &open_keys, now);
    let [Work::Listing { request, seed, .. }] = work.as_slice() else {
        panic!("one discovery listing at a time");
    };
    assert_eq!(seed, first);
    assert!(overview.plan(&seeds, &open_keys, now).is_empty());
    overview.finish(OverviewResult::Listing {
        request: *request,
        result: Err("not a repository".into()),
    });
    let work = overview.plan(&seeds, &open_keys, now);
    let [Work::Listing { seed, .. }] = work.as_slice() else {
        panic!("the next repository is discovered");
    };
    assert_eq!(seed, second);
    overview.jobs.clear();
    assert!(
        overview
            .plan(&seeds[..1], &open_keys, now + Duration::from_secs(1))
            .is_empty(),
        "a failed listing waits for the listing interval"
    );
    assert!(matches!(
        overview
            .plan(&seeds[..1], &open_keys, now + LISTING_INTERVAL)
            .as_slice(),
        [Work::Listing { .. }]
    ));
}

#[test]
fn stale_listing_and_status_results_never_replace_newer_ones() {
    let main = Path::new("C:/Code/vEdit");
    let linked = Path::new("C:/Code/vEdit-agent");
    let mut overview = RepositoryOverview::default();
    let older = overview.start(Target::Listing(path_key(main)), true, Instant::now());
    let newer = overview.start(Target::Listing(path_key(linked)), false, Instant::now());
    overview.finish(OverviewResult::Listing {
        request: newer,
        result: Ok(Listing {
            entries: vec![
                entry(main, Some("main"), Some('1')),
                entry(linked, Some("agent"), Some('2')),
            ],
            headlines: Vec::new(),
        }),
    });
    overview.finish(OverviewResult::Listing {
        request: older,
        result: Ok(Listing {
            entries: vec![entry(main, Some("main"), Some('0'))],
            headlines: Vec::new(),
        }),
    });
    assert_eq!(overview.repositories.len(), 1);
    assert_eq!(overview.repositories[0].entries.len(), 2);
    assert_eq!(overview.repositories[0].request, newer);
    // A failure after a good listing keeps the worktrees and notes the error.
    let failed = overview.start(Target::Listing(path_key(main)), false, Instant::now());
    overview.finish(OverviewResult::Listing {
        request: failed,
        result: Err("timed out".into()),
    });
    assert_eq!(overview.repositories[0].entries.len(), 2);
    assert_eq!(overview.repositories[0].error.as_deref(), Some("timed out"));

    let slow = overview.start(Target::Status(path_key(linked)), false, Instant::now());
    let fast = overview.start(Target::Status(path_key(linked)), false, Instant::now());
    overview.finish(OverviewResult::Status {
        request: fast,
        result: Ok(StatusRead {
            status: status(Some("agent"), '2', 4, 0, None),
            headline: Some(headline('2', "Fresh", 10)),
        }),
    });
    overview.finish(OverviewResult::Status {
        request: slow,
        result: Ok(StatusRead {
            status: status(Some("agent"), '1', 0, 0, None),
            headline: None,
        }),
    });
    let current = overview.statuses[&path_key(linked)]
        .result
        .as_ref()
        .unwrap();
    assert_eq!(
        (current.changes, current.head.clone()),
        (4, Some(hash('2')))
    );
    // Results without a registered request, for example from before a
    // repository was closed, are ignored.
    overview.finish(OverviewResult::Status {
        request: 9_999,
        result: Err("late".into()),
    });
    assert!(overview.statuses[&path_key(linked)].result.is_ok());
    assert_eq!(overview.headlines[&hash('2')].subject, "Fresh");
}

#[test]
fn closing_the_last_project_of_a_repository_forgets_its_worktrees() {
    let main = Path::new("C:/Code/vEdit");
    let linked = Path::new("C:/Code/vEdit-agent");
    let mut overview = RepositoryOverview::default();
    list(
        &mut overview,
        main,
        vec![
            entry(main, Some("main"), Some('1')),
            entry(linked, Some("agent"), Some('2')),
        ],
        Vec::new(),
    );
    read(
        &mut overview,
        linked,
        Ok(status(Some("agent"), '2', 1, 0, None)),
    );
    // The repository stays while any of its worktrees is open.
    let seeds = vec![(path_key(linked), linked.to_path_buf())];
    overview.plan(&seeds, &HashMap::new(), Instant::now());
    assert_eq!(overview.repositories.len(), 1);
    overview.plan(&[], &HashMap::new(), Instant::now());
    assert!(overview.repositories.is_empty() && overview.statuses.is_empty());
}

#[test]
fn git_actions_except_staging_make_the_overview_due() {
    let mut app = VEditApp::empty(egui::Context::default());
    for (message, refresh) in [
        ("Stage changes", false),
        ("Unstage changes", false),
        ("Create commit", true),
        ("Fetch", true),
        ("Switch branch", true),
    ] {
        app.repositories.refresh_requested = None;
        app.sender
            .send(WorkerResult::GitAction {
                reply: None,
                root: PathBuf::from("C:/vedit-no-open-project"),
                message: message.into(),
                result: Ok(String::new()),
            })
            .unwrap();
        app.poll();
        assert_eq!(
            app.repositories.refresh_requested.is_some(),
            refresh,
            "{message}"
        );
    }
}

#[test]
fn headline_cache_is_bounded() {
    let mut overview = RepositoryOverview::default();
    for index in 0..MAX_HEADLINES + 10 {
        overview.remember_headline(git::CommitHeadline {
            hash: format!("{index:040x}"),
            subject: String::new(),
            author: String::new(),
            timestamp: 0,
        });
    }
    assert_eq!(overview.headlines.len(), MAX_HEADLINES);
    assert!(!overview.headlines.contains_key(&format!("{:040x}", 0)));
    assert!(
        overview
            .headlines
            .contains_key(&format!("{:040x}", MAX_HEADLINES + 9))
    );
}

/// Unique temporary folders that are removed again; they never contain a
/// repository, so render-only fixtures cannot reach the user's checkouts.
struct Folders(PathBuf);

impl Folders {
    fn new(names: &[&str]) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir().join(format!(
            "vedit-repositories-ui-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        for name in names {
            fs::create_dir_all(base.join(name)).unwrap();
        }
        Self(fs::canonicalize(base).unwrap())
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Folders {
    fn drop(&mut self) {
        assert!(
            self.0
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("vedit-repositories-ui-")
        );
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn project_with_summary(root: &Path, status: git::WorktreeStatus) -> Project {
    let mut project = Project::new(root.to_path_buf());
    project.git_root = Some(root.to_path_buf());
    project.git_branch = status.tracking.name.clone();
    project.git_tracking = Some(status.tracking.clone());
    project.git_indicator_count = Some(status.changes);
    project.git_summary = Some(project_status::GitSummary {
        repository: root.to_path_buf(),
        status,
    });
    project.last_git_refresh = Instant::now() + Duration::from_secs(3600);
    project.last_git_indicator_refresh = Instant::now() + Duration::from_secs(3600);
    project
}

/// Two open worktrees of one repository plus one that is not open.
fn overview_app(folders: &Folders, with_unopened: bool) -> VEditApp {
    let main = folders.path("vEdit");
    let feature = folders.path("vEdit-feature");
    let agent = folders.path("agent wt");
    let mut app = VEditApp::empty(egui::Context::default());
    app.settings.autosave = false;
    app.set_sidebar_view_visible(SidebarView::Explorer, false);
    app.move_sidebar(SidebarView::SourceControl, sidebar_layout::Side::Left);
    app.projects.push(project_with_summary(
        &main,
        status(Some("main"), '1', 0, 0, Some((0, 0))),
    ));
    app.projects.push(project_with_summary(
        &feature,
        status(Some("feature"), '2', 3, 0, Some((1, 0))),
    ));
    let mut entries = vec![
        entry(&main, Some("main"), Some('1')),
        entry(&feature, Some("feature"), Some('2')),
    ];
    if with_unopened {
        entries.push(entry(&agent, Some("agent/run"), Some('3')));
    }
    list(
        &mut app.repositories,
        &main,
        entries,
        vec![
            headline('1', "Create the editor workspace", 1_790_000_000),
            headline('2', "Improve repository navigation", 1_790_420_000),
            headline('3', "Let the agent refactor search", 1_790_426_000),
        ],
    );
    if with_unopened {
        read(
            &mut app.repositories,
            &agent,
            Ok(status(Some("agent/run"), '3', 7, 0, Some((4, 1)))),
        );
    }
    app.bind_current_pane();
    app
}

fn screen(app: VEditApp) -> Harness<'static, VEditApp> {
    let mut screen = Harness::builder()
        .with_size([1200.0, 800.0])
        .with_os(egui::os::OperatingSystem::Windows)
        .build_ui_state(
            |ui, app: &mut VEditApp| {
                app.ctx = ui.ctx().clone();
                // Render-only: no disk, session or project Git refreshes.
                app.last_disk_check = Instant::now();
                app.last_session_save = Instant::now();
                for project in &mut app.projects {
                    project.last_git_refresh = Instant::now();
                    project.last_git_indicator_refresh = Instant::now();
                }
                app.draw_ui(ui);
            },
            app,
        );
    set_style(&screen.ctx);
    screen.ctx.all_styles_mut(|style| {
        style.animation_time = 0.0;
        style.scroll_animation = egui::style::ScrollAnimation::none();
    });
    screen.run_steps(3);
    screen
}

#[test]
fn switching_projects_scopes_worktrees_even_before_the_next_refresh() {
    let folders = Folders::new(&[
        "vEdit",
        "vEdit-feature",
        "agent wt",
        "other",
        "other-wt",
        "plain",
    ]);
    let mut app = overview_app(&folders, true);
    let other = folders.path("other");
    let other_wt = folders.path("other-wt");
    app.projects.push(project_with_summary(
        &other,
        status(Some("main"), '4', 0, 0, None),
    ));
    app.projects.push(Project::new(folders.path("plain")));
    list(
        &mut app.repositories,
        &other,
        vec![
            entry(&other, Some("main"), Some('4')),
            entry(&other_wt, Some("feature"), Some('4')),
        ],
        Vec::new(),
    );
    let mut screen = screen(app);
    assert!(screen.query_by_label("Worktree agent wt").is_some());
    assert!(screen.query_by_label("Worktree other-wt").is_none());
    screen.get_by_label("Project: other").click();
    screen.run_steps(2);
    assert!(screen.query_by_label("Worktree other-wt").is_some());
    assert!(screen.query_by_label("Worktree agent wt").is_none());
    screen.get_by_label("Project: plain").click();
    screen.run_steps(2);
    assert!(screen.state().repository_rows().is_empty());
    // A late result from the previous project cannot bring its rows back.
    list(
        &mut screen.state_mut().repositories,
        &other,
        vec![
            entry(&other, Some("main"), Some('4')),
            entry(&other_wt, Some("feature"), Some('4')),
        ],
        Vec::new(),
    );
    screen.run_steps(2);
    assert!(screen.state().repository_rows().is_empty());
    screen.get_by_label("Project: vEdit-feature").click();
    screen.run_steps(2);
    assert!(screen.query_by_label("Repository vEdit").is_some());
    assert!(screen.query_by_label("Worktree agent wt").is_some());
    assert!(screen.query_by_label("Worktree other-wt").is_none());
}

#[test]
fn overview_appears_only_with_more_than_one_worktree() {
    let folders = Folders::new(&["vEdit", "vEdit-feature"]);
    let mut app = overview_app(&folders, false);
    app.projects.truncate(1);
    // The only listed worktree besides the open one is gone from the listing.
    let main = folders.path("vEdit");
    list(
        &mut app.repositories,
        &main,
        vec![entry(&main, Some("main"), Some('1'))],
        Vec::new(),
    );
    let mut screen = screen(app);
    assert!(screen.query_by_label("Repositories").is_none());
    let feature = folders.path("vEdit-feature");
    list(
        &mut screen.state_mut().repositories,
        &main,
        vec![
            entry(&main, Some("main"), Some('1')),
            entry(&feature, Some("feature"), Some('2')),
        ],
        Vec::new(),
    );
    // Its status is fresh, so the closed worktree needs no read either.
    read(
        &mut screen.state_mut().repositories,
        &feature,
        Ok(status(Some("feature"), '2', 0, 0, None)),
    );
    screen.run_steps(2);
    assert!(screen.query_by_label("Repositories").is_some());
    assert!(screen.query_by_label("Repository vEdit").is_some());
    assert!(screen.query_by_label("Worktree vEdit-feature").is_some());
    assert!(
        screen.state().repositories.jobs.is_empty(),
        "fresh results start no Git process"
    );
}

#[test]
fn clicking_rows_switches_projects_and_opens_worktrees_as_projects() {
    let folders = Folders::new(&["vEdit", "vEdit-feature", "agent wt"]);
    let mut screen = screen(overview_app(&folders, true));
    assert_eq!(screen.state().active_project, 0);
    screen.get_by_label("Worktree vEdit-feature").click();
    screen.run_steps(2);
    assert_eq!(screen.state().active_project, 1);
    assert_eq!(screen.state().projects.len(), 2);
    screen.get_by_label("Worktree agent wt").click();
    screen.run_steps(2);
    let app = screen.state();
    assert_eq!(app.projects.len(), 3, "the existing add-project path ran");
    assert_eq!(app.active_project, 2);
    assert_eq!(app.projects[2].root, folders.path("agent wt"));
    // Clicking it again switches to the new project instead of adding twice.
    screen.get_by_label("Repository vEdit").click();
    screen.run_steps(2);
    screen.get_by_label("Worktree agent wt").click();
    screen.run_steps(2);
    assert_eq!(screen.state().projects.len(), 3);
    assert_eq!(screen.state().active_project, 2);
}

#[test]
fn keyboard_moves_through_rows_and_enter_activates_one() {
    let folders = Folders::new(&["vEdit", "vEdit-feature", "agent wt"]);
    let mut screen = screen(overview_app(&folders, true));
    let header = egui::Id::new(HEADER_ID);
    screen.ctx.memory_mut(|memory| memory.request_focus(header));
    screen.run_steps(1);
    assert!(screen.state().git_content_has_focus());
    // Header, vEdit, agent wt, vEdit-feature: linked worktrees sort by name.
    for _ in 0..3 {
        screen.key_press(Key::ArrowDown);
        screen.run_steps(1);
    }
    let focused = screen.ctx.memory(|memory| memory.focused());
    assert_eq!(
        focused,
        Some(row_id(&path_key(&folders.path("vEdit-feature"))))
    );
    screen.key_press(Key::Enter);
    screen.run_steps(2);
    assert_eq!(screen.state().active_project, 1);
}

#[test]
fn collapsing_hides_rows_pauses_reads_and_persists_in_the_session() {
    let folders = Folders::new(&["vEdit", "vEdit-feature", "agent wt"]);
    let mut screen = screen(overview_app(&folders, true));
    assert!(screen.query_by_label("Worktree agent wt").is_some());
    screen.get_by_label("Repositories").click();
    screen.run_steps(2);
    assert!(screen.state().repositories.collapsed);
    assert!(screen.query_by_label("Worktree agent wt").is_none());
    assert!(screen.query_by_label("Repositories").is_some());
    let session = screen.state().session();
    assert!(session.repositories_collapsed);
    let restored: Session = serde_json::from_slice(&serde_json::to_vec(&session).unwrap()).unwrap();
    assert!(restored.repositories_collapsed);
    let legacy: Session = serde_json::from_str(r#"{"projects":[]}"#).unwrap();
    assert!(!legacy.repositories_collapsed, "expanded by default");
    screen.get_by_label("Repositories").click();
    screen.run_steps(2);
    assert!(!screen.state().repositories.collapsed);
    assert!(screen.query_by_label("Worktree agent wt").is_some());
}

#[test]
fn hovering_a_worktree_shows_cached_file_and_line_counts() {
    let folders = Folders::new(&["vEdit", "vEdit-feature", "agent wt"]);
    let mut app = overview_app(&folders, true);
    let path = folders.path("agent wt");
    app.repositories.details.insert(
        path_key(&path),
        DetailsEntry {
            result: Ok(ChangeDetails {
                files: 7,
                added: 123,
                removed: 45,
                binary: 1,
                unknown: 1,
            }),
            refreshed: Instant::now(),
            request: 0,
        },
    );
    let mut screen = screen(app);
    let now = 1_790_426_840_i64;
    screen
        .ctx
        .data_mut(|data| data.insert_temp(egui::Id::new("git-history-test-clock"), now));
    screen.ctx.all_styles_mut(|style| {
        style.interaction.tooltip_delay = 0.0;
        style.interaction.tooltip_grace_time = 0.0;
        style.interaction.show_tooltips_only_when_still = false;
    });
    screen.hover_at(screen.get_by_label("Worktree agent wt").rect().center());
    screen.run_steps(4);
    for label in [
        "7 files changed",
        "+123",
        "−45",
        "Partial line counts · 1 file unavailable",
    ] {
        assert!(
            screen.query_by_label(label).is_some(),
            "missing hover label: {label}"
        );
    }
    assert!(screen.state().repositories.jobs.is_empty());
}

#[test]
fn visible_overview_settles_without_repainting_continuously() {
    let folders = Folders::new(&["vEdit", "vEdit-feature", "agent wt"]);
    let mut screen = screen(overview_app(&folders, true));
    assert!(screen.query_by_label("Repositories").is_some());
    let steps = screen
        .try_run()
        .expect("an idle repository overview must not request continuous repaints");
    assert!(steps <= 4, "{steps}");
    assert!(screen.state().repositories.jobs.is_empty());
}

#[test]
fn overview_screenshot() {
    let folders = Folders::new(&["vEdit", "vEdit-feature", "agent wt"]);
    let mut app = overview_app(&folders, true);
    let review = folders.path("review wt");
    let missing = folders.path("old experiment");
    let main = folders.path("vEdit");
    let feature = folders.path("vEdit-feature");
    let agent = folders.path("agent wt");
    let mut gone = entry(&missing, Some("experiment"), Some('5'));
    gone.prunable = Some(String::new());
    list(
        &mut app.repositories,
        &main,
        vec![
            entry(&main, Some("main"), Some('1')),
            entry(&feature, Some("feature"), Some('2')),
            entry(&agent, Some("agent/run"), Some('3')),
            entry(&review, None, Some('4')),
            gone,
        ],
        vec![headline(
            '4',
            "Review the overview layout with a long subject that must truncate",
            1_789_000_000,
        )],
    );
    read(
        &mut app.repositories,
        &review,
        Ok(status(None, '4', 2, 2, None)),
    );
    app.projects[1]
        .git_summary
        .as_mut()
        .unwrap()
        .status
        .tracking
        .divergence_known = false;
    let mut screen = visual_tests::harness(app);
    screen.ctx.data_mut(|data| {
        data.insert_temp(egui::Id::new("git-history-test-clock"), 1_790_426_840_i64)
    });
    visual_tests::snapshot(&mut screen, "vedit_git_repositories");

    // Pulling History up shrinks Changes to its header, then the repository
    // list, until History fills nearly the whole panel.
    let start = screen
        .get_by_label("Resize Changes and History")
        .rect()
        .center();
    let title = screen.get_by_label("Source control panel title").rect();
    screen.event(egui::Event::PointerMoved(start));
    screen.event(egui::Event::PointerButton {
        pos: start,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    screen.run_steps(1);
    let end = egui::pos2(start.x, title.bottom());
    screen.event(egui::Event::PointerMoved(end));
    screen.run_steps(2);
    screen.event(egui::Event::PointerButton {
        pos: end,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    screen.run_steps(4);
    visual_tests::snapshot(&mut screen, "vedit_git_history_pulled_up");
    let history = screen.get_by_label("History").rect();
    assert!(
        history.top() < title.bottom() + 200.0,
        "History stays low at {history:?}"
    );
    assert!(screen.get_by_label("Changes").rect().bottom() < history.top());
}

/// A real repository with seven linked worktrees created by `git worktree add`.
/// Identity, signing and hooks are configured only in this repository.
struct RealWorktrees {
    base: PathBuf,
    main: PathBuf,
    linked: Vec<PathBuf>,
}

fn run_git(directory: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

impl RealWorktrees {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir().join(format!(
            "vedit-repositories-real-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let main = base.join("vEdit main");
        let hooks = base.join("hooks");
        fs::create_dir_all(&main).unwrap();
        fs::create_dir_all(&hooks).unwrap();
        git::initialize(&main).unwrap();
        run_git(&main, &["symbolic-ref", "HEAD", "refs/heads/main"]);
        for (key, value) in [
            ("user.name", "vEdit Repositories Test"),
            ("user.email", "vedit-tests@example.invalid"),
            ("commit.gpgsign", "false"),
            ("core.autocrlf", "false"),
            ("core.hooksPath", hooks.to_str().unwrap()),
        ] {
            run_git(&main, &["config", key, value]);
        }
        fs::write(main.join("README.md"), "base\n").unwrap();
        git::stage_all(&main).unwrap();
        git::commit(&main, "Base commit").unwrap();
        let mut linked = Vec::new();
        for index in 1..=7 {
            let name = if index == 7 {
                "Überarbeitung".to_owned()
            } else {
                format!("agent {index}")
            };
            let path = base.join("worktrees").join(name);
            run_git(
                &main,
                &[
                    "worktree",
                    "add",
                    "-b",
                    &format!("agent/{index}"),
                    path.to_str().unwrap(),
                ],
            );
            linked.push(fs::canonicalize(path).unwrap());
        }
        Self {
            main: fs::canonicalize(main).unwrap(),
            base,
            linked,
        }
    }
}

impl Drop for RealWorktrees {
    fn drop(&mut self) {
        assert_eq!(self.base.parent(), Some(std::env::temp_dir().as_path()));
        let _ = fs::remove_dir_all(&self.base);
    }
}

/// Run every planned read synchronously for each simulated second and count
/// the Git processes they start.
fn simulate(
    overview: &mut RepositoryOverview,
    seeds: &[(String, PathBuf)],
    open: &HashMap<String, Option<String>>,
    start: Instant,
    seconds: u64,
) -> Vec<usize> {
    let _ = git::take_command_count();
    (0..seconds)
        .map(|second| {
            let now = start + Duration::from_secs(second);
            for work in overview.plan(seeds, open, now) {
                let result = work.run();
                overview.finish(result);
            }
            git::take_command_count()
        })
        .collect()
}

#[test]
fn hovered_worktree_details_count_both_comparisons_and_untracked_files() {
    let fixture = RealWorktrees::new();
    let path = &fixture.linked[0];
    fs::write(path.join("README.md"), "first\nsecond\n").unwrap();
    git::stage_all(path).unwrap();
    fs::write(path.join("README.md"), "first\nthird\nfourth\n").unwrap();
    fs::write(path.join("new ü.txt"), "new\ntext\n").unwrap();
    fs::write(path.join("binary.dat"), b"\0binary").unwrap();
    let details = read_change_details(path).unwrap();
    assert_eq!(
        details,
        ChangeDetails {
            files: 3,
            added: 6,
            removed: 2,
            binary: 1,
            unknown: 0,
        }
    );
    let mut overview = RepositoryOverview::default();
    list(
        &mut overview,
        &fixture.main,
        git::worktree_list(&fixture.main).unwrap(),
        Vec::new(),
    );
    let now = Instant::now();
    let work = overview.details_work(path, now).unwrap();
    assert!(
        overview.details_work(path, now).is_none(),
        "no duplicate hover job"
    );
    overview.finish(work.run());
    assert!(
        overview
            .details_work(path, now + Duration::from_secs(1))
            .is_none(),
        "reuse cached hover details"
    );
    let rows = overview.rows(&[]);
    let row = rows
        .iter()
        .find(|row| path_key(&row.path) == path_key(path))
        .unwrap();
    assert_tooltip_labels(
        row,
        1_800_000_000,
        &[
            "3 files changed",
            "+6",
            "−2",
            "1 binary file · no line counts",
        ],
    );
    // A new read becomes due, but still waits for a hover and the shared budget.
    let later = now + STATUS_INTERVAL;
    overview.start(Target::Status("busy-1".into()), false, later);
    overview.start(Target::Status("busy-2".into()), false, later);
    assert!(overview.details_work(path, later).is_none());
    overview.jobs.clear();
    assert!(overview.details_work(path, later).is_some());
}

#[test]
fn hover_details_mark_missing_counts_and_retry_errors_without_looping() {
    let files = vec![git::FileStatus {
        path: "too-large.txt".into(),
        original_path: None,
        index: git::FileChange::None,
        worktree: git::FileChange::Untracked,
    }];
    assert_eq!(
        change_details(&files, &git::WorkingTreeStats::default()).unknown,
        1
    );
    let mut overview = RepositoryOverview::default();
    let path = Path::new("C:/test-worktree");
    let now = Instant::now();
    let Some(Work::Details { request, .. }) = overview.details_work(path, now) else {
        panic!("expected hover read");
    };
    overview.finish(OverviewResult::Details {
        request,
        result: Err("repository unavailable".into()),
    });
    assert!(
        overview
            .details_work(path, now + Duration::from_secs(1))
            .is_none()
    );
    assert!(overview.details_work(path, now + STATUS_INTERVAL).is_some());
    // Duplicate/late responses cannot replace a newer result.
    let later_request = overview.start(Target::Details(path_key(path)), false, now);
    overview.finish(OverviewResult::Details {
        request: later_request,
        result: Ok(ChangeDetails::default()),
    });
    overview.finish(OverviewResult::Details {
        request,
        result: Err("old result".into()),
    });
    assert!(overview.details[&path_key(path)].result.is_ok());
}

#[test]
fn real_worktrees_group_under_one_repository_at_a_bounded_idle_cost() {
    let fixture = RealWorktrees::new();
    fs::write(fixture.linked[0].join("draft.txt"), "agent draft\n").unwrap();
    let main_status = git::worktree_status(&fixture.main).unwrap();
    let main_key = path_key(&fixture.main);
    let seeds = vec![(main_key.clone(), fixture.main.clone())];
    let open = HashMap::from([(main_key, main_status.head.clone())]);
    let mut overview = RepositoryOverview::default();
    let per_second = simulate(&mut overview, &seeds, &open, Instant::now(), 180);
    assert_eq!(
        per_second[0], 2,
        "discovery: one worktree list plus one log for the shared head"
    );
    assert!(per_second.iter().all(|count| *count <= MAX_JOBS));
    let warm_minute: usize = per_second[60..120].iter().sum();
    // Four cached listings (every 15 s) plus seven status reads twice (every 30 s).
    assert_eq!(warm_minute, 4 + 7 * 2, "{per_second:?}");

    let projects = [OpenProject {
        index: 0,
        current: true,
        root_key: path_key(&fixture.main),
        repository_key: Some(path_key(&fixture.main)),
        repository: Some(&fixture.main),
        status: Some(&main_status),
        error: None,
    }];
    let rows = overview.rows(&projects);
    assert_eq!(rows.len(), 8);
    assert_eq!(rows[0].name, "vEdit main");
    assert!(rows[0].current && !rows[0].linked);
    assert_eq!(rows[0].headline.as_ref().unwrap().subject, "Base commit");
    let draft = rows
        .iter()
        .find(|row| row.path == fixture.linked[0])
        .expect("linked worktree with a space");
    assert_eq!(draft.branch.as_deref(), Some("agent/1"));
    assert_eq!(draft.status.as_ref().unwrap().as_ref().unwrap().changes, 1);
    assert!(rows.iter().any(|row| row.name == "Überarbeitung"));
    assert!(
        rows[1..]
            .iter()
            .all(|row| row.linked && row.project.is_none())
    );

    // Folded, only a slower listing remains; status reads pause.
    overview.collapsed = true;
    let folded = simulate(
        &mut overview,
        &seeds,
        &open,
        Instant::now() + Duration::from_secs(600),
        120,
    );
    assert_eq!(folded[60..120].iter().sum::<usize>(), 1, "{folded:?}");
}

/// Manual measurement of one overview refresh cycle's wall time.
#[test]
#[ignore = "manual benchmark: run with --ignored --nocapture"]
fn benchmark_repository_overview() {
    let fixture = RealWorktrees::new();
    let cached: HashSet<String> = git::worktree_status(&fixture.main)
        .unwrap()
        .head
        .into_iter()
        .collect();
    let rounds = 20;
    let started = Instant::now();
    for _ in 0..rounds {
        let listing = read_listing(&fixture.main, &cached, Vec::new()).unwrap();
        assert_eq!(listing.entries.len(), 8);
        assert!(listing.headlines.is_empty());
    }
    let listing = started.elapsed() / rounds;
    let started = Instant::now();
    for _ in 0..rounds {
        for path in &fixture.linked {
            read_status(path, cached.iter().next().map(String::as_str)).unwrap();
        }
    }
    let status = started.elapsed() / (rounds * fixture.linked.len() as u32);
    eprintln!(
        "worktree list: {listing:?} per call; worktree status: {status:?} per call; \
         expanded minute with 7 unopened worktrees: {:?} of Git time",
        listing * 4 + status * 14
    );
}

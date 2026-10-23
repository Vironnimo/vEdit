//! Agents in several projects and worktrees at once: which project a CLI
//! request reaches, where opened files land, background presentation and
//! the notification inbox.
use super::*;
use crate::cli::{NotificationLevel, NotificationOpen, NotificationReview, TaskAction};

/// A temporary tree: A is a repository with a linked worktree A/wt nested
/// inside it, B is a plain project, C a plain folder and R a repository that
/// is not open.
struct Folders {
    root: PathBuf,
}

impl Folders {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "vedit-agent-projects-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let folders = Self {
            root: fs::canonicalize(root).unwrap(),
        };
        for (path, text) in [
            ("A/.git/HEAD", "ref: refs/heads/main\n"),
            ("A/main.md", "main\n"),
            ("A/wt/.git", "gitdir: ../.git/worktrees/wt\n"),
            ("A/wt/src/lib.md", "one\ntwo\nthree\n"),
            ("B/one.md", "one\ntwo\nthree\n"),
            ("B/two.md", "two\n"),
            ("C/loose.md", "loose\n"),
            ("R/.git/HEAD", "ref: refs/heads/main\n"),
            ("R/sub/x.md", "x\n"),
        ] {
            let path = folders.path(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
        folders
    }

    fn path(&self, relative: &str) -> PathBuf {
        relative
            .split('/')
            .fold(self.root.clone(), |path, part| path.join(part))
    }

    fn app(&self, projects: &[&str]) -> VEditApp {
        let mut app = VEditApp::empty(egui::Context::default());
        app.settings.autosave = false;
        app.recovery_session_path = self.path("session.json");
        for name in projects {
            let root = self.path(name);
            let mut project = Project::new(root.clone());
            project.index = Some(Arc::new(workspace::WorkspaceIndex::empty(root)));
            project.last_git_refresh = Instant::now();
            app.projects.push(project);
        }
        app.bind_current_pane();
        app
    }
}

impl Drop for Folders {
    fn drop(&mut self) {
        let temporary = fs::canonicalize(std::env::temp_dir()).unwrap();
        assert!(self.root.starts_with(temporary));
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn settle(app: &mut VEditApp) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while app
        .projects
        .iter()
        .flat_map(|project| &project.tabs)
        .any(|tab| tab.loading)
    {
        assert!(Instant::now() < deadline, "file loads did not complete");
        std::thread::sleep(Duration::from_millis(5));
        app.poll();
    }
}

fn open(app: &mut VEditApp, path: PathBuf, background: bool) -> Response {
    let response = app.control_request(Request::Open {
        files: vec![OpenFile::source(path)],
        background,
    });
    settle(app);
    response
}

fn root_of(app: &VEditApp, index: usize) -> String {
    crate::paths::display(&app.projects[index].root)
}

#[test]
fn project_commands_use_the_named_then_the_callers_then_the_visible_project() {
    let folders = Folders::new();
    let mut app = folders.app(&["A", "A/wt", "B"]);
    app.projects[0].run.output = "main output\n".into();
    app.projects[1].run.output = "worktree output\n".into();
    app.control_caller = Some(folders.path("A/wt/src"));
    assert_eq!(
        app.control_project_index(None).unwrap(),
        (1, "caller_directory")
    );
    let output = app.control_request(Request::Output {
        start_line: 1,
        start_column: 1,
        line_count: 10,
        workspace: None,
    });
    assert!(output.ok, "{:?}", output.error);
    assert_eq!(output.result["text"], "worktree output\n");
    assert_eq!(output.result["workspace"], root_of(&app, 1));
    assert_eq!(output.result["workspace_source"], "caller_directory");
    // Windows paths compare without regard to letter case.
    #[cfg(windows)]
    {
        let upper = crate::paths::display(&folders.path("A/wt/src")).to_uppercase();
        app.control_caller = Some(PathBuf::from(upper));
        assert_eq!(app.control_project_index(None).unwrap().0, 1);
    }
    // The parent repository's own folders stay with the parent project.
    app.control_caller = Some(folders.path("A"));
    assert_eq!(
        app.control_project_index(None).unwrap(),
        (0, "caller_directory")
    );
    // An explicit workspace wins over the caller's directory.
    let stop = app.control_request(Request::Task {
        action: TaskAction::Stop,
        workspace: Some(folders.path("B")),
    });
    assert!(stop.ok, "{:?}", stop.error);
    assert_eq!(stop.result["workspace"], root_of(&app, 2));
    assert_eq!(stop.result["workspace_source"], "request");
    assert_eq!(stop.result["running"], false);
    // Outside every project, the visible one is used.
    app.control_caller = Some(folders.path("C"));
    assert_eq!(
        app.control_project_index(None).unwrap(),
        (0, "active_project")
    );
    app.control_caller = None;
    assert_eq!(
        app.control_project_index(None).unwrap(),
        (0, "active_project")
    );
    let unknown = app
        .control_project_index(Some(&folders.path("C")))
        .unwrap_err();
    assert_eq!(unknown.error.unwrap().code, "unknown_workspace");
    // Each project keeps its own task state in status.
    let status = app.control_request(Request::Status {});
    assert_eq!(status.result["workspaces"][1]["task"]["status"], "Ready");
}

#[test]
fn files_use_open_projects_and_other_files_without_discovering_repositories() {
    let folders = Folders::new();
    let mut app = folders.app(&["A"]);
    assert!(open(&mut app, folders.path("A/main.md"), false).ok);
    assert!(open(&mut app, folders.path("A/wt/src/lib.md"), false).ok);
    assert_eq!(app.projects.len(), 1);
    assert_eq!(app.active_project, 0);
    assert!(open(&mut app, folders.path("R/sub/x.md"), false).ok);
    assert!(app.project().unwrap().is_other_files());
    assert!(open(&mut app, folders.path("C/loose.md"), false).ok);
    assert_eq!(app.projects.len(), 2);
    assert_eq!(app.project().unwrap().tabs.len(), 2);
    let status = app.control_request(Request::Status {});
    assert!(status.result["active_workspace"].is_null());
    assert_eq!(status.result["active_workspace_kind"], "other_files");
    assert_eq!(status.result["workspaces"].as_array().unwrap().len(), 1);
    assert_eq!(status.result["layout"]["workspace_kind"], "other_files");
}

#[test]
fn a_new_named_draft_joins_the_project_that_will_contain_it() {
    let folders = Folders::new();
    let mut app = folders.app(&["A", "B"]);
    let created = app.control_request(Request::New {
        title: None,
        text: String::new(),
        path: Some(folders.path("B/docs/new.md")),
    });
    assert!(created.ok, "{:?}", created.error);
    assert_eq!(app.active_project, 1);
    assert_eq!(app.projects[1].tabs.len(), 1);
    let nested = app.control_request(Request::New {
        title: None,
        text: String::new(),
        path: Some(folders.path("A/wt/src/new.md")),
    });
    assert!(nested.ok, "{:?}", nested.error);
    assert_eq!(app.projects[app.active_project].root, folders.path("A"));
    // A named draft outside open projects belongs to Other files.
    let outside = app.control_request(Request::New {
        title: None,
        text: String::new(),
        path: Some(folders.path("C/missing/new.md")),
    });
    assert!(outside.ok, "{:?}", outside.error);
    assert!(app.project().unwrap().is_other_files());
}

#[test]
fn background_open_keeps_the_users_project_document_and_cursor() {
    let folders = Folders::new();
    let mut app = folders.app(&["A", "B"]);
    assert!(open(&mut app, folders.path("A/main.md"), false).ok);
    let visible = app.tab().unwrap().id;
    // Another project: the file waits there; nothing visible changes.
    let other = open(&mut app, folders.path("B/two.md"), true);
    assert!(other.ok, "{:?}", other.error);
    assert_eq!(other.result["background"], true);
    assert_eq!(other.result["active_workspace"], root_of(&app, 0));
    assert_eq!(app.active_project, 0);
    assert_eq!(app.tab().unwrap().id, visible);
    assert_eq!(app.projects[1].tabs.len(), 1);
    assert!(!app.projects[1].tabs[0].loading);
    // The visible project: a new tab behind the current document.
    let same = open(&mut app, folders.path("A/wt/src/lib.md"), true);
    assert!(same.ok);
    assert_eq!(
        app.active_project, 0,
        "the existing project keeps its visible document"
    );
    assert_eq!(app.projects.len(), 2);
    assert_eq!(app.projects[0].tabs.len(), 2);
    let behind = app.control_request(Request::Open {
        files: vec![OpenFile::source(folders.path("A/.git/HEAD"))],
        background: true,
    });
    settle(&mut app);
    assert!(behind.ok);
    assert_eq!(app.tab().unwrap().id, visible);
    let added = app.projects[0].tabs.last().unwrap().id;
    assert!(app.current_pane_tab_ids().contains(&added));
    // The document on screen keeps its cursor; the item says so.
    app.tab_mut().unwrap().set_cursor(2..2);
    let mut file = OpenFile::source(folders.path("A/main.md"));
    file.location = Some(Location { line: 1, column: 3 });
    let kept = app.control_request(Request::Open {
        files: vec![file],
        background: true,
    });
    settle(&mut app);
    assert!(kept.ok);
    assert!(kept.result["items"][0]["note"].is_string());
    assert_eq!(app.tab().unwrap().selection, 2..2);
    assert!(app.tab().unwrap().pending_agent_location.is_none());
    // A document out of sight gets the location for when the user looks.
    let mut file = OpenFile::source(folders.path("B/two.md"));
    file.location = Some(Location { line: 1, column: 2 });
    let hidden = app.control_request(Request::Open {
        files: vec![file],
        background: true,
    });
    assert!(hidden.ok);
    assert!(hidden.result["items"][0]["note"].is_null());
    assert_eq!(app.active_project, 0);
}

#[test]
fn notifications_join_the_callers_project_inbox_without_changing_anything() {
    let folders = Folders::new();
    let mut app = folders.app(&["A", "B"]);
    app.projects[1].git_root = Some(folders.path("B"));
    let notify = |message: &str| Request::Notify {
        message: message.into(),
        title: Some("Tests".into()),
        level: NotificationLevel::Success,
        workspace: None,
        open: None,
        review: None,
    };
    // Notifications reach the inbox even while the user has a dialog open.
    app.settings_open = true;
    app.control_caller = Some(folders.path("B"));
    let first = app.control_request(notify("All tests pass"));
    assert!(first.ok, "{:?}", first.error);
    assert_eq!(first.result["workspace"], root_of(&app, 1));
    assert_eq!(first.result["workspace_source"], "caller_directory");
    assert_eq!(first.result["coalesced"], false);
    assert_eq!(first.result["unread_in_workspace"], 1);
    assert_eq!(first.result["flashed"], true);
    let repeat = app.control_request(notify("All tests pass"));
    assert_eq!(repeat.result["id"], first.result["id"]);
    assert_eq!(repeat.result["coalesced"], true);
    assert_eq!(repeat.result["repeat_count"], 2);
    assert_eq!(
        repeat.result["flashed"], false,
        "the taskbar flashes once per interval"
    );
    assert_eq!(app.active_project, 0);
    app.settings_open = false;
    // Without a caller directory the --open path chooses the project.
    app.control_caller = None;
    let open = app.control_request(Request::Notify {
        message: "Look at this".into(),
        title: None,
        level: NotificationLevel::Warning,
        workspace: None,
        open: Some(NotificationOpen {
            path: folders.path("B/one.md"),
            location: Some(Location { line: 2, column: 1 }),
        }),
        review: None,
    });
    assert_eq!(open.result["workspace_source"], "open_path");
    let status = app.control_request(Request::Status {});
    assert_eq!(status.result["notifications"]["unread"], 2);
    assert_eq!(status.result["workspaces"][0]["unread_notifications"], 0);
    assert_eq!(status.result["workspaces"][1]["unread_notifications"], 2);
    // A review needs a repository; nothing is recorded otherwise.
    let review = app.control_request(Request::Notify {
        message: "Review please".into(),
        title: None,
        level: NotificationLevel::Info,
        workspace: Some(folders.path("A")),
        open: None,
        review: Some(NotificationReview::Working),
    });
    assert_eq!(review.error.unwrap().code, "no_repository");
    let both = app.control_request(Request::Notify {
        message: "Both".into(),
        title: None,
        level: NotificationLevel::Info,
        workspace: None,
        open: Some(NotificationOpen {
            path: folders.path("B/one.md"),
            location: None,
        }),
        review: Some(NotificationReview::Working),
    });
    assert_eq!(both.error.unwrap().code, "invalid_request");
    assert_eq!(app.notifications.unread(), 2);
    assert!(
        app.session_dirty,
        "receiving a notification schedules persistence"
    );
    // Restart retains the inbox independently of which projects are open.
    app.flush_recovery();
    let saved: Session =
        serde_json::from_slice(&fs::read(&app.recovery_session_path).unwrap()).unwrap();
    assert_eq!(saved.notifications.unread(), 2);
    assert_eq!(saved.notifications.unread_for(&folders.path("B")), 2);
    assert_eq!(
        saved.notifications.status_json(&[])["unread_outside_open_projects"],
        2
    );
    // Restore into a fresh application just as startup does.
    app = folders.app(&["A"]);
    app.notifications = saved.notifications;
    assert!(!app.notifications.is_open());
    app.run_notification(first.result["id"].as_u64().unwrap());
    assert_eq!(
        app.notifications.unread(),
        2,
        "an unavailable project action reads nothing"
    );
    app.projects.push(Project::new(folders.path("B")));
    // Clicking the entry switches to its project and opens the file there.
    app.run_notification(open.result["id"].as_u64().unwrap());
    settle(&mut app);
    app.apply_pending_agent_locations();
    assert_eq!(app.active_project, 1);
    let tab = app.tab().unwrap();
    assert_eq!(
        tab.path.as_deref(),
        Some(folders.path("B/one.md").as_path())
    );
    assert_eq!(tab.selection.start, 4, "line 2 of the file");
    // Opening one action reads only its entry; the overview reads nothing.
    assert_eq!(app.notifications.unread(), 1);
    assert!(
        app.session_dirty,
        "opening an action persists its read state"
    );
    app.open_notifications();
    let ctx = app.ctx.clone();
    let mut output = ctx.run_ui(egui::RawInput::default(), |_| app.notifications_dialog());
    output.textures_delta.clear();
    assert_eq!(app.notifications.unread(), 1);
    let arrived_while_open = app.control_request(notify("Another result"));
    assert_eq!(arrived_while_open.result["unread"], 2);
    app.flush_recovery();
    let saved: Session =
        serde_json::from_slice(&fs::read(&app.recovery_session_path).unwrap()).unwrap();
    let mut restored = folders.app(&["B", "A"]);
    restored.notifications = saved.notifications;
    assert!(
        !restored.notifications.is_open(),
        "the open dialog is not restored"
    );
    let status = restored.control_request(Request::Status {});
    assert_eq!(status.result["notifications"]["total"], 3);
    assert_eq!(status.result["notifications"]["unread"], 2);
    assert_eq!(status.result["workspaces"][0]["unread_notifications"], 2);
    assert_eq!(status.result["workspaces"][1]["unread_notifications"], 0);
    let repeat = restored.control_request(notify("All tests pass"));
    assert_eq!(repeat.result["id"], first.result["id"]);
    assert_eq!(repeat.result["repeat_count"], 3);
    let next = restored.control_request(notify("After restart"));
    assert!(
        next.result["id"].as_u64().unwrap() > arrived_while_open.result["id"].as_u64().unwrap()
    );
    let legacy: Session = serde_json::from_str("{}").unwrap();
    assert!(
        legacy.notifications.items_empty(),
        "old sessions remain readable"
    );
}

#[test]
fn notifications_without_projects_schedule_background_session_save() {
    let folders = Folders::new();
    let mut app = folders.app(&[]);
    let response = app.control_request(Request::Notify {
        message: "Done without an open project".into(),
        title: None,
        level: NotificationLevel::Info,
        workspace: None,
        open: None,
        review: None,
    });
    assert!(response.ok);
    let ctx = app.ctx.clone();
    let mut delay = Duration::ZERO;
    for _ in 0..4 {
        app.last_session_save = Instant::now();
        let mut output = ctx.run_ui(egui::RawInput::default(), |_| app.autosave());
        delay = output.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
        output.textures_delta.clear();
    }
    assert!(delay > Duration::ZERO && delay <= Duration::from_secs(1));
    assert!(
        !app.recovery_session_path.exists(),
        "save is still debounced"
    );
    app.last_session_save = Instant::now() - Duration::from_secs(2);
    app.autosave();
    assert!(!app.session_dirty);
    assert!(app.recovery_writer.wait_idle(Duration::from_secs(2)));
    let saved: Session =
        serde_json::from_slice(&fs::read(&app.recovery_session_path).unwrap()).unwrap();
    assert_eq!(saved.notifications.unread(), 1);
}

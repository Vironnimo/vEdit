use super::*;
use egui_kittest::kittest::Queryable;

#[test]
fn project_launch_toolbar_and_dropdown_screenshots() {
    let fixture = Fixture::new();
    let root = fixture.0.join("launch-demo");
    fs::create_dir_all(&root).unwrap();
    let config = launch::decode(br#"{"profiles":[{"id":"web","name":"Web UI","program":"app.exe"},{"id":"desktop","name":"Desktop","program":"app.exe"},{"id":"headless","name":"Headless","program":"app.exe"}],"default":"web"}"#).unwrap();
    read_or_configure(
        &root,
        &Action::Configure {
            configuration: config.clone(),
            revision: 0,
        },
    )
    .unwrap();
    let mut app = VEditApp::empty(egui::Context::default());
    crate::app::visual_tests::workspace(&mut app);
    app.projects[0].root = root;
    app.projects[0].launch.configuration = config;
    app.projects[0].launch.selected = Some("web".into());
    app.projects[0].launch.status = "Completed".into();
    app.projects[0].launch.running_id = Some("web".into());
    app.projects[0].launch.exit_code = Some(0);
    app.projects[0].launch.output =
        "Web UI\nListening on http://localhost:3000\nApplication closed.\n".into();
    app.projects[0].launch.show_output = true;
    app.output_visible = true;
    let mut screen = crate::app::visual_tests::harness(app);
    crate::app::visual_tests::snapshot(&mut screen, "vedit_project_launch");
    screen.get_by_label("Launch option").click();
    screen.run_steps(2);
    crate::app::visual_tests::snapshot(&mut screen, "vedit_project_launch_options");
    screen.get_by_label("Headless").click();
    screen.run_steps(2);
    let until = Instant::now() + Duration::from_secs(5);
    while screen.state().projects[0].launch.selected.as_deref() != Some("headless") {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(5));
        screen.run_steps(1);
    }
    assert!(screen.state().projects[0].launch.task.is_none());
    screen.set_size(egui::vec2(760.0, 540.0));
    crate::app::visual_tests::snapshot(&mut screen, "vedit_project_launch_compact");
}

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "vedit-launch café & 'quoted' {}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    fn config(&self, config: &Configuration) -> u64 {
        read_or_configure(
            &self.0,
            &Action::Configure {
                configuration: config.clone(),
                revision: 0,
            },
        )
        .unwrap()
        .revision
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn configuration() -> Configuration {
    launch::decode(br#"{"profiles":[{"id":"web","name":"Web UI","program":"powershell.exe","args":["-NoProfile","-NonInteractive","-Command","[Console]::Out.WriteLine('ready'); Start-Sleep -Seconds 30"]},{"id":"desktop","name":"Desktop","program":"missing-vedit-launch-fixture.exe"}],"default":"web"}"#).unwrap()
}

#[cfg(windows)]
#[test]
fn launch_buttons_start_and_stop_selected_application() {
    let fixture = Fixture::new();
    fixture.config(&configuration());
    let mut app = VEditApp::empty(egui::Context::default());
    app.projects.push(Project::new(fixture.0.clone()));
    app.projects[0].launch.configuration = configuration();
    app.settings.autosave = false;
    let mut screen = crate::app::visual_tests::harness(app);
    screen.get_by_label("▶ Run").click();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        screen.run_steps(1);
        if screen.state().projects[0]
            .launch
            .output
            .lines()
            .any(|line| line == "ready")
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Play did not start the application"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    screen.get_by_label("■ Stop").click();
    loop {
        screen.run_steps(1);
        if screen.state().projects[0].launch.task.is_none() {
            break;
        }
        assert!(Instant::now() < deadline, "Stop did not finish");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(screen.state().projects[0].launch.status, "Stopped");
}

#[test]
fn launch_configuration_drafts_block_cli_replacement() {
    let fixture = Fixture::new();
    let revision = fixture.config(&configuration());
    let mut app = VEditApp::empty(egui::Context::default());
    let mut project = Project::new(fixture.0.clone());
    let mut tab = EditorTab::new(
        Some(fixture.0.join(launch::CONFIG_PATH)),
        "launch.json".into(),
    );
    tab.loading = false;
    tab.dirty = true;
    project.tabs.push(tab);
    app.projects.push(project);
    let response = action(
        &mut app,
        0,
        Action::Configure {
            configuration: Configuration::default(),
            revision,
        },
    );
    assert_eq!(response.error.unwrap().code, "unsaved_configuration");
    assert_eq!(
        read_or_configure(&fixture.0, &Action::List)
            .unwrap()
            .configuration
            .unwrap()
            .profiles
            .len(),
        2
    );
}

fn action(app: &mut VEditApp, index: usize, action: Action) -> Response {
    let (sender, receiver) = mpsc::channel();
    app.begin_launch_action(index, action, Some(sender));
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(response) = receiver.try_recv() {
            return response;
        }
        assert!(Instant::now() < until, "launch action timed out");
        app.poll_launches();
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn wait(app: &mut VEditApp, condition: impl Fn(&VEditApp) -> bool) {
    let until = Instant::now() + Duration::from_secs(10);
    while !condition(app) {
        assert!(Instant::now() < until, "application lifecycle timed out");
        app.poll_launches();
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn launch_configuration_replaces_atomically_and_refuses_stale_writers() {
    let fixture = Fixture::new();
    let mut config = configuration();
    let revision = fixture.config(&config);
    let original = fs::read(fixture.0.join(launch::CONFIG_PATH)).unwrap();
    config.profiles[1].name = "Desktop 東京".into();
    assert!(
        read_or_configure(
            &fixture.0,
            &Action::Configure {
                configuration: config.clone(),
                revision: 0
            }
        )
        .is_err()
    );
    assert_eq!(
        fs::read(fixture.0.join(launch::CONFIG_PATH)).unwrap(),
        original
    );
    let updated = read_or_configure(
        &fixture.0,
        &Action::Configure {
            configuration: config.clone(),
            revision,
        },
    )
    .unwrap();
    assert_ne!(updated.revision, revision);
    assert_eq!(updated.configuration.unwrap(), config);
    assert!(
        read_or_configure(
            &fixture.0,
            &Action::Configure {
                configuration: config,
                revision
            }
        )
        .is_err()
    );
}

#[test]
fn malformed_launch_configuration_can_be_repaired_with_its_revision() {
    let fixture = Fixture::new();
    fixture.config(&configuration());
    fs::write(fixture.0.join(launch::CONFIG_PATH), "{bad json}").unwrap();
    let loaded = read_or_configure(&fixture.0, &Action::List).unwrap();
    assert!(loaded.configuration.is_err());
    assert!(
        read_or_configure(
            &fixture.0,
            &Action::Configure {
                configuration: configuration(),
                revision: loaded.revision
            }
        )
        .is_ok()
    );
}

#[test]
fn launch_selection_survives_session_and_removed_profiles_fall_back() {
    let fixture = Fixture::new();
    fixture.config(&configuration());
    let mut app = VEditApp::empty(egui::Context::default());
    app.projects.push(Project::new(fixture.0.clone()));
    assert!(
        action(
            &mut app,
            0,
            Action::Select {
                id: "desktop".into()
            }
        )
        .ok
    );
    let session = app.session();
    assert_eq!(
        session.projects[0].launch_selected.as_deref(),
        Some("desktop")
    );
    let mut restored = VEditApp::empty(egui::Context::default());
    restored.restore_session_projects(session.projects);
    assert_eq!(
        restored.projects[0].launch.selected.as_deref(),
        Some("desktop")
    );
    let state = &mut app.projects[0].launch;
    state.configuration.profiles.pop();
    assert_eq!(
        state
            .configuration
            .selected(state.selected.as_deref())
            .unwrap()
            .id,
        "web"
    );
    let failure = action(
        &mut app,
        0,
        Action::Select {
            id: "unknown".into(),
        },
    );
    assert_eq!(failure.error.unwrap().code, "unknown_launch");
}

#[test]
fn launch_blocks_unsaved_buffers_and_never_saves_a_draft() {
    let fixture = Fixture::new();
    fixture.config(&configuration());
    let mut app = VEditApp::empty(egui::Context::default());
    app.projects.push(Project::new(fixture.0.clone()));
    let mut tab = EditorTab::new(None, "Draft".into());
    tab.dirty = true;
    tab.agent_unsaved = true;
    app.projects[0].tabs.push(tab);
    let response = action(&mut app, 0, Action::Start { id: None });
    assert_eq!(response.error.unwrap().code, "unsaved_project");
    assert!(app.projects[0].launch.task.is_none());
    assert!(app.projects[0].tabs[0].agent_unsaved);
}

#[cfg(windows)]
#[test]
fn launch_runs_per_project_without_active_files_and_stops_then_restarts() {
    let first = Fixture::new();
    let second = Fixture::new();
    first.config(&configuration());
    second.config(&configuration());
    let mut app = VEditApp::empty(egui::Context::default());
    app.projects.push(Project::new(first.0.clone()));
    app.projects.push(Project::new(second.0.clone()));
    for index in 0..2 {
        assert!(action(&mut app, index, Action::Start { id: None }).ok);
    }
    wait(&mut app, |app| {
        app.projects
            .iter()
            .all(|p| p.launch.output.lines().any(|line| line == "ready"))
    });
    assert!(app.task.is_none());
    assert_eq!(
        action(&mut app, 0, Action::Start { id: None })
            .error
            .unwrap()
            .code,
        "launch_running"
    );
    assert!(action(&mut app, 0, Action::Stop).ok);
    wait(&mut app, |app| app.projects[0].launch.task.is_none());
    assert!(app.projects[1].launch.task.is_some());
    assert_eq!(app.projects[0].launch.status, "Stopped");
    assert!(action(&mut app, 0, Action::Start { id: None }).ok);
    wait(&mut app, |app| {
        app.projects[0]
            .launch
            .output
            .lines()
            .any(|line| line == "ready")
    });
    for index in 0..2 {
        assert!(action(&mut app, index, Action::Stop).ok);
    }
    wait(&mut app, |app| {
        app.projects.iter().all(|p| p.launch.task.is_none())
    });
}

#[test]
fn launch_reports_spawn_errors_and_cancels_pending_start() {
    let fixture = Fixture::new();
    fixture.config(&configuration());
    let mut app = VEditApp::empty(egui::Context::default());
    app.projects.push(Project::new(fixture.0.clone()));
    assert!(
        action(
            &mut app,
            0,
            Action::Start {
                id: Some("desktop".into())
            }
        )
        .ok
    );
    wait(&mut app, |app| app.projects[0].launch.task.is_none());
    assert_eq!(app.projects[0].launch.status, "Failed");
    assert!(app.projects[0].launch.error.is_some());
    app.begin_launch_action(
        0,
        Action::Start {
            id: Some("web".into()),
        },
        None,
    );
    assert!(action(&mut app, 0, Action::Stop).ok);
    app.poll_launches();
    assert!(app.projects[0].launch.task.is_none());
}

#[test]
fn launch_rolling_output_keeps_recent_unicode_and_bounds_lines() {
    let mut state = LaunchState::default();
    state.append(&"東京".repeat(tasks::MAX_OUTPUT_BYTES));
    state.append("\nlatest 😀");
    assert!(state.output.ends_with("latest 😀"));
    assert!(state.output.len() <= tasks::MAX_OUTPUT_BYTES);
    assert!(state.output_truncated);
    state.append(&"x\n".repeat(20_000));
    assert!(state.output.lines().count() <= tasks::MAX_OUTPUT_LINES + 1);
}

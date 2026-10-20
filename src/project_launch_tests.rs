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
    app.projects[0]
        .launch
        .output
        .append("Web UI\nListening on http://localhost:3000\nApplication closed.\n");
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
            .joined()
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
            .all(|p| p.launch.output.joined().lines().any(|line| line == "ready"))
    });
    assert!(
        app.projects
            .iter()
            .all(|project| project.run.task.is_none())
    );
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
            .joined()
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
    let mut log = OutputLog::default();
    log.append(&"東京".repeat(tasks::MAX_OUTPUT_BYTES));
    log.append("\nlatest 😀");
    assert_eq!(log.joined(), "latest 😀");
    assert_eq!(log.first, 2);
    assert!(log.truncated());
    log.append(&"x\n".repeat(20_000));
    let joined = log.joined();
    assert_eq!(joined.len(), log.bytes);
    assert!(log.bytes <= tasks::MAX_OUTPUT_BYTES);
    assert_eq!(log.lines.len(), tasks::MAX_OUTPUT_LINES);
    assert!(joined.lines().count() <= tasks::MAX_OUTPUT_LINES + 1);
    assert!(log.partial.is_empty());
    // "東京…" was line 1; "latest 😀x" is line 2.
    assert_eq!(log.last_line(), 20_001);
    assert_eq!(log.first, 20_001 - tasks::MAX_OUTPUT_LINES + 1);
}

/// Everything a run wrote, untrimmed, with the start of each absolute line.
struct Written {
    text: String,
    starts: Vec<usize>,
}

impl Written {
    fn new() -> Self {
        Self {
            text: String::new(),
            starts: vec![0],
        }
    }

    fn write(&mut self, log: &mut OutputLog, text: &str) {
        let base = self.text.len();
        self.starts
            .extend(text.match_indices('\n').map(|(index, _)| base + index + 1));
        self.text.push_str(text);
        log.append(text);
    }

    /// Byte offset of an absolute one-based position.
    fn offset(&self, line: usize, column: usize) -> usize {
        let start = self.starts[line - 1];
        self.text[start..]
            .char_indices()
            .map(|(index, _)| start + index)
            .chain([self.text.len()])
            .nth(column - 1)
            .unwrap()
    }

    /// A page is the exact run text at its absolute position, and its
    /// continuation resumes immediately after it.
    fn assert_exact(&self, page: &Value) {
        let number = |key: &str| page[key].as_u64().unwrap() as usize;
        let start = self.offset(number("start_line"), number("start_column"));
        let text = page["text"].as_str().unwrap();
        assert_eq!(&self.text[start..start + text.len()], text);
        if !page["next_line"].is_null() {
            assert_eq!(
                self.offset(number("next_line"), number("next_column")),
                start + text.len()
            );
        }
    }
}

fn page(log: &OutputLog, line: usize, column: usize, count: usize) -> Value {
    log.page(line, column, count, 48 * 1024).unwrap()
}

#[test]
fn launch_output_continuation_stays_absolute_while_old_lines_are_trimmed() {
    let mut log = OutputLog::default();
    let mut reference = Written::new();
    for line in 1..=100 {
        reference.write(&mut log, &format!("line {line}\n"));
    }
    let first = page(&log, 1, 1, 10);
    assert_eq!(
        first["text"],
        (1..=10).map(|n| format!("line {n}\n")).collect::<String>()
    );
    assert_eq!(first["next_line"], 11);
    assert_eq!(first["total_lines"], 100);
    assert_eq!(first["first_line"], 1);
    assert_eq!(first["skipped_lines"], 0);
    for line in 101..=tasks::MAX_OUTPUT_LINES + 50 {
        reference.write(&mut log, &format!("line {line}\n"));
    }
    // Line 11 was trimmed: the page starts at the oldest retained line instead.
    let resumed = page(&log, 11, 1, 10);
    reference.assert_exact(&resumed);
    assert_eq!(resumed["first_line"], 51);
    assert_eq!(resumed["start_line"], 51);
    assert_eq!(resumed["start_column"], 1);
    assert_eq!(resumed["skipped_lines"], 40);
    assert_eq!(resumed["end_line"], 60);
    assert_eq!(resumed["next_line"], 61);
    assert_eq!(resumed["total_lines"], tasks::MAX_OUTPUT_LINES + 50);
    assert!(resumed["text"].as_str().unwrap().starts_with("line 51\n"));
    // Following continuations neither repeat nor skip absolute lines.
    let mut line = 61;
    let mut seen = 60;
    while let Some(next) = {
        let current = page(&log, line, 1, 500);
        reference.assert_exact(&current);
        assert_eq!(current["skipped_lines"], 0);
        let text = current["text"].as_str().unwrap();
        for (index, row) in text.lines().enumerate() {
            assert_eq!(row, format!("line {}", line + index));
        }
        seen += text.lines().count();
        current["next_line"].as_u64()
    } {
        line = next as usize;
    }
    assert_eq!(seen, tasks::MAX_OUTPUT_LINES + 50);
}

#[test]
fn launch_output_page_after_the_last_line_is_empty_until_more_output_arrives() {
    let mut log = OutputLog::default();
    let empty = page(&log, 1, 1, 100);
    assert_eq!(empty["text"], "");
    assert_eq!(empty["total_lines"], 1);
    assert!(empty["next_line"].is_null());
    log.append("a\nb\n");
    let end = page(&log, 3, 1, 100);
    assert_eq!(end["text"], "");
    assert_eq!(end["start_line"], 3);
    assert_eq!(end["end_line"], 2);
    assert_eq!(end["total_lines"], 2);
    assert_eq!(end["first_line"], 1);
    assert!(end["next_line"].is_null());
    assert!(log.page(4, 1, 100, 1024).is_err());
    assert!(log.page(3, 2, 100, 1024).is_err());
    assert!(log.page(0, 1, 100, 1024).is_err());
    assert!(log.page(2, 3, 100, 1024).is_err());
    log.append("c\npartial");
    let new = page(&log, 3, 1, 100);
    assert_eq!(new["text"], "c\npartial");
    assert_eq!(new["total_lines"], 4);
    assert!(new["next_line"].is_null());
    assert_eq!(page(&log, 5, 1, 100)["text"], "");
}

#[test]
fn launch_output_trims_whole_lines_by_bytes_and_keeps_crlf_unicode_columns() {
    let mut log = OutputLog::default();
    let mut reference = Written::new();
    let long = "é".repeat(4096);
    for index in 0..300 {
        reference.write(&mut log, &format!("{index:03} {long}\r\n"));
    }
    assert!(log.truncated());
    assert!(log.bytes <= tasks::MAX_OUTPUT_BYTES);
    assert_eq!(log.joined().len(), log.bytes);
    assert_eq!(log.first_column_offset, 0);
    let first = log.first;
    assert!(first > 1);
    assert!(log.joined().starts_with(&format!("{:03} ", first - 1)));
    // Byte-limited pages split lines at characters, never between CR and LF.
    let mut line = first;
    let mut column = 1;
    let mut text = String::new();
    loop {
        let current = log.page(line, column, 2, 1000).unwrap();
        reference.assert_exact(&current);
        text.push_str(current["text"].as_str().unwrap());
        let Some(next) = current["next_line"].as_u64() else {
            break;
        };
        line = next as usize;
        column = current["next_column"].as_u64().unwrap() as usize;
    }
    assert_eq!(text, log.joined());

    let mut log = OutputLog::default();
    log.append("東京 😀\r");
    log.append("\nnext\r\n");
    let unicode = page(&log, 1, 3, 1);
    assert_eq!(unicode["text"], " 😀\r\n");
    assert_eq!(unicode["next_line"], 2);
    assert_eq!(page(&log, 1, 5, 1)["text"], "\r\n");
    assert!(log.page(1, 6, 1, 1024).is_err());
}

#[test]
fn launch_output_oversized_line_keeps_its_tail_with_absolute_columns() {
    let mut log = OutputLog::default();
    let mut reference = Written::new();
    reference.write(&mut log, "header\n");
    let chunk = "é".repeat(64 * 1024);
    for _ in 0..12 {
        reference.write(&mut log, &chunk);
    }
    assert_eq!(log.first, 2);
    assert!(log.lines.is_empty());
    assert!(log.bytes <= tasks::MAX_OUTPUT_BYTES);
    assert!(log.bytes > tasks::MAX_OUTPUT_BYTES - 4);
    let trimmed = log.first_column_offset;
    assert_eq!(trimmed, 12 * 64 * 1024 - log.partial.chars().count());
    // A continuation inside the retained tail stays exact while the head is cut.
    let before = log.page(2, trimmed + 1, 1, 4096).unwrap();
    reference.assert_exact(&before);
    assert_eq!(before["byte_limit_reached"], true);
    assert_eq!(before["next_line"], 2);
    let column = before["next_column"].as_u64().unwrap() as usize;
    reference.write(&mut log, &"é".repeat(1024));
    assert_eq!(log.first_column_offset, trimmed + 1024);
    let after = log.page(2, column, 1, 4096).unwrap();
    reference.assert_exact(&after);
    assert_eq!(after["start_column"], column);
    // A start inside the trimmed head begins at the first retained character.
    let head = page(&log, 1, 1, 1);
    reference.assert_exact(&head);
    assert_eq!(head["start_line"], 2);
    assert_eq!(head["start_column"], trimmed + 1025);
    assert_eq!(head["skipped_lines"], 1);
    reference.write(&mut log, "\ndone\n");
    assert_eq!(log.first, 3);
    assert_eq!(log.first_column_offset, 0);
    reference.assert_exact(&page(&log, 3, 1, 1));
}

#[test]
fn launch_output_many_small_chunks_stay_bounded_and_join_lazily() {
    let mut log = OutputLog::default();
    for index in 0..30_000 {
        log.append("x");
        log.append(if index % 2 == 0 { "\n" } else { "\r\n" });
    }
    assert_eq!(log.lines.len(), tasks::MAX_OUTPUT_LINES);
    assert_eq!(log.first, 30_000 - tasks::MAX_OUTPUT_LINES + 1);
    assert_eq!(log.last_line(), 30_000);
    assert!(log.lines.iter().all(|line| line.len() <= 3));
    assert_eq!(
        log.lines.iter().map(String::len).sum::<usize>() + log.partial.len(),
        log.bytes
    );
    // The display text is rebuilt once after changes, not per chunk or frame.
    assert!(log.display_stale);
    let pointer = log.text().as_ptr();
    assert!(!log.display_stale);
    assert_eq!(log.text().as_ptr(), pointer);
    let joined = log.joined();
    assert_eq!(log.text(), joined);
    log.append("y");
    assert!(log.display_stale);
    assert!(log.text().ends_with("x\r\ny"));
}

#[test]
fn launch_start_resets_output_numbering_and_cli_pages_report_it() {
    let fixture = Fixture::new();
    fixture.config(&configuration());
    let mut app = VEditApp::empty(egui::Context::default());
    app.projects.push(Project::new(fixture.0.clone()));
    app.projects[0]
        .launch
        .output
        .append(&"old\n".repeat(tasks::MAX_OUTPUT_LINES + 5));
    let old = action(
        &mut app,
        0,
        Action::Output {
            start_line: 1,
            start_column: 1,
            line_count: 1,
        },
    );
    assert_eq!(old.result["first_line"], 6);
    assert_eq!(old.result["skipped_lines"], 5);
    assert_eq!(old.result["output_truncated"], true);
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
    let new = action(
        &mut app,
        0,
        Action::Output {
            start_line: 1,
            start_column: 1,
            line_count: 1,
        },
    );
    assert_eq!(new.result["first_line"], 1);
    assert_eq!(new.result["skipped_lines"], 0);
    assert_eq!(new.result["output_truncated"], false);
    assert!(
        new.result["text"]
            .as_str()
            .unwrap()
            .starts_with("Desktop · ")
    );
    assert_eq!(new.result["run"]["status"], "Failed");
}

#[test]
fn launch_configuration_revisions_stay_json_safe_and_accept_unmasked_values() {
    let fixture = Fixture::new();
    let path = fixture.0.join(launch::CONFIG_PATH);
    let mut config = configuration();
    // Nearly every 64-bit content hash exceeds 2^53; vary a name until one does.
    let mut revision = fixture.config(&config);
    let mut full = workspace::read_document(&path, launch::MAX_CONFIG_BYTES)
        .unwrap()
        .fingerprint
        .hash;
    for attempt in 0.. {
        if full > REVISION_MASK {
            break;
        }
        assert!(attempt < 64, "no content hash above 2^53");
        config.profiles[0].name = format!("Web UI {attempt}");
        revision = read_or_configure(
            &fixture.0,
            &Action::Configure {
                configuration: config.clone(),
                revision,
            },
        )
        .unwrap()
        .revision;
        full = workspace::read_document(&path, launch::MAX_CONFIG_BYTES)
            .unwrap()
            .fingerprint
            .hash;
    }
    assert!(revision <= REVISION_MASK && revision > 0);
    assert_eq!(revision, (full & REVISION_MASK).max(1));
    let listed = read_or_configure(&fixture.0, &Action::List).unwrap();
    assert_eq!(listed.revision, revision);
    assert_eq!(
        serde_json::to_value(revision).unwrap().as_f64().unwrap() as u64,
        revision
    );

    let original = fs::read(&path).unwrap();
    for wrong in [full ^ 2, revision ^ 2, 0] {
        assert!(
            read_or_configure(
                &fixture.0,
                &Action::Configure {
                    configuration: config.clone(),
                    revision: wrong,
                },
            )
            .err()
            .expect("a wrong revision was accepted")
            .contains(&format!("current revision {revision}"))
        );
        assert_eq!(fs::read(&path).unwrap(), original);
    }
    // A revision issued by an earlier build before masking still identifies the file.
    config.profiles[0].name = "Web UI accepted".into();
    let updated = read_or_configure(
        &fixture.0,
        &Action::Configure {
            configuration: config.clone(),
            revision: full.max(1),
        },
    )
    .unwrap();
    assert_eq!(updated.configuration.unwrap(), config);
    assert!(updated.revision <= REVISION_MASK && updated.revision > 0);
}

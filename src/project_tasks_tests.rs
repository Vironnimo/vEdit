use super::*;

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "vedit project tasks café & 'quoted' {} {}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }

    fn write(&self, relative: &str) -> PathBuf {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "").unwrap();
        path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn active_python_file_overrides_rust_manifest_and_keeps_path_one_argument() {
    let fixture = Fixture::new();
    fixture.write("Cargo.toml");
    let script = fixture.write("run café & 'quoted'.py");
    let config = TaskConfiguration::default();
    let resolved = config
        .resolve(&fixture.0, Some(&script), ProjectTaskAction::RunFile)
        .unwrap();
    assert_eq!(
        resolved.command,
        tasks::CommandSpec::Program {
            program: config.python_program(&fixture.0).into(),
            args: vec!["-u".into(), script.as_os_str().into()],
        }
    );
    assert_eq!(resolved.root, fixture.0);
}

#[test]
fn automatic_and_relative_python_interpreters_resolve_inside_project() {
    let fixture = Fixture::new();
    let relative = if cfg!(windows) {
        ".venv/Scripts/python.exe"
    } else {
        ".venv/bin/python"
    };
    let executable = fixture.write(relative);
    fixture.write(".venv/pyvenv.cfg");
    let mut config = TaskConfiguration::default();
    assert_eq!(PathBuf::from(config.python_program(&fixture.0)), executable);
    let resolved = config
        .resolve(&fixture.0, None, ProjectTaskAction::Check)
        .unwrap();
    assert!(
        resolved
            .environment
            .contains(&("PYTHONIOENCODING".into(), "utf-8".into()))
    );
    assert!(resolved.environment.contains(&(
        "VIRTUAL_ENV".into(),
        fixture.0.join(".venv").into_os_string()
    )));
    let search_path = &resolved
        .environment
        .iter()
        .find(|(key, _)| key == "PATH")
        .unwrap()
        .1;
    assert_eq!(
        std::env::split_paths(search_path).next().as_deref(),
        executable.parent()
    );
    config.python_interpreter = "tools/my python".into();
    assert_eq!(
        PathBuf::from(config.python_program(&fixture.0)),
        fixture.0.join("tools/my python")
    );
    config.python_interpreter = "python3.13".into();
    assert_eq!(config.python_program(&fixture.0), "python3.13");
}

#[test]
#[cfg(windows)]
fn selected_system_python_precedes_inherited_environment_for_python_and_scripts() {
    let fixture = Fixture::new();
    let interpreter = fixture.write("system python/python.exe");
    let script = fixture.write("system python/Scripts/pip.exe");
    let config = TaskConfiguration {
        python_interpreter: interpreter.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let environment = config.environment(&fixture.0, None);
    let search_path = &environment.iter().find(|(key, _)| key == "PATH").unwrap().1;
    let directories: Vec<_> = std::env::split_paths(search_path).collect();
    assert_eq!(directories[0], interpreter.parent().unwrap());
    assert_eq!(directories[1], script.parent().unwrap());
    assert!(environment.contains(&("VIRTUAL_ENV".into(), OsString::new())));
}

#[test]
fn dart_presets_use_analyzer_test_and_active_file_compilation() {
    let fixture = Fixture::new();
    fixture.write("pubspec.yaml");
    let file = fixture.write("bin/main.dart");
    let config = TaskConfiguration::default();
    for (action, arguments) in [
        (ProjectTaskAction::Check, vec![OsString::from("analyze")]),
        (ProjectTaskAction::Test, vec![OsString::from("test")]),
        (
            ProjectTaskAction::Build,
            vec!["compile".into(), "exe".into(), file.as_os_str().into()],
        ),
    ] {
        let tasks::CommandSpec::Program { program, args } = config
            .resolve(&fixture.0, Some(&file), action)
            .unwrap()
            .command
        else {
            panic!("Dart presets must run directly without shell interpolation");
        };
        assert_eq!(args, arguments);
        assert!(
            Path::new(&program)
                .file_stem()
                .unwrap()
                .to_string_lossy()
                .starts_with("dart")
        );
    }
}

#[test]
fn dart_tasks_inherit_selected_sdk_but_project_override_wins() {
    let fixture = Fixture::new();
    let file = fixture.write("main.dart");
    let selected = fixture.write("chosen sdk/bin/dart.exe");
    let override_sdk = fixture.write("other sdk/bin/dart.exe");
    let mut app = VEditApp::empty(egui::Context::default());
    app.settings
        .language_servers
        .iter_mut()
        .find(|language| language.language_id == "dart")
        .unwrap()
        .program = selected.to_string_lossy().into_owned();
    let mut project = Project::new(fixture.0.clone());
    project
        .tabs
        .push(EditorTab::new(Some(file), "main.dart".into()));
    app.projects.push(project);
    let resolved = app.resolve_project_task(ProjectTaskAction::Check).unwrap();
    assert_eq!(
        resolved.command,
        tasks::CommandSpec::Program {
            program: selected.into_os_string(),
            args: vec!["analyze".into()]
        }
    );
    app.projects[0].tasks.dart_executable = override_sdk.to_string_lossy().into_owned();
    let resolved = app.resolve_project_task(ProjectTaskAction::Check).unwrap();
    assert_eq!(
        resolved.command,
        tasks::CommandSpec::Program {
            program: override_sdk.as_os_str().into(),
            args: vec!["analyze".into()]
        }
    );
    let search_path = &resolved
        .environment
        .iter()
        .find(|(key, _)| key == "PATH")
        .unwrap()
        .1;
    assert!(
        std::env::split_paths(search_path)
            .any(|directory| Some(directory.as_path()) == override_sdk.parent())
    );
}

#[test]
fn custom_commands_keep_source_literal_and_provide_paths_as_environment() {
    let fixture = Fixture::new();
    let file = fixture.write("special & $(injection).other");
    let source = "some-tool $env:VEDIT_FILE --arg";
    let config = TaskConfiguration {
        run_file: source.into(),
        ..Default::default()
    };
    let resolved = config
        .resolve(&fixture.0, Some(&file), ProjectTaskAction::RunFile)
        .unwrap();
    assert_eq!(resolved.command, tasks::CommandSpec::Shell(source.into()));
    assert!(
        resolved
            .environment
            .contains(&("VEDIT_FILE".into(), file.into_os_string()))
    );
    assert!(
        resolved
            .environment
            .contains(&("VEDIT_ROOT".into(), fixture.0.as_os_str().into()))
    );
}

#[test]
fn saved_morrow_commands_keep_their_environment_after_rename() {
    let fixture = Fixture::new();
    let file = fixture.write("draft with spaces.py");
    let source = "& $env:MORROW_PYTHON $env:MORROW_FILE";
    let config = TaskConfiguration {
        run_file: source.into(),
        ..Default::default()
    };
    let resolved = config
        .resolve(&fixture.0, Some(&file), ProjectTaskAction::RunFile)
        .unwrap();
    assert_eq!(resolved.command, tasks::CommandSpec::Shell(source.into()));
    for suffix in ["ROOT", "FILE", "PYTHON"] {
        let value = |prefix: &str| {
            resolved
                .environment
                .iter()
                .find(|(name, _)| name == &OsString::from(format!("{prefix}_{suffix}")))
                .map(|(_, value)| value)
        };
        assert!(value("VEDIT").is_some());
        assert_eq!(value("MORROW"), value("VEDIT"));
    }
}

#[test]
fn run_file_never_treats_readme_as_python_from_project_fallback() {
    let fixture = Fixture::new();
    fixture.write("pyproject.toml");
    let readme = fixture.write("README.md");
    let config = TaskConfiguration::default();
    assert!(
        config
            .resolve(&fixture.0, Some(&readme), ProjectTaskAction::RunFile)
            .is_err()
    );
    assert!(
        config
            .resolve(&fixture.0, Some(&readme), ProjectTaskAction::Check)
            .is_ok()
    );
    assert!(
        config
            .resolve(&fixture.0, None, ProjectTaskAction::RunFile)
            .is_err()
    );
}

#[test]
fn project_tasks_persist_and_legacy_projects_get_defaults() {
    let fixture = Fixture::new();
    let mut app = VEditApp::empty(egui::Context::default());
    let mut project = Project::new(fixture.0.clone());
    project.tasks = TaskConfiguration {
        profile: TaskProfile::Custom,
        python_interpreter: "my-python".into(),
        check: "custom-check".into(),
        ..Default::default()
    };
    let expected = project.tasks.clone();
    app.projects.push(project);
    let saved = serde_json::to_string(&app.session()).unwrap();
    let session: Session = serde_json::from_str(&saved).unwrap();
    let mut restored = VEditApp::empty(egui::Context::default());
    restored.restore_session_projects(session.projects);
    assert_eq!(restored.projects[0].tasks, expected);
    let legacy: SessionProject =
        serde_json::from_value(serde_json::json!({"root":fixture.0})).unwrap();
    assert_eq!(legacy.tasks, TaskConfiguration::default());
}

#[test]
fn tasks_refuse_dirty_or_still_saving_buffers_without_starting_a_process() {
    let fixture = Fixture::new();
    let file = fixture.write("main.py");
    let mut app = VEditApp::empty(egui::Context::default());
    let mut project = Project::new(fixture.0.clone());
    let mut tab = EditorTab::new(Some(file), "main.py".into());
    tab.dirty = true;
    tab.agent_unsaved = true;
    project.tabs.push(tab);
    app.projects.push(project);
    assert!(
        app.resolve_project_task(ProjectTaskAction::RunFile)
            .is_err()
    );
    assert!(app.task.is_none());
    assert!(app.projects[0].tabs[0].agent_unsaved);
    app.projects[0].tabs[0].dirty = false;
    app.projects[0].tabs[0].saving = true;
    assert!(app.resolve_project_task(ProjectTaskAction::Check).is_err());
    app.projects[0].tabs[0].saving = false;
    assert!(app.resolve_project_task(ProjectTaskAction::RunFile).is_ok());
}

#[test]
fn unknown_language_requires_explicit_task_instead_of_running_cargo() {
    let fixture = Fixture::new();
    let file = fixture.write("main.js");
    let config = TaskConfiguration::default();
    assert!(
        config
            .resolve(&fixture.0, Some(&file), ProjectTaskAction::Check)
            .is_err()
    );
    let custom = TaskConfiguration {
        check: "node --check main.js".into(),
        ..config
    };
    assert!(
        custom
            .resolve(&fixture.0, Some(&file), ProjectTaskAction::Check)
            .is_ok()
    );
}

#[test]
#[ignore = "requires Python on PATH; exercises native and custom-shell task output"]
fn live_python_tasks_preserve_unicode_stdout_stderr_and_paths_without_utf8_mode() {
    let fixture = Fixture::new();
    let file = fixture.write("print ü café.py");
    fs::write(&file, "import sys\nassert sys.flags.utf8_mode == 0\nprint('stdout üé 🚀')\nprint('stderr äö 🐍', file=sys.stderr)\nprint(__file__)\n").unwrap();
    let custom = if cfg!(windows) {
        "& $env:VEDIT_PYTHON $env:VEDIT_FILE"
    } else {
        "\"$VEDIT_PYTHON\" \"$VEDIT_FILE\""
    };
    for config in [
        TaskConfiguration::default(),
        TaskConfiguration {
            run_file: custom.into(),
            ..Default::default()
        },
    ] {
        let mut resolved = config
            .resolve(&fixture.0, Some(&file), ProjectTaskAction::RunFile)
            .unwrap();
        // Exercise the IO encoding contract independently from Python's
        // interpreter-wide UTF-8 mode and the machine's locale defaults.
        resolved.environment.push(("PYTHONUTF8".into(), "0".into()));
        let (code, output) = run_and_collect(resolved);
        assert_eq!(code, Some(0), "{output}");
        assert!(output.contains("stdout üé 🚀"), "{output:?}");
        assert!(output.contains("stderr äö 🐍"), "{output:?}");
        assert!(output.contains("print ü café.py"), "{output:?}");
        assert!(!output.contains('\u{fffd}'), "{output:?}");
    }
}

fn run_and_collect(resolved: ResolvedTask) -> (Option<i32>, String) {
    let task = tasks::spawn_configured(resolved.root, resolved.command, resolved.environment);
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut output = String::new();
    loop {
        match task
            .receiver
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("Python task did not finish")
        {
            tasks::TaskEvent::Started => {}
            tasks::TaskEvent::Output(text) => output.push_str(&text),
            tasks::TaskEvent::Error(error) => panic!("Python task failed: {error}"),
            tasks::TaskEvent::Finished { code, cancelled } => {
                assert!(!cancelled);
                return (code, output);
            }
        }
    }
}

#[test]
#[ignore = "requires Python on PATH; exercises source syntax checks without executing project code"]
fn live_python_check_covers_all_extensions_exclusions_and_same_timestamp_edits() {
    let fixture = Fixture::new();
    let files: Vec<_> = ["main.py", "window.pyw", "types.pyi", "UPPER.PY"]
        .into_iter()
        .map(|name| {
            let path = fixture.write(name);
            fs::write(&path, "x = 1\n").unwrap();
            path
        })
        .collect();
    for excluded in [
        ".venv/invalid.py",
        "venv/invalid.pyw",
        "env/invalid.pyi",
        ".git/invalid.PY",
        "node_modules/invalid.py",
    ] {
        fs::write(fixture.write(excluded), "x = (\n").unwrap();
    }
    fs::write(
        fixture.write("never_execute.py"),
        "raise RuntimeError('Syntax checks must not execute this file')\n",
    )
    .unwrap();
    let config = TaskConfiguration::default();
    let (code, output) = run_and_collect(
        config
            .resolve(&fixture.0, Some(&files[0]), ProjectTaskAction::Check)
            .unwrap(),
    );
    assert_eq!(code, Some(0), "{output}");
    assert!(output.contains("5 file(s), passed"), "{output}");
    assert!(!fixture.0.join("__pycache__").exists());
    for path in files {
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        // Preserve both file size and exact modification time: a cached pyc
        // timestamp/size check would incorrectly accept this new syntax error.
        fs::write(&path, "x = (\n").unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
        let (code, output) = run_and_collect(
            config
                .resolve(&fixture.0, Some(&path), ProjectTaskAction::Check)
                .unwrap(),
        );
        assert_eq!(code, Some(1), "{}: {output}", path.display());
        assert!(output.contains("SyntaxError"), "{output}");
        assert!(
            output.contains(path.file_name().unwrap().to_str().unwrap()),
            "{output}"
        );
        fs::write(&path, "x = 1\n").unwrap();
    }
}

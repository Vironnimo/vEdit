use super::*;
use std::process::{Command, Output, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
#[ignore = "requires VEDIT_CLI_TEST_BINARY pointing to the built vedit-cli executable"]
fn status_message_details_cli_round_trip_in_isolated_editor() {
    let binary = std::env::var_os("VEDIT_CLI_TEST_BINARY").expect("set VEDIT_CLI_TEST_BINARY");
    let root = std::env::temp_dir().join(format!(
        "vedit-cli-round-trip-message-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let fixture = Fixture(root);
    let endpoint = fixture.0.join("control");
    let mut app = VEditApp::empty(egui::Context::default());
    app.recovery_session_path = fixture.0.join("session.json");
    app.control = Some(cli::Server::start_test(app.ctx.clone(), endpoint.clone()).unwrap());
    let message = "Rust analysis stopped (exit code: 101).\r\nFull error ü 🦀\r\n".repeat(80);
    app.status(message.clone(), true);
    app.open_message_details(message.clone(), true);
    app.status("Later status", false);
    let mut invoke = |args: &[&str]| {
        let mut command = Command::new(&binary);
        command.args(args);
        let output = run(&mut app, command, &endpoint);
        (
            output.status.code().unwrap(),
            serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        )
    };
    let (code, response) = invoke(&["status"]);
    assert_eq!(code, 0);
    assert_eq!(response["result"]["status_message"]["text"], "Later status");
    assert_eq!(response["result"]["message_details"]["text"], message);
    assert_eq!(response["result"]["dialog"], "message_details");
    let (code, response) = invoke(&["new", "Draft"]);
    assert_ne!(code, 0);
    assert_eq!(response["error"]["code"], "user_dialog_open");
    let (code, response) = invoke(&["read", "999"]);
    assert_ne!(code, 0);
    assert_eq!(response["error"]["code"], "unknown_tab");
    assert!(app.projects.is_empty());
}

#[test]
#[ignore = "requires VEDIT_CLI_TEST_BINARY pointing to the built vedit-cli executable"]
fn editor_key_settings_cli_round_trip_in_isolated_editor() {
    let binary = std::env::var_os("VEDIT_CLI_TEST_BINARY").expect("set VEDIT_CLI_TEST_BINARY");
    let root = std::env::temp_dir().join(format!(
        "vedit-cli-round-trip-editor-keys-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let fixture = Fixture(root);
    let endpoint = fixture.0.join("control");
    let patch = fixture.0.join("editor-keys.json");
    fs::write(
        &patch,
        r#"{"multicursor":{"modifier":"ctrl","spread_paste":false},"vim_keys":{"Ctrl+V":"vim","Ctrl+F":"editor"}}"#,
    )
    .unwrap();
    let invalid = fixture.0.join("invalid.json");
    fs::write(&invalid, r#"{"vim":false,"multicursor":{"limit":99}}"#).unwrap();
    let reset = fixture.0.join("reset-vim-key.json");
    fs::write(&reset, r#"{"vim_keys":{"Ctrl+V":null}}"#).unwrap();
    let invalid_key = fixture.0.join("invalid-vim-key.json");
    fs::write(
        &invalid_key,
        r#"{"word_wrap":true,"vim_keys":{"Ctrl+F":null,"Ctrl+Z":"vim"}}"#,
    )
    .unwrap();
    let invalid_target = fixture.0.join("invalid-vim-target.json");
    fs::write(
        &invalid_target,
        r#"{"word_wrap":true,"vim_keys":{"Ctrl+F":null,"Ctrl+V":"default"}}"#,
    )
    .unwrap();
    let mut app = VEditApp::empty(egui::Context::default());
    app.recovery_session_path = fixture.0.join("session.json");
    app.control = Some(cli::Server::start_test(app.ctx.clone(), endpoint.clone()).unwrap());
    let mut invoke = |args: &[&str]| {
        let mut command = Command::new(&binary);
        command.args(args);
        let output = run(&mut app, command, &endpoint);
        (
            output.status.code().unwrap(),
            serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        )
    };
    let (code, response) = invoke(&["settings", patch.to_str().unwrap()]);
    assert_eq!(code, 0, "{response}");
    assert_eq!(
        response["result"]["settings"]["multicursor"]["modifier"],
        "ctrl"
    );
    assert_eq!(response["result"]["settings"]["multicursor"]["limit"], 1000);
    assert_eq!(
        response["result"]["settings"]["vim_keys"],
        json!({"Ctrl+V":"vim","Ctrl+F":"editor"})
    );
    let catalog = response["result"]["vim_key_actions"].as_array().unwrap();
    assert_eq!(catalog.len(), cli::VIM_CONTROL_KEYS.len());
    let paste = catalog
        .iter()
        .find(|entry| entry["key"] == "Ctrl+V")
        .unwrap();
    assert_eq!(paste["target"], "vim");
    assert_eq!(paste["default"], "editor");
    assert_eq!(paste["effective"], "vim");
    assert_eq!(paste["scope"], "source_editor");
    assert_eq!(paste["outside_modes"], "editor");
    assert!(
        paste["modes"]
            .as_array()
            .unwrap()
            .contains(&json!("visualblock"))
    );
    assert!(
        !paste["modes"]
            .as_array()
            .unwrap()
            .contains(&json!("insert"))
    );
    let find = catalog
        .iter()
        .find(|entry| entry["key"] == "Ctrl+F")
        .unwrap();
    assert_eq!(find["target"], "editor");
    assert_eq!(find["default"], "vim");
    assert_eq!(find["effective"], "editor");
    println!(
        "Applied editor key preferences: multicursor={}, vim_keys={}",
        response["result"]["settings"]["multicursor"], response["result"]["settings"]["vim_keys"]
    );
    let (code, response) = invoke(&["settings", reset.to_str().unwrap()]);
    assert_eq!(code, 0, "{response}");
    assert_eq!(
        response["result"]["settings"]["vim_keys"],
        json!({"Ctrl+F":"editor"})
    );
    let expected_settings = response["result"]["settings"].clone();
    let expected_catalog = response["result"]["vim_key_actions"].clone();
    let paste = expected_catalog
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["key"] == "Ctrl+V")
        .unwrap();
    assert_eq!(paste["target"], "default");
    assert_eq!(paste["effective"], "editor");
    println!(
        "Reset Ctrl+V; retained Vim key overrides: {}",
        expected_settings["vim_keys"]
    );
    let (code, response) = invoke(&["settings", invalid.to_str().unwrap()]);
    assert_ne!(code, 0);
    assert!(
        response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("100")
    );
    println!("Invalid limit: {}", response["error"]);
    for invalid in [&invalid_key, &invalid_target] {
        let (code, response) = invoke(&["settings", invalid.to_str().unwrap()]);
        assert_ne!(code, 0, "{response}");
        assert_eq!(response["ok"], false);
        assert!(response["error"].is_object());
        println!("Invalid Vim key preference: {}", response["error"]);
    }
    let (code, response) = invoke(&["settings", fixture.0.join("missing.json").to_str().unwrap()]);
    assert_ne!(code, 0);
    assert!(response["error"].is_object());
    let (code, response) = invoke(&["settings"]);
    assert_eq!(code, 0);
    assert_eq!(response["result"]["settings"], expected_settings);
    assert_eq!(response["result"]["vim_key_actions"], expected_catalog);
    assert_eq!(response["result"]["settings"]["vim"], true);
    assert_eq!(
        response["result"]["settings"]["multicursor"]["spread_paste"],
        false
    );
    assert!(app.recovery_writer.wait_idle(Duration::from_secs(10)));
}

#[test]
#[ignore = "requires VEDIT_CLI_TEST_BINARY pointing to the built vedit-cli executable"]
fn other_files_cli_round_trip_in_isolated_editor() {
    let binary = std::env::var_os("VEDIT_CLI_TEST_BINARY").expect("set VEDIT_CLI_TEST_BINARY");
    let root = std::env::temp_dir().join(format!(
        "vedit-cli-round-trip-other-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let fixture = Fixture(root);
    let endpoint = fixture.0.join("control");
    let a = fixture.0.join("notes café.txt");
    let b = fixture.0.join("other.txt");
    fs::write(&a, "café 🦀").unwrap();
    fs::write(&b, "second").unwrap();
    let mut app = VEditApp::empty(egui::Context::default());
    app.recovery_session_path = fixture.0.join("session.json");
    app.control = Some(cli::Server::start_test(app.ctx.clone(), endpoint.clone()).unwrap());
    let mut cli = |args: &[&str]| {
        let mut command = Command::new(&binary);
        command.args(args);
        let output = run(&mut app, command, &endpoint);
        (
            output.status.code().unwrap(),
            serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        )
    };
    let (code, opened) = cli(&["open", a.to_str().unwrap(), b.to_str().unwrap()]);
    assert_eq!(code, 0, "{opened}");
    assert!(opened["result"]["active_workspace"].is_null());
    let tab = &opened["result"]["items"][0]["tab"];
    assert!(tab["workspace"].is_null());
    assert_eq!(tab["workspace_kind"], "other_files");
    let (code, page) = cli(&["tabs", "--limit", "1"]);
    assert_eq!(code, 0);
    assert_eq!(page["result"]["total"], 2);
    let next = page["result"]["next_offset"].to_string();
    let (code, page2) = cli(&["tabs", "--limit", "1", "--offset", &next]);
    assert_eq!(code, 0);
    assert_ne!(
        page["result"]["items"][0]["id"],
        page2["result"]["items"][0]["id"]
    );
    let (code, missing) = cli(&["open", fixture.0.join("missing.txt").to_str().unwrap()]);
    assert_ne!(code, 0);
    assert_eq!(missing["result"]["items"][0]["ok"], false);
    let (code, usage) = cli(&["open"]);
    assert_ne!(code, 0);
    assert_eq!(usage["error"]["code"], "invalid_arguments");
    let (code, status) = cli(&["status"]);
    assert_eq!(code, 0);
    assert_eq!(status["result"]["active_workspace_kind"], "other_files");
    assert!(
        status["result"]["workspaces"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    println!(
        "Opened two loose files; workspace=null, workspace_kind=other_files; pagination, usage and missing-file errors verified."
    );
}

#[test]
#[cfg(windows)]
#[ignore = "requires VEDIT_CLI_TEST_BINARY pointing to the built vedit-cli executable"]
fn launch_cli_round_trip_in_isolated_editor() {
    let binary = std::env::var_os("VEDIT_CLI_TEST_BINARY").expect("set VEDIT_CLI_TEST_BINARY");
    let root = std::env::temp_dir().join(format!(
        "vedit-cli-round-trip-launch-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let fixture = Fixture(root);
    let endpoint = fixture.0.join("control");
    let project = fixture.0.join("project café & 東京");
    fs::create_dir_all(&project).unwrap();
    fs::write(project.join("start.ps1"), "param([string]$Mode)\n[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)\n[Console]::Out.WriteLine(\"ready $Mode $env:LAUNCH_TEST\")\nStart-Sleep -Seconds 30\n").unwrap();
    let input = fixture.0.join("launch-input.json");
    let config = json!({"profiles":[{"id":"web","name":"Web 東京","program":"powershell.exe","args":["-NoProfile","-NonInteractive","-ExecutionPolicy","Bypass","-File","start.ps1","-Mode","web"],"env":{"LAUNCH_TEST":"café 東京"}},{"id":"headless","name":"Headless","program":"missing-vedit-launch.exe"}],"default":"web"});
    fs::write(&input, serde_json::to_vec(&config).unwrap()).unwrap();
    let mut app = VEditApp::empty(egui::Context::default());
    app.projects.push(Project::new(fixture.0.clone()));
    app.projects.push(Project::new(project.clone()));
    app.recovery_session_path = fixture.0.join("session.json");
    app.control = Some(cli::Server::start_test(app.ctx.clone(), endpoint.clone()).unwrap());
    let mut cli = |args: &[&str]| {
        let mut command = Command::new(&binary);
        command
            .arg("launch")
            .args(args)
            .arg("--workspace")
            .arg(&project);
        let output = run(&mut app, command, &endpoint);
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        (output.status.code().unwrap(), value)
    };
    let (code, list) = cli(&["list"]);
    assert_eq!(code, 0);
    assert_eq!(list["result"]["revision"], 0);
    let (code, configured) = cli(&["configure", "--file", input.to_str().unwrap()]);
    assert_eq!(code, 0, "{configured}");
    assert_eq!(
        configured["result"]["configuration"]["profiles"][0]["name"],
        "Web 東京"
    );
    let (code, conflict) = cli(&["configure", "--file", input.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(
        conflict["error"]["message"]
            .as_str()
            .unwrap()
            .contains("revision")
    );
    let (code, missing) = cli(&["start", "absent"]);
    assert_eq!(code, 1);
    assert_eq!(missing["error"]["code"], "unknown_launch");
    let (code, selected) = cli(&["select", "web"]);
    assert_eq!(code, 0);
    assert_eq!(selected["result"]["selected"], "web");
    for _ in 0..2 {
        let (code, started) = cli(&["start"]);
        assert_eq!(code, 0, "{started}");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let (code, output) = cli(&["output"]);
            assert_eq!(code, 0);
            if output["result"]["text"]
                .as_str()
                .unwrap()
                .contains("ready web café 東京")
            {
                break;
            }
            assert!(Instant::now() < deadline, "{output}");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(cli(&["stop"]).0, 0);
        loop {
            let (_, output) = cli(&["output"]);
            if output["result"]["run"]["running"] == false {
                assert_eq!(output["result"]["run"]["status"], "Stopped");
                break;
            }
            assert!(Instant::now() < deadline, "{output}");
        }
    }
    assert_eq!(app.active_project, 0);
    assert!(app.projects[0].launch.configuration.profiles.is_empty());
}

struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let root = fs::canonicalize(&self.0).unwrap();
        assert!(root.starts_with(fs::canonicalize(std::env::temp_dir()).unwrap()));
        assert!(
            root.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("vedit-cli-round-trip-")
        );
        let _ = fs::remove_dir_all(root);
    }
}

fn run(app: &mut VEditApp, mut command: Command, endpoint: &Path) -> Output {
    let child = command
        .env("VEDIT_CONTROL_DIR", endpoint)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let (send, receive) = mpsc::channel();
    // Drain capture pipes while the GUI handles the request, including large pages.
    let worker = std::thread::spawn(move || send.send(child.wait_with_output().unwrap()).unwrap());
    let deadline = Instant::now() + Duration::from_secs(55);
    let output = loop {
        if let Ok(output) = receive.try_recv() {
            break output;
        }
        assert!(
            Instant::now() < deadline,
            "CLI did not return before its deadline"
        );
        app.poll_control();
        app.poll();
        std::thread::sleep(Duration::from_millis(5));
    };
    worker.join().unwrap();
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

#[test]
#[ignore = "requires VEDIT_CLI_TEST_BINARY pointing to the built vedit-cli executable"]
fn cli_agent_workflow_round_trip_in_isolated_editor() {
    let binary = std::env::var_os("VEDIT_CLI_TEST_BINARY").expect("set VEDIT_CLI_TEST_BINARY");
    let root = std::env::temp_dir().join(format!(
        "vedit-cli-round-trip-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let fixture = Fixture(root);
    let endpoint = fixture.0.join("control");
    let mut app = VEditApp::empty(egui::Context::default());
    app.projects.push(Project::new(fixture.0.clone()));
    app.recovery_session_path = fixture.0.join("session.json");
    app.control = Some(cli::Server::start_test(app.ctx.clone(), endpoint.clone()).unwrap());
    let mut cli = |args: &[&str]| {
        let mut command = Command::new(&binary);
        command.args(args);
        let output = run(&mut app, command, &endpoint);
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        (output.status.code().unwrap(), value)
    };
    let plan = fixture.0.join("plan.md");
    fs::write(&plan, "# Review\nZürich 東京 😀\n").unwrap();
    let (code, opened) = cli(&["open", plan.to_str().unwrap(), "--preview"]);
    assert_eq!(code, 0);
    assert_eq!(opened["result"]["items"][0]["ok"], true);
    let text_file = fixture.0.join("input.txt");
    let text = "Zürich 東京 😀\r\n--help\n";
    let mut encoded = vec![0xff, 0xfe];
    for unit in text.encode_utf16() {
        encoded.extend(unit.to_le_bytes());
    }
    fs::write(&text_file, encoded).unwrap();
    let (code, created) = cli(&[
        "new",
        "Proposal 東京",
        "--file",
        text_file.to_str().unwrap(),
    ]);
    assert_eq!(code, 0);
    assert_eq!(created["result"]["tab"]["title"], "Proposal 東京");
    assert_eq!(created["result"]["created_on_disk"], false);
    let id = created["result"]["tab"]["id"].to_string();
    let (code, read) = cli(&["read", &id, "--line", "1", "--count", "100"]);
    assert_eq!(code, 0);
    assert_eq!(read["result"]["text"], text);
    let revision = read["result"]["revision"].to_string();
    fs::write(&text_file, "changed 東京\r\n").unwrap();
    let (code, edit) = cli(&[
        "edit",
        &id,
        "--file",
        text_file.to_str().unwrap(),
        "--revision",
        &revision,
    ]);
    assert_eq!(code, 0);
    assert_eq!(edit["result"]["tab"]["dirty"], true);
    assert_eq!(edit["result"]["tab"]["autosave_paused"], true);
    let (code, search) = cli(&["search", "changed", fixture.0.to_str().unwrap()]);
    assert_eq!(code, 0);
    assert_eq!(search["result"]["includes_unsaved_buffers"], true);
    let found = search["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["tab_id"] == created["result"]["tab"]["id"])
        .unwrap();
    assert!(found["path"].is_null());
    assert_eq!(found["unsaved"], true);
    assert_eq!(found["revision"], edit["result"]["tab"]["revision"]);
    let (code, limited) = cli(&[
        "search",
        "東京",
        fixture.0.to_str().unwrap(),
        "--limit",
        "1",
    ]);
    assert_eq!(code, 0);
    assert_eq!(limited["result"]["truncated"], true);
    let (code, missing_search) = cli(&[
        "search",
        "needle",
        fixture.0.join("missing").to_str().unwrap(),
    ]);
    assert_eq!(code, 1);
    assert_eq!(missing_search["error"]["code"], "unknown_workspace");
    let (code, invalid_search) = cli(&["search", "needle", "--limit", "zero"]);
    assert_eq!(code, 1);
    assert_eq!(invalid_search["error"]["code"], "invalid_arguments");
    let (code, stale) = cli(&[
        "edit",
        &id,
        "--file",
        text_file.to_str().unwrap(),
        "--revision",
        &revision,
    ]);
    assert_eq!(code, 1);
    assert_eq!(stale["error"]["code"], "revision_conflict");
    let (code, missing) = cli(&["read", "18446744073709551615"]);
    assert_eq!(code, 1);
    assert_eq!(missing["error"]["code"], "unknown_tab");
    let (code, usage) = cli(&["edit", &id]);
    assert_eq!(code, 1);
    assert_eq!(usage["error"]["code"], "invalid_arguments");
    let (code, page) = cli(&["tabs", "--limit=1"]);
    assert_eq!(code, 0);
    let next = page["result"]["next_offset"].to_string();
    let (code, page2) = cli(&["tabs", "--limit", "1", "--offset", &next]);
    assert_eq!(code, 0);
    assert_ne!(
        page["result"]["items"][0]["id"],
        page2["result"]["items"][0]["id"]
    );
    let (code, marks) = cli(&["agent-marks"]);
    assert_eq!(code, 0);
    assert!(marks["result"]["items"].as_array().unwrap().is_empty());
    let (code, partial) = cli(&["close", &id, "18446744073709551615"]);
    assert_eq!(code, 2);
    assert_eq!(partial["result"]["complete"], false);

    // Exercise the skill's actual PowerShell encoding setup and inline Unicode.
    #[cfg(windows)]
    {
        let quote = |value: &str| format!("'{}'", value.replace('\'', "''"));
        for shell in ["powershell.exe", "pwsh.exe"] {
            if Command::new(shell)
                .args(["-NoProfile", "-NonInteractive", "-Command", "exit 0"])
                .output()
                .is_err()
            {
                continue;
            }
            let script = format!(
                "[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false); & {} new 'Zürich 東京 😀' --file {}",
                quote(binary.to_str().unwrap()),
                quote(text_file.to_str().unwrap())
            );
            let mut command = Command::new(shell);
            command.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
            let output = run(&mut app, command, &endpoint);
            assert!(output.status.success());
            let value: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(value["result"]["tab"]["title"], "Zürich 東京 😀");
            println!("{shell}: Unicode file/argument round trip passed");
        }
    }
    println!(
        "CLI entry point: open, read, new, edit, stale revision, missing tab, usage error, two pages, marks and partial result passed"
    );
    drop(app);
}

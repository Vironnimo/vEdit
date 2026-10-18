use super::*;
use std::process::{Command, Output, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

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

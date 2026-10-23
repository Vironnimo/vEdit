//! Running-window discovery, startup and request delivery.
use super::*;

pub fn execute(request: &Request, start: bool) -> Result<Response, String> {
    // Refuse an ambiguous destination before any startup or mutation.
    if crate::profile::selected_window().is_none() && running_windows().len() > 1 {
        return send(request);
    }
    if transport::endpoint_alive() || !start {
        return send(request);
    }
    if crate::profile::selected_window().is_none() {
        // Startup can also recover additional windows. Pin this request to the
        // main window we start, rather than becoming ambiguous partway through.
        crate::profile::select_window("main".into())?;
    }
    if crate::profile::window_id() != "main" && !crate::profile::recovery_directory().is_dir() {
        return Err("The selected window has no saved session. Use vedit-cli windows to choose a window, or vedit --new-window to create one. No request was sent.".into());
    }
    {
        let executable = std::env::current_exe()
            .map_err(|e| e.to_string())?
            .parent()
            .ok_or("CLI executable has no parent")?
            .join(if cfg!(windows) { "vedit.exe" } else { "vedit" });
        if !executable.is_file() {
            return Err(format!(
                "vEdit is not running and {} was not found. Start the app, or put both executables in the same directory.",
                executable.display()
            ));
        }
        let mut child = spawn::spawn_window(&executable, crate::profile::window_id())
            .map_err(|e| format!("Cannot start vEdit: {e}"))?;
        let deadline = Instant::now() + Duration::from_secs(12);
        loop {
            if transport::endpoint_alive() {
                return send(request);
            }
            if child.has_exited().map_err(|e| e.to_string())? {
                return Err("vEdit exited before its controls became ready. An older open version may not support the CLI; close it normally and start the new version.".into());
            }
            if Instant::now() >= deadline {
                return Err("vEdit started but controls are not ready after 12 seconds. Inspect the app before retrying.".into());
            }
            std::thread::sleep(Duration::from_millis(80));
        }
    }
}

pub fn forward_paths(paths: Vec<PathBuf>) -> Result<Option<Response>, String> {
    if !transport::endpoint_alive() {
        return Ok(None);
    }
    let mut files = Vec::new();
    for path in paths {
        let path = absolute(path);
        if path.is_dir() {
            let response = send(&Request::Workspace { path })?;
            if !response.ok {
                return Ok(Some(response));
            }
        } else {
            files.push(OpenFile::source(path));
        }
    }
    let response = if files.is_empty() {
        send(&Request::Raise {})?
    } else {
        send(&Request::Open {
            files,
            background: false,
        })?
    };
    Ok(Some(response))
}

pub fn spawn_window(executable: &std::path::Path, id: &str) -> std::io::Result<()> {
    spawn::spawn_window(executable, id).map(|_| ())
}

/// A prefix keeps --window-looking labels and filenames literal in commands.
pub fn window_arguments(
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<(Option<String>, Vec<OsString>), String> {
    let mut args: Vec<_> = arguments.into_iter().collect();
    if args.first().is_some_and(|arg| arg == "--window") {
        if args.len() < 3 {
            return Err(
                "Use vedit-cli --window ID COMMAND; list IDs with vedit-cli windows".into(),
            );
        }
        let id = args[1]
            .to_str()
            .filter(|id| crate::profile::valid_window(id))
            .ok_or("Invalid window ID; use vedit-cli windows")?
            .to_owned();
        args.drain(..2);
        Ok((Some(id), args))
    } else {
        Ok((None, args))
    }
}

pub fn windows_response() -> Response {
    let running = running_windows();
    let saved = saved_window_ids()
        .into_iter()
        .filter(|id| !running.iter().any(|window| &window.id == id))
        .filter(|id| {
            crate::profile::window_directory(&crate::profile::base_recovery_directory(), id)
                .join("session.json")
                .is_file()
        })
        .collect::<Vec<_>>();
    let windows: Vec<_> = running
        .iter()
        .map(|window| {
            let mut value = serde_json::to_value(window).expect("window metadata");
            let endpoint =
                crate::profile::window_directory(&transport::base_endpoint_directory(), &window.id)
                    .join("control.json");
            match transport::send_to(&Request::Status {}, &endpoint) {
                Ok(response) if response.ok => {
                    value["active_workspace"] = response.result["active_workspace"].clone();
                    value["active_workspace_kind"] =
                        response.result["active_workspace_kind"].clone();
                    value["workspaces"] = response.result["workspaces"]
                        .as_array()
                        .map(|items| {
                            items
                                .iter()
                                .map(|item| item["path"].clone())
                                .collect::<Vec<_>>()
                        })
                        .into();
                    value["active_tab"] = response.result["active_tab"].clone();
                }
                Ok(response) => value["error"] = serde_json::to_value(response.error).unwrap(),
                Err(error) => value["error"] = error.into(),
            }
            value
        })
        .collect();
    Response::success(
        json!({"windows": windows, "saved_windows": saved, "select": "vedit-cli --window ID COMMAND", "restore": "vedit --window ID"}),
    )
}

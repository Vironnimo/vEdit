use super::*;

fn collect(task: &RunningTask) -> Vec<TaskEvent> {
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut events = Vec::new();
    loop {
        let event = task
            .receiver
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("command did not finish promptly");
        let finished = matches!(event, TaskEvent::Finished { .. });
        events.push(event);
        if finished {
            return events;
        }
    }
}

fn directory() -> PathBuf {
    std::env::current_dir().unwrap()
}

fn sleeping_command() -> String {
    #[cfg(windows)]
    {
        "Write-Output 'ready'; Start-Sleep -Seconds 20".into()
    }
    #[cfg(not(windows))]
    {
        "printf 'ready\\n'; sleep 20".into()
    }
}

#[test]
fn streams_stdout_and_stderr_and_preserves_exit_code() {
    #[cfg(windows)]
    let command =
        "[Console]::Out.WriteLine('hello'); [Console]::Error.WriteLine('problem'); exit 7";
    #[cfg(not(windows))]
    let command = "printf 'hello\\n'; printf 'problem\\n' >&2; exit 7";
    let task = spawn_task(directory(), command.into());
    let events = collect(&task);
    let output: String = events
        .iter()
        .filter_map(|event| match event {
            TaskEvent::Output(text) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(output.contains("hello"), "{events:?}");
    assert!(output.contains("problem"), "{events:?}");
    assert_eq!(
        events.last(),
        Some(&TaskEvent::Finished {
            code: Some(7),
            cancelled: false
        })
    );
}

#[test]
fn reports_a_missing_working_directory() {
    let root = directory().join("this-directory-must-not-exist-vedit-task-test");
    let task = spawn_task(root, "echo test".into());
    let events = collect(&task);
    assert!(matches!(events.first(), Some(TaskEvent::Error(_))));
    assert_eq!(
        events.last(),
        Some(&TaskEvent::Finished {
            code: None,
            cancelled: false
        })
    );
}

#[test]
fn direct_program_preserves_literal_arguments_environment_and_exit_code() {
    let root = std::env::temp_dir().join(format!(
        "vedit native task café & 'quoted' {} {}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let literal = "literal café & 'quote' $(must_not_execute)";
    #[cfg(windows)]
    let (program, script, text, mut args) = (
        "powershell.exe",
        root.join("native script.ps1"),
        "[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); [Console]::Out.WriteLine($args[0]); [Console]::Out.WriteLine($env:VEDIT_FILE); exit 13",
        vec![
            OsString::from("-NoProfile"),
            "-NonInteractive".into(),
            "-ExecutionPolicy".into(),
            "Bypass".into(),
            "-File".into(),
        ],
    );
    #[cfg(not(windows))]
    let (program, script, text, mut args) = (
        "/bin/sh",
        root.join("native script.sh"),
        "printf '%s\\n' \"$1\" \"$VEDIT_FILE\"; exit 13",
        Vec::new(),
    );
    std::fs::write(&script, text).unwrap();
    args.push(script.as_os_str().into());
    args.push(literal.into());
    let task = spawn_configured(
        root.clone(),
        CommandSpec::Program {
            program: program.into(),
            args,
        },
        vec![("VEDIT_FILE".into(), literal.into())],
    );
    let events = collect(&task);
    let _ = std::fs::remove_file(script);
    let _ = std::fs::remove_dir(root);
    let output: String = events
        .iter()
        .filter_map(|event| match event {
            TaskEvent::Output(text) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(output.matches(literal).count(), 2, "{events:?}");
    assert_eq!(
        events.last(),
        Some(&TaskEvent::Finished {
            code: Some(13),
            cancelled: false
        })
    );
}

#[test]
fn interactive_terminal_rejects_missing_directories_and_files() {
    let missing = directory().join("this-directory-must-not-exist-vedit-terminal-test");
    assert!(
        open_terminal(&missing, Vec::new())
            .unwrap_err()
            .contains("Could not open a terminal")
    );
    let file = directory().join("Cargo.toml");
    assert!(
        open_terminal(&file, Vec::new())
            .unwrap_err()
            .contains("not a project directory")
    );
}

#[test]
fn runs_in_a_directory_with_spaces_and_unicode() {
    let root = std::env::temp_dir().join(format!(
        "vedit task café & project {} {}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    #[cfg(windows)]
    let command = "[Console]::Out.WriteLine((Get-Location).Path)";
    #[cfg(not(windows))]
    let command = "pwd";
    let task = spawn_task(root.clone(), command.into());
    let events = collect(&task);
    std::fs::remove_dir(&root).unwrap();
    let output: String = events
        .iter()
        .filter_map(|event| match event {
            TaskEvent::Output(text) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(output.contains(root.to_str().unwrap()), "{events:?}");
    assert_eq!(
        events.last(),
        Some(&TaskEvent::Finished {
            code: Some(0),
            cancelled: false
        })
    );
}

#[test]
fn cancellation_stops_a_running_command_promptly() {
    let task = spawn_task(directory(), sleeping_command());
    loop {
        match task.receiver.recv_timeout(Duration::from_secs(10)).unwrap() {
            TaskEvent::Output(text) if text.contains("ready") => break,
            TaskEvent::Finished { .. } => panic!("command exited before cancellation"),
            _ => {}
        }
    }
    let started = Instant::now();
    task.cancel();
    let events = collect(&task);
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(
        events.last(),
        Some(&TaskEvent::Finished {
            code: None,
            cancelled: true
        })
    );
}

#[test]
fn timeout_stops_a_running_command_and_reports_the_reason() {
    let task = spawn_with_timeout(directory(), sleeping_command(), Duration::from_millis(500));
    let events = collect(&task);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, TaskEvent::Error(text) if text.contains("timed out")))
    );
    assert_eq!(
        events.last(),
        Some(&TaskEvent::Finished {
            code: None,
            cancelled: false
        })
    );
}

#[test]
fn output_budget_bounds_bytes_lines_and_emits_one_notice() {
    for source in [
        "🦀".repeat(MAX_OUTPUT_BYTES),
        "line\n".repeat(MAX_OUTPUT_LINES + 10),
    ] {
        let mut budget = OutputBudget::default();
        let result = budget.take(&source).concat();
        assert!(result.len() <= MAX_OUTPUT_BYTES);
        assert!(result.bytes().filter(|byte| *byte == b'\n').count() <= MAX_OUTPUT_LINES);
        assert!(result.ends_with(TRUNCATION_NOTICE));
        assert!(budget.take("extra output").is_empty());
    }
}

#[test]
fn truncated_reader_keeps_draining_the_pipe_and_emits_one_notice() {
    struct CountedReader {
        data: std::io::Cursor<Vec<u8>>,
        consumed: Arc<std::sync::atomic::AtomicUsize>,
    }
    impl Read for CountedReader {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            let count = self.data.read(bytes)?;
            self.consumed.fetch_add(count, Ordering::Relaxed);
            Ok(count)
        }
    }
    let source = "🦀 Grüße\r\n".repeat(MAX_OUTPUT_LINES * 3).into_bytes();
    let total = source.len();
    let consumed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (sender, receiver) = mpsc::sync_channel(128);
    let reader = spawn_reader(
        CountedReader {
            data: std::io::Cursor::new(source),
            consumed: consumed.clone(),
        },
        sender,
        Arc::new(Mutex::new(OutputBudget::default())),
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap();
    let output: String = receiver
        .into_iter()
        .map(|event| match event {
            TaskEvent::Output(text) => text,
            other => panic!("unexpected reader event: {other:?}"),
        })
        .collect();
    reader.join().unwrap();
    assert_eq!(consumed.load(Ordering::Relaxed), total);
    assert_eq!(output.matches(TRUNCATION_NOTICE).count(), 1);
    assert!(output.ends_with(TRUNCATION_NOTICE));
    assert!(!output.contains('\u{fffd}'));
    assert!(output.len() <= MAX_OUTPUT_BYTES);
    assert!(output.lines().count() <= MAX_OUTPUT_LINES);
}

#[test]
fn utf8_split_between_reads_is_preserved_and_invalid_bytes_are_replaced() {
    let mut decoder = Utf8Decoder::default();
    let source = "hello 🦀 schön".as_bytes();
    let mut decoded = String::new();
    for byte in source {
        decoded.push_str(&decoder.decode(&[*byte]).0);
    }
    decoded.push_str(&decoder.finish().0);
    assert_eq!(decoded, "hello 🦀 schön");
    assert_eq!(decoder.decode(&[0xff, b'x', 0xe2]).0, "\u{fffd}x");
    assert_eq!(decoder.finish().0, "\u{fffd}");
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::ffi::c_void;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, id: u32) -> *mut c_void;
        fn WaitForSingleObject(handle: *mut c_void, milliseconds: u32) -> u32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }

    fn descendant_task() -> (RunningTask, *mut c_void) {
        let task = spawn_task(
            directory(),
            "$info = [Diagnostics.ProcessStartInfo]::new('powershell.exe'); \
             $info.Arguments = '-NoProfile -NonInteractive -Command Start-Sleep -Seconds 20'; \
             $info.UseShellExecute = $false; $info.CreateNoWindow = $true; \
             $child = [Diagnostics.Process]::Start($info); \
             [Console]::Out.WriteLine($child.Id); Start-Sleep -Seconds 20"
                .into(),
        );
        let mut output = String::new();
        let id = loop {
            match task.receiver.recv_timeout(Duration::from_secs(10)).unwrap() {
                TaskEvent::Output(text) => {
                    output.push_str(&text);
                    if let Some(line) = output.lines().find(|_| output.contains('\n')) {
                        break line.trim().parse::<u32>().expect("child PID");
                    }
                }
                other => panic!("did not receive descendant PID: {other:?}"),
            }
        };
        let handle = unsafe { OpenProcess(0x0010_0000, 0, id) }; // SYNCHRONIZE
        assert!(!handle.is_null(), "descendant process was not alive");
        (task, handle)
    }

    fn assert_process_stopped(handle: *mut c_void) {
        let result = unsafe { WaitForSingleObject(handle, 5_000) };
        unsafe {
            CloseHandle(handle);
        }
        assert_eq!(result, 0, "descendant process survived task shutdown");
    }

    #[test]
    fn cancellation_kills_descendant_processes() {
        let (task, descendant) = descendant_task();
        task.cancel();
        collect(&task);
        assert_process_stopped(descendant);
    }

    #[test]
    fn dropping_the_task_kills_descendant_processes_without_polling() {
        let (task, descendant) = descendant_task();
        drop(task);
        assert_process_stopped(descendant);
    }
}

//! Background commands for the editor's output pane.
//!
//! Commands run in their project directory, with piped output and no console
//! window. Poll [`RunningTask::receiver`] from the UI; no UI code runs here.

use std::ffi::OsString;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

pub const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
pub const MAX_OUTPUT_LINES: usize = 10_000;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const POLL_INTERVAL: Duration = Duration::from_millis(25);
const TRUNCATION_NOTICE: &str = "\n[Output limit reached; further output omitted.]\n";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskEvent {
    /// A chunk of stdout or stderr, already decoded as UTF-8.
    Output(String),
    /// Final event, sent after the process stops and its output is drained.
    Finished { code: Option<i32>, cancelled: bool },
    /// A diagnostic; the task still sends a separate `Finished` event.
    Error(String),
}

pub struct RunningTask {
    pub receiver: Receiver<TaskEvent>,
    cancel_requested: Arc<AtomicBool>,
}

impl RunningTask {
    /// Request cancellation. The worker terminates the command's process tree.
    pub fn cancel(&self) {
        self.cancel_requested.store(true, Ordering::Release);
    }
}

impl Drop for RunningTask {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Start a user-supplied shell command without blocking the UI.
///
/// Windows uses PowerShell, other platforms use `/bin/sh`. The directory and
/// command are passed as process arguments rather than interpolated into a
/// second shell. Tasks stop after ten minutes. Output is bounded by both a
/// one-MiB budget and a 10,000-line budget, shared by stdout and stderr.
#[cfg(test)]
pub fn spawn_task(root: PathBuf, command: String) -> RunningTask {
    spawn_with_timeout(root, command, DEFAULT_TIMEOUT)
}

/// A built-in task is passed as an executable and arguments, never assembled
/// into shell source. Shell commands are reserved for explicitly edited tasks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandSpec {
    Shell(String),
    Program {
        program: OsString,
        args: Vec<OsString>,
    },
}

impl CommandSpec {
    pub fn display(&self) -> String {
        match self {
            Self::Shell(command) => command.clone(),
            Self::Program { program, args } => std::iter::once(program)
                .chain(args)
                .map(|value| format!("{:?}", value.to_string_lossy()))
                .collect::<Vec<_>>()
                .join(" "),
        }
    }
}

pub fn spawn_configured(
    root: PathBuf,
    command: CommandSpec,
    environment: Vec<(OsString, OsString)>,
) -> RunningTask {
    spawn_invocation(root, command, environment, DEFAULT_TIMEOUT)
}

/// Open an interactive terminal in a project directory.
///
/// This is deliberately a visible, independent window. Unlike output-pane
/// tasks, it remains open until the user closes the shell.
pub fn open_terminal(root: &Path, environment: Vec<(OsString, OsString)>) -> Result<(), String> {
    let metadata = root.metadata().map_err(|error| {
        format!(
            "Could not open a terminal in {}: {error}",
            crate::paths::display(root)
        )
    })?;
    if !metadata.is_dir() {
        return Err(format!(
            "{} is not a project directory.",
            crate::paths::display(root)
        ));
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut command = Command::new("powershell.exe");
        command
            .args(["-NoLogo", "-NoExit", "-NoProfile"])
            .current_dir(root)
            .envs(environment)
            .creation_flags(0x0000_0010); // CREATE_NEW_CONSOLE
        let mut child = command.spawn().map_err(|error| {
            format!(
                "Could not open PowerShell in {}: {error}",
                crate::paths::display(root)
            )
        })?;
        // Waiting in a detached worker releases the process handle when the
        // user exits, without blocking the editor or tying shell lifetime to it.
        let _ = thread::Builder::new()
            .name("vedit-terminal".into())
            .spawn(move || {
                let _ = child.wait();
            });
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = environment;
        Err("Opening an interactive terminal is currently supported on Windows.".into())
    }
}

#[cfg(test)]
fn spawn_with_timeout(root: PathBuf, command: String, timeout: Duration) -> RunningTask {
    spawn_invocation(root, CommandSpec::Shell(command), Vec::new(), timeout)
}

fn spawn_invocation(
    root: PathBuf,
    command: CommandSpec,
    environment: Vec<(OsString, OsString)>,
    timeout: Duration,
) -> RunningTask {
    // Bounded even if the UI is temporarily unable to poll. Readers use
    // cancellable sends so a full queue cannot prevent stopping a process.
    let (sender, receiver) = mpsc::sync_channel(128);
    let cancel_requested = Arc::new(AtomicBool::new(false));
    let task = RunningTask {
        receiver,
        cancel_requested: Arc::clone(&cancel_requested),
    };
    let fallback_sender = sender.clone();
    if let Err(error) = thread::Builder::new()
        .name("vedit-task".into())
        .spawn(move || {
            run_task(
                root,
                command,
                environment,
                timeout,
                sender,
                cancel_requested,
            )
        })
    {
        let _ = fallback_sender.send(TaskEvent::Error(format!(
            "Could not start the command worker: {error}"
        )));
        let _ = fallback_sender.send(TaskEvent::Finished {
            code: None,
            cancelled: false,
        });
    }
    task
}

fn run_task(
    root: PathBuf,
    command: CommandSpec,
    environment: Vec<(OsString, OsString)>,
    timeout: Duration,
    sender: SyncSender<TaskEvent>,
    cancel_requested: Arc<AtomicBool>,
) {
    let finish = |code, cancelled| {
        let _ = sender.send(TaskEvent::Finished { code, cancelled });
    };
    if cancel_requested.load(Ordering::Acquire) {
        finish(None, true);
        return;
    }
    if matches!(&command, CommandSpec::Shell(command) if command.trim().is_empty()) {
        let _ = sender.send(TaskEvent::Error("Enter a command to run.".into()));
        finish(None, false);
        return;
    }
    let mut process = match command {
        CommandSpec::Shell(command) => shell_command(&command),
        CommandSpec::Program { program, args } => {
            let mut process = Command::new(program);
            process.args(args);
            #[cfg(windows)]
            hide_window(&mut process);
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                process.process_group(0);
            }
            process
        }
    };
    process
        .current_dir(&root)
        .envs(environment)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = match process.spawn() {
        Ok(child) => child,
        Err(error) => {
            let _ = sender.send(TaskEvent::Error(format!(
                "Could not start the command in {}: {error}",
                crate::paths::display(&root)
            )));
            finish(None, false);
            return;
        }
    };

    let mut process = ProcessGuard::new(child);
    let budget = Arc::new(Mutex::new(OutputBudget::default()));
    let readers = [
        process.child.stdout.take().map(|pipe| {
            spawn_reader(
                pipe,
                sender.clone(),
                Arc::clone(&budget),
                Arc::clone(&cancel_requested),
            )
        }),
        process.child.stderr.take().map(|pipe| {
            spawn_reader(
                pipe,
                sender.clone(),
                Arc::clone(&budget),
                Arc::clone(&cancel_requested),
            )
        }),
    ];
    let started = Instant::now();
    let mut cancelled = false;
    let mut code = None;
    let mut failure = None;
    loop {
        if readers.iter().flatten().any(Result::is_err) {
            process.terminate();
            break;
        }
        if cancel_requested.load(Ordering::Acquire) {
            cancelled = true;
            process.terminate();
            break;
        }
        if started.elapsed() >= timeout {
            failure = Some(format!(
                "The command timed out after {} seconds and was stopped.",
                timeout.as_secs()
            ));
            process.terminate();
            break;
        }
        match process.child.try_wait() {
            Ok(Some(status)) => {
                code = status.code();
                break;
            }
            Ok(None) => thread::sleep(POLL_INTERVAL),
            Err(error) => {
                failure = Some(format!("Could not check command status: {error}"));
                process.terminate();
                break;
            }
        }
    }
    // A shell may exit while a background descendant still holds its output
    // pipes. End those descendants before waiting for the readers to drain.
    process.terminate();
    let _ = process.child.wait();
    if let Some(failure) = failure {
        // Never let a full output queue delay terminating the process.
        let _ = sender.send(TaskEvent::Error(failure));
    }
    for reader in readers.into_iter().flatten() {
        match reader {
            Ok(reader) => {
                if reader.join().is_err() {
                    let _ = sender.send(TaskEvent::Error(
                        "An output reader stopped unexpectedly.".into(),
                    ));
                }
            }
            Err(error) => {
                let _ = sender.send(TaskEvent::Error(format!(
                    "Could not read command output: {error}"
                )));
            }
        }
    }
    finish(code, cancelled);
}

fn shell_command(command: &str) -> Command {
    #[cfg(windows)]
    {
        let mut shell = Command::new("powershell.exe");
        shell.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command"]);
        // PowerShell 5 otherwise chooses the legacy console code page for
        // native programs such as cargo and Git.
        shell.arg(format!(
            "[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); $OutputEncoding = [Console]::OutputEncoding; {command}"
        ));
        hide_window(&mut shell);
        shell
    }
    #[cfg(not(windows))]
    {
        let mut shell = Command::new("/bin/sh");
        shell.args(["-c", command]);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            shell.process_group(0);
        }
        shell
    }
}

fn spawn_reader(
    mut pipe: impl Read + Send + 'static,
    sender: SyncSender<TaskEvent>,
    budget: Arc<Mutex<OutputBudget>>,
    cancel_requested: Arc<AtomicBool>,
) -> io::Result<thread::JoinHandle<()>> {
    thread::Builder::new()
        .name("vedit-task-output".into())
        .spawn(move || {
            let mut buffer = [0_u8; 8192];
            let mut decoder = Utf8Decoder::default();
            let mut truncated = false;
            loop {
                let text = match pipe.read(&mut buffer) {
                    Ok(0) => decoder.finish(),
                    // The shared budget never reopens. Keep draining the pipe
                    // so the command can finish, but avoid decoding/allocating
                    // every later chunk once this reader observes truncation.
                    Ok(_) if truncated => continue,
                    Ok(count) => decoder.decode(&buffer[..count]),
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => {
                        if !cancel_requested.load(Ordering::Acquire) {
                            let _ = send_output(
                                &sender,
                                TaskEvent::Error(format!("Could not read command output: {error}")),
                                &cancel_requested,
                            );
                        }
                        break;
                    }
                };
                let eof = text.1;
                if !text.0.is_empty() {
                    let mut budget = budget
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    let chunks = budget.take(&text.0);
                    truncated = budget.truncated;
                    for chunk in chunks {
                        if !send_output(&sender, TaskEvent::Output(chunk), &cancel_requested) {
                            return;
                        }
                    }
                }
                if eof {
                    break;
                }
            }
        })
}

fn send_output(
    sender: &SyncSender<TaskEvent>,
    mut event: TaskEvent,
    cancel_requested: &AtomicBool,
) -> bool {
    loop {
        match sender.try_send(event) {
            Ok(()) => return true,
            Err(TrySendError::Disconnected(_)) => return false,
            Err(TrySendError::Full(unsent)) => {
                if cancel_requested.load(Ordering::Acquire) {
                    return false;
                }
                event = unsent;
                thread::sleep(POLL_INTERVAL);
            }
        }
    }
}

#[derive(Default)]
struct OutputBudget {
    bytes: usize,
    newlines: usize,
    truncated: bool,
}

impl OutputBudget {
    fn take(&mut self, text: &str) -> Vec<String> {
        if self.truncated || text.is_empty() {
            return Vec::new();
        }
        let bytes_left = (MAX_OUTPUT_BYTES - TRUNCATION_NOTICE.len()).saturating_sub(self.bytes);
        let lines_left = (MAX_OUTPUT_LINES - 2).saturating_sub(self.newlines);
        let mut end = 0;
        let mut newlines = 0;
        for (index, character) in text.char_indices() {
            let next = index + character.len_utf8();
            if next > bytes_left || (character == '\n' && newlines == lines_left) {
                break;
            }
            end = next;
            newlines += usize::from(character == '\n');
        }
        let mut chunks = Vec::with_capacity(2);
        if end > 0 {
            chunks.push(text[..end].to_owned());
            self.bytes += end;
            self.newlines += newlines;
        }
        if end < text.len() {
            self.truncated = true;
            chunks.push(TRUNCATION_NOTICE.to_owned());
            self.bytes += TRUNCATION_NOTICE.len();
            self.newlines += 2;
        }
        chunks
    }
}

#[derive(Default)]
struct Utf8Decoder {
    pending: Vec<u8>,
}

impl Utf8Decoder {
    fn decode(&mut self, bytes: &[u8]) -> (String, bool) {
        self.pending.extend_from_slice(bytes);
        let mut text = String::new();
        let mut offset = 0;
        while offset < self.pending.len() {
            match std::str::from_utf8(&self.pending[offset..]) {
                Ok(valid) => {
                    text.push_str(valid);
                    offset = self.pending.len();
                }
                Err(error) => {
                    let valid_end = offset + error.valid_up_to();
                    // The decoder has verified that this prefix is valid.
                    text.push_str(
                        std::str::from_utf8(&self.pending[offset..valid_end]).unwrap_or_default(),
                    );
                    match error.error_len() {
                        Some(length) => {
                            text.push('\u{fffd}');
                            offset = valid_end + length;
                        }
                        None => {
                            offset = valid_end;
                            break;
                        }
                    }
                }
            }
        }
        self.pending.drain(..offset);
        (text, false)
    }

    fn finish(&mut self) -> (String, bool) {
        let text = String::from_utf8_lossy(&self.pending).into_owned();
        self.pending.clear();
        (text, true)
    }
}

#[cfg(windows)]
fn hide_window(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
}

struct ProcessGuard {
    child: Child,
    tree: ProcessTree,
}

impl ProcessGuard {
    fn new(child: Child) -> Self {
        Self {
            tree: ProcessTree::new(&child),
            child,
        }
    }

    fn terminate(&mut self) {
        self.tree.terminate(&mut self.child);
    }
}

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        self.terminate();
        let _ = self.child.wait();
    }
}

struct ProcessTree {
    #[cfg(windows)]
    job: Option<windows_job::Job>,
    #[cfg(unix)]
    id: u32,
    terminated: bool,
}

impl ProcessTree {
    fn new(child: &Child) -> Self {
        Self {
            #[cfg(windows)]
            job: windows_job::Job::assign(child),
            #[cfg(unix)]
            id: child.id(),
            terminated: false,
        }
    }

    fn terminate(&mut self, child: &mut Child) {
        if self.terminated {
            return;
        }
        self.terminated = true;
        #[cfg(windows)]
        {
            if let Some(job) = self.job.take() {
                // Closing the job kills every descendant, including children
                // whose parent shell has already exited.
                drop(job);
            } else if matches!(child.try_wait(), Ok(None)) {
                let mut killer = Command::new("taskkill.exe");
                killer.args(["/PID", &child.id().to_string(), "/T", "/F"]);
                killer
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                hide_window(&mut killer);
                let _ = killer.status();
            }
        }
        #[cfg(unix)]
        {
            let _ = Command::new("/bin/kill")
                .args(["-KILL", "--", &format!("-{}", self.id)])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        let _ = child.kill();
    }
}

#[cfg(windows)]
mod windows_job {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;

    type Handle = *mut c_void;

    #[repr(C)]
    #[derive(Default)]
    struct BasicLimits {
        process_time: i64,
        job_time: i64,
        flags: u32,
        minimum_working_set: usize,
        maximum_working_set: usize,
        active_processes: u32,
        affinity: usize,
        priority: u32,
        scheduling: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct ExtendedLimits {
        basic: BasicLimits,
        io_counters: [u64; 6],
        process_memory: usize,
        job_memory: usize,
        peak_process_memory: usize,
        peak_job_memory: usize,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> Handle;
        fn SetInformationJobObject(job: Handle, class: u32, data: *const c_void, size: u32) -> i32;
        fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
        fn CloseHandle(handle: Handle) -> i32;
    }

    pub(super) struct Job(Handle);

    impl Job {
        pub(super) fn assign(child: &Child) -> Option<Self> {
            // All handles stay on this worker. The pointers refer to live,
            // correctly laid out Win32 structures for the duration of calls.
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if handle.is_null() {
                    return None;
                }
                let job = Self(handle);
                let mut limits = ExtendedLimits::default();
                limits.basic.flags = 0x2000; // JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
                if SetInformationJobObject(
                    handle,
                    9, // JobObjectExtendedLimitInformation
                    (&limits as *const ExtendedLimits).cast(),
                    std::mem::size_of::<ExtendedLimits>() as u32,
                ) == 0
                    || AssignProcessToJobObject(handle, child.as_raw_handle().cast()) == 0
                {
                    return None;
                }
                Some(job)
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
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
}

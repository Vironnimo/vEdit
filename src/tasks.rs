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
    /// Application process was spawned; this does not imply service readiness.
    Started,
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
    spawn_invocation(root, command, environment, Some(DEFAULT_TIMEOUT))
}

/// Applications keep running until exit/cancellation. The caller retains a rolling log.
pub fn spawn_application(
    root: PathBuf,
    command: CommandSpec,
    environment: Vec<(OsString, OsString)>,
) -> RunningTask {
    spawn_invocation(root, command, environment, None)
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
    spawn_invocation(root, CommandSpec::Shell(command), Vec::new(), Some(timeout))
}

fn spawn_invocation(
    root: PathBuf,
    command: CommandSpec,
    environment: Vec<(OsString, OsString)>,
    timeout: Option<Duration>,
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
    timeout: Option<Duration>,
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
    if timeout.is_none() {
        let _ = sender.send(TaskEvent::Started);
    }
    let budget = Arc::new(Mutex::new(OutputBudget::new(timeout.is_none())));
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
        if let Some(timeout) = timeout
            && started.elapsed() >= timeout
        {
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
    // Termination has already completed. Drain captured output before the
    // final diagnostic so readers cannot append text after the reason for exit.
    if let Some(failure) = failure {
        let _ = sender.send(TaskEvent::Error(failure));
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

mod output;
mod process;

#[cfg(test)]
use output::Utf8Decoder;
use output::{OutputBudget, spawn_reader};
use process::ProcessGuard;
#[cfg(windows)]
use process::hide_window;

#[cfg(test)]
mod tests;

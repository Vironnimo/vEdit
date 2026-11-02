//! Own the PTY, shell and its descendants away from the UI thread.
use super::*;
use portable_pty::{Child, CommandBuilder, native_pty_system};
use std::{
    io::{Read, Write},
    path::PathBuf,
    time::Duration,
};

const SHELL_SETUP: &str = "try { Import-Module PSReadLine -ErrorAction Stop; Set-PSReadLineOption -HistorySavePath $env:VEDIT_TERMINAL_HISTORY -HistorySaveStyle SaveIncrementally -ErrorAction Stop; Set-Location -LiteralPath $env:VEDIT_TERMINAL_ROOT -ErrorAction Stop } catch { Write-Error $_; exit 1 }";

pub(super) struct ShellSpec {
    pub(super) root: PathBuf,
    pub(super) history_dir: PathBuf,
    pub(super) environment: Vec<(OsString, OsString)>,
}

pub(super) fn run(
    specification: ShellSpec,
    receiver: mpsc::Receiver<Command>,
    commands: mpsc::Sender<Command>,
    term: Arc<Mutex<Term<Proxy>>>,
    state: &Arc<Mutex<State>>,
    ctx: &egui::Context,
) -> Result<(), String> {
    let ShellSpec {
        root,
        history_dir,
        environment,
    } = specification;
    let root = root
        .canonicalize()
        .map_err(|error| format!("Could not open project folder: {error}"))?;
    if !root.is_dir() {
        return Err("The terminal requires a project folder".into());
    }
    std::fs::create_dir_all(&history_dir)
        .map_err(|error| format!("Could not create terminal history directory: {error}"))?;
    let history = history_dir.join(format!("{}.txt", project_key(&root)));
    let size = state.lock().unwrap().size;
    let pair = native_pty_system()
        .openpty(size)
        .map_err(|error| format!("Could not create terminal: {error}"))?;
    let mut command = CommandBuilder::new(shell_program(&environment));
    // Explicit native integration tests use private project/history roots and
    // must never execute the developer's personal startup profile.
    #[cfg(test)]
    command.arg("-NoProfile");
    command.args(["-NoLogo", "-NoExit", "-Command", SHELL_SETUP]);
    command.cwd(&root);
    for (key, value) in environment {
        command.env(key, value);
    }
    command.env("VEDIT_TERMINAL_HISTORY", &history);
    command.env("VEDIT_TERMINAL_ROOT", crate::paths::display(&root));
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    command.env("TERM_PROGRAM", "vEdit");
    let child = pair
        .slave
        .spawn_command(command)
        .map_err(|error| format!("Could not start PowerShell: {error}"))?;
    let mut child = OwnedChild::new(child)?;
    drop(pair.slave);
    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| error.to_string())?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|error| error.to_string())?;
    let (writes, write_receiver) = mpsc::channel::<Vec<u8>>();

    // A shell which is not reading stdin must not stop the owner from handling
    // Stop/resize. Closing its Job unblocks both pipe workers during teardown.
    let errors = commands.clone();
    thread::Builder::new()
        .name("vedit-terminal-input".into())
        .spawn(move || {
            let mut writer = writer;
            for bytes in write_receiver {
                if let Err(error) = writer.write_all(&bytes).and_then(|()| writer.flush()) {
                    let _ = errors.send(Command::Failed(format!("Terminal input failed: {error}")));
                    break;
                }
            }
        })
        .map_err(|error| format!("Could not start terminal input: {error}"))?;

    let context = ctx.clone();
    let errors = commands;
    thread::Builder::new()
        .name("vedit-terminal-output".into())
        .spawn(move || {
            read_output(reader, term, &context, &errors);
        })
        .map_err(|error| format!("Could not start terminal output: {error}"))?;

    state.lock().unwrap().status = "Running".into();
    ctx.request_repaint();
    let completion = loop {
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(Command::Write(bytes)) => {
                let _ = writes.send(bytes);
            }
            Ok(Command::Resize(size)) => {
                if let Err(error) = pair.master.resize(size) {
                    break format!("Terminal resize failed: {error}");
                }
            }
            Ok(Command::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                break "Stopped".into();
            }
            Ok(Command::Failed(error)) => break error,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        match child.child.try_wait() {
            Ok(Some(exit)) => break format!("Exited with code {}", exit.exit_code()),
            Ok(None) => {}
            Err(error) => break format!("Terminal process failed: {error}"),
        }
    };
    // Terminate descendants even when PowerShell itself has already exited.
    drop(child);
    drop(writes);
    drop(pair.master);
    let mut state = state.lock().unwrap();
    state.running = false;
    state.status = completion;
    ctx.request_repaint();
    Ok(())
}

fn read_output(
    mut reader: Box<dyn Read + Send>,
    term: Arc<Mutex<Term<Proxy>>>,
    ctx: &egui::Context,
    errors: &mpsc::Sender<Command>,
) {
    let mut parser: Processor = Processor::new();
    let mut buffer = [0_u8; 8192];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(length) => {
                parser.advance(&mut *term.lock().unwrap(), &buffer[..length]);
                ctx.request_repaint();
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            // Windows closes the output pipe when its process tree ends.
            Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => break,
            Err(error) => {
                let _ = errors.send(Command::Failed(format!("Terminal output failed: {error}")));
                break;
            }
        }
    }
}

/// Fixed FNV-1a over the canonical path, independent of Rust's Hash impl. Keep
/// it stable so the project's PowerShell history survives editor upgrades.
fn project_key(root: &Path) -> String {
    let identity = crate::paths::display(root);
    #[cfg(windows)]
    let identity = identity.to_lowercase();
    let hash = identity.bytes().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    });
    format!("{hash:016x}")
}

fn shell_program(environment: &[(OsString, OsString)]) -> OsString {
    #[cfg(windows)]
    {
        let path = environment
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case("PATH"))
            .map(|(_, value)| value.clone())
            .or_else(|| std::env::var_os("PATH"));
        if let Some(program) = path
            .as_ref()
            .into_iter()
            .flat_map(std::env::split_paths)
            .map(|directory| directory.join("pwsh.exe"))
            .find(|program| program.is_file())
        {
            return program.into_os_string();
        }
        if let Some(program_files) = std::env::var_os("ProgramFiles") {
            let program = PathBuf::from(program_files).join("PowerShell/7/pwsh.exe");
            if program.is_file() {
                return program.into_os_string();
            }
        }
        std::env::var_os("SystemRoot")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
            .join("System32/WindowsPowerShell/v1.0/powershell.exe")
            .into_os_string()
    }
    #[cfg(not(windows))]
    {
        let _ = environment;
        OsString::from("pwsh")
    }
}

struct OwnedChild {
    child: Box<dyn Child + Send + Sync>,
    #[cfg(windows)]
    job: Option<crate::tasks::process::windows_job::Job>,
}

impl OwnedChild {
    fn new(mut child: Box<dyn Child + Send + Sync>) -> Result<Self, String> {
        #[cfg(windows)]
        let job = child
            .as_raw_handle()
            .and_then(crate::tasks::process::windows_job::Job::assign_raw);
        #[cfg(windows)]
        if job.is_none() {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Could not attach the terminal process to its cleanup Job".into());
        }
        Ok(Self {
            child,
            #[cfg(windows)]
            job,
        })
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        #[cfg(windows)]
        drop(self.job.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

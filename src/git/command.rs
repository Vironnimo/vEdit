//! Bounded Git subprocesses, cancellation, and owned process trees.

use super::*;

#[cfg(all(test, windows))]
thread_local! {
    static TEST_DEADLINE_READY: std::cell::RefCell<Option<mpsc::Receiver<()>>> =
        const { std::cell::RefCell::new(None) };
}

/// Process-tree tests acquire the exact descendant handle before starting the
/// timeout under test. The ordinary bounded-command test still exercises the
/// unmodified startup deadline; production has no readiness gate.
#[cfg(all(test, windows))]
pub(super) fn run_bounded_after_ready(
    command: Command,
    timeout: Duration,
    output_limit: usize,
    operation: &str,
    ready: mpsc::Receiver<()>,
) -> GitResult<Output> {
    TEST_DEADLINE_READY.with(|slot| *slot.borrow_mut() = Some(ready));
    let result = run_bounded(command, timeout, output_limit, operation);
    let _ = TEST_DEADLINE_READY.with(|slot| slot.borrow_mut().take());
    result
}

pub(super) fn run_and_capture_change(
    root: &Path,
    args: &[&str],
    operation: &str,
) -> GitResult<String> {
    let output = mutate_git(root, &string_args(args), operation)?;
    Ok(output_text(&output))
}

pub(super) fn string_args(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

pub(super) fn validate_paths(paths: &[PathBuf], operation: &str) -> GitResult<()> {
    use std::path::Component;
    if paths.iter().any(|path| {
        path.as_os_str().is_empty()
            || path.components().any(|part| {
                matches!(
                    part,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
    }) {
        return Err(GitError::new(
            operation,
            "file paths must stay inside the repository",
        ));
    }
    Ok(())
}

pub(super) fn raw_git(root: &Path, args: &[OsString], operation: &str) -> GitResult<Output> {
    raw_git_with_input(root, args, operation, None)
}

pub(super) fn raw_git_with_input(
    root: &Path,
    args: &[OsString],
    operation: &str,
    input: Option<Vec<u8>>,
) -> GitResult<Output> {
    run_git(root, args, operation, input, Access::Query, OUTPUT_LIMIT)
}

/// Settings from user or repository configuration that would change the
/// machine-readable output vEdit parses. Each is pinned to Git's default.
/// Later `-c` arguments of an individual command still take precedence.
const PINNED_CONFIG: &[&str] = &[
    "credential.interactive=never",
    "core.askPass=",
    "color.ui=false",
    "color.diff=false",
    "color.status=false",
    "color.branch=false",
    // Signature verification output would precede formatted log records.
    "log.showSignature=false",
    "core.quotePath=true",
    "diff.noprefix=false",
    "diff.mnemonicPrefix=false",
    "diff.srcPrefix=a/",
    "diff.dstPrefix=b/",
    "diff.relative=false",
    "diff.suppressBlankEmpty=false",
    "status.relativePaths=true",
    "i18n.logOutputEncoding=UTF-8",
];

pub(super) fn run_git(
    root: &Path,
    args: &[OsString],
    operation: &str,
    input: Option<Vec<u8>>,
    access: Access,
    output_limit: usize,
) -> GitResult<Output> {
    #[cfg(test)]
    GIT_COMMAND_COUNT.with(|count| count.set(count.get() + 1));
    let mut command = Command::new("git");
    command.arg("--no-pager").arg("--literal-pathspecs");
    for setting in PINNED_CONFIG {
        command.arg("-c").arg(setting);
    }
    command
        .arg("-C")
        .arg(root.as_os_str())
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "Never")
        .env("GIT_ASKPASS", "")
        .env("SSH_ASKPASS_REQUIRE", "never")
        .env("GIT_MERGE_AUTOEDIT", "no")
        // Make Git's diagnostics deterministic for classification and display.
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env("LANGUAGE", "C")
        // Reading status should not contend with another Git process for an
        // optional index refresh lock.
        .env("GIT_OPTIONAL_LOCKS", "0");
    run_bounded_with_input(
        command,
        timeout(access),
        output_limit,
        operation,
        input,
        current_cancellation(),
    )
}

/// Run a change the user asked for. It has no short time limit and stops
/// early only through [`cancellable`]. A failure that leaves the index lock
/// behind explains how to recover; vEdit never removes the lock itself.
pub(super) fn mutate_git(root: &Path, args: &[OsString], operation: &str) -> GitResult<Output> {
    run_git(root, args, operation, None, Access::Mutation, OUTPUT_LIMIT)
        .and_then(|output| {
            if output.status.success() {
                Ok(output)
            } else {
                Err(command_error(operation, &output))
            }
        })
        .map_err(|error| with_index_lock_hint(root, error))
}

pub(super) fn with_index_lock_hint(root: &Path, mut error: GitError) -> GitError {
    // Git names the lock itself when another process holds it.
    if error.details.contains("index.lock") {
        return error;
    }
    if let Some(lock) = index_lock_path(root).filter(|lock| lock.is_file()) {
        error.details.push_str(&format!(
            "\n{} remains, so Git may have been interrupted. If no other Git program is working in this repository, delete that file before the next Git action. vEdit does not delete it.",
            crate::paths::display(&lock)
        ));
    }
    error
}

/// The index lock of the working tree at `root`. A linked worktree keeps its
/// index in the main repository's `.git/worktrees/<name>` directory.
pub(super) fn index_lock_path(root: &Path) -> Option<PathBuf> {
    let dot_git = root.join(".git");
    let git_dir = if dot_git.is_dir() {
        dot_git
    } else if let Ok(text) = std::fs::read_to_string(&dot_git) {
        let target = PathBuf::from(
            text.lines()
                .find_map(|line| line.strip_prefix("gitdir:"))?
                .trim(),
        );
        if target.is_absolute() {
            target
        } else {
            root.join(target)
        }
    } else {
        let output = raw_git(
            root,
            &string_args(&["rev-parse", "--git-path", "index.lock"]),
            "locate index lock",
        )
        .ok()
        .filter(|output| output.status.success())?;
        let path = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
        return Some(if path.is_absolute() {
            path
        } else {
            root.join(path)
        });
    };
    Some(git_dir.join("index.lock"))
}

/// Drain both pipes concurrently: waiting before reading can deadlock as soon
/// as a hook, diff or remote writes more than one pipe buffer. The limit and
/// deadline cover pipe draining as well as the Git process itself.
#[cfg(test)]
pub(super) fn run_bounded(
    command: Command,
    timeout: Duration,
    output_limit: usize,
    operation: &str,
) -> GitResult<Output> {
    run_bounded_with_input(command, timeout, output_limit, operation, None, None)
}

enum ReadFailure {
    TooLarge,
    Io(io::Error),
}

pub(super) fn run_bounded_with_input(
    mut command: Command,
    timeout: Duration,
    output_limit: usize,
    operation: &str,
    input: Option<Vec<u8>>,
    cancellation: Option<Cancellation>,
) -> GitResult<Output> {
    command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let child = command.spawn().map_err(|error| {
        GitError::new(
            operation,
            format!("could not start Git (is it installed and on PATH?): {error}"),
        )
    })?;
    let mut process = RunningProcess::new(child);
    let stdout = process.child.stdout.take().expect("stdout was piped");
    let stderr = process.child.stderr.take().expect("stderr was piped");
    let (sender, receiver) = mpsc::channel();
    let mut input_done = input.is_none();
    if let Some(input) = input {
        let mut stdin = process.child.stdin.take().expect("stdin was piped");
        let sender = sender.clone();
        thread::Builder::new()
            .name("git-input".into())
            .spawn(move || {
                use std::io::Write;
                let result = stdin
                    .write_all(&input)
                    .map(|()| Vec::new())
                    .map_err(ReadFailure::Io);
                drop(stdin);
                let _ = sender.send((2_u8, result));
            })
            .map_err(|error| {
                GitError::new(operation, format!("could not write Git input: {error}"))
            })?;
    }
    for (stream, reader) in [
        (0_u8, Box::new(stdout) as Box<dyn Read + Send>),
        (1_u8, Box::new(stderr) as Box<dyn Read + Send>),
    ] {
        let sender = sender.clone();
        thread::Builder::new()
            .name("git-output".into())
            .spawn(move || {
                let mut reader = reader;
                let mut bytes = Vec::new();
                let mut buffer = [0_u8; 8192];
                let result = loop {
                    match reader.read(&mut buffer) {
                        Ok(0) => break Ok(bytes),
                        Ok(length) => {
                            if bytes.len().saturating_add(length) > output_limit {
                                break Err(ReadFailure::TooLarge);
                            }
                            bytes.extend_from_slice(&buffer[..length]);
                        }
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                        Err(error) => break Err(ReadFailure::Io(error)),
                    }
                };
                let _ = sender.send((stream, result));
            })
            .map_err(|error| {
                GitError::new(operation, format!("could not read Git output: {error}"))
            })?;
    }
    drop(sender);
    #[cfg(all(test, windows))]
    if let Some(ready) = TEST_DEADLINE_READY.with(|slot| slot.borrow_mut().take()) {
        ready
            .recv_timeout(Duration::from_secs(10))
            .map_err(|error| {
                GitError::new(operation, format!("test writer readiness failed: {error}"))
            })?;
    }
    let deadline = Instant::now() + timeout;
    let mut status = None;
    let mut exited_at = None;
    let mut stdout = None;
    let mut stderr = None;
    loop {
        if status.is_none() {
            status = process.child.try_wait().map_err(|error| {
                GitError::new(operation, format!("could not wait for Git: {error}"))
            })?;
            if status.is_some() {
                exited_at = Some(Instant::now());
            }
        }
        if let Some(status) = status
            && stdout.is_some()
            && stderr.is_some()
            && input_done
        {
            // Helpers that Git started and that detached from its pipes, such
            // as the fsmonitor daemon or gpg-agent, keep running.
            process.release();
            return Ok(Output {
                status,
                stdout: stdout.take().unwrap(),
                stderr: stderr.take().unwrap(),
            });
        }
        if cancellation
            .as_ref()
            .is_some_and(Cancellation::is_cancelled)
        {
            return Err(GitError::new(
                operation,
                "cancelled; Git and the programs it started were stopped, so the repository may be partly updated",
            ));
        }
        if exited_at.is_some_and(|exited| exited.elapsed() >= EXIT_PIPE_GRACE) {
            // A background descendant still holds Git's output pipes. Stop it
            // so that the completed command's output can be read.
            exited_at = None;
            process.close_job();
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(GitError::new(operation, timeout_details(timeout)));
        }
        match receiver.recv_timeout(remaining.min(Duration::from_millis(10))) {
            Ok((stream, result)) => {
                let bytes = result.map_err(|failure| match failure {
                    ReadFailure::TooLarge => GitError {
                        output_too_large: true,
                        ..GitError::new(operation, "Git output is too large to display")
                    },
                    ReadFailure::Io(error) => GitError::new(operation, error.to_string()),
                })?;
                match stream {
                    0 => stdout = Some(bytes),
                    1 => stderr = Some(bytes),
                    _ => input_done = true,
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if stdout.is_none() || stderr.is_none() {
                    return Err(GitError::new(
                        operation,
                        "Git output reader stopped unexpectedly",
                    ));
                }
                thread::sleep(remaining.min(Duration::from_millis(10)));
            }
        }
    }
}

fn timeout_details(timeout: Duration) -> String {
    if timeout >= Duration::from_secs(120) {
        format!(
            "stopped after {} minutes; Git and the programs it started were stopped, so the repository may be partly updated",
            timeout.as_secs() / 60
        )
    } else {
        format!(
            "timed out after {} seconds; check remote connectivity or repository hooks and try again",
            timeout.as_secs()
        )
    }
}

struct RunningProcess {
    child: Child,
    #[cfg(windows)]
    job: Option<windows_job::Job>,
    /// Git exited normally and its remaining processes were released.
    released: bool,
}

impl RunningProcess {
    fn new(child: Child) -> Self {
        #[cfg(windows)]
        let job = windows_job::Job::attach(&child);
        Self {
            child,
            #[cfg(windows)]
            job,
            released: false,
        }
    }

    /// Stop every process that is still part of Git's process tree.
    fn close_job(&mut self) {
        #[cfg(windows)]
        drop(self.job.take());
        #[cfg(unix)]
        {
            unsafe extern "C" {
                fn kill(pid: i32, signal: i32) -> i32;
            }
            if let Ok(pid) = i32::try_from(self.child.id()) {
                unsafe {
                    kill(-pid, 9);
                }
            }
        }
    }

    /// Let processes that outlive a completed Git command keep running.
    fn release(&mut self) {
        #[cfg(windows)]
        if let Some(job) = self.job.take() {
            job.release();
        }
        self.released = true;
    }
}

impl Drop for RunningProcess {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        self.close_job();
        let _ = self.child.kill();
        // Reap the process with a short bound. Output readers are never joined
        // on a stuck pipe, even if job assignment is unavailable on this host.
        let deadline = Instant::now() + Duration::from_millis(100);
        while matches!(self.child.try_wait(), Ok(None)) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
    }
}

/// A Windows job ensures stopping Git also stops SSH, hooks and credential
/// helpers that inherited its pipes. No terminal window is needed to stop the
/// process tree. Closing the handle stops the tree on every error path,
/// including when vEdit exits while Git is still running.
#[cfg(windows)]
mod windows_job {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;

    const KILL_ON_JOB_CLOSE: u32 = 0x2000;
    const EXTENDED_LIMIT_INFORMATION: u32 = 9;

    #[repr(C)]
    struct BasicLimits {
        process_time: i64,
        job_time: i64,
        flags: u32,
        min_working_set: usize,
        max_working_set: usize,
        active_processes: u32,
        affinity: usize,
        priority: u32,
        scheduling: u32,
    }

    #[repr(C)]
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
        fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> *mut c_void;
        fn SetInformationJobObject(
            job: *mut c_void,
            class: u32,
            info: *const c_void,
            length: u32,
        ) -> i32;
        fn AssignProcessToJobObject(job: *mut c_void, process: *mut c_void) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }

    pub(super) struct Job(*mut c_void);

    impl Job {
        pub(super) fn attach(child: &Child) -> Option<Self> {
            let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if handle.is_null() {
                return None;
            }
            let job = Self(handle);
            if !job.set_flags(KILL_ON_JOB_CLOSE)
                || unsafe { AssignProcessToJobObject(handle, child.as_raw_handle()) } == 0
            {
                return None;
            }
            Some(job)
        }

        fn set_flags(&self, flags: u32) -> bool {
            // All fields are integer counters, flags or sizes; zero is valid.
            let mut limits: ExtendedLimits = unsafe { std::mem::zeroed() };
            limits.basic.flags = flags;
            unsafe {
                SetInformationJobObject(
                    self.0,
                    EXTENDED_LIMIT_INFORMATION,
                    &limits as *const _ as *const c_void,
                    std::mem::size_of::<ExtendedLimits>() as u32,
                ) != 0
            }
        }

        /// Close the job without stopping the processes that remain in it.
        /// If the limit cannot be cleared, closing stops them as before.
        pub(super) fn release(self) {
            self.set_flags(0);
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

pub(super) fn checked_git(root: &Path, args: &[OsString], operation: &str) -> GitResult<Output> {
    let output = raw_git(root, args, operation)?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(command_error(operation, &output))
    }
}

pub(super) fn command_error(operation: &str, output: &Output) -> GitError {
    let details = output_text(output);
    let details = if details.is_empty() {
        format!("process exited with {}", output.status)
    } else {
        details
    };
    GitError::new(operation, details)
}

pub(super) fn output_text(output: &Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    match (stdout.is_empty(), stderr.is_empty()) {
        (true, true) => String::new(),
        (false, true) => stdout,
        (true, false) => stderr,
        (false, false) => format!("{stdout}\n{stderr}"),
    }
}

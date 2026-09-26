//! Small, dependency-free Git CLI helpers for the editor's repository UI.
//!
//! Paths passed to [`stage_paths`] and [`unstage_paths`] are relative to the
//! repository root. Git is always invoked with `-C <root>` and arguments are
//! passed directly to the process, so paths and commit messages are never
//! interpreted by a shell.

use std::error::Error;
use std::ffi::OsString;
use std::fmt;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

const LOCAL_TIMEOUT: Duration = Duration::from_secs(15);
const NETWORK_TIMEOUT: Duration = Duration::from_secs(90);
const OUTPUT_LIMIT: usize = 8 * 1024 * 1024;

pub type GitResult<T> = Result<T, GitError>;

/// A readable error returned by a Git operation.
#[derive(Debug, Clone)]
pub struct GitError {
    operation: String,
    details: String,
}

impl GitError {
    fn new(operation: impl Into<String>, details: impl Into<String>) -> Self {
        Self {
            operation: operation.into(),
            details: details.into(),
        }
    }
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Git {} failed: {}", self.operation, self.details)
    }
}

impl Error for GitError {}

/// A concise classification of one side of a file's Git status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileChange {
    None,
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    Unmerged,
    Untracked,
    Other(char),
}

/// The staged (index) and unstaged (working tree) state for one path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStatus {
    /// Current path relative to the repository root, as reported by Git.
    pub path: String,
    /// Previous path for a detected rename or copy, when Git reports one.
    pub original_path: Option<String>,
    pub index: FileChange,
    pub worktree: FileChange,
}

impl FileStatus {
    pub fn is_staged(&self) -> bool {
        self.index != FileChange::None
    }

    pub fn is_unstaged(&self) -> bool {
        self.worktree != FileChange::None && self.worktree != FileChange::Untracked
    }

    pub fn is_untracked(&self) -> bool {
        self.worktree == FileChange::Untracked
    }

    pub fn is_conflicted(&self) -> bool {
        self.index == FileChange::Unmerged
            || self.worktree == FileChange::Unmerged
            || matches!(
                (self.index, self.worktree),
                (FileChange::Added, FileChange::Added) | (FileChange::Deleted, FileChange::Deleted)
            )
    }
}

/// Current branch and changed files for a repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoStatus {
    /// `None` when HEAD is detached.
    pub branch: Option<String>,
    pub files: Vec<FileStatus>,
}

/// A recent commit suitable for a compact history list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitSummary {
    pub hash: String,
    pub author: String,
    /// Date formatted by Git as `YYYY-MM-DD`.
    pub date: String,
    pub subject: String,
}

/// Tracking information for the current branch. Counts are based on the last
/// fetch, so reading this never makes a network request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchInfo {
    pub name: Option<String>,
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
}

/// Initialize a Git repository in `root`.
pub fn initialize(root: &Path) -> GitResult<()> {
    let args = string_args(&["init"]);
    checked_git(root, &args, "initialize repository")?;
    Ok(())
}

/// Returns the top-level working tree path, or `None` when `root` is not in a
/// repository.
pub fn repository_root(root: &Path) -> GitResult<Option<PathBuf>> {
    let args = string_args(&["rev-parse", "--show-toplevel"]);
    let output = raw_git(root, &args, "repository detection")?;
    if output.status.success() {
        let path = String::from_utf8_lossy(&output.stdout);
        return Ok(Some(PathBuf::from(path.trim_end_matches(['\r', '\n']))));
    }
    if output.status.code() == Some(128)
        && String::from_utf8_lossy(&output.stderr).contains("not a git repository")
    {
        return Ok(None);
    }
    Err(command_error("repository detection", &output))
}

/// Returns the current branch, or `None` when HEAD is detached.
pub fn current_branch(root: &Path) -> GitResult<Option<String>> {
    let args = string_args(&["branch", "--show-current"]);
    let output = checked_git(root, &args, "read current branch")?;
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    Ok((!branch.is_empty()).then_some(branch))
}

pub fn branch_info(root: &Path) -> GitResult<BranchInfo> {
    let name = current_branch(root)?;
    let mut info = BranchInfo {
        name,
        upstream: None,
        ahead: 0,
        behind: 0,
    };
    if info.name.is_none() {
        return Ok(info);
    }
    let args = string_args(&[
        "rev-parse",
        "--abbrev-ref",
        "--symbolic-full-name",
        "@{upstream}",
    ]);
    let output = raw_git(root, &args, "read upstream branch")?;
    if !output.status.success() {
        // Both a new repository and a branch without tracking configuration
        // report 128. They are normal states in the source-control panel.
        if output.status.code() == Some(128) {
            return Ok(info);
        }
        return Err(command_error("read upstream branch", &output));
    }
    info.upstream = Some(String::from_utf8_lossy(&output.stdout).trim().to_owned());
    let args = string_args(&["rev-list", "--left-right", "--count", "HEAD...@{upstream}"]);
    let output = checked_git(root, &args, "read branch divergence")?;
    let text = String::from_utf8_lossy(&output.stdout);
    let counts = text
        .split_whitespace()
        .map(str::parse::<usize>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| {
            GitError::new(
                "read branch divergence",
                "Git returned invalid commit counts",
            )
        })?;
    if counts.len() != 2 {
        return Err(GitError::new(
            "read branch divergence",
            "Git returned invalid commit counts",
        ));
    }
    info.ahead = counts[0];
    info.behind = counts[1];
    Ok(info)
}

/// Reads the current branch and concise staged, unstaged, and untracked file
/// status. Renames retain both the new path and Git's reported previous path.
pub fn status(root: &Path) -> GitResult<RepoStatus> {
    let args = string_args(&["status", "--porcelain=v1", "-z", "--untracked-files=all"]);
    let output = checked_git(root, &args, "read repository status")?;
    let branch = current_branch(root)?;
    let files = parse_status(&output.stdout)?;
    Ok(RepoStatus { branch, files })
}

/// Stage the given repository-relative paths.
pub fn stage_paths(root: &Path, paths: &[PathBuf]) -> GitResult<()> {
    if paths.is_empty() {
        return Ok(());
    }
    validate_paths(paths, "stage files")?;
    let mut args = string_args(&["add", "--"]);
    args.extend(paths.iter().map(|path| path.as_os_str().to_owned()));
    checked_git(root, &args, "stage files")?;
    Ok(())
}

/// Stage all additions, modifications, and deletions in the working tree.
pub fn stage_all(root: &Path) -> GitResult<()> {
    let args = string_args(&["add", "-A"]);
    checked_git(root, &args, "stage all files")?;
    Ok(())
}

/// Remove the given paths from the index while keeping working-tree contents.
pub fn unstage_paths(root: &Path, paths: &[PathBuf]) -> GitResult<()> {
    if paths.is_empty() {
        return Ok(());
    }
    unstage(root, paths)
}

/// Remove every staged path from the index while keeping working-tree contents.
pub fn unstage_all(root: &Path) -> GitResult<()> {
    unstage(root, &[PathBuf::from(".")])
}

/// Commit the current index with `message` and return Git's output.
pub fn commit(root: &Path, message: &str) -> GitResult<String> {
    if message.trim().is_empty() {
        return Err(GitError::new("commit", "enter a commit message first"));
    }
    let mut args = string_args(&["commit", "-m"]);
    args.push(OsString::from(message));
    let output = checked_git(root, &args, "commit")?;
    Ok(output_text(&output))
}

/// Push the current branch using the repository's configured Git defaults.
pub fn push(root: &Path) -> GitResult<String> {
    run_and_capture(root, &["push"], "push")
}

/// Fetch from the configured remotes.
pub fn fetch(root: &Path) -> GitResult<String> {
    run_and_capture(root, &["fetch"], "fetch")
}

/// Pull the configured upstream into the current branch.
pub fn pull(root: &Path) -> GitResult<String> {
    run_and_capture(root, &["pull", "--ff-only"], "pull")
}

/// Read a plain-text patch for a repository-relative path. External diff
/// programs and text-conversion commands are disabled so previewing a change
/// cannot open another application or run a repository-defined program.
pub fn diff(root: &Path, path: &Path, staged: bool) -> GitResult<String> {
    validate_paths(&[path.to_path_buf()], "read file diff")?;
    let mut args = string_args(&["diff", "--no-ext-diff", "--no-textconv", "--no-color"]);
    if staged {
        args.push(OsString::from("--cached"));
    }
    args.push(OsString::from("--"));
    args.push(path.as_os_str().to_owned());
    let output = checked_git(root, &args, "read file diff")?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Return up to `limit` commits, newest first.
pub fn recent_log(root: &Path, limit: usize) -> GitResult<Vec<CommitSummary>> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    // NUL separates fields and record separator separates commits, so spaces in
    // subjects/authors do not need quoting or parsing heuristics.
    let format = "%H%x00%an%x00%ad%x00%s%x1e";
    let mut args = string_args(&["log", "--date=short"]);
    args.push(OsString::from(format!("--format={format}")));
    args.push(OsString::from("-n"));
    args.push(OsString::from(limit.to_string()));
    let output = raw_git(root, &args, "read recent history")?;
    if !output.status.success() {
        // `git log` cannot run before the first commit. Verify that the branch
        // is unborn rather than concealing corrupt refs or missing directories.
        if output.status.code() == Some(128) && !has_head(root)? {
            return Ok(Vec::new());
        }
        return Err(command_error("read recent history", &output));
    }
    parse_log(&output.stdout)
}

fn unstage(root: &Path, paths: &[PathBuf]) -> GitResult<()> {
    validate_paths(paths, "unstage files")?;
    let mut args = if has_head(root)? {
        string_args(&["restore", "--staged", "--"])
    } else {
        // An unborn branch has no HEAD source for `git restore`; removing the
        // cached entries preserves the files on disk and makes them untracked.
        string_args(&["rm", "--cached", "-r", "--force", "--ignore-unmatch", "--"])
    };
    args.extend(paths.iter().map(|path| path.as_os_str().to_owned()));
    checked_git(root, &args, "unstage files")?;
    Ok(())
}

fn has_head(root: &Path) -> GitResult<bool> {
    let head = raw_git(
        root,
        &string_args(&["rev-parse", "--verify", "HEAD"]),
        "check repository HEAD",
    )?;
    if head.status.success() {
        return Ok(true);
    }
    let branch = raw_git(
        root,
        &string_args(&["symbolic-ref", "--quiet", "HEAD"]),
        "check unborn branch",
    )?;
    if branch.status.success() {
        let reference = String::from_utf8_lossy(&branch.stdout).trim().to_owned();
        let exists = raw_git(
            root,
            &string_args(&["show-ref", "--verify", "--quiet", &reference]),
            "check unborn branch",
        )?;
        if exists.status.code() == Some(1) {
            return Ok(false);
        }
    }
    Err(command_error("check repository HEAD", &head))
}

fn run_and_capture(root: &Path, args: &[&str], operation: &str) -> GitResult<String> {
    let args = string_args(args);
    let output = checked_git(root, &args, operation)?;
    Ok(output_text(&output))
}

fn string_args(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

fn validate_paths(paths: &[PathBuf], operation: &str) -> GitResult<()> {
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

fn raw_git(root: &Path, args: &[OsString], operation: &str) -> GitResult<Output> {
    let mut command = Command::new("git");
    command
        .arg("--no-pager")
        .arg("--literal-pathspecs")
        .args([
            "-c",
            "credential.interactive=never",
            "-c",
            "core.askPass=",
            "-c",
            "color.ui=false",
        ])
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
        // Reading status should not contend with another Git process for an
        // optional index refresh lock.
        .env("GIT_OPTIONAL_LOCKS", "0");
    let network = args
        .first()
        .is_some_and(|arg| matches!(arg.to_str(), Some("push" | "fetch" | "pull")));
    run_bounded(
        command,
        if network {
            NETWORK_TIMEOUT
        } else {
            LOCAL_TIMEOUT
        },
        OUTPUT_LIMIT,
        operation,
    )
}

/// Drain both pipes concurrently: waiting before reading can deadlock as soon
/// as a hook, diff or remote writes more than one pipe buffer. The limit and
/// deadline cover pipe draining as well as the Git process itself.
fn run_bounded(
    mut command: Command,
    timeout: Duration,
    output_limit: usize,
    operation: &str,
) -> GitResult<Output> {
    command
        .stdin(Stdio::null())
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
    for (is_stdout, reader) in [
        (true, Box::new(stdout) as Box<dyn Read + Send>),
        (false, Box::new(stderr) as Box<dyn Read + Send>),
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
                                break Err(io::Error::other("Git output is too large to display"));
                            }
                            bytes.extend_from_slice(&buffer[..length]);
                        }
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                        Err(error) => break Err(error),
                    }
                };
                let _ = sender.send((is_stdout, result));
            })
            .map_err(|error| {
                GitError::new(operation, format!("could not read Git output: {error}"))
            })?;
    }
    drop(sender);
    let deadline = Instant::now() + timeout;
    let mut status = None;
    let mut stdout = None;
    let mut stderr = None;
    loop {
        if status.is_none() {
            status = process.child.try_wait().map_err(|error| {
                GitError::new(operation, format!("could not wait for Git: {error}"))
            })?;
            if status.is_some() {
                process.close_job();
            }
        }
        if let Some(status) = status
            && stdout.is_some()
            && stderr.is_some()
        {
            return Ok(Output {
                status,
                stdout: stdout.take().unwrap(),
                stderr: stderr.take().unwrap(),
            });
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(GitError::new(
                operation,
                format!(
                    "timed out after {} seconds; check remote connectivity or repository hooks and try again",
                    timeout.as_secs()
                ),
            ));
        }
        match receiver.recv_timeout(remaining.min(Duration::from_millis(10))) {
            Ok((is_stdout, result)) => {
                let bytes = result.map_err(|error| GitError::new(operation, error.to_string()))?;
                if is_stdout {
                    stdout = Some(bytes);
                } else {
                    stderr = Some(bytes);
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

struct RunningProcess {
    child: Child,
    #[cfg(windows)]
    job: Option<windows_job::Job>,
}

impl RunningProcess {
    fn new(child: Child) -> Self {
        #[cfg(windows)]
        let job = windows_job::Job::attach(&child);
        Self {
            child,
            #[cfg(windows)]
            job,
        }
    }

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
}

impl Drop for RunningProcess {
    fn drop(&mut self) {
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

/// A Windows job ensures timing out Git also stops SSH, hooks and credential
/// helpers that inherited its pipes. No terminal window is needed to stop the
/// process tree, and closing the handle guarantees cleanup on every error path.
#[cfg(windows)]
mod windows_job {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;

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
            // All fields are integer counters, flags or sizes; zero is valid.
            let mut limits: ExtendedLimits = unsafe { std::mem::zeroed() };
            limits.basic.flags = 0x2000; // JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if handle.is_null() {
                    return None;
                }
                let job = Self(handle);
                if SetInformationJobObject(
                    handle,
                    9,
                    &limits as *const _ as *const c_void,
                    std::mem::size_of::<ExtendedLimits>() as u32,
                ) == 0
                    || AssignProcessToJobObject(handle, child.as_raw_handle()) == 0
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

fn checked_git(root: &Path, args: &[OsString], operation: &str) -> GitResult<Output> {
    let output = raw_git(root, args, operation)?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(command_error(operation, &output))
    }
}

fn command_error(operation: &str, output: &Output) -> GitError {
    let details = output_text(output);
    let details = if details.is_empty() {
        format!("process exited with {}", output.status)
    } else {
        details
    };
    GitError::new(operation, details)
}

fn output_text(output: &Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    match (stdout.is_empty(), stderr.is_empty()) {
        (true, true) => String::new(),
        (false, true) => stdout,
        (true, false) => stderr,
        (false, false) => format!("{stdout}\n{stderr}"),
    }
}

fn parse_status(bytes: &[u8]) -> GitResult<Vec<FileStatus>> {
    let mut records = bytes.split(|byte| *byte == 0);
    let mut files = Vec::new();
    while let Some(record) = records.next() {
        if record.is_empty() {
            continue;
        }
        if record.len() < 4 || record[2] != b' ' {
            return Err(GitError::new(
                "parse repository status",
                "Git returned an unexpected porcelain status record",
            ));
        }
        let x = record[0] as char;
        let y = record[1] as char;
        let path = String::from_utf8_lossy(&record[3..]).into_owned();
        let is_rename_or_copy = matches!(x, 'R' | 'C') || matches!(y, 'R' | 'C');
        let original_path = if is_rename_or_copy {
            let previous = records
                .next()
                .filter(|previous| !previous.is_empty())
                .ok_or_else(|| {
                    GitError::new(
                        "parse repository status",
                        "Git returned a rename without its original path",
                    )
                })?;
            Some(String::from_utf8_lossy(previous).into_owned())
        } else {
            None
        };
        let (index, worktree) = if x == '?' && y == '?' {
            (FileChange::None, FileChange::Untracked)
        } else {
            (change_from_code(x), change_from_code(y))
        };
        files.push(FileStatus {
            path,
            original_path,
            index,
            worktree,
        });
    }
    Ok(files)
}

fn change_from_code(code: char) -> FileChange {
    match code {
        ' ' => FileChange::None,
        'M' => FileChange::Modified,
        'A' => FileChange::Added,
        'D' => FileChange::Deleted,
        'R' => FileChange::Renamed,
        'C' => FileChange::Copied,
        'U' => FileChange::Unmerged,
        '?' => FileChange::Untracked,
        other => FileChange::Other(other),
    }
}

fn parse_log(bytes: &[u8]) -> GitResult<Vec<CommitSummary>> {
    let mut commits = Vec::new();
    for record in bytes.split(|byte| *byte == 0x1e) {
        let record = record
            .iter()
            .skip_while(|byte| **byte == b'\r' || **byte == b'\n')
            .copied()
            .collect::<Vec<_>>();
        if record.is_empty() {
            continue;
        }
        let mut fields = record.split(|byte| *byte == 0);
        let hash = fields.next();
        let author = fields.next();
        let date = fields.next();
        let subject = fields.next();
        match (hash, author, date, subject) {
            (Some(hash), Some(author), Some(date), Some(subject)) => {
                commits.push(CommitSummary {
                    hash: String::from_utf8_lossy(hash).trim().to_owned(),
                    author: String::from_utf8_lossy(author).into_owned(),
                    date: String::from_utf8_lossy(date).into_owned(),
                    subject: String::from_utf8_lossy(subject)
                        .trim_end_matches(['\r', '\n'])
                        .to_owned(),
                });
            }
            _ => {
                return Err(GitError::new(
                    "parse recent history",
                    "Git returned an unexpected log record",
                ));
            }
        }
    }
    Ok(commits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static NEXT_REPOSITORY: AtomicU64 = AtomicU64::new(0);

    struct TestRepository(PathBuf);

    impl TestRepository {
        fn empty_directory() -> Self {
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "morrow-git-tests-{}-{timestamp}-{} with spaces",
                std::process::id(),
                NEXT_REPOSITORY.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn new() -> Self {
            let repository = Self::empty_directory();
            initialize(&repository.0).unwrap();
            repository.git(&["symbolic-ref", "HEAD", "refs/heads/main"]);
            repository.git(&["config", "user.name", "Morrow Git Test"]);
            repository.git(&["config", "user.email", "morrow-tests@example.invalid"]);
            repository.git(&["config", "commit.gpgsign", "false"]);
            repository.git(&["config", "core.autocrlf", "false"]);
            let hooks = repository.0.join("test-hooks");
            fs::create_dir(&hooks).unwrap();
            repository.git(&["config", "core.hooksPath", hooks.to_str().unwrap()]);
            fs::write(
                repository.0.join(".gitignore"),
                "test-hooks/\nhook-completed.txt\n",
            )
            .unwrap();
            repository
        }

        fn git(&self, args: &[&str]) -> Output {
            checked_git(&self.0, &string_args(args), "test fixture").unwrap()
        }

        fn write(&self, path: &str, contents: &str) {
            fs::write(self.0.join(path), contents).unwrap();
        }

        fn commit_all(&self, message: &str) {
            stage_all(&self.0).unwrap();
            commit(&self.0, message).unwrap();
        }
    }

    impl Drop for TestRepository {
        fn drop(&mut self) {
            // Only remove the uniquely named directory created by this fixture.
            assert_eq!(self.0.parent(), Some(std::env::temp_dir().as_path()));
            assert!(
                self.0
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("morrow-git-tests-")
            );
            for _ in 0..3 {
                if fs::remove_dir_all(&self.0).is_ok() {
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
        }
    }

    #[test]
    fn detects_repository_from_a_nested_directory() {
        let repository = TestRepository::new();
        let nested = repository.0.join("src");
        fs::create_dir(&nested).unwrap();
        let detected = repository_root(&nested).unwrap().unwrap();
        assert_eq!(
            fs::canonicalize(detected).unwrap(),
            fs::canonicalize(&repository.0).unwrap()
        );
        let outside = TestRepository::empty_directory();
        assert_eq!(repository_root(&outside.0).unwrap(), None);
        assert!(recent_log(&outside.0, 6).is_err());
        assert!(repository_root(&outside.0.join("missing-directory")).is_err());
    }

    #[test]
    fn new_repository_has_a_branch_and_empty_history() {
        let repository = TestRepository::new();
        assert_eq!(
            current_branch(&repository.0).unwrap().as_deref(),
            Some("main")
        );
        assert!(recent_log(&repository.0, 6).unwrap().is_empty());
        assert_eq!(
            branch_info(&repository.0).unwrap(),
            BranchInfo {
                name: Some("main".into()),
                upstream: None,
                ahead: 0,
                behind: 0,
            }
        );
    }

    #[test]
    fn staging_and_unstaging_before_first_commit_preserve_files() {
        let repository = TestRepository::new();
        let file = "--über file.rs";
        repository.write(file, "fn main() {}\n");
        stage_paths(&repository.0, &[PathBuf::from(file)]).unwrap();
        let repository_status = status(&repository.0).unwrap();
        assert!(
            repository_status
                .files
                .iter()
                .any(|entry| entry.path == file && entry.is_staged())
        );
        unstage_paths(&repository.0, &[PathBuf::from(file)]).unwrap();
        assert_eq!(
            fs::read_to_string(repository.0.join(file)).unwrap(),
            "fn main() {}\n"
        );
        assert!(
            status(&repository.0)
                .unwrap()
                .files
                .iter()
                .any(|entry| entry.path == file && entry.is_untracked())
        );
        stage_all(&repository.0).unwrap();
        unstage_all(&repository.0).unwrap();
        assert!(
            status(&repository.0)
                .unwrap()
                .files
                .iter()
                .all(FileStatus::is_untracked)
        );
    }

    #[test]
    fn staged_and_worktree_diffs_show_the_correct_versions() {
        let repository = TestRepository::new();
        repository.write("code.rs", "original\n");
        repository.commit_all("First commit");
        repository.write("code.rs", "staged version\n");
        stage_paths(&repository.0, &[PathBuf::from("code.rs")]).unwrap();
        repository.write("code.rs", "working version\n");
        let entry = status(&repository.0)
            .unwrap()
            .files
            .into_iter()
            .find(|entry| entry.path == "code.rs")
            .unwrap();
        assert!(entry.is_staged() && entry.is_unstaged());
        let staged = diff(&repository.0, Path::new("code.rs"), true).unwrap();
        assert!(staged.contains("-original\n") && staged.contains("+staged version\n"));
        let worktree = diff(&repository.0, Path::new("code.rs"), false).unwrap();
        assert!(worktree.contains("-staged version\n") && worktree.contains("+working version\n"));
        unstage_all(&repository.0).unwrap();
        assert_eq!(
            fs::read_to_string(repository.0.join("code.rs")).unwrap(),
            "working version\n"
        );
    }

    #[test]
    fn diff_preview_never_runs_external_diff_or_textconv_helpers() {
        let repository = TestRepository::new();
        repository.write(".gitattributes", "code.txt diff=custom\n");
        repository.write("code.txt", "original\n");
        repository.commit_all("Initial");
        repository.git(&["config", "diff.external", "morrow-nonexistent-diff-program"]);
        repository.git(&[
            "config",
            "diff.custom.textconv",
            "morrow-nonexistent-textconv-program",
        ]);
        repository.write("code.txt", "changed\n");
        let patch = diff(&repository.0, Path::new("code.txt"), false).unwrap();
        assert!(patch.contains("+changed\n"));
    }

    #[test]
    fn corrupt_head_does_not_look_like_an_unborn_branch_or_modify_the_index() {
        let repository = TestRepository::new();
        repository.write("code.rs", "original\n");
        repository.commit_all("Initial");
        repository.write("code.rs", "staged\n");
        stage_paths(&repository.0, &[PathBuf::from("code.rs")]).unwrap();
        let index = fs::read(repository.0.join(".git/index")).unwrap();
        fs::write(repository.0.join(".git/refs/heads/main"), "invalid ref\n").unwrap();
        assert!(recent_log(&repository.0, 6).is_err());
        assert!(unstage_all(&repository.0).is_err());
        assert_eq!(fs::read(repository.0.join(".git/index")).unwrap(), index);
    }

    #[test]
    fn renames_keep_both_paths_and_history_preserves_messages() {
        let repository = TestRepository::new();
        repository.write("old name.txt", "same contents\n");
        repository.commit_all("Initial subject with spaces and ü");
        fs::rename(
            repository.0.join("old name.txt"),
            repository.0.join("new name.txt"),
        )
        .unwrap();
        stage_all(&repository.0).unwrap();
        let renamed = status(&repository.0)
            .unwrap()
            .files
            .into_iter()
            .find(|entry| entry.path == "new name.txt")
            .unwrap();
        assert_eq!(renamed.index, FileChange::Renamed);
        assert_eq!(renamed.original_path.as_deref(), Some("old name.txt"));
        commit(&repository.0, "Rename with a literal $(shell) message").unwrap();
        let log = recent_log(&repository.0, 2).unwrap();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].subject, "Rename with a literal $(shell) message");
        assert_eq!(log[1].subject, "Initial subject with spaces and ü");
        assert_eq!(log[0].author, "Morrow Git Test");
        assert_eq!(log[0].hash.len(), 40);
        assert_eq!(log[0].date.len(), 10);
        repository.git(&["checkout", "--detach"]);
        assert_eq!(branch_info(&repository.0).unwrap().name, None);
    }

    #[test]
    fn branch_tracking_counts_ahead_and_behind_without_fetching() {
        let repository = TestRepository::new();
        repository.write("common.txt", "initial\n");
        repository.commit_all("Initial");
        repository.git(&["branch", "tracking"]);
        repository.git(&["config", "branch.main.remote", "."]);
        repository.git(&["config", "branch.main.merge", "refs/heads/tracking"]);
        repository.write("main.txt", "main\n");
        repository.commit_all("Main branch commit");
        repository.git(&["checkout", "tracking"]);
        repository.write("upstream.txt", "upstream\n");
        repository.commit_all("Tracking branch commit");
        repository.git(&["checkout", "main"]);
        assert_eq!(
            branch_info(&repository.0).unwrap(),
            BranchInfo {
                name: Some("main".into()),
                upstream: Some("tracking".into()),
                ahead: 1,
                behind: 1,
            }
        );
        assert!(pull(&repository.0).is_err());
        assert_eq!(
            fs::read_to_string(repository.0.join("main.txt")).unwrap(),
            "main\n"
        );
        assert!(!repository.0.join("upstream.txt").exists());
    }

    #[test]
    fn relative_paths_cannot_escape_the_repository() {
        let repository = TestRepository::new();
        for path in [
            PathBuf::from("../outside.txt"),
            repository.0.join("inside.txt"),
            PathBuf::new(),
        ] {
            assert!(stage_paths(&repository.0, std::slice::from_ref(&path)).is_err());
            assert!(unstage_paths(&repository.0, std::slice::from_ref(&path)).is_err());
            assert!(diff(&repository.0, &path, false).is_err());
        }
        assert!(commit(&repository.0, " \n\t").is_err());
        assert!(stage_paths(&repository.0, &[]).is_ok());
    }

    #[test]
    fn rejects_malformed_status_records_and_preserves_conflicts() {
        assert!(parse_status(b"R  new.txt\0").is_err());
        assert!(parse_status(b"M\0").is_err());
        let files = parse_status(
            b"UU conflict.txt\0?? untracked.txt\0AA added-twice.txt\0DD deleted-twice.txt\0",
        )
        .unwrap();
        assert_eq!(files[0].index, FileChange::Unmerged);
        assert_eq!(files[0].worktree, FileChange::Unmerged);
        assert!(files[1].is_untracked());
        assert!(!files[1].is_staged());
        assert!(files[0].is_conflicted() && files[2].is_conflicted() && files[3].is_conflicted());
        assert!(!files[1].is_conflicted());
    }

    fn script(script: &str) -> Command {
        #[cfg(windows)]
        let mut command = Command::new("powershell.exe");
        #[cfg(not(windows))]
        let mut command = Command::new("sh");
        #[cfg(windows)]
        command.args(["-NoProfile", "-NonInteractive", "-Command", script]);
        #[cfg(not(windows))]
        command.args(["-c", script]);
        command
    }

    #[test]
    fn slow_commands_are_bounded() {
        #[cfg(windows)]
        let command = script("Start-Sleep -Seconds 30");
        #[cfg(not(windows))]
        let command = script("sleep 30");
        let started = Instant::now();
        let error =
            run_bounded(command, Duration::from_millis(250), 1024, "slow test").unwrap_err();
        assert!(error.to_string().contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn both_output_streams_are_drained_and_oversized_output_is_rejected() {
        #[cfg(windows)]
        let command =
            script("[Console]::Out.Write(('o' * 100000)); [Console]::Error.Write(('e' * 100000))");
        #[cfg(not(windows))]
        let command = script(
            "head -c 100000 /dev/zero | tr '\\0' o; head -c 100000 /dev/zero | tr '\\0' e >&2",
        );
        let output = run_bounded(command, Duration::from_secs(5), 200000, "output test").unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout.len(), 100000);
        assert_eq!(output.stderr.len(), 100000);
        #[cfg(windows)]
        let command = script("[Console]::Out.Write(('x' * 100000))");
        #[cfg(not(windows))]
        let command = script("head -c 100000 /dev/zero");
        let error =
            run_bounded(command, Duration::from_secs(5), 1024, "large output test").unwrap_err();
        assert!(error.to_string().contains("too large"));
    }

    #[cfg(windows)]
    #[test]
    fn timing_out_git_stops_its_hook_process_tree() {
        let repository = TestRepository::new();
        repository.write("file.txt", "contents\n");
        stage_all(&repository.0).unwrap();
        fs::write(
            repository.0.join("test-hooks/pre-commit"),
            "#!/bin/sh\nsleep 2\nprintf 'still alive' > hook-completed.txt\n",
        )
        .unwrap();
        let mut command = Command::new("git");
        command
            .arg("-C")
            .arg(&repository.0)
            .args(["commit", "-m", "Blocked by hook"]);
        let error = run_bounded(
            command,
            Duration::from_millis(300),
            OUTPUT_LIMIT,
            "commit hook test",
        )
        .unwrap_err();
        assert!(error.to_string().contains("timed out"));
        thread::sleep(Duration::from_millis(2300));
        assert!(
            !repository.0.join("hook-completed.txt").exists(),
            "the timed-out hook must not continue running"
        );
        assert!(recent_log(&repository.0, 1).unwrap().is_empty());
    }
}

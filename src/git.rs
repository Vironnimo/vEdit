//! Small, dependency-free Git CLI helpers for the editor's repository UI.
//!
//! Paths passed to [`stage_paths`] and [`unstage_paths`] are relative to the
//! repository root. Git is always invoked with `-C <root>` and arguments are
//! passed directly to the process, so paths and commit messages are never
//! interpreted by a shell.

use std::collections::HashMap;
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
const UNTRACKED_FILE_LIMIT: usize = 1024 * 1024;
const UNTRACKED_TOTAL_LIMIT: usize = 8 * 1024 * 1024;
const UNTRACKED_PATH_LIMIT: usize = 128;

#[cfg(test)]
thread_local! {
    static GIT_COMMAND_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

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
    pub parents: Vec<String>,
    pub refs: Vec<CommitRef>,
    pub author: String,
    /// Author timestamp in ISO 8601, including its original time zone.
    pub date: String,
    pub timestamp: i64,
    /// Reachable from a remote-tracking ref in the frozen history snapshot.
    pub on_remote: bool,
    pub subject: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RefKind {
    Local,
    Remote,
    Tag,
    Head,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitRef {
    pub hash: String,
    pub name: String,
    pub kind: RefKind,
    pub current: bool,
}

/// Immutable traversal roots and decorations. Pages use these object IDs, so a
/// fetch or commit between requests cannot reorder an already displayed graph.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HistorySnapshot {
    pub tips: Vec<String>,
    pub refs: Vec<CommitRef>,
    pub local_only: std::sync::Arc<std::collections::HashSet<String>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HistoryPage {
    pub commits: Vec<CommitSummary>,
    pub has_more: bool,
}

pub const HISTORY_PAGE_SIZE: usize = 60;

/// Line changes reported by Git. Binary files deliberately have no line counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LineStats {
    pub added: u64,
    pub removed: u64,
    pub binary: bool,
}

/// Compact commit totals for history previews. Binary and rename changes each
/// count as one changed file; `lines.binary` means at least one is binary.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CommitStats {
    pub files_changed: usize,
    pub lines: LineStats,
}

/// Separate comparisons of HEAD to index and index to working tree. Untracked
/// text files are counted in `unstaged`, within a bounded file and byte budget.
/// A missing entry means its statistics could not be computed, not zero changes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkingTreeStats {
    pub staged: HashMap<String, LineStats>,
    pub unstaged: HashMap<String, LineStats>,
}

/// One committed change; rename and copy entries preserve the previous path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitFile {
    pub path: String,
    pub original_path: Option<String>,
    pub status: FileChange,
    pub stats: LineStats,
}

/// Full commit metadata and changes compared with its first parent. A root
/// commit is compared with the empty tree, so its files are all additions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitDetails {
    pub hash: String,
    pub parents: Vec<String>,
    pub author: String,
    pub author_email: String,
    /// ISO 8601 author timestamp, including its time zone.
    pub date: String,
    pub message: String,
    pub files: Vec<CommitFile>,
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

#[cfg(test)]
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
#[cfg(test)]
pub fn status(root: &Path) -> GitResult<RepoStatus> {
    let files = status_files(root)?;
    let branch = current_branch(root)?;
    Ok(RepoStatus { branch, files })
}

/// Read file status and local branch tracking in the same Git process.
pub fn status_with_tracking(root: &Path) -> GitResult<(RepoStatus, BranchInfo)> {
    let args = string_args(&[
        "status",
        "--porcelain=v2",
        "--branch",
        "--ahead-behind",
        "-z",
        "--untracked-files=all",
        "--renames",
    ]);
    let output = checked_git(root, &args, "read repository status")?;
    parse_status_with_tracking(&output.stdout)
}

/// Count changed paths without reading branch, diff statistics or history.
pub fn uncommitted_change_count(root: &Path) -> GitResult<usize> {
    Ok(status_files(root)?.len())
}

fn status_files(root: &Path) -> GitResult<Vec<FileStatus>> {
    let args = string_args(&[
        "status",
        "--porcelain=v1",
        "-z",
        "--untracked-files=all",
        "--renames",
    ]);
    let output = checked_git(root, &args, "read repository status")?;
    parse_status(&output.stdout)
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
    tracked_diff(root, path, None, staged)
}

/// Preview a status entry with both rename paths, or an untracked addition.
pub fn status_diff(root: &Path, file: &FileStatus, staged: bool) -> GitResult<String> {
    if !staged && file.is_untracked() {
        return untracked_diff(root, Path::new(&file.path));
    }
    tracked_diff(
        root,
        Path::new(&file.path),
        (if staged { file.index } else { file.worktree })
            .eq(&FileChange::Renamed)
            .then(|| file.original_path.as_deref().map(Path::new))
            .flatten(),
        staged,
    )
}

fn tracked_diff(
    root: &Path,
    path: &Path,
    original_path: Option<&Path>,
    staged: bool,
) -> GitResult<String> {
    let mut paths = vec![path.to_path_buf()];
    if let Some(original_path) = original_path {
        paths.push(original_path.to_path_buf());
    }
    validate_paths(&paths, "read file diff")?;
    let mut args = string_args(&[
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "--find-renames",
    ]);
    if staged {
        args.push(OsString::from("--cached"));
    }
    args.push(OsString::from("--"));
    args.extend(paths.iter().map(|path| path.as_os_str().to_owned()));
    let output = checked_git(root, &args, "read file diff")?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Read each tracked comparison only when the status includes changes for it.
/// NUL-delimited paths retain Unicode and control bytes.
pub fn working_tree_stats(root: &Path, files: &[FileStatus]) -> GitResult<WorkingTreeStats> {
    let common = [
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "--numstat",
        "-z",
        "--find-renames",
    ];
    let mut statistics = WorkingTreeStats::default();
    if files.iter().any(FileStatus::is_staged) {
        let mut staged_args = string_args(&common);
        staged_args.push(OsString::from("--cached"));
        let staged = checked_git(root, &staged_args, "read staged line statistics")?;
        statistics.staged = parse_numstat(&staged.stdout)?.into_iter().collect();
    }
    if files.iter().any(FileStatus::is_unstaged) {
        let unstaged = checked_git(
            root,
            &string_args(&common),
            "read working tree line statistics",
        )?;
        statistics.unstaged = parse_numstat(&unstaged.stdout)?.into_iter().collect();
    }
    let mut remaining = UNTRACKED_TOTAL_LIMIT;
    for file in files
        .iter()
        .filter(|file| file.is_untracked())
        .take(UNTRACKED_PATH_LIMIT)
    {
        let limit = remaining.min(UNTRACKED_FILE_LIMIT);
        if limit == 0 {
            break;
        }
        // A concurrently removed file, a link outside the repository, or a
        // large untracked asset does not invalidate the tracked statistics.
        if let Ok(bytes) = read_untracked_bytes(root, Path::new(&file.path), limit) {
            remaining = remaining.saturating_sub(bytes.len());
            statistics
                .unstaged
                .insert(file.path.clone(), added_file_stats(&bytes));
        }
    }
    Ok(statistics)
}

/// Read compact statistics without loading commit metadata, changed-file
/// statuses, or patches. Merge commits compare with their first parent, and
/// root commits compare with the empty tree, just like `commit_details`.
pub fn commit_stats(root: &Path, full_hash: &str) -> GitResult<CommitStats> {
    validate_commit_hash(full_hash)?;
    let mut args = string_args(&[
        "diff-tree",
        "--no-commit-id",
        "-r",
        "--root",
        "--diff-merges=first-parent",
        "--no-ext-diff",
        "--no-textconv",
        "--find-renames",
        "--numstat",
        "-z",
    ]);
    // Reject tree/blob IDs as well as mutable revision expressions. The input
    // is already hexadecimal, so this suffix cannot introduce Git options.
    args.push(OsString::from(format!("{full_hash}^{{commit}}")));
    args.push(OsString::from("--"));
    let output = checked_git(root, &args, "read commit statistics")?;
    let statistics = parse_numstat(&output.stdout)?;
    let mut totals = CommitStats {
        files_changed: statistics.len(),
        ..CommitStats::default()
    };
    for (_, stats) in statistics {
        totals.lines.added = totals.lines.added.saturating_add(stats.added);
        totals.lines.removed = totals.lines.removed.saturating_add(stats.removed);
        totals.lines.binary |= stats.binary;
    }
    Ok(totals)
}

/// Inspect an immutable full commit ID. Ref expressions and option-like strings
/// are rejected so the caller cannot accidentally inspect a moving revision.
pub fn commit_details(root: &Path, full_hash: &str) -> GitResult<CommitDetails> {
    validate_commit_hash(full_hash)?;
    let metadata = checked_git(
        root,
        &string_args(&[
            "show",
            "--no-patch",
            "--no-notes",
            "--format=%H%x00%P%x00%an%x00%ae%x00%aI%x00%B%x00",
            full_hash,
            "--",
        ]),
        "read commit details",
    )?;
    let mut details = parse_commit_metadata(&metadata.stdout)?;
    let comparison = commit_comparison_args(&details);
    let mut names_args = string_args(&[
        "diff-tree",
        "--no-commit-id",
        "-r",
        "--root",
        "--no-ext-diff",
        "--no-textconv",
        "--find-renames",
        "--name-status",
        "-z",
    ]);
    names_args.extend(comparison.iter().cloned());
    names_args.push(OsString::from("--"));
    let names = checked_git(root, &names_args, "read committed files")?;
    let mut stats_args = string_args(&[
        "diff-tree",
        "--no-commit-id",
        "-r",
        "--root",
        "--no-ext-diff",
        "--no-textconv",
        "--find-renames",
        "--numstat",
        "-z",
    ]);
    stats_args.extend(comparison);
    stats_args.push(OsString::from("--"));
    let statistics = checked_git(root, &stats_args, "read committed line statistics")?;
    let statistics: HashMap<_, _> = parse_numstat(&statistics.stdout)?.into_iter().collect();
    details.files = parse_commit_files(&names.stdout)?;
    for file in &mut details.files {
        file.stats = statistics.get(&file.path).copied().unwrap_or_default();
    }
    Ok(details)
}

/// Preview the selected commit's patch, preserving renames and comparing merge
/// commits with the same first parent used by `commit_details`.
pub fn commit_diff(root: &Path, full_hash: &str, path: &Path) -> GitResult<String> {
    validate_paths(&[path.to_path_buf()], "read commit diff")?;
    let details = commit_details(root, full_hash)?;
    let Some(file) = details
        .files
        .iter()
        .find(|file| Path::new(&file.path) == path)
    else {
        return Err(GitError::new(
            "read commit diff",
            "the selected file is not changed in this commit",
        ));
    };
    commit_file_diff(root, &details, file)
}

/// Preview a file from already loaded commit details in one Git invocation.
/// Both commit identity and repository-relative paths are revalidated, while
/// immutable object IDs let callers safely cache metadata across refreshes.
pub fn commit_file_diff(
    root: &Path,
    details: &CommitDetails,
    file: &CommitFile,
) -> GitResult<String> {
    validate_commit_hash(&details.hash)?;
    for parent in &details.parents {
        validate_commit_hash(parent)?;
    }
    if !details.files.contains(file) {
        return Err(GitError::new(
            "read commit diff",
            "the selected file does not belong to these commit details",
        ));
    }
    let mut paths = vec![PathBuf::from(&file.path)];
    if file.status == FileChange::Renamed
        && let Some(original_path) = &file.original_path
    {
        paths.push(PathBuf::from(original_path));
    }
    validate_paths(&paths, "read commit diff")?;
    let mut args = string_args(&[
        "diff-tree",
        "--no-commit-id",
        "-r",
        "--root",
        "-p",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "--find-renames",
    ]);
    args.extend(commit_comparison_args(details));
    args.push(OsString::from("--"));
    args.extend(paths.iter().map(|path| path.as_os_str().to_owned()));
    let output = checked_git(root, &args, "read commit diff")?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Preview a small untracked file as an addition without staging it or invoking
/// attributes, text-conversion programs, or an external diff tool.
pub fn untracked_diff(root: &Path, path: &Path) -> GitResult<String> {
    let bytes = read_untracked_bytes(root, path, UNTRACKED_FILE_LIMIT)?;
    let stats = added_file_stats(&bytes);
    let display_path = path.to_string_lossy().replace('\\', "/");
    let before = format!("a/{display_path}");
    let after = format!("b/{display_path}");
    let mut patch = format!("diff --git {before:?} {after:?}\nnew file mode 100644\n");
    if stats.binary {
        patch.push_str(&format!("Binary files /dev/null and {after:?} differ\n"));
        return Ok(patch);
    }
    patch.push_str(&format!("--- /dev/null\n+++ {after:?}\n"));
    if !bytes.is_empty() {
        patch.push_str(&format!("@@ -0,0 +1,{} @@\n", stats.added));
        let text = String::from_utf8_lossy(&bytes);
        for line in text.split_inclusive('\n') {
            patch.push('+');
            patch.push_str(line);
        }
        if !bytes.ends_with(b"\n") {
            patch.push_str("\n\\ No newline at end of file\n");
        }
    }
    Ok(patch)
}

fn validate_commit_hash(hash: &str) -> GitResult<()> {
    if !matches!(hash.len(), 40 | 64) || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(GitError::new(
            "read commit",
            "a full hexadecimal commit ID is required",
        ));
    }
    Ok(())
}

fn commit_comparison_args(details: &CommitDetails) -> Vec<OsString> {
    let mut args = Vec::new();
    if let Some(parent) = details.parents.first() {
        args.push(OsString::from(parent));
    }
    args.push(OsString::from(&details.hash));
    args
}

fn read_untracked_bytes(root: &Path, path: &Path, limit: usize) -> GitResult<Vec<u8>> {
    const OPERATION: &str = "read untracked file";
    validate_paths(&[path.to_path_buf()], OPERATION)?;
    let read_error = |error: io::Error| GitError::new(OPERATION, error.to_string());
    let absolute = root.join(path);
    let metadata = std::fs::symlink_metadata(&absolute).map_err(read_error)?;
    if !metadata.file_type().is_file() || metadata.len() > limit as u64 {
        return Err(GitError::new(
            OPERATION,
            "preview is limited to regular files up to 1 MiB",
        ));
    }
    let canonical_root = std::fs::canonicalize(root).map_err(read_error)?;
    let canonical_file = std::fs::canonicalize(&absolute).map_err(read_error)?;
    if !canonical_file.starts_with(canonical_root) {
        return Err(GitError::new(
            OPERATION,
            "the file resolves outside the repository",
        ));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(canonical_file)
        .map_err(read_error)?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(read_error)?;
    if bytes.len() > limit {
        return Err(GitError::new(
            OPERATION,
            "the file grew beyond the preview limit",
        ));
    }
    Ok(bytes)
}

fn added_file_stats(bytes: &[u8]) -> LineStats {
    if bytes.iter().take(8000).any(|byte| *byte == 0) {
        return LineStats {
            binary: true,
            ..LineStats::default()
        };
    }
    LineStats {
        added: bytes.iter().filter(|byte| **byte == b'\n').count() as u64
            + u64::from(!bytes.is_empty() && !bytes.ends_with(b"\n")),
        removed: 0,
        binary: false,
    }
}

/// Return up to `limit` commits in topological order across repository branches.
#[cfg(test)]
pub fn recent_log(root: &Path, limit: usize) -> GitResult<Vec<CommitSummary>> {
    let snapshot = history_snapshot(root)?;
    Ok(history_page(root, &snapshot, 0, limit)?.commits)
}

pub fn history_snapshot(root: &Path) -> GitResult<HistorySnapshot> {
    history_snapshot_cached(root, None)
}

pub fn history_snapshot_cached(
    root: &Path,
    previous: Option<&HistorySnapshot>,
) -> GitResult<HistorySnapshot> {
    let head = read_head(root)?;
    let output = checked_git(
        root,
        &string_args(&[
            "for-each-ref",
            "--format=%(objectname)%00%(refname)%00%(symref)%00%(*objectname)%00%(*objecttype)%00%(objecttype)%00%(HEAD)",
            "refs/heads",
            "refs/remotes",
            "refs/tags",
        ]),
        "read history branches",
    )?;
    let mut snapshot = HistorySnapshot::default();
    for line in output.stdout.split(|byte| *byte == b'\n') {
        if line.is_empty() {
            continue;
        }
        let fields: Vec<_> = line.split(|byte| *byte == 0).collect();
        if fields.len() != 7 {
            return Err(GitError::new("read history branches", "invalid ref record"));
        }
        // origin/HEAD is an alias of a remote branch, not an extra branch tip.
        if !fields[2].is_empty() {
            continue;
        }
        let name = String::from_utf8_lossy(fields[1]);
        let (name, kind) = if let Some(name) = name.strip_prefix("refs/heads/") {
            (name, RefKind::Local)
        } else if let Some(name) = name.strip_prefix("refs/remotes/") {
            (name, RefKind::Remote)
        } else if let Some(name) = name.strip_prefix("refs/tags/") {
            (name, RefKind::Tag)
        } else {
            continue;
        };
        let hash = if kind == RefKind::Tag && !fields[3].is_empty() {
            if fields[4] != b"commit" {
                continue;
            }
            fields[3]
        } else {
            if fields[5] != b"commit" {
                continue;
            }
            fields[0]
        };
        let hash = String::from_utf8_lossy(hash).into_owned();
        validate_commit_hash(&hash)?;
        snapshot.refs.push(CommitRef {
            current: kind == RefKind::Local && fields[6] == b"*" && head.as_deref() == Some(&hash),
            hash,
            name: name.to_owned(),
            kind,
        });
    }
    if let Some(head) = head {
        validate_commit_hash(&head)?;
        if !snapshot.refs.iter().any(|reference| reference.current) {
            snapshot.refs.push(CommitRef {
                hash: head,
                name: "HEAD".into(),
                kind: RefKind::Head,
                current: true,
            });
        }
    }
    snapshot.refs.sort_by(|left, right| {
        (!left.current, left.kind, &left.name).cmp(&(!right.current, right.kind, &right.name))
    });
    snapshot.tips = snapshot
        .refs
        .iter()
        .map(|reference| reference.hash.clone())
        .collect();
    snapshot.tips.sort();
    snapshot.tips.dedup();
    if let Some(previous) = previous
        && snapshot.tips == previous.tips
        && snapshot.refs == previous.refs
    {
        snapshot.local_only = previous.local_only.clone();
        return Ok(snapshot);
    }
    let remote_tips: Vec<_> = snapshot
        .refs
        .iter()
        .filter(|reference| reference.kind == RefKind::Remote)
        .map(|reference| reference.hash.as_str())
        .collect();
    if !remote_tips.is_empty() {
        let mut input = snapshot.tips.join("\n") + "\n";
        for hash in remote_tips {
            input.push('^');
            input.push_str(hash);
            input.push('\n');
        }
        let output = raw_git_with_input(
            root,
            &string_args(&["rev-list", "--stdin"]),
            "compare local and remote history",
            Some(input.into_bytes()),
        )?;
        if !output.status.success() {
            return Err(command_error("compare local and remote history", &output));
        }
        snapshot.local_only = std::sync::Arc::new(
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .map(str::to_owned)
                .collect(),
        );
    }
    Ok(snapshot)
}

pub fn history_page(
    root: &Path,
    snapshot: &HistorySnapshot,
    offset: usize,
    limit: usize,
) -> GitResult<HistoryPage> {
    if limit == 0 || snapshot.tips.is_empty() {
        return Ok(HistoryPage::default());
    }
    for hash in &snapshot.tips {
        validate_commit_hash(hash)?;
    }
    let mut args = string_args(&[
        "log",
        "--topo-order",
        "--date=short",
        "--no-show-signature",
        "--format=%H%x00%P%x00%an%x00%aI%x00%at%x00%s%x00",
        "--stdin",
    ]);
    args.push(OsString::from(format!("--skip={offset}")));
    args.push(OsString::from(format!(
        "--max-count={}",
        limit.saturating_add(1)
    )));
    // stdin avoids Windows' command-line length limit for repositories with
    // thousands of branches. Only validated full object IDs enter this input.
    let input = snapshot.tips.join("\n") + "\n";
    let output = raw_git_with_input(root, &args, "read history page", Some(input.into_bytes()))?;
    if !output.status.success() {
        return Err(command_error("read history page", &output));
    }
    let mut commits = parse_log(&output.stdout)?;
    let has_more = commits.len() > limit;
    commits.truncate(limit);
    for commit in &mut commits {
        commit.on_remote = snapshot.refs.iter().any(|r| r.kind == RefKind::Remote)
            && !snapshot.local_only.contains(&commit.hash);
        commit.refs = snapshot
            .refs
            .iter()
            .filter(|reference| reference.hash == commit.hash)
            .cloned()
            .collect();
    }
    Ok(HistoryPage { commits, has_more })
}

/// Switch a named local branch or create a tracking branch from an exact remote ref.
/// Git itself refuses worktree conflicts; never discard, force, stash or reset.
pub fn switch_branch(root: &Path, target: &str) -> GitResult<String> {
    let snapshot = history_snapshot(root)?;
    let reference = snapshot
        .refs
        .iter()
        .find(|reference| {
            let prefix = match reference.kind {
                RefKind::Local => "refs/heads/",
                RefKind::Remote => "refs/remotes/",
                _ => return false,
            };
            target == format!("{prefix}{}", reference.name)
        })
        .ok_or_else(|| {
            GitError::new(
                "switch branch",
                "branch no longer exists; refresh the branch list",
            )
        })?;
    let args = match reference.kind {
        RefKind::Local => string_args(&["switch", "--no-guess", "--", &reference.name]),
        RefKind::Remote => string_args(&["switch", "--track", "--", target]),
        _ => unreachable!(),
    };
    checked_git(root, &args, "switch branch")?;
    Ok(current_branch(root)?.unwrap_or_default())
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
    Ok(read_head(root)?.is_some())
}

fn read_head(root: &Path) -> GitResult<Option<String>> {
    let head = raw_git(
        root,
        &string_args(&["rev-parse", "--verify", "HEAD"]),
        "check repository HEAD",
    )?;
    if head.status.success() {
        let hash = String::from_utf8_lossy(&head.stdout).trim().to_owned();
        validate_commit_hash(&hash)?;
        return Ok(Some(hash));
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
            return Ok(None);
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
    raw_git_with_input(root, args, operation, None)
}

fn raw_git_with_input(
    root: &Path,
    args: &[OsString],
    operation: &str,
    input: Option<Vec<u8>>,
) -> GitResult<Output> {
    #[cfg(test)]
    GIT_COMMAND_COUNT.with(|count| count.set(count.get() + 1));
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
    run_bounded_with_input(
        command,
        if network {
            NETWORK_TIMEOUT
        } else {
            LOCAL_TIMEOUT
        },
        OUTPUT_LIMIT,
        operation,
        input,
    )
}

/// Drain both pipes concurrently: waiting before reading can deadlock as soon
/// as a hook, diff or remote writes more than one pipe buffer. The limit and
/// deadline cover pipe draining as well as the Git process itself.
#[cfg(test)]
fn run_bounded(
    command: Command,
    timeout: Duration,
    output_limit: usize,
    operation: &str,
) -> GitResult<Output> {
    run_bounded_with_input(command, timeout, output_limit, operation, None)
}

fn run_bounded_with_input(
    mut command: Command,
    timeout: Duration,
    output_limit: usize,
    operation: &str,
    input: Option<Vec<u8>>,
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
                let result = stdin.write_all(&input).map(|()| Vec::new());
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
                                break Err(io::Error::other("Git output is too large to display"));
                            }
                            bytes.extend_from_slice(&buffer[..length]);
                        }
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                        Err(error) => break Err(error),
                    }
                };
                let _ = sender.send((stream, result));
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
            && input_done
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
            Ok((stream, result)) => {
                let bytes = result.map_err(|error| GitError::new(operation, error.to_string()))?;
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

fn nul_records<'a>(bytes: &'a [u8], operation: &str) -> GitResult<Vec<&'a [u8]>> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    let Some(bytes) = bytes.strip_suffix(&[0]) else {
        return Err(GitError::new(
            operation,
            "Git returned a truncated NUL-delimited record",
        ));
    };
    Ok(bytes.split(|byte| *byte == 0).collect())
}

/// `--numstat -z` emits a normal path in the third field, but a rename has an
/// empty third field followed by two distinct NUL-delimited paths. Splitting
/// the whole output on tabs or lines would corrupt valid file names.
fn parse_numstat(bytes: &[u8]) -> GitResult<Vec<(String, LineStats)>> {
    const OPERATION: &str = "parse line statistics";
    let records = nul_records(bytes, OPERATION)?;
    let mut records = records.into_iter();
    let mut statistics = Vec::new();
    while let Some(record) = records.next() {
        let mut fields = record.splitn(3, |byte| *byte == b'\t');
        let added = fields.next().unwrap_or_default();
        let removed = fields
            .next()
            .ok_or_else(|| GitError::new(OPERATION, "missing removed line count"))?;
        let path = fields
            .next()
            .ok_or_else(|| GitError::new(OPERATION, "missing file path"))?;
        let stats = if added == b"-" && removed == b"-" {
            LineStats {
                binary: true,
                ..LineStats::default()
            }
        } else {
            let parse_count = |count: &[u8]| -> GitResult<u64> {
                std::str::from_utf8(count)
                    .ok()
                    .and_then(|text| text.parse().ok())
                    .ok_or_else(|| GitError::new(OPERATION, "invalid line count"))
            };
            LineStats {
                added: parse_count(added)?,
                removed: parse_count(removed)?,
                binary: false,
            }
        };
        let path = if path.is_empty() {
            let old = records.next().filter(|record| !record.is_empty());
            let new = records.next().filter(|record| !record.is_empty());
            match (old, new) {
                (Some(_), Some(new)) => new,
                _ => return Err(GitError::new(OPERATION, "incomplete rename paths")),
            }
        } else {
            path
        };
        statistics.push((String::from_utf8_lossy(path).into_owned(), stats));
    }
    Ok(statistics)
}

fn parse_commit_metadata(bytes: &[u8]) -> GitResult<CommitDetails> {
    const OPERATION: &str = "parse commit details";
    let fields: Vec<_> = bytes.splitn(7, |byte| *byte == 0).collect();
    if fields.len() != 7 || !fields[6].iter().all(|byte| matches!(byte, b'\r' | b'\n')) {
        return Err(GitError::new(
            OPERATION,
            "Git returned unexpected commit metadata",
        ));
    }
    let string = |index| String::from_utf8_lossy(fields[index]).into_owned();
    let hash = string(0);
    validate_commit_hash(&hash)?;
    let parents: Vec<_> = string(1).split_whitespace().map(str::to_owned).collect();
    for parent in &parents {
        validate_commit_hash(parent)?;
    }
    Ok(CommitDetails {
        hash,
        parents,
        author: string(2),
        author_email: string(3),
        date: string(4),
        message: string(5),
        files: Vec::new(),
    })
}

fn parse_commit_files(bytes: &[u8]) -> GitResult<Vec<CommitFile>> {
    const OPERATION: &str = "parse committed files";
    let records = nul_records(bytes, OPERATION)?;
    let mut records = records.into_iter();
    let mut files = Vec::new();
    while let Some(code) = records.next() {
        let Some(code) = code.first().copied() else {
            return Err(GitError::new(OPERATION, "empty file status"));
        };
        let path = records
            .next()
            .filter(|path| !path.is_empty())
            .ok_or_else(|| GitError::new(OPERATION, "missing file path"))?;
        let (original_path, path) = if matches!(code, b'R' | b'C') {
            let next = records
                .next()
                .filter(|path| !path.is_empty())
                .ok_or_else(|| GitError::new(OPERATION, "missing renamed file path"))?;
            (Some(String::from_utf8_lossy(path).into_owned()), next)
        } else {
            (None, path)
        };
        files.push(CommitFile {
            path: String::from_utf8_lossy(path).into_owned(),
            original_path,
            status: change_from_code(code as char),
            stats: LineStats::default(),
        });
    }
    Ok(files)
}

fn parse_status_with_tracking(bytes: &[u8]) -> GitResult<(RepoStatus, BranchInfo)> {
    let invalid = || {
        GitError::new(
            "parse repository status",
            "Git returned an unexpected porcelain v2 status record",
        )
    };
    if !bytes.ends_with(&[0]) {
        return Err(invalid());
    }
    let mut tracking = BranchInfo {
        name: None,
        upstream: None,
        ahead: 0,
        behind: 0,
    };
    let mut saw_branch = false;
    let mut files = Vec::new();
    let mut records = bytes.split(|byte| *byte == 0);
    while let Some(record) = records.next() {
        if record.is_empty() {
            continue;
        }
        if let Some(name) = record.strip_prefix(b"# branch.head ") {
            if name.is_empty() {
                return Err(invalid());
            }
            saw_branch = true;
            tracking.name =
                (name != b"(detached)").then(|| String::from_utf8_lossy(name).into_owned());
            continue;
        }
        if let Some(upstream) = record.strip_prefix(b"# branch.upstream ") {
            tracking.upstream = Some(String::from_utf8_lossy(upstream).into_owned());
            continue;
        }
        if let Some(counts) = record.strip_prefix(b"# branch.ab ") {
            let counts = std::str::from_utf8(counts).map_err(|_| invalid())?;
            let (ahead, behind) = counts.split_once(' ').ok_or_else(invalid)?;
            tracking.ahead = ahead
                .strip_prefix('+')
                .ok_or_else(invalid)?
                .parse()
                .map_err(|_| invalid())?;
            tracking.behind = behind
                .strip_prefix('-')
                .ok_or_else(invalid)?
                .parse()
                .map_err(|_| invalid())?;
            continue;
        }
        // Porcelain v2 allows future optional headers; they do not change paths.
        if record.starts_with(b"# ") || record.starts_with(b"! ") {
            continue;
        }
        if let Some(path) = record.strip_prefix(b"? ") {
            if path.is_empty() {
                return Err(invalid());
            }
            files.push(FileStatus {
                path: String::from_utf8_lossy(path).into_owned(),
                original_path: None,
                index: FileChange::None,
                worktree: FileChange::Untracked,
            });
            continue;
        }
        let field_count = match record[0] {
            b'1' => 9,
            b'2' => 10,
            b'u' => 11,
            _ => return Err(invalid()),
        };
        // Only split the fixed prefix: spaces, tabs and newlines in the path
        // belong to the filename, and rename sources are separate NUL records.
        let fields: Vec<_> = record.splitn(field_count, |byte| *byte == b' ').collect();
        if fields.len() != field_count
            || fields[0].len() != 1
            || fields[1].len() != 2
            || fields[field_count - 1].is_empty()
        {
            return Err(invalid());
        }
        let original_path = if record[0] == b'2' {
            let original = records
                .next()
                .filter(|path| !path.is_empty())
                .ok_or_else(invalid)?;
            Some(String::from_utf8_lossy(original).into_owned())
        } else {
            None
        };
        files.push(FileStatus {
            path: String::from_utf8_lossy(fields[field_count - 1]).into_owned(),
            original_path,
            index: change_from_code(fields[1][0] as char),
            worktree: change_from_code(fields[1][1] as char),
        });
    }
    if !saw_branch {
        return Err(invalid());
    }
    Ok((
        RepoStatus {
            branch: tracking.name.clone(),
            files,
        },
        tracking,
    ))
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
        ' ' | '.' => FileChange::None,
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
    let mut fields = bytes.split(|byte| *byte == 0);
    while let Some(hash) = fields.next() {
        let hash = String::from_utf8_lossy(hash).trim().to_owned();
        if hash.is_empty() {
            continue;
        }
        let (Some(parents), Some(author), Some(date), Some(timestamp), Some(subject)) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        ) else {
            return Err(GitError::new(
                "parse recent history",
                "Git returned an unexpected log record",
            ));
        };
        validate_commit_hash(&hash)?;
        let parents: Vec<_> = String::from_utf8_lossy(parents)
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        for parent in &parents {
            validate_commit_hash(parent)?;
        }
        commits.push(CommitSummary {
            hash,
            parents,
            refs: Vec::new(),
            author: String::from_utf8_lossy(author).into_owned(),
            date: String::from_utf8_lossy(date).into_owned(),
            timestamp: String::from_utf8_lossy(timestamp)
                .parse()
                .map_err(|_| GitError::new("parse recent history", "invalid author timestamp"))?,
            on_remote: false,
            subject: String::from_utf8_lossy(subject).into_owned(),
        });
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
                "vedit-git-tests-{}-{timestamp}-{} with spaces",
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
            repository.git(&["config", "user.name", "vEdit Git Test"]);
            repository.git(&["config", "user.email", "vedit-tests@example.invalid"]);
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

        fn head_hash(&self) -> String {
            String::from_utf8(self.git(&["rev-parse", "--verify", "HEAD"]).stdout)
                .unwrap()
                .trim()
                .to_owned()
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
                    .starts_with("vedit-git-tests-")
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
    fn unchanged_history_snapshot_uses_two_git_processes() {
        let repository = TestRepository::new();
        repository.commit_all("Initial");
        let head = repository.head_hash();
        repository.git(&["update-ref", "refs/remotes/origin/main", &head]);
        let previous = history_snapshot(&repository.0).unwrap();
        GIT_COMMAND_COUNT.with(|count| count.set(0));
        let refreshed = history_snapshot_cached(&repository.0, Some(&previous)).unwrap();
        assert_eq!(refreshed, previous);
        assert!(std::sync::Arc::ptr_eq(
            &refreshed.local_only,
            &previous.local_only
        ));
        assert_eq!(GIT_COMMAND_COUNT.with(|count| count.get()), 2);
    }

    #[test]
    fn history_has_real_merge_parents_local_remote_refs_and_annotated_tags() {
        let repository = TestRepository::new();
        repository.write("base.txt", "base\n");
        repository.commit_all("Base");
        repository.git(&["checkout", "-b", "feature/split-views"]);
        repository.write("feature.txt", "feature\n");
        repository.commit_all("Feature");
        let feature = repository.head_hash();
        repository.git(&["checkout", "main"]);
        repository.write("main.txt", "main\n");
        repository.commit_all("Main");
        repository.git(&["merge", "--no-ff", "--no-edit", "feature/split-views"]);
        let merge = repository.head_hash();
        repository.git(&["update-ref", "refs/remotes/origin/main", &merge]);
        repository.git(&[
            "update-ref",
            "refs/remotes/origin/feature/split-views",
            &feature,
        ]);
        repository.git(&[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ]);
        repository.git(&["tag", "-a", "v0.4.4", "-m", "Release"]);
        let blob =
            String::from_utf8(repository.git(&["rev-parse", "HEAD:base.txt"]).stdout).unwrap();
        repository.git(&["update-ref", "refs/tags/a-blob-tag", blob.trim()]);
        let snapshot = history_snapshot(&repository.0).unwrap();
        assert!(
            snapshot
                .refs
                .iter()
                .any(|reference| reference.current && reference.name == "main")
        );
        assert!(
            snapshot
                .refs
                .iter()
                .any(|reference| reference.kind == RefKind::Remote
                    && reference.name == "origin/main")
        );
        assert!(
            snapshot
                .refs
                .iter()
                .any(|reference| reference.kind == RefKind::Tag
                    && reference.name == "v0.4.4"
                    && reference.hash == merge)
        );
        assert!(
            !snapshot
                .refs
                .iter()
                .any(|reference| reference.name == "origin/HEAD" || reference.name == "a-blob-tag")
        );
        let page = history_page(&repository.0, &snapshot, 0, 20).unwrap();
        assert!(!page.has_more);
        assert_eq!(page.commits.len(), 4);
        assert_eq!(page.commits[0].hash, merge);
        assert_eq!(page.commits[0].parents.len(), 2);
        assert_eq!(page.commits[0].parents[1], feature);
        assert!(
            page.commits[0]
                .refs
                .iter()
                .any(|reference| reference.current)
        );
        let positions: HashMap<_, _> = page
            .commits
            .iter()
            .enumerate()
            .map(|(index, commit)| (&commit.hash, index))
            .collect();
        for (index, commit) in page.commits.iter().enumerate() {
            for parent in &commit.parents {
                assert!(
                    positions[parent] > index,
                    "parents must follow all their children"
                );
            }
        }
        repository.git(&["checkout", "--detach", &feature]);
        let detached = history_snapshot(&repository.0).unwrap();
        assert!(detached.refs.iter().any(|reference| reference.current
            && reference.kind == RefKind::Head
            && reference.hash == feature));
        assert!(
            !detached
                .refs
                .iter()
                .any(|reference| reference.current && reference.kind == RefKind::Local)
        );
    }

    #[test]
    fn history_pages_cover_long_history_and_stay_frozen_when_refs_move() {
        let repository = TestRepository::new();
        // One import produces enough genuine commits to cross several pages
        // without spawning a Git process for each fixture commit.
        let mut input = String::new();
        for mark in 1..=137 {
            let message = format!("History {mark}\n");
            let contents = format!("Revision {mark}\n");
            input.push_str(&format!("commit refs/heads/main\nmark :{mark}\ncommitter vEdit Test <test@example.invalid> {} +0000\ndata {}\n{}",
                1_700_000_000 + mark, message.len(), message));
            if mark > 1 {
                input.push_str(&format!("from :{}\n", mark - 1));
            }
            input.push_str(&format!(
                "M 100644 inline history.txt\ndata {}\n{}\n",
                contents.len(),
                contents
            ));
        }
        input.push_str("done\n");
        let output = raw_git_with_input(
            &repository.0,
            &string_args(&["fast-import", "--quiet", "--done"]),
            "build history fixture",
            Some(input.into_bytes()),
        )
        .unwrap();
        assert!(output.status.success(), "{}", output_text(&output));
        let snapshot = history_snapshot(&repository.0).unwrap();
        let full = history_page(&repository.0, &snapshot, 0, 1000).unwrap();
        assert_eq!(full.commits.len(), 137);
        let first = history_page(&repository.0, &snapshot, 0, HISTORY_PAGE_SIZE).unwrap();
        assert!(first.has_more);
        assert_eq!(first.commits.len(), HISTORY_PAGE_SIZE);
        // A rewound local branch and a new remote branch must not change the
        // traversal or decorations already being browsed.
        repository.git(&["update-ref", "refs/heads/main", &full.commits[10].hash]);
        repository.git(&[
            "update-ref",
            "refs/remotes/origin/review",
            &full.commits[5].hash,
        ]);
        assert_ne!(snapshot, history_snapshot(&repository.0).unwrap());
        let mut commits = first.commits;
        while commits.len() < full.commits.len() {
            let page =
                history_page(&repository.0, &snapshot, commits.len(), HISTORY_PAGE_SIZE).unwrap();
            assert!(!page.commits.is_empty());
            let has_more = page.has_more;
            commits.extend(page.commits);
            assert_eq!(has_more, commits.len() < full.commits.len());
        }
        assert_eq!(commits, full.commits);
        assert!(
            !history_page(&repository.0, &snapshot, commits.len(), HISTORY_PAGE_SIZE)
                .unwrap()
                .has_more
        );
    }

    #[test]
    fn remote_only_unmerged_branch_is_in_history() {
        let repository = TestRepository::new();
        repository.write("base.txt", "base\n");
        repository.commit_all("Base");
        repository.git(&["checkout", "-b", "review"]);
        repository.write("review.txt", "review\n");
        repository.commit_all("Remote review");
        let review = repository.head_hash();
        repository.git(&["checkout", "main"]);
        repository.git(&["update-ref", "refs/remotes/origin/review", &review]);
        repository.git(&["branch", "-D", "review"]);
        let snapshot = history_snapshot(&repository.0).unwrap();
        let page = history_page(&repository.0, &snapshot, 0, 10).unwrap();
        let commit = page
            .commits
            .iter()
            .find(|commit| commit.hash == review)
            .unwrap();
        assert_eq!(commit.subject, "Remote review");
        assert!(commit.refs.iter().any(
            |reference| reference.name == "origin/review" && reference.kind == RefKind::Remote
        ));
        assert_eq!(page.commits.len(), 2);
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
    fn idle_status_and_tracking_use_one_git_process() {
        let repository = TestRepository::new();
        repository.commit_all("Initial");
        repository.git(&["branch", "tracking"]);
        repository.git(&["config", "branch.main.remote", "."]);
        repository.git(&["config", "branch.main.merge", "refs/heads/tracking"]);
        GIT_COMMAND_COUNT.with(|count| count.set(0));
        let (status, tracking) = status_with_tracking(&repository.0).unwrap();
        assert!(status.files.is_empty());
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert_eq!(tracking.upstream.as_deref(), Some("tracking"));
        assert_eq!((tracking.ahead, tracking.behind), (0, 0));
        assert_eq!(GIT_COMMAND_COUNT.with(|count| count.get()), 1);
    }

    #[test]
    fn clean_worktree_statistics_do_not_launch_git() {
        let repository = TestRepository::new();
        repository.commit_all("Initial");
        let files = status(&repository.0).unwrap().files;
        assert!(files.is_empty());
        GIT_COMMAND_COUNT.with(|count| count.set(0));
        assert_eq!(
            working_tree_stats(&repository.0, &files).unwrap(),
            WorkingTreeStats::default()
        );
        assert_eq!(GIT_COMMAND_COUNT.with(|count| count.get()), 0);
    }

    #[test]
    fn combined_status_preserves_changes_and_branch_states() {
        let repository = TestRepository::new();
        let check = || {
            let (combined, tracking) = status_with_tracking(&repository.0).unwrap();
            assert_eq!(combined, status(&repository.0).unwrap());
            assert_eq!(tracking, branch_info(&repository.0).unwrap());
        };
        // An unborn branch and untracked files still have a useful status.
        repository.write("--über old name.txt", "initial\n");
        check();
        stage_all(&repository.0).unwrap();
        check();
        commit(&repository.0, "Initial").unwrap();
        repository.git(&["branch", "tracking"]);
        repository.git(&["config", "branch.main.remote", "."]);
        repository.git(&["config", "branch.main.merge", "refs/heads/tracking"]);
        repository.write("local.txt", "local\n");
        repository.commit_all("Ahead");
        repository.git(&["checkout", "tracking"]);
        repository.write("remote.txt", "remote\n");
        repository.commit_all("Behind");
        repository.git(&["checkout", "main"]);
        check();
        fs::rename(
            repository.0.join("--über old name.txt"),
            repository.0.join("--über renamed file.txt"),
        )
        .unwrap();
        stage_all(&repository.0).unwrap();
        repository.write("--über renamed file.txt", "working change\n");
        repository.write("untracked file.txt", "new\n");
        check();
        repository.commit_all("Rename");
        repository.git(&["checkout", "--detach"]);
        check();
    }

    #[test]
    fn combined_status_preserves_merge_conflicts() {
        let repository = TestRepository::new();
        repository.write("conflict.txt", "base\n");
        repository.commit_all("Base");
        repository.git(&["branch", "other"]);
        repository.write("conflict.txt", "main\n");
        repository.commit_all("Main");
        repository.git(&["checkout", "other"]);
        repository.write("conflict.txt", "other\n");
        repository.commit_all("Other");
        let merge = raw_git(
            &repository.0,
            &string_args(&["merge", "main"]),
            "create merge conflict",
        )
        .unwrap();
        assert!(!merge.status.success());
        let (combined, _) = status_with_tracking(&repository.0).unwrap();
        assert_eq!(combined, status(&repository.0).unwrap());
        assert_eq!(combined.files.len(), 1);
        assert!(combined.files[0].is_conflicted());
    }

    #[test]
    fn worktree_statistics_only_run_comparisons_with_changed_files() {
        let repository = TestRepository::new();
        repository.write("file.txt", "initial\n");
        repository.commit_all("Initial");
        repository.write("untracked.txt", "first\nsecond\n");
        let files = status(&repository.0).unwrap().files;
        GIT_COMMAND_COUNT.with(|count| count.set(0));
        let statistics = working_tree_stats(&repository.0, &files).unwrap();
        assert_eq!(statistics.unstaged["untracked.txt"].added, 2);
        assert_eq!(GIT_COMMAND_COUNT.with(|count| count.get()), 0);

        repository.write("file.txt", "modified\n");
        let files = status(&repository.0).unwrap().files;
        GIT_COMMAND_COUNT.with(|count| count.set(0));
        let statistics = working_tree_stats(&repository.0, &files).unwrap();
        assert_eq!(statistics.unstaged["file.txt"].added, 1);
        assert!(statistics.staged.is_empty());
        assert_eq!(GIT_COMMAND_COUNT.with(|count| count.get()), 1);

        stage_all(&repository.0).unwrap();
        let files = status(&repository.0).unwrap().files;
        GIT_COMMAND_COUNT.with(|count| count.set(0));
        let statistics = working_tree_stats(&repository.0, &files).unwrap();
        assert_eq!(statistics.staged["file.txt"].added, 1);
        assert_eq!(statistics.staged["untracked.txt"].added, 2);
        assert!(statistics.unstaged.is_empty());
        assert_eq!(GIT_COMMAND_COUNT.with(|count| count.get()), 1);
    }

    #[test]
    fn staging_and_unstaging_before_first_commit_preserve_files() {
        let repository = TestRepository::new();
        let file = "--über file.rs";
        repository.write(file, "fn main() {}\n");
        stage_paths(&repository.0, &[PathBuf::from(file)]).unwrap();
        let repository_status = status(&repository.0).unwrap();
        assert_eq!(
            working_tree_stats(&repository.0, &repository_status.files)
                .unwrap()
                .staged[file]
                .added,
            1
        );
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
        repository.git(&["config", "diff.external", "vedit-nonexistent-diff-program"]);
        repository.git(&[
            "config",
            "diff.custom.textconv",
            "vedit-nonexistent-textconv-program",
        ]);
        repository.write("code.txt", "changed\n");
        let patch = diff(&repository.0, Path::new("code.txt"), false).unwrap();
        assert!(patch.contains("+changed\n"));
        let files = status(&repository.0).unwrap().files;
        assert_eq!(
            working_tree_stats(&repository.0, &files).unwrap().unstaged["code.txt"].added,
            1
        );
        repository.commit_all("Changed without conversion helpers");
        let hash = repository.head_hash();
        // A working-tree path with the same spelling must not make Git treat a
        // full immutable revision as an ambiguous path argument.
        repository.write(&hash, "untracked path named after the commit\n");
        let details = commit_details(&repository.0, &hash).unwrap();
        let file = details
            .files
            .iter()
            .find(|file| file.path == "code.txt")
            .unwrap();
        assert_eq!(file.stats.added, 1);
        assert!(
            commit_file_diff(&repository.0, &details, file)
                .unwrap()
                .contains("+changed\n")
        );
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
        assert_eq!(log[0].author, "vEdit Git Test");
        assert_eq!(log[0].hash.len(), 40);
        assert!(log[0].date.contains('T') && log[0].date.len() >= 20);
        assert!(log[0].timestamp > 0);
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
    fn line_statistics_separate_index_worktree_binary_and_untracked_content() {
        let repository = TestRepository::new();
        repository.write("code ü.rs", "one\ntwo\nthree\n");
        fs::write(repository.0.join("asset.bin"), b"original\0asset").unwrap();
        repository.commit_all("Initial");
        repository.write("code ü.rs", "one\nchanged\nthree\n");
        stage_paths(&repository.0, &[PathBuf::from("code ü.rs")]).unwrap();
        repository.write("code ü.rs", "one\nchanged\nthree\nfour\n");
        fs::write(repository.0.join("asset.bin"), b"modified\0asset").unwrap();
        repository.write("untracked ü.txt", "first\nsecond");
        repository.write("empty.txt", "");
        fs::write(repository.0.join("new.bin"), b"new\0asset").unwrap();
        fs::write(
            repository.0.join("large.txt"),
            vec![b'x'; UNTRACKED_FILE_LIMIT + 1],
        )
        .unwrap();
        let index_before = fs::read(repository.0.join(".git/index")).unwrap();
        let files = status(&repository.0).unwrap().files;
        let stats = working_tree_stats(&repository.0, &files).unwrap();
        assert_eq!(
            stats.staged["code ü.rs"],
            LineStats {
                added: 1,
                removed: 1,
                binary: false
            }
        );
        assert_eq!(
            stats.unstaged["code ü.rs"],
            LineStats {
                added: 1,
                removed: 0,
                binary: false
            }
        );
        assert!(stats.unstaged["asset.bin"].binary);
        assert!(stats.unstaged["new.bin"].binary);
        assert_eq!(stats.unstaged["untracked ü.txt"].added, 2);
        assert_eq!(stats.unstaged["empty.txt"], LineStats::default());
        assert!(!stats.unstaged.contains_key("large.txt"));
        assert_eq!(
            fs::read(repository.0.join(".git/index")).unwrap(),
            index_before
        );
        let patch = untracked_diff(&repository.0, Path::new("untracked ü.txt")).unwrap();
        assert!(patch.contains("@@ -0,0 +1,2 @@\n+first\n+second\n\\ No newline at end of file\n"));
        assert!(
            untracked_diff(&repository.0, Path::new("new.bin"))
                .unwrap()
                .contains("Binary files")
        );
        assert!(untracked_diff(&repository.0, Path::new("large.txt")).is_err());
        assert!(untracked_diff(&repository.0, Path::new("../outside.txt")).is_err());
    }

    #[test]
    fn root_commit_details_include_full_metadata_message_files_and_line_counts() {
        let repository = TestRepository::new();
        repository.git(&["config", "user.name", "vEdit Göß Test"]);
        repository.write("root ü.txt", "one\ntwo\n");
        fs::write(repository.0.join("asset.bin"), b"binary\0content").unwrap();
        repository.commit_all(
            "Root subject\n\nA complete multiline body.\nSecond paragraph with ü and tabs\ttoo.",
        );
        let hash = repository.head_hash();
        let details = commit_details(&repository.0, &hash).unwrap();
        assert_eq!(
            commit_stats(&repository.0, &hash).unwrap(),
            CommitStats {
                // The fixture's two-line .gitignore is part of the root commit.
                files_changed: 3,
                lines: LineStats {
                    added: 4,
                    removed: 0,
                    binary: true,
                },
            }
        );
        assert_eq!(details.hash, hash);
        assert!(details.parents.is_empty());
        assert_eq!(details.author, "vEdit Göß Test");
        assert_eq!(details.author_email, "vedit-tests@example.invalid");
        assert!(details.date.contains('T') && details.date.len() >= 20);
        assert!(
            details
                .message
                .starts_with("Root subject\n\nA complete multiline body.\nSecond paragraph")
        );
        assert!(details.message.contains("ü and tabs\ttoo."));
        let root_file = details
            .files
            .iter()
            .find(|file| file.path == "root ü.txt")
            .unwrap();
        assert_eq!(root_file.status, FileChange::Added);
        assert_eq!(
            root_file.stats,
            LineStats {
                added: 2,
                removed: 0,
                binary: false
            }
        );
        assert!(
            details
                .files
                .iter()
                .find(|file| file.path == "asset.bin")
                .unwrap()
                .stats
                .binary
        );
        let patch = commit_diff(&repository.0, &hash, Path::new("root ü.txt")).unwrap();
        assert!(patch.contains("+one\n+two\n") && patch.contains("/dev/null"));
        assert!(commit_diff(&repository.0, &hash, Path::new("not-in-commit.txt")).is_err());
        for invalid in [
            "HEAD",
            "--help",
            "a12345",
            "../../HEAD",
            "fffffffffffffffffffffffffffffffffffffffz",
        ] {
            assert!(commit_details(&repository.0, invalid).is_err());
            assert!(commit_stats(&repository.0, invalid).is_err());
            assert!(commit_diff(&repository.0, invalid, Path::new("root ü.txt")).is_err());
        }
    }

    #[test]
    fn commit_statistics_count_text_binary_deletions_and_empty_commits() {
        let repository = TestRepository::new();
        repository.write("modified ü.txt", "one\ntwo\nthree\n");
        repository.write("deleted.txt", "old\ncontents\n");
        fs::write(repository.0.join("asset.bin"), b"original\0asset").unwrap();
        repository.commit_all("Initial");
        repository.write("modified ü.txt", "one\nchanged\nthree\n");
        fs::remove_file(repository.0.join("deleted.txt")).unwrap();
        repository.write("added.txt", "new\ncontents without a trailing newline");
        fs::write(repository.0.join("asset.bin"), b"modified\0asset").unwrap();
        repository.commit_all("Text additions, removals, and a binary change");
        let hash = repository.head_hash();
        // Working-tree edits must not alter immutable commit hover totals.
        repository.write("modified ü.txt", "uncommitted\n");
        assert_eq!(
            commit_stats(&repository.0, &hash).unwrap(),
            CommitStats {
                files_changed: 4,
                lines: LineStats {
                    added: 3,
                    removed: 3,
                    binary: true,
                },
            }
        );
        let tree = String::from_utf8(repository.git(&["rev-parse", "HEAD^{tree}"]).stdout)
            .unwrap()
            .trim()
            .to_owned();
        assert!(commit_stats(&repository.0, &tree).is_err());
        repository.git(&["commit", "--allow-empty", "-m", "Empty commit"]);
        assert_eq!(
            commit_stats(&repository.0, &repository.head_hash()).unwrap(),
            CommitStats::default()
        );
    }

    #[test]
    fn commit_rename_statistics_and_patch_preserve_both_paths() {
        let repository = TestRepository::new();
        // Status classification and counted comparisons must agree even when
        // the user's default diff settings disable rename detection.
        repository.git(&["config", "diff.renames", "false"]);
        let initial = "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\n";
        repository.write("old ü name.txt", initial);
        repository.commit_all("Initial");
        let parent = repository.head_hash();
        fs::rename(
            repository.0.join("old ü name.txt"),
            repository.0.join("new ü name.txt"),
        )
        .unwrap();
        repository.write("new ü name.txt", &format!("{initial}eleven\n"));
        stage_all(&repository.0).unwrap();
        let stats =
            working_tree_stats(&repository.0, &status(&repository.0).unwrap().files).unwrap();
        assert_eq!(stats.staged["new ü name.txt"].added, 1);
        assert_eq!(stats.staged["new ü name.txt"].removed, 0);
        let entry = status(&repository.0)
            .unwrap()
            .files
            .into_iter()
            .find(|file| file.path == "new ü name.txt")
            .unwrap();
        let staged_patch = status_diff(&repository.0, &entry, true).unwrap();
        assert!(staged_patch.contains("rename from ") && staged_patch.contains("rename to "));
        assert!(staged_patch.contains("+eleven\n"));
        commit(&repository.0, "Rename with one added line").unwrap();
        let hash = repository.head_hash();
        let details = commit_details(&repository.0, &hash).unwrap();
        assert_eq!(details.parents, vec![parent]);
        assert_eq!(
            commit_stats(&repository.0, &hash).unwrap(),
            CommitStats {
                files_changed: 1,
                lines: LineStats {
                    added: 1,
                    removed: 0,
                    binary: false,
                },
            }
        );
        assert_eq!(details.files.len(), 1);
        let file = &details.files[0];
        assert_eq!(file.path, "new ü name.txt");
        assert_eq!(file.original_path.as_deref(), Some("old ü name.txt"));
        assert_eq!(file.status, FileChange::Renamed);
        assert_eq!(file.stats.added, 1);
        let patch = commit_diff(&repository.0, &hash, Path::new("new ü name.txt")).unwrap();
        assert!(patch.contains("rename from ") && patch.contains("rename to "));
        assert!(patch.contains("+eleven\n"));
        assert_eq!(
            commit_file_diff(&repository.0, &details, file).unwrap(),
            patch
        );
    }

    #[test]
    fn copied_file_previews_do_not_include_changes_to_the_source() {
        let repository = TestRepository::new();
        repository.write("source.txt", "original contents\n");
        repository.commit_all("Initial");
        repository.write("source.txt", "changed source only\n");
        repository.write("copy.txt", "original contents\n");
        stage_all(&repository.0).unwrap();
        // Copy detection is configurable in Git. Preserve the status returned
        // by such a configuration without expanding the selected path to the
        // independently modified source file.
        let copied = FileStatus {
            path: "copy.txt".into(),
            original_path: Some("source.txt".into()),
            index: FileChange::Copied,
            worktree: FileChange::None,
        };
        let patch = status_diff(&repository.0, &copied, true).unwrap();
        assert!(patch.contains("b/copy.txt"));
        assert!(!patch.contains("changed source only"));
        assert!(!patch.contains("b/source.txt"));
        commit(&repository.0, "Copy and independently change its source").unwrap();
        let hash = repository.head_hash();
        let mut details = commit_details(&repository.0, &hash).unwrap();
        let copy = details
            .files
            .iter_mut()
            .find(|file| file.path == "copy.txt")
            .unwrap();
        copy.status = FileChange::Copied;
        copy.original_path = Some("source.txt".into());
        let file = details
            .files
            .iter()
            .find(|file| file.path == "copy.txt")
            .unwrap();
        let patch = commit_file_diff(&repository.0, &details, file).unwrap();
        assert!(patch.contains("b/copy.txt"));
        assert!(!patch.contains("changed source only"));
        assert!(!patch.contains("b/source.txt"));
    }

    #[test]
    fn merge_commit_files_and_patch_compare_to_the_first_parent() {
        let repository = TestRepository::new();
        repository.write("common.txt", "common\n");
        repository.commit_all("Initial");
        repository.git(&["checkout", "-b", "feature"]);
        repository.write("feature.txt", "feature branch\n");
        repository.commit_all("Feature");
        repository.git(&["checkout", "main"]);
        repository.write("main.txt", "main branch\n");
        repository.commit_all("Main");
        // The graph includes all branch tips. Sibling tips may share dates;
        // the first row is not an alias for the checked-out branch's HEAD.
        let first_parent = repository.head_hash();
        repository.git(&["merge", "--no-ff", "--no-edit", "feature"]);
        let hash = repository.head_hash();
        let details = commit_details(&repository.0, &hash).unwrap();
        assert_eq!(details.parents.len(), 2);
        assert_eq!(details.parents[0], first_parent);
        assert_eq!(details.files.len(), 1);
        assert_eq!(details.files[0].path, "feature.txt");
        assert_eq!(details.files[0].stats.added, 1);
        assert_eq!(
            commit_stats(&repository.0, &hash).unwrap(),
            CommitStats {
                files_changed: 1,
                lines: LineStats {
                    added: 1,
                    removed: 0,
                    binary: false,
                },
            }
        );
        assert!(
            commit_diff(&repository.0, &hash, Path::new("feature.txt"))
                .unwrap()
                .contains("+feature branch\n")
        );
    }

    #[test]
    fn nul_delimited_statistics_and_commit_paths_keep_tabs_newlines_and_renames() {
        let stats = parse_numstat(b"2\t1\tdir/with\ttab\nline.txt\0-\t-\tasset.bin\x000\t0\t\0old\tname\n.txt\0new\tname\n.txt\0").unwrap();
        assert_eq!(stats[0].0, "dir/with\ttab\nline.txt");
        assert_eq!(
            stats[0].1,
            LineStats {
                added: 2,
                removed: 1,
                binary: false
            }
        );
        assert!(stats[1].1.binary);
        assert_eq!(stats[2], ("new\tname\n.txt".into(), LineStats::default()));
        let files = parse_commit_files(
            b"R100\0old\tname\n.txt\0new\tname\n.txt\0M\0dir/with\ttab\nline.txt\0",
        )
        .unwrap();
        assert_eq!(files[0].original_path.as_deref(), Some("old\tname\n.txt"));
        assert_eq!(files[0].path, "new\tname\n.txt");
        assert_eq!(files[1].path, "dir/with\ttab\nline.txt");
        for invalid in [
            b"1\t2\tpath".as_slice(),
            b"1\t2\t\0old\0",
            b"x\t1\tfile\0",
            b"-\t2\tfile\0",
        ] {
            assert!(parse_numstat(invalid).is_err());
        }
        assert!(parse_commit_files(b"R100\0old\0").is_err());
        assert!(parse_commit_files(b"M\0truncated").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn real_git_statistics_handle_control_characters_in_file_names() {
        let repository = TestRepository::new();
        let file = "tab\tand\nnewline ü.txt";
        repository.write(file, "before\n");
        repository.commit_all("Initial");
        repository.write(file, "after\nextra\n");
        stage_paths(&repository.0, &[PathBuf::from(file)]).unwrap();
        let stats =
            working_tree_stats(&repository.0, &status(&repository.0).unwrap().files).unwrap();
        assert_eq!(
            stats.staged[file],
            LineStats {
                added: 2,
                removed: 1,
                binary: false
            }
        );
        commit(&repository.0, "Control characters").unwrap();
        let hash = repository.head_hash();
        let details = commit_details(&repository.0, &hash).unwrap();
        assert_eq!(details.files[0].path, file);
        assert_eq!(details.files[0].stats, stats.staged[file]);
    }

    #[test]
    fn porcelain_v2_preserves_paths_and_rejects_truncated_records() {
        let (status, tracking) = parse_status_with_tracking(
            b"# branch.oid (initial)\0# branch.head main\0# future.header data\0\
              1 .M N... 100644 100644 100644 a b tab\tand\nnewline.txt\0\
              2 R. N... 100644 100644 100644 a b R100 new name.txt\0old name.txt\0\
              u UU N... 100644 100644 100644 100644 a b c conflict.txt\0\
              ? new file.txt\0",
        )
        .unwrap();
        assert_eq!(tracking.name.as_deref(), Some("main"));
        assert_eq!(status.files.len(), 4);
        assert_eq!(status.files[0].path, "tab\tand\nnewline.txt");
        assert_eq!(status.files[0].index, FileChange::None);
        assert_eq!(status.files[0].worktree, FileChange::Modified);
        assert_eq!(status.files[1].path, "new name.txt");
        assert_eq!(
            status.files[1].original_path.as_deref(),
            Some("old name.txt")
        );
        assert!(status.files[2].is_conflicted());
        assert!(status.files[3].is_untracked());
        for invalid in [
            b"# branch.head main".as_slice(),
            b"# branch.head main\0# branch.ab 1 -2\0",
            b"# branch.head main\0# branch.ab +1 -x\0",
            b"# branch.head main\0? \0",
            b"# branch.head main\x001 .M missing fields\0",
            b"# branch.head main\x002 R. N... 100644 100644 100644 a b R100 new.txt\0",
        ] {
            assert!(parse_status_with_tracking(invalid).is_err());
        }
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
    #[test]
    fn remote_reachability_is_frozen_and_includes_ancestors_and_local_merges() {
        let repo = TestRepository::new();
        repo.write("file.txt", "base");
        repo.commit_all("base");
        let base = repo.head_hash();
        repo.git(&["update-ref", "refs/remotes/origin/main", &base]);
        repo.git(&["checkout", "-b", "topic"]);
        repo.write("topic.txt", "topic");
        repo.commit_all("topic");
        let topic = repo.head_hash();
        repo.git(&["checkout", "main"]);
        repo.write("file.txt", "local");
        repo.commit_all("local");
        repo.git(&["merge", "--no-ff", "--no-edit", "topic"]);
        let merge = repo.head_hash();
        let snapshot = history_snapshot(&repo.0).unwrap();
        repo.git(&["update-ref", "refs/remotes/origin/main", &merge]);
        let page = history_page(&repo.0, &snapshot, 0, 50).unwrap();
        assert!(
            page.commits
                .iter()
                .find(|c| c.hash == base)
                .unwrap()
                .on_remote
        );
        assert!(
            !page
                .commits
                .iter()
                .find(|c| c.hash == topic)
                .unwrap()
                .on_remote
        );
        assert!(
            !page
                .commits
                .iter()
                .find(|c| c.hash == merge)
                .unwrap()
                .on_remote
        );
        let refreshed = history_snapshot(&repo.0).unwrap();
        assert!(
            history_page(&repo.0, &refreshed, 0, 50)
                .unwrap()
                .commits
                .iter()
                .all(|c| c.on_remote)
        );
        repo.git(&["update-ref", "-d", "refs/remotes/origin/main"]);
        assert!(
            recent_log(&repo.0, 50)
                .unwrap()
                .iter()
                .all(|c| !c.on_remote)
        );
    }

    #[test]
    fn named_branch_switches_preserve_conflicting_files_and_track_remote_branches() {
        let repo = TestRepository::new();
        repo.write("file.txt", "base");
        repo.commit_all("base");
        repo.git(&["checkout", "-b", "topic"]);
        repo.write("file.txt", "topic");
        repo.commit_all("topic");
        assert_eq!(switch_branch(&repo.0, "refs/heads/main").unwrap(), "main");
        repo.write("file.txt", "unsaved on disk");
        assert!(switch_branch(&repo.0, "refs/heads/topic").is_err());
        assert_eq!(
            fs::read_to_string(repo.0.join("file.txt")).unwrap(),
            "unsaved on disk"
        );
        assert_eq!(current_branch(&repo.0).unwrap().as_deref(), Some("main"));
        assert!(switch_branch(&repo.0, "--discard-changes").is_err());
        assert!(switch_branch(&repo.0, "refs/heads/missing").is_err());
        repo.git(&["restore", "file.txt"]);
        repo.git(&["remote", "add", "origin", "."]);
        repo.git(&[
            "update-ref",
            "refs/remotes/origin/review",
            &repo.head_hash(),
        ]);
        assert_eq!(
            switch_branch(&repo.0, "refs/remotes/origin/review").unwrap(),
            "review"
        );
        assert_eq!(
            branch_info(&repo.0).unwrap().upstream.as_deref(),
            Some("origin/review")
        );
    }
}

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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

/// Read-only queries are stopped with their process tree after this limit.
const QUERY_TIMEOUT: Duration = Duration::from_secs(15);
/// User-initiated changes can run hooks, LFS filters or large checkouts. They
/// are stopped only by the user, by this safety limit or when vEdit exits.
const MUTATION_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// After Git exits, a descendant that still holds its output pipes is stopped
/// once this grace period ends. Helpers that detached from the pipes survive.
const EXIT_PIPE_GRACE: Duration = Duration::from_secs(2);
const OUTPUT_LIMIT: usize = 8 * 1024 * 1024;
/// A status of a working tree with very many unignored files.
const STATUS_OUTPUT_LIMIT: usize = 64 * 1024 * 1024;
const UNTRACKED_FILE_LIMIT: usize = 1024 * 1024;
const UNTRACKED_TOTAL_LIMIT: usize = 8 * 1024 * 1024;
const UNTRACKED_PATH_LIMIT: usize = 128;

#[cfg(test)]
thread_local! {
    static GIT_COMMAND_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static TIMEOUTS: std::cell::Cell<Option<(Duration, Duration)>> =
        const { std::cell::Cell::new(None) };
    static STATUS_LIMIT: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

thread_local! {
    static CANCELLATION: std::cell::RefCell<Option<Cancellation>> =
        const { std::cell::RefCell::new(None) };
}

/// Replace the query and mutation limits of Git processes started on this
/// thread, so tests need not wait for the real limits.
#[cfg(test)]
pub(crate) fn set_test_timeouts(query: Duration, mutation: Duration) {
    TIMEOUTS.with(|timeouts| timeouts.set(Some((query, mutation))));
}

fn timeout(access: Access) -> Duration {
    #[cfg(test)]
    if let Some((query, mutation)) = TIMEOUTS.with(std::cell::Cell::get) {
        return match access {
            Access::Query => query,
            Access::Mutation => mutation,
        };
    }
    match access {
        Access::Query => QUERY_TIMEOUT,
        Access::Mutation => MUTATION_TIMEOUT,
    }
}

fn status_output_limit() -> usize {
    #[cfg(test)]
    if let Some(limit) = STATUS_LIMIT.with(std::cell::Cell::get) {
        return limit;
    }
    STATUS_OUTPUT_LIMIT
}

/// Whether a Git process only reads the repository or carries out a change the
/// user asked for. Interrupting a change can leave a partly updated working
/// tree and a stale `index.lock`, so changes get no short time limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Access {
    Query,
    Mutation,
}

/// Lets the user stop a running Git action, together with its hooks and helpers.
#[derive(Clone, Debug, Default)]
pub struct Cancellation(Arc<AtomicBool>);

impl Cancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Run `work` so that every Git process it starts on this thread stops when
/// `cancellation` is triggered.
pub fn cancellable<T>(cancellation: &Cancellation, work: impl FnOnce() -> T) -> T {
    struct Restore(Option<Cancellation>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let previous = self.0.take();
            CANCELLATION.with(|current| *current.borrow_mut() = previous);
        }
    }
    let previous = CANCELLATION.with(|current| current.replace(Some(cancellation.clone())));
    let _restore = Restore(previous);
    work()
}

fn current_cancellation() -> Option<Cancellation> {
    CANCELLATION.with(|current| current.borrow().clone())
}

/// Git processes started on this thread since the last call.
#[cfg(test)]
pub(crate) fn take_command_count() -> usize {
    GIT_COMMAND_COUNT.with(|count| count.replace(0))
}

pub type GitResult<T> = Result<T, GitError>;

/// A readable error returned by a Git operation.
#[derive(Debug, Clone)]
pub struct GitError {
    operation: String,
    details: String,
    output_too_large: bool,
}

impl GitError {
    fn new(operation: impl Into<String>, details: impl Into<String>) -> Self {
        Self {
            operation: operation.into(),
            details: details.into(),
            output_too_large: false,
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
    /// Git listed untracked folders instead of every untracked file because
    /// there were too many of them.
    pub untracked_collapsed: bool,
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
    /// Full ref name of the checked-out branch's configured upstream.
    pub upstream: Option<String>,
    /// Commits unreachable from any frozen remote ref; `None` means no remote
    /// refs existed. Retained independently of visible ref decorations.
    pub local_only: Option<std::sync::Arc<std::collections::HashSet<String>>>,
}

impl HistorySnapshot {
    /// Keep the current branch and its upstream as traversal roots. Tags can
    /// decorate reachable commits, but never pull unrelated history into view.
    pub fn current_branch(&self) -> Self {
        let selected = |reference: &CommitRef| {
            reference.current
                || self.upstream.as_deref().is_some_and(|upstream| {
                    let prefix = match reference.kind {
                        RefKind::Local => "refs/heads/",
                        RefKind::Remote => "refs/remotes/",
                        _ => return false,
                    };
                    upstream.strip_prefix(prefix) == Some(reference.name.as_str())
                })
        };
        let mut tips: Vec<_> = self
            .refs
            .iter()
            .filter(|r| selected(r))
            .map(|r| r.hash.clone())
            .collect();
        tips.sort();
        tips.dedup();
        Self {
            tips,
            refs: self
                .refs
                .iter()
                .filter(|r| selected(r) || r.kind == RefKind::Tag)
                .cloned()
                .collect(),
            upstream: self.upstream.clone(),
            local_only: self.local_only.clone(),
        }
    }
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
    /// False when the configured upstream has no fetched commit to compare.
    pub divergence_known: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchComparison {
    pub base: String,
    pub ahead: usize,
    pub behind: usize,
}

mod changes;
mod command;
mod history;
mod repository;
mod review;

pub use changes::*;
use command::*;
pub use history::*;
pub use repository::*;
pub use review::*;

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

#[cfg(test)]
mod tests;

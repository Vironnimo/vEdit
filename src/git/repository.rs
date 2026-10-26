//! Repository discovery, working-tree status, and staging operations.

use super::*;

/// Initialize a Git repository in `root`.
pub fn initialize(root: &Path) -> GitResult<()> {
    let args = string_args(&["init"]);
    mutate_git(root, &args, "initialize repository")?;
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

/// The enclosing repository is only active when it is the opened folder itself
/// or the user has selected this particular parent repository for the project.
pub struct ProjectRepository {
    pub root: Option<PathBuf>,
    pub parent: Option<PathBuf>,
}

pub fn project_repository(
    project: &Path,
    approved_parent: Option<&Path>,
) -> GitResult<ProjectRepository> {
    let Some(repository) = repository_root(project)? else {
        return Ok(ProjectRepository {
            root: None,
            parent: None,
        });
    };
    let canonical = |path: &Path| {
        std::fs::canonicalize(path)
            .map_err(|error| GitError::new("repository detection", error.to_string()))
    };
    let repository_path = canonical(&repository)?;
    // Git reports `C:/...`, while project roots and their documents are usually
    // canonical (`\\?\C:\...`). Spell the repository like the project, so
    // paths under the project relate to it by prefix.
    let depth = canonical(project)?
        .strip_prefix(&repository_path)
        .map(|inside| inside.components().count())
        .ok();
    let repository = depth
        .and_then(|depth| (0..depth).try_fold(project, |path, _| path.parent()))
        .map_or(repository, Path::to_path_buf);
    if depth == Some(0) {
        return Ok(ProjectRepository {
            root: Some(repository),
            parent: None,
        });
    }
    let approved = approved_parent
        .and_then(|path| std::fs::canonicalize(path).ok())
        .is_some_and(|path| path == repository_path);
    Ok(ProjectRepository {
        root: approved.then(|| repository.clone()),
        parent: Some(repository),
    })
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
        divergence_known: false,
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
    info.divergence_known = true;
    Ok(info)
}

/// Reads the current branch and concise staged, unstaged, and untracked file
/// status. Renames retain both the new path and Git's reported previous path.
#[cfg(test)]
pub fn status(root: &Path) -> GitResult<RepoStatus> {
    let files = status_files(root)?;
    let branch = current_branch(root)?;
    Ok(RepoStatus {
        branch,
        files,
        untracked_collapsed: false,
    })
}

fn status_with_tracking_args(untracked: &str) -> Vec<OsString> {
    string_args(&[
        "status",
        "--porcelain=v2",
        "--branch",
        "--ahead-behind",
        "-z",
        untracked,
        "--renames",
    ])
}

/// Porcelain v2 status listing every untracked file. Above the output limit,
/// for example with many unignored build outputs, untracked folders are
/// listed as a whole instead; the flag reports that fallback.
fn read_status_output(root: &Path, operation: &str) -> GitResult<(Vec<u8>, bool)> {
    let run = |untracked: &str| {
        run_git(
            root,
            &status_with_tracking_args(untracked),
            operation,
            None,
            Access::Query,
            status_output_limit(),
        )
        .and_then(|output| {
            if output.status.success() {
                Ok(output.stdout)
            } else {
                Err(command_error(operation, &output))
            }
        })
    };
    match run("--untracked-files=all") {
        Ok(stdout) => Ok((stdout, false)),
        Err(error) if error.output_too_large => Ok((run("--untracked-files=normal")?, true)),
        Err(error) => Err(error),
    }
}

/// Read file status and local branch tracking in the same Git process.
#[cfg(test)]
pub fn status_with_tracking(root: &Path) -> GitResult<(RepoStatus, BranchInfo)> {
    let (status, tracking, _) = status_with_head(root)?;
    Ok((status, tracking))
}

/// The commit HEAD named when the status was read.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum StatusHead {
    Commit(String),
    /// The current branch has no commits yet.
    Unborn,
}

/// [`status_with_tracking`] plus the HEAD commit reported by the same process.
/// The head is `None` when Git did not report a valid one.
pub fn status_with_head(root: &Path) -> GitResult<(RepoStatus, BranchInfo, Option<StatusHead>)> {
    let (stdout, collapsed) = read_status_output(root, "read repository status")?;
    let (mut status, tracking, head) = parse_status_with_tracking(&stdout)?;
    status.untracked_collapsed = collapsed;
    Ok((status, tracking, head))
}

/// Branch, upstream divergence and change totals of one working tree. Reading
/// it starts a single Git process and never contacts a remote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeStatus {
    /// Full HEAD object ID; `None` on a branch without commits.
    pub head: Option<String>,
    /// `name` is `None` while HEAD is detached.
    pub tracking: BranchInfo,
    /// Changed paths, counted like the Source control summary.
    pub changes: usize,
    pub conflicts: usize,
}

/// The same porcelain v2 status as [`status_with_tracking`], reduced to the
/// totals an overview needs. File records are counted, not retained.
pub fn worktree_status(root: &Path) -> GitResult<WorktreeStatus> {
    let (stdout, _) = read_status_output(root, "read worktree status")?;
    let (status, tracking, _) = parse_status_with_tracking(&stdout)?;
    Ok(WorktreeStatus {
        head: status_branch_oid(&stdout),
        changes: status.files.len(),
        conflicts: status
            .files
            .iter()
            .filter(|file| file.is_conflicted())
            .count(),
        tracking,
    })
}

/// `# branch.oid` of a porcelain v2 status; `(initial)` means no commit yet.
pub(super) fn status_branch_oid(bytes: &[u8]) -> Option<String> {
    bytes
        .split(|byte| *byte == 0)
        .find_map(|record| record.strip_prefix(b"# branch.oid "))
        .map(|oid| String::from_utf8_lossy(oid).into_owned())
        .filter(|oid| validate_commit_hash(oid).is_ok())
}

/// One working tree reported by `git worktree list --porcelain`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeEntry {
    pub path: PathBuf,
    /// Full HEAD object ID; `None` for a bare repository or a branch without commits.
    pub head: Option<String>,
    /// Checked-out local branch without `refs/heads/`.
    pub branch: Option<String>,
    pub bare: bool,
    pub detached: bool,
    /// Present when the worktree is locked; the reason may be empty.
    pub locked: Option<String>,
    /// Present when Git could prune the worktree, usually because it is missing.
    pub prunable: Option<String>,
}

/// List the main working tree (or bare repository) first, then its linked
/// worktrees. One Git process covers the whole repository.
pub fn worktree_list(root: &Path) -> GitResult<Vec<WorktreeEntry>> {
    const OPERATION: &str = "list worktrees";
    let output = raw_git(
        root,
        &string_args(&["worktree", "list", "--porcelain", "-z"]),
        OPERATION,
    )?;
    if output.status.success() {
        return parse_worktree_list(&output.stdout, 0);
    }
    // `-z` exists since Git 2.36. Older versions print one record per line,
    // which only differs for paths that contain a line break.
    if String::from_utf8_lossy(&output.stderr).contains("unknown switch") {
        let output = checked_git(
            root,
            &string_args(&["worktree", "list", "--porcelain"]),
            OPERATION,
        )?;
        return parse_worktree_list(&output.stdout, b'\n');
    }
    Err(command_error(OPERATION, &output))
}

/// Records are separated by `terminator`; an empty record ends an entry.
/// Unknown attributes are ignored so newer Git versions remain readable.
pub(super) fn parse_worktree_list(bytes: &[u8], terminator: u8) -> GitResult<Vec<WorktreeEntry>> {
    let invalid = |details: &str| GitError::new("parse worktree list", details.to_owned());
    let mut entries: Vec<WorktreeEntry> = Vec::new();
    let mut open = false;
    for record in bytes.split(|byte| *byte == terminator) {
        // Line-terminated output from Windows Git may carry a carriage return.
        let record = if terminator == b'\n' {
            record.strip_suffix(b"\r").unwrap_or(record)
        } else {
            record
        };
        if record.is_empty() {
            open = false;
            continue;
        }
        let (name, value) = match record.iter().position(|byte| *byte == b' ') {
            Some(space) => (&record[..space], Some(&record[space + 1..])),
            None => (record, None),
        };
        let text =
            |value: Option<&[u8]>| String::from_utf8_lossy(value.unwrap_or_default()).into_owned();
        if name == b"worktree" {
            let path = text(value);
            if path.is_empty() {
                return Err(invalid("a worktree has no path"));
            }
            entries.push(WorktreeEntry {
                path: PathBuf::from(path),
                head: None,
                branch: None,
                bare: false,
                detached: false,
                locked: None,
                prunable: None,
            });
            open = true;
            continue;
        }
        let Some(entry) = entries.last_mut().filter(|_| open) else {
            return Err(invalid("an attribute precedes its worktree"));
        };
        match name {
            b"HEAD" => {
                let head = text(value);
                validate_commit_hash(&head)?;
                // An unborn branch reports the null object ID.
                entry.head = head.bytes().any(|byte| byte != b'0').then_some(head);
            }
            b"branch" => {
                let reference = text(value);
                entry.branch = Some(
                    reference
                        .strip_prefix("refs/heads/")
                        .unwrap_or(&reference)
                        .to_owned(),
                );
            }
            b"bare" => entry.bare = true,
            b"detached" => entry.detached = true,
            b"locked" => entry.locked = Some(text(value)),
            b"prunable" => entry.prunable = Some(text(value)),
            _ => {}
        }
    }
    Ok(entries)
}

/// The subject and author of a commit for one-line overviews.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitHeadline {
    pub hash: String,
    pub subject: String,
    pub author: String,
    /// Author time as Unix seconds.
    pub timestamp: i64,
}

/// Read the headlines of several commits with one Git process. Object IDs
/// that do not exist in this repository are skipped instead of failing.
pub fn commit_headlines(root: &Path, hashes: &[String]) -> GitResult<Vec<CommitHeadline>> {
    const OPERATION: &str = "read latest commits";
    if hashes.is_empty() {
        return Ok(Vec::new());
    }
    for hash in hashes {
        validate_commit_hash(hash)?;
    }
    let input = hashes.join("\n") + "\n";
    let output = raw_git_with_input(
        root,
        &string_args(&[
            "log",
            "--no-walk=unsorted",
            "--ignore-missing",
            "--no-show-signature",
            "--format=%H%x00%at%x00%an%x00%s%x00",
            "--stdin",
        ]),
        OPERATION,
        Some(input.into_bytes()),
    )?;
    if !output.status.success() {
        return Err(command_error(OPERATION, &output));
    }
    parse_headlines(&output.stdout)
}

pub(super) fn parse_headlines(bytes: &[u8]) -> GitResult<Vec<CommitHeadline>> {
    let invalid = || GitError::new("parse latest commits", "Git returned an unexpected record");
    let mut headlines = Vec::new();
    // `%s` and `%an` are single lines, so each record is one line ending in
    // NUL. Requiring that final empty field rejects a truncated record.
    for record in bytes.split(|byte| *byte == b'\n') {
        let record = record.strip_suffix(b"\r").unwrap_or(record);
        if record.is_empty() {
            continue;
        }
        let fields: Vec<&[u8]> = record.split(|byte| *byte == 0).collect();
        let [hash, timestamp, author, subject, end] = fields.as_slice() else {
            return Err(invalid());
        };
        if !end.is_empty() {
            return Err(invalid());
        }
        let hash = String::from_utf8_lossy(hash).into_owned();
        validate_commit_hash(&hash)?;
        headlines.push(CommitHeadline {
            hash,
            timestamp: String::from_utf8_lossy(timestamp)
                .parse()
                .map_err(|_| invalid())?,
            author: String::from_utf8_lossy(author).into_owned(),
            subject: String::from_utf8_lossy(subject).into_owned(),
        });
    }
    Ok(headlines)
}

#[cfg(test)]
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
    mutate_git(root, &args, "stage files")?;
    Ok(())
}

/// Stage all additions, modifications, and deletions in the working tree.
pub fn stage_all(root: &Path) -> GitResult<()> {
    let args = string_args(&["add", "-A"]);
    mutate_git(root, &args, "stage all files")?;
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
    let output = mutate_git(root, &args, "commit")?;
    Ok(output_text(&output))
}

/// Push using Git's configured destination, establishing tracking on a default
/// push when the current branch has no upstream yet.
pub fn push(root: &Path) -> GitResult<String> {
    run_and_capture_change(root, &["-c", "push.autoSetupRemote=true", "push"], "push")
}

/// Fetch from the configured remotes.
pub fn fetch(root: &Path) -> GitResult<String> {
    run_and_capture_change(root, &["fetch"], "fetch")
}

/// Pull the configured upstream into the current branch.
pub fn pull(root: &Path) -> GitResult<String> {
    run_and_capture_change(root, &["pull", "--ff-only"], "pull")
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
    mutate_git(root, &args, "unstage files")?;
    Ok(())
}

fn has_head(root: &Path) -> GitResult<bool> {
    Ok(read_head(root)?.is_some())
}

pub(super) fn read_head(root: &Path) -> GitResult<Option<String>> {
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

pub(super) fn parse_status_with_tracking(
    bytes: &[u8],
) -> GitResult<(RepoStatus, BranchInfo, Option<StatusHead>)> {
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
        divergence_known: false,
    };
    let mut saw_branch = false;
    let mut head = None;
    let mut files = Vec::new();
    let mut records = bytes.split(|byte| *byte == 0);
    while let Some(record) = records.next() {
        if record.is_empty() {
            continue;
        }
        if let Some(oid) = record.strip_prefix(b"# branch.oid ") {
            // An unexpected value only costs the caller a separate HEAD read.
            head = if oid == b"(initial)" {
                Some(StatusHead::Unborn)
            } else {
                std::str::from_utf8(oid)
                    .ok()
                    .filter(|hash| validate_commit_hash(hash).is_ok())
                    .map(|hash| StatusHead::Commit(hash.to_owned()))
            };
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
            tracking.divergence_known = true;
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
            untracked_collapsed: false,
        },
        tracking,
        head,
    ))
}

#[cfg(test)]
pub(super) fn parse_status(bytes: &[u8]) -> GitResult<Vec<FileStatus>> {
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

pub(super) fn change_from_code(code: char) -> FileChange {
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

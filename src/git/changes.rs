//! File diffs and line statistics for working trees and commits.

use super::*;

/// Read a plain-text patch for a repository-relative path. External diff
/// programs and text-conversion commands are disabled so previewing a change
/// cannot open another application or run a repository-defined program.
pub fn diff(root: &Path, path: &Path, staged: bool) -> GitResult<String> {
    tracked_diff(root, path, None, staged)
}

/// Preview a status entry with both rename paths, or an untracked addition.
pub fn status_diff(root: &Path, file: &FileStatus, staged: bool) -> GitResult<String> {
    let staged = staged && !file.is_conflicted();
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
    } else {
        // Unmerged index entries otherwise produce combined hunks. Compare
        // their working text with our side, using the ordinary two-way format.
        args.push(OsString::from("--ours"));
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
    Ok(untracked_patch(path, &bytes))
}

/// The synthetic addition patch for already read untracked file contents.
pub(super) fn untracked_patch(path: &Path, bytes: &[u8]) -> String {
    let stats = added_file_stats(bytes);
    let display_path = path.to_string_lossy().replace('\\', "/");
    let before = format!("a/{display_path}");
    let after = format!("b/{display_path}");
    let mut patch = format!("diff --git {before:?} {after:?}\nnew file mode 100644\n");
    if stats.binary {
        patch.push_str(&format!("Binary files /dev/null and {after:?} differ\n"));
        return patch;
    }
    patch.push_str(&format!("--- /dev/null\n+++ {after:?}\n"));
    if !bytes.is_empty() {
        patch.push_str(&format!("@@ -0,0 +1,{} @@\n", stats.added));
        let text = String::from_utf8_lossy(bytes);
        for line in text.split_inclusive('\n') {
            patch.push('+');
            patch.push_str(line);
        }
        if !bytes.ends_with(b"\n") {
            patch.push_str("\n\\ No newline at end of file\n");
        }
    }
    patch
}

pub(super) fn validate_commit_hash(hash: &str) -> GitResult<()> {
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

pub(super) fn read_untracked_bytes(root: &Path, path: &Path, limit: usize) -> GitResult<Vec<u8>> {
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

pub(super) fn added_file_stats(bytes: &[u8]) -> LineStats {
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

/// `--numstat -z` emits a normal path in the third field, but a rename has an
/// empty third field followed by two distinct NUL-delimited paths. Splitting
/// the whole output on tabs or lines would corrupt valid file names.
pub(super) fn parse_numstat(bytes: &[u8]) -> GitResult<Vec<(String, LineStats)>> {
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

pub(super) fn parse_commit_files(bytes: &[u8]) -> GitResult<Vec<CommitFile>> {
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

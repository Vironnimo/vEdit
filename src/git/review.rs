//! Review comparisons, bounded patches, and gutter baselines.

use super::*;

/// A file whose added plus removed lines exceed this loads only on request.
pub const REVIEW_LARGE_FILE_LINES: u64 = 3_000;
/// Changed lines whose patches load together with the file list. Later files
/// keep their statistics and load when they are scrolled into view.
const REVIEW_EAGER_LINES: u64 = 40_000;
/// Untracked files listed by one working-tree review; the rest are counted.
const REVIEW_UNTRACKED_LIMIT: usize = 2_000;
/// One patch command covers at most this many paths and path bytes, which
/// keeps the command line far below the Windows limit.
const REVIEW_CHUNK_FILES: usize = 48;
const REVIEW_CHUNK_PATH_BYTES: usize = 12 * 1024;
const REVIEW_REVISION_BYTES: usize = 256;

/// What a change review compares. New sources (for example "since last seen")
/// can be added as variants; consumers only rely on the resolved comparison.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ReviewSource {
    /// Staged, unstaged and untracked changes compared with HEAD.
    WorkingTree,
    /// One commit compared with its first parent; a root commit with the empty tree.
    Commit(String),
    /// Commits on HEAD since it left `base` (`base...HEAD`). `None` uses the
    /// local main branch, or master when there is no main.
    Branch { base: Option<String> },
}

impl ReviewSource {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::WorkingTree => "working",
            Self::Commit(_) => "commit",
            Self::Branch { .. } => "branch",
        }
    }
}

/// A resolved review. Both sides are immutable object IDs (or the working
/// tree), so a patch loaded later still belongs to the listed files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewComparison {
    pub source: ReviewSource,
    /// Scope of the seen state. It stays the same across refreshes, also when
    /// the branch gained commits; per-file content keys detect changed files.
    pub key: String,
    pub title: String,
    pub detail: String,
    /// Tree-ish compared from: a commit, a merge base or the empty tree.
    pub old: String,
    /// Commit compared to; `None` compares with the working tree.
    pub new: Option<String>,
}

impl ReviewComparison {
    fn range_args(&self) -> Vec<OsString> {
        let mut args = vec![OsString::from(&self.old)];
        args.extend(self.new.iter().map(OsString::from));
        args
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewPatch {
    Loaded(String),
    /// Above [`REVIEW_LARGE_FILE_LINES`]; loads only when the user asks.
    Large,
    /// Beyond the eager budget; loads when the file is shown.
    Deferred,
    Binary,
    Unavailable(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewFile {
    pub path: String,
    pub original_path: Option<String>,
    pub status: FileChange,
    /// `None` when Git reported no statistics or they were not computed.
    pub stats: Option<LineStats>,
    /// Changes whenever the compared content of this file changes: blob IDs
    /// for committed content, size and modification time for working files.
    pub content_key: u64,
    pub patch: ReviewPatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeSet {
    pub comparison: ReviewComparison,
    /// Grouped by folder: sorted by parent folder, then by file name.
    pub files: Vec<ReviewFile>,
    /// Untracked files beyond the listing limit.
    pub omitted_untracked: usize,
}

/// Reject option-like, oversized or control-character revision expressions
/// before they reach Git. Revisions are always passed as separate arguments.
pub fn validate_review_revision(revision: &str) -> GitResult<()> {
    if revision.is_empty()
        || revision.len() > REVIEW_REVISION_BYTES
        || revision.starts_with('-')
        || revision.chars().any(char::is_control)
    {
        return Err(GitError::new(
            "resolve review",
            "a revision must have 1 to 256 bytes, must not start with '-' and must not contain control characters",
        ));
    }
    Ok(())
}

fn resolve_commit(root: &Path, revision: &str, operation: &str) -> GitResult<String> {
    validate_review_revision(revision)?;
    let output = raw_git(
        root,
        &[
            OsString::from("rev-parse"),
            OsString::from("--verify"),
            OsString::from("--quiet"),
            OsString::from(format!("{revision}^{{commit}}")),
        ],
        operation,
    )?;
    if !output.status.success() {
        return Err(GitError::new(
            operation,
            format!("{revision} does not name a commit in this repository"),
        ));
    }
    let hash = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    validate_commit_hash(&hash)?;
    Ok(hash)
}

/// The empty tree of this repository's object format (SHA-1 or SHA-256).
fn empty_tree(root: &Path) -> GitResult<String> {
    let output = raw_git_with_input(
        root,
        &string_args(&["hash-object", "-t", "tree", "--stdin"]),
        "read empty tree",
        Some(Vec::new()),
    )?;
    if !output.status.success() {
        return Err(command_error("read empty tree", &output));
    }
    let hash = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    validate_commit_hash(&hash)?;
    Ok(hash)
}

/// Resolve a review source to immutable object IDs without reading patches.
pub fn resolve_review(root: &Path, source: &ReviewSource) -> GitResult<ReviewComparison> {
    const OPERATION: &str = "resolve review";
    match source {
        ReviewSource::WorkingTree => Ok(ReviewComparison {
            source: source.clone(),
            key: "working".into(),
            title: "Working tree".into(),
            detail: "Staged, unstaged and untracked changes compared with HEAD".into(),
            old: match read_head(root)? {
                Some(head) => head,
                None => empty_tree(root)?,
            },
            new: None,
        }),
        ReviewSource::Commit(revision) => {
            let hash = resolve_commit(root, revision, OPERATION)?;
            let output = checked_git(
                root,
                &string_args(&[
                    "show",
                    "--no-patch",
                    "--no-notes",
                    "--format=%P%x00%s",
                    &hash,
                    "--",
                ]),
                OPERATION,
            )?;
            let text = String::from_utf8_lossy(&output.stdout);
            let (parents, subject) = text
                .split_once('\0')
                .ok_or_else(|| GitError::new(OPERATION, "Git returned unexpected commit data"))?;
            let parents: Vec<_> = parents.split_whitespace().collect();
            for parent in &parents {
                validate_commit_hash(parent)?;
            }
            let short = &hash[..7];
            Ok(ReviewComparison {
                source: source.clone(),
                key: format!("commit:{hash}"),
                title: format!("{short} {}", subject.trim_end_matches(['\r', '\n'])),
                detail: match parents.len() {
                    0 => format!("Root commit {hash}, compared with an empty tree"),
                    1 => format!("Commit {hash}, compared with its parent"),
                    _ => format!("Merge commit {hash}, compared with its first parent"),
                },
                old: match parents.first() {
                    Some(parent) => (*parent).to_owned(),
                    None => empty_tree(root)?,
                },
                new: Some(hash),
            })
        }
        ReviewSource::Branch { base } => {
            let head = read_head(root)?
                .ok_or_else(|| GitError::new(OPERATION, "the repository has no commits yet"))?;
            let (base_name, base_hash) = match base {
                Some(base) => (base.clone(), resolve_commit(root, base, OPERATION)?),
                None => {
                    let name = local_main_branch(root)?.ok_or_else(|| {
                        GitError::new(OPERATION, "there is no local main or master branch")
                    })?;
                    let hash = resolve_commit(root, &format!("refs/heads/{name}"), OPERATION)?;
                    (name.to_owned(), hash)
                }
            };
            let branch = current_branch(root)?;
            let name = branch.as_deref().unwrap_or("HEAD");
            let output = raw_git(
                root,
                &string_args(&["merge-base", &head, &base_hash]),
                OPERATION,
            )?;
            if !output.status.success() {
                return Err(if output.status.code() == Some(1) {
                    GitError::new(
                        OPERATION,
                        format!("{name} and {base_name} have no common history"),
                    )
                } else {
                    command_error(OPERATION, &output)
                });
            }
            let merge_base = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            validate_commit_hash(&merge_base)?;
            Ok(ReviewComparison {
                source: source.clone(),
                key: format!("branch:{name}\0{base_name}"),
                title: format!("{name} vs {base_name}"),
                detail: format!(
                    "Commits on {name} since it left {base_name} (merge base {})",
                    &merge_base[..7]
                ),
                old: merge_base,
                new: Some(head),
            })
        }
    }
}

/// List a review's files and load patches within the eager budget. Large
/// files, binary files and files beyond the budget keep their statistics.
pub fn change_set(root: &Path, source: &ReviewSource) -> GitResult<ChangeSet> {
    let comparison = resolve_review(root, source)?;
    let (mut files, omitted_untracked) = review_files(root, &comparison)?;
    let mut budget = REVIEW_EAGER_LINES;
    let mut untracked_bytes = UNTRACKED_TOTAL_LIMIT;
    let mut eager = Vec::new();
    for (index, file) in files.iter_mut().enumerate() {
        if file.status == FileChange::Untracked {
            classify_untracked(root, file, &mut untracked_bytes, &mut budget);
            continue;
        }
        file.patch = match file.stats {
            Some(stats) if stats.binary => ReviewPatch::Binary,
            Some(stats) if stats.added + stats.removed > REVIEW_LARGE_FILE_LINES => {
                ReviewPatch::Large
            }
            Some(stats) if stats.added + stats.removed <= budget => {
                budget -= stats.added + stats.removed;
                eager.push(index);
                ReviewPatch::Deferred
            }
            _ => ReviewPatch::Deferred,
        };
    }
    let requested: Vec<_> = eager.iter().map(|index| files[*index].clone()).collect();
    for (index, patch) in eager
        .into_iter()
        .zip(review_patches(root, &comparison, &requested))
    {
        files[index].patch = patch;
    }
    Ok(ChangeSet {
        comparison,
        files,
        omitted_untracked,
    })
}

fn classify_untracked(
    root: &Path,
    file: &mut ReviewFile,
    bytes_left: &mut usize,
    budget: &mut u64,
) {
    let path = Path::new(&file.path);
    let metadata = std::fs::symlink_metadata(root.join(path));
    file.patch = match metadata {
        Err(error) => ReviewPatch::Unavailable(error.to_string()),
        Ok(metadata) if !metadata.file_type().is_file() => {
            ReviewPatch::Unavailable("Not a regular file; no preview".into())
        }
        Ok(metadata) if metadata.len() > UNTRACKED_FILE_LIMIT as u64 => {
            ReviewPatch::Unavailable("Untracked files over 1 MiB are not previewed".into())
        }
        Ok(metadata) if metadata.len() as usize > *bytes_left => ReviewPatch::Deferred,
        Ok(_) => match read_untracked_bytes(root, path, UNTRACKED_FILE_LIMIT) {
            Err(error) => ReviewPatch::Unavailable(error.details),
            Ok(bytes) => {
                *bytes_left = bytes_left.saturating_sub(bytes.len());
                let stats = added_file_stats(&bytes);
                file.stats = Some(stats);
                if stats.binary {
                    ReviewPatch::Binary
                } else if stats.added > REVIEW_LARGE_FILE_LINES {
                    ReviewPatch::Large
                } else if stats.added <= *budget {
                    *budget -= stats.added;
                    ReviewPatch::Loaded(untracked_patch(path, &bytes))
                } else {
                    ReviewPatch::Deferred
                }
            }
        },
    };
}

/// Load the patches of already listed files, one result per requested file.
/// Each failure is reported for its file; it never discards other patches.
pub fn review_patches(
    root: &Path,
    comparison: &ReviewComparison,
    files: &[ReviewFile],
) -> Vec<ReviewPatch> {
    let mut patches: Vec<Option<ReviewPatch>> = vec![None; files.len()];
    let mut tracked = Vec::new();
    for (index, file) in files.iter().enumerate() {
        if file.status == FileChange::Untracked {
            let path = Path::new(&file.path);
            patches[index] = Some(
                match read_untracked_bytes(root, path, UNTRACKED_FILE_LIMIT) {
                    Ok(bytes) if added_file_stats(&bytes).binary => ReviewPatch::Binary,
                    Ok(bytes) => ReviewPatch::Loaded(untracked_patch(path, &bytes)),
                    Err(error) => ReviewPatch::Unavailable(error.details),
                },
            );
        } else {
            tracked.push(index);
        }
    }
    let mut start = 0;
    while start < tracked.len() {
        let mut end = start;
        let mut bytes = 0;
        while end < tracked.len() && end - start < REVIEW_CHUNK_FILES {
            let file = &files[tracked[end]];
            let size = file.path.len() + file.original_path.as_ref().map_or(0, String::len);
            if end > start && bytes + size > REVIEW_CHUNK_PATH_BYTES {
                break;
            }
            bytes += size;
            end += 1;
        }
        let chunk: Vec<_> = tracked[start..end]
            .iter()
            .map(|index| &files[*index])
            .collect();
        let results = match tracked_review_patches(root, comparison, &chunk) {
            Ok(texts) => texts.into_iter().map(ReviewPatch::Loaded).collect(),
            // One oversized patch must not hide the other files of its chunk.
            Err(_) if chunk.len() > 1 => chunk
                .iter()
                .map(
                    |file| match tracked_review_patches(root, comparison, &[file]) {
                        Ok(mut texts) => ReviewPatch::Loaded(texts.remove(0)),
                        Err(error) => ReviewPatch::Unavailable(error.details),
                    },
                )
                .collect(),
            Err(error) => vec![ReviewPatch::Unavailable(error.details)],
        };
        for (index, patch) in tracked[start..end].iter().zip(results) {
            patches[*index] = Some(patch);
        }
        start = end;
    }
    patches
        .into_iter()
        .map(|patch| patch.unwrap_or_else(|| ReviewPatch::Unavailable("No patch".into())))
        .collect()
}

/// One Git command for several files. Blocks are matched to files by their
/// exact `diff --git` header, so neither configured diff order nor extra
/// blocks can attach a patch to the wrong file.
fn tracked_review_patches(
    root: &Path,
    comparison: &ReviewComparison,
    files: &[&ReviewFile],
) -> GitResult<Vec<String>> {
    const OPERATION: &str = "read review patch";
    let mut paths = Vec::new();
    for file in files {
        paths.push(PathBuf::from(&file.path));
        if let Some(original) = &file.original_path {
            paths.push(PathBuf::from(original));
        }
    }
    validate_paths(&paths, OPERATION)?;
    let mut args = string_args(&[
        "-c",
        "core.quotePath=false",
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "--find-renames",
        "--src-prefix=a/",
        "--dst-prefix=b/",
    ]);
    args.extend(comparison.range_args());
    args.push(OsString::from("--"));
    args.extend(paths.iter().map(|path| path.as_os_str().to_owned()));
    let output = checked_git(root, &args, OPERATION)?;
    let text = String::from_utf8_lossy(&output.stdout);
    let headers: Vec<_> = files
        .iter()
        .map(|file| {
            format!(
                "diff --git {} {}",
                quoted_patch_path("a/", file.original_path.as_deref().unwrap_or(&file.path)),
                quoted_patch_path("b/", &file.path)
            )
        })
        .collect();
    let mut patches = vec![String::new(); files.len()];
    let mut assigned = vec![false; files.len()];
    for block in split_patch_blocks(&text) {
        let header = block
            .lines()
            .next()
            .unwrap_or_default()
            .trim_end_matches('\r');
        let index = headers
            .iter()
            .enumerate()
            .position(|(index, expected)| !assigned[index] && expected == header);
        if let Some(index) = index {
            assigned[index] = true;
            patches[index] = block.to_owned();
        }
    }
    Ok(patches)
}

pub(super) fn split_patch_blocks(text: &str) -> Vec<&str> {
    let mut starts: Vec<_> = text
        .match_indices("diff --git ")
        .map(|(index, _)| index)
        .filter(|index| *index == 0 || text.as_bytes()[index - 1] == b'\n')
        .collect();
    starts.push(text.len());
    starts
        .windows(2)
        .map(|range| &text[range[0]..range[1]])
        .collect()
}

/// A path as Git prints it in a patch header with `core.quotePath=false`.
pub(super) fn quoted_patch_path(prefix: &str, path: &str) -> String {
    let full = format!("{prefix}{path}");
    if !full
        .bytes()
        .any(|byte| byte < 0x20 || byte == 0x7f || byte == b'"' || byte == b'\\')
    {
        return full;
    }
    let mut quoted = vec![b'"'];
    for byte in full.bytes() {
        match byte {
            7 => quoted.extend_from_slice(b"\\a"),
            8 => quoted.extend_from_slice(b"\\b"),
            b'\t' => quoted.extend_from_slice(b"\\t"),
            b'\n' => quoted.extend_from_slice(b"\\n"),
            11 => quoted.extend_from_slice(b"\\v"),
            12 => quoted.extend_from_slice(b"\\f"),
            b'\r' => quoted.extend_from_slice(b"\\r"),
            b'"' => quoted.extend_from_slice(b"\\\""),
            b'\\' => quoted.extend_from_slice(b"\\\\"),
            byte if byte < 0x20 || byte == 0x7f => {
                quoted.extend_from_slice(format!("\\{byte:03o}").as_bytes())
            }
            byte => quoted.push(byte),
        }
    }
    quoted.push(b'"');
    String::from_utf8_lossy(&quoted).into_owned()
}

/// Tracked changes from `--raw` (blob IDs) and `--numstat`, plus untracked
/// files for a working-tree review, in folder order.
fn review_files(root: &Path, comparison: &ReviewComparison) -> GitResult<(Vec<ReviewFile>, usize)> {
    const OPERATION: &str = "list review files";
    let common = [
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "--find-renames",
        "-z",
    ];
    let mut raw_args = string_args(&common);
    raw_args.extend(string_args(&["--raw", "--no-abbrev"]));
    raw_args.extend(comparison.range_args());
    raw_args.push(OsString::from("--"));
    let raw = checked_git(root, &raw_args, OPERATION)?;
    let mut numstat_args = string_args(&common);
    numstat_args.push(OsString::from("--numstat"));
    numstat_args.extend(comparison.range_args());
    numstat_args.push(OsString::from("--"));
    let numstat = checked_git(root, &numstat_args, OPERATION)?;
    let statistics: HashMap<_, _> = parse_numstat(&numstat.stdout)?.into_iter().collect();
    let working_tree = comparison.new.is_none();
    let mut seen = std::collections::HashSet::new();
    let mut files = Vec::new();
    for change in parse_review_raw(&raw.stdout)? {
        // Git can report an unmerged path more than once.
        if !seen.insert(change.path.clone()) {
            continue;
        }
        let unhashed = change.new_blob.bytes().all(|byte| byte == b'0');
        let identity = (working_tree && unhashed && change.status != b'D')
            .then(|| working_file_identity(&root.join(&change.path)));
        let content_key = review_content_key(&(
            change.status,
            &change.path,
            &change.original_path,
            &change.old_blob,
            &change.new_blob,
            identity,
        ));
        files.push(ReviewFile {
            stats: statistics.get(&change.path).copied(),
            path: change.path,
            original_path: change.original_path,
            status: change_from_code(change.status as char),
            content_key,
            patch: ReviewPatch::Deferred,
        });
    }
    let mut omitted = 0;
    if working_tree {
        let output = checked_git(
            root,
            &string_args(&["ls-files", "--others", "--exclude-standard", "-z"]),
            "list untracked files",
        )?;
        let mut listed = 0;
        for path in nul_records(&output.stdout, "list untracked files")? {
            let path = String::from_utf8_lossy(path).into_owned();
            if path.is_empty() || seen.contains(&path) {
                continue;
            }
            if listed == REVIEW_UNTRACKED_LIMIT {
                omitted += 1;
                continue;
            }
            listed += 1;
            let identity = working_file_identity(&root.join(&path));
            files.push(ReviewFile {
                content_key: review_content_key(&(b'?', &path, identity)),
                path,
                original_path: None,
                status: FileChange::Untracked,
                stats: None,
                patch: ReviewPatch::Deferred,
            });
        }
    }
    files.sort_by_cached_key(|file| review_order(&file.path));
    Ok((files, omitted))
}

/// Folder first, then file name; case-insensitive with a byte-order tiebreak.
fn review_order(path: &str) -> (String, String, String) {
    let (folder, name) = path.rsplit_once('/').unwrap_or(("", path));
    (folder.to_lowercase(), name.to_lowercase(), path.to_owned())
}

fn working_file_identity(path: &Path) -> Option<(u64, u128)> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |time| time.as_nanos());
    Some((metadata.len(), modified))
}

/// HEAD versions above this size get no gutter change markers.
pub const BASE_VERSION_LIMIT: u64 = 2 * 1024 * 1024;

/// A file's committed version, compared with an editor buffer for gutter
/// change markers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BaseVersion {
    /// The committed UTF-8 text, without a byte order mark.
    Text(String),
    /// Not in the commit but staged: every line of the buffer is added.
    Added,
    /// Neither committed nor staged, for example untracked or ignored.
    Untracked,
    Binary,
    TooLarge,
    /// A symbolic link, submodule, Git LFS pointer or non-UTF-8 text.
    Unsupported,
}

/// Read `path` (relative to the repository root, `/`-separated) from
/// `commit` without running filters, text conversion or LFS. `None` means
/// the branch has no commit yet. Two Git processes at most.
pub fn base_version(root: &Path, commit: Option<&str>, path: &str) -> GitResult<BaseVersion> {
    const OPERATION: &str = "read committed version";
    validate_paths(&[PathBuf::from(path)], OPERATION)?;
    if let Some(commit) = commit {
        validate_commit_hash(commit)?;
        let listing = checked_git(
            root,
            &[
                OsString::from("ls-tree"),
                OsString::from("-z"),
                OsString::from("--long"),
                OsString::from("--full-tree"),
                OsString::from(commit),
                OsString::from("--"),
                OsString::from(path),
            ],
            OPERATION,
        )?;
        for record in nul_records(&listing.stdout, OPERATION)? {
            let Some(tab) = record.iter().position(|byte| *byte == b'\t') else {
                continue;
            };
            if &record[tab + 1..] != path.as_bytes() {
                continue;
            }
            let fields = String::from_utf8_lossy(&record[..tab]);
            let fields: Vec<_> = fields.split_ascii_whitespace().collect();
            let [mode, kind, object, size] = fields[..] else {
                return Err(GitError::new(
                    OPERATION,
                    "Git returned an unexpected tree entry",
                ));
            };
            if kind != "blob" || !matches!(mode, "100644" | "100755") {
                return Ok(BaseVersion::Unsupported);
            }
            validate_commit_hash(object)?;
            if size
                .parse::<u64>()
                .map_or(true, |size| size > BASE_VERSION_LIMIT)
            {
                return Ok(BaseVersion::TooLarge);
            }
            let blob = checked_git(root, &string_args(&["cat-file", "blob", object]), OPERATION)?;
            return Ok(base_text(blob.stdout));
        }
    }
    let staged = checked_git(
        root,
        &[
            OsString::from("ls-files"),
            OsString::from("-z"),
            OsString::from("--cached"),
            OsString::from("--full-name"),
            OsString::from("--"),
            OsString::from(path),
        ],
        OPERATION,
    )?;
    let staged = nul_records(&staged.stdout, OPERATION)?
        .into_iter()
        .any(|record| record == path.as_bytes());
    Ok(if staged {
        BaseVersion::Added
    } else {
        BaseVersion::Untracked
    })
}

fn base_text(bytes: Vec<u8>) -> BaseVersion {
    if bytes.iter().take(8000).any(|byte| *byte == 0) {
        return BaseVersion::Binary;
    }
    // A pointer would compare as a whole-file change with the checked-out content.
    if bytes.starts_with(b"version https://git-lfs.github.com/spec/") {
        return BaseVersion::Unsupported;
    }
    match String::from_utf8(bytes) {
        Ok(mut text) => {
            if text.starts_with('\u{feff}') {
                text.drain(..'\u{feff}'.len_utf8());
            }
            BaseVersion::Text(text)
        }
        Err(_) => BaseVersion::Unsupported,
    }
}

/// Changed paths whose size and modification time a fingerprint includes.
const FINGERPRINT_FILES: usize = 2_000;

/// A key that changes with HEAD, the reported status and the size or
/// modification time of every changed file. Polls compare it to notice
/// edits of files that already were modified.
pub fn status_fingerprint(root: &Path, head: Option<&StatusHead>, files: &[FileStatus]) -> u64 {
    let mut entries: Vec<_> = files
        .iter()
        .enumerate()
        .map(|(index, file)| {
            let identity = (index < FINGERPRINT_FILES && file.worktree != FileChange::Deleted)
                .then(|| {
                    working_file_identity(
                        &file
                            .path
                            .split('/')
                            .fold(root.to_path_buf(), |path, part| path.join(part)),
                    )
                })
                .flatten();
            (
                &file.path,
                &file.original_path,
                format!("{:?}{:?}", file.index, file.worktree),
                identity,
            )
        })
        .collect();
    entries.sort();
    review_content_key(&(head, entries))
}

/// HEAD and [`status_fingerprint`] read now, with one status process.
pub fn working_tree_fingerprint(root: &Path) -> GitResult<(Option<StatusHead>, u64)> {
    let (status, _, head) = status_with_head(root)?;
    let key = status_fingerprint(root, head.as_ref(), &status.files);
    Ok((head, key))
}

fn review_content_key(value: &impl std::hash::Hash) -> u64 {
    use std::hash::Hasher;
    let mut hasher = std::hash::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

struct RawChange {
    status: u8,
    old_blob: String,
    new_blob: String,
    path: String,
    original_path: Option<String>,
}

/// `--raw -z`: `:old_mode new_mode old_blob new_blob status` NUL path NUL,
/// with a second path for renames and copies.
fn parse_review_raw(bytes: &[u8]) -> GitResult<Vec<RawChange>> {
    const OPERATION: &str = "parse review files";
    let invalid = || GitError::new(OPERATION, "Git returned an unexpected raw diff record");
    let records = nul_records(bytes, OPERATION)?;
    let mut records = records.into_iter();
    let mut changes = Vec::new();
    while let Some(header) = records.next() {
        let header = header.strip_prefix(b":").ok_or_else(invalid)?;
        let fields: Vec<_> = header.split(|byte| *byte == b' ').collect();
        let [_, _, old_blob, new_blob, status] = fields.as_slice() else {
            return Err(invalid());
        };
        let code = status.first().copied().ok_or_else(invalid)?;
        let path = records
            .next()
            .filter(|path| !path.is_empty())
            .ok_or_else(invalid)?;
        let (original_path, path) = if matches!(code, b'R' | b'C') {
            let destination = records
                .next()
                .filter(|path| !path.is_empty())
                .ok_or_else(invalid)?;
            (
                Some(String::from_utf8_lossy(path).into_owned()),
                destination,
            )
        } else {
            (None, path)
        };
        changes.push(RawChange {
            status: code,
            old_blob: String::from_utf8_lossy(old_blob).into_owned(),
            new_blob: String::from_utf8_lossy(new_blob).into_owned(),
            path: String::from_utf8_lossy(path).into_owned(),
            original_path,
        });
    }
    Ok(changes)
}

//! Filesystem work for background workers. No function in this module needs the UI thread.
//!
//! Indexing and search have hard limits and cancellation; the UI can retain its last
//! completed index while a refresh runs. Documents use UTF-8 and an LF editing buffer,
//! restoring the original line-ending convention and UTF-8 BOM when they are saved.

use ignore::WalkBuilder;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use regex::RegexBuilder;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, VecDeque};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

pub const MAX_DOCUMENT_BYTES: u64 = 8 * 1024 * 1024;
const MAX_WARNINGS: usize = 20;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug)]
pub struct IndexOptions {
    pub show_ignored_files: bool,
    pub max_entries: usize,
    pub max_depth: usize,
    pub max_duration: Duration,
    pub cancel: Option<Arc<AtomicBool>>,
}

impl Default for IndexOptions {
    fn default() -> Self {
        Self {
            show_ignored_files: false,
            max_entries: 100_000,
            max_depth: 64,
            max_duration: Duration::from_secs(15),
            cancel: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct WorkspaceEntry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub depth: usize,
}

#[derive(Clone, Debug)]
pub struct WorkspaceIndex {
    pub root: PathBuf,
    pub entries: Vec<WorkspaceEntry>,
    pub files: Vec<PathBuf>,
    /// A Cargo manifest admitted by the default index policy, independent of
    /// the Explorer's option to display ignored and generated files.
    pub has_rust_manifest: bool,
    pub truncated: bool,
    pub warnings: Vec<String>,
    children: BTreeMap<PathBuf, Vec<usize>>,
    searchable_files: Vec<String>,
    quick_open_cache: Arc<Mutex<VecDeque<QuickOpenResults>>>,
}

#[derive(Clone, Debug)]
struct QuickOpenResults {
    query: String,
    limit: usize,
    paths: Vec<PathBuf>,
}

impl WorkspaceIndex {
    pub fn empty(root: PathBuf) -> Self {
        Self {
            root,
            entries: Vec::new(),
            files: Vec::new(),
            has_rust_manifest: false,
            truncated: false,
            warnings: Vec::new(),
            children: BTreeMap::new(),
            searchable_files: Vec::new(),
            quick_open_cache: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    /// A cached directory listing, sorted with folders first. Does not touch the disk.
    pub fn children<'a>(&'a self, parent: &Path) -> impl Iterator<Item = &'a WorkspaceEntry> {
        self.children
            .get(parent)
            .into_iter()
            .flatten()
            .map(|&index| &self.entries[index])
    }

    /// Ranked path search. At most `limit` paths are cloned, even in large workspaces.
    pub fn quick_open(&self, query: &str, limit: usize) -> Vec<PathBuf> {
        let query = query.trim().replace('\\', "/").to_lowercase();
        let limit = limit.min(200);
        if limit == 0 {
            return Vec::new();
        }
        if query.is_empty() {
            return self.files.iter().take(limit).cloned().collect();
        }
        // The palette repaints while the query is unchanged. Avoid rescanning the
        // workspace on those frames; a fresh immutable index has its own cache.
        if let Ok(mut cache) = self.quick_open_cache.lock()
            && let Some(position) = cache
                .iter()
                .position(|item| item.query == query && item.limit == limit)
            && let Some(item) = cache.remove(position)
        {
            let paths = item.paths.clone();
            cache.push_front(item);
            return paths;
        }
        let mut best: BinaryHeap<Reverse<(i64, Reverse<usize>)>> = BinaryHeap::new();
        for (index, path) in self.searchable_files.iter().enumerate() {
            let Some(score) = path_score(path, &query) else {
                continue;
            };
            best.push(Reverse((score, Reverse(index))));
            if best.len() > limit {
                best.pop();
            }
        }
        let mut results: Vec<_> = best.into_iter().map(|Reverse(item)| item).collect();
        results.sort_unstable_by(|a, b| b.cmp(a));
        let paths: Vec<_> = results
            .into_iter()
            .map(|(_, Reverse(index))| self.files[index].clone())
            .collect();
        if let Ok(mut cache) = self.quick_open_cache.lock() {
            // Keep common backspace queries, with bounded memory even if a user
            // cycles through hundreds of different searches.
            const MAX_CACHED_QUERIES: usize = 8;
            cache.retain(|item| item.query != query || item.limit != limit);
            cache.push_front(QuickOpenResults {
                query,
                limit,
                paths: paths.clone(),
            });
            cache.truncate(MAX_CACHED_QUERIES);
        }
        paths
    }
}

fn path_score(path: &str, query: &str) -> Option<i64> {
    let basename = path.rsplit('/').next().unwrap_or(path);
    let length_penalty = (path.len() as i64).min(2_000);
    if basename == query {
        return Some(20_000 - length_penalty);
    }
    if basename.starts_with(query) {
        return Some(15_000 - length_penalty);
    }
    if let Some(position) = basename.find(query) {
        return Some(12_000 - length_penalty - position as i64);
    }
    if let Some(position) = path.find(query) {
        return Some(10_000 - length_penalty - position as i64);
    }
    let mut query_chars = query.chars();
    let mut next = query_chars.next()?;
    let mut first = None;
    let mut consecutive = 0;
    let mut previous = None;
    for (position, character) in path.chars().enumerate() {
        if character != next {
            continue;
        }
        first.get_or_insert(position);
        if previous == position.checked_sub(1) {
            consecutive += 1;
        }
        previous = Some(position);
        match query_chars.next() {
            Some(character) => next = character,
            None => {
                return Some(
                    5_000 - length_penalty - (position - first.unwrap_or(0)) as i64
                        + consecutive * 10,
                );
            }
        }
    }
    None
}

fn generated_index_name(name: &std::ffi::OsStr) -> bool {
    matches!(
        name.to_str(),
        Some(
            "target"
                | "node_modules"
                | ".next"
                | ".nuxt"
                | ".venv"
                | "venv"
                | ".cache"
                | ".vs"
                | "dist"
                | "build"
                | "coverage"
                | "__pycache__"
        )
    )
}

pub fn build_index(root: &Path, options: IndexOptions) -> Result<WorkspaceIndex, String> {
    if !root.is_dir() {
        return Err(format!(
            "Workspace folder is unavailable: {}",
            crate::paths::display(root)
        ));
    }
    let started = Instant::now();
    let mut index = WorkspaceIndex::empty(root.to_path_buf());
    let show_ignored_files = options.show_ignored_files;
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false)
        .follow_links(false)
        .ignore(true)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(false)
        .require_git(false)
        .max_depth(Some(options.max_depth))
        .filter_entry(move |entry| {
            if entry.depth() == 0 {
                return true;
            }
            if entry.file_type().is_some_and(|kind| kind.is_symlink()) {
                return false;
            }
            let name = entry.file_name().to_string_lossy();
            if [".git", ".hg", ".svn"]
                .iter()
                .any(|metadata| name.eq_ignore_ascii_case(metadata))
            {
                return false;
            }
            show_ignored_files || !generated_index_name(entry.file_name())
        });
    // Showing dependency caches must not give their Cargo manifests authority
    // to start Rust analysis. Match just the encountered manifest candidates
    // against the normal rules; directory matchers are lazy and cached, so no
    // second filesystem walk is necessary.
    let mut rust_matcher = if show_ignored_files {
        let matcher = builder.build_matchers().pop();
        builder.ignore(false).git_ignore(false).git_exclude(false);
        matcher
    } else {
        None
    };
    for result in builder.build() {
        if cancelled(&options.cancel)
            || started.elapsed() >= options.max_duration
            || index.entries.len() >= options.max_entries
        {
            index.truncated = true;
            break;
        }
        let entry = match result {
            Ok(entry) => entry,
            Err(error) => {
                if index.warnings.len() < MAX_WARNINGS {
                    index.warnings.push(error.to_string());
                }
                continue;
            }
        };
        if entry.depth() == 0 {
            continue;
        }
        let Some(file_type) = entry.file_type() else {
            continue;
        };
        // Exclude links, devices and sockets; in particular, never recurse out of a project.
        if !(file_type.is_file() || file_type.is_dir()) {
            continue;
        }
        if file_type.is_dir() && entry.depth() == options.max_depth {
            index.truncated = true;
        }
        if !index.has_rust_manifest
            && file_type.is_file()
            && entry.file_name() == "Cargo.toml"
            && let Ok(relative) = entry.path().strip_prefix(root)
            && !relative
                .components()
                .any(|part| generated_index_name(part.as_os_str()))
        {
            let ignored = if let Some(matcher) = &mut rust_matcher {
                let (matched, error) = matcher.matched_with_errors(relative, false);
                if let Some(error) = error
                    && index.warnings.len() < MAX_WARNINGS
                {
                    index.warnings.push(error.to_string());
                }
                matched.is_ignore()
            } else {
                false
            };
            index.has_rust_manifest = !ignored;
        }
        index.entries.push(WorkspaceEntry {
            path: entry.path().to_path_buf(),
            name: entry.file_name().to_string_lossy().into_owned(),
            is_dir: file_type.is_dir(),
            depth: entry.depth(),
        });
    }
    index.entries.sort_unstable_by(|a, b| {
        a.path
            .parent()
            .cmp(&b.path.parent())
            .then_with(|| b.is_dir.cmp(&a.is_dir))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.path.cmp(&b.path))
    });
    for (position, entry) in index.entries.iter().enumerate() {
        if let Some(parent) = entry.path.parent() {
            index
                .children
                .entry(parent.to_path_buf())
                .or_default()
                .push(position);
        }
        if !entry.is_dir {
            index.files.push(entry.path.clone());
        }
    }
    index.files.sort_unstable_by_key(|path| {
        path.strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_lowercase()
    });
    index.searchable_files = index
        .files
        .iter()
        .map(|path| {
            path.strip_prefix(root)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/")
                .to_lowercase()
        })
        .collect();
    Ok(index)
}

fn cancelled(cancel: &Option<Arc<AtomicBool>>) -> bool {
    cancel
        .as_ref()
        .is_some_and(|cancel| cancel.load(Ordering::Relaxed))
}

#[derive(Clone, Debug)]
pub struct SearchOptions {
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub max_matches: usize,
    pub max_file_bytes: u64,
    pub max_total_bytes: u64,
    pub max_duration: Duration,
    pub cancel: Option<Arc<AtomicBool>>,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            case_sensitive: false,
            whole_word: false,
            max_matches: 1_000,
            max_file_bytes: 2 * 1024 * 1024,
            max_total_bytes: 128 * 1024 * 1024,
            max_duration: Duration::from_secs(10),
            cancel: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SearchMatch {
    pub path: PathBuf,
    pub line: usize,
    pub column: usize,
    pub start_char: usize,
    pub end_char: usize,
    pub preview: String,
}

#[derive(Clone, Debug, Default)]
pub struct SearchResults {
    pub matches: Vec<SearchMatch>,
    pub files_searched: usize,
    pub files_skipped: usize,
    pub truncated: bool,
    pub warnings: Vec<String>,
}

/// Optional project-search filters. The Explorer's cached index has independent
/// pruning: a search with an empty list can inspect these folders when Git allows it.
pub fn default_search_excludes() -> Vec<String> {
    [
        "node_modules",
        "bower_components",
        "vendor",
        "target",
        ".git",
        "dist",
        "build",
        ".venv",
        "venv",
        "__pycache__",
        ".next",
        ".nuxt",
        "coverage",
        ".cache",
        ".vs",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

pub fn normalize_search_excludes(excludes: &[String]) -> Result<Vec<String>, String> {
    crate::cli::normalize_search_excludes(excludes)
}

fn search_exclusion_matcher(root: &Path, excludes: &[String]) -> Result<Gitignore, String> {
    let excludes = normalize_search_excludes(excludes)?;
    let mut builder = GitignoreBuilder::new(root);
    builder.allow_unclosed_class(false);
    builder
        .case_insensitive(cfg!(windows))
        .map_err(|error| error.to_string())?;
    for pattern in excludes {
        // A list of patterns has no comment lines: a folder named #cache should
        // be excluded just like every other literal folder name.
        let literal_hash;
        let pattern = if pattern.starts_with('#') {
            literal_hash = format!("\\{pattern}");
            &literal_hash
        } else {
            &pattern
        };
        builder.add_line(None, pattern).map_err(|error| {
            format!("Invalid search exclusion {pattern:?}: {error}. Use a folder or glob such as **/*.min.js.")
        })?;
    }
    builder
        .build()
        .map_err(|error| format!("Invalid search exclusions: {error}"))
}

#[cfg(test)]
fn validate_search_excludes(excludes: &[String]) -> Result<(), String> {
    search_exclusion_matcher(Path::new("."), excludes).map(|_| ())
}

/// Search a fresh, bounded filesystem walk on a background worker. Folder and
/// glob exclusions are applied before reading files or descending directories.
/// Git ignore files remain respected even when the optional list is empty.
pub fn search_with_excludes(
    root: &Path,
    query: &str,
    options: SearchOptions,
    excludes: &[String],
) -> Result<SearchResults, String> {
    let started = Instant::now();
    let root = fs::canonicalize(root)
        .map_err(|error| format!("Could not open search project: {error}"))?;
    if !root.is_dir() {
        return Err("The search project must be a folder.".into());
    }
    let matcher = search_exclusion_matcher(&root, excludes)?;
    let cancel = options.cancel.clone();
    let duration = options.max_duration;
    let count = Arc::new(AtomicUsize::new(0));
    let bounded = Arc::new(AtomicBool::new(false));
    let bounded_walk = bounded.clone();
    let mut walker = WalkBuilder::new(&root);
    walker
        .hidden(false)
        .follow_links(false)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(false)
        .require_git(false)
        .max_depth(Some(64))
        .filter_entry(move |entry| {
            if entry.depth() == 0 {
                return true;
            }
            if cancelled(&cancel)
                || started.elapsed() >= duration
                || count.fetch_add(1, Ordering::Relaxed) >= 100_000
            {
                bounded_walk.store(true, Ordering::Relaxed);
                return false;
            }
            let is_dir = entry.file_type().is_some_and(|kind| kind.is_dir());
            if is_dir && matches!(entry.file_name().to_str(), Some(".git" | ".hg" | ".svn")) {
                return false;
            }
            if is_dir && entry.depth() == 64 {
                bounded_walk.store(true, Ordering::Relaxed);
            }
            !matcher.matched(entry.path(), is_dir).is_ignore()
        });
    let paths = walker.build().filter_map(|entry| match entry {
        Ok(entry) if entry.file_type().is_some_and(|kind| kind.is_file()) => {
            Some(Ok(entry.path().to_path_buf()))
        }
        Ok(_) => None,
        Err(error) => Some(Err(error.to_string())),
    });
    let mut result = search_paths(paths, query, options, started)?;
    result.truncated |= bounded.load(Ordering::Relaxed);
    Ok(result)
}

/// Literal project search; punctuation in a query is never interpreted as a regex.
#[cfg(test)]
pub fn search_project(
    index: &WorkspaceIndex,
    query: &str,
    options: SearchOptions,
) -> Result<SearchResults, String> {
    let mut results = search_paths(
        index.files.iter().cloned().map(Ok),
        query,
        options,
        Instant::now(),
    )?;
    results.truncated |= index.truncated;
    Ok(results)
}

fn search_paths(
    paths: impl Iterator<Item = Result<PathBuf, String>>,
    query: &str,
    options: SearchOptions,
    started: Instant,
) -> Result<SearchResults, String> {
    let mut results = SearchResults::default();
    if query.is_empty() {
        return Ok(results);
    }
    if query.len() > 16_384 {
        return Err("Search query is too long (maximum 16 KiB)".into());
    }
    let pattern = RegexBuilder::new(&regex::escape(query))
        .case_insensitive(!options.case_sensitive)
        .build()
        .map_err(|error| format!("Could not prepare search: {error}"))?;
    let mut bytes_searched = 0u64;
    'files: for path in paths {
        if cancelled(&options.cancel)
            || started.elapsed() >= options.max_duration
            || results.matches.len() >= options.max_matches
            || bytes_searched >= options.max_total_bytes
        {
            results.truncated = true;
            break;
        }
        let path = match path {
            Ok(path) => path,
            Err(error) => {
                if results.warnings.len() < MAX_WARNINGS {
                    results.warnings.push(error);
                }
                continue;
            }
        };
        let remaining = options.max_total_bytes.saturating_sub(bytes_searched);
        let limit = remaining.min(options.max_file_bytes);
        let document = match read_document(&path, limit) {
            Ok(document) => document,
            Err(error) => {
                results.files_skipped += 1;
                // Binary files and large assets are expected. Keep errors actionable.
                if !error.contains("binary")
                    && !error.contains("UTF-8")
                    && !error.contains("too large")
                    && results.warnings.len() < MAX_WARNINGS
                {
                    results.warnings.push(error);
                }
                continue;
            }
        };
        bytes_searched += document.fingerprint.len;
        results.files_searched += 1;
        let text = &document.text;
        let mut last_byte = 0;
        let mut char_offset = 0;
        let mut line = 1;
        let mut line_start = 0;
        for found in pattern.find_iter(text) {
            if cancelled(&options.cancel) || started.elapsed() >= options.max_duration {
                results.truncated = true;
                break 'files;
            }
            if options.whole_word
                && (text[..found.start()]
                    .chars()
                    .next_back()
                    .is_some_and(crate::search::word_char)
                    || text[found.end()..]
                        .chars()
                        .next()
                        .is_some_and(crate::search::word_char))
            {
                continue;
            }
            let preceding = &text[last_byte..found.start()];
            char_offset += preceding.chars().count();
            for (position, character) in preceding.char_indices() {
                if character == '\n' {
                    line += 1;
                    line_start = last_byte + position + 1;
                }
            }
            let start_char = char_offset;
            let match_chars = found.as_str().chars().count();
            let line_end = text[found.start()..]
                .find('\n')
                .map_or(text.len(), |end| found.start() + end);
            let preview = line_preview(&text[line_start..line_end], found.start() - line_start);
            results.matches.push(SearchMatch {
                path: path.clone(),
                line,
                column: text[line_start..found.start()].chars().count() + 1,
                start_char,
                end_char: start_char + match_chars,
                preview,
            });
            // Include matches containing newlines in the next result's line accounting.
            last_byte = found.start();
            if results.matches.len() >= options.max_matches {
                results.truncated = true;
                break 'files;
            }
        }
    }
    Ok(results)
}

fn line_preview(line: &str, match_byte: usize) -> String {
    const PREVIEW_CHARS: usize = 240;
    let match_char = line[..match_byte].chars().count();
    let start = match_char.saturating_sub(80);
    let mut chars = line.chars().skip(start);
    let mut result = String::new();
    if start > 0 {
        result.push('…');
    }
    result.extend(chars.by_ref().take(PREVIEW_CHARS));
    if chars.next().is_some() {
        result.push('…');
    }
    result
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum LineEnding {
    #[default]
    Lf,
    CrLf,
    Cr,
    Mixed,
}

impl fmt::Display for LineEnding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Lf => "LF",
            Self::CrLf => "CRLF",
            Self::Cr => "CR",
            Self::Mixed => "Mixed",
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FileFingerprint {
    pub len: u64,
    pub modified: Option<SystemTime>,
    pub hash: u64,
}

impl FileFingerprint {
    pub(crate) fn same_content(&self, other: &Self) -> bool {
        self.len == other.len && self.hash == other.hash
    }
}

#[derive(Clone, Debug)]
pub struct LoadedDocument {
    pub text: String,
    pub line_ending: LineEnding,
    pub bom: bool,
    pub mixed_line_endings: bool,
    pub fingerprint: FileFingerprint,
}

pub fn read_document(path: &Path, max_bytes: u64) -> Result<LoadedDocument, String> {
    let (bytes, fingerprint) = read_bytes(path, max_bytes)
        .map_err(|error| format!("Could not open {}: {error}", crate::paths::display(path)))?;
    let bom = bytes.starts_with(&[0xef, 0xbb, 0xbf]);
    let bytes = if bom { &bytes[3..] } else { &bytes[..] };
    if bytes.contains(&0) {
        return Err(format!(
            "{} is a binary or UTF-16 file; only UTF-8 text is supported",
            crate::paths::display(path)
        ));
    }
    let source = std::str::from_utf8(bytes)
        .map_err(|_| format!("{} is not valid UTF-8 text", crate::paths::display(path)))?;
    let controls = source
        .chars()
        .take(8_192)
        .filter(|character| {
            character.is_control() && !matches!(character, '\n' | '\r' | '\t' | '\u{c}')
        })
        .count();
    if controls > 8 {
        return Err(format!(
            "{} appears to be a binary file",
            crate::paths::display(path)
        ));
    }
    let crlf = source.matches("\r\n").count();
    let cr = source.matches('\r').count() - crlf;
    let lf = source.matches('\n').count() - crlf;
    let mixed_line_endings = [crlf, cr, lf]
        .into_iter()
        .filter(|count| *count > 0)
        .count()
        > 1;
    let line_ending = if mixed_line_endings {
        LineEnding::Mixed
    } else if crlf > 0 && crlf >= cr && crlf >= lf {
        LineEnding::CrLf
    } else if cr > lf {
        LineEnding::Cr
    } else {
        LineEnding::Lf
    };
    Ok(LoadedDocument {
        text: source.replace("\r\n", "\n").replace('\r', "\n"),
        line_ending,
        bom,
        mixed_line_endings,
        fingerprint,
    })
}

pub fn fingerprint(path: &Path, max_bytes: u64) -> Result<FileFingerprint, String> {
    read_bytes(path, max_bytes)
        .map(|(_, fingerprint)| fingerprint)
        .map_err(|error| {
            format!(
                "Could not inspect {} before saving: {error}",
                crate::paths::display(path)
            )
        })
}

fn read_bytes(path: &Path, max_bytes: u64) -> io::Result<(Vec<u8>, FileFingerprint)> {
    let mut file = File::open(path)?;
    let before = file.metadata()?;
    if !before.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    if before.len() > max_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("file is too large (limit {} MiB)", max_bytes / 1024 / 1024),
        ));
    }
    let mut bytes = Vec::with_capacity(before.len().min(max_bytes) as usize);
    (&mut file)
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "file is too large",
        ));
    }
    let after = file.metadata()?;
    if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
        return Err(io::Error::other(
            "file changed while it was being read; try again",
        ));
    }
    let fingerprint = FileFingerprint {
        len: bytes.len() as u64,
        modified: after.modified().ok(),
        hash: content_hash(&bytes),
    };
    Ok((bytes, fingerprint))
}

fn content_hash(bytes: &[u8]) -> u64 {
    // A deterministic content fingerprint avoids missing same-size writes with coarse timestamps.
    bytes.iter().fold(0xcbf29ce484222325u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

#[derive(Debug)]
pub enum SaveError {
    Conflict(PathBuf),
    Io(String),
}

impl fmt::Display for SaveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflict(path) => write!(
                formatter,
                "{} changed on disk. Reload it or use Save As to preserve your edits.",
                crate::paths::display(path)
            ),
            Self::Io(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for SaveError {}

/// Atomically replace a document using a temporary file in the same directory.
/// `expected = None` creates a new file and refuses to overwrite an existing one.
/// Existing files must match the last loaded/saved content fingerprint.
pub fn save_document(
    path: &Path,
    text: &str,
    line_ending: LineEnding,
    bom: bool,
    expected: Option<&FileFingerprint>,
) -> Result<FileFingerprint, SaveError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let current_metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(SaveError::Io(format!(
                "Could not inspect {}: {error}",
                crate::paths::display(path)
            )));
        }
    };
    if let Some(metadata) = &current_metadata {
        if metadata.file_type().is_symlink() {
            return Err(SaveError::Io(
                "Open the symbolic link's target before saving it".into(),
            ));
        }
        if metadata.permissions().readonly() {
            return Err(SaveError::Io(format!(
                "{} is read-only",
                crate::paths::display(path)
            )));
        }
    }
    check_expected(path, expected)?;
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let encoded = match line_ending {
        LineEnding::Lf => normalized,
        LineEnding::CrLf => normalized.replace('\n', "\r\n"),
        LineEnding::Cr => normalized.replace('\n', "\r"),
        LineEnding::Mixed => encode_mixed_endings(path, &normalized, expected)?,
    };
    let mut bytes = Vec::with_capacity(encoded.len() + usize::from(bom) * 3);
    if bom {
        bytes.extend_from_slice(&[0xef, 0xbb, 0xbf]);
    }
    bytes.extend_from_slice(encoded.as_bytes());
    if bytes.len() as u64 > MAX_DOCUMENT_BYTES {
        return Err(SaveError::Io(format!(
            "Document is too large to save safely (maximum {} MiB)",
            MAX_DOCUMENT_BYTES / 1024 / 1024
        )));
    }
    let (temp_path, mut temporary) = create_temporary(parent)
        .map_err(|error| SaveError::Io(format!("Could not create save file: {error}")))?;
    let result = (|| -> Result<FileFingerprint, SaveError> {
        temporary
            .write_all(&bytes)
            .and_then(|_| temporary.sync_all())
            .map_err(|error| {
                SaveError::Io(format!(
                    "Could not write {}: {error}",
                    crate::paths::display(path)
                ))
            })?;
        if let Some(metadata) = current_metadata {
            temporary
                .set_permissions(metadata.permissions())
                .map_err(|error| {
                    SaveError::Io(format!("Could not preserve file permissions: {error}"))
                })?;
        }
        // The write may take time. Check again immediately before committing it.
        check_expected(path, expected)?;
        drop(temporary);
        if expected.is_none() {
            // Publish without replacing another file created after the final check.
            publish_new_file(&temp_path, path).map_err(|error| {
                if error.kind() == io::ErrorKind::AlreadyExists {
                    SaveError::Conflict(path.to_path_buf())
                } else {
                    SaveError::Io(format!(
                        "Could not save {}: {error}",
                        crate::paths::display(path)
                    ))
                }
            })?;
        } else {
            fs::rename(&temp_path, path).map_err(|error| {
                SaveError::Io(format!(
                    "Could not replace {}: {error}",
                    crate::paths::display(path)
                ))
            })?;
        }
        let metadata = fs::metadata(path)
            .map_err(|error| SaveError::Io(format!("Saved file is unavailable: {error}")))?;
        Ok(FileFingerprint {
            len: bytes.len() as u64,
            modified: metadata.modified().ok(),
            hash: content_hash(&bytes),
        })
    })();
    let _ = fs::remove_file(&temp_path);
    result
}

fn check_expected(path: &Path, expected: Option<&FileFingerprint>) -> Result<(), SaveError> {
    match expected {
        None => match fs::symlink_metadata(path) {
            Ok(_) => Err(SaveError::Conflict(path.to_path_buf())),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(SaveError::Io(format!(
                "Could not inspect {}: {error}",
                crate::paths::display(path)
            ))),
        },
        Some(expected) => {
            let (_, current) =
                read_bytes(path, expected.len.saturating_add(1)).map_err(|error| {
                    if matches!(
                        error.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::InvalidInput
                    ) {
                        SaveError::Conflict(path.to_path_buf())
                    } else {
                        SaveError::Io(format!(
                            "Could not verify {} before saving: {error}",
                            crate::paths::display(path)
                        ))
                    }
                })?;
            if current.same_content(expected) {
                Ok(())
            } else {
                Err(SaveError::Conflict(path.to_path_buf()))
            }
        }
    }
}

fn create_temporary(parent: &Path) -> io::Result<(PathBuf, File)> {
    for _ in 0..64 {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(
            ".morrow-save-{}-{sequence}.tmp",
            std::process::id()
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "no temporary save filename is available",
    ))
}

/// Preserve existing mixed separators around edits. Unchanged prefix/suffix lines keep
/// their exact separators; newly inserted lines adopt the most common existing ending.
fn encode_mixed_endings(
    path: &Path,
    normalized: &str,
    expected: Option<&FileFingerprint>,
) -> Result<String, SaveError> {
    let Some(expected) = expected else {
        return Ok(normalized.to_owned());
    };
    let (bytes, fingerprint) = read_bytes(path, expected.len.saturating_add(1))
        .map_err(|error| SaveError::Io(format!("Could not preserve line endings: {error}")))?;
    if !fingerprint.same_content(expected) {
        return Err(SaveError::Conflict(path.to_path_buf()));
    }
    let source = std::str::from_utf8(bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes))
        .map_err(|_| SaveError::Io("Existing file is no longer UTF-8 text".into()))?;
    let old_lines = line_records(source);
    let new_lines = line_records(normalized);
    let mut counts = [0usize; 3];
    for (_, ending) in &old_lines {
        match *ending {
            "\r\n" => counts[0] += 1,
            "\n" => counts[1] += 1,
            "\r" => counts[2] += 1,
            _ => {}
        }
    }
    let common = if counts[0] > 0 && counts[0] >= counts[1] && counts[0] >= counts[2] {
        "\r\n"
    } else if counts[2] > counts[1] {
        "\r"
    } else {
        "\n"
    };
    let prefix = old_lines
        .iter()
        .zip(&new_lines)
        .take_while(|(old, new)| old.0 == new.0)
        .count();
    let suffix = old_lines[prefix..]
        .iter()
        .rev()
        .zip(new_lines[prefix..].iter().rev())
        .take_while(|(old, new)| old.0 == new.0)
        .count();
    let equal_middle = old_lines.len().saturating_sub(prefix + suffix)
        == new_lines.len().saturating_sub(prefix + suffix);
    let mut encoded = String::with_capacity(normalized.len());
    for (index, (line, ending)) in new_lines.iter().enumerate() {
        encoded.push_str(line);
        if ending.is_empty() {
            continue;
        }
        let previous = if index < prefix || equal_middle {
            old_lines.get(index)
        } else if index >= new_lines.len() - suffix {
            old_lines.get(old_lines.len() - (new_lines.len() - index))
        } else {
            None
        };
        encoded.push_str(
            previous
                .map(|(_, ending)| *ending)
                .filter(|ending| !ending.is_empty())
                .unwrap_or(common),
        );
    }
    Ok(encoded)
}

fn line_records(text: &str) -> Vec<(&str, &str)> {
    let bytes = text.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0;
    let mut position = 0;
    while position < bytes.len() {
        if matches!(bytes[position], b'\r' | b'\n') {
            let end = position;
            position += 1;
            if bytes[end] == b'\r' && bytes.get(position) == Some(&b'\n') {
                position += 1;
            }
            lines.push((&text[start..end], &text[end..position]));
            start = position;
        } else {
            position += 1;
        }
    }
    if start < text.len() {
        lines.push((&text[start..], ""));
    }
    lines
}

#[cfg(not(windows))]
fn publish_new_file(temporary: &Path, destination: &Path) -> io::Result<()> {
    fs::hard_link(temporary, destination)
}

#[cfg(windows)]
fn publish_new_file(temporary: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, destination: *const u16, flags: u32) -> i32;
    }
    let source: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
    let target: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    // MOVEFILE_WRITE_THROUGH, without REPLACE_EXISTING. Works on FAT as well as NTFS.
    let moved = unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), 0x8) };
    if moved == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
#[path = "project_search_backend_tests.rs"]
mod project_search_backend_tests;

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);

    impl Fixture {
        fn new(name: &str) -> Self {
            let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "morrow-workspace-{name}-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn write(&self, relative: &str, bytes: impl AsRef<[u8]>) -> PathBuf {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, bytes).unwrap();
            path
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn index_respects_nested_gitignore_and_generated_directories() {
        let fixture = Fixture::new("ignore");
        fixture.write(".gitignore", "ignored/\n*.log\n");
        fixture.write("src/.gitignore", "generated.rs\n");
        let source = fixture.write("src/main.rs", "fn main() {}\n");
        fixture.write("src/generated.rs", "generated");
        fixture.write("ignored/secret.txt", "secret");
        fixture.write("target/debug/cache", "cache");
        fixture.write("node_modules/package/index.js", "package");
        fixture.write("output.log", "log");
        fixture.write(".github/workflow.yml", "name: test\n");
        let index = build_index(&fixture.0, IndexOptions::default()).unwrap();
        assert!(index.files.contains(&source));
        assert!(
            index
                .files
                .contains(&fixture.0.join(".github/workflow.yml"))
        );
        assert!(!index.files.iter().any(|path| {
            path.ends_with("generated.rs")
                || path.ends_with("secret.txt")
                || path.ends_with("cache")
                || path.ends_with("index.js")
                || path.ends_with("output.log")
        }));
        let root_entries: Vec<_> = index.children(&fixture.0).collect();
        assert!(root_entries.first().unwrap().is_dir);
        assert!(!index.truncated);
    }

    #[test]
    fn showing_ignored_files_includes_nested_rules_and_generated_files_in_quick_open() {
        let fixture = Fixture::new("show-ignored");
        fixture.write(".gitignore", "ignored/\n*.log\n");
        fixture.write(".ignore", "local-only.txt\n");
        fixture.write(".git/info/exclude", "from-exclude.txt\n");
        fixture.write("src/.gitignore", "generated.rs\n");
        let ignored = [
            fixture.write("ignored/secret.txt", "ignored"),
            fixture.write("src/generated.rs", "generated"),
            fixture.write("target/debug/cache", "cache"),
            fixture.write("node_modules/package/index.js", "dependency"),
            fixture.write("dist/bundle.js", "bundle"),
            fixture.write("local-only.txt", "local"),
            fixture.write("from-exclude.txt", "local repository exclude"),
            fixture.write("output.log", "log"),
        ];
        let hidden = build_index(&fixture.0, IndexOptions::default()).unwrap();
        let shown = build_index(
            &fixture.0,
            IndexOptions {
                show_ignored_files: true,
                ..Default::default()
            },
        )
        .unwrap();
        for path in ignored {
            let query = path.file_name().unwrap().to_string_lossy();
            assert!(!hidden.files.contains(&path), "{path:?}");
            assert!(hidden.quick_open(&query, 40).is_empty(), "{path:?}");
            assert!(shown.files.contains(&path), "{path:?}");
            assert!(shown.quick_open(&query, 40).contains(&path), "{path:?}");
            assert!(
                shown
                    .children(path.parent().unwrap())
                    .any(|entry| entry.path == path)
            );
        }
        assert!(!shown.truncated);
    }

    #[test]
    fn vcs_metadata_stays_hidden_when_showing_ignored_files() {
        let fixture = Fixture::new("metadata");
        for path in [
            ".git/config",
            ".hg/store/data",
            ".svn/entries",
            "nested/.GIT/HEAD",
            "worktree/.git",
        ] {
            fixture.write(path, "metadata");
        }
        let workflow = fixture.write(".github/workflow.yml", "workflow");
        for show_ignored_files in [false, true] {
            let index = build_index(
                &fixture.0,
                IndexOptions {
                    show_ignored_files,
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(index.files, vec![workflow.clone()]);
            assert!(index.entries.iter().all(|entry| {
                ![".git", ".hg", ".svn"]
                    .iter()
                    .any(|metadata| entry.name.eq_ignore_ascii_case(metadata))
            }));
        }
    }

    #[test]
    fn rust_detection_ignores_dependency_and_ignored_manifests_but_keeps_nested_projects() {
        let fixture = Fixture::new("rust-detection");
        fixture.write("package.json", "{}");
        fixture.write(".gitignore", "ignored/\n");
        fixture.write(".ignore", "local/\n");
        fixture.write(".git/info/exclude", "private/\n");
        for manifest in [
            "node_modules/native/Cargo.toml",
            "target/nested/Cargo.toml",
            "ignored/Cargo.toml",
            "local/Cargo.toml",
            "private/Cargo.toml",
        ] {
            fixture.write(manifest, "[package]\nname = 'dependency'\n");
        }
        for show_ignored_files in [false, true] {
            let index = build_index(
                &fixture.0,
                IndexOptions {
                    show_ignored_files,
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(
                index.quick_open("Cargo.toml", 40).len(),
                if show_ignored_files { 5 } else { 0 }
            );
            assert!(!index.has_rust_manifest);
        }
        // Monorepos without a manifest at their root were already supported.
        let manifest = fixture.write(
            "crates/application/Cargo.toml",
            "[package]\nname = 'application'\n",
        );
        for show_ignored_files in [false, true] {
            let index = build_index(
                &fixture.0,
                IndexOptions {
                    show_ignored_files,
                    ..Default::default()
                },
            )
            .unwrap();
            assert!(index.has_rust_manifest);
            assert!(index.files.contains(&manifest));
        }
    }

    #[cfg(unix)]
    #[test]
    fn showing_ignored_files_never_follows_or_lists_symbolic_links() {
        let fixture = Fixture::new("links");
        let outside = Fixture::new("links-outside");
        let secret = outside.write("secret.txt", "outside");
        std::os::unix::fs::symlink(&outside.0, fixture.0.join("external")).unwrap();
        std::os::unix::fs::symlink(&secret, fixture.0.join("linked.txt")).unwrap();
        for show_ignored_files in [false, true] {
            let index = build_index(
                &fixture.0,
                IndexOptions {
                    show_ignored_files,
                    ..Default::default()
                },
            )
            .unwrap();
            assert!(index.entries.is_empty());
            assert!(index.quick_open("secret", 40).is_empty());
        }
    }

    #[test]
    fn index_is_bounded_and_can_be_cancelled() {
        let fixture = Fixture::new("bounded");
        for index in 0..30 {
            fixture.write(&format!("file-{index}.txt"), "text");
        }
        let index = build_index(
            &fixture.0,
            IndexOptions {
                show_ignored_files: true,
                max_entries: 5,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(index.entries.len(), 5);
        assert!(index.truncated);
        let cancelled = build_index(
            &fixture.0,
            IndexOptions {
                show_ignored_files: true,
                cancel: Some(Arc::new(AtomicBool::new(true))),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(cancelled.files.is_empty());
        assert!(cancelled.truncated);
    }

    #[test]
    fn quick_open_ranks_filename_before_path_and_supports_subsequences() {
        let fixture = Fixture::new("quick-open");
        let best = fixture.write("src/main.rs", "");
        fixture.write("main.rs/other.txt", "");
        fixture.write("src/menu_items.rs", "");
        let index = build_index(&fixture.0, IndexOptions::default()).unwrap();
        assert_eq!(index.quick_open("main.rs", 1), vec![best]);
        assert_eq!(index.quick_open("mirs", 10).len(), 3);
        assert!(index.quick_open("does-not-exist", 10).is_empty());
        assert!(index.quick_open("main", 0).is_empty());
    }

    #[test]
    fn quick_open_cache_preserves_limits_unicode_and_index_refreshes() {
        let fixture = Fixture::new("quick-open-cache");
        let unicode = fixture.write("src/grüße.rs", "");
        fixture.write("src/main.rs", "");
        fixture.write("src/main_test.rs", "");
        let index = build_index(&fixture.0, IndexOptions::default()).unwrap();
        assert_eq!(index.quick_open("SRC\\GRÜ", 40), vec![unicode.clone()]);
        assert_eq!(index.quick_open(" src/grü ", 40), vec![unicode]);
        assert_eq!(index.quick_open("main", 1).len(), 1);
        assert_eq!(index.quick_open("main", 40).len(), 2);
        assert!(index.quick_open("missing", 40).is_empty());
        assert!(index.quick_open("main", 0).is_empty());
        let clone = index.clone();
        assert_eq!(clone.quick_open("main", 40), index.quick_open("main", 40));
        for query in ["src", "rs", "mai", "a", "m", "test", "g", "grü", "n"] {
            index.quick_open(query, 40);
        }
        assert_eq!(index.quick_open_cache.lock().unwrap().len(), 8);

        let added = fixture.write("src/missing.rs", "");
        let refreshed = build_index(&fixture.0, IndexOptions::default()).unwrap();
        assert!(refreshed.quick_open_cache.lock().unwrap().is_empty());
        assert_eq!(refreshed.quick_open("missing", 40), vec![added]);
        assert!(index.quick_open("missing", 40).is_empty());
    }

    #[test]
    #[ignore = "Manual release-mode workspace latency benchmark"]
    fn benchmark_quick_open_and_directory_lookup() {
        use std::hint::black_box;

        let mut index = WorkspaceIndex::empty(PathBuf::from("benchmark"));
        for file in 0..100_000 {
            let relative = format!("crate_{:04}/src/module_{file:06}.rs", file / 20);
            let path = index.root.join(&relative);
            let position = index.entries.len();
            index
                .children
                .entry(path.parent().unwrap().to_path_buf())
                .or_default()
                .push(position);
            index.entries.push(WorkspaceEntry {
                name: format!("module_{file:06}.rs"),
                depth: 3,
                is_dir: false,
                path: path.clone(),
            });
            index.files.push(path);
            index.searchable_files.push(relative);
        }
        let rounds = 100;
        let started = Instant::now();
        for _ in 0..rounds {
            index.quick_open_cache.lock().unwrap().clear();
            black_box(index.quick_open("mod123", 40));
        }
        let cold = started.elapsed() / rounds;
        let started = Instant::now();
        for _ in 0..10_000 {
            black_box(index.quick_open("mod123", 40));
        }
        let cached = started.elapsed() / 10_000;
        let directory = index.root.join("crate_1000/src");
        let started = Instant::now();
        for _ in 0..10_000 {
            assert_eq!(black_box(index.children(&directory).count()), 20);
        }
        let directory_lookup = started.elapsed() / 10_000;
        println!(
            "100,000 files: cold quick-open {cold:?}, cached quick-open {cached:?}, 20-child directory lookup {directory_lookup:?}"
        );
    }

    #[test]
    fn search_handles_unicode_offsets_literal_queries_and_whole_words() {
        let fixture = Fixture::new("search");
        let path = fixture.write(
            "src/main.rs",
            "ü Grüße\nHELLO hello_world hello.\n[hello]\n",
        );
        fixture.write("binary.bin", [0, 1, 2, 3]);
        let index = build_index(&fixture.0, IndexOptions::default()).unwrap();
        let results = search_project(
            &index,
            "hello",
            SearchOptions {
                whole_word: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(results.matches.len(), 3);
        let first = &results.matches[0];
        assert_eq!(first.path, path);
        assert_eq!(
            (first.line, first.column, first.start_char, first.end_char),
            (2, 1, 8, 13)
        );
        assert_eq!(results.matches[1].column, 19);
        assert_eq!(results.files_skipped, 1);
        let literal = search_project(&index, "[hello]", SearchOptions::default()).unwrap();
        assert_eq!(literal.matches.len(), 1);
        assert_eq!(literal.matches[0].line, 3);
        let sensitive = search_project(
            &index,
            "HELLO",
            SearchOptions {
                case_sensitive: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(sensitive.matches.len(), 1);
    }

    #[test]
    fn disk_whole_word_search_keeps_unicode_identifiers_intact() {
        let fixture = Fixture::new("unicode-word-search");
        let path = fixture.write("words.txt", "git\u{301} git\u{200d} git‿ git");
        let results = search_with_excludes(
            &fixture.0,
            "git",
            SearchOptions {
                whole_word: true,
                ..Default::default()
            },
            &[],
        )
        .unwrap();
        assert_eq!(results.matches.len(), 1);
        let found = &results.matches[0];
        assert_eq!(found.path, fs::canonicalize(path).unwrap());
        assert_eq!(
            (found.line, found.column, found.start_char, found.end_char),
            (1, 16, 15, 18)
        );
    }

    #[test]
    fn search_counts_multiline_matches_and_limits_results() {
        let fixture = Fixture::new("search-limits");
        fixture.write("notes.txt", "aa\nbb\naa\nbb\n");
        let index = build_index(&fixture.0, IndexOptions::default()).unwrap();
        let multiline = search_project(&index, "aa\nbb", SearchOptions::default()).unwrap();
        assert_eq!(multiline.matches.len(), 2);
        assert_eq!(
            (multiline.matches[1].line, multiline.matches[1].start_char),
            (3, 6)
        );
        let limited = search_project(
            &index,
            "a",
            SearchOptions {
                max_matches: 1,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(limited.matches.len(), 1);
        assert!(limited.truncated);
    }

    #[test]
    fn loading_rejects_large_binary_and_invalid_utf8_files() {
        let fixture = Fixture::new("validation");
        let too_big = fixture.write("large.txt", "12345");
        let binary = fixture.write("binary.dat", [0, 65]);
        let invalid = fixture.write("invalid.txt", [0xff, 0xfe]);
        assert!(
            read_document(&too_big, 4)
                .unwrap_err()
                .contains("too large")
        );
        assert!(read_document(&binary, 100).unwrap_err().contains("binary"));
        assert!(read_document(&invalid, 100).unwrap_err().contains("UTF-8"));
    }

    #[test]
    fn atomic_save_preserves_crlf_and_bom_and_detects_external_edits() {
        let fixture = Fixture::new("save");
        let path = fixture.write("document.txt", b"\xef\xbb\xbfhello\r\nworld\r\n");
        let document = read_document(&path, MAX_DOCUMENT_BYTES).unwrap();
        assert_eq!(document.text, "hello\nworld\n");
        assert_eq!(document.line_ending, LineEnding::CrLf);
        assert!(document.bom);
        let saved = save_document(
            &path,
            "hello\nedited\n",
            document.line_ending,
            document.bom,
            Some(&document.fingerprint),
        )
        .unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"\xef\xbb\xbfhello\r\nedited\r\n");
        fs::write(&path, "external edit").unwrap();
        assert!(matches!(
            save_document(&path, "my edit", LineEnding::Lf, false, Some(&saved)),
            Err(SaveError::Conflict(_))
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), "external edit");
        assert!(!fs::read_dir(&fixture.0).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".morrow-save")
        }));
    }

    #[test]
    fn save_as_never_overwrites_existing_files_and_deleted_files_conflict() {
        let fixture = Fixture::new("save-as");
        let path = fixture.0.join("new.txt");
        let fingerprint = save_document(&path, "draft", LineEnding::Lf, false, None).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "draft");
        assert!(matches!(
            save_document(&path, "replace", LineEnding::Lf, false, None),
            Err(SaveError::Conflict(_))
        ));
        fs::remove_file(&path).unwrap();
        assert!(matches!(
            save_document(&path, "replace", LineEnding::Lf, false, Some(&fingerprint)),
            Err(SaveError::Conflict(_))
        ));
        assert!(!path.exists());
    }

    #[test]
    fn mixed_endings_survive_edits_and_insertions() {
        let fixture = Fixture::new("mixed-endings");
        let path = fixture.write("mixed.txt", "one\r\ntwo\nthree\r\nfour\r");
        let original = read_document(&path, MAX_DOCUMENT_BYTES).unwrap();
        assert_eq!(original.line_ending, LineEnding::Mixed);
        assert!(original.mixed_line_endings);
        let saved = save_document(
            &path,
            "one\ntwo edited\nthree\nfour\n",
            LineEnding::Mixed,
            false,
            Some(&original.fingerprint),
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "one\r\ntwo edited\nthree\r\nfour\r"
        );
        save_document(
            &path,
            "one\ninserted\ntwo edited\nthree\nfour\n",
            LineEnding::Mixed,
            false,
            Some(&saved),
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "one\r\ninserted\r\ntwo edited\nthree\r\nfour\r"
        );
    }

    #[test]
    fn same_size_external_edits_are_not_overwritten() {
        let fixture = Fixture::new("same-size-conflict");
        let path = fixture.write("source.rs", "original");
        let original = read_document(&path, MAX_DOCUMENT_BYTES).unwrap();
        fs::write(&path, "external").unwrap();
        assert!(matches!(
            save_document(
                &path,
                "my edits",
                LineEnding::Lf,
                false,
                Some(&original.fingerprint)
            ),
            Err(SaveError::Conflict(_))
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), "external");
    }

    #[test]
    fn read_only_file_is_left_intact() {
        let fixture = Fixture::new("read-only");
        let path = fixture.write("locked.txt", "original");
        let original = read_document(&path, MAX_DOCUMENT_BYTES).unwrap();
        let original_permissions = fs::metadata(&path).unwrap().permissions();
        let mut locked_permissions = original_permissions.clone();
        locked_permissions.set_readonly(true);
        fs::set_permissions(&path, locked_permissions).unwrap();
        let result = save_document(
            &path,
            "modified",
            LineEnding::Lf,
            false,
            Some(&original.fingerprint),
        );
        let content = fs::read_to_string(&path).unwrap();
        fs::set_permissions(&path, original_permissions).unwrap();
        assert!(result.unwrap_err().to_string().contains("read-only"));
        assert_eq!(content, "original");
    }

    #[test]
    fn depth_limit_is_reported_without_following_deeper_folders() {
        let fixture = Fixture::new("depth");
        fixture.write("one/two/three/source.rs", "source");
        let index = build_index(
            &fixture.0,
            IndexOptions {
                max_depth: 2,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(index.truncated);
        assert!(index.files.is_empty());
        assert!(index.entries.iter().all(|entry| entry.depth <= 2));
    }
}

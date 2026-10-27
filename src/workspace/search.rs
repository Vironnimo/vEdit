//! Bounded project searches over current Documents and disk, independent of Quick Open pruning.
#[cfg(test)]
use super::WorkspaceIndex;
use super::{MAX_WARNINGS, cancelled, decode_document, read_bytes};
use crate::cli::normalize_search_excludes;
use ignore::{
    WalkBuilder,
    gitignore::{Gitignore, GitignoreBuilder},
};
use regex::RegexBuilder;
use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

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
pub struct SearchBuffer {
    pub path: Option<PathBuf>,
    pub document: SearchDocument,
    /// None suppresses disk fallback for a buffer beyond the snapshot budget.
    pub text: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchDocument {
    pub id: u64,
    pub revision: u64,
    pub title: String,
    pub unsaved: bool,
}

#[derive(Clone, Debug)]
pub struct SearchMatch {
    pub path: Option<PathBuf>,
    pub document: Option<SearchDocument>,
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

/// Optional project-search filters. The Quick Open index has independent
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
    .map(|folder| format!("{folder}/"))
    .collect()
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
#[cfg(test)]
pub fn search_with_excludes(
    root: &Path,
    query: &str,
    options: SearchOptions,
    excludes: &[String],
) -> Result<SearchResults, String> {
    search_with_buffers(root, query, options, excludes, Vec::new())
}

pub fn search_with_buffers(
    root: &Path,
    query: &str,
    options: SearchOptions,
    excludes: &[String],
    buffers: Vec<SearchBuffer>,
) -> Result<SearchResults, String> {
    let started = Instant::now();
    let root = fs::canonicalize(root)
        .map_err(|error| format!("Could not open search project: {error}"))?;
    if !root.is_dir() {
        return Err("The search project must be a folder.".into());
    }
    let matcher = search_exclusion_matcher(&root, excludes)?;
    let buffer_matcher = matcher.clone();
    let cancel = options.cancel.clone();
    let duration = options.max_duration;
    let count = Arc::new(AtomicUsize::new(0));
    let bounded = Arc::new(AtomicBool::new(false));
    let bounded_walk = bounded.clone();
    let mut walker = WalkBuilder::new(&root);
    walker
        // Keep test fixtures independent of ancestor checkout ignore files.
        .parents(!cfg!(test))
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
            if [".git", ".hg", ".svn"].iter().any(|metadata| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(metadata)
            }) {
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
    let mut overlays = HashMap::new();
    let mut unnamed = Vec::new();
    for buffer in buffers {
        if cancelled(&options.cancel) || started.elapsed() >= options.max_duration {
            bounded.store(true, Ordering::Relaxed);
            break;
        }
        if let Some(path) = &buffer.path {
            if fs::symlink_metadata(path)
                .is_err_and(|error| error.kind() == io::ErrorKind::NotFound)
                && missing_buffer_allowed(&root, path, &buffer_matcher)
            {
                unnamed.push(Ok(SearchInput::Buffer(buffer)));
            } else {
                overlays.insert(search_path_key(path), buffer);
            }
        } else {
            unnamed.push(Ok(SearchInput::Buffer(buffer)));
        }
    }
    let inputs = paths
        .map(|path| {
            path.map(|path| {
                overlays
                    .remove(&search_path_key(&path))
                    .map_or_else(|| SearchInput::Disk(path), SearchInput::Buffer)
            })
        })
        .chain(unnamed);
    let mut result = search_inputs(inputs, query, options, started)?;
    result.truncated |= bounded.load(Ordering::Relaxed);
    Ok(result)
}

/// Missing named drafts have no walk entry. Apply the same path/ignore policy
/// without creating files or following links to discover their parent folders.
fn missing_buffer_allowed(root: &Path, path: &Path, excludes: &Gitignore) -> bool {
    let displayed_root = PathBuf::from(crate::paths::display(root));
    let displayed_path = PathBuf::from(crate::paths::display(path));
    let mut components = displayed_path.components();
    for root_component in displayed_root.components() {
        let Some(component) = components.next() else {
            return false;
        };
        if search_path_key(Path::new(component.as_os_str()))
            != search_path_key(Path::new(root_component.as_os_str()))
        {
            return false;
        }
    }
    // Preserve filename case for case-sensitive ignore patterns, even on Windows.
    let relative = components.as_path();
    if relative.components().count() > 64
        || relative.components().any(|component| {
            !matches!(component, std::path::Component::Normal(_))
                || [".git", ".hg", ".svn"].iter().any(|name| {
                    component
                        .as_os_str()
                        .to_string_lossy()
                        .eq_ignore_ascii_case(name)
                })
        })
    {
        return false;
    }
    let path = root.join(relative);
    if excludes
        .matched_path_or_any_parents(&path, false)
        .is_ignore()
    {
        return false;
    }
    let mut directories: Vec<_> = path
        .parent()
        .unwrap()
        .ancestors()
        .take_while(|dir| dir.starts_with(root))
        .collect();
    directories.reverse();
    for directory in &directories {
        match fs::symlink_metadata(directory) {
            Ok(metadata) if metadata.file_type().is_symlink() => return false,
            Err(error) if error.kind() != io::ErrorKind::NotFound => return false,
            _ => {}
        }
    }
    // .ignore takes precedence over Git rules; the nearest matching file wins.
    let mut rules: Vec<(bool, Gitignore)> = Vec::new();
    let ignored = |rules: &[(bool, Gitignore)], path: &Path, directory: bool| {
        let mut git_ignored = false;
        let mut ignore = None;
        for (git, matcher) in rules {
            let matched = matcher.matched_path_or_any_parents(path, directory);
            if !matched.is_none() {
                if *git {
                    git_ignored = matched.is_ignore();
                } else {
                    ignore = Some(matched.is_ignore());
                }
            }
        }
        ignore.unwrap_or(git_ignored)
    };
    let mut git_directories = directories.clone();
    if !cfg!(test) {
        git_directories.extend(root.ancestors().skip(1));
        git_directories.sort_by_key(|directory| directory.components().count());
    }
    for directory in git_directories {
        if directory != root && directory.starts_with(root) && ignored(&rules, directory, true) {
            return false;
        }
        for (name, git) in [
            (".git/info/exclude", true),
            (".gitignore", true),
            (".ignore", false),
        ] {
            let file = directory.join(name);
            if !file.is_file() {
                continue;
            }
            let mut builder = GitignoreBuilder::new(directory);
            if builder.add(file).is_some() {
                return false;
            }
            let Ok(matcher) = builder.build() else {
                return false;
            };
            rules.push((git, matcher));
        }
    }
    !ignored(&rules, &path, false)
}

fn search_path_key(path: &Path) -> String {
    let path = crate::paths::display(path).replace('\\', "/");
    if cfg!(windows) {
        path.to_lowercase()
    } else {
        path
    }
}

enum SearchInput {
    Disk(PathBuf),
    Buffer(SearchBuffer),
}

/// Literal project search; punctuation in a query is never interpreted as a regex.
#[cfg(test)]
fn search_project(
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

#[cfg(test)]
fn search_paths(
    paths: impl Iterator<Item = Result<PathBuf, String>>,
    query: &str,
    options: SearchOptions,
    started: Instant,
) -> Result<SearchResults, String> {
    search_inputs(
        paths.map(|path| path.map(SearchInput::Disk)),
        query,
        options,
        started,
    )
}

fn search_inputs(
    inputs: impl Iterator<Item = Result<SearchInput, String>>,
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
    'files: for input in inputs {
        if cancelled(&options.cancel)
            || started.elapsed() >= options.max_duration
            || results.matches.len() >= options.max_matches
            || bytes_searched >= options.max_total_bytes
        {
            results.truncated = true;
            break;
        }
        let input = match input {
            Ok(input) => input,
            Err(error) => {
                if results.warnings.len() < MAX_WARNINGS {
                    results.warnings.push(error);
                }
                continue;
            }
        };
        let remaining = options.max_total_bytes.saturating_sub(bytes_searched);
        let limit = remaining.min(options.max_file_bytes);
        let (path, document, text) = match input {
            SearchInput::Buffer(buffer) => {
                let Some(text) = buffer.text.filter(|text| text.len() as u64 <= limit) else {
                    results.files_skipped += 1;
                    continue;
                };
                bytes_searched += text.len() as u64;
                (buffer.path, Some(buffer.document), text)
            }
            SearchInput::Disk(path) => {
                let (bytes, fingerprint) = match read_bytes(&path, limit) {
                    Ok(snapshot) => snapshot,
                    Err(error) => {
                        results.files_skipped += 1;
                        if error.kind() != io::ErrorKind::InvalidInput
                            && results.warnings.len() < MAX_WARNINGS
                        {
                            results.warnings.push(format!(
                                "Could not open {}: {error}",
                                crate::paths::display(&path)
                            ));
                        }
                        continue;
                    }
                };
                // Reading a binary or malformed text file still consumes the search
                // budget. Charge the snapshot before decoding it, so repositories full
                // of assets cannot bypass the advertised bound.
                bytes_searched += fingerprint.len;
                let document = match decode_document(&path, bytes, fingerprint) {
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
                (Some(path), None, document.text)
            }
        };
        results.files_searched += 1;
        let text = text.as_str();
        let mut last_byte = 0;
        let mut char_offset = 0;
        let mut line = 1;
        let mut line_start = 0;
        let mut line_char_start = 0;
        let mut line_end = text.find('\n').unwrap_or(text.len());
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
            for (position, _) in preceding.match_indices('\n') {
                line += 1;
                line_start = last_byte + position + 1;
            }
            if line_start > last_byte {
                line_char_start = char_offset - text[line_start..found.start()].chars().count();
            }
            if found.start() > line_end {
                line_end = text[found.start()..]
                    .find('\n')
                    .map_or(text.len(), |end| found.start() + end);
            }
            let start_char = char_offset;
            let match_chars = found.as_str().chars().count();
            let preview = line_preview(&text[line_start..line_end], found.start() - line_start);
            results.matches.push(SearchMatch {
                path: path.clone(),
                document: document.clone(),
                line,
                column: start_char - line_char_start + 1,
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
    // Locate the short context window from the match, rather than rescanning
    // a potentially megabyte-long minified line for every result.
    let start = line[..match_byte]
        .char_indices()
        .rev()
        .nth(79)
        .map_or(0, |(byte, _)| byte);
    let mut chars = line[start..].chars();
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

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../project_search_backend_tests.rs"]
mod project_search_backend_tests;

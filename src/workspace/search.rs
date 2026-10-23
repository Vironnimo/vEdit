//! Fresh, bounded project disk searches, independent of Quick Open pruning.
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
    let mut result = search_paths(paths, query, options, started)?;
    result.truncated |= bounded.load(Ordering::Relaxed);
    Ok(result)
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
        results.files_searched += 1;
        let text = &document.text;
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

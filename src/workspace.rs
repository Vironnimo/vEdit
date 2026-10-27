//! Filesystem work for background workers, with bounded indexing and cancellation.
//!
//! The immutable Quick Open index lives here. Private units own complete lazy
//! Explorer listings, bounded project search, and format-preserving document I/O.
//! Callers continue to use this module's filesystem API; none needs the UI thread.

mod documents;
mod explorer;
mod search;

pub use documents::{
    FileFingerprint, LineEnding, LoadedDocument, SaveError, SavedDocument, fingerprint,
    read_document, remove_stale_save_temporaries, save_document,
};
pub(crate) use documents::{decode_document, read_bytes};
pub use explorer::{ExplorerDirectory, ExplorerTree, read_explorer_directory};
#[cfg(test)]
pub use search::SearchMatch;
pub use search::{
    SearchBuffer, SearchDocument, SearchOptions, SearchResults, default_search_excludes,
    search_with_buffers,
};

use ignore::WalkBuilder;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const MAX_DOCUMENT_BYTES: u64 = 8 * 1024 * 1024;
const MAX_WARNINGS: usize = 20;

#[derive(Clone, Debug)]
pub struct IndexOptions {
    /// Internal Quick Open indexing policy. The Explorer always uses complete
    /// lazy directory listings and never consults this option.
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
    pub ignored: bool,
}

#[derive(Clone, Debug)]
pub struct WorkspaceIndex {
    pub root: PathBuf,
    pub entries: Vec<WorkspaceEntry>,
    pub files: Vec<PathBuf>,
    /// A Cargo manifest admitted by the default analysis policy, independent
    /// of whether ignored and generated files are indexed for Quick Open.
    pub has_rust_manifest: bool,
    pub truncated: bool,
    pub warnings: Vec<String>,
    children: BTreeMap<PathBuf, Vec<usize>>,
    searchable_files: Vec<String>,
    quick_open_cache: Arc<Mutex<VecDeque<QuickOpenResults>>>,
    explorer_directories: BTreeMap<PathBuf, Arc<ExplorerDirectory>>,
    explorer_snapshot: bool,
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
            explorer_directories: BTreeMap::new(),
            explorer_snapshot: false,
        }
    }

    /// Give an eager rendering fixture a stable logical location, without
    /// reading or modifying that location on disk.
    #[cfg(test)]
    pub fn at_fixture_root(mut self, root: PathBuf) -> Self {
        assert!(!self.explorer_snapshot);
        let relocate = |path: &Path| root.join(path.strip_prefix(&self.root).unwrap());
        for entry in &mut self.entries {
            entry.path = relocate(&entry.path);
        }
        for path in &mut self.files {
            *path = relocate(path);
        }
        self.children = self
            .children
            .into_iter()
            .map(|(path, children)| (relocate(&path), children))
            .collect();
        self.root = root;
        self.quick_open_cache = Arc::new(Mutex::new(VecDeque::new()));
        self
    }

    /// A cached directory listing, sorted with folders first. Does not touch the disk.
    pub fn children<'a>(&'a self, parent: &Path) -> impl Iterator<Item = &'a WorkspaceEntry> {
        self.explorer_directories
            .get(parent)
            .into_iter()
            .flat_map(|directory| directory.entries.iter())
            .chain(
                self.children
                    .get(parent)
                    .into_iter()
                    .flatten()
                    .map(|&index| &self.entries[index]),
            )
    }

    pub fn entry(&self, path: &Path) -> Option<&WorkspaceEntry> {
        self.children(path.parent()?)
            .find(|entry| entry.path == path)
    }

    pub fn has_entry(&self, path: &Path) -> bool {
        self.entry(path).is_some()
    }

    #[cfg(test)]
    pub fn all_entries(&self) -> impl Iterator<Item = &WorkspaceEntry> {
        self.entries.iter().chain(
            self.explorer_directories
                .values()
                .flat_map(|directory| directory.entries.iter()),
        )
    }

    pub fn is_explorer_snapshot(&self) -> bool {
        self.explorer_snapshot
    }

    /// An absent row in an unloaded lazy directory is not proof that a selected
    /// file disappeared. Eager indexes retain their existing reconciliation.
    pub fn directory_loaded(&self, directory: &Path) -> bool {
        !self.explorer_snapshot || self.explorer_directories.contains_key(directory)
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

/// Rank a path and query normalized to lowercase with `/` separators.
pub(crate) fn path_score(path: &str, query: &str) -> Option<i64> {
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

pub(crate) fn generated_index_name(name: &std::ffi::OsStr) -> bool {
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
        // Keep test fixtures independent of ancestor checkout ignore files.
        .parents(!cfg!(test))
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
            show_ignored_files
                || !entry.file_type().is_some_and(|kind| kind.is_dir())
                || !generated_index_name(entry.file_name())
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
            ignored: false,
        });
    }
    index.entries.sort_by_cached_key(|entry| {
        (
            entry.path.parent().map(Path::to_path_buf),
            Reverse(entry.is_dir),
            entry.name.to_lowercase(),
            entry.path.clone(),
        )
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
    index.files.sort_by_cached_key(|path| {
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

#[cfg(test)]
#[path = "workspace/test_support.rs"]
mod test_support;

#[cfg(test)]
#[path = "workspace/index_tests.rs"]
mod tests;

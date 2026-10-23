//! Complete directory listings and publication of lazy Explorer snapshots.
use super::{MAX_WARNINGS, WorkspaceEntry, WorkspaceIndex};
use ignore::WalkBuilder;
use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

static EXPLORER_GENERATION: AtomicU64 = AtomicU64::new(1);

/// A complete, nonrecursive folder listing for the Explorer. Git ignore rules,
/// generated-folder policies, search limits and maximum tree depth never hide
/// ordinary entries here. Only version-control metadata stays hidden.
#[derive(Clone, Debug)]
pub struct ExplorerDirectory {
    pub root: PathBuf,
    pub path: PathBuf,
    pub entries: Vec<WorkspaceEntry>,
    pub warnings: Vec<String>,
}

/// Enumerate one folder on a background worker. Never call this from rendering.
/// User-expanded linked folders are followed, including external targets. This
/// remains a single directory read, so a link never triggers recursive scanning.
pub fn read_explorer_directory(root: &Path, directory: &Path) -> Result<ExplorerDirectory, String> {
    let relative = directory
        .strip_prefix(root)
        .map_err(|_| "The Explorer folder is outside this project".to_owned())?;
    if relative.components().any(|part| {
        matches!(
            part,
            std::path::Component::ParentDir
                | std::path::Component::RootDir
                | std::path::Component::Prefix(_)
        )
    }) {
        return Err("The Explorer folder is outside this project".into());
    }
    let reader = fs::read_dir(directory).map_err(|error| {
        format!(
            "Could not list {}: {error}",
            crate::paths::display(directory)
        )
    })?;
    let mut listing = ExplorerDirectory {
        root: root.to_path_buf(),
        path: directory.to_path_buf(),
        entries: Vec::new(),
        warnings: Vec::new(),
    };
    // Evaluate Git rules without filtering or recursively walking the tree.
    // A fresh matcher observes edits on refresh; ancestors are cached within
    // this listing, including the inherited status of ignored directories.
    let mut ignored = WalkBuilder::new(root)
        // Cargo's Git ceiling does not affect the ignore crate. Test fixtures
        // must not inherit the checkout's .tmp exclusion; shipped builds do.
        .parents(!cfg!(test))
        .hidden(false)
        .ignore(false)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .require_git(false)
        .build_matchers()
        .pop()
        .expect("one Explorer root has one ignore matcher");
    for result in reader {
        let entry = match result {
            Ok(entry) => entry,
            Err(error) => {
                if listing.warnings.len() < MAX_WARNINGS {
                    listing.warnings.push(error.to_string());
                }
                continue;
            }
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        if [".git", ".hg", ".svn"]
            .iter()
            .any(|metadata| name.eq_ignore_ascii_case(metadata))
        {
            continue;
        }
        let path = entry.path();
        let is_dir = match entry.file_type() {
            Ok(kind) => kind.is_dir() || (kind.is_symlink() && path.is_dir()),
            Err(error) => {
                // A metadata error must not silently hide an otherwise named
                // entry. Keep it visible and report the failed inspection.
                if listing.warnings.len() < MAX_WARNINGS {
                    listing.warnings.push(format!(
                        "Could not inspect {}: {error}",
                        crate::paths::display(&path)
                    ));
                }
                false
            }
        };
        let (matched, error) = ignored.matched_with_errors(
            path.strip_prefix(root)
                .expect("entry belongs to the listed folder"),
            is_dir,
        );
        if let Some(error) = error
            && listing.warnings.len() < MAX_WARNINGS
        {
            listing.warnings.push(error.to_string());
        }
        listing.entries.push(WorkspaceEntry {
            path,
            name,
            is_dir,
            ignored: matched.is_ignore(),
        });
    }
    // Cache allocating keys once per entry, rather than once per comparison.
    listing.entries.sort_by_cached_key(|entry| {
        (
            Reverse(entry.is_dir),
            entry.name.to_lowercase(),
            entry.path.clone(),
        )
    });
    Ok(listing)
}

/// A lazy Explorer cache, independent of the bounded Quick Open index.
/// Snapshots share immutable directory arrays, so loading one child never copies
/// the entries of a large root folder. `snapshot().children(path)` is compatible
/// with existing Explorer navigation; flat `entries`/`files` stay unused here.
#[derive(Debug)]
pub struct ExplorerTree {
    snapshot: Arc<WorkspaceIndex>,
    loaded: BTreeSet<PathBuf>,
    /// Each in-flight read carries a unique token. Refreshing a folder while
    /// it is read orphans the old token, so a listing taken before the change
    /// can never replace a newer one.
    pending: BTreeMap<PathBuf, u64>,
    errors: BTreeMap<PathBuf, String>,
}

impl ExplorerTree {
    pub fn new(root: PathBuf) -> Self {
        let mut snapshot = WorkspaceIndex::empty(root);
        snapshot.explorer_snapshot = true;
        Self {
            snapshot: Arc::new(snapshot),
            loaded: BTreeSet::new(),
            pending: BTreeMap::new(),
            errors: BTreeMap::new(),
        }
    }

    pub fn snapshot(&self) -> Arc<WorkspaceIndex> {
        self.snapshot.clone()
    }

    /// Reserve a worker request. Duplicate, cached and failed requests wait
    /// until refresh; return this token with the worker completion.
    pub fn begin(&mut self, directory: &Path) -> Option<u64> {
        if !directory.starts_with(&self.snapshot.root)
            || self.loaded.contains(directory)
            || self.pending.contains_key(directory)
            || self.errors.contains_key(directory)
        {
            return None;
        }
        let token = EXPLORER_GENERATION.fetch_add(1, Ordering::Relaxed);
        self.pending.insert(directory.to_path_buf(), token);
        Some(token)
    }

    /// Publish only the latest reserved request for this folder.
    pub fn complete(
        &mut self,
        directory: &Path,
        token: u64,
        result: Result<ExplorerDirectory, String>,
    ) -> bool {
        if self.pending.get(directory) != Some(&token) {
            return false;
        }
        self.pending.remove(directory);
        match result {
            Ok(listing) if listing.root == self.snapshot.root && listing.path == directory => {
                // Navigation remembers snapshot identity while awaiting a
                // rename refresh. Publish a distinct allocation even when no
                // renderer currently holds the previous Arc. Directory arrays
                // remain shared, so this only clones the small directory map.
                let mut snapshot = (*self.snapshot).clone();
                snapshot
                    .explorer_directories
                    .insert(directory.to_path_buf(), Arc::new(listing));
                snapshot.warnings = snapshot
                    .explorer_directories
                    .values()
                    .flat_map(|listing| listing.warnings.iter().cloned())
                    .take(MAX_WARNINGS)
                    .collect();
                self.snapshot = Arc::new(snapshot);
                self.loaded.insert(directory.to_path_buf());
                self.errors.remove(directory);
            }
            Ok(_) => {
                self.errors.insert(
                    directory.to_path_buf(),
                    "The folder listing belongs to a different project or folder".into(),
                );
            }
            Err(error) => {
                self.errors.insert(directory.to_path_buf(), error);
            }
        }
        true
    }

    /// Keep previous rows visible while fresh requests run. Late workers from
    /// the previous generation cannot replace the refreshed directory cache.
    pub fn invalidate(&mut self) {
        self.loaded.clear();
        self.pending.clear();
        self.errors.clear();
    }

    /// Read these folders again when they are next shown, keeping their
    /// current rows meanwhile. A read already in flight may predate the
    /// change, so its result is discarded.
    pub fn refresh(&mut self, directories: &BTreeSet<PathBuf>) {
        for directory in directories {
            self.loaded.remove(directory);
            self.pending.remove(directory);
            self.errors.remove(directory);
        }
    }

    /// Drop listed folders that no longer exist from their parent's rows and
    /// forget their own listings, so an expanded folder that was deleted does
    /// not briefly report itself as unavailable. The parent is read again.
    pub fn forget(&mut self, removed: &BTreeSet<PathBuf>) {
        if removed.is_empty() {
            return;
        }
        let mut snapshot = (*self.snapshot).clone();
        for path in removed {
            snapshot
                .explorer_directories
                .retain(|directory, _| !directory.starts_with(path));
            self.loaded.retain(|directory| !directory.starts_with(path));
            self.pending
                .retain(|directory, _| !directory.starts_with(path));
            self.errors
                .retain(|directory, _| !directory.starts_with(path));
            if let Some(parent) = path.parent()
                && let Some(listing) = snapshot.explorer_directories.get_mut(parent)
                && listing.entries.iter().any(|entry| entry.path == *path)
            {
                let listing = Arc::make_mut(listing);
                listing.entries.retain(|entry| entry.path != *path);
                self.loaded.remove(parent);
                self.pending.remove(parent);
            }
        }
        self.snapshot = Arc::new(snapshot);
    }

    /// Folders that currently have a listing, loaded or awaiting a refresh.
    pub fn listed(&self) -> impl Iterator<Item = &Path> {
        self.snapshot
            .explorer_directories
            .keys()
            .map(PathBuf::as_path)
    }

    pub fn has_listing(&self, directory: &Path) -> bool {
        self.snapshot.explorer_directories.contains_key(directory)
    }

    #[cfg(test)]
    pub fn is_loaded(&self, directory: &Path) -> bool {
        self.loaded.contains(directory)
    }

    pub fn is_loading(&self, directory: &Path) -> bool {
        self.pending.contains_key(directory)
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    pub fn error(&self, directory: &Path) -> Option<&str> {
        self.errors.get(directory).map(String::as_str)
    }

    pub fn errors(&self) -> impl Iterator<Item = (&Path, &str)> {
        self.errors
            .iter()
            .map(|(path, error)| (path.as_path(), error.as_str()))
    }
}

#[cfg(test)]
#[path = "explorer_tests.rs"]
mod tests;

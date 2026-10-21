//! A bounded history of files and folders the user has used, independent of open tabs.
use super::*;

const LIMIT: usize = 30;
type Origin = Option<(u64, u64, Option<u64>)>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Kind {
    File,
    Folder,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Entry {
    pub path: PathBuf,
    pub kind: Kind,
}

pub(super) struct OpenResult {
    request: u64,
    origin: Origin,
    entry: Entry,
    result: Result<PathBuf, String>,
}

impl VEditApp {
    pub(super) fn remember_recent(&mut self, path: PathBuf, kind: Kind) {
        if !path.is_absolute() || path.as_os_str().is_empty() {
            return;
        }
        if self
            .recent
            .first()
            .is_some_and(|entry| entry.kind == kind && control::same_workspace(&entry.path, &path))
        {
            return;
        }
        self.recent
            .retain(|entry| !control::same_workspace(&entry.path, &path));
        self.recent.insert(0, Entry { path, kind });
        self.recent.truncate(LIMIT);
        self.session_dirty = true;
    }

    pub(super) fn restore_recent(&mut self, entries: Vec<Entry>) {
        self.recent.clear();
        // Normalize old or edited session data without probing disks/network shares.
        for entry in entries.into_iter().take(LIMIT).rev() {
            self.remember_recent(entry.path, entry.kind);
        }
    }

    pub(super) fn remember_recent_document(&mut self, id: u64) {
        let path = self
            .projects
            .iter()
            .flat_map(|project| &project.tabs)
            .find(|tab| tab.id == id)
            .filter(|tab| !tab.loading && tab.error.is_none())
            .and_then(|tab| tab.path.clone());
        if let Some(path) = path {
            self.remember_recent(path, Kind::File);
        }
    }

    pub(super) fn matching_recent(&self, query: &str) -> Vec<Entry> {
        let terms: Vec<_> = query.split_whitespace().map(str::to_lowercase).collect();
        self.recent
            .iter()
            .filter(|entry| {
                let path = crate::paths::display(&entry.path).to_lowercase();
                terms.iter().all(|term| path.contains(term))
            })
            .cloned()
            .collect()
    }

    pub(super) fn clear_recent(&mut self) {
        self.recent.clear();
        self.recent_opening.clear();
        self.recent_open_request = self.recent_open_request.wrapping_add(1);
        self.session_dirty = true;
        self.status("Recent history cleared", false);
    }

    fn recent_origin(&self) -> Origin {
        self.project().map(|project| {
            (
                project.instance_id,
                self.current_pane_id(),
                self.tab().map(|tab| tab.id),
            )
        })
    }

    pub(super) fn open_recent(&mut self, entry: Entry) {
        self.recent_open_request = self.recent_open_request.wrapping_add(1);
        // An existing unsaved document remains usable even if its disk file vanished.
        if entry.kind == Kind::File && self.focus_open_file(&entry.path, false) {
            return;
        }
        if entry.kind == Kind::Folder
            && let Some(index) = self.projects.iter().position(|project| {
                project
                    .folder_root()
                    .is_some_and(|root| control::same_workspace(root, &entry.path))
            })
        {
            self.switch_project(index);
            self.remember_recent(entry.path, entry.kind);
            return;
        }
        let request = self.recent_open_request;
        let origin = self.recent_origin();
        self.status(
            format!("Opening {}…", crate::paths::display(&entry.path)),
            false,
        );
        self.worker(move || {
            let result = (|| {
                let path = fs::canonicalize(&entry.path).map_err(|error| error.to_string())?;
                let metadata = fs::metadata(&path).map_err(|error| error.to_string())?;
                match entry.kind {
                    Kind::File if metadata.is_file() => Ok(path),
                    Kind::Folder if metadata.is_dir() => Ok(path),
                    Kind::File => Err("This path is no longer a file".into()),
                    Kind::Folder => Err("This path is no longer a folder".into()),
                }
            })();
            WorkerResult::RecentOpened(OpenResult {
                request,
                origin,
                entry,
                result,
            })
        });
    }

    pub(super) fn finish_recent_open(&mut self, opened: OpenResult) {
        if opened.request != self.recent_open_request {
            return;
        }
        if opened.origin != self.recent_origin() || self.chrome_blocked() {
            self.status("Open recent cancelled because the active view changed; choose the item again to open it.", false);
            return;
        }
        match opened.result {
            Ok(path) => {
                self.status = None;
                match opened.entry.kind {
                    Kind::Folder => self.add_resolved_project(path),
                    Kind::File => {
                        let index = self.file_workspace(&path);
                        self.switch_project(index);
                        self.open_resolved_file(path, false, true);
                    }
                }
            }
            Err(error) => self.status(
                format!(
                    "Could not reopen {}: {error}",
                    crate::paths::display(&opened.entry.path)
                ),
                true,
            ),
        }
    }
}

#[cfg(test)]
#[path = "recent_tests.rs"]
mod tests;

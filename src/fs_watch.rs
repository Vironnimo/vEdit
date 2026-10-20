//! Follows names appearing, disappearing and moving below a project root, so
//! the Explorer and Quick Open keep up with agents working in a terminal.
//!
//! One recursive ReadDirectoryChangesW watch per project costs no CPU while
//! nothing changes. The notification thread only classifies and records paths;
//! the UI drains a coalesced batch after a short quiet period and does all
//! directory reads on workers. Content writes never wake the window, and
//! churn in `.git` internals or generated folders (`target`, `node_modules`
//! and similar) is dropped unless the Explorer currently lists that folder.
use notify::{
    EventKind, RecursiveMode, Watcher,
    event::{ModifyKind, RenameMode},
};
use std::{
    collections::{BTreeSet, HashSet},
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// A batch is published once no change arrived for this long…
pub const QUIET: Duration = Duration::from_millis(200);
/// …or at the latest after this long, so steady churn cannot starve it.
pub const MAX_DELAY: Duration = Duration::from_millis(1000);

type Wake = Box<dyn Fn(Duration) + Send + Sync>;

/// What a drained batch asks the Explorer and Quick Open to refresh.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Changes {
    /// Listed folders whose names changed; re-read them in the background.
    pub directories: BTreeSet<PathBuf>,
    /// Listed folders that were removed or renamed away.
    pub removed: BTreeSet<PathBuf>,
    /// The bounded Quick Open index no longer matches the disk.
    pub index: bool,
    /// Ignore rules changed, or the platform lost events: re-read everything
    /// that is listed.
    pub rescan: bool,
}

impl Changes {
    fn is_empty(&self) -> bool {
        self.directories.is_empty() && self.removed.is_empty() && !self.index && !self.rescan
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Poll {
    Idle,
    Wait(Duration),
    Ready(Changes),
}

#[derive(Default)]
struct State {
    /// Folders the Explorer has listed; only their names matter to it.
    listed: HashSet<PathBuf>,
    changes: Changes,
    first: Option<Instant>,
    last: Option<Instant>,
}

struct Shared {
    root: PathBuf,
    state: Mutex<State>,
    wake: Wake,
}

pub struct ProjectWatcher {
    _watcher: notify::RecommendedWatcher,
    shared: Arc<Shared>,
}

impl std::fmt::Debug for ProjectWatcher {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProjectWatcher")
            .field("root", &self.shared.root)
            .finish()
    }
}

impl ProjectWatcher {
    /// Opens the watch handle; call it on a worker. `wake` is asked to
    /// schedule a frame after the given delay when a new batch begins.
    pub fn start(
        root: &Path,
        wake: impl Fn(Duration) + Send + Sync + 'static,
    ) -> Result<Self, String> {
        let shared = Arc::new(Shared {
            root: root.to_path_buf(),
            state: Mutex::new(State::default()),
            wake: Box::new(wake),
        });
        let handler = shared.clone();
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                handler.record(event);
            })
            .map_err(|error| format!("File watching is unavailable: {error}"))?;
        watcher
            .watch(root, RecursiveMode::Recursive)
            .map_err(|error| format!("File watching is unavailable: {error}"))?;
        Ok(Self {
            _watcher: watcher,
            shared,
        })
    }

    /// Tell the watcher which folders the Explorer currently shows a listing of.
    pub fn set_listed(&self, listed: impl IntoIterator<Item = PathBuf>) {
        self.shared.lock().listed = listed.into_iter().collect();
    }

    /// Drain the batch once it is quiet; otherwise say how long to wait.
    pub fn poll(&self, now: Instant) -> Poll {
        self.shared.lock().poll(now)
    }
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn record(&self, event: notify::Result<notify::Event>) {
        let mut state = self.lock();
        let was_empty = state.changes.is_empty();
        let recorded = match event {
            Ok(event) => state.classify(&self.root, &event),
            // The platform could not tell what changed: compare listings.
            Err(_) => {
                state.changes.rescan = true;
                state.changes.index = true;
                true
            }
        };
        if !recorded {
            return;
        }
        let now = Instant::now();
        state.first.get_or_insert(now);
        state.last = Some(now);
        drop(state);
        if was_empty {
            (self.wake)(QUIET);
        }
    }
}

impl State {
    fn poll(&mut self, now: Instant) -> Poll {
        let (Some(first), Some(last)) = (self.first, self.last) else {
            return Poll::Idle;
        };
        let quiet = now.saturating_duration_since(last);
        let waited = now.saturating_duration_since(first);
        if quiet >= QUIET || waited >= MAX_DELAY {
            self.first = None;
            self.last = None;
            return Poll::Ready(std::mem::take(&mut self.changes));
        }
        Poll::Wait((QUIET - quiet).min(MAX_DELAY - waited))
    }

    /// True when the event changed the pending batch.
    fn classify(&mut self, root: &Path, event: &notify::Event) -> bool {
        if event.need_rescan() {
            self.changes.rescan = true;
            self.changes.index = true;
            return true;
        }
        enum Name {
            Added,
            Removed,
            Changed,
            Content,
        }
        let name = match event.kind {
            EventKind::Create(_) | EventKind::Modify(ModifyKind::Name(RenameMode::To)) => {
                Name::Added
            }
            EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(RenameMode::From)) => {
                Name::Removed
            }
            EventKind::Modify(ModifyKind::Name(_)) => Name::Changed,
            EventKind::Modify(_) => Name::Content,
            _ => return false,
        };
        let mut recorded = false;
        for path in &event.paths {
            let Ok(relative) = path.strip_prefix(root) else {
                continue;
            };
            let components: Vec<_> = relative.components().collect();
            let Some((last, parents)) = components.split_last() else {
                // The project folder itself was removed or renamed.
                self.changes.rescan = true;
                self.changes.index = true;
                recorded = true;
                continue;
            };
            if parents.iter().any(|part| is_named(part, ".git")) {
                continue;
            }
            let ignore_rules = is_named(last, ".gitignore") || is_named(last, ".ignore");
            if matches!(name, Name::Content) {
                // Writing a file keeps every name; only ignore rules matter.
                if ignore_rules {
                    self.changes.rescan = true;
                    self.changes.index = true;
                    recorded = true;
                }
                continue;
            }
            if ignore_rules {
                self.changes.rescan = true;
            }
            if let Some(parent) = path.parent()
                && self.listed.contains(parent)
            {
                self.changes.directories.insert(parent.to_path_buf());
                recorded = true;
            }
            if self.listed.contains(path.as_path()) {
                match name {
                    Name::Removed => {
                        self.changes.removed.insert(path.clone());
                    }
                    _ => {
                        // Replaced right away (an atomic save or a quick
                        // re-creation): keep showing it and read it again.
                        self.changes.removed.remove(path);
                        self.changes.directories.insert(path.clone());
                    }
                }
                recorded = true;
            } else if matches!(name, Name::Added) {
                self.changes.removed.remove(path);
            }
            if !components
                .iter()
                .any(|part| is_named(part, ".git") || generated(part))
            {
                self.changes.index = true;
                recorded = true;
            }
            recorded |= ignore_rules;
        }
        recorded
    }
}

fn is_named(component: &Component<'_>, name: &str) -> bool {
    matches!(component, Component::Normal(part) if part.eq_ignore_ascii_case(name))
}

fn generated(component: &Component<'_>) -> bool {
    matches!(component, Component::Normal(part) if crate::workspace::generated_index_name(part))
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{CreateKind, DataChange, RemoveKind};

    fn root() -> PathBuf {
        PathBuf::from(r"C:\project")
    }

    fn state(listed: &[&str]) -> State {
        State {
            listed: listed
                .iter()
                .map(|path| {
                    if path.is_empty() {
                        root()
                    } else {
                        root().join(path)
                    }
                })
                .collect(),
            ..Default::default()
        }
    }

    fn event(kind: EventKind, path: &str) -> notify::Event {
        notify::Event::new(kind).add_path(root().join(path))
    }

    fn set(paths: &[&str]) -> BTreeSet<PathBuf> {
        paths
            .iter()
            .map(|path| {
                if path.is_empty() {
                    root()
                } else {
                    root().join(path)
                }
            })
            .collect()
    }

    #[test]
    fn new_files_refresh_their_listed_folder_and_the_index() {
        let mut state = state(&["", "src"]);
        let create = EventKind::Create(CreateKind::Any);
        assert!(state.classify(&root(), &event(create, "src/new.rs")));
        assert!(state.classify(&root(), &event(create, "docs/deep/x.md")));
        assert_eq!(state.changes.directories, set(&["src"]));
        assert!(state.changes.index);
        assert!(!state.changes.rescan);
    }

    #[test]
    fn content_writes_git_internals_and_unlisted_generated_folders_stay_silent() {
        let mut state = state(&["", "src"]);
        let write = EventKind::Modify(ModifyKind::Data(DataChange::Any));
        assert!(!state.classify(&root(), &event(write, "src/main.rs")));
        let create = EventKind::Create(CreateKind::Any);
        assert!(!state.classify(&root(), &event(create, ".git/objects/ab/cdef")));
        assert!(!state.classify(&root(), &event(create, "target/debug/deps/x.d")));
        assert!(!state.classify(&root(), &event(create, "web/node_modules/a/b.js")));
        assert!(state.changes.is_empty());
        // A listed generated folder still updates the Explorer, not the index.
        let mut state = super::tests::state(&["", "target"]);
        assert!(state.classify(&root(), &event(create, "target/new.txt")));
        assert_eq!(state.changes.directories, set(&["target"]));
        assert!(!state.changes.index);
        // Creating `.git` itself is a visible root entry.
        let mut state = super::tests::state(&[""]);
        assert!(state.classify(&root(), &event(create, ".git")));
        assert_eq!(state.changes.directories, set(&[""]));
    }

    #[test]
    fn removed_listed_folders_are_dropped_unless_they_return_in_the_batch() {
        let mut state = state(&["", "old", "kept"]);
        let remove = EventKind::Remove(RemoveKind::Any);
        state.classify(&root(), &event(remove, "old"));
        state.classify(
            &root(),
            &event(
                EventKind::Modify(ModifyKind::Name(RenameMode::From)),
                "kept",
            ),
        );
        state.classify(
            &root(),
            &event(EventKind::Modify(ModifyKind::Name(RenameMode::To)), "kept"),
        );
        assert_eq!(state.changes.removed, set(&["old"]));
        assert_eq!(state.changes.directories, set(&["", "kept"]));
    }

    #[test]
    fn ignore_rule_edits_rescan_and_root_removal_rescans() {
        let mut state = state(&[""]);
        let write = EventKind::Modify(ModifyKind::Data(DataChange::Any));
        assert!(state.classify(&root(), &event(write, "src/.gitignore")));
        assert!(state.changes.rescan && state.changes.index);
        let mut state = super::tests::state(&[]);
        let removed = notify::Event::new(EventKind::Remove(RemoveKind::Folder)).add_path(root());
        assert!(state.classify(&root(), &removed));
        assert!(state.changes.rescan);
        let mut state = super::tests::state(&[]);
        let rescan = notify::Event::new(EventKind::Other).set_flag(notify::event::Flag::Rescan);
        assert!(state.classify(&root(), &rescan));
        assert!(state.changes.rescan);
    }

    #[test]
    fn batches_wait_for_quiet_but_never_longer_than_the_maximum_delay() {
        let mut state = state(&[""]);
        let start = Instant::now();
        assert_eq!(state.poll(start), Poll::Idle);
        state.changes.index = true;
        state.first = Some(start);
        state.last = Some(start);
        assert_eq!(
            state.poll(start + Duration::from_millis(50)),
            Poll::Wait(Duration::from_millis(150))
        );
        // Steady churn keeps resetting quiet, but the maximum delay wins.
        state.last = Some(start + Duration::from_millis(950));
        assert_eq!(
            state.poll(start + Duration::from_millis(960)),
            Poll::Wait(Duration::from_millis(40))
        );
        let Poll::Ready(changes) = state.poll(start + MAX_DELAY) else {
            panic!("the batch was not published");
        };
        assert!(changes.index);
        assert_eq!(state.poll(start + MAX_DELAY), Poll::Idle);
    }

    #[test]
    fn a_real_watch_reports_a_new_file_in_a_listed_folder() {
        let root = std::env::temp_dir().join(format!(
            "vedit-fs-watch-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("src")).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let watcher = ProjectWatcher::start(&root, move |delay| {
            let _ = sender.send(delay);
        })
        .unwrap();
        watcher.set_listed([root.clone(), root.join("src")]);
        std::fs::write(root.join("src").join("agent.rs"), "fn main() {}\n").unwrap();
        assert_eq!(
            receiver.recv_timeout(Duration::from_secs(5)),
            Ok(QUIET),
            "the watcher did not wake the window"
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        let changes = loop {
            match watcher.poll(Instant::now()) {
                Poll::Ready(changes) => break changes,
                _ => {
                    assert!(Instant::now() < deadline, "the batch never became quiet");
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        };
        assert!(changes.directories.contains(&root.join("src")));
        assert!(changes.index);
        drop(watcher);
        std::fs::remove_dir_all(&root).unwrap();
    }
}

//! Serialized, atomic recovery writes with per-file revision ordering.
//!
//! Clone one writer for all workers that can write or remove recovery files.
//! Revisions are supplied by each document; [`RecoveryWriter::stamp`] provides
//! a shared sequence for snapshots that do not already have a revision.

use std::{
    collections::HashMap,
    ffi::OsString,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

#[derive(Clone, Copy)]
struct Commit {
    revision: u64,
    removed: bool,
}

#[derive(Default)]
struct Shared {
    commits: Mutex<HashMap<PathBuf, Commit>>,
    sequence: AtomicU64,
}

/// One coordination point for recovery writes and deletions, shared by clones.
#[derive(Clone, Default)]
pub struct RecoveryWriter {
    shared: Arc<Shared>,
}

impl RecoveryWriter {
    /// Return a monotonically increasing stamp shared by all clones.
    pub fn stamp(&self) -> u64 {
        self.shared.sequence.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Atomically replace a file unless a newer revision already succeeded.
    ///
    /// A removal also blocks writes at its own revision. Failed writes do not
    /// advance the committed revision, so retrying the same revision is safe.
    pub fn write(&self, path: &Path, revision: u64, bytes: &[u8]) -> Result<(), String> {
        let mut commits = self
            .shared
            .commits
            .lock()
            .map_err(|_| "Recovery writer lock is poisoned".to_owned())?;
        if commits.get(path).is_some_and(|commit| {
            revision < commit.revision || (commit.removed && revision == commit.revision)
        }) {
            return Ok(());
        }

        let file_name = path.file_name().ok_or_else(|| {
            format!(
                "Recovery path has no file name: {}",
                crate::paths::display(path)
            )
        })?;
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty());
        if let Some(parent) = parent {
            fs::create_dir_all(parent)
                .map_err(|error| format!("Could not create recovery folder: {error}"))?;
        }

        // Create exclusively: another writer/process must never share our
        // temporary file. The destination's directory keeps rename atomic.
        let (mut file, temporary) = loop {
            let mut name = OsString::from(".");
            name.push(file_name);
            name.push(format!(
                ".morrow-{}-{}.tmp",
                std::process::id(),
                self.stamp()
            ));
            let temporary = parent.unwrap_or_else(|| Path::new(".")).join(name);
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
            {
                Ok(file) => break (file, Temporary(temporary)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(format!("Could not create recovery file: {error}")),
            }
        };
        let written = file.write_all(bytes).and_then(|_| file.sync_all());
        drop(file);
        written.map_err(|error| format!("Could not write recovery file: {error}"))?;
        fs::rename(&temporary.0, path)
            .map_err(|error| format!("Could not replace recovery file: {error}"))?;
        commits.insert(
            path.to_owned(),
            Commit {
                revision,
                removed: false,
            },
        );
        Ok(())
    }

    /// Remove a recovery file and remember a tombstone at this revision.
    ///
    /// An older removal is ignored. Missing files count as a successful removal;
    /// later workers cannot recreate the file with an equal or older revision.
    /// A failed deletion still records the tombstone and can be retried.
    pub fn remove(&self, path: &Path, revision: u64) -> Result<(), String> {
        let mut commits = self
            .shared
            .commits
            .lock()
            .map_err(|_| "Recovery writer lock is poisoned".to_owned())?;
        if commits
            .get(path)
            .is_some_and(|commit| revision < commit.revision)
        {
            return Ok(());
        }
        let removed = fs::remove_file(path);
        commits.insert(
            path.to_owned(),
            Commit {
                revision,
                removed: true,
            },
        );
        match removed {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("Could not remove recovery file: {error}")),
        }
    }
}

struct Temporary(PathBuf);

impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::Barrier, thread};

    struct Directory(PathBuf);

    impl Directory {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "morrow-recovery-test-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn file(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn stale_writes_do_not_replace_latest_and_paths_are_independent() {
        let directory = Directory::new();
        let writer = RecoveryWriter::default();
        let first = directory.file("first.txt");
        let second = directory.file("second.txt");
        writer.write(&first, 20, b"latest").unwrap();
        writer.write(&first, 19, b"stale").unwrap();
        writer.write(&second, 1, b"independent").unwrap();
        assert_eq!(fs::read(&first).unwrap(), b"latest");
        assert_eq!(fs::read(&second).unwrap(), b"independent");
        writer.write(&first, 20, b"same revision retry").unwrap();
        assert_eq!(fs::read(&first).unwrap(), b"same revision retry");
    }

    #[test]
    fn concurrent_clones_commit_highest_revision() {
        let directory = Directory::new();
        let path = directory.file("concurrent.txt");
        let writer = RecoveryWriter::default();
        let barrier = Arc::new(Barrier::new(16));
        let workers: Vec<_> = (1..=16)
            .map(|revision| {
                let writer = writer.clone();
                let barrier = barrier.clone();
                let path = path.clone();
                thread::spawn(move || {
                    barrier.wait();
                    writer
                        .write(&path, revision, revision.to_string().as_bytes())
                        .unwrap();
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(fs::read(&path).unwrap(), b"16");
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 1);
    }

    #[test]
    fn overwrite_is_complete_and_temporary_files_are_removed() {
        let directory = Directory::new();
        let path = directory.file("nested/snapshot.txt");
        let writer = RecoveryWriter::default();
        writer.write(&path, 1, &vec![b'a'; 64 * 1024]).unwrap();
        writer.write(&path, 2, b"short replacement").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"short replacement");
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn removals_block_equal_and_older_writes_but_allow_new_edits() {
        let directory = Directory::new();
        let path = directory.file("deleted.txt");
        let writer = RecoveryWriter::default();
        writer.write(&path, 5, b"old").unwrap();
        writer.remove(&path, 4).unwrap();
        assert!(path.is_file());
        writer.remove(&path, 5).unwrap();
        writer.write(&path, 4, b"older").unwrap();
        writer.write(&path, 5, b"equal").unwrap();
        assert!(!path.exists());
        writer.write(&path, 6, b"new edit").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new edit");
        writer.remove(&path, 6).unwrap();
        writer.remove(&path, 7).unwrap();
        writer.clone().write(&path, 7, b"late worker").unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn failed_write_can_retry_same_revision() {
        let directory = Directory::new();
        let path = directory.file("snapshot.txt");
        let writer = RecoveryWriter::default();
        fs::create_dir(&path).unwrap();
        assert!(writer.write(&path, 9, b"retry").is_err());
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 1);
        fs::remove_dir(&path).unwrap();
        writer.write(&path, 9, b"retry").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"retry");
        writer.remove(&path, 9).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn failed_removal_records_tombstone_and_can_retry() {
        let directory = Directory::new();
        let path = directory.file("snapshot.txt");
        let writer = RecoveryWriter::default();
        fs::create_dir(&path).unwrap();
        assert!(writer.remove(&path, 9).is_err());
        fs::remove_dir(&path).unwrap();
        writer.write(&path, 9, b"late worker").unwrap();
        assert!(!path.exists());
        writer.remove(&path, 9).unwrap();
        writer.write(&path, 10, b"future edit").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"future edit");
    }

    #[test]
    fn stamps_are_unique_and_shared_across_concurrent_clones() {
        let writer = RecoveryWriter::default();
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let writer = writer.clone();
                thread::spawn(move || (0..100).map(|_| writer.stamp()).collect::<Vec<_>>())
            })
            .collect();
        let mut stamps = Vec::new();
        for worker in workers {
            stamps.extend(worker.join().unwrap());
        }
        stamps.sort_unstable();
        assert_eq!(stamps, (1..=800).collect::<Vec<_>>());
        assert_eq!(writer.stamp(), 801);
    }
}

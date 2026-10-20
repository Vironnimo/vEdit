//! Serialized, atomic recovery writes with per-file revision ordering.
//!
//! Clone one writer for all workers that can write or remove recovery files.
//! [`RecoveryWriter::stamp`] orders document snapshots, removals and session
//! writes across all clones. The editor tracks source revisions separately:
//! a completed save can change a recovery baseline without changing the text.

use std::{
    collections::HashMap,
    ffi::OsString,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime},
};

// The NUL distinguishes a recovery record from any supported legacy text file.
// Metadata and unescaped buffer bytes are committed in one atomic write, so a
// newer session cannot accidentally pair an older buffer with a new baseline.
// This is a persisted format identifier, not a display name. Keep it readable
// by both releases so an upgrade cannot turn recovery metadata into user text.
const DOCUMENT_HEADER: &[u8] = b"Morrow recovery v1\0";
const MAX_METADATA_BYTES: usize = 4096;

#[derive(serde::Serialize, serde::Deserialize)]
struct DocumentMetadata {
    baseline: Option<crate::workspace::FileFingerprint>,
    line_ending: crate::workspace::LineEnding,
    bom: bool,
}

pub struct RecoveredDocument {
    pub text: String,
    pub baseline: Option<crate::workspace::FileFingerprint>,
    pub line_ending: Option<crate::workspace::LineEnding>,
    pub bom: Option<bool>,
}

/// Encode one self-contained recovery snapshot, including its disk baseline.
pub fn encode_document(
    text: &str,
    baseline: Option<&crate::workspace::FileFingerprint>,
    line_ending: crate::workspace::LineEnding,
    bom: bool,
) -> Result<Vec<u8>, String> {
    let metadata = serde_json::to_vec(&DocumentMetadata {
        baseline: baseline.cloned(),
        line_ending,
        bom,
    })
    .map_err(|error| format!("Could not encode recovery metadata: {error}"))?;
    let mut bytes = Vec::with_capacity(DOCUMENT_HEADER.len() + metadata.len() + 1 + text.len());
    bytes.extend_from_slice(DOCUMENT_HEADER);
    bytes.extend_from_slice(&metadata);
    bytes.push(b'\n');
    bytes.extend_from_slice(text.as_bytes());
    Ok(bytes)
}

/// Read a current record or an older raw-text recovery. Legacy text has no
/// provable baseline and must never authorize replacement of a named file.
pub fn read_document(path: &Path, max_bytes: u64) -> Result<RecoveredDocument, String> {
    let maximum = max_bytes.saturating_add((DOCUMENT_HEADER.len() + MAX_METADATA_BYTES + 1) as u64);
    let (bytes, fingerprint) = crate::workspace::read_bytes(path, maximum)
        .map_err(|error| format!("Could not read recovery: {error}"))?;
    let Some(record) = bytes.strip_prefix(DOCUMENT_HEADER) else {
        if bytes.len() as u64 > max_bytes {
            return Err("Recovery document exceeds the supported size".into());
        }
        let legacy = crate::workspace::decode_document(path, bytes, fingerprint)?;
        return Ok(RecoveredDocument {
            text: legacy.text,
            baseline: None,
            line_ending: None,
            bom: None,
        });
    };
    let separator = record
        .iter()
        .position(|byte| *byte == b'\n')
        .filter(|position| *position <= MAX_METADATA_BYTES)
        .ok_or_else(|| "Recovery metadata is incomplete".to_owned())?;
    let metadata: DocumentMetadata = serde_json::from_slice(&record[..separator])
        .map_err(|error| format!("Recovery metadata is invalid: {error}"))?;
    let text = &record[separator + 1..];
    if text.len() as u64 > max_bytes {
        return Err("Recovery document exceeds the supported size".into());
    }
    let text = std::str::from_utf8(text)
        .map_err(|error| format!("Recovery text is not valid UTF-8: {error}"))?
        .to_owned();
    Ok(RecoveredDocument {
        text,
        baseline: metadata.baseline,
        line_ending: Some(metadata.line_ending),
        bom: Some(metadata.bom),
    })
}

#[derive(Clone, Copy)]
struct Commit {
    revision: u64,
    removed: bool,
}

#[derive(Default)]
struct Shared {
    commits: Mutex<HashMap<PathBuf, Commit>>,
    sequence: AtomicU64,
    /// Background writes and removals that have not finished yet.
    in_flight: AtomicUsize,
}

/// The start of this session's recovery file names: the process ID and a
/// random part, so a later session that receives a reused process ID cannot
/// pick a name that an earlier session's recovery still uses.
pub fn session_prefix() -> &'static str {
    static PREFIX: OnceLock<String> = OnceLock::new();
    PREFIX.get_or_init(|| {
        use std::hash::{BuildHasher, Hasher};
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u128(
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_nanos()),
        );
        format!("{}-{:016x}", std::process::id(), hasher.finish())
    })
}

/// The process ID that created a recovery file or one of its temporary files,
/// for names that this or an earlier vEdit release generates:
/// `<pid>-<n>.txt`, `<pid>-<random>-<n>.txt`, their `-after-unreadable-<n>`
/// variants and `.<name>.vedit-<pid>-<n>.tmp`.
fn recovery_owner(name: &str) -> Option<u32> {
    let digits = |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    if let Some(temporary) = name
        .strip_prefix('.')
        .and_then(|rest| rest.strip_suffix(".tmp"))
    {
        let (_, owner) = temporary.rsplit_once(".vedit-")?;
        let (pid, sequence) = owner.split_once('-')?;
        return (digits(pid) && digits(sequence))
            .then(|| pid.parse().ok())
            .flatten();
    }
    let stem = name.strip_suffix(".txt")?;
    let stem = match stem.split_once("-after-unreadable-") {
        Some((stem, stamp)) if digits(stamp) => stem,
        Some(_) => return None,
        None => stem,
    };
    let mut parts = stem.split('-');
    let pid = parts.next().filter(|pid| digits(pid))?;
    let rest: Vec<_> = parts.collect();
    let valid = match rest.as_slice() {
        [sequence] => digits(sequence),
        [random, sequence] => {
            random.len() == 16
                && random.bytes().all(|byte| byte.is_ascii_hexdigit())
                && digits(sequence)
        }
        _ => false,
    };
    valid.then(|| pid.parse().ok()).flatten()
}

/// Remove recovery files in `directory` that nothing can restore any more:
/// files with vEdit's naming that no restored document references, whose
/// process has exited, that are older than `minimum_age` and, for documents,
/// that still read as complete recovery records. Unreadable records stay so
/// that a user can inspect them. Returns the removed paths.
pub fn remove_orphans(
    directory: &Path,
    referenced: &[PathBuf],
    minimum_age: Duration,
) -> Vec<PathBuf> {
    let referenced: Vec<_> = referenced
        .iter()
        .filter_map(|path| path.file_name())
        .map(|name| name.to_string_lossy().to_lowercase())
        .collect();
    let current = std::process::id();
    let mut removed = Vec::new();
    let Ok(entries) = fs::read_dir(directory) else {
        return removed;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(pid) = recovery_owner(&name) else {
            continue;
        };
        let path = entry.path();
        let old_enough = entry
            .metadata()
            .ok()
            .filter(fs::Metadata::is_file)
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age >= minimum_age);
        if !old_enough
            || pid == current
            || referenced.contains(&name.to_lowercase())
            || crate::process_liveness::running(pid)
        {
            continue;
        }
        let temporary = name.starts_with('.');
        if (temporary || read_document(&path, u64::MAX).is_ok()) && fs::remove_file(&path).is_ok() {
            removed.push(path);
        }
    }
    removed
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
                ".vedit-{}-{}.tmp",
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

    /// [`Self::write`] on a background thread, for snapshots whose result
    /// nobody waits for, such as the session manifest.
    pub fn write_in_background(&self, path: PathBuf, revision: u64, bytes: Vec<u8>) {
        self.in_background(move |writer| {
            let _ = writer.write(&path, revision, &bytes);
        });
    }

    /// [`Self::remove`] on a background thread. A failed removal, for example
    /// while a virus scanner holds the file, is retried a few times.
    pub fn remove_in_background(&self, path: PathBuf, revision: u64) {
        self.in_background(move |writer| {
            for delay in [0, 250, 1000, 4000] {
                std::thread::sleep(Duration::from_millis(delay));
                if writer.remove(&path, revision).is_ok() {
                    return;
                }
            }
        });
    }

    fn in_background(&self, work: impl FnOnce(&Self) + Send + 'static) {
        let writer = self.clone();
        writer.shared.in_flight.fetch_add(1, Ordering::SeqCst);
        std::thread::spawn(move || {
            struct Done(RecoveryWriter);
            impl Drop for Done {
                fn drop(&mut self) {
                    self.0.shared.in_flight.fetch_sub(1, Ordering::SeqCst);
                }
            }
            let done = Done(writer);
            work(&done.0);
        });
    }

    /// Wait until background writes and removals have finished.
    #[cfg(test)]
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        while self.shared.in_flight.load(Ordering::SeqCst) > 0 {
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        true
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
                "vedit-recovery-test-{}-{}-{}",
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

    #[test]
    fn document_record_keeps_buffer_baseline_and_encoding_in_one_write() {
        let directory = Directory::new();
        let path = directory.file("draft.recovery");
        let baseline = crate::workspace::FileFingerprint {
            len: 123,
            modified: Some(std::time::UNIX_EPOCH + std::time::Duration::new(42, 17)),
            hash: u64::MAX - 9,
        };
        let text = "# Entwurf 🦀\n\"quoted\"\\path\nzweite Zeile\n";
        let bytes = encode_document(
            text,
            Some(&baseline),
            crate::workspace::LineEnding::CrLf,
            true,
        )
        .unwrap();
        assert!(bytes.starts_with(DOCUMENT_HEADER));
        assert!(
            bytes.ends_with(text.as_bytes()),
            "buffer bytes are not JSON-escaped"
        );
        RecoveryWriter::default().write(&path, 1, &bytes).unwrap();
        let recovered = read_document(&path, 1024).unwrap();
        assert_eq!(recovered.text, text);
        assert_eq!(recovered.baseline, Some(baseline));
        assert_eq!(
            recovered.line_ending,
            Some(crate::workspace::LineEnding::CrLf)
        );
        assert_eq!(recovered.bom, Some(true));
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }

    #[test]
    fn legacy_recovery_is_readable_without_claiming_a_verified_baseline() {
        let directory = Directory::new();
        let path = directory.file("legacy.txt");
        let source = "\u{feff}Legacy ü\r\n{\"text\":\"not metadata\"}\r\n";
        fs::write(&path, source).unwrap();
        let recovered = read_document(&path, 1024).unwrap();
        assert_eq!(recovered.text, "Legacy ü\n{\"text\":\"not metadata\"}\n");
        assert!(recovered.baseline.is_none());
        assert!(recovered.line_ending.is_none());
        assert!(recovered.bom.is_none());
        assert_eq!(fs::read(&path).unwrap(), source.as_bytes());
    }

    #[test]
    fn invalid_or_future_records_are_not_mistaken_for_legacy_text() {
        let directory = Directory::new();
        let path = directory.file("invalid.recovery");
        for suffix in [
            b"{".as_slice(),
            b"{}\ntext".as_slice(),
            b"invalid\ntext".as_slice(),
        ] {
            let mut bytes = DOCUMENT_HEADER.to_vec();
            bytes.extend_from_slice(suffix);
            fs::write(&path, &bytes).unwrap();
            assert!(read_document(&path, 1024).is_err());
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
        let bytes = b"Morrow recovery v2\0future format";
        fs::write(&path, bytes).unwrap();
        assert!(read_document(&path, 1024).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }

    #[test]
    fn record_size_limit_applies_to_the_buffer_not_its_small_metadata() {
        let directory = Directory::new();
        let path = directory.file("bounded.recovery");
        let text = "é🦀\n";
        let bytes = encode_document(text, None, crate::workspace::LineEnding::Lf, false).unwrap();
        fs::write(&path, &bytes).unwrap();
        assert_eq!(read_document(&path, text.len() as u64).unwrap().text, text);
        assert!(read_document(&path, text.len() as u64 - 1).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }

    #[test]
    fn legacy_recovery_cannot_use_the_metadata_size_allowance_for_its_text() {
        let directory = Directory::new();
        let path = directory.file("oversized-legacy.txt");
        let text = "legacy text is larger than the buffer limit";
        fs::write(&path, text).unwrap();
        assert!(read_document(&path, text.len() as u64 - 1).is_err());
        assert_eq!(read_document(&path, text.len() as u64).unwrap().text, text);
        assert_eq!(fs::read_to_string(&path).unwrap(), text);
    }

    #[test]
    fn recovery_requires_a_regular_file() {
        let directory = Directory::new();
        let nested = directory.file("not-a-snapshot");
        fs::create_dir(&nested).unwrap();
        assert!(read_document(&nested, 1024).is_err());
        assert!(nested.is_dir());
    }

    fn exited_process_id() -> u32 {
        #[cfg(windows)]
        let mut child = std::process::Command::new("cmd")
            .args(["/C", "exit"])
            .spawn()
            .unwrap();
        #[cfg(not(windows))]
        let mut child = std::process::Command::new("true").spawn().unwrap();
        child.wait().unwrap();
        child.id()
    }

    #[test]
    fn recovery_names_are_unique_per_session_and_name_their_owner() {
        let prefix = session_prefix();
        assert_eq!(prefix, session_prefix());
        let (pid, random) = prefix.split_once('-').unwrap();
        assert_eq!(pid, std::process::id().to_string());
        assert_eq!(random.len(), 16);
        let current = std::process::id();
        for name in [
            format!("{prefix}-7.txt"),
            format!("{prefix}-4-after-unreadable-12.txt"),
            format!("{current}-7.txt"),
            format!("{current}-3-after-unreadable-12.txt"),
            format!(".{prefix}-7.txt.vedit-{current}-4.tmp"),
        ] {
            assert_eq!(recovery_owner(&name), Some(current), "{name}");
        }
        for name in [
            "session.json",
            "notes.txt",
            "12-notes.txt",
            "12-0123456789abcdeg-1.txt",
            "12-0123-1.txt",
            "12-1-after-unreadable-x.txt",
            "12-1.txt.bak",
            ".12-1.txt.vedit-12-x.tmp",
            ".12-1.txt.vedit-12-1.tmp.old",
        ] {
            assert_eq!(recovery_owner(name), None, "{name}");
        }
    }

    #[test]
    fn only_old_unreferenced_readable_recovery_of_exited_sessions_is_removed() {
        let directory = Directory::new();
        let exited = exited_process_id();
        let current = std::process::id();
        let record =
            encode_document("draft ü\n", None, crate::workspace::LineEnding::Lf, false).unwrap();
        let write = |name: String, bytes: &[u8]| {
            let path = directory.file(&name);
            fs::write(&path, bytes).unwrap();
            path
        };
        let orphans = [
            write(format!("{exited}-1.txt"), &record),
            write(format!("{exited}-0123456789abcdef-2.txt"), b"legacy text"),
            write(
                format!("{exited}-0123456789abcdef-9-after-unreadable-3.txt"),
                &record,
            ),
            write(format!(".{exited}-1.txt.vedit-{exited}-4.tmp"), b"partial"),
        ];
        let referenced = write(format!("{exited}-0123456789ABCDEF-5.txt"), &record);
        let mut unreadable = DOCUMENT_HEADER.to_vec();
        unreadable.extend_from_slice(b"invalid\ntext");
        let kept = [
            referenced.clone(),
            write(format!("{current}-6.txt"), &record),
            write(format!("{exited}-7.txt"), &unreadable),
            write("session.json".into(), b"{}"),
            write("notes.txt".into(), &record),
        ];
        let subfolder = directory.file(&format!("{exited}-8.txt"));
        fs::create_dir(&subfolder).unwrap();

        // Recent files may still belong to a session that is starting up.
        let referenced_upper = directory.file(&format!("{exited}-0123456789abcdef-5.TXT"));
        assert!(
            remove_orphans(
                &directory.0,
                std::slice::from_ref(&referenced_upper),
                Duration::from_secs(3600)
            )
            .is_empty()
        );
        assert!(orphans.iter().chain(&kept).all(|path| path.is_file()));

        let mut removed = remove_orphans(&directory.0, &[referenced_upper], Duration::ZERO);
        removed.sort();
        let mut expected = orphans.to_vec();
        expected.sort();
        assert_eq!(removed, expected);
        assert!(orphans.iter().all(|path| !path.exists()));
        assert!(kept.iter().all(|path| path.is_file()));
        assert!(subfolder.is_dir());
    }
}

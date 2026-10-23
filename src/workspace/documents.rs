//! Bounded document snapshots and atomic saves that preserve format and baseline.
use super::MAX_DOCUMENT_BYTES;
use std::{
    collections::BTreeSet,
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::SystemTime,
};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

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
    decode_document(path, bytes, fingerprint)
}

/// Decode one already-read snapshot. Recovery format detection and legacy
/// decoding must operate on the same bytes, even when a writer replaces the
/// file concurrently.
pub(crate) fn decode_document(
    path: &Path,
    bytes: Vec<u8>,
    fingerprint: FileFingerprint,
) -> Result<LoadedDocument, String> {
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

pub(crate) fn read_bytes(path: &Path, max_bytes: u64) -> io::Result<(Vec<u8>, FileFingerprint)> {
    // Explorer listings include special filesystem entries. Inspect before
    // opening so selecting a FIFO or device cannot block a document worker.
    if !fs::metadata(path)?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
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

#[derive(Debug)]
pub struct SavedDocument {
    pub fingerprint: FileFingerprint,
    pub line_ending: LineEnding,
    pub format_fallback: bool,
}

/// Atomically replace a document using a temporary file in the same directory.
/// `expected = None` creates a new file and refuses to overwrite an existing one.
/// Existing files must match the last loaded/saved content fingerprint.
/// Mixed line endings come only from a verified original document, never from a
/// different Save As destination. If that original is unavailable or changed,
/// saving the user's buffer remains possible, with an explicit LF fallback.
pub fn save_document(
    path: &Path,
    text: &str,
    line_ending: LineEnding,
    bom: bool,
    expected: Option<&FileFingerprint>,
    source: Option<(&Path, &FileFingerprint)>,
) -> Result<SavedDocument, SaveError> {
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
    let mut format_fallback = false;
    let encoded = match line_ending {
        LineEnding::Lf => normalized,
        LineEnding::CrLf => normalized.replace('\n', "\r\n"),
        LineEnding::Cr => normalized.replace('\n', "\r"),
        LineEnding::Mixed => match verified_format_source(source) {
            Some(source) => encode_mixed_endings(&source, &normalized),
            None => {
                format_fallback = true;
                normalized
            }
        },
    };
    let line_ending = if line_ending == LineEnding::Mixed {
        let crlf = encoded.matches("\r\n").count();
        let cr = encoded.matches('\r').count() - crlf;
        let lf = encoded.matches('\n').count() - crlf;
        match (crlf > 0, cr > 0, lf > 0) {
            (true, false, false) => LineEnding::CrLf,
            (false, true, false) => LineEnding::Cr,
            (false, false, _) => LineEnding::Lf,
            _ => LineEnding::Mixed,
        }
    } else {
        line_ending
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
    let result = (|| -> Result<SavedDocument, SaveError> {
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
        Ok(SavedDocument {
            fingerprint: FileFingerprint {
                len: bytes.len() as u64,
                modified: metadata.modified().ok(),
                hash: content_hash(&bytes),
            },
            line_ending,
            format_fallback,
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

/// Remove `.vedit-save-<pid>-<n>.tmp` files that an interrupted save of an
/// exited vEdit process left in `directory`, where they would otherwise show
/// up in `git status`. Files of this or another running process and all other
/// files stay. Each directory is checked once per session.
pub fn remove_stale_save_temporaries(directory: &Path) {
    static CHECKED: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());
    if !CHECKED
        .lock()
        .map(|mut checked| checked.insert(directory.to_path_buf()))
        .unwrap_or(false)
    {
        return;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let current = std::process::id();
    for entry in entries.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(save_temporary_owner) else {
            continue;
        };
        if pid != current
            && entry.file_type().is_ok_and(|kind| kind.is_file())
            && !crate::process_liveness::running(pid)
        {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// The process ID in a name created by [`create_temporary`].
fn save_temporary_owner(name: &str) -> Option<u32> {
    let (pid, sequence) = name
        .strip_prefix(".vedit-save-")?
        .strip_suffix(".tmp")?
        .split_once('-')?;
    let digits = |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    (digits(pid) && digits(sequence))
        .then(|| pid.parse().ok())
        .flatten()
}

fn create_temporary(parent: &Path) -> io::Result<(PathBuf, File)> {
    for _ in 0..64 {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(".vedit-save-{}-{sequence}.tmp", std::process::id()));
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
fn encode_mixed_endings(source: &str, normalized: &str) -> String {
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
    encoded
}

fn verified_format_source(source: Option<(&Path, &FileFingerprint)>) -> Option<String> {
    let (path, expected) = source?;
    let (bytes, current) = read_bytes(path, MAX_DOCUMENT_BYTES).ok()?;
    if !current.same_content(expected) {
        return None;
    }
    std::str::from_utf8(bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes))
        .ok()
        .map(str::to_owned)
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
#[path = "documents_tests.rs"]
mod tests;

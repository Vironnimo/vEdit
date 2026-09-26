//! Persistent selections the user hands to an agent.
//!
//! A mark always owns the exact selected text. Its live highlight is a separate,
//! conservative attachment: changing selected text makes it stale, and opening
//! a saved file again only attaches it after the complete document matches.
//! The application owns durable session writes and the CLI's response budget.

use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fmt,
    ops::Range,
    path::{Path, PathBuf},
    sync::OnceLock,
};

pub const MAX_NOTE_CHARS: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CharRange {
    pub start: usize,
    pub end: usize,
}

impl From<Range<usize>> for CharRange {
    fn from(range: Range<usize>) -> Self {
        Self {
            start: range.start,
            end: range.end,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkPosition {
    pub line: usize,
    pub column: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkTarget {
    pub workspace: PathBuf,
    pub path: Option<PathBuf>,
    pub title: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentFingerprint {
    pub bytes: usize,
    pub chars: usize,
    pub hash: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserMark {
    pub id: u64,
    /// A runtime hint, never sufficient to reattach a restored untitled buffer.
    pub document_id: u64,
    pub target: MarkTarget,
    /// Zero-based Unicode scalar offsets, with an exclusive end.
    pub range: CharRange,
    /// One-based Unicode positions for the last verified range.
    pub start: MarkPosition,
    pub end: MarkPosition,
    pub snippet: String,
    pub note: String,
    pub stale: bool,
    pub source_fingerprint: DocumentFingerprint,
    #[serde(skip)]
    attached: bool,
    #[serde(skip)]
    runtime_identity: bool,
    /// Sparse byte offsets make repeated Unicode snippet pages linear overall.
    #[serde(skip)]
    snippet_offsets: OnceLock<Vec<usize>>,
}

impl UserMark {
    pub fn is_attached(&self) -> bool {
        self.attached && !self.stale
    }

    /// Valid only inside the running process; serialized tab IDs alone never
    /// identify an unnamed buffer after restart or after that buffer closes.
    pub fn has_runtime_identity(&self) -> bool {
        self.runtime_identity
    }

    pub fn snippet_chars(&self) -> usize {
        self.range.end.saturating_sub(self.range.start)
    }

    fn snippet_slice(&self, start: usize, end: usize) -> String {
        if start == end {
            return String::new();
        }
        if self.snippet.len() == self.snippet_chars() {
            return self.snippet[start..end].to_owned();
        }
        const STRIDE: usize = 4096;
        let offsets = self.snippet_offsets.get_or_init(|| {
            self.snippet
                .char_indices()
                .enumerate()
                .filter_map(|(index, (byte, _))| (index % STRIDE == 0).then_some(byte))
                .collect()
        });
        let byte = offsets[start / STRIDE];
        self.snippet[byte..]
            .chars()
            .skip(start % STRIDE)
            .take(end - start)
            .collect()
    }
}

/// Borrowed buffer identity; obtaining a mark never changes editor selection.
#[derive(Clone, Copy)]
pub struct MarkDocument<'a> {
    pub id: u64,
    pub workspace: &'a Path,
    pub path: Option<&'a Path>,
    pub title: &'a str,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MarkBatch {
    pub id: u64,
    pub request_id: String,
    pub items: Vec<UserMark>,
    #[serde(default)]
    pub acknowledged: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct TakeOutcome {
    pub batch_id: Option<u64>,
    pub count: usize,
    pub replayed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SnippetChunk {
    pub text: String,
    pub offset: usize,
    pub total_chars: usize,
    pub next_offset: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MarkError {
    InvalidRange,
    NoteTooLong,
    UnknownMark(u64),
    UnknownBatch(u64),
    ReceiptExpired { batch_id: u64 },
    BatchPending { batch_id: u64 },
    InvalidRequestId,
    InvalidChunk,
}

impl MarkError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidRange
            | Self::InvalidChunk
            | Self::InvalidRequestId
            | Self::NoteTooLong => "invalid_request",
            Self::UnknownMark(_) => "mark_not_found",
            Self::UnknownBatch(_) => "batch_not_found",
            Self::ReceiptExpired { .. } => "receipt_expired",
            Self::BatchPending { .. } => "batch_pending",
        }
    }

    pub fn batch_id(&self) -> Option<u64> {
        match self {
            Self::UnknownBatch(id) => Some(*id),
            Self::ReceiptExpired { batch_id } | Self::BatchPending { batch_id } => Some(*batch_id),
            _ => None,
        }
    }
}

impl fmt::Display for MarkError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRange => {
                formatter.write_str("Select a non-empty range inside the document")
            }
            Self::NoteTooLong => write!(
                formatter,
                "A mark note supports at most {MAX_NOTE_CHARS} Unicode characters"
            ),
            Self::UnknownMark(id) => write!(
                formatter,
                "Agent mark {id} does not exist in this collection"
            ),
            Self::UnknownBatch(id) => {
                write!(formatter, "Agent mark batch {id} is no longer available")
            }
            Self::ReceiptExpired { batch_id } => write!(
                formatter,
                "This request already consumed batch {batch_id}, whose snapshot has expired; no new marks were consumed"
            ),
            Self::BatchPending { batch_id } => write!(
                formatter,
                "Batch {batch_id} is awaiting acknowledgement; read it and acknowledge it before taking new marks"
            ),
            Self::InvalidRequestId => formatter.write_str(
                "A take request ID must contain 1 to 128 UTF-8 bytes without control characters",
            ),
            Self::InvalidChunk => formatter.write_str(
                "Snippet offset must be inside the snapshot, and count must be between 1 and 65536",
            ),
        }
    }
}

impl std::error::Error for MarkError {}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RestoreSource {
    #[default]
    Removed,
    Taken(u64),
}

struct Baseline {
    revision: u64,
    text: String,
    fingerprint: DocumentFingerprint,
    collection_revision: u64,
    target: MarkTarget,
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct AgentMarks {
    pub items: Vec<UserMark>,
    pub last_removed: Vec<UserMark>,
    pub last_taken: Option<MarkBatch>,
    next_id: u64,
    next_batch_id: u64,
    /// Tombstones make retries of older batches safe without retaining all text.
    used_requests: HashMap<String, u64>,
    restore_source: RestoreSource,
    #[serde(skip)]
    baselines: HashMap<u64, Baseline>,
    #[serde(skip)]
    collection_revision: u64,
    #[serde(skip)]
    known_documents: HashSet<u64>,
}

impl Default for AgentMarks {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            last_removed: Vec::new(),
            last_taken: None,
            next_id: 1,
            next_batch_id: 1,
            used_requests: HashMap::new(),
            restore_source: RestoreSource::Removed,
            baselines: HashMap::new(),
            collection_revision: 0,
            known_documents: HashSet::new(),
        }
    }
}

impl Clone for AgentMarks {
    fn clone(&self) -> Self {
        Self {
            items: self.items.clone(),
            last_removed: self.last_removed.clone(),
            last_taken: self.last_taken.clone(),
            next_id: self.next_id,
            next_batch_id: self.next_batch_id,
            used_requests: self.used_requests.clone(),
            restore_source: self.restore_source,
            known_documents: self.known_documents.clone(),
            ..Self::default()
        }
        // Serialization skips runtime identity and attachments. A clone keeps
        // them for an in-process rollback, but never copies entire documents.
    }
}

impl AgentMarks {
    pub fn add(
        &mut self,
        document: MarkDocument<'_>,
        text: &str,
        revision: u64,
        range: Range<usize>,
        note: &str,
    ) -> Result<u64, MarkError> {
        validate_note(note)?;
        let (fingerprint, lines) = inspect_document(text);
        if range.start >= range.end || range.end > fingerprint.chars {
            return Err(MarkError::InvalidRange);
        }
        self.known_documents.insert(document.id);
        self.synchronize_document(document, text, revision);
        self.ensure_baseline(document, text, revision, fingerprint);
        let id = self.allocate_id();
        self.items.push(UserMark {
            id,
            document_id: document.id,
            target: MarkTarget {
                workspace: document.workspace.to_path_buf(),
                path: document.path.map(Path::to_path_buf),
                title: document.title.to_owned(),
            },
            range: range.clone().into(),
            start: position(&lines, range.start),
            end: position(&lines, range.end),
            snippet: char_slice(text, range.start, range.end),
            note: note.to_owned(),
            stale: false,
            source_fingerprint: fingerprint,
            attached: true,
            runtime_identity: true,
            snippet_offsets: OnceLock::new(),
        });
        self.touch_collection();
        Ok(id)
    }

    /// Process one buffer revision once, regardless of how many marks it has.
    /// Edits wholly outside a mark relocate it. Overlapping edits retain the
    /// captured text and disable the highlight rather than guess a new range.
    pub fn synchronize_document(
        &mut self,
        document: MarkDocument<'_>,
        text: &str,
        revision: u64,
    ) -> bool {
        if self.baselines.get(&document.id).is_some_and(|baseline| {
            baseline.revision == revision
                && baseline.collection_revision == self.collection_revision
                && target_matches_descriptor(&baseline.target, document)
        }) {
            return false;
        }
        let known_document = self.known_documents.contains(&document.id);
        let indices: Vec<_> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(index, mark)| {
                document_matches(mark, document, known_document).then_some(index)
            })
            .collect();
        if indices.is_empty() {
            self.baselines.remove(&document.id);
            return false;
        }
        self.known_documents.insert(document.id);
        let old = self.baselines.remove(&document.id);
        let same_revision = old
            .as_ref()
            .is_some_and(|baseline| baseline.revision == revision);
        let (fingerprint, lines) = if same_revision {
            let baseline = old.as_ref().unwrap();
            (baseline.fingerprint, line_offsets(text))
        } else {
            inspect_document(text)
        };
        let change = old
            .as_ref()
            .filter(|baseline| baseline.fingerprint != fingerprint)
            .map(|baseline| {
                changed_range(
                    &baseline.text,
                    baseline.fingerprint.chars,
                    text,
                    fingerprint.chars,
                )
            });
        let mut changed = false;
        for index in indices {
            let mark = &mut self.items[index];
            let before = (
                mark.document_id,
                mark.range,
                mark.start,
                mark.end,
                mark.stale,
                mark.source_fingerprint,
                mark.attached,
            );
            // A verified path identifies this running buffer even when its
            // text changed. Never leave the recovered process's old tab ID
            // beside a newly granted runtime identity.
            mark.runtime_identity = true;
            mark.document_id = document.id;
            if mark.source_fingerprint == fingerprint && verified_range(mark, fingerprint.chars) {
                mark.stale = false;
                mark.attached = true;
                mark.document_id = document.id;
            } else if mark.attached
                && !mark.stale
                && old
                    .as_ref()
                    .is_some_and(|baseline| baseline.fingerprint == mark.source_fingerprint)
            {
                if let Some((old_changed, new_changed)) = &change {
                    if old_changed.end <= mark.range.start {
                        let removed = old_changed.end - old_changed.start;
                        let inserted = new_changed.end - new_changed.start;
                        mark.range.start = mark.range.start - removed + inserted;
                        mark.range.end = mark.range.end - removed + inserted;
                    } else if old_changed.start < mark.range.end {
                        mark.stale = true;
                        mark.attached = false;
                    }
                }
                if !mark.stale && verified_range(mark, fingerprint.chars) {
                    mark.source_fingerprint = fingerprint;
                    mark.start = position(&lines, mark.range.start);
                    mark.end = position(&lines, mark.range.end);
                    mark.document_id = document.id;
                    mark.attached = true;
                } else {
                    mark.stale = true;
                    mark.attached = false;
                }
            } else {
                mark.stale = true;
                mark.attached = false;
            }
            let after = (
                mark.document_id,
                mark.range,
                mark.start,
                mark.end,
                mark.stale,
                mark.source_fingerprint,
                mark.attached,
            );
            changed |= before != after;
            if !target_matches_descriptor(&mark.target, document) {
                mark.target = owned_target(document);
                changed = true;
            }
        }
        let baseline_text = match old {
            Some(baseline) if baseline.fingerprint == fingerprint => baseline.text,
            _ => text.to_owned(),
        };
        self.baselines.insert(
            document.id,
            Baseline {
                revision,
                text: baseline_text,
                fingerprint,
                collection_revision: self.collection_revision,
                target: owned_target(document),
            },
        );
        changed
    }

    pub fn visible_marks(&self, document_id: u64) -> impl Iterator<Item = &UserMark> {
        self.items
            .iter()
            .filter(move |mark| mark.document_id == document_id && mark.is_attached())
    }

    /// A closed document's frozen selections remain available to agents.
    pub fn detach_document(&mut self, document_id: u64) {
        self.baselines.remove(&document_id);
        self.known_documents.remove(&document_id);
        for mark in self
            .items
            .iter_mut()
            .chain(self.last_removed.iter_mut())
            .chain(
                self.last_taken
                    .iter_mut()
                    .flat_map(|batch| batch.items.iter_mut()),
            )
        {
            if mark.document_id == document_id {
                mark.attached = false;
                mark.runtime_identity = false;
            }
        }
        self.touch_collection();
    }

    /// A durable transaction stages a clone without copying buffer text. Once
    /// that transaction is saved, transfer the original baselines so the next
    /// edit can still relocate surviving marks from the exact previous text.
    pub fn preserve_live_caches_from(&mut self, previous: &mut Self) {
        let current_ids: HashSet<_> = self
            .items
            .iter()
            .filter(|mark| mark.has_runtime_identity())
            .map(|mark| mark.document_id)
            .collect();
        for (id, baseline) in std::mem::take(&mut previous.baselines) {
            if current_ids.contains(&id) {
                self.baselines.entry(id).or_insert(baseline);
            }
        }
        self.baselines.retain(|id, _| current_ids.contains(id));
        for baseline in self.baselines.values_mut() {
            // Restore may have introduced previously detached marks. Force one
            // attachment pass even when the buffer revision did not change.
            baseline.collection_revision = self.collection_revision.wrapping_sub(1);
        }
    }

    pub fn get(&self, id: u64, batch_id: Option<u64>) -> Option<&UserMark> {
        let items = match batch_id {
            Some(id) => &self.batch(Some(id))?.items,
            None => &self.items,
        };
        items.iter().find(|mark| mark.id == id)
    }

    pub fn batch(&self, batch_id: Option<u64>) -> Option<&MarkBatch> {
        self.last_taken
            .as_ref()
            .filter(|batch| batch_id.is_none_or(|id| batch.id == id))
    }

    pub fn last_batch(&self) -> Option<&MarkBatch> {
        self.last_taken.as_ref()
    }

    pub fn set_note(&mut self, id: u64, note: &str) -> Result<bool, MarkError> {
        validate_note(note)?;
        let mark = self
            .items
            .iter_mut()
            .find(|mark| mark.id == id)
            .ok_or(MarkError::UnknownMark(id))?;
        if mark.note == note {
            return Ok(false);
        }
        mark.note = note.to_owned();
        Ok(true)
    }

    pub fn remove(&mut self, id: u64) -> bool {
        let Some(index) = self.items.iter().position(|mark| mark.id == id) else {
            return false;
        };
        self.last_removed = vec![self.items.remove(index)];
        self.last_removed[0].attached = false;
        self.restore_source = RestoreSource::Removed;
        self.touch_collection();
        true
    }

    pub fn clear(&mut self) -> usize {
        if self.items.is_empty() {
            return 0;
        }
        self.last_removed = std::mem::take(&mut self.items);
        for mark in &mut self.last_removed {
            mark.attached = false;
        }
        self.restore_source = RestoreSource::Removed;
        self.baselines.clear();
        self.touch_collection();
        self.last_removed.len()
    }

    pub fn take(&mut self, request_id: &str) -> Result<TakeOutcome, MarkError> {
        if request_id.is_empty()
            || request_id.len() > 128
            || request_id.chars().any(char::is_control)
        {
            return Err(MarkError::InvalidRequestId);
        }
        if let Some(batch_id) = self.used_requests.get(request_id).copied() {
            if batch_id == 0 {
                return Ok(TakeOutcome {
                    batch_id: None,
                    count: 0,
                    replayed: true,
                });
            }
            let batch = self
                .batch(Some(batch_id))
                .ok_or(MarkError::ReceiptExpired { batch_id })?;
            return Ok(TakeOutcome {
                batch_id: Some(batch.id),
                count: batch.items.len(),
                replayed: true,
            });
        }
        if let Some(batch) = self.last_taken.as_ref().filter(|batch| !batch.acknowledged) {
            return Err(MarkError::BatchPending { batch_id: batch.id });
        }
        if self.items.is_empty() {
            self.used_requests.insert(request_id.to_owned(), 0);
            return Ok(TakeOutcome {
                batch_id: None,
                count: 0,
                replayed: false,
            });
        }
        let id = self.allocate_batch_id();
        let mut items = std::mem::take(&mut self.items);
        for mark in &mut items {
            mark.attached = false;
        }
        let count = items.len();
        self.last_taken = Some(MarkBatch {
            id,
            request_id: request_id.to_owned(),
            items,
            acknowledged: false,
        });
        self.used_requests.insert(request_id.to_owned(), id);
        self.last_removed.clear();
        self.restore_source = RestoreSource::Taken(id);
        self.baselines.clear();
        self.touch_collection();
        Ok(TakeOutcome {
            batch_id: Some(id),
            count,
            replayed: false,
        })
    }

    pub fn ack(&mut self, batch_id: u64) -> Result<bool, MarkError> {
        let batch = self
            .last_taken
            .as_mut()
            .filter(|batch| batch.id == batch_id)
            .ok_or(MarkError::UnknownBatch(batch_id))?;
        let changed = !batch.acknowledged;
        batch.acknowledged = true;
        Ok(changed)
    }

    pub fn restore(&mut self, batch_id: Option<u64>) -> Result<usize, MarkError> {
        let source = match batch_id {
            Some(id) => {
                &self
                    .batch(Some(id))
                    .ok_or(MarkError::UnknownBatch(id))?
                    .items
            }
            None => self.restore_items(),
        };
        let current_ids: HashSet<_> = self.items.iter().map(|mark| mark.id).collect();
        let mut restored: Vec<_> = source
            .iter()
            .filter(|mark| !current_ids.contains(&mark.id))
            .cloned()
            .collect();
        for mark in &mut restored {
            mark.attached = false;
        }
        let count = restored.len();
        self.items.extend(restored);
        if count > 0 {
            self.touch_collection();
        }
        Ok(count)
    }

    pub fn restore_count(&self) -> usize {
        let ids: HashSet<_> = self.items.iter().map(|mark| mark.id).collect();
        self.restore_items()
            .iter()
            .filter(|mark| !ids.contains(&mark.id))
            .count()
    }

    pub fn can_restore(&self) -> bool {
        self.restore_count() > 0
    }

    pub fn snippet_chunk(
        &self,
        id: u64,
        batch_id: Option<u64>,
        offset: usize,
        count: usize,
    ) -> Result<SnippetChunk, MarkError> {
        if let Some(batch_id) = batch_id.filter(|id| self.batch(Some(*id)).is_none()) {
            return Err(MarkError::UnknownBatch(batch_id));
        }
        let mark = self.get(id, batch_id).ok_or(MarkError::UnknownMark(id))?;
        let total_chars = mark.snippet_chars();
        if count == 0 || count > 65_536 || offset > total_chars {
            return Err(MarkError::InvalidChunk);
        }
        let end = offset.saturating_add(count).min(total_chars);
        Ok(SnippetChunk {
            text: mark.snippet_slice(offset, end),
            offset,
            total_chars,
            next_offset: (end < total_chars).then_some(end),
        })
    }

    fn restore_items(&self) -> &[UserMark] {
        match self.restore_source {
            RestoreSource::Removed => &self.last_removed,
            RestoreSource::Taken(id) => self.batch(Some(id)).map_or(&[], |batch| &batch.items),
        }
    }

    fn ensure_baseline(
        &mut self,
        document: MarkDocument<'_>,
        text: &str,
        revision: u64,
        fingerprint: DocumentFingerprint,
    ) {
        self.baselines
            .entry(document.id)
            .or_insert_with(|| Baseline {
                revision,
                text: text.to_owned(),
                fingerprint,
                collection_revision: self.collection_revision,
                target: owned_target(document),
            });
    }

    fn touch_collection(&mut self) {
        self.collection_revision = self.collection_revision.wrapping_add(1);
    }

    fn allocate_id(&mut self) -> u64 {
        loop {
            let id = self.next_id.max(1);
            self.next_id = id.wrapping_add(1).max(1);
            if !self
                .items
                .iter()
                .chain(&self.last_removed)
                .chain(self.last_taken.iter().flat_map(|batch| &batch.items))
                .any(|mark| mark.id == id)
            {
                return id;
            }
        }
    }

    fn allocate_batch_id(&mut self) -> u64 {
        loop {
            let id = self.next_batch_id.max(1);
            self.next_batch_id = id.wrapping_add(1).max(1);
            if !self.used_requests.values().any(|used| *used == id) {
                return id;
            }
        }
    }
}

fn validate_note(note: &str) -> Result<(), MarkError> {
    if note.chars().take(MAX_NOTE_CHARS + 1).count() > MAX_NOTE_CHARS {
        Err(MarkError::NoteTooLong)
    } else {
        Ok(())
    }
}

fn document_matches(mark: &UserMark, document: MarkDocument<'_>, known_document: bool) -> bool {
    if known_document && mark.runtime_identity && mark.document_id == document.id {
        return true;
    }
    match (mark.target.path.as_deref(), document.path) {
        (Some(mark_path), Some(document_path)) => {
            path_identity(mark_path) == path_identity(document_path)
                && path_identity(&mark.target.workspace) == path_identity(document.workspace)
        }
        (None, None) => known_document && mark.runtime_identity && mark.document_id == document.id,
        _ => false,
    }
}

fn target_matches_descriptor(target: &MarkTarget, document: MarkDocument<'_>) -> bool {
    target.workspace == document.workspace
        && target.path.as_deref() == document.path
        && target.title == document.title
}

fn owned_target(document: MarkDocument<'_>) -> MarkTarget {
    MarkTarget {
        workspace: document.workspace.to_path_buf(),
        path: document.path.map(Path::to_path_buf),
        title: document.title.to_owned(),
    }
}

fn path_identity(path: &Path) -> String {
    let display = crate::paths::display(path);
    #[cfg(windows)]
    {
        display.replace('/', "\\").to_lowercase()
    }
    #[cfg(not(windows))]
    {
        display
    }
}

fn inspect_document(text: &str) -> (DocumentFingerprint, Vec<usize>) {
    let hash = text.bytes().fold(0xcbf29ce484222325u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    });
    let mut chars = 0;
    let mut lines = vec![0];
    for character in text.chars() {
        chars += 1;
        if character == '\n' {
            lines.push(chars);
        }
    }
    (
        DocumentFingerprint {
            bytes: text.len(),
            chars,
            hash,
        },
        lines,
    )
}

fn line_offsets(text: &str) -> Vec<usize> {
    let mut lines = vec![0];
    for (index, character) in text.chars().enumerate() {
        if character == '\n' {
            lines.push(index + 1);
        }
    }
    lines
}

fn position(lines: &[usize], index: usize) -> MarkPosition {
    let row = lines
        .partition_point(|start| *start <= index)
        .saturating_sub(1);
    MarkPosition {
        line: row + 1,
        column: index - lines[row] + 1,
    }
}

fn char_slice(text: &str, start: usize, end: usize) -> String {
    text.chars()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect()
}

fn verified_range(mark: &UserMark, current_chars: usize) -> bool {
    mark.range.start < mark.range.end && mark.range.end <= current_chars
}

fn changed_range(
    old: &str,
    old_len: usize,
    new: &str,
    new_len: usize,
) -> (Range<usize>, Range<usize>) {
    let prefix = old
        .chars()
        .zip(new.chars())
        .take_while(|(left, right)| left == right)
        .count();
    let suffix = old
        .chars()
        .rev()
        .zip(new.chars().rev())
        .take((old_len - prefix).min(new_len - prefix))
        .take_while(|(left, right)| left == right)
        .count();
    (prefix..old_len - suffix, prefix..new_len - suffix)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document(id: u64) -> MarkDocument<'static> {
        MarkDocument {
            id,
            workspace: Path::new("C:/project"),
            path: Some(Path::new("C:/project/plan.md")),
            title: "plan.md",
        }
    }

    fn mark(store: &mut AgentMarks, text: &str, range: Range<usize>) -> u64 {
        store.add(document(1), text, 1, range, "").unwrap()
    }

    #[test]
    fn capture_owns_exact_unicode_multiline_crlf_snapshot_and_exclusive_positions() {
        let text = "a🦊\r\nbé\r\nc";
        let mut store = AgentMarks::default();
        let id = mark(&mut store, text, 1..6);
        let item = store.get(id, None).unwrap();
        assert_eq!(item.snippet, "🦊\r\nbé");
        assert_eq!(item.range, CharRange { start: 1, end: 6 });
        assert_eq!(item.start, MarkPosition { line: 1, column: 2 });
        assert_eq!(item.end, MarkPosition { line: 2, column: 3 });
        assert_eq!(store.visible_marks(1).count(), 1);
    }

    #[test]
    fn outside_edits_rebase_unicode_ranges_and_line_positions() {
        let mut store = AgentMarks::default();
        let id = mark(&mut store, "before\n🦊hello\nafter", 8..13);
        assert!(store.synchronize_document(document(1), "new\nbefore\n🦊hello\nafter", 2));
        let item = store.get(id, None).unwrap();
        assert_eq!(item.snippet, "hello");
        assert_eq!(item.range, CharRange { start: 12, end: 17 });
        assert_eq!(item.start, MarkPosition { line: 3, column: 2 });
        assert!(!item.stale);
        assert!(store.synchronize_document(document(1), "new\nbefore\n🦊hello\ntail", 3));
        assert_eq!(store.get(id, None).unwrap().range.start, 12);
        assert_eq!(store.visible_marks(1).count(), 1);
    }

    #[test]
    fn insertion_at_start_moves_mark_and_at_end_keeps_its_snapshot() {
        let mut store = AgentMarks::default();
        let id = mark(&mut store, "abcDEFghi", 3..6);
        store.synchronize_document(document(1), "abc🦊DEFghi", 2);
        assert_eq!(
            store.get(id, None).unwrap().range,
            CharRange { start: 4, end: 7 }
        );
        store.synchronize_document(document(1), "abc🦊DEF!ghi", 3);
        assert_eq!(
            store.get(id, None).unwrap().range,
            CharRange { start: 4, end: 7 }
        );
        assert_eq!(store.visible_marks(1).count(), 1);
    }

    #[test]
    fn overlapping_edit_disables_old_highlight_and_retains_captured_text() {
        let mut store = AgentMarks::default();
        let id = mark(&mut store, "abcDEFghi", 3..6);
        store.synchronize_document(document(1), "abcDxyFghi", 2);
        assert_eq!(store.visible_marks(1).count(), 0);
        assert!(store.get(id, None).unwrap().stale);
        assert_eq!(store.get(id, None).unwrap().snippet, "DEF");
        store.synchronize_document(document(1), "abcDEFghi", 3);
        assert_eq!(
            store.visible_marks(1).count(),
            1,
            "undo to the verified full document can safely reattach"
        );
    }

    #[test]
    fn disjoint_edits_in_one_revision_are_conservatively_stale() {
        let mut store = AgentMarks::default();
        let id = mark(&mut store, "abcDEFghi", 3..6);
        store.synchronize_document(document(1), "aXcDEFgYi", 2);
        assert!(store.get(id, None).unwrap().stale);
        assert_eq!(store.visible_marks(1).count(), 0);
    }

    #[test]
    fn unchanged_revision_keeps_one_document_baseline_allocation() {
        let mut store = AgentMarks::default();
        mark(&mut store, "abcDEFghi", 3..6);
        store.synchronize_document(document(1), "abcDEFghi", 1);
        let allocation = store.baselines[&1].text.as_ptr();
        for _ in 0..100 {
            assert!(!store.synchronize_document(document(1), "abcDEFghi", 1));
            assert_eq!(store.baselines[&1].text.as_ptr(), allocation);
        }
        store.synchronize_document(document(1), "abcDEFghi", 2);
        assert_eq!(store.baselines[&1].text.as_ptr(), allocation);
    }

    #[test]
    fn restored_file_requires_complete_document_match_before_highlighting() {
        let mut store = AgentMarks::default();
        mark(&mut store, "abcDEFghi", 3..6);
        let saved = serde_json::to_vec(&store).unwrap();
        let mut restored: AgentMarks = serde_json::from_slice(&saved).unwrap();
        assert_eq!(restored.visible_marks(1).count(), 0);
        restored.synchronize_document(document(9), "abcDEFghi!", 1);
        assert_eq!(
            restored.visible_marks(9).count(),
            0,
            "even an unchanged snippet does not prove the document identity"
        );
        restored.synchronize_document(document(9), "abcDEFghi", 2);
        assert_eq!(restored.visible_marks(9).count(), 1);
    }

    #[test]
    fn closed_snapshot_remains_available_but_untitled_restart_never_reattaches_by_id() {
        let mut store = AgentMarks::default();
        let untitled = MarkDocument {
            path: None,
            title: "Untitled",
            ..document(1)
        };
        let id = store.add(untitled, "hello", 1, 0..5, "").unwrap();
        let serialized = serde_json::to_string(&store).unwrap();
        let mut restored: AgentMarks = serde_json::from_str(&serialized).unwrap();
        restored.synchronize_document(untitled, "hello", 1);
        assert_eq!(restored.visible_marks(1).count(), 0);
        assert_eq!(restored.get(id, None).unwrap().snippet, "hello");
        store.detach_document(1);
        assert_eq!(store.visible_marks(1).count(), 0);
        assert_eq!(store.get(id, None).unwrap().snippet, "hello");
    }

    #[test]
    fn clear_restore_merges_new_marks_without_overwriting_their_notes() {
        let mut store = AgentMarks::default();
        let old = mark(&mut store, "abcDEFghi", 3..6);
        assert_eq!(store.clear(), 1);
        let new = mark(&mut store, "abcDEFghi", 0..3);
        assert_eq!(store.restore(None).unwrap(), 1);
        store.set_note(old, "updated").unwrap();
        assert_eq!(store.restore(None).unwrap(), 0);
        assert_eq!(store.get(old, None).unwrap().note, "updated");
        assert_eq!(
            store.items.iter().map(|mark| mark.id).collect::<Vec<_>>(),
            vec![new, old]
        );
        store.synchronize_document(document(1), "abcDEFghi", 1);
        assert_eq!(store.visible_marks(1).count(), 2);
    }

    #[test]
    fn consumed_receipt_survives_restart_and_same_request_never_consumes_new_marks() {
        let mut store = AgentMarks::default();
        let old = mark(&mut store, "abcDEFghi", 3..6);
        let first = store.take("agent-request-one").unwrap();
        assert_eq!(first.count, 1);
        assert!(store.items.is_empty());
        let mut restored: AgentMarks =
            serde_json::from_slice(&serde_json::to_vec(&store).unwrap()).unwrap();
        let new = mark(&mut restored, "abcDEFghi", 0..3);
        let retry = restored.take("agent-request-one").unwrap();
        assert_eq!(retry.batch_id, first.batch_id);
        assert!(retry.replayed);
        assert_eq!(restored.items[0].id, new);
        assert_eq!(restored.batch(first.batch_id).unwrap().items[0].id, old);
        assert!(matches!(
            restored.take("agent-request-two"),
            Err(MarkError::BatchPending { .. })
        ));
        assert_eq!(restored.items[0].id, new);
    }

    #[test]
    fn acknowledged_receipt_can_be_restored_and_old_request_tombstone_is_safe() {
        let mut store = AgentMarks::default();
        let old = mark(&mut store, "abcDEFghi", 3..6);
        let batch = store.take("first").unwrap().batch_id.unwrap();
        assert_eq!(store.restore_count(), 1);
        assert!(store.ack(batch).unwrap());
        assert!(!store.ack(batch).unwrap());
        let new = mark(&mut store, "abcDEFghi", 0..3);
        assert_eq!(store.restore(None).unwrap(), 1);
        assert_eq!(
            store.items.iter().map(|mark| mark.id).collect::<Vec<_>>(),
            vec![new, old]
        );
        store.take("second").unwrap();
        mark(&mut store, "abcDEFghi", 6..9);
        assert!(
            matches!(store.take("first"), Err(MarkError::ReceiptExpired { batch_id }) if batch_id == batch)
        );
        assert_eq!(store.items.len(), 1);
    }

    #[test]
    fn empty_take_never_replays_an_unrelated_old_batch() {
        let mut store = AgentMarks::default();
        assert_eq!(store.take("empty").unwrap().batch_id, None);
        mark(&mut store, "hello", 0..5);
        let batch = store.take("first").unwrap().batch_id.unwrap();
        store.ack(batch).unwrap();
        let empty = store.take("fresh").unwrap();
        assert_eq!(empty.batch_id, None);
        assert_eq!(empty.count, 0);
        assert_eq!(store.batch(None).unwrap().id, batch);
    }

    #[test]
    fn retry_of_empty_take_never_consumes_new_marks_and_unicode_tokens_are_valid() {
        let mut store = AgentMarks::default();
        assert_eq!(store.take("Auftrag 🦊").unwrap().batch_id, None);
        mark(&mut store, "hello", 0..5);
        let retry = store.take("Auftrag 🦊").unwrap();
        assert!(retry.replayed);
        assert_eq!(retry.batch_id, None);
        assert_eq!(store.items.len(), 1);
        let mut restored: AgentMarks =
            serde_json::from_slice(&serde_json::to_vec(&store).unwrap()).unwrap();
        assert!(restored.take("Auftrag 🦊").unwrap().replayed);
        assert_eq!(restored.items.len(), 1);
    }

    #[test]
    fn save_as_and_rename_retarget_verified_open_buffer_without_reusing_old_path() {
        let mut store = AgentMarks::default();
        let original = MarkDocument {
            path: None,
            title: "Untitled",
            ..document(1)
        };
        let id = store.add(original, "hello", 1, 0..5, "").unwrap();
        assert!(store.synchronize_document(document(1), "hello", 1));
        assert_eq!(
            store.get(id, None).unwrap().target.path.as_deref(),
            document(1).path
        );
        let renamed = MarkDocument {
            path: Some(Path::new("C:/project/renamed.md")),
            title: "renamed.md",
            ..document(1)
        };
        assert!(store.synchronize_document(renamed, "hello", 1));
        assert_eq!(
            store.get(id, None).unwrap().target.path.as_deref(),
            renamed.path
        );
        assert_eq!(store.visible_marks(1).count(), 1);
    }

    #[test]
    fn removing_last_mark_still_allows_restore_and_duplicate_notes_are_not_overwritten() {
        let mut store = AgentMarks::default();
        let id = mark(&mut store, "hello", 0..5);
        assert!(store.remove(id));
        assert!(!store.remove(id));
        assert_eq!(store.restore_count(), 1);
        assert_eq!(store.restore(None).unwrap(), 1);
        store.set_note(id, "changed after restore").unwrap();
        assert_eq!(store.restore(None).unwrap(), 0);
        assert_eq!(store.get(id, None).unwrap().note, "changed after restore");
    }

    #[test]
    fn restoring_open_untitled_marks_keeps_live_identity_without_persisting_it() {
        let mut store = AgentMarks::default();
        let untitled = MarkDocument {
            path: None,
            title: "Untitled",
            ..document(1)
        };
        store.add(untitled, "hello", 1, 0..5, "").unwrap();
        store.clear();
        store.synchronize_document(untitled, "hello", 1);
        store.restore(None).unwrap();
        store.synchronize_document(untitled, "hello", 1);
        assert_eq!(store.visible_marks(1).count(), 1);
        let serialized = serde_json::to_vec(&store).unwrap();
        let mut restored: AgentMarks = serde_json::from_slice(&serialized).unwrap();
        restored.synchronize_document(untitled, "hello", 1);
        assert_eq!(restored.visible_marks(1).count(), 0);
    }

    #[test]
    fn new_untitled_capture_after_restart_never_attaches_a_reused_runtime_id() {
        let mut store = AgentMarks::default();
        let untitled = MarkDocument {
            path: None,
            title: "Untitled",
            ..document(1)
        };
        let old = store.add(untitled, "hello", 1, 0..5, "").unwrap();
        let mut restored: AgentMarks =
            serde_json::from_slice(&serde_json::to_vec(&store).unwrap()).unwrap();
        let new = restored.add(untitled, "hello", 1, 0..5, "").unwrap();
        restored.synchronize_document(untitled, "hello", 1);
        assert_ne!(old, new);
        assert_eq!(
            restored
                .visible_marks(1)
                .map(|mark| mark.id)
                .collect::<Vec<_>>(),
            vec![new]
        );
        assert_eq!(restored.get(old, None).unwrap().snippet, "hello");
    }

    #[test]
    fn closing_untitled_clears_identity_in_current_removed_and_taken_snapshots() {
        let untitled = MarkDocument {
            path: None,
            title: "Untitled",
            ..document(1)
        };
        for storage in 0..3 {
            let mut store = AgentMarks::default();
            let old = store.add(untitled, "hello", 1, 0..5, "").unwrap();
            match storage {
                1 => {
                    store.clear();
                }
                2 => {
                    store.take("receipt").unwrap();
                }
                _ => {}
            }
            let receipt_before = store
                .last_taken
                .as_ref()
                .map(|batch| serde_json::to_value(batch).unwrap());
            store.detach_document(1);
            let item = match storage {
                1 => &store.last_removed[0],
                2 => &store.last_taken.as_ref().unwrap().items[0],
                _ => &store.items[0],
            };
            assert!(!item.has_runtime_identity());
            assert!(!item.is_attached());
            assert_eq!(
                store
                    .last_taken
                    .as_ref()
                    .map(|batch| serde_json::to_value(batch).unwrap()),
                receipt_before,
                "closing a buffer must not alter its frozen receipt"
            );
            if storage != 0 {
                store.restore(None).unwrap();
            }
            store.synchronize_document(untitled, "hello", 1);
            assert_eq!(store.visible_marks(1).count(), 0);
            let new = store.add(untitled, "hello", 1, 0..5, "").unwrap();
            store.synchronize_document(untitled, "hello", 1);
            assert_eq!(
                store
                    .visible_marks(1)
                    .map(|item| item.id)
                    .collect::<Vec<_>>(),
                vec![new]
            );
            assert!(!store.get(old, None).unwrap().has_runtime_identity());
        }
    }

    #[test]
    fn stale_untitled_identity_is_live_only_until_close_or_restart() {
        let mut store = AgentMarks::default();
        let untitled = MarkDocument {
            path: None,
            title: "Untitled",
            ..document(1)
        };
        let id = store.add(untitled, "hello", 1, 0..5, "").unwrap();
        store.synchronize_document(untitled, "changed", 2);
        let item = store.get(id, None).unwrap();
        assert!(item.stale);
        assert!(item.has_runtime_identity());
        assert!(!item.is_attached());
        let restored: AgentMarks =
            serde_json::from_slice(&serde_json::to_vec(&store).unwrap()).unwrap();
        assert!(!restored.get(id, None).unwrap().has_runtime_identity());
        store.detach_document(1);
        assert!(!store.get(id, None).unwrap().has_runtime_identity());
    }

    #[test]
    fn durable_ack_and_replayed_take_preserve_exact_baselines_without_text_clone() {
        for replay in [false, true] {
            let mut store = AgentMarks::default();
            mark(&mut store, "abcDEFghi", 3..6);
            let batch_id = store.take("first").unwrap().batch_id.unwrap();
            let pending = mark(&mut store, "abcDEFghi", 3..6);
            let allocation = store.baselines[&1].text.as_ptr();
            let mut staged = store.clone();
            assert!(staged.baselines.is_empty());
            if replay {
                assert!(staged.take("first").unwrap().replayed);
            } else {
                staged.ack(batch_id).unwrap();
            }
            staged.preserve_live_caches_from(&mut store);
            assert!(store.baselines.is_empty());
            assert_eq!(staged.baselines[&1].text.as_ptr(), allocation);
            staged.synchronize_document(document(1), "new\nabcDEFghi", 2);
            let mark = staged.get(pending, None).unwrap();
            assert!(!mark.stale);
            assert_eq!(mark.range, CharRange { start: 7, end: 10 });
            assert_eq!(mark.snippet, "DEF");
            assert_eq!(staged.visible_marks(1).count(), 1);
        }
    }

    #[test]
    fn durable_clear_drops_baselines_when_no_current_mark_survives() {
        let mut store = AgentMarks::default();
        mark(&mut store, "hello", 0..5);
        let mut staged = store.clone();
        staged.clear();
        staged.preserve_live_caches_from(&mut store);
        assert!(staged.baselines.is_empty());
        assert!(store.baselines.is_empty());
        assert_eq!(staged.last_removed[0].snippet, "hello");
    }

    #[test]
    fn stale_recovered_named_mark_binds_current_id_before_old_id_can_be_reused() {
        let mut store = AgentMarks::default();
        let original = mark(&mut store, "hello", 0..5);
        let mut restored: AgentMarks =
            serde_json::from_slice(&serde_json::to_vec(&store).unwrap()).unwrap();
        restored.synchronize_document(document(9), "changed", 1);
        let mark = restored.get(original, None).unwrap();
        assert!(mark.stale);
        assert!(mark.has_runtime_identity());
        assert_eq!(mark.document_id, 9);
        let unrelated = MarkDocument {
            path: Some(Path::new("C:/project/other.md")),
            title: "other.md",
            ..document(1)
        };
        restored.add(unrelated, "hello", 1, 0..5, "").unwrap();
        restored.synchronize_document(unrelated, "hello", 1);
        let mark = restored.get(original, None).unwrap();
        assert_eq!(mark.document_id, 9);
        assert_eq!(mark.target.path.as_deref(), document(9).path);
        assert!(mark.stale);
        assert!(!mark.is_attached());
        assert_eq!(restored.visible_marks(1).count(), 1);
    }

    #[test]
    fn unlimited_mark_count_and_chunked_unicode_snapshot_never_drop_content() {
        let mut store = AgentMarks::default();
        let snippet = "🦊é\n".repeat(30_000);
        let count = snippet.chars().count();
        let id = mark(&mut store, &snippet, 0..count);
        let batch = store.take("large").unwrap().batch_id.unwrap();
        let mut recovered = String::new();
        let mut offset = 0;
        loop {
            let chunk = store.snippet_chunk(id, Some(batch), offset, 4096).unwrap();
            recovered.push_str(&chunk.text);
            match chunk.next_offset {
                Some(next) => offset = next,
                None => break,
            }
        }
        assert_eq!(recovered, snippet);
        store.ack(batch).unwrap();
        for _ in 0..1001 {
            mark(&mut store, "x", 0..1);
        }
        let next = store.take("many").unwrap();
        assert_eq!(next.count, 1001);
        assert_eq!(store.batch(next.batch_id).unwrap().items.len(), 1001);
    }

    #[test]
    fn invalid_capture_note_and_chunk_requests_leave_collection_unchanged() {
        let mut store = AgentMarks::default();
        for range in [0..0, Range { start: 2, end: 1 }, 0..9] {
            assert_eq!(
                store.add(document(1), "hello", 1, range, ""),
                Err(MarkError::InvalidRange)
            );
        }
        assert!(store.items.is_empty());
        let id = store
            .add(document(1), "hello", 1, 0..5, &"🦊".repeat(MAX_NOTE_CHARS))
            .unwrap();
        assert_eq!(
            store.set_note(id, &"🦊".repeat(MAX_NOTE_CHARS + 1)),
            Err(MarkError::NoteTooLong)
        );
        assert_eq!(
            store.get(id, None).unwrap().note.chars().count(),
            MAX_NOTE_CHARS
        );
        assert_eq!(
            store.snippet_chunk(id, None, 6, 1),
            Err(MarkError::InvalidChunk)
        );
        assert_eq!(
            store.snippet_chunk(id, None, 0, 0),
            Err(MarkError::InvalidChunk)
        );
        assert_eq!(store.take("\n"), Err(MarkError::InvalidRequestId));
        assert_eq!(store.items.len(), 1);
    }
}

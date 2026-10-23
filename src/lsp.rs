//! A small asynchronous Language Server Protocol client. The UI only queues document
//! updates and polls events; process I/O, initialization and timeouts stay on
//! background threads. Positions use the LSP's negotiated UTF-16 encoding.

use eframe::egui;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

mod codec;
mod protocol;
mod replies;
mod runtime;

use codec::{
    canonical_path, document_end_position, normalize_path, path_to_uri, read_message, uri_to_path,
    write_message,
};
pub use codec::{char_to_position, position_to_char};
use protocol::client_capabilities;
#[cfg(test)]
use protocol::configuration_section;
pub use replies::symbol_kind_label;
#[cfg(test)]
use replies::{PREVIEW_CHARS, line_preview, plain_snippet};
use replies::{
    build_references, parse_completions, parse_definition, parse_diagnostics, parse_hover,
    parse_locations, parse_symbols, read_preview_source,
};
use runtime::{EventEmitter, run_server};
#[cfg(test)]
use runtime::{Inbound, send_inbound};

static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);

const MESSAGE_LIMIT: usize = 16 * 1024 * 1024;
const HEADER_LIMIT: usize = 8 * 1024;
const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
const BACKPRESSURE_RETRY_INTERVAL: Duration = Duration::from_millis(20);
const PROCESS_CHECK_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextRange {
    pub start: Position,
    pub end: Position,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub range: TextRange,
    /// LSP severity: 1 error, 2 warning, 3 information, 4 hint.
    pub severity: u8,
    pub message: String,
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
    pub range: TextRange,
    pub new_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionItem {
    pub label: String,
    pub detail: Option<String>,
    pub insert_text: String,
    pub text_edit: Option<TextEdit>,
    /// Usually imports required by the selected completion.
    pub additional_text_edits: Vec<TextEdit>,
}

/// Hover text as the server sent it. Plain text is never parsed as Markdown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoverContents {
    pub markdown: bool,
    pub text: String,
}

/// One reference with a preview of its source line. `line` and `column` are
/// one-based Unicode scalar coordinates in the text the server analyzed; the
/// UTF-16 `position` remains authoritative when a buffer changed since then.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    pub path: PathBuf,
    pub position: Position,
    pub line: usize,
    pub column: usize,
    /// The trimmed source line, or empty when the file could not be read.
    pub preview: String,
    /// The referenced characters within `preview`, in Unicode scalars.
    pub highlight: std::ops::Range<usize>,
}

/// A document or workspace symbol. Document symbols carry no path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub detail: Option<String>,
    /// LSP SymbolKind, 1 (File) to 26 (TypeParameter).
    pub kind: u32,
    /// The enclosing symbol or container name, if any.
    pub container: Option<String>,
    pub path: Option<PathBuf>,
    pub position: Position,
    /// Nesting depth of hierarchical document symbols.
    pub depth: usize,
}

/// Optional requests a server may or may not provide. Their availability is
/// known once the server has initialized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    Hover,
    References,
    DocumentSymbols,
    WorkspaceSymbols,
}

impl Feature {
    fn bit(self) -> u32 {
        match self {
            Self::Hover => 1,
            Self::References => 2,
            Self::DocumentSymbols => 4,
            Self::WorkspaceSymbols => 8,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Hover => "hover information",
            Self::References => "references",
            Self::DocumentSymbols => "document symbols",
            Self::WorkspaceSymbols => "workspace symbols",
        }
    }

    fn provided(self, capabilities: &Value) -> bool {
        let value = &capabilities[match self {
            Self::Hover => "hoverProvider",
            Self::References => "referencesProvider",
            Self::DocumentSymbols => "documentSymbolProvider",
            Self::WorkspaceSymbols => "workspaceSymbolProvider",
        }];
        value == &Value::Bool(true) || value.is_object()
    }
}

const ALL_FEATURES: [Feature; 4] = [
    Feature::Hover,
    Feature::References,
    Feature::DocumentSymbols,
    Feature::WorkspaceSymbols,
];
const REFERENCE_LIMIT: usize = 2000;
const SYMBOL_LIMIT: usize = 5000;
const HOVER_LIMIT: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub enum LspEvent {
    Ready,
    Diagnostics {
        path: PathBuf,
        version: Option<i32>,
        diagnostics: Vec<Diagnostic>,
    },
    Completions {
        request_id: u64,
        path: PathBuf,
        version: i32,
        position: Position,
        items: Vec<CompletionItem>,
    },
    Definition {
        request_id: u64,
        path: PathBuf,
        line: u32,
        column: u32,
    },
    DefinitionNotFound {
        request_id: u64,
    },
    /// `None` when the server has nothing to show at that position.
    Hover {
        request_id: u64,
        contents: Option<HoverContents>,
    },
    /// Locations in server order. `truncated` when more than the limit existed.
    References {
        request_id: u64,
        references: Vec<Reference>,
        truncated: bool,
    },
    /// Document symbols in document order, or workspace symbols in server order.
    Symbols {
        request_id: u64,
        symbols: Vec<Symbol>,
    },
    /// The request ended without a successful response. Cancelled or stale
    /// requests carry no user-facing error, but still clear pending UI state.
    RequestFailed {
        request_id: u64,
        error: Option<String>,
    },
    Error(String),
}

enum ClientCommand {
    Open {
        path: PathBuf,
        text: String,
    },
    Change {
        path: PathBuf,
        text: String,
        version: i32,
    },
    Close(PathBuf),
    Save(PathBuf),
    Completion {
        id: u64,
        path: PathBuf,
        position: Position,
        version: i32,
    },
    Definition {
        id: u64,
        path: PathBuf,
        position: Position,
    },
    Hover {
        id: u64,
        path: PathBuf,
        position: Position,
    },
    References {
        id: u64,
        path: PathBuf,
        position: Position,
    },
    DocumentSymbols {
        id: u64,
        path: PathBuf,
    },
    WorkspaceSymbols {
        id: u64,
        query: String,
    },
}

/// What an application test can observe about a queued request.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestRequest {
    Hover { id: u64, position: Position },
    References { id: u64, position: Position },
    DocumentSymbols { id: u64 },
    WorkspaceSymbols { id: u64, query: String },
    Other,
}

/// Process and protocol settings for one language server. Arguments are passed
/// directly to the executable, never interpolated into a shell command.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerConfig {
    pub name: String,
    pub program: String,
    pub args: Vec<String>,
    pub language_id: String,
    pub initialization_options: Value,
    /// Workspace settings including section names, e.g. {"python": {...}}.
    pub settings: Value,
    pub install_hint: String,
}

impl ServerConfig {
    #[cfg(test)]
    pub fn rust() -> Self {
        Self {
            name: "Rust analysis".into(),
            program: "rust-analyzer".into(),
            args: Vec::new(),
            language_id: "rust".into(),
            initialization_options: configuration(),
            settings: json!({"rust-analyzer": configuration()}),
            install_hint: "Install rust-analyzer with rustup component add rust-analyzer.".into(),
        }
    }
}

pub struct LanguageServer {
    pub receiver: Receiver<LspEvent>,
    commands: SyncSender<ClientCommand>,
    wake: SyncSender<()>,
    stopped: Arc<AtomicBool>,
    shutdown_requested: Arc<AtomicBool>,
    /// Bits of the optional features the initialized server provides.
    features: Arc<AtomicU32>,
    /// Distinguishes this process from a restarted server for the same language.
    instance: u64,
}

static NEXT_INSTANCE: AtomicU64 = AtomicU64::new(1);

/// Commands accepted by a [`LanguageServer::test_channel_with_requests`] fake.
#[cfg(test)]
pub struct TestRequests(Receiver<ClientCommand>);

#[cfg(test)]
impl TestRequests {
    fn next(&self) -> Option<TestRequest> {
        Some(match self.0.try_recv().ok()? {
            ClientCommand::Hover { id, position, .. } => TestRequest::Hover { id, position },
            ClientCommand::References { id, position, .. } => {
                TestRequest::References { id, position }
            }
            ClientCommand::DocumentSymbols { id, .. } => TestRequest::DocumentSymbols { id },
            ClientCommand::WorkspaceSymbols { id, query } => {
                TestRequest::WorkspaceSymbols { id, query }
            }
            _ => TestRequest::Other,
        })
    }

    /// The next request other than document synchronization.
    pub fn next_request(&self) -> Option<TestRequest> {
        std::iter::from_fn(|| self.next()).find(|request| *request != TestRequest::Other)
    }
}

impl LanguageServer {
    /// Event-only fake for application integration tests. No process, worker or
    /// filesystem access is needed, and its stopped state initially is false.
    #[cfg(test)]
    pub fn test_channel() -> (Self, SyncSender<LspEvent>) {
        let (server, events, _requests) = Self::test_channel_with_requests();
        (server, events)
    }

    /// Like [`Self::test_channel`], but it accepts requests and lets the test
    /// observe them. Every optional feature is initially available.
    #[cfg(test)]
    pub fn test_channel_with_requests() -> (Self, SyncSender<LspEvent>, TestRequests) {
        let (commands, command_rx) = mpsc::sync_channel(64);
        let (wake, _wake_rx) = mpsc::sync_channel(1);
        let (events, receiver) = mpsc::sync_channel(64);
        (
            Self {
                receiver,
                commands,
                wake,
                stopped: Arc::new(AtomicBool::new(false)),
                shutdown_requested: Arc::new(AtomicBool::new(false)),
                features: Arc::new(AtomicU32::new(
                    ALL_FEATURES.iter().map(|feature| feature.bit()).sum(),
                )),
                instance: NEXT_INSTANCE.fetch_add(1, Ordering::Relaxed),
            },
            events,
            TestRequests(command_rx),
        )
    }

    #[cfg(test)]
    pub fn set_test_feature(&self, feature: Feature, available: bool) {
        if available {
            self.features.fetch_or(feature.bit(), Ordering::AcqRel);
        } else {
            self.features.fetch_and(!feature.bit(), Ordering::AcqRel);
        }
    }

    /// Start without waiting for rust-analyzer or its project index. A missing
    /// server is reported as an Error event; ordinary editing remains usable.
    #[cfg(test)]
    pub fn start(root: PathBuf, ctx: Option<egui::Context>) -> Self {
        Self::start_configured(root, ServerConfig::rust(), ctx)
    }

    pub fn start_configured(
        root: PathBuf,
        config: ServerConfig,
        ctx: Option<egui::Context>,
    ) -> Self {
        let (command_tx, command_rx) = mpsc::sync_channel(128);
        let (wake, wake_rx) = mpsc::sync_channel(1);
        let worker_wake = wake.clone();
        let (event_tx, receiver) = mpsc::sync_channel(128);
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_stopped = Arc::clone(&stopped);
        let emitter = EventEmitter::new(event_tx, ctx);
        let shutdown_requested = Arc::clone(&emitter.shutdown_requested);
        let fallback = emitter.clone();
        let server_name = config.name.clone();
        let features = Arc::new(AtomicU32::new(0));
        let worker_features = Arc::clone(&features);
        if let Err(error) = thread::Builder::new()
            .name("vedit-language-server".into())
            .spawn(move || {
                let result = run_server(
                    root,
                    config,
                    command_rx,
                    &worker_wake,
                    wake_rx,
                    &emitter,
                    &worker_stopped,
                    worker_features,
                );
                let manually_stopped = worker_stopped.swap(true, Ordering::AcqRel);
                if let Err(error) = result
                    && !manually_stopped
                {
                    emitter.emit(LspEvent::Error(error));
                }
            })
        {
            stopped.store(true, Ordering::Release);
            fallback.emit(LspEvent::Error(format!(
                "Could not start {server_name}: {error}"
            )));
        }
        Self {
            receiver,
            commands: command_tx,
            wake,
            stopped,
            shutdown_requested,
            features,
            instance: NEXT_INSTANCE.fetch_add(1, Ordering::Relaxed),
        }
    }

    /// Whether the initialized server advertised `feature`. False until Ready.
    pub fn supports(&self, feature: Feature) -> bool {
        self.features.load(Ordering::Acquire) & feature.bit() != 0
    }

    /// A process-unique identity; a restarted language gets a new one.
    pub fn instance(&self) -> u64 {
        self.instance
    }

    /// An opened document starts at version 1.
    pub fn open(&self, path: &Path, text: &str) -> bool {
        self.send(ClientCommand::Open {
            path: path.to_path_buf(),
            text: text.to_owned(),
        })
    }

    /// Supply monotonically increasing versions. Updates queued before
    /// initialization are coalesced, and stale document updates are ignored.
    pub fn change(&self, path: &Path, text: &str, version: i32) -> bool {
        self.send(ClientCommand::Change {
            path: path.to_path_buf(),
            text: text.to_owned(),
            version,
        })
    }

    pub fn close(&self, path: &Path) -> bool {
        self.send(ClientCommand::Close(path.to_path_buf()))
    }
    pub fn save(&self, path: &Path) -> bool {
        self.send(ClientCommand::Save(path.to_path_buf()))
    }

    /// A zero request ID means the queue was unavailable. Match both this ID
    /// and document version before applying a completion received later.
    pub fn completion(&self, path: &Path, position: Position, version: i32) -> u64 {
        self.queue_request(|id| ClientCommand::Completion {
            id,
            path: path.to_path_buf(),
            position,
            version,
        })
    }

    pub fn definition(&self, path: &Path, position: Position) -> u64 {
        self.queue_request(|id| ClientCommand::Definition {
            id,
            path: path.to_path_buf(),
            position,
        })
    }

    /// Like the other requests, zero means the queue was unavailable.
    pub fn hover(&self, path: &Path, position: Position) -> u64 {
        self.queue_request(|id| ClientCommand::Hover {
            id,
            path: path.to_path_buf(),
            position,
        })
    }

    pub fn references(&self, path: &Path, position: Position) -> u64 {
        self.queue_request(|id| ClientCommand::References {
            id,
            path: path.to_path_buf(),
            position,
        })
    }

    pub fn document_symbols(&self, path: &Path) -> u64 {
        self.queue_request(|id| ClientCommand::DocumentSymbols {
            id,
            path: path.to_path_buf(),
        })
    }

    pub fn workspace_symbols(&self, query: &str) -> u64 {
        self.queue_request(|id| ClientCommand::WorkspaceSymbols {
            id,
            query: query.to_owned(),
        })
    }

    fn queue_request(&self, command: impl FnOnce(u64) -> ClientCommand) -> u64 {
        let id = NEXT_REQUEST.fetch_add(1, Ordering::Relaxed);
        if self.send(command(id)) { id } else { 0 }
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }
    pub fn shutdown(&self) {
        self.shutdown_requested.store(true, Ordering::Release);
        self.stopped.store(true, Ordering::Release);
        let _ = self.wake.try_send(());
    }

    fn send(&self, command: ClientCommand) -> bool {
        if self.is_stopped() || self.commands.try_send(command).is_err() {
            return false;
        }
        let _ = self.wake.try_send(());
        true
    }
}

impl Drop for LanguageServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct Outbound {
    token: u64,
    message: Value,
}
struct Document {
    text: String,
    version: i32,
    opened: bool,
    dirty: bool,
    saved: bool,
    synced_end: Position,
}
enum RequestKind {
    Initialize,
    Completion {
        path: PathBuf,
        position: Position,
        version: i32,
    },
    Definition,
    Hover,
    References,
    DocumentSymbols,
    WorkspaceSymbols,
}
struct PendingRequest {
    kind: RequestKind,
    deadline: Instant,
}

#[derive(Debug, Clone, Copy, Default)]
struct SyncOptions {
    open_close: bool,
    change: u64,
    save: bool,
    save_include_text: bool,
}

struct Protocol {
    config: ServerConfig,
    sync: SyncOptions,
    supports_completion: bool,
    supports_definition: bool,
    /// Shared with the UI's [`LanguageServer::supports`].
    features: Arc<AtomicU32>,
    failed_requests: VecDeque<LspEvent>,
    root: PathBuf,
    root_uri: String,
    writer: SyncSender<Outbound>,
    queued_writes: VecDeque<Outbound>,
    next_write: u64,
    writes: HashMap<u64, Instant>,
    pending: HashMap<u64, PendingRequest>,
    documents: HashMap<PathBuf, Document>,
    queued_requests: VecDeque<ClientCommand>,
    ready: bool,
}

#[cfg(test)]
fn configuration() -> Value {
    json!({"checkOnSave":true,"cargo":{"targetDir":true},"procMacro":{"enable":true},"cachePriming":{"numThreads":2}})
}

#[cfg(test)]
#[path = "lsp/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "polyglot_live_tests.rs"]
mod polyglot_live_tests;

#[cfg(test)]
#[path = "lsp_intel_tests.rs"]
mod intel_tests;

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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);

const MESSAGE_LIMIT: usize = 16 * 1024 * 1024;
const HEADER_LIMIT: usize = 8 * 1024;
const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(20);

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
    stopped: Arc<AtomicBool>,
    shutdown_requested: Arc<AtomicBool>,
}

impl LanguageServer {
    /// Event-only fake for application integration tests. No process, worker or
    /// filesystem access is needed, and its stopped state initially is false.
    #[cfg(test)]
    pub fn test_channel() -> (Self, SyncSender<LspEvent>) {
        let (commands, _command_rx) = mpsc::sync_channel(8);
        let (events, receiver) = mpsc::sync_channel(64);
        (
            Self {
                receiver,
                commands,
                stopped: Arc::new(AtomicBool::new(false)),
                shutdown_requested: Arc::new(AtomicBool::new(false)),
            },
            events,
        )
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
        let (event_tx, receiver) = mpsc::sync_channel(128);
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_stopped = Arc::clone(&stopped);
        let emitter = EventEmitter::new(event_tx, ctx);
        let shutdown_requested = Arc::clone(&emitter.shutdown_requested);
        let fallback = emitter.clone();
        let server_name = config.name.clone();
        if let Err(error) = thread::Builder::new()
            .name("morrow-language-server".into())
            .spawn(move || {
                let result = run_server(root, config, command_rx, &emitter, &worker_stopped);
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
            stopped,
            shutdown_requested,
        }
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
        let id = NEXT_REQUEST.fetch_add(1, Ordering::Relaxed);
        if self.send(ClientCommand::Completion {
            id,
            path: path.to_path_buf(),
            position,
            version,
        }) {
            id
        } else {
            0
        }
    }

    pub fn definition(&self, path: &Path, position: Position) -> u64 {
        let id = NEXT_REQUEST.fetch_add(1, Ordering::Relaxed);
        if self.send(ClientCommand::Definition {
            id,
            path: path.to_path_buf(),
            position,
        }) {
            id
        } else {
            0
        }
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }
    pub fn shutdown(&self) {
        self.shutdown_requested.store(true, Ordering::Release);
        self.stopped.store(true, Ordering::Release);
    }

    fn send(&self, command: ClientCommand) -> bool {
        !self.is_stopped() && self.commands.try_send(command).is_ok()
    }
}

impl Drop for LanguageServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[derive(Clone)]
struct EventEmitter {
    sender: SyncSender<LspEvent>,
    ctx: Option<egui::Context>,
    /// Distinct from worker termination: a fatal Error must survive until the
    /// UI reads it, even after the worker has marked the server stopped.
    shutdown_requested: Arc<AtomicBool>,
    backpressure_nanos: Arc<AtomicU64>,
}

impl EventEmitter {
    fn new(sender: SyncSender<LspEvent>, ctx: Option<egui::Context>) -> Self {
        Self {
            sender,
            ctx,
            shutdown_requested: Arc::new(AtomicBool::new(false)),
            backpressure_nanos: Arc::new(AtomicU64::new(0)),
        }
    }

    fn take_backpressure(&self) -> Duration {
        Duration::from_nanos(self.backpressure_nanos.swap(0, Ordering::AcqRel))
    }

    fn emit(&self, mut event: LspEvent) {
        // Match canonical workspace/tab keys without doing filesystem work in
        // the UI's event polling path. All normal emitters run on this worker.
        match &mut event {
            LspEvent::Diagnostics { path, .. }
            | LspEvent::Completions { path, .. }
            | LspEvent::Definition { path, .. } => {
                *path = path.canonicalize().unwrap_or_else(|_| normalize_path(path));
            }
            _ => {}
        }
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint();
        }
        // A native dialog can stop UI polling for minutes. Keep every lifecycle,
        // request and diagnostic event instead of silently losing Ready/errors or
        // leaving old diagnostics on screen. Bounded backpressure also limits the
        // reader queue and subprocess pipe. Shutdown remains cancellable and never
        // waits for the UI to drain the channel before ProcessGuard can run.
        let mut blocked_since = None;
        loop {
            if self.shutdown_requested.load(Ordering::Acquire) {
                break;
            }
            match self.sender.try_send(event) {
                Ok(()) | Err(mpsc::TrySendError::Disconnected(_)) => break,
                Err(mpsc::TrySendError::Full(returned)) => {
                    event = returned;
                    blocked_since.get_or_insert_with(Instant::now);
                    thread::sleep(POLL_INTERVAL);
                }
            }
        }
        if let Some(started) = blocked_since {
            let nanos = started.elapsed().as_nanos().min(u64::MAX as u128) as u64;
            self.backpressure_nanos.fetch_add(nanos, Ordering::AcqRel);
        }
        // Wake after enqueue as well: the first wake may have been rendered
        // before try_send made a previously empty queue observable to the UI.
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint();
        }
    }
}

enum Inbound {
    Message(Value),
    Written(u64),
    Error(String),
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

impl SyncOptions {
    fn from_capabilities(capabilities: &Value) -> Self {
        let value = &capabilities["textDocumentSync"];
        if let Some(change) = value.as_u64() {
            return Self {
                open_close: change != 0,
                change,
                ..Self::default()
            };
        }
        let save = &value["save"];
        Self {
            open_close: value["openClose"].as_bool().unwrap_or(false),
            change: value["change"].as_u64().unwrap_or(0),
            save: save == &Value::Bool(true) || save.is_object(),
            save_include_text: save["includeText"].as_bool().unwrap_or(false),
        }
    }
}

fn configuration_section(settings: &Value, section: &str) -> Value {
    if section.is_empty() {
        return settings.clone();
    }
    if let Some(value) = settings.get(section) {
        return value.clone();
    }
    if let Some((first, rest)) = section.split_once('.') {
        return configuration_section(&settings[first], rest);
    }
    Value::Null
}

struct Protocol {
    config: ServerConfig,
    sync: SyncOptions,
    supports_completion: bool,
    supports_definition: bool,
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

fn run_server(
    root: PathBuf,
    config: ServerConfig,
    commands: Receiver<ClientCommand>,
    events: &EventEmitter,
    stopped: &AtomicBool,
) -> Result<(), String> {
    if stopped.load(Ordering::Acquire) {
        return Ok(());
    }
    let root = root
        .canonicalize()
        .map_err(|error| format!("Could not open the language server workspace: {error}"))?;
    let root_uri = path_to_uri(&root)?;
    #[cfg(windows)]
    if Path::new(&config.program)
        .extension()
        .is_some_and(|extension| {
            extension.to_string_lossy().eq_ignore_ascii_case("cmd")
                || extension.to_string_lossy().eq_ignore_ascii_case("bat")
        })
    {
        return Err(format!(
            "{} requires an executable, not a shell script. Use node.exe with the server JavaScript file as an argument, or the SDK executable. {}",
            config.name, config.install_hint
        ));
    }
    let mut command = Command::new(&config.program);
    command
        .args(&config.args)
        .current_dir(&root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let child = command.spawn().map_err(|error| {
        format!(
            "{} could not start. {} {error}",
            config.name, config.install_hint
        )
    })?;
    let mut process = ProcessGuard::new(child);
    let stdin = process
        .child
        .stdin
        .take()
        .ok_or("Language server has no input pipe")?;
    let stdout = process
        .child
        .stdout
        .take()
        .ok_or("Language server has no output pipe")?;
    let stderr = process
        .child
        .stderr
        .take()
        .ok_or("Language server has no error pipe")?;
    let (inbound_tx, inbound_rx) = mpsc::sync_channel(32);
    let (writer_tx, writer_rx) = mpsc::sync_channel::<Outbound>(32);
    let reader_tx = inbound_tx.clone();
    thread::Builder::new()
        .name("morrow-lsp-reader".into())
        .spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                match read_message(&mut reader) {
                    Ok(Some(message)) => {
                        if reader_tx.send(Inbound::Message(message)).is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        let _ = reader_tx.send(Inbound::Error(format!(
                            "Invalid language server response: {error}"
                        )));
                        break;
                    }
                }
            }
        })
        .map_err(|error| format!("Could not read language server: {error}"))?;
    let write_tx = inbound_tx.clone();
    thread::Builder::new()
        .name("morrow-lsp-writer".into())
        .spawn(move || {
            let mut writer = stdin;
            for outbound in writer_rx {
                if let Err(error) = write_message(&mut writer, &outbound.message) {
                    let _ = write_tx.send(Inbound::Error(format!(
                        "Could not update language server: {error}"
                    )));
                    break;
                }
                if write_tx.send(Inbound::Written(outbound.token)).is_err() {
                    break;
                }
            }
        })
        .map_err(|error| format!("Could not write language server: {error}"))?;
    drop(inbound_tx);
    let stderr_tail = Arc::new(Mutex::new(Vec::<u8>::new()));
    let tail = Arc::clone(&stderr_tail);
    thread::Builder::new()
        .name("morrow-lsp-stderr".into())
        .spawn(move || {
            let mut reader = stderr;
            let mut buffer = [0_u8; 4096];
            while let Ok(length) = reader.read(&mut buffer) {
                if length == 0 {
                    break;
                }
                if let Ok(mut tail) = tail.lock() {
                    tail.extend_from_slice(&buffer[..length]);
                    let excess = tail.len().saturating_sub(16 * 1024);
                    if excess > 0 {
                        tail.drain(..excess);
                    }
                }
            }
        })
        .map_err(|error| format!("Could not read language server logs: {error}"))?;

    let mut protocol = Protocol {
        config,
        sync: SyncOptions::default(),
        supports_completion: false,
        supports_definition: false,
        failed_requests: VecDeque::new(),
        root,
        root_uri,
        writer: writer_tx,
        queued_writes: VecDeque::new(),
        next_write: 1,
        writes: HashMap::new(),
        pending: HashMap::new(),
        documents: HashMap::new(),
        queued_requests: VecDeque::new(),
        ready: false,
    };
    protocol.pending.insert(
        0,
        PendingRequest {
            kind: RequestKind::Initialize,
            deadline: Instant::now() + INITIALIZE_TIMEOUT,
        },
    );
    protocol.queue(json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{
        "processId":std::process::id(), "rootUri":protocol.root_uri,
        "clientInfo":{"name":"Morrow","version":env!("CARGO_PKG_VERSION")},
        "workspaceFolders":protocol.workspace_folders(),
        "capabilities":{
            "general":{"positionEncodings":["utf-16"]},
            "workspace":{"configuration":true,"workspaceFolders":true,"applyEdit":false},
            "window":{"workDoneProgress":true},
            "textDocument":{
                "synchronization":{"dynamicRegistration":false,"didSave":true},
                "publishDiagnostics":{"versionSupport":true},
                "completion":{"contextSupport":true,"completionItem":{"snippetSupport":false,"insertReplaceSupport":true}},
                "definition":{"linkSupport":true}
            }
        }, "initializationOptions":protocol.config.initialization_options
    }}))?;

    while !stopped.load(Ordering::Acquire) {
        protocol.extend_deadlines(events.take_backpressure());
        if let Some(status) = process
            .child
            .try_wait()
            .map_err(|error| format!("Could not monitor language server: {error}"))?
        {
            let tail = stderr_tail
                .lock()
                .map(|tail| String::from_utf8_lossy(&tail).trim().to_owned())
                .unwrap_or_default();
            return Err(if tail.is_empty() {
                format!(
                    "{} stopped ({status}). {}",
                    protocol.config.name, protocol.config.install_hint
                )
            } else {
                format!("{} stopped: {tail}", protocol.config.name)
            });
        }
        for _ in 0..128 {
            match commands.try_recv() {
                Ok(command) => protocol.command(command)?,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
            }
        }
        if protocol.ready {
            protocol.flush_documents()?;
        }
        protocol.flush_writes()?;
        match inbound_rx.recv_timeout(POLL_INTERVAL) {
            Ok(Inbound::Message(message)) => protocol.message(message, events)?,
            Ok(Inbound::Written(token)) => {
                protocol.writes.remove(&token);
            }
            Ok(Inbound::Error(error)) => return Err(error),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(format!(
                    "{} disconnected unexpectedly.",
                    protocol.config.name
                ));
            }
        }
        while let Some(event) = protocol.failed_requests.pop_front() {
            events.emit(event);
        }
        protocol.extend_deadlines(events.take_backpressure());
        let now = Instant::now();
        if protocol.writes.values().any(|deadline| *deadline <= now) {
            return Err(format!(
                "{} stopped accepting document updates and was shut down. Editing is still available.",
                protocol.config.name
            ));
        }
        let expired = protocol
            .pending
            .iter()
            .filter_map(|(id, request)| (request.deadline <= now).then_some(*id))
            .collect::<Vec<_>>();
        for id in expired {
            let request = protocol.pending.remove(&id).unwrap();
            if matches!(request.kind, RequestKind::Initialize) {
                return Err(format!(
                    "{} initialization timed out. Check the language server and workspace configuration.",
                    protocol.config.name
                ));
            }
            protocol.notify("$/cancelRequest", json!({"id":id}))?;
            events.emit(LspEvent::RequestFailed {
                request_id: id,
                error: Some(format!(
                    "{} request timed out; the project may still be indexing.",
                    protocol.config.name
                )),
            });
        }
    }
    // Dropping the guard stops the entire language server process tree. Waiting
    // for an LSP shutdown response here would make closing the editor fragile.
    Ok(())
}

#[cfg(test)]
fn configuration() -> Value {
    json!({"checkOnSave":true,"cargo":{"targetDir":true},"procMacro":{"enable":true},"cachePriming":{"numThreads":2}})
}

impl Protocol {
    fn extend_deadlines(&mut self, paused: Duration) {
        // Time spent waiting for the editor to consume events is not a server
        // timeout. The reader/writer acknowledgements are backpressured too.
        if paused.is_zero() {
            return;
        }
        for deadline in self.writes.values_mut() {
            *deadline += paused;
        }
        for request in self.pending.values_mut() {
            request.deadline += paused;
        }
    }

    fn queue(&mut self, message: Value) -> Result<(), String> {
        if self.queued_writes.len() >= 128 {
            return Err(
                "Language server input is overloaded; restart the workspace analysis.".into(),
            );
        }
        let token = self.next_write;
        self.next_write += 1;
        let outbound = Outbound { token, message };
        if self.queued_writes.is_empty() {
            match self.writer.try_send(outbound) {
                Ok(()) => {
                    self.writes.insert(token, Instant::now() + WRITE_TIMEOUT);
                }
                Err(mpsc::TrySendError::Full(outbound)) => self.queued_writes.push_back(outbound),
                Err(mpsc::TrySendError::Disconnected(_)) => {
                    return Err("Language server input disconnected.".into());
                }
            }
        } else {
            self.queued_writes.push_back(outbound);
        }
        Ok(())
    }

    fn flush_writes(&mut self) -> Result<(), String> {
        while let Some(outbound) = self.queued_writes.pop_front() {
            let token = outbound.token;
            match self.writer.try_send(outbound) {
                Ok(()) => {
                    self.writes.insert(token, Instant::now() + WRITE_TIMEOUT);
                }
                Err(mpsc::TrySendError::Full(outbound)) => {
                    self.queued_writes.push_front(outbound);
                    break;
                }
                Err(mpsc::TrySendError::Disconnected(_)) => {
                    return Err("Language server input disconnected.".into());
                }
            }
        }
        Ok(())
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        self.queue(json!({"jsonrpc":"2.0","method":method,"params":params}))
    }

    fn workspace_folders(&self) -> Value {
        json!([{"uri":self.root_uri,"name":self.root.file_name().unwrap_or_default().to_string_lossy()}])
    }

    fn absolute_path(&self, path: &Path) -> PathBuf {
        normalize_path(&if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root.join(path)
        })
    }

    fn command(&mut self, command: ClientCommand) -> Result<(), String> {
        match command {
            ClientCommand::Open { path, text } => {
                let path = self.absolute_path(&path);
                if text.len() > MESSAGE_LIMIT / 2 {
                    return Err("This document is too large for language analysis.".into());
                }
                match self.documents.get_mut(&path) {
                    Some(document) => {
                        document.text = text;
                        document.version += 1;
                        document.dirty = true;
                    }
                    None => {
                        self.documents.insert(
                            path,
                            Document {
                                text,
                                version: 1,
                                opened: false,
                                dirty: true,
                                saved: false,
                                synced_end: Position::default(),
                            },
                        );
                    }
                }
            }
            ClientCommand::Change {
                path,
                text,
                version,
            } => {
                let path = self.absolute_path(&path);
                if text.len() > MESSAGE_LIMIT / 2 {
                    return Err("This document is too large for language analysis.".into());
                }
                if let Some(document) = self.documents.get_mut(&path) {
                    if version > document.version {
                        document.text = text;
                        document.version = version;
                        document.dirty = true;
                    }
                } else {
                    self.documents.insert(
                        path,
                        Document {
                            text,
                            version: version.max(1),
                            opened: false,
                            dirty: true,
                            saved: false,
                            synced_end: Position::default(),
                        },
                    );
                }
            }
            ClientCommand::Close(path) => {
                let path = self.absolute_path(&path);
                if self
                    .documents
                    .remove(&path)
                    .is_some_and(|document| document.opened && self.sync.open_close)
                {
                    self.notify(
                        "textDocument/didClose",
                        json!({"textDocument":{"uri":path_to_uri(&path)?}}),
                    )?;
                }
            }
            ClientCommand::Save(path) => {
                let path = self.absolute_path(&path);
                if let Some(document) = self.documents.get_mut(&path) {
                    document.saved = true;
                }
            }
            request @ (ClientCommand::Completion { .. } | ClientCommand::Definition { .. }) => {
                if self.ready {
                    self.request(request)?;
                } else {
                    if self.queued_requests.len() >= 64 {
                        self.queued_requests.pop_front();
                    }
                    self.queued_requests.push_back(request);
                }
            }
        }
        Ok(())
    }

    fn flush_documents(&mut self) -> Result<(), String> {
        let messages = self.documents.iter_mut().filter_map(|(path, document)| {
            if document.opened && !document.dirty && !document.saved { return None; }
            let uri = match path_to_uri(path) { Ok(uri) => uri, Err(error) => return Some(Err(error)) };
            let mut messages = Vec::new();
            if !document.opened {
                document.opened = true;
                document.dirty = false;
                if self.sync.open_close {
                    messages.push(json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri,"languageId":self.config.language_id,"version":document.version,"text":document.text}}}));
                }
                document.synced_end = char_to_position(&document.text, document.text.chars().count());
            } else if document.dirty {
                document.dirty = false;
                if self.sync.change != 0 {
                    // Replace the last transmitted version, measured in UTF-16.
                    // Intermediate edits coalesced before sending are irrelevant.
                    let change = if self.sync.change == 2 {
                        json!({"range":{"start":Position::default(),"end":document.synced_end},"text":document.text})
                    } else { json!({"text":document.text}) };
                    messages.push(json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":uri,"version":document.version},"contentChanges":[change]}}));
                }
                document.synced_end = char_to_position(&document.text, document.text.chars().count());
            }
            if document.saved {
                document.saved = false;
                if self.sync.save {
                    let mut params = json!({"textDocument":{"uri":uri}});
                    if self.sync.save_include_text { params["text"] = json!(document.text); }
                    messages.push(json!({"jsonrpc":"2.0","method":"textDocument/didSave","params":params}));
                }
            }
            if messages.is_empty() { None } else { Some(Ok(messages)) }
        }).collect::<Result<Vec<_>, String>>()?;
        for message in messages.into_iter().flatten() {
            self.queue(message)?;
        }
        Ok(())
    }

    fn request(&mut self, command: ClientCommand) -> Result<(), String> {
        self.flush_documents()?;
        let (id, path, position, method, kind) = match command {
            ClientCommand::Completion {
                id,
                path,
                position,
                version,
            } => {
                let path = self.absolute_path(&path);
                (
                    id,
                    path.clone(),
                    position,
                    "textDocument/completion",
                    RequestKind::Completion {
                        path,
                        position,
                        version,
                    },
                )
            }
            ClientCommand::Definition { id, path, position } => (
                id,
                self.absolute_path(&path),
                position,
                "textDocument/definition",
                RequestKind::Definition,
            ),
            _ => return Ok(()),
        };
        let supported = match kind {
            RequestKind::Completion { .. } => self.supports_completion,
            RequestKind::Definition => self.supports_definition,
            RequestKind::Initialize => true,
        };
        if !supported {
            let feature = if matches!(kind, RequestKind::Completion { .. }) {
                "completion"
            } else {
                "go to definition"
            };
            self.failed_requests.push_back(LspEvent::RequestFailed {
                request_id: id,
                error: Some(format!("{} does not provide {feature}.", self.config.name)),
            });
            return Ok(());
        }
        self.pending.insert(
            id,
            PendingRequest {
                kind,
                deadline: Instant::now() + REQUEST_TIMEOUT,
            },
        );
        self.queue(json!({"jsonrpc":"2.0","id":id,"method":method,"params":{"textDocument":{"uri":path_to_uri(&path)?},"position":position}}))
    }

    fn message(&mut self, message: Value, events: &EventEmitter) -> Result<(), String> {
        if let Some(method) = message.get("method").and_then(Value::as_str) {
            if let Some(id) = message.get("id") {
                let result = match method {
                    "workspace/configuration" => {
                        let items = message
                            .pointer("/params/items")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default();
                        Value::Array(
                            items
                                .iter()
                                .map(|item| {
                                    let section =
                                        item.get("section").and_then(Value::as_str).unwrap_or("");
                                    configuration_section(&self.config.settings, section)
                                })
                                .collect(),
                        )
                    }
                    "workspace/workspaceFolders" => self.workspace_folders(),
                    "workspace/applyEdit" => {
                        json!({"applied":false,"failureReason":"Automatic workspace edits are not supported."})
                    }
                    "client/registerCapability"
                    | "client/unregisterCapability"
                    | "window/workDoneProgress/create"
                    | "window/showMessageRequest" => Value::Null,
                    _ => {
                        return self.queue(json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Unsupported client method"}}));
                    }
                };
                return self.queue(json!({"jsonrpc":"2.0","id":id,"result":result}));
            }
            if method == "textDocument/publishDiagnostics" {
                if let Some((path, version, diagnostics)) =
                    parse_diagnostics(message.get("params").unwrap_or(&Value::Null))
                {
                    let fresh = self
                        .documents
                        .get(&normalize_path(&path))
                        .is_none_or(|document| {
                            version.is_none_or(|version| version >= document.version)
                        });
                    if fresh {
                        events.emit(LspEvent::Diagnostics {
                            path,
                            version,
                            diagnostics,
                        });
                    }
                }
            } else if method == "window/showMessage"
                && message.pointer("/params/type").and_then(Value::as_u64) == Some(1)
                && let Some(text) = message.pointer("/params/message").and_then(Value::as_str)
            {
                events.emit(LspEvent::Error(text.to_owned()));
            }
            return Ok(());
        }
        let Some(id) = message.get("id").and_then(Value::as_u64) else {
            return Ok(());
        };
        let Some(request) = self.pending.remove(&id) else {
            return Ok(());
        };
        if let Some(error) = message.get("error") {
            let text = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Unknown language server error");
            if matches!(request.kind, RequestKind::Initialize) {
                return Err(format!("{} could not initialize: {text}", self.config.name));
            }
            // Cancelled or stale requests are normal when typing or indexing.
            let cancelled = matches!(
                error.get("code").and_then(Value::as_i64),
                Some(-32802..=-32800)
            );
            events.emit(LspEvent::RequestFailed {
                request_id: id,
                error: (!cancelled).then(|| format!("{}: {text}", self.config.name)),
            });
            return Ok(());
        }
        let result = message.get("result").unwrap_or(&Value::Null);
        match request.kind {
            RequestKind::Initialize => {
                if let Some(encoding) = result
                    .pointer("/capabilities/positionEncoding")
                    .and_then(Value::as_str)
                    && encoding != "utf-16"
                {
                    return Err(format!(
                        "{} selected an unsupported position encoding: {encoding}",
                        self.config.name
                    ));
                }
                let capabilities = &result["capabilities"];
                self.sync = SyncOptions::from_capabilities(capabilities);
                self.supports_completion = capabilities["completionProvider"].is_object();
                self.supports_definition = capabilities["definitionProvider"] == Value::Bool(true)
                    || capabilities["definitionProvider"].is_object();
                self.notify("initialized", json!({}))?;
                self.notify(
                    "workspace/didChangeConfiguration",
                    json!({"settings":self.config.settings}),
                )?;
                self.ready = true;
                self.flush_documents()?;
                while let Some(command) = self.queued_requests.pop_front() {
                    self.request(command)?;
                }
                events.emit(LspEvent::Ready);
            }
            RequestKind::Completion {
                path,
                position,
                version,
            } => {
                events.emit(LspEvent::Completions {
                    request_id: id,
                    path,
                    version,
                    position,
                    items: parse_completions(result),
                });
            }
            RequestKind::Definition => {
                if let Some((path, position)) = parse_definition(result) {
                    events.emit(LspEvent::Definition {
                        request_id: id,
                        path,
                        line: position.line,
                        column: position.character,
                    });
                } else {
                    events.emit(LspEvent::DefinitionNotFound { request_id: id });
                }
            }
        }
        Ok(())
    }
}

fn read_message(reader: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut content_length = None;
    let mut total = 0;
    loop {
        let mut line = String::new();
        let count = reader
            .take((HEADER_LIMIT + 1) as u64)
            .read_line(&mut line)?;
        if count == 0 {
            return if total == 0 {
                Ok(None)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "incomplete LSP header",
                ))
            };
        }
        total += count;
        if total > HEADER_LIMIT {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "LSP header is too large",
            ));
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
        let Some((name, value)) = line.trim_end().split_once(':') else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "malformed LSP header",
            ));
        };
        if name.eq_ignore_ascii_case("Content-Length") {
            if content_length.is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "duplicate Content-Length",
                ));
            }
            content_length = Some(value.trim().parse::<usize>().map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid Content-Length")
            })?);
        }
    }
    let length = content_length
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing Content-Length"))?;
    if length == 0 || length > MESSAGE_LIMIT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "LSP message exceeds size limit",
        ));
    }
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes)?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn write_message(writer: &mut impl Write, message: &Value) -> io::Result<()> {
    let bytes = serde_json::to_vec(message)?;
    if bytes.len() > MESSAGE_LIMIT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "LSP message is too large",
        ));
    }
    write!(writer, "Content-Length: {}\r\n\r\n", bytes.len())?;
    writer.write_all(&bytes)?;
    writer.flush()
}

fn parse_range(value: &Value) -> Option<TextRange> {
    let range: TextRange = serde_json::from_value(value.clone()).ok()?;
    ((range.start.line, range.start.character) <= (range.end.line, range.end.character))
        .then_some(range)
}
fn parse_edit(value: &Value) -> Option<TextEdit> {
    Some(TextEdit {
        range: parse_range(value.get("range").or_else(|| value.get("replace"))?)?,
        new_text: value.get("newText")?.as_str()?.to_owned(),
    })
}

fn parse_diagnostics(params: &Value) -> Option<(PathBuf, Option<i32>, Vec<Diagnostic>)> {
    let path = uri_to_path(params.get("uri")?.as_str()?)?;
    let version = params
        .get("version")
        .and_then(Value::as_i64)
        .and_then(|value| i32::try_from(value).ok());
    let diagnostics = params
        .get("diagnostics")?
        .as_array()?
        .iter()
        .take(2000)
        .filter_map(|value| {
            Some(Diagnostic {
                range: parse_range(value.get("range")?)?,
                severity: value
                    .get("severity")
                    .and_then(Value::as_u64)
                    .unwrap_or(1)
                    .clamp(1, 4) as u8,
                message: value.get("message")?.as_str()?.to_owned(),
                source: value
                    .get("source")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            })
        })
        .collect();
    Some((path, version, diagnostics))
}

fn parse_completions(result: &Value) -> Vec<CompletionItem> {
    let Some(items) = result
        .as_array()
        .or_else(|| result.get("items").and_then(Value::as_array))
    else {
        return Vec::new();
    };
    let mut items = items
        .iter()
        .filter_map(|value| {
            let label = value.get("label")?.as_str()?.to_owned();
            let mut text_edit = value.get("textEdit").and_then(parse_edit);
            let mut insert_text = text_edit
                .as_ref()
                .map(|edit| edit.new_text.clone())
                .or_else(|| {
                    value
                        .get("insertText")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .unwrap_or_else(|| label.clone());
            if value.get("insertTextFormat").and_then(Value::as_u64) == Some(2) {
                insert_text = plain_snippet(&insert_text);
                if let Some(edit) = &mut text_edit {
                    edit.new_text = plain_snippet(&edit.new_text);
                }
            }
            let additional_text_edits = value
                .get("additionalTextEdits")
                .and_then(Value::as_array)
                .map(|edits| edits.iter().take(100).filter_map(parse_edit).collect())
                .unwrap_or_default();
            let detail = value
                .get("detail")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let sort = value
                .get("sortText")
                .and_then(Value::as_str)
                .unwrap_or(&label)
                .to_owned();
            Some((
                sort,
                CompletionItem {
                    label,
                    detail,
                    insert_text,
                    text_edit,
                    additional_text_edits,
                },
            ))
        })
        .collect::<Vec<_>>();
    items.sort_by(|a, b| a.0.cmp(&b.0));
    items.into_iter().take(200).map(|(_, item)| item).collect()
}

fn parse_definition(result: &Value) -> Option<(PathBuf, Position)> {
    let value = result
        .as_array()
        .and_then(|items| items.first())
        .unwrap_or(result);
    let uri = value
        .get("uri")
        .or_else(|| value.get("targetUri"))?
        .as_str()?;
    let range = value
        .get("targetSelectionRange")
        .or_else(|| value.get("range"))
        .or_else(|| value.get("targetRange"))?;
    Some((uri_to_path(uri)?, parse_range(range)?.start))
}

fn plain_snippet(snippet: &str) -> String {
    let mut output = String::new();
    let mut chars = snippet.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\'
            && chars
                .peek()
                .is_some_and(|next| matches!(next, '$' | '}' | '\\'))
        {
            output.push(chars.next().unwrap());
        } else if ch == '$' && chars.peek().is_some_and(char::is_ascii_digit) {
            while chars.peek().is_some_and(char::is_ascii_digit) {
                chars.next();
            }
        } else if ch == '$' && chars.peek() == Some(&'{') {
            chars.next();
            let mut placeholder = String::new();
            let mut depth = 1;
            for next in chars.by_ref() {
                if next == '{' {
                    depth += 1;
                }
                if next == '}' {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                placeholder.push(next);
            }
            if let Some((_, fallback)) = placeholder.split_once(':') {
                output.push_str(&plain_snippet(fallback));
            } else if let Some((_, choices)) = placeholder.split_once('|') {
                output.push_str(
                    choices
                        .split(',')
                        .next()
                        .unwrap_or("")
                        .trim_end_matches('|'),
                );
            }
        } else {
            output.push(ch);
        }
    }
    output
}

/// Convert egui's Unicode scalar offset to a zero-based UTF-16 LSP position.
pub fn char_to_position(text: &str, char_offset: usize) -> Position {
    let mut position = Position::default();
    let mut chars = text.chars().peekable();
    for _ in 0..char_offset {
        let Some(ch) = chars.next() else {
            break;
        };
        if ch == '\n' {
            position.line += 1;
            position.character = 0;
        } else if ch != '\r' || chars.peek() != Some(&'\n') {
            position.character += ch.len_utf16() as u32;
        }
    }
    position
}

/// Clamp stale positions to their line end/document end. A position inside a
/// surrogate pair clamps to the beginning of the corresponding Unicode scalar.
pub fn position_to_char(text: &str, position: Position) -> usize {
    let mut line = 0;
    let mut column = 0;
    let mut index = 0;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if line == position.line {
            if column >= position.character
                || ch == '\n'
                || (ch == '\r' && chars.peek() == Some(&'\n'))
            {
                return index;
            }
            let units = ch.len_utf16() as u32;
            if column + units > position.character {
                return index;
            }
            column += units;
        }
        index += 1;
        if ch == '\n' {
            line += 1;
            column = 0;
        }
    }
    index
}

fn normalize_path(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix("\\\\?\\UNC\\") {
        return PathBuf::from(format!("\\\\{rest}"));
    }
    let text = text.strip_prefix("\\\\?\\").unwrap_or(&text);
    #[cfg(windows)]
    {
        let mut text = text.to_owned();
        if text.as_bytes().get(1) == Some(&b':') && text.as_bytes()[0].is_ascii_alphabetic() {
            let drive = (text.as_bytes()[0] as char)
                .to_ascii_uppercase()
                .to_string();
            text.replace_range(0..1, &drive);
        }
        PathBuf::from(text)
    }
    #[cfg(not(windows))]
    PathBuf::from(text)
}

fn path_to_uri(path: &Path) -> Result<String, String> {
    let path = normalize_path(path);
    let text = path
        .to_str()
        .ok_or("Language server cannot represent this file path as Unicode")?
        .replace('\\', "/");
    let windows_drive =
        text.as_bytes().get(1) == Some(&b':') && text.as_bytes().get(2) == Some(&b'/');
    let (prefix, path) = if let Some(unc) = text.strip_prefix("//") {
        ("file://", unc)
    } else if windows_drive {
        ("file:///", text.as_str())
    } else if text.starts_with('/') {
        ("file://", text.as_str())
    } else {
        return Err("Language server requires an absolute file path".into());
    };
    let mut uri = String::from(prefix);
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~' | b'/' | b':') {
            uri.push(byte as char);
        } else {
            use std::fmt::Write;
            let _ = write!(uri, "%{byte:02X}");
        }
    }
    Ok(uri)
}

fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let encoded = uri.strip_prefix("file://")?;
    let mut bytes = Vec::with_capacity(encoded.len());
    let source = encoded.as_bytes();
    let mut index = 0;
    while index < source.len() {
        if source[index] == b'%' {
            let digits = source.get(index + 1..index + 3)?;
            let hex = std::str::from_utf8(digits).ok()?;
            bytes.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            bytes.push(source[index]);
            index += 1;
        }
    }
    let decoded = String::from_utf8(bytes).ok()?;
    if decoded.contains('\0') {
        return None;
    }
    let decoded = if let Some(path) = decoded.strip_prefix("localhost/") {
        format!("/{path}")
    } else {
        decoded
    };
    let decoded = if let Some(stripped) = decoded.strip_prefix('/') {
        if decoded.as_bytes().get(2) == Some(&b':') {
            stripped.to_owned()
        } else {
            decoded
        }
    } else {
        format!("//{decoded}")
    };
    #[cfg(windows)]
    let decoded = decoded.replace('/', "\\");
    Some(normalize_path(Path::new(&decoded)))
}

struct ProcessGuard {
    child: Child,
    #[cfg(windows)]
    job: Option<windows_job::Job>,
}
impl ProcessGuard {
    fn new(child: Child) -> Self {
        #[cfg(windows)]
        let job = windows_job::Job::attach(&child);
        Self {
            child,
            #[cfg(windows)]
            job,
        }
    }
}
impl Drop for ProcessGuard {
    fn drop(&mut self) {
        #[cfg(windows)]
        drop(self.job.take());
        #[cfg(unix)]
        {
            unsafe extern "C" {
                fn kill(pid: i32, signal: i32) -> i32;
            }
            if let Ok(pid) = i32::try_from(self.child.id()) {
                unsafe {
                    kill(-pid, 9);
                }
            }
        }
        let _ = self.child.kill();
        let deadline = Instant::now() + Duration::from_millis(100);
        while matches!(self.child.try_wait(), Ok(None)) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
    }
}

#[cfg(windows)]
mod windows_job {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;
    #[repr(C)]
    struct BasicLimits {
        process_time: i64,
        job_time: i64,
        flags: u32,
        min_working_set: usize,
        max_working_set: usize,
        active_processes: u32,
        affinity: usize,
        priority: u32,
        scheduling: u32,
    }
    #[repr(C)]
    struct ExtendedLimits {
        basic: BasicLimits,
        io_counters: [u64; 6],
        process_memory: usize,
        job_memory: usize,
        peak_process_memory: usize,
        peak_job_memory: usize,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> *mut c_void;
        fn SetInformationJobObject(
            job: *mut c_void,
            class: u32,
            info: *const c_void,
            length: u32,
        ) -> i32;
        fn AssignProcessToJobObject(job: *mut c_void, process: *mut c_void) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }
    pub(super) struct Job(*mut c_void);
    impl Job {
        pub(super) fn attach(child: &Child) -> Option<Self> {
            let mut limits: ExtendedLimits = unsafe { std::mem::zeroed() };
            limits.basic.flags = 0x2000;
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if handle.is_null() {
                    return None;
                }
                let job = Self(handle);
                if SetInformationJobObject(
                    handle,
                    9,
                    &limits as *const _ as *const c_void,
                    std::mem::size_of::<ExtendedLimits>() as u32,
                ) == 0
                    || AssignProcessToJobObject(handle, child.as_raw_handle()) == 0
                {
                    return None;
                }
                Some(job)
            }
        }
    }
    impl Drop for Job {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Cursor;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn range(line: u32, start: u32, end: u32) -> Value {
        json!({"start":{"line":line,"character":start},"end":{"line":line,"character":end}})
    }

    #[test]
    fn utf16_positions_round_trip_unicode_and_clamp_surrogate_pairs() {
        let text = "a🦀ü\n日本語😀z\n";
        for offset in 0..=text.chars().count() {
            assert_eq!(
                position_to_char(text, char_to_position(text, offset)),
                offset
            );
        }
        assert_eq!(
            char_to_position(text, 2),
            Position {
                line: 0,
                character: 3
            }
        );
        assert_eq!(
            position_to_char(
                text,
                Position {
                    line: 0,
                    character: 2
                }
            ),
            1
        );
        assert_eq!(
            position_to_char(
                text,
                Position {
                    line: 0,
                    character: 1000
                }
            ),
            3
        );
        assert_eq!(
            position_to_char(
                text,
                Position {
                    line: 100,
                    character: 10
                }
            ),
            text.chars().count()
        );
        assert_eq!(
            char_to_position(text, 1000),
            Position {
                line: 2,
                character: 0
            }
        );
    }

    #[test]
    fn crlf_positions_exclude_line_breaks_from_columns() {
        let text = "🦀x\r\ny\r\n";
        assert_eq!(
            char_to_position(text, 2),
            Position {
                line: 0,
                character: 3
            }
        );
        assert_eq!(
            char_to_position(text, 3),
            Position {
                line: 0,
                character: 3
            }
        );
        assert_eq!(
            char_to_position(text, 4),
            Position {
                line: 1,
                character: 0
            }
        );
        assert_eq!(
            position_to_char(
                text,
                Position {
                    line: 0,
                    character: 99
                }
            ),
            2
        );
        assert_eq!(
            position_to_char(
                text,
                Position {
                    line: 1,
                    character: 1
                }
            ),
            5
        );
    }

    #[test]
    fn file_uris_round_trip_unicode_spaces_and_reserved_characters() {
        #[cfg(windows)]
        let path = Path::new(r"C:\Code\über #1%\🦀.rs");
        #[cfg(not(windows))]
        let path = Path::new("/code/über #1%/🦀.rs");
        let uri = path_to_uri(path).unwrap();
        assert!(uri.contains("%C3%BCber%20%231%25/"));
        assert!(uri.ends_with("%F0%9F%A6%80.rs"));
        assert_eq!(uri_to_path(&uri).unwrap(), path);
        assert!(uri_to_path("https://example.com/code.rs").is_none());
        assert!(uri_to_path("file:///bad%XZ.rs").is_none());
        assert!(uri_to_path("file:///bad%00.rs").is_none());
        assert!(path_to_uri(Path::new("relative/file.rs")).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn windows_file_uris_handle_extended_prefixes_and_unc_shares() {
        let extended = Path::new(r"\\?\C:\Code\main.rs");
        assert_eq!(path_to_uri(extended).unwrap(), "file:///C:/Code/main.rs");
        let unc = Path::new(r"\\?\UNC\server\share\hello world.rs");
        let uri = path_to_uri(unc).unwrap();
        assert_eq!(uri, "file://server/share/hello%20world.rs");
        assert_eq!(
            uri_to_path(&uri).unwrap(),
            Path::new(r"\\server\share\hello world.rs")
        );
        assert_eq!(
            uri_to_path("file://localhost/C:/Code/main.rs").unwrap(),
            Path::new(r"C:\Code\main.rs")
        );
        assert_eq!(
            uri_to_path("file:///C%3A/Code/main.rs").unwrap(),
            Path::new(r"C:\Code\main.rs")
        );
    }

    #[test]
    fn framing_counts_utf8_bytes_and_reads_multiple_messages() {
        let first = json!({"jsonrpc":"2.0","method":"test","params":{"text":"🦀 ü"}});
        let second = json!({"jsonrpc":"2.0","id":7,"result":null});
        let mut bytes = Vec::new();
        write_message(&mut bytes, &first).unwrap();
        write_message(&mut bytes, &second).unwrap();
        let mut reader = Cursor::new(bytes);
        assert_eq!(read_message(&mut reader).unwrap(), Some(first));
        assert_eq!(read_message(&mut reader).unwrap(), Some(second));
        assert_eq!(read_message(&mut reader).unwrap(), None);
    }

    #[test]
    fn malformed_or_oversized_frames_fail_before_allocating_the_body() {
        let cases = [
            b"Content-Length: nope\r\n\r\n".as_slice(),
            b"Content-Length: 17000000\r\n\r\n".as_slice(),
            b"Content-Length: 2\r\nContent-Length: 2\r\n\r\n{}".as_slice(),
            b"Content-Type: application/json\r\n\r\n{}".as_slice(),
            b"Content-Length: 10\r\n\r\n{}".as_slice(),
            b"Content-Length: 3\r\n\r\nx{}".as_slice(),
            b"Content-Length: 0\r\n\r\n".as_slice(),
        ];
        for bytes in cases {
            assert!(read_message(&mut Cursor::new(bytes)).is_err());
        }
        assert!(read_message(&mut Cursor::new(vec![b'a'; HEADER_LIMIT + 2])).is_err());
    }

    #[test]
    fn completion_parsing_supports_lists_replace_edits_imports_and_plain_snippets() {
        let result = json!({"isIncomplete":false,"items":[
            {"label":"later","sortText":"2","insertText":"later()"},
            {"label":"first","sortText":"1","detail":"fn first(value: i32)","insertTextFormat":2,
                "textEdit":{"insert":range(2,3,3),"replace":range(2,3,8),"newText":"first(${1:value})$0"},
                "additionalTextEdits":[{"range":range(0,0,0),"newText":"use sample::first;\n"}]}
        ]});
        let items = parse_completions(&result);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].label, "first");
        assert_eq!(items[0].insert_text, "first(value)");
        assert_eq!(items[0].text_edit.as_ref().unwrap().range.end.character, 8);
        assert_eq!(
            items[0].additional_text_edits[0].new_text,
            "use sample::first;\n"
        );
        assert_eq!(items[1].insert_text, "later()");
        assert!(parse_completions(&Value::Null).is_empty());
        assert_eq!(
            plain_snippet("${1|red,green,blue|} ${2:${3:nested}} \\$literal $0"),
            "red nested $literal "
        );
    }

    #[test]
    fn definition_parsing_prefers_target_selection_ranges() {
        let result = json!([{"targetUri":"file:///C:/Code/main.rs","targetRange":range(5,0,20),"targetSelectionRange":range(6,7,12)}]);
        let (_, position) = parse_definition(&result).unwrap();
        assert_eq!(
            position,
            Position {
                line: 6,
                character: 7
            }
        );
        let result = json!({"uri":"file:///C:/Code/main.rs","range":range(1,2,3)});
        assert_eq!(
            parse_definition(&result).unwrap().1,
            Position {
                line: 1,
                character: 2
            }
        );
        assert!(parse_definition(&json!([])).is_none());
        assert!(parse_definition(&Value::Null).is_none());
    }

    fn protocol() -> (Protocol, Receiver<Outbound>) {
        let (writer, receiver) = mpsc::sync_channel(32);
        let root = std::env::temp_dir();
        let root_uri = path_to_uri(&root).unwrap();
        (
            Protocol {
                config: ServerConfig::rust(),
                sync: SyncOptions {
                    open_close: true,
                    change: 1,
                    save: true,
                    save_include_text: false,
                },
                supports_completion: true,
                supports_definition: true,
                failed_requests: VecDeque::new(),
                root,
                root_uri,
                writer,
                queued_writes: VecDeque::new(),
                next_write: 1,
                writes: HashMap::new(),
                pending: HashMap::new(),
                documents: HashMap::new(),
                queued_requests: VecDeque::new(),
                ready: false,
            },
            receiver,
        )
    }

    #[test]
    fn full_event_queue_preserves_lifecycle_requests_and_diagnostics_until_ui_resumes() {
        let (sender, receiver) = mpsc::sync_channel(1);
        sender.send(LspEvent::Ready).unwrap();
        let emitter = EventEmitter::new(sender, None);
        let worker_emitter = emitter.clone();
        let (started_tx, started_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            started_tx.send(()).unwrap();
            worker_emitter.emit(LspEvent::RequestFailed {
                request_id: 87,
                error: Some("A request failed while a dialog was open".into()),
            });
            worker_emitter.emit(LspEvent::Diagnostics {
                path: std::env::temp_dir().join("queue.py"),
                version: Some(3),
                diagnostics: Vec::new(),
            });
            done_tx.send(()).unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(matches!(
            done_rx.recv_timeout(Duration::from_millis(40)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        assert!(matches!(
            receiver.recv_timeout(Duration::from_secs(1)).unwrap(),
            LspEvent::Ready
        ));
        assert!(matches!(
            receiver.recv_timeout(Duration::from_secs(1)).unwrap(),
            LspEvent::RequestFailed {
                request_id: 87,
                error: Some(_)
            }
        ));
        assert!(
            matches!(receiver.recv_timeout(Duration::from_secs(1)).unwrap(), LspEvent::Diagnostics { version:Some(3), diagnostics, .. } if diagnostics.is_empty())
        );
        done_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        worker.join().unwrap();
        assert!(!emitter.take_backpressure().is_zero());
    }

    #[test]
    fn stopped_server_preserves_its_fatal_error_until_consumed() {
        let (sender, receiver) = mpsc::sync_channel(1);
        sender.send(LspEvent::Ready).unwrap();
        let emitter = EventEmitter::new(sender, None);
        let (commands, _commands_rx) = mpsc::sync_channel(1);
        let server = LanguageServer {
            receiver,
            commands,
            stopped: Arc::new(AtomicBool::new(true)),
            shutdown_requested: Arc::clone(&emitter.shutdown_requested),
        };
        let worker = thread::spawn(move || emitter.emit(LspEvent::Error("server exited".into())));
        assert!(server.is_stopped());
        assert!(matches!(
            server
                .receiver
                .recv_timeout(Duration::from_secs(1))
                .unwrap(),
            LspEvent::Ready
        ));
        assert!(
            matches!(server.receiver.recv_timeout(Duration::from_secs(1)).unwrap(), LspEvent::Error(error) if error == "server exited")
        );
        worker.join().unwrap();
    }

    #[test]
    fn shutdown_releases_a_worker_blocked_on_full_events_without_ui_drain() {
        let (sender, receiver) = mpsc::sync_channel(1);
        sender.send(LspEvent::Ready).unwrap();
        let emitter = EventEmitter::new(sender, None);
        let (commands, _commands_rx) = mpsc::sync_channel(1);
        let server = LanguageServer {
            receiver,
            commands,
            stopped: Arc::new(AtomicBool::new(false)),
            shutdown_requested: Arc::clone(&emitter.shutdown_requested),
        };
        let (done_tx, done_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            emitter.emit(LspEvent::Error("pending error".into()));
            done_tx.send(()).unwrap();
        });
        assert!(matches!(
            done_rx.recv_timeout(Duration::from_millis(40)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        server.shutdown();
        done_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("shutdown must release event backpressure without reading the queue");
        worker.join().unwrap();
        assert!(server.is_stopped());
        assert!(matches!(
            server.receiver.try_recv().unwrap(),
            LspEvent::Ready
        ));
        assert!(server.receiver.try_recv().is_err());
    }

    #[test]
    fn event_backpressure_extends_request_and_write_deadlines() {
        let (mut protocol, _outgoing) = protocol();
        let deadline = Instant::now();
        protocol.writes.insert(1, deadline);
        protocol.pending.insert(
            2,
            PendingRequest {
                kind: RequestKind::Definition,
                deadline,
            },
        );
        let paused = Duration::from_secs(120);
        protocol.extend_deadlines(paused);
        assert_eq!(protocol.writes[&1], deadline + paused);
        assert_eq!(protocol.pending[&2].deadline, deadline + paused);
    }

    fn initialize_protocol(protocol: &mut Protocol, capabilities: Value) {
        protocol.pending.insert(
            0,
            PendingRequest {
                kind: RequestKind::Initialize,
                deadline: Instant::now() + INITIALIZE_TIMEOUT,
            },
        );
        let (sender, _receiver) = mpsc::sync_channel(8);
        protocol
            .message(
                json!({"id":0,"result":{"capabilities":capabilities}}),
                &EventEmitter::new(sender, None),
            )
            .unwrap();
    }

    #[test]
    fn python_and_dart_open_documents_with_the_configured_language_id_and_settings() {
        for language in ["python", "dart"] {
            let (mut protocol, outgoing) = protocol();
            protocol.config.language_id = language.into();
            protocol.config.settings = json!({language: {"configured": true}});
            let path = protocol.root.join(format!("language.{language}"));
            protocol
                .command(ClientCommand::Open {
                    path,
                    text: "example".into(),
                })
                .unwrap();
            initialize_protocol(
                &mut protocol,
                json!({"textDocumentSync":{"openClose":true,"change":1}}),
            );
            let messages = outgoing
                .try_iter()
                .map(|out| out.message)
                .collect::<Vec<_>>();
            assert_eq!(messages[0]["method"], "initialized");
            assert_eq!(messages[1]["method"], "workspace/didChangeConfiguration");
            assert_eq!(
                messages[1]["params"]["settings"][language]["configured"],
                true
            );
            assert_eq!(messages[2]["method"], "textDocument/didOpen");
            assert_eq!(
                messages[2]["params"]["textDocument"]["languageId"],
                language
            );
        }
    }

    #[test]
    fn generic_configuration_supports_whole_nested_and_dotted_sections() {
        let settings = json!({"python":{"pythonPath":"/venv/python", "analysis":{"typeCheckingMode":"basic"}}, "dart":{"lineLength":100}, "custom.flat":{"enabled":true}});
        assert_eq!(configuration_section(&settings, ""), settings);
        assert_eq!(
            configuration_section(&settings, "python.pythonPath"),
            "/venv/python"
        );
        assert_eq!(
            configuration_section(&settings, "python.analysis.typeCheckingMode"),
            "basic"
        );
        assert_eq!(
            configuration_section(&settings, "custom.flat")["enabled"],
            true
        );
        assert_eq!(configuration_section(&settings, "missing"), Value::Null);
    }

    #[test]
    fn incremental_changes_use_previous_sent_utf16_range_and_save_includes_text_when_requested() {
        let (mut protocol, outgoing) = protocol();
        protocol.config.language_id = "python".into();
        let path = protocol.root.join("unicode.py");
        protocol
            .command(ClientCommand::Open {
                path: path.clone(),
                text: "a\u{1f600}\r\nz\u{1f600}".into(),
            })
            .unwrap();
        initialize_protocol(
            &mut protocol,
            json!({"textDocumentSync":{"openClose":true,"change":2,"save":{"includeText":true}}}),
        );
        outgoing.try_iter().for_each(drop);
        protocol
            .command(ClientCommand::Change {
                path: path.clone(),
                text: "intermediate".into(),
                version: 2,
            })
            .unwrap();
        protocol
            .command(ClientCommand::Change {
                path: path.clone(),
                text: "new\u{1f600}".into(),
                version: 3,
            })
            .unwrap();
        protocol.command(ClientCommand::Save(path.clone())).unwrap();
        protocol.flush_documents().unwrap();
        let change = outgoing.try_recv().unwrap().message;
        assert_eq!(change["method"], "textDocument/didChange");
        assert_eq!(change["params"]["textDocument"]["version"], 3);
        assert_eq!(
            change["params"]["contentChanges"][0]["range"],
            json!({"start":{"line":0,"character":0},"end":{"line":1,"character":3}})
        );
        assert_eq!(
            change["params"]["contentChanges"][0]["text"],
            "new\u{1f600}"
        );
        let save = outgoing.try_recv().unwrap().message;
        assert_eq!(save["method"], "textDocument/didSave");
        assert_eq!(save["params"]["text"], "new\u{1f600}");
        protocol
            .command(ClientCommand::Change {
                path,
                text: "last".into(),
                version: 4,
            })
            .unwrap();
        protocol.flush_documents().unwrap();
        assert_eq!(
            outgoing.try_recv().unwrap().message["params"]["contentChanges"][0]["range"]["end"],
            json!({"line":0,"character":5})
        );
    }

    #[test]
    fn absent_features_report_unavailable_without_sending_requests_or_fake_success() {
        let (mut protocol, outgoing) = protocol();
        protocol.config.name = "Python analysis".into();
        initialize_protocol(&mut protocol, json!({}));
        outgoing.try_iter().for_each(drop);
        let path = protocol.root.join("features.py");
        protocol
            .command(ClientCommand::Completion {
                id: 71,
                path: path.clone(),
                position: Position::default(),
                version: 1,
            })
            .unwrap();
        protocol
            .command(ClientCommand::Definition {
                id: 72,
                path,
                position: Position::default(),
            })
            .unwrap();
        assert!(outgoing.try_recv().is_err());
        assert!(protocol.pending.is_empty());
        assert!(
            matches!(protocol.failed_requests.pop_front(), Some(LspEvent::RequestFailed { request_id:71,error:Some(error) }) if error.contains("Python analysis") && error.contains("completion"))
        );
        assert!(
            matches!(protocol.failed_requests.pop_front(), Some(LspEvent::RequestFailed { request_id:72,error:Some(error) }) if error.contains("go to definition"))
        );
    }

    #[test]
    fn disabled_sync_and_save_capabilities_produce_no_notifications() {
        let (mut protocol, outgoing) = protocol();
        initialize_protocol(
            &mut protocol,
            json!({"textDocumentSync":{"openClose":false,"change":0,"save":false}}),
        );
        outgoing.try_iter().for_each(drop);
        let path = protocol.root.join("no-sync.py");
        protocol
            .command(ClientCommand::Open {
                path: path.clone(),
                text: "first".into(),
            })
            .unwrap();
        protocol.flush_documents().unwrap();
        protocol
            .command(ClientCommand::Change {
                path: path.clone(),
                text: "second".into(),
                version: 2,
            })
            .unwrap();
        protocol.command(ClientCommand::Save(path.clone())).unwrap();
        protocol.flush_documents().unwrap();
        protocol.command(ClientCommand::Close(path)).unwrap();
        assert!(outgoing.try_recv().is_err());
    }

    #[test]
    fn document_updates_are_coalesced_until_initialized_and_stale_changes_are_ignored() {
        let (mut protocol, outgoing) = protocol();
        let path = protocol.root.join("morrow-lsp-document.rs");
        protocol
            .command(ClientCommand::Open {
                path: path.clone(),
                text: "old".into(),
            })
            .unwrap();
        protocol
            .command(ClientCommand::Change {
                path: path.clone(),
                text: "newest".into(),
                version: 3,
            })
            .unwrap();
        protocol
            .command(ClientCommand::Change {
                path: path.clone(),
                text: "stale".into(),
                version: 2,
            })
            .unwrap();
        assert!(outgoing.try_recv().is_err());
        protocol.ready = true;
        protocol.flush_documents().unwrap();
        let message = outgoing.try_recv().unwrap().message;
        assert_eq!(message["method"], "textDocument/didOpen");
        assert_eq!(message["params"]["textDocument"]["text"], "newest");
        assert_eq!(message["params"]["textDocument"]["version"], 3);
        protocol
            .command(ClientCommand::Change {
                path: path.clone(),
                text: "fourth".into(),
                version: 4,
            })
            .unwrap();
        protocol
            .command(ClientCommand::Completion {
                id: 7,
                path,
                position: Position::default(),
                version: 4,
            })
            .unwrap();
        assert_eq!(
            outgoing.try_recv().unwrap().message["method"],
            "textDocument/didChange"
        );
        assert_eq!(
            outgoing.try_recv().unwrap().message["method"],
            "textDocument/completion"
        );
    }

    #[test]
    fn rpc_request_errors_emit_failure_only_and_later_success_still_works() {
        for completion in [false, true] {
            let (mut protocol, _outgoing) = protocol();
            protocol.ready = true;
            let path = protocol.root.join("morrow-lsp-error.rs");
            let (sender, events) = mpsc::sync_channel(8);
            let emitter = EventEmitter::new(sender, None);
            let request = |id| {
                if completion {
                    ClientCommand::Completion {
                        id,
                        path: path.clone(),
                        position: Position::default(),
                        version: 1,
                    }
                } else {
                    ClientCommand::Definition {
                        id,
                        path: path.clone(),
                        position: Position::default(),
                    }
                }
            };
            protocol.command(request(41)).unwrap();
            protocol.message(json!({"jsonrpc":"2.0","id":41,"error":{"code":-32603,"message":"temporary server failure"}}), &emitter).unwrap();
            assert!(
                matches!(events.try_recv().unwrap(), LspEvent::RequestFailed { request_id: 41, error: Some(error) } if error.contains("temporary server failure"))
            );
            assert!(
                events.try_recv().is_err(),
                "an RPC error must not synthesize a successful empty response"
            );
            assert!(!protocol.pending.contains_key(&41));

            protocol.command(request(42)).unwrap();
            let result = if completion {
                json!([{ "label": "recovered" }])
            } else {
                Value::Null
            };
            protocol
                .message(json!({"jsonrpc":"2.0","id":42,"result":result}), &emitter)
                .unwrap();
            let event = events.try_recv().unwrap();
            if completion {
                assert!(
                    matches!(event, LspEvent::Completions { request_id: 42, items, .. } if items.len() == 1 && items[0].label == "recovered")
                );
            } else {
                assert!(matches!(
                    event,
                    LspEvent::DefinitionNotFound { request_id: 42 }
                ));
            }
            assert!(events.try_recv().is_err());
            assert!(!protocol.pending.contains_key(&42));
        }
    }

    #[test]
    fn rpc_cancellation_clears_the_request_without_success_or_a_warning() {
        let (mut protocol, _outgoing) = protocol();
        protocol.ready = true;
        let path = protocol.root.join("morrow-lsp-cancel.rs");
        let (sender, events) = mpsc::sync_channel(8);
        let emitter = EventEmitter::new(sender, None);
        protocol
            .command(ClientCommand::Definition {
                id: 51,
                path,
                position: Position::default(),
            })
            .unwrap();
        protocol.message(json!({"jsonrpc":"2.0","id":51,"error":{"code":-32800,"message":"Request cancelled"}}), &emitter).unwrap();
        assert!(matches!(
            events.try_recv().unwrap(),
            LspEvent::RequestFailed {
                request_id: 51,
                error: None
            }
        ));
        assert!(events.try_recv().is_err());
        assert!(!protocol.pending.contains_key(&51));
    }

    #[test]
    fn configuration_and_registration_requests_receive_responses() {
        let (mut protocol, outgoing) = protocol();
        let (sender, _events) = mpsc::sync_channel(8);
        let emitter = EventEmitter::new(sender, None);
        protocol.message(json!({"jsonrpc":"2.0","id":"config","method":"workspace/configuration","params":{"items":[{"section":"rust-analyzer"},{"section":"rust-analyzer.cargo"},{"section":"other"}]}}), &emitter).unwrap();
        let response = outgoing.try_recv().unwrap().message;
        assert_eq!(response["id"], "config");
        assert_eq!(response["result"][0]["checkOnSave"], true);
        assert_eq!(response["result"][1]["targetDir"], true);
        assert!(response["result"][2].is_null());
        protocol
            .message(
                json!({"id":9,"method":"client/registerCapability","params":{}}),
                &emitter,
            )
            .unwrap();
        assert!(outgoing.try_recv().unwrap().message["result"].is_null());
        protocol
            .message(
                json!({"id":10,"method":"workspace/applyEdit","params":{}}),
                &emitter,
            )
            .unwrap();
        assert_eq!(
            outgoing.try_recv().unwrap().message["result"]["applied"],
            false
        );
    }

    #[test]
    fn writer_pressure_is_bounded_and_preserves_message_order() {
        let (mut protocol, outgoing) = protocol();
        for number in 0..100 {
            protocol.notify("test", json!({"number":number})).unwrap();
        }
        assert!(!protocol.queued_writes.is_empty());
        let mut received = Vec::new();
        while received.len() < 100 {
            while let Ok(outbound) = outgoing.try_recv() {
                received.push(outbound.message["params"]["number"].as_u64().unwrap());
            }
            protocol.flush_writes().unwrap();
        }
        assert_eq!(received, (0..100).collect::<Vec<_>>());
        assert!(protocol.queued_writes.is_empty());
        for number in 0..160 {
            protocol.notify("test", json!({"number":number})).unwrap();
        }
        assert!(protocol.notify("overflow", json!({})).is_err());
    }

    #[test]
    fn versioned_diagnostics_do_not_replace_newer_document_state() {
        let (mut protocol, _outgoing) = protocol();
        let path = protocol.root.join("morrow-lsp-diagnostics.rs");
        protocol
            .command(ClientCommand::Change {
                path: path.clone(),
                text: "new".into(),
                version: 5,
            })
            .unwrap();
        let (sender, events) = mpsc::sync_channel(8);
        let emitter = EventEmitter::new(sender, None);
        let notification = |version| json!({"method":"textDocument/publishDiagnostics","params":{"uri":path_to_uri(&path).unwrap(),"version":version,"diagnostics":[{"range":range(0,0,1),"severity":2,"source":"rust-analyzer","message":"example warning"}]}});
        protocol.message(notification(4), &emitter).unwrap();
        assert!(events.try_recv().is_err());
        protocol.message(notification(5), &emitter).unwrap();
        let LspEvent::Diagnostics {
            diagnostics,
            version,
            ..
        } = events.try_recv().unwrap()
        else {
            panic!("expected diagnostics");
        };
        assert_eq!(version, Some(5));
        assert_eq!(diagnostics[0].severity, 2);
        assert_eq!(diagnostics[0].message, "example warning");
    }

    struct TestWorkspace(PathBuf);
    impl TestWorkspace {
        fn new() -> Self {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "morrow-lsp-tests-{}-{stamp} with spaces ü",
                std::process::id()
            ));
            fs::create_dir(&root).unwrap();
            fs::create_dir(root.join("src")).unwrap();
            fs::write(root.join("Cargo.toml"), "[package]\nname = \"morrow_lsp_smoke\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n").unwrap();
            Self(root)
        }
    }
    impl Drop for TestWorkspace {
        fn drop(&mut self) {
            assert_eq!(self.0.parent(), Some(std::env::temp_dir().as_path()));
            assert!(
                self.0
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("morrow-lsp-tests-")
            );
            for _ in 0..10 {
                if fs::remove_dir_all(&self.0).is_ok() {
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
        }
    }

    #[test]
    #[ignore = "requires the installed rust-analyzer and Rust toolchain; run explicitly"]
    fn live_rust_analyzer_diagnostics_completion_and_definition() {
        let workspace = TestWorkspace::new();
        let path = workspace.0.join("src/lib.rs");
        let text =
            "pub fn double(value: i32) -> i32 { value * 2 }\npub fn run() { let _ = doub; }\n";
        fs::write(&path, text).unwrap();
        let server = LanguageServer::start(workspace.0.clone(), None);
        assert!(server.open(&path, text));
        let deadline = Instant::now() + Duration::from_secs(60);
        let completion_position = char_to_position(text, text.find("doub;").unwrap() + 4);
        let mut ready = false;
        let mut had_diagnostic = false;
        let mut had_completion = false;
        let mut next_request = Instant::now();
        while Instant::now() < deadline && !(ready && had_diagnostic && had_completion) {
            if ready && Instant::now() >= next_request {
                assert_ne!(server.completion(&path, completion_position, 1), 0);
                next_request = Instant::now() + Duration::from_millis(500);
            }
            match server.receiver.recv_timeout(Duration::from_millis(100)) {
                Ok(LspEvent::Ready) => ready = true,
                Ok(LspEvent::Diagnostics {
                    path: diagnostic_path,
                    diagnostics,
                    ..
                }) => {
                    if normalize_path(&diagnostic_path) == normalize_path(&path)
                        && diagnostics.iter().any(|item| item.severity == 1)
                    {
                        had_diagnostic = true;
                    }
                }
                Ok(LspEvent::Completions { items, .. }) => {
                    if items.iter().any(|item| item.label.starts_with("double")) {
                        had_completion = true;
                    }
                }
                Ok(LspEvent::Error(error)) => panic!("live rust-analyzer error: {error}"),
                _ => {}
            }
        }
        assert!(ready, "rust-analyzer never initialized");
        assert!(
            had_diagnostic,
            "rust-analyzer did not report the unresolved value"
        );
        assert!(
            had_completion,
            "rust-analyzer did not offer the local double function"
        );
        let updated =
            "pub fn double(value: i32) -> i32 { value * 2 }\npub fn run() { let _ = double(4); }\n";
        assert!(server.change(&path, updated, 2));
        let definition_position =
            char_to_position(updated, updated.rfind("double(4)").unwrap() + 2);
        let mut requested_id = server.definition(&path, definition_position);
        assert_ne!(requested_id, 0);
        let definition_deadline = Instant::now() + Duration::from_secs(20);
        next_request = Instant::now() + Duration::from_secs(1);
        let mut found = false;
        while Instant::now() < definition_deadline && !found {
            match server.receiver.recv_timeout(Duration::from_millis(100)) {
                Ok(LspEvent::Definition {
                    request_id,
                    path: target,
                    line,
                    column,
                }) if request_id == requested_id => {
                    assert_eq!(normalize_path(&target), normalize_path(&path));
                    assert_eq!(line, 0);
                    assert_eq!(column, 7);
                    found = true;
                }
                Ok(LspEvent::Error(error)) => {
                    panic!("live rust-analyzer definition error: {error}")
                }
                _ => {}
            }
            if !found && Instant::now() >= next_request {
                requested_id = server.definition(&path, definition_position);
                next_request = Instant::now() + Duration::from_secs(1);
            }
        }
        assert!(
            found,
            "rust-analyzer did not navigate to the local definition"
        );
        server.shutdown();
        drop(server);
        // Give the supervisor its polling interval to stop child processes
        // before the fixture cleans up their working directory.
        thread::sleep(Duration::from_millis(150));
    }
}

#[cfg(test)]
#[path = "polyglot_live_tests.rs"]
mod polyglot_live_tests;

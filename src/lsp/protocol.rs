//! Negotiated synchronization, pending requests and JSON-RPC dispatch.
use super::*;

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

pub(super) fn configuration_section(settings: &Value, section: &str) -> Value {
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

impl Protocol {
    pub(super) fn wait_duration(&self, now: Instant) -> Duration {
        self.writes
            .values()
            .copied()
            .chain(self.pending.values().map(|request| request.deadline))
            .map(|deadline| deadline.saturating_duration_since(now))
            .min()
            .unwrap_or(PROCESS_CHECK_INTERVAL)
            .min(PROCESS_CHECK_INTERVAL)
    }

    pub(super) fn extend_deadlines(&mut self, paused: Duration) {
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

    pub(super) fn queue(&mut self, message: Value) -> Result<(), String> {
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

    pub(super) fn flush_writes(&mut self) -> Result<(), String> {
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

    pub(super) fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        self.queue(json!({"jsonrpc":"2.0","method":method,"params":params}))
    }

    pub(super) fn workspace_folders(&self) -> Value {
        json!([{"uri":self.root_uri,"name":self.root.file_name().unwrap_or_default().to_string_lossy()}])
    }

    fn absolute_path(&self, path: &Path) -> PathBuf {
        normalize_path(&if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root.join(path)
        })
    }

    pub(super) fn command(&mut self, command: ClientCommand) -> Result<(), String> {
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
            request @ (ClientCommand::Completion { .. }
            | ClientCommand::Definition { .. }
            | ClientCommand::Hover { .. }
            | ClientCommand::References { .. }
            | ClientCommand::DocumentSymbols { .. }
            | ClientCommand::WorkspaceSymbols { .. }) => {
                if self.ready {
                    self.request(request)?;
                } else {
                    if self.queued_requests.len() >= 64
                        && let Some(evicted) = self.queued_requests.pop_front()
                    {
                        let id = match evicted {
                            ClientCommand::Completion { id, .. }
                            | ClientCommand::Definition { id, .. }
                            | ClientCommand::Hover { id, .. }
                            | ClientCommand::References { id, .. }
                            | ClientCommand::DocumentSymbols { id, .. }
                            | ClientCommand::WorkspaceSymbols { id, .. } => id,
                            _ => unreachable!("only requests enter this queue"),
                        };
                        self.failed_requests.push_back(LspEvent::RequestFailed {
                            request_id: id,
                            error: None,
                        });
                    }
                    self.queued_requests.push_back(request);
                }
            }
        }
        Ok(())
    }

    pub(super) fn flush_documents(&mut self) -> Result<(), String> {
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
                if self.sync.change == 2 {
                    document.synced_end = document_end_position(&document.text);
                }
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
                if self.sync.change == 2 {
                    document.synced_end = document_end_position(&document.text);
                }
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
        let (id, method, params, kind, feature) = match command {
            ClientCommand::Hover { id, path, position } => (
                id,
                "textDocument/hover",
                json!({"textDocument":{"uri":path_to_uri(&self.absolute_path(&path))?},"position":position}),
                RequestKind::Hover,
                Feature::Hover,
            ),
            ClientCommand::References { id, path, position } => (
                id,
                "textDocument/references",
                json!({"textDocument":{"uri":path_to_uri(&self.absolute_path(&path))?},"position":position,"context":{"includeDeclaration":true}}),
                RequestKind::References,
                Feature::References,
            ),
            ClientCommand::DocumentSymbols { id, path } => (
                id,
                "textDocument/documentSymbol",
                json!({"textDocument":{"uri":path_to_uri(&self.absolute_path(&path))?}}),
                RequestKind::DocumentSymbols,
                Feature::DocumentSymbols,
            ),
            ClientCommand::WorkspaceSymbols { id, query } => (
                id,
                "workspace/symbol",
                json!({ "query": query }),
                RequestKind::WorkspaceSymbols,
                Feature::WorkspaceSymbols,
            ),
            command => return self.position_request(command),
        };
        if self.features.load(Ordering::Acquire) & feature.bit() == 0 {
            self.failed_requests.push_back(LspEvent::RequestFailed {
                request_id: id,
                error: Some(format!(
                    "{} does not provide {}.",
                    self.config.name,
                    feature.label()
                )),
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
        self.queue(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
    }

    fn position_request(&mut self, command: ClientCommand) -> Result<(), String> {
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
            _ => true,
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

    pub(super) fn message(&mut self, message: Value, events: &EventEmitter) -> Result<(), String> {
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
                // Publish before Ready so the editor never offers an absent feature.
                self.features.store(
                    ALL_FEATURES
                        .iter()
                        .filter(|feature| feature.provided(capabilities))
                        .map(|feature| feature.bit())
                        .sum(),
                    Ordering::Release,
                );
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
            RequestKind::Hover => events.emit(LspEvent::Hover {
                request_id: id,
                contents: parse_hover(result),
            }),
            RequestKind::References => self.resolve_references(id, result, events),
            RequestKind::DocumentSymbols => events.emit(LspEvent::Symbols {
                request_id: id,
                symbols: parse_symbols(result, false),
            }),
            RequestKind::WorkspaceSymbols => events.emit(LspEvent::Symbols {
                request_id: id,
                symbols: parse_symbols(result, true),
            }),
        }
        Ok(())
    }

    /// Previews need source lines: the text this server analyzed for open
    /// documents, otherwise the file on disk. Reading files happens on a
    /// short-lived thread so the protocol loop keeps serving other requests.
    fn resolve_references(&self, id: u64, result: &Value, events: &EventEmitter) {
        let (locations, truncated) = parse_locations(result);
        let mut open = HashMap::new();
        for (path, _) in &locations {
            if let Some(document) = self.documents.get(path) {
                open.entry(path.clone())
                    .or_insert_with(|| document.text.clone());
            }
        }
        let emitter = events.clone();
        if let Err(error) = thread::Builder::new()
            .name("vedit-lsp-references".into())
            .spawn(move || {
                let references = build_references(locations, &open, read_preview_source);
                emitter.emit(LspEvent::References {
                    request_id: id,
                    references,
                    truncated,
                });
            })
        {
            events.emit(LspEvent::RequestFailed {
                request_id: id,
                error: Some(format!("Could not read reference previews: {error}")),
            });
        }
    }
}

/// Exactly the requests and notifications this client implements.
pub(super) fn client_capabilities() -> Value {
    let symbol_kinds = json!({"valueSet":(1..=26).collect::<Vec<u32>>()});
    json!({
        "general":{"positionEncodings":["utf-16"]},
        "workspace":{"configuration":true,"workspaceFolders":true,"applyEdit":false,
            "symbol":{"symbolKind":symbol_kinds}},
        "window":{"workDoneProgress":true},
        "textDocument":{
            "synchronization":{"dynamicRegistration":false,"didSave":true},
            "publishDiagnostics":{"versionSupport":true},
            "completion":{"contextSupport":true,"completionItem":{"snippetSupport":false,"insertReplaceSupport":true}},
            "definition":{"linkSupport":true},
            "hover":{"contentFormat":["markdown","plaintext"]},
            "references":{},
            "documentSymbol":{"hierarchicalDocumentSymbolSupport":true,"symbolKind":symbol_kinds}
        }
    })
}

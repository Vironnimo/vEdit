//! Process supervision, bounded event delivery and cancellation-safe teardown.
use super::*;

#[derive(Clone)]
pub(super) struct EventEmitter {
    sender: SyncSender<LspEvent>,
    ctx: Option<egui::Context>,
    /// Distinct from worker termination: a fatal Error must survive until the
    /// UI reads it, even after the worker has marked the server stopped.
    pub(super) shutdown_requested: Arc<AtomicBool>,
    backpressure_nanos: Arc<AtomicU64>,
}

impl EventEmitter {
    pub(super) fn new(sender: SyncSender<LspEvent>, ctx: Option<egui::Context>) -> Self {
        Self {
            sender,
            ctx,
            shutdown_requested: Arc::new(AtomicBool::new(false)),
            backpressure_nanos: Arc::new(AtomicU64::new(0)),
        }
    }

    pub(super) fn take_backpressure(&self) -> Duration {
        Duration::from_nanos(self.backpressure_nanos.swap(0, Ordering::AcqRel))
    }

    pub(super) fn emit(&self, mut event: LspEvent) {
        // Match canonical workspace/tab keys without doing filesystem work in
        // the UI's event polling path. All normal emitters run on this worker.
        match &mut event {
            LspEvent::Diagnostics { path, .. }
            | LspEvent::Completions { path, .. }
            | LspEvent::Definition { path, .. } => {
                *path = canonical_path(path);
            }
            LspEvent::Symbols { symbols, .. } => {
                let mut known = HashMap::new();
                for path in symbols.iter_mut().filter_map(|symbol| symbol.path.as_mut()) {
                    *path = known
                        .entry(path.clone())
                        .or_insert_with(|| canonical_path(path))
                        .clone();
                }
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
                    thread::sleep(BACKPRESSURE_RETRY_INTERVAL);
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

pub(super) enum Inbound {
    Message(Value),
    Written(u64),
    Error(String),
}

pub(super) fn send_inbound(
    sender: &SyncSender<Inbound>,
    wake: &SyncSender<()>,
    message: Inbound,
) -> bool {
    let sent = sender.send(message).is_ok();
    // Keep the queues bounded independently, but let either queue wake the
    // supervisor immediately. A capacity-one wake queue coalesces bursts and
    // preserves signals sent between draining the inputs and going to sleep.
    let _ = wake.try_send(());
    sent
}
#[allow(clippy::too_many_arguments)]
pub(super) fn run_server(
    root: PathBuf,
    config: ServerConfig,
    commands: Receiver<ClientCommand>,
    wake: &SyncSender<()>,
    wake_rx: Receiver<()>,
    events: &EventEmitter,
    stopped: &AtomicBool,
    features: Arc<AtomicU32>,
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
    let reader_wake = wake.clone();
    thread::Builder::new()
        .name("vedit-lsp-reader".into())
        .spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                match read_message(&mut reader) {
                    Ok(Some(message)) => {
                        if !send_inbound(&reader_tx, &reader_wake, Inbound::Message(message)) {
                            break;
                        }
                    }
                    Ok(None) => {
                        let _ = reader_wake.try_send(());
                        break;
                    }
                    Err(error) => {
                        send_inbound(
                            &reader_tx,
                            &reader_wake,
                            Inbound::Error(format!("Invalid language server response: {error}")),
                        );
                        break;
                    }
                }
            }
        })
        .map_err(|error| format!("Could not read language server: {error}"))?;
    let write_tx = inbound_tx.clone();
    let writer_wake = wake.clone();
    thread::Builder::new()
        .name("vedit-lsp-writer".into())
        .spawn(move || {
            let mut writer = stdin;
            for outbound in writer_rx {
                if let Err(error) = write_message(&mut writer, &outbound.message) {
                    send_inbound(
                        &write_tx,
                        &writer_wake,
                        Inbound::Error(format!("Could not update language server: {error}")),
                    );
                    break;
                }
                if !send_inbound(&write_tx, &writer_wake, Inbound::Written(outbound.token)) {
                    break;
                }
            }
        })
        .map_err(|error| format!("Could not write language server: {error}"))?;
    drop(inbound_tx);
    let stderr_tail = Arc::new(Mutex::new(Vec::<u8>::new()));
    let tail = Arc::clone(&stderr_tail);
    let (stderr_done, stderr_finished) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("vedit-lsp-stderr".into())
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
            let _ = stderr_done.send(());
        })
        .map_err(|error| format!("Could not read language server logs: {error}"))?;

    let mut protocol = Protocol {
        config,
        sync: SyncOptions::default(),
        supports_completion: false,
        supports_definition: false,
        features,
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
    protocol.queue(
        json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{
            "processId":std::process::id(), "rootUri":protocol.root_uri,
            "clientInfo":{"name":"vEdit","version":env!("CARGO_PKG_VERSION")},
            "workspaceFolders":protocol.workspace_folders(),
            "capabilities":client_capabilities(),
            "initializationOptions":protocol.config.initialization_options
        }}),
    )?;

    while !stopped.load(Ordering::Acquire) {
        protocol.extend_deadlines(events.take_backpressure());
        if let Some(status) = process
            .child
            .try_wait()
            .map_err(|error| format!("Could not monitor language server: {error}"))?
        {
            // Process exit can beat the final stderr read. Wait only on this
            // worker, and stay bounded if a descendant still holds the pipe.
            let _ = stderr_finished.recv_timeout(Duration::from_millis(200));
            let tail = stderr_tail
                .lock()
                .map(|tail| String::from_utf8_lossy(&tail).trim().to_owned())
                .unwrap_or_default();
            return Err(server_exit_message(
                &protocol.config.name,
                &protocol.root,
                &status.to_string(),
                &tail,
            ));
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
        for _ in 0..32 {
            match inbound_rx.try_recv() {
                Ok(Inbound::Message(message)) => protocol.message(message, events)?,
                Ok(Inbound::Written(token)) => {
                    protocol.writes.remove(&token);
                }
                Ok(Inbound::Error(error)) => return Err(error),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err(format!(
                        "{} disconnected unexpectedly.",
                        protocol.config.name
                    ));
                }
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
        // Commands, incoming messages, completed writes and shutdown wake this
        // wait. Idle servers only need a fallback process check, while pending
        // requests and writes retain their precise timeout deadlines.
        let _ = wake_rx.recv_timeout(protocol.wait_duration(Instant::now()));
    }
    // Dropping the guard stops the entire language server process tree. Waiting
    // for an LSP shutdown response here would make closing the editor fragile.
    Ok(())
}

fn server_exit_message(name: &str, root: &Path, status: &str, log: &str) -> String {
    let mut message = format!(
        "{name} analysis stopped ({status}).\nProject: {}\nUse Restart language analysis to reconnect. Editing is still available.",
        root.display()
    );
    let mut lines = log.lines();
    if let Some(panic) = lines.find(|line| line.contains("panicked at")) {
        message.push_str("\n\nCause: ");
        message.push_str(panic);
        if let Some(reason) = lines.next().filter(|line| !line.trim().is_empty()) {
            message.push('\n');
            message.push_str(reason);
        }
    }
    if !log.is_empty() {
        message.push_str("\n\nRecent server output (up to 16 KiB):\n");
        message.push_str(log);
    }
    message
}

#[test]
fn server_exit_reports_status_and_panic_before_older_warnings() {
    let log = "old WARN notify error\nthread 'LspServer' panicked at base-db/lib.rs:174:\nUnable to get FileSourceRootInput; this is a bug\nstack backtrace:\nframe 🦀";
    let message = server_exit_message("Rust", Path::new("C:/project"), "exit code: 101", log);
    assert_eq!(
        message.lines().next(),
        Some("Rust analysis stopped (exit code: 101).")
    );
    assert!(
        message.find("Unable to get FileSourceRootInput").unwrap()
            < message.find("old WARN").unwrap()
    );
    assert!(message.contains("C:/project"));
    assert!(message.ends_with(log));
    let empty = server_exit_message("Rust", Path::new("C:/project"), "exit code: 5", "");
    assert!(empty.contains("exit code: 5"));
    assert!(empty.contains("Restart language analysis"));
    assert!(!empty.contains("Install"));
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

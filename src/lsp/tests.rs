//! Shared protocol fixtures and the optional live Rust language-server test.
#[path = "tests/codec.rs"]
mod codec;
#[path = "tests/runtime.rs"]
mod runtime;
#[path = "tests/synchronization.rs"]
mod synchronization;

use super::*;
use std::fs;
use std::io::Cursor;
use std::time::{SystemTime, UNIX_EPOCH};

fn range(line: u32, start: u32, end: u32) -> Value {
    json!({"start":{"line":line,"character":start},"end":{"line":line,"character":end}})
}

pub(super) fn protocol() -> (Protocol, Receiver<Outbound>) {
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
            features: Arc::new(AtomicU32::new(
                ALL_FEATURES.iter().map(|feature| feature.bit()).sum(),
            )),
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

pub(super) fn initialize_protocol(protocol: &mut Protocol, capabilities: Value) {
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

pub(super) struct TestWorkspace(pub(super) PathBuf);
impl TestWorkspace {
    pub(super) fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "vedit-lsp-tests-{}-{stamp} with spaces ü",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join("src")).unwrap();
        fs::write(root.join("Cargo.toml"), "[package]\nname = \"vedit_lsp_smoke\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n").unwrap();
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
                .starts_with("vedit-lsp-tests-")
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
    let text = "pub fn double(value: i32) -> i32 { value * 2 }\npub fn run() { let _ = doub; }\n";
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
    let definition_position = char_to_position(updated, updated.rfind("double(4)").unwrap() + 2);
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
    // Allow process teardown after shutdown wakes the supervisor, before
    // the fixture cleans up the child processes' working directory.
    thread::sleep(Duration::from_millis(150));
}

//! Wakeups, bounded queues and lifecycle event delivery contracts.
use super::*;

#[test]
fn idle_supervisor_waits_for_events_and_keeps_request_deadlines() {
    let (mut protocol, _outgoing) = protocol();
    let now = Instant::now();
    assert_eq!(protocol.wait_duration(now), Duration::from_secs(1));

    protocol.pending.insert(
        1,
        PendingRequest {
            kind: RequestKind::Definition,
            deadline: now + Duration::from_millis(80),
        },
    );
    assert_eq!(protocol.wait_duration(now), Duration::from_millis(80));
    protocol.writes.insert(2, now + Duration::from_millis(30));
    assert_eq!(protocol.wait_duration(now), Duration::from_millis(30));
    assert_eq!(
        protocol.wait_duration(now + Duration::from_millis(31)),
        Duration::ZERO
    );
}

#[test]
fn client_commands_and_shutdown_wake_a_sleeping_supervisor() {
    let (_events, receiver) = mpsc::sync_channel(1);
    let (commands, command_rx) = mpsc::sync_channel(8);
    let (wake, wake_rx) = mpsc::sync_channel(1);
    let server = LanguageServer {
        receiver,
        commands,
        wake,
        stopped: Arc::new(AtomicBool::new(false)),
        shutdown_requested: Arc::new(AtomicBool::new(false)),
        features: Arc::new(AtomicU32::new(0)),
        instance: 0,
    };
    let (ready_tx, ready_rx) = mpsc::channel();
    let (received_tx, received_rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        ready_tx.send(()).unwrap();
        wake_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(command_rx.try_recv(), Ok(ClientCommand::Save(_))));
        received_tx.send(()).unwrap();
        wake_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        received_tx.send(()).unwrap();
    });
    ready_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(server.save(Path::new("example.rs")));
    received_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("an accepted command must interrupt an idle supervisor wait");
    server.shutdown();
    received_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("shutdown must interrupt an idle supervisor wait");
    worker.join().unwrap();
}

#[test]
fn inbound_messages_and_write_completions_coalesce_wakes_without_losing_input() {
    let (sender, receiver) = mpsc::sync_channel(8);
    let (wake, wake_rx) = mpsc::sync_channel(1);
    assert!(send_inbound(&sender, &wake, Inbound::Written(7)));
    assert!(send_inbound(
        &sender,
        &wake,
        Inbound::Message(json!({"id":2,"result":null})),
    ));
    assert!(wake_rx.try_recv().is_ok());
    assert!(wake_rx.try_recv().is_err());
    assert!(matches!(receiver.try_recv(), Ok(Inbound::Written(7))));
    assert!(matches!(receiver.try_recv(), Ok(Inbound::Message(_))));

    assert!(send_inbound(
        &sender,
        &wake,
        Inbound::Error("closed".into())
    ));
    assert!(wake_rx.try_recv().is_ok());
    assert!(matches!(receiver.try_recv(), Ok(Inbound::Error(_))));
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
    let (wake, _wake_rx) = mpsc::sync_channel(1);
    let server = LanguageServer {
        receiver,
        commands,
        wake,
        stopped: Arc::new(AtomicBool::new(true)),
        shutdown_requested: Arc::clone(&emitter.shutdown_requested),
        features: Arc::new(AtomicU32::new(0)),
        instance: 0,
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
    let (wake, _wake_rx) = mpsc::sync_channel(1);
    let server = LanguageServer {
        receiver,
        commands,
        wake,
        stopped: Arc::new(AtomicBool::new(false)),
        shutdown_requested: Arc::clone(&emitter.shutdown_requested),
        features: Arc::new(AtomicU32::new(0)),
        instance: 0,
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

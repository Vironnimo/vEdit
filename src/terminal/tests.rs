use super::*;
use alacritty_terminal::{
    grid::Dimensions,
    index::{Column, Line, Point, Side},
    selection::{Selection, SelectionType},
};
use egui::{Event, Key, Modifiers};

fn key(key: Key, modifiers: Modifiers) -> Event {
    Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}

fn visible_text(session: &TerminalSession) -> String {
    session
        .term
        .lock()
        .unwrap()
        .grid()
        .display_iter()
        .map(|indexed| indexed.cell.c)
        .collect()
}

#[test]
fn session_interprets_output_and_answers_terminal_protocol_queries() {
    let mut session = TerminalSession::in_memory();
    let bytes = "Grüße 🦀".as_bytes();
    for byte in bytes {
        session.feed_output(&[*byte]);
    }
    session.feed_output(b"\x1b]2;Build\x07\x1b[6n\x1b]11;?\x07");
    assert_eq!(session.title(), "Build");
    assert!(visible_text(&session).contains("Grüße 🦀"));
    let responses = String::from_utf8(session.take_input_bytes()).unwrap();
    assert!(responses.contains("\x1b[1;9R"), "{responses:?}");
    assert!(responses.contains("rgb:1515/1313/0f0f"), "{responses:?}");
    session.resize(PtySize {
        cols: 120,
        rows: 30,
        pixel_width: 1200,
        pixel_height: 600,
    });
    let term = session.term.lock().unwrap();
    assert_eq!((term.columns(), term.screen_lines()), (120, 30));
}

#[test]
fn shell_input_preserves_control_keys_unicode_altgr_and_native_clipboard_pairs() {
    let mut session = TerminalSession::in_memory();
    session.handle_events(&[
        Event::Text("cargo test".into()),
        key(Key::Enter, Modifiers::NONE),
        key(Key::ArrowUp, Modifiers::NONE),
        key(Key::Tab, Modifiers::NONE),
        key(Key::R, Modifiers::CTRL),
        key(Key::C, Modifiers::CTRL),
        Event::Copy,
        key(Key::V, Modifiers::CTRL),
        Event::Paste("Grüße\r\nnext".into()),
        key(
            Key::Q,
            Modifiers {
                ctrl: true,
                alt: true,
                ..Modifiers::NONE
            },
        ),
        Event::Text("@".into()),
    ]);
    assert_eq!(
        session.take_input_bytes(),
        "cargo test\r\x1b[A\t\x12\x03Grüße\rnext@".as_bytes()
    );
    session.feed_output(b"\x1b[?1h\x1b[?2004h");
    session.handle_events(&[
        key(Key::ArrowUp, Modifiers::NONE),
        Event::Paste("one\ntwo\x1b[201~".into()),
    ]);
    assert_eq!(
        session.take_input_bytes(),
        b"\x1bOA\x1b[200~one\ntwo[201~\x1b[201~"
    );
    session.handle_events(&[
        key(
            Key::C,
            Modifiers {
                ctrl: true,
                shift: true,
                ..Modifiers::NONE
            },
        ),
        Event::Copy,
    ]);
    assert!(session.take_input_bytes().is_empty());
}

#[test]
fn selected_terminal_text_copies_instead_of_interrupting_the_shell() {
    let mut session = TerminalSession::in_memory();
    session.feed_output(b"copy me");
    let mut selection = Selection::new(
        SelectionType::Simple,
        Point::new(Line(0), Column(0)),
        Side::Left,
    );
    selection.update(Point::new(Line(0), Column(6)), Side::Right);
    session.term.lock().unwrap().selection = Some(selection);
    let ctx = session.ctx.clone();
    let mut output = ctx.run_ui(egui::RawInput::default(), |_| {
        session.handle_events(&[key(Key::C, Modifiers::CTRL), Event::Copy]);
    });
    assert!(session.take_input_bytes().is_empty());
    assert!(
        output
            .platform_output
            .commands
            .iter()
            .any(|command| matches!(command,
        egui::OutputCommand::CopyText(text) if text == "copy me"))
    );
    output.textures_delta.clear();
}

#[test]
fn scrollback_is_bounded_and_widget_keeps_terminal_navigation_focus() {
    let mut session = TerminalSession::in_memory();
    for _ in 0..10_100 {
        session.feed_output(b"line\r\n");
    }
    assert!(session.term.lock().unwrap().grid().history_size() <= MAX_HISTORY_LINES);
    let ctx = egui::Context::default();
    let id = egui::Id::new("terminal-contract");
    let screen_rect = Some(egui::Rect::from_min_size(
        egui::Pos2::ZERO,
        egui::vec2(640.0, 400.0),
    ));
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect,
            ..Default::default()
        },
        |ui| {
            session.show(ui, id, true, 12.0);
        },
    );
    output.textures_delta.clear();
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect,
            events: vec![
                key(Key::Tab, Modifiers::NONE),
                key(Key::Escape, Modifiers::NONE),
            ],
            ..Default::default()
        },
        |ui| {
            let response = session.show(ui, id, false, 12.0);
            assert!(response.has_focus());
        },
    );
    output.textures_delta.clear();
    assert_eq!(session.take_input_bytes(), b"\t\x1b");
    for _ in 0..20 {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect,
                events: vec![
                    Event::PointerMoved(egui::pos2(100.0, 100.0)),
                    Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(0.0, 3.0),
                        phase: egui::TouchPhase::Move,
                        modifiers: Modifiers::NONE,
                    },
                ],
                ..Default::default()
            },
            |ui| {
                session.show(ui, id, false, 12.0);
            },
        );
        output.textures_delta.clear();
    }
    assert!(session.term.lock().unwrap().grid().display_offset() > 0);
}

#[cfg(windows)]
#[test]
#[ignore = "manual integration: Windows ConPTY and PowerShell process lifecycle"]
fn native_powershell_supports_history_interruption_resize_and_tree_cleanup() {
    use std::time::{Duration, Instant};
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(".tmp/terminal-native")
        .join(format!("{}-{serial}", std::process::id()));
    let project = root.join("Grüße project");
    let history = root.join("history");
    std::fs::create_dir_all(&project).unwrap();
    let pid_path = root.join("child.pid");
    let spawn = || {
        let pid_path = pid_path.clone();
        TerminalSession::spawn(
            &project,
            &history,
            move || {
                vec![(
                    OsString::from("VEDIT_TEST_PID_PATH"),
                    pid_path.into_os_string(),
                )]
            },
            egui::Context::default(),
        )
        .unwrap()
    };
    let wait_text = |session: &TerminalSession, needle: &str| {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !visible_text(session).contains(needle) {
            assert!(
                session.is_running(),
                "{}: {}",
                session.status(),
                visible_text(session)
            );
            assert!(
                Instant::now() < deadline,
                "missing {needle:?}: {}",
                visible_text(session)
            );
            thread::sleep(Duration::from_millis(10));
        }
    };
    let command = |session: &mut TerminalSession, script: &str| {
        session.handle_events(&[Event::Text(script.into()), key(Key::Enter, Modifiers::NONE)]);
    };
    let stop = |session: &mut TerminalSession| {
        session.stop();
        let deadline = Instant::now() + Duration::from_secs(10);
        while session.is_running() {
            assert!(Instant::now() < deadline, "terminal did not stop");
            thread::sleep(Duration::from_millis(10));
        }
    };

    let mut session = spawn();
    wait_text(&session, "PS ");
    command(&mut session, "Write-Output ('Grü'+'ße 🦀')");
    wait_text(&session, "Grüße 🦀");
    session.resize(PtySize {
        cols: 120,
        rows: 30,
        pixel_width: 1200,
        pixel_height: 600,
    });
    command(
        &mut session,
        "Write-Output ('size:'+[Console]::WindowWidth+'x'+[Console]::WindowHeight)",
    );
    wait_text(&session, "size:120x30");
    command(
        &mut session,
        "Write-Output ('sleep'+'ing'); Start-Sleep -Seconds 90",
    );
    wait_text(&session, "sleeping");
    let prompts_before_interrupt = visible_text(&session).matches("PS ").count();
    session.handle_events(&[key(Key::C, Modifiers::CTRL)]);
    let deadline = Instant::now() + Duration::from_secs(10);
    while visible_text(&session).matches("PS ").count() <= prompts_before_interrupt {
        assert!(
            Instant::now() < deadline,
            "Ctrl+C did not return to the prompt: {}",
            visible_text(&session)
        );
        thread::sleep(Duration::from_millis(10));
    }
    command(&mut session, "Write-Output ('inter'+'rupted')");
    wait_text(&session, "interrupted");
    command(&mut session, "Write-Output ('terminal-'+'history')");
    wait_text(&session, "terminal-history");
    stop(&mut session);
    drop(session);

    let other_project = root.join("Other project");
    std::fs::create_dir_all(&other_project).unwrap();
    let mut other =
        TerminalSession::spawn(&other_project, &history, Vec::new, egui::Context::default())
            .unwrap();
    wait_text(&other, "PS ");
    other.handle_events(&[
        key(Key::ArrowUp, Modifiers::NONE),
        key(Key::Enter, Modifiers::NONE),
    ]);
    command(&mut other, "Write-Output ('isol'+'ated')");
    wait_text(&other, "isolated");
    assert!(
        !visible_text(&other).contains("terminal-history"),
        "project history leaked: {}",
        visible_text(&other)
    );
    stop(&mut other);
    drop(other);

    let mut session = spawn();
    wait_text(&session, "PS ");
    session.handle_events(&[
        key(Key::ArrowUp, Modifiers::NONE),
        key(Key::Enter, Modifiers::NONE),
    ]);
    wait_text(&session, "terminal-history");
    command(
        &mut session,
        "$child = Start-Process -FilePath (Get-Process -Id $PID).Path -ArgumentList '-NoProfile','-Command','Start-Sleep -Seconds 90' -WindowStyle Hidden -PassThru; [IO.File]::WriteAllText($env:VEDIT_TEST_PID_PATH, [string]$child.Id)",
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while !pid_path.is_file() {
        assert!(
            Instant::now() < deadline,
            "child was not started: {}",
            visible_text(&session)
        );
        thread::sleep(Duration::from_millis(10));
    }
    let pid = std::fs::read_to_string(&pid_path)
        .unwrap()
        .parse::<u32>()
        .unwrap();
    assert!(crate::process_liveness::running(pid));
    stop(&mut session);
    let deadline = Instant::now() + Duration::from_secs(10);
    while crate::process_liveness::running(pid) {
        assert!(
            Instant::now() < deadline,
            "terminal descendant survived closing"
        );
        thread::sleep(Duration::from_millis(10));
    }
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}

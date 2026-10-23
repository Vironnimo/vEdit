use super::*;
use egui_kittest::{Harness, kittest::Queryable};

const SUMMARY: &str = "Rust analysis stopped (exit code: 101).";

fn message() -> String {
    format!(
        "{SUMMARY}\r\nCause: missing source root 🦀\r\n{}",
        "stack frame ü\r\n".repeat(250)
    )
}

fn harness() -> Harness<'static, VEditApp> {
    let mut app = VEditApp::empty(egui::Context::default());
    visual_tests::workspace(&mut app);
    app.settings.autosave = false;
    app.status(message(), true);
    let mut screen = Harness::builder()
        .with_size([1100.0, 760.0])
        .with_os(egui::os::OperatingSystem::Windows)
        .build_ui_state(
            |ui, app: &mut VEditApp| {
                app.ctx = ui.ctx().clone();
                app.draw_ui(ui);
            },
            app,
        );
    set_style(&screen.ctx);
    screen.ctx.all_styles_mut(|style| {
        style.animation_time = 0.0;
        style.scroll_animation = egui::style::ScrollAnimation::none();
        style.visuals.text_cursor.blink = false;
    });
    screen.run_steps(4);
    screen
}

fn copy_button(screen: &mut Harness<'_, VEditApp>, label: &str) -> Vec<String> {
    let pos = screen.get_by_label(label).rect().center();
    click_at(screen, pos)
}

fn click_at(screen: &mut Harness<'_, VEditApp>, pos: egui::Pos2) -> Vec<String> {
    let mut copied = Vec::new();
    for event in [
        egui::Event::PointerMoved(pos),
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        },
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        },
    ] {
        screen.event(event);
        screen.run_steps(1);
        for command in &screen.output().platform_output.commands {
            if let egui::OutputCommand::CopyText(text) = command {
                copied.push(text.clone());
            }
        }
    }
    copied
}

#[test]
fn status_context_menu_copies_the_complete_multiline_message() {
    let mut screen = harness();
    screen.get_by_label(SUMMARY).click_secondary();
    screen.run_steps(3);
    assert_eq!(copy_button(&mut screen, "Copy message"), vec![message()]);
    assert!(screen.state().status_details.is_none());
}

#[test]
fn details_keep_the_original_message_and_protect_editor_input() {
    let mut screen = harness();
    let original = screen.state().tab().unwrap().text.clone();
    screen.get_by_label(SUMMARY).click();
    screen.run_steps(3);
    assert_eq!(screen.state().control_dialog(), Some("message_details"));
    screen.state_mut().status("A later status", false);
    screen.run_steps(2);
    assert_eq!(copy_button(&mut screen, "Copy all"), vec![message()]);
    screen.event(egui::Event::Text("dd".into()));
    screen.run_steps(2);
    assert_eq!(screen.state().tab().unwrap().text, original);
    let observation = screen
        .state_mut()
        .control_request(crate::cli::Request::Status {});
    assert!(observation.ok);
    assert_eq!(observation.result["dialog"], "message_details");
    assert_eq!(
        observation.result["status_message"]["text"],
        "A later status"
    );
    assert_eq!(observation.result["message_details"]["text"], message());
    assert_eq!(observation.result["message_details"]["error"], true);
    let tab = screen.state().tab().unwrap();
    let request = crate::cli::Request::Command {
        tab_id: tab.id,
        command: crate::cli::EditorCommand::DeleteLine,
        expected_revision: tab.revision,
    };
    let mutation = screen.state_mut().control_request(request);
    assert_eq!(mutation.error.unwrap().code, "user_dialog_open");
    screen.get_by_label("Close").click();
    screen.run_steps(3);
    assert!(screen.state().status_details.is_none());
    assert_eq!(screen.state().tab().unwrap().text, original);
}

#[test]
fn details_allow_select_all_and_copy_but_not_editing() {
    let mut screen = harness();
    screen.get_by_label(SUMMARY).click();
    screen.run_steps(3);
    // Focus the read-only text using the actual accessibility node.
    use egui::accesskit::Role;
    let rect = screen
        .get_all_by_role(Role::MultilineTextInput)
        .find(|node| node.value().is_some_and(|value| value.starts_with(SUMMARY)))
        .expect("message text field")
        .rect();
    click_at(&mut screen, rect.min + Vec2::splat(12.0));
    screen.run_steps(2);
    let modifiers = egui::Modifiers {
        ctrl: true,
        command: true,
        ..Default::default()
    };
    screen.event(egui::Event::Key {
        key: Key::A,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    });
    screen.run_steps(1);
    screen.event(egui::Event::Copy);
    screen.run_steps(1);
    assert!(screen.output().platform_output.commands.iter().any(
        |command| matches!(command, egui::OutputCommand::CopyText(text) if text == &message())
    ));
    screen.event(egui::Event::Text("replacement".into()));
    screen.event(egui::Event::Paste("pasted".into()));
    screen.run_steps(2);
    assert_eq!(copy_button(&mut screen, "Copy all"), vec![message()]);
    screen.event(egui::Event::Key {
        key: Key::Escape,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    });
    screen.run_steps(2);
    assert!(screen.state().status_details.is_none());
}

#[test]
fn error_details_render_with_bounded_height() {
    let mut screen = harness();
    screen.get_by_label(SUMMARY).click();
    screen.run_steps(3);
    assert!(screen.get_by_label("Copy all").rect().max.y < 760.0);
    if std::env::var_os("VEDIT_STATUS_PREVIEW").is_some() {
        screen.snapshot_options(
            "status_error_details",
            &egui_kittest::SnapshotOptions::new()
                .output_path(PathBuf::from(".tmp/rust-analysis-investigation/preview")),
        );
    }
}

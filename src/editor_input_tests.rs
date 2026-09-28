//! Native input reaches TextEdit before application-owned editing commands.
use super::*;
use egui_kittest::Harness;

fn fixture(text: &str, vim: bool) -> MorrowApp {
    let mut app = MorrowApp::empty(egui::Context::default());
    app.settings.vim = vim;
    app.settings.autosave = false;
    app.settings.smooth_scroll = false;
    app.sidebar_visible = false;
    let mut project = Project::new(std::env::temp_dir().join("morrow-editor-input-unit"));
    let mut tab = EditorTab::new(None, "input.txt".into());
    tab.text = text.into();
    tab.saved_text = text.into();
    project.tabs.push(tab);
    app.projects.push(project);
    app.bind_current_pane();
    app
}

fn harness(text: &str, vim: bool) -> Harness<'static, MorrowApp> {
    let mut screen = Harness::builder()
        .with_size([1100.0, 740.0])
        .with_os(egui::os::OperatingSystem::Windows)
        .build_ui_state(
            |ui, app: &mut MorrowApp| {
                app.ctx = ui.ctx().clone();
                app.shortcuts();
                app.handle_vim();
                app.editor_ui(ui);
            },
            fixture(text, vim),
        );
    set_style(&screen.ctx);
    screen.run_steps(3);
    screen
}

fn key(key: Key, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers {
            command: modifiers.ctrl,
            ..modifiers
        },
    }
}

/// Custom commands resume only after their native prefix has been rendered.
/// Wait for that explicit state transition, never for animation or wall time.
pub(super) fn settle(screen: &mut Harness<'_, MorrowApp>) {
    for _ in 0..32 {
        if !screen.state().editor_input_pending() {
            return;
        }
        screen.run_steps(1);
    }
    panic!("source input continuation did not drain");
}

fn events(screen: &mut Harness<'_, MorrowApp>, events: impl IntoIterator<Item = egui::Event>) {
    screen.input_mut().events.extend(events);
    screen.run_steps(1);
    settle(screen);
}

fn text<'a>(screen: &'a Harness<'_, MorrowApp>) -> &'a str {
    &screen.state().tab().unwrap().text
}

#[test]
fn native_text_undo_text_preserves_order_and_appends_new_keyboard_input() {
    let mut screen = harness("tail", false);
    screen.input_mut().events.extend([
        egui::Event::Text("X".into()),
        key(Key::Z, egui::Modifiers::CTRL),
        egui::Event::Text("Y".into()),
    ]);
    screen.run_steps(1);
    assert_eq!(text(&screen), "Xtail");
    assert!(screen.state().editor_input_pending());
    // This later event follows the saved suffix without invalidating its owner.
    screen
        .input_mut()
        .events
        .push(egui::Event::Text("Z".into()));
    settle(&mut screen);
    assert_eq!(text(&screen), "YZtail");
    events(&mut screen, [key(Key::Z, egui::Modifiers::CTRL)]);
    assert_eq!(text(&screen), "tail");
    events(&mut screen, [key(Key::Y, egui::Modifiers::CTRL)]);
    assert_eq!(text(&screen), "YZtail");
}

#[test]
fn native_text_enter_and_tab_use_the_preceding_text_and_native_caret() {
    let mut screen = harness("", false);
    events(
        &mut screen,
        [
            egui::Event::Text("a".into()),
            key(Key::Enter, egui::Modifiers::NONE),
            egui::Event::Text("b".into()),
            key(Key::Tab, egui::Modifiers::NONE),
            egui::Event::Text("🦀".into()),
        ],
    );
    assert_eq!(text(&screen), "a\nb    🦀");
    assert_eq!(screen.state().tab().unwrap().selection, 8..8);
    events(&mut screen, [key(Key::Z, egui::Modifiers::CTRL)]);
    assert_eq!(text(&screen), "a\nb");
    events(&mut screen, [key(Key::Y, egui::Modifiers::CTRL)]);
    assert_eq!(text(&screen), "a\nb    🦀");
}

#[test]
fn a_leading_native_command_applies_immediately() {
    let mut screen = harness("tail", false);
    screen
        .input_mut()
        .events
        .push(key(Key::Enter, egui::Modifiers::NONE));
    screen.run_steps(1);
    assert_eq!(text(&screen), "\ntail");
    assert_eq!(screen.state().tab().unwrap().selection, 1..1);
    assert!(!screen.state().editor_input_pending());
    events(&mut screen, [key(Key::Z, egui::Modifiers::CTRL)]);
    assert_eq!(text(&screen), "tail");
    events(&mut screen, [key(Key::Y, egui::Modifiers::CTRL)]);
    assert_eq!(text(&screen), "\ntail");
    assert_eq!(screen.state().tab().unwrap().selection, 1..1);
}

#[test]
fn vim_native_word_motion_and_paste_match_separate_frames() {
    let mut batched = harness("one two", true);
    let mut separate = harness("one two", true);
    for screen in [&mut batched, &mut separate] {
        events(screen, [egui::Event::Text("A".into())]);
    }
    events(
        &mut batched,
        [
            key(Key::ArrowLeft, egui::Modifiers::CTRL),
            egui::Event::Paste("X".into()),
        ],
    );
    events(&mut separate, [key(Key::ArrowLeft, egui::Modifiers::CTRL)]);
    events(&mut separate, [egui::Event::Paste("X".into())]);
    assert_eq!(text(&batched), "one Xtwo");
    assert_eq!(text(&batched), text(&separate));
    assert_eq!(
        batched.state().tab().unwrap().selection,
        separate.state().tab().unwrap().selection
    );
    events(&mut batched, [key(Key::Z, egui::Modifiers::CTRL)]);
    assert_eq!(text(&batched), "one two");
}

#[test]
fn vim_word_deletion_before_enter_and_escape_keeps_native_modifier_semantics() {
    for (entry, deletion) in [("A", Key::Backspace), ("i", Key::Delete)] {
        let mut batched = harness("one two", true);
        let mut separate = harness("one two", true);
        for screen in [&mut batched, &mut separate] {
            events(screen, [egui::Event::Text(entry.into())]);
        }
        events(
            &mut batched,
            [
                key(deletion, egui::Modifiers::CTRL),
                key(Key::Enter, egui::Modifiers::NONE),
                egui::Event::Text("🦀".into()),
                key(Key::Escape, egui::Modifiers::NONE),
            ],
        );
        events(&mut separate, [key(deletion, egui::Modifiers::CTRL)]);
        events(&mut separate, [key(Key::Enter, egui::Modifiers::NONE)]);
        events(&mut separate, [egui::Event::Text("🦀".into())]);
        events(&mut separate, [key(Key::Escape, egui::Modifiers::NONE)]);
        assert_eq!(text(&batched), text(&separate), "deletion={deletion:?}");
        assert_eq!(batched.state().tab().unwrap().vim.mode, VimMode::Normal);
        events(&mut batched, [egui::Event::Text("u".into())]);
        assert_eq!(text(&batched), "one two");
    }
}

#[test]
fn pending_native_suffix_is_cancelled_after_external_source_change() {
    let mut screen = harness("tail", false);
    screen.input_mut().events.extend([
        egui::Event::Text("X".into()),
        key(Key::Z, egui::Modifiers::CTRL),
        egui::Event::Text("Y".into()),
    ]);
    screen.run_steps(1);
    assert!(screen.state().editor_input_pending());
    let tab = screen.state_mut().tab_mut().unwrap();
    tab.text = "external".into();
    tab.changed();
    settle(&mut screen);
    assert_eq!(text(&screen), "external");
}

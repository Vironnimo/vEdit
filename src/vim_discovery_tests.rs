//! Discoverability stays in the source Pane without taking unfinished input.
use super::*;
use egui_kittest::{Harness, kittest::Queryable};

fn harness() -> Harness<'static, VEditApp> {
    let mut app = VEditApp::empty(egui::Context::default());
    visual_tests::workspace(&mut app);
    let mut screen = visual_tests::harness(app);
    visual_tests::settle_render(&mut screen);
    screen
}

fn input(screen: &mut Harness<'_, VEditApp>, text: &str) {
    screen.event(egui::Event::Text(text.into()));
    screen.run_steps(2);
}

fn at(screen: &mut Harness<'_, VEditApp>, time: f64) {
    screen.input_mut().time = Some(time);
    screen.run_steps(1);
}

fn optional_image(screen: &mut Harness<'_, VEditApp>, name: &str) {
    if let Some(directory) = std::env::var_os("VEDIT_VIM_DISCOVERY_IMAGES") {
        let directory = PathBuf::from(directory);
        fs::create_dir_all(&directory).unwrap();
        screen.render().unwrap().save(directory.join(name)).unwrap();
    }
}

#[test]
fn vim_command_reference_search_owns_input_and_restores_the_source_focus() {
    let mut screen = harness();
    let document = screen.state().tab().unwrap().id;
    let before = screen.state().tab().unwrap().text.clone();
    let cursor = screen.state().tab().unwrap().vim.cursor;
    screen.get_by_label("Vim commands").click();
    screen.run_steps(3);
    assert_eq!(screen.state().control_dialog(), Some("vim_commands"));
    input(&mut screen, "register");
    assert!(
        screen
            .query_by_label_contains("Use a named register")
            .is_some()
    );
    assert!(
        screen.query_by_label("h j k l").is_none(),
        "search filters unrelated commands"
    );
    assert_eq!(screen.state().tab().unwrap().text, before);
    assert_eq!(screen.state().tab().unwrap().vim.cursor, cursor);
    optional_image(&mut screen, "vim-reference.png");
    screen.event(vim_macros::key(Key::Escape, egui::Modifiers::NONE));
    screen.run_steps(3);
    assert!(!screen.state().vim_reference_open());
    let widget = screen.state().document_widget_id(document);
    assert!(screen.ctx.memory(|memory| memory.has_focus(widget)));
    input(&mut screen, "l");
    assert_eq!(screen.state().tab().unwrap().vim.cursor, cursor + 1);
}

#[test]
fn vim_pending_hints_wait_for_the_delay_follow_the_toggle_and_complete_normally() {
    let mut screen = harness();
    let before = screen.state().tab().unwrap().text.clone();
    screen.event(egui::Event::Text("d".into()));
    screen.run_steps(1);
    let started = screen.ctx.input(|input| input.time);
    assert!(screen.query_by_label("d …").is_none());
    at(&mut screen, started + 0.2);
    assert!(screen.query_by_label("d …").is_none());
    at(&mut screen, started + 0.4);
    assert!(screen.query_by_label("d …").is_some());
    assert!(screen.query_by_label("w / b / e").is_some());
    assert_eq!(
        screen.state().tab().unwrap().text,
        before,
        "hints do not complete the operator"
    );
    // A newly shown Area measures its size before its first painted frame.
    at(&mut screen, started + 0.41);
    let last_hint = screen.get_by_label("Apply through a search").rect();
    let footer = screen
        .get_by_label("Esc cancels · ? beside the mode opens all Vim commands")
        .rect();
    assert!(
        last_hint.max.y <= footer.min.y && last_hint.height() >= 12.0,
        "all operator continuations fit above the footer"
    );
    optional_image(&mut screen, "vim-hints.png");
    screen.state_mut().settings.vim_command_hints = false;
    screen.run_steps(1);
    assert!(screen.query_by_label("d …").is_none());
    screen.state_mut().settings.vim_command_hints = true;
    screen.run_steps(1);
    let restarted = screen.ctx.input(|input| input.time);
    at(&mut screen, restarted + 0.4);
    assert!(screen.query_by_label("d …").is_some());
    input(&mut screen, "w");
    assert_ne!(screen.state().tab().unwrap().text, before);
    assert!(screen.query_by_label("d …").is_none());
    screen.state_mut().vim_registers.set_text(
        'a',
        Yank {
            text: "inserted".into(),
            linewise: false,
            block_width: None,
        },
    );
    input(&mut screen, "i");
    screen.event(vim_macros::key(
        Key::R,
        egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
    ));
    screen.run_steps(1);
    let register_started = screen.ctx.input(|input| input.time);
    at(&mut screen, register_started + 0.4);
    assert!(screen.query_by_label("^R …").is_some());
    input(&mut screen, "a");
    assert!(screen.state().tab().unwrap().text.contains("inserted"));
    assert!(screen.query_by_label("^R …").is_none());
}

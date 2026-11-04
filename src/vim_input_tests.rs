//! Vim Insert input integration: native text, IME composition and mode handover.
use super::*;
use egui_kittest::Harness;

fn harness(text: &str) -> Harness<'static, VEditApp> {
    let mut app = VEditApp::empty(egui::Context::default());
    app.settings.vim = true;
    app.settings.autosave = false;
    app.settings.smooth_scroll = false;
    app.sidebar_visible = false;
    let mut project = Project::new(std::env::temp_dir().join("vedit-vim-input"));
    let mut tab = EditorTab::new(None, "input.txt".into());
    tab.text = text.into();
    tab.saved_text = text.into();
    project.tabs.push(tab);
    app.projects.push(project);
    app.bind_current_pane();
    let mut screen = Harness::builder()
        .with_size([1000.0, 700.0])
        .with_os(egui::os::OperatingSystem::Windows)
        .build_ui_state(
            |ui, app: &mut VEditApp| {
                app.ctx = ui.ctx().clone();
                app.prepare_vim_macros();
                app.shortcuts();
                app.handle_vim();
                app.editor_ui(ui);
                app.observe_navigation();
                app.finish_vim_macros();
            },
            app,
        );
    set_style(&screen.ctx);
    screen.run_steps(3);
    screen
}

fn text(screen: &Harness<'_, VEditApp>) -> String {
    screen.state().tab().unwrap().text.clone()
}

fn input(screen: &mut Harness<'_, VEditApp>, value: &str) {
    screen
        .input_mut()
        .events
        .push(egui::Event::Text(value.into()));
    screen.run_steps(1);
}

fn ime(screen: &mut Harness<'_, VEditApp>, event: egui::ImeEvent) {
    screen.input_mut().events.push(egui::Event::Ime(event));
    screen.run_steps(1);
}

fn preedit() -> egui::ImeEvent {
    egui::ImeEvent::Preedit {
        text: "にほ".into(),
        active_range_chars: Some(0..2),
    }
}

#[test]
fn insert_mode_shows_the_ime_composition_before_its_commit() {
    let mut screen = harness("abc");
    input(&mut screen, "i");
    assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::Insert);
    ime(&mut screen, preedit());
    assert_eq!(
        text(&screen),
        "にほabc",
        "the source widget draws the composition before the commit"
    );
    ime(&mut screen, egui::ImeEvent::Commit("日本".into()));
    assert_eq!(text(&screen), "日本abc");
    assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::Insert);
    input(&mut screen, "x");
    assert_eq!(text(&screen), "日本xabc");
}

#[test]
fn replace_mode_applies_ime_text_without_a_native_composition() {
    let mut screen = harness("abc");
    input(&mut screen, "R");
    ime(&mut screen, preedit());
    assert_eq!(
        text(&screen),
        "abc",
        "Replace applies text itself and keeps the buffer unchanged while composing"
    );
    ime(&mut screen, egui::ImeEvent::Commit("X".into()));
    assert_eq!(text(&screen), "Xbc");
}

use super::*;
use egui_kittest::{Harness, kittest::Queryable};

fn harness(size: [f32; 2]) -> Harness<'static, String> {
    let mut screen = Harness::builder().with_size(size).build_ui_state(
        |ui, text: &mut String| {
            begin_hover_frame(ui.ctx());
            ui.button("Details").on_hover_details(text.as_str());
            ui.button("Hint").on_hover_text("A short instruction");
            end_hover_frame(ui.ctx());
        },
        format!(
            "C:\\Projekt\\Änderungen 🦀\r\n{}",
            "Long detail line\r\n".repeat(80)
        ),
    );
    screen.ctx.all_styles_mut(|style| {
        style.interaction.tooltip_delay = 0.0;
        style.interaction.tooltip_grace_time = 0.0;
        style.interaction.show_tooltips_only_when_still = false;
        style.animation_time = 0.0;
        style.scroll_animation = egui::style::ScrollAnimation::none();
    });
    screen.run_steps(2);
    screen
}

fn click(screen: &mut Harness<'_, String>, pos: egui::Pos2) -> Vec<String> {
    let mut copied = Vec::new();
    for event in [
        Event::PointerMoved(pos),
        Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        },
        Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        },
    ] {
        screen.event(event);
        screen.run_steps(1);
        copied.extend(screen.output().platform_output.commands.iter().filter_map(
            |cmd| match cmd {
                egui::OutputCommand::CopyText(text) => Some(text.clone()),
                _ => None,
            },
        ));
    }
    copied
}

#[test]
fn details_survive_entering_and_scrolling_and_copy_the_full_snapshot() {
    for size in [[700.0, 480.0], [320.0, 220.0]] {
        let mut screen = harness(size);
        let original = screen.state().clone();
        screen.get_by_label("Details").hover();
        screen.run_steps(3);
        let rect = state(&screen.ctx).open.unwrap().rect;
        assert!(rect.max.y < size[1]);
        assert!(rect.max.x < size[0]);
        screen.event(Event::PointerMoved(rect.center()));
        screen.run_steps(2);
        screen.event(Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -120.0),
            modifiers: egui::Modifiers::NONE,
            phase: egui::TouchPhase::Move,
        });
        screen.run_steps(2);
        *screen.state_mut() = "Replaced in background".into();
        screen.run_steps(1);
        let copy = screen.get_by_label("Copy all").rect().center();
        assert_eq!(click(&mut screen, copy), vec![original], "window: {size:?}");
        screen.key_press(egui::Key::Escape);
        screen.run_steps(2);
        assert!(screen.query_by_label("Copy all").is_none());
    }
}

#[test]
fn details_select_text_without_leaking_keys_and_hints_remain_passive() {
    let mut screen = harness([700.0, 480.0]);
    screen.get_by_label("Hint").hover();
    screen.run_steps(3);
    assert!(screen.query_by_label("A short instruction").is_some());
    assert!(screen.query_by_label("Copy all").is_none());
    screen.get_by_label("Details").hover();
    screen.run_steps(3);
    let text = screen.get_by_label_contains("C:\\Projekt").rect();
    click(&mut screen, text.min + egui::vec2(20.0, 5.0));
    screen.event(Event::Key {
        key: egui::Key::A,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
    });
    screen.run_steps(1);
    screen.event(Event::Copy);
    screen.run_steps(1);
    assert!(
        screen
            .output()
            .platform_output
            .commands
            .iter()
            .any(|c| matches!(c, egui::OutputCommand::CopyText(t) if t.contains("Projekt")))
    );
    assert!(screen.ctx.input(|i| !i.events.iter().any(keyboard)));
    screen.event(Event::PointerMoved(egui::pos2(650.0, 450.0)));
    screen.run_steps(25);
    assert!(screen.query_by_label("Copy all").is_none());
}

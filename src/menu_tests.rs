use super::*;
use egui_kittest::{Harness, kittest::Queryable};

fn menu_harness() -> Harness<'static> {
    Harness::builder().with_size([640.0, 320.0]).build_ui(|ui| {
        egui::MenuBar::new().ui(ui, |ui| {
            menu(ui, "File", "File actions", |ui| {
                if menu_item(ui, "Open document", "Ctrl+O", "Open a file").clicked() {
                    ui.close();
                }
            });
            menu(ui, "View", "View actions", |ui| {
                if menu_item(ui, "Toggle sidebar", "Ctrl+B", "Arrange the editor").clicked() {
                    ui.close();
                }
            });
            menu(ui, "Run", "Run actions", |ui| {
                if menu_item(ui, "Build project", "", "Build this project").clicked() {
                    ui.close();
                }
            });
        });
        ui.add_space(80.0);
        let _ = ui.button("Editor surface");
    })
}

#[test]
fn an_open_menu_follows_the_pointer_across_the_menu_bar() {
    let mut screen = menu_harness();
    screen.run_steps(2);
    screen.get_by_label("File").click();
    screen.run_steps(2);
    assert!(screen.query_by_label_contains("Open document").is_some());

    screen.get_by_label("View").hover();
    screen.run_steps(3);
    assert!(screen.query_by_label_contains("Toggle sidebar").is_some());
    assert!(screen.query_by_label_contains("Open document").is_none());

    screen.get_by_label("Run").hover();
    screen.run_steps(3);
    assert!(screen.query_by_label_contains("Build project").is_some());
    assert!(screen.query_by_label_contains("Toggle sidebar").is_none());

    screen.get_by_label("File").hover();
    screen.run_steps(3);
    assert!(screen.query_by_label_contains("Open document").is_some());
    assert!(screen.query_by_label_contains("Build project").is_none());
}

#[test]
fn hovering_a_closed_menu_bar_does_not_open_a_menu() {
    let mut screen = menu_harness();
    screen.run_steps(2);
    for title in ["File", "View", "Run"] {
        screen.get_by_label(title).hover();
        screen.run_steps(3);
        assert!(!egui::Popup::is_any_open(&screen.ctx));
    }
}

#[test]
fn menu_escape_outside_click_and_action_end_hover_switching() {
    let mut screen = menu_harness();
    screen.run_steps(2);
    screen.get_by_label("File").click();
    screen.run_steps(2);
    screen.get_by_label("Run").hover();
    screen.run_steps(3);
    screen.key_press(egui::Key::Escape);
    screen.run_steps(2);
    assert!(!egui::Popup::is_any_open(&screen.ctx));
    screen.get_by_label("View").hover();
    screen.run_steps(2);
    assert!(screen.query_by_label_contains("Toggle sidebar").is_none());

    screen.get_by_label("View").click();
    screen.run_steps(2);
    screen.get_by_label_contains("Toggle sidebar").click();
    screen.run_steps(2);
    assert!(!egui::Popup::is_any_open(&screen.ctx));

    screen.get_by_label("File").click();
    screen.run_steps(2);
    screen.get_by_label("Editor surface").click();
    screen.run_steps(2);
    assert!(!egui::Popup::is_any_open(&screen.ctx));
    screen.get_by_label("Run").hover();
    screen.run_steps(2);
    assert!(screen.query_by_label_contains("Build project").is_none());
}

//! Task output presentation alongside the project's interactive terminals.
use super::*;

impl VEditApp {
    pub(super) fn output_ui(&mut self, ui: &mut egui::Ui) {
        let drop_only = !self.output_visible && self.terminal_tab_drag_active();
        if !self.output_visible && !drop_only {
            self.task_output_cache.clear();
            self.clear_hidden_output_focus();
            return;
        }
        let panel = egui::Panel::bottom("output")
            .default_size(if drop_only { 80.0 } else { 240.0 })
            .size_range(80.0..=600.0)
            .resizable(true)
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(Margin::symmetric(14, 9)),
            )
            .show(ui, |ui| {
                if drop_only {
                    ui.centered_and_justified(|ui| {
                        ui.label("Drop terminal here");
                    });
                    return;
                }
                self.begin_output_region(ui);
                ui.horizontal(|ui| {
                    self.terminal_panel_tab(ui);
                    let output_tab = design::ghost_button(
                        ui,
                        "Output",
                        &format!(
                            "Command output · {}",
                            self.shortcut_label(keybindings::Action::ToggleOutput)
                        ),
                    );
                    let terminal_visible = self.panel_terminal_id().is_some();
                    let application_visible = self
                        .project()
                        .is_some_and(|project| project.launch.show_output)
                        && !self.problems_visible;
                    if !self.problems_visible && !application_visible && !terminal_visible {
                        ui.painter().line_segment(
                            [
                                output_tab.rect.left_bottom(),
                                output_tab.rect.right_bottom(),
                            ],
                            Stroke::new(1.5, ACCENT),
                        );
                    }
                    if output_tab.clicked() {
                        self.select_task_output();
                    }
                    self.launch_output_tab(ui);
                    let problems = self.problem_list();
                    let count = problems.counted();
                    let problems_tab = design::ghost_button(
                        ui,
                        &if count > 0 {
                            format!("Problems {count}")
                        } else {
                            "Problems".to_owned()
                        },
                        &format!(
                            "{} · {}",
                            problems.summary(),
                            self.shortcut_label(keybindings::Action::Problems)
                        ),
                    );
                    if self.problems_visible {
                        ui.painter().line_segment(
                            [
                                problems_tab.rect.left_bottom(),
                                problems_tab.rect.right_bottom(),
                            ],
                            Stroke::new(1.5, ACCENT),
                        );
                    }
                    if problems_tab.clicked() {
                        self.problems_visible = true;
                    }
                    ui.add_space(14.0);
                    if !application_visible && !terminal_visible && !self.problems_visible {
                        let run_status = self
                            .project()
                            .map_or("Ready", |project| project.run.status.as_str());
                        ui.label(RichText::new(run_status).size(11.0).color(MUTED));
                        if self
                            .project()
                            .is_some_and(|project| project.run.task.is_some())
                        {
                            ui.spinner();
                        }
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if design::ghost_button(
                            ui,
                            "Hide",
                            "Hide the bottom panel; terminals keep running",
                        )
                        .clicked()
                        {
                            self.output_visible = false;
                            self.clear_hidden_output_focus();
                        }
                        if terminal_visible || self.problems_visible || application_visible {
                            return;
                        }
                        if design::ghost_button(
                            ui,
                            "Configure tasks",
                            "Edit this project's commands and Python interpreter",
                        )
                        .clicked()
                        {
                            self.open_project_task_settings();
                        }
                        let Some(run) = self
                            .projects
                            .get_mut(self.active_project)
                            .map(|project| &mut project.run)
                        else {
                            return;
                        };
                        if let Some(task) = &run.task {
                            if design::ghost_button(ui, "Stop", "Stop the running task").clicked() {
                                task.cancel();
                            }
                        } else if design::ghost_button(ui, "Clear", "Clear command output")
                            .clicked()
                        {
                            run.output.clear();
                        }
                        if design::ghost_button(ui, "Copy", "Copy all command output").clicked() {
                            ui.ctx().copy_text(run.output.clone());
                        }
                    });
                });
                if self.problems_visible {
                    self.task_output_cache.clear();
                    self.problems_ui(ui);
                    return;
                }
                if self.terminal_panel_ui(ui) {
                    self.task_output_cache.clear();
                    return;
                }
                let unsaved = self.project_task_unsaved_count();
                if unsaved > 0 {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new(format!(
                                "{unsaved} unsaved document(s). Save before running tasks."
                            ))
                            .size(11.0)
                            .color(ORANGE),
                        );
                        if design::ghost_button(
                            ui,
                            "Save all files",
                            "Save edits explicitly, then run again after saving finishes",
                        )
                        .clicked()
                        {
                            self.save_all();
                        }
                    });
                }
                if self.launch_output_ui(ui) {
                    return;
                }
                ui.separator();
                let clicked = egui::ScrollArea::both()
                    .stick_to_bottom(true)
                    .id_salt("task_output")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let output = self
                            .projects
                            .get(self.active_project)
                            .map_or("", |project| project.run.output.as_str());
                        self.task_output_cache.show_with_links(ui, output, TEXT)
                    })
                    .inner;
                if let Some(link) = clicked {
                    let cwd = self.project().and_then(|project| project.run.cwd.clone());
                    self.open_output_link(link, cwd);
                }
            });
        self.register_terminal_panel_drop_rect(panel.response.rect);
        if !drop_only {
            self.finish_output_region(ui, panel.response.rect);
        }
    }
}

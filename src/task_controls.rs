//! Task execution controls and output presentation.
use super::*;

impl VEditApp {
    pub(super) fn run_task(&mut self, command: String) {
        if let Some(project) = self.projects.get_mut(self.active_project) {
            project.launch.show_output = false;
        }
        if self.project().is_some_and(|p| p.run.task.is_some()) {
            self.status("Stop the current task first", true);
            return;
        }
        let Some(root) = self.active_folder().map(Path::to_path_buf) else {
            return;
        };
        if self.project_task_unsaved_count() > 0 {
            self.output_visible = true;
            self.problems_visible = false;
            self.status("Save your changes before running a command. Use Save all files in Output, wait for saving to finish, then run again.", true);
            return;
        }
        self.output_visible = true;
        self.problems_visible = false;
        self.command = command.clone();
        let environment = self
            .project_task_configuration()
            .unwrap()
            .environment(&root, self.tab().and_then(|tab| tab.path.as_deref()));
        let run = &mut self.projects[self.active_project].run;
        run.output = format!("> {command}\n{}\n\n", crate::paths::display(&root));
        run.status = "Running".into();
        run.command = command.clone();
        run.cwd = Some(root.clone());
        run.task = Some(tasks::spawn_configured(
            root,
            tasks::CommandSpec::Shell(command),
            environment,
        ));
    }
    pub(super) fn open_terminal(&mut self) {
        let Some(root) = self.active_folder().map(Path::to_path_buf) else {
            return;
        };
        let environment = self
            .project_task_configuration()
            .unwrap()
            .environment(&root, self.tab().and_then(|tab| tab.path.as_deref()));
        match tasks::open_terminal(&root, environment) {
            Ok(()) => self.status("Terminal opened in this project", false),
            Err(error) => self.status(error, true),
        }
    }
    pub(super) fn output_ui(&mut self, ui: &mut egui::Ui) {
        if !self.output_visible {
            self.task_output_cache.clear();
            self.clear_hidden_output_focus();
            return;
        }
        let panel = egui::Panel::bottom("output")
            .default_size(240.0)
            .size_range(120.0..=600.0)
            .resizable(true)
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(Margin::symmetric(14, 9)),
            )
            .show(ui, |ui| {
                self.begin_output_region(ui);
                ui.horizontal(|ui| {
                    let output_tab = design::ghost_button(ui, "Output", &format!("Command output · {}", self.shortcut_label(keybindings::Action::ToggleOutput)));
                    if !self.problems_visible && !self.project().is_some_and(|p| p.launch.show_output) {
                        ui.painter().line_segment(
                            [
                                output_tab.rect.left_bottom(),
                                output_tab.rect.right_bottom(),
                            ],
                            Stroke::new(1.5, ACCENT),
                        );
                    }
                    if output_tab.clicked() {
                        self.problems_visible = false;
                        if let Some(project) = self.projects.get_mut(self.active_project) { project.launch.show_output = false; }
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
                        &format!("{} · {}", problems.summary(), self.shortcut_label(keybindings::Action::Problems)),
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
                    let application_visible = self.project().is_some_and(|p| p.launch.show_output) && !self.problems_visible;
                    let run_status = self.project().map_or("Ready", |p| p.run.status.as_str()).to_owned();
                    let running = self.project().is_some_and(|p| p.run.task.is_some());
                    if !application_visible { ui.label(RichText::new(run_status).size(11.0).color(MUTED)); }
                    if running && !application_visible {
                        ui.spinner();
                    }
                    // The rail's Output icon and its shortcut hide the panel.
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if design::ghost_button(
                            ui,
                            "Configure tasks",
                            "Edit this project's commands and Python interpreter",
                        )
                        .clicked()
                        {
                            self.open_project_task_settings();
                        }
                        if design::ghost_button(
                            ui,
                            "Terminal",
                            "Open an interactive terminal in this project for prompts, shells, and long-running commands.",
                        )
                        .clicked()
                        {
                            self.open_terminal();
                        }
                        if application_visible { return; }
                        let Some(run) = self.projects.get_mut(self.active_project).map(|p| &mut p.run) else { return; };
                        if let Some(task) = &run.task {
                            if design::ghost_button(ui, "Stop", "Stop the running command")
                                .clicked()
                            {
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
                let unsaved = self.project_task_unsaved_count();
                if unsaved > 0 {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(format!("{unsaved} unsaved document(s). Save before running commands.")).size(11.0).color(ORANGE));
                        if design::ghost_button(ui, "Save all files", "Save edits explicitly, then run again after saving finishes").clicked() {
                            self.save_all();
                        }
                    });
                }
                if self.launch_output_ui(ui) { return; }
                ui.horizontal(|ui| {
                    ui.label(RichText::new(">").monospace().color(ACCENT));
                    let response = ui.add_sized(
                        [ui.available_width() - 66.0, 28.0],
                        egui::TextEdit::singleline(&mut self.command)
                            .id(split::output_command_id())
                            .font(FontId::monospace(12.0))
                            .hint_text("Run a command in this project…"),
                    );
                    let enter = response.has_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    self.register_region_focus_widget(response.id);
                    if ui
                        .add_enabled(
                            !self.project().is_some_and(|p| p.run.task.is_some()) && !self.command.trim().is_empty(),
                            egui::Button::new("Run").frame_when_inactive(false),
                        )
                        .on_hover_text("Commands run without input and stop after 10 minutes. Use Terminal for prompts and long-running commands.")
                        .clicked()
                        || enter
                    {
                        self.run_task(self.command.clone());
                    }
                });
                ui.separator();
                let clicked = egui::ScrollArea::both()
                    .stick_to_bottom(true)
                    .id_salt("task_output")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let output = self.projects.get(self.active_project).map_or("", |p| p.run.output.as_str());
                        self.task_output_cache.show_with_links(ui, output, TEXT)
                    })
                    .inner;
                if let Some(link) = clicked {
                    let cwd = self.project().and_then(|p| p.run.cwd.clone());
                    self.open_output_link(link, cwd);
                }
            });
        self.finish_output_region(ui, panel.response.rect);
    }
}

use super::*;

impl VEditApp {
    pub(super) fn main_menu_button(&mut self, ui: &mut egui::Ui) {
        let response = icon_button(ui, Icon::Menu, false, "File, View and Run actions");
        egui::Popup::menu(&response).show(|ui| {
            ui.set_min_width(230.0);
            self.main_menu_contents(ui);
        });
    }

    fn main_menu_contents(&mut self, ui: &mut egui::Ui) {
        design::menu(ui, "File", "Files, projects and saving", |ui| {
            if design::menu_item(
                ui,
                "Open project",
                &self.shortcut_label(keybindings::Action::OpenProject),
                "Add a project folder",
            )
            .clicked()
            {
                self.pick_project();
                ui.close();
            }
            if design::menu_item(
                ui,
                "Open file",
                &self.shortcut_label(keybindings::Action::OpenFile),
                "Choose a file from disk",
            )
            .clicked()
            {
                self.pick_file();
                ui.close();
            }
            if design::menu_item(
                ui,
                "Go to file",
                &self.shortcut_label(keybindings::Action::QuickOpen),
                "Find a file in this project",
            )
            .clicked()
            {
                self.open_palette(Palette::Files);
                ui.close();
            }
            if design::menu_item(
                ui,
                "New file",
                &self.shortcut_label(keybindings::Action::NewFile),
                "Create an untitled draft",
            )
            .clicked()
            {
                self.new_file();
                ui.close();
            }
            if ui.add_enabled_ui(self.can_reopen_closed_tab(), |ui| {
                design::menu_item(
                    ui,
                    "Reopen closed tab",
                    &self.shortcut_label(keybindings::Action::ReopenTab),
                    "Reopen the last closed file in this project at its previous cursor position",
                )
            }).inner.clicked() {
                self.reopen_closed_tab();
                ui.close();
            }
            ui.separator();
            if design::menu_item(
                ui,
                "Save",
                &self.shortcut_label(keybindings::Action::Save),
                "Save the current document",
            )
            .clicked()
            {
                self.save_current();
                ui.close();
            }
            if design::menu_item(
                ui,
                "Save all",
                &self.shortcut_label(keybindings::Action::SaveAll),
                "Save changes in every project",
            )
            .clicked()
            {
                self.save_all();
                ui.close();
            }
            if design::menu_item(
                ui,
                "Save as...",
                &self.shortcut_label(keybindings::Action::SaveAs),
                "Save this document to a new path",
            )
            .clicked()
            {
                if let Some(id) = self.tab().map(|tab| tab.id) {
                    self.request_save_as(id);
                }
                ui.close();
            }
            let can_compare = self
                .tab()
                .is_some_and(|tab| tab.path.is_some() && !tab.loading);
            if ui
                .add_enabled_ui(can_compare, |ui| {
                    design::menu_item(
                        ui,
                        "Compare with saved",
                        &self.shortcut_label(keybindings::Action::CompareWithSaved),
                        "Show the unsaved changes of this document as a diff",
                    )
                })
                .inner
                .on_disabled_hover_text("New files have no saved version to compare with")
                .clicked()
            {
                self.compare_current_with_saved();
                ui.close();
            }
            ui.separator();
            if design::menu_item(
                ui,
                "Settings",
                &self.shortcut_label(keybindings::Action::Settings),
                "Vim, font and editor preferences",
            )
            .clicked()
            {
                self.settings_open = true;
                ui.close();
            }
        });
        design::menu(ui, "View", "Navigate and arrange your workspace", |ui| {
            if design::menu_item(ui, "For agent", &self.shortcut_label(keybindings::Action::AgentMarks), "Open selected blocks across all projects, add notes, or restore the last collection").clicked() {
                self.open_agent_marks();
                ui.close();
            }
            if design::menu_item(
                ui,
                "Agent notifications",
                &self.shortcut_label(keybindings::Action::AgentNotifications),
                "Messages agents sent during this session; open their file or review",
            )
            .clicked()
            {
                self.open_notifications();
                ui.close();
            }
            if design::menu_item(
                ui,
                "Bookmarks",
                &self.shortcut_label(keybindings::Action::Bookmarks),
                "Search saved file and cursor positions",
            )
            .clicked()
            {
                self.open_bookmarks();
                ui.close();
            }
            if design::menu_item(
                ui,
                "Toggle bookmark",
                &self.shortcut_label(keybindings::Action::ToggleBookmark),
                "Mark or unmark the current file and line",
            )
            .clicked()
            {
                self.toggle_bookmark();
                ui.close();
            }
            if design::menu_item(
                ui,
                "Jump to visible text",
                &self.shortcut_label(keybindings::Action::ScreenJump),
                "Find a visible match and jump by its label",
            )
            .clicked()
            {
                self.start_screen_jump();
                ui.close();
            }
            ui.separator();
            if design::menu_item(
                ui,
                "Explorer",
                &self.shortcut_label(keybindings::Action::FocusExplorer),
                "Focus project files · Arrows or J/K to navigate · F2 to rename",
            )
            .clicked()
            {
                self.focus_explorer();
                ui.close();
            }
            if design::menu_item(
                ui,
                "Search project",
                &self.shortcut_label(keybindings::Action::SearchProject),
                "Find text across files",
            )
            .clicked()
            {
                self.open_project_search();
                ui.close();
            }
            if design::menu_item(
                ui,
                "Source control",
                &self.shortcut_label(keybindings::Action::SourceControl),
                "Changes, staging and commits",
            )
            .clicked()
            {
                self.show_sidebar(SidebarView::SourceControl);
                ui.close();
            }
            if design::menu_item(
                ui,
                "Problems",
                &self.shortcut_label(keybindings::Action::Problems),
                "Language errors and warnings",
            )
            .clicked()
            {
                self.output_visible = true;
                self.problems_visible = true;
                ui.close();
            }
            if design::menu_item(
                ui,
                "Output",
                &self.shortcut_label(keybindings::Action::ToggleOutput),
                "Show or hide command output",
            )
            .clicked()
            {
                self.output_visible = !self.output_visible;
                self.problems_visible = false;
                ui.close();
            }
            if self
                .tab()
                .is_some_and(|tab| is_markdown(tab.path.as_deref()))
                && design::menu_item(
                    ui,
                    "Markdown source / preview",
                    &self.shortcut_label(keybindings::Action::MarkdownPreview),
                    "Read Markdown or edit its source",
                )
                .clicked()
            {
                let view = if self
                    .tab()
                    .is_some_and(|tab| tab.agent_view == crate::cli::DocumentView::Preview)
                {
                    crate::cli::DocumentView::Source
                } else {
                    crate::cli::DocumentView::Preview
                };
                self.set_document_view(view);
                ui.close();
            }
            ui.separator();
            self.sidebar_menu_ui(ui);
            self.split_menu_ui(ui);
            ui.separator();
            if ui
                .checkbox(&mut self.settings.word_wrap, "Word wrap")
                .on_hover_text(format!(
                    "Wrap long lines · {}",
                    self.shortcut_label(keybindings::Action::ToggleWrap)
                ))
                .changed()
            {
                self.session_dirty = true;
            }
            if ui
                .checkbox(&mut self.settings.vim, "Vim motions")
                .on_hover_text("Normal, Insert, Visual and line Visual modes")
                .changed()
            {
                self.session_dirty = true;
            }
        });
        design::menu(ui, "Run", "Build, test and run commands", |ui| {
            for (name, action) in [
                ("Run application", keybindings::Action::RunApplication),
                ("Stop application", keybindings::Action::StopApplication),
            ] {
                if design::menu_item(
                    ui,
                    name,
                    &self.shortcut_label(action),
                    "Use the selected project launch option",
                )
                .clicked()
                {
                    self.command_action(name);
                    ui.close();
                }
            }
            ui.separator();
            if design::menu_item(
                ui,
                "Restart language analysis",
                &self.shortcut_label(keybindings::Action::RestartLanguage),
                "Reconnect the active language server and refresh diagnostics and suggestions",
            )
            .clicked()
            {
                self.restart_active_language();
                ui.close();
            }
            ui.separator();
            if design::menu_item(
                ui,
                "Open terminal",
                &self.shortcut_label(keybindings::Action::Terminal),
                "Open PowerShell in this project",
            )
            .clicked()
            {
                self.open_terminal();
                ui.close();
            }
            ui.separator();
            for (name, action, command) in [
                (
                    "Check",
                    keybindings::Action::CheckProject,
                    project_tasks::ProjectTaskAction::Check,
                ),
                (
                    "Test",
                    keybindings::Action::TestProject,
                    project_tasks::ProjectTaskAction::Test,
                ),
                (
                    "Build",
                    keybindings::Action::BuildProject,
                    project_tasks::ProjectTaskAction::Build,
                ),
                (
                    "Run active file",
                    keybindings::Action::RunFile,
                    project_tasks::ProjectTaskAction::RunFile,
                ),
            ] {
                if design::menu_item(
                    ui,
                    name,
                    &self.shortcut_label(action),
                    "Use this project's configured command",
                )
                .clicked()
                {
                    self.run_project_task(command);
                    ui.close();
                }
            }
            if design::menu_item(
                ui,
                "Configure project tasks",
                &self.shortcut_label(keybindings::Action::ConfigureTasks),
                "Choose commands and runtime for this project",
            )
            .clicked()
            {
                self.open_project_task_settings();
                ui.close();
            }
            ui.separator();
            if design::menu_item(
                ui,
                "Custom command",
                "",
                "Run a command and view its output",
            )
            .clicked()
            {
                self.output_visible = true;
                self.problems_visible = false;
                ui.close();
            }
        });
    }
}

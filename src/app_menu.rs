use super::*;

impl VEditApp {
    pub(super) fn main_menu_button(&mut self, ui: &mut egui::Ui) {
        let response = icon_button(
            ui,
            Icon::Menu,
            false,
            "File, Selection, View and Run actions",
        );
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
                self.run_shortcut_action(keybindings::Action::OpenProject);
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
                self.run_shortcut_action(keybindings::Action::OpenFile);
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
                self.run_shortcut_action(keybindings::Action::QuickOpen);
                ui.close();
            }
            if design::menu_item(
                ui,
                "Open recent",
                &self.shortcut_label(keybindings::Action::OpenRecent),
                "Find recently used files and project folders",
            )
            .clicked()
            {
                self.run_shortcut_action(keybindings::Action::OpenRecent);
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
                self.run_shortcut_action(keybindings::Action::NewFile);
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
                self.run_shortcut_action(keybindings::Action::ReopenTab);
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
                self.run_shortcut_action(keybindings::Action::Save);
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
                self.run_shortcut_action(keybindings::Action::SaveAll);
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
                self.run_shortcut_action(keybindings::Action::SaveAs);
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
                self.run_shortcut_action(keybindings::Action::CompareWithSaved);
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
                self.run_shortcut_action(keybindings::Action::Settings);
                ui.close();
            }
        });
        design::menu(
            ui,
            "Selection",
            "Select and edit several places together",
            |ui| {
                for action in keybindings::Action::ALL
                    .iter()
                    .copied()
                    .filter(|a| a.multicursor())
                {
                    let enabled = self.tab().is_some_and(|t| {
                        !t.loading
                            && t.agent_view == crate::cli::DocumentView::Source
                            && (!matches!(
                                action,
                                keybindings::Action::CursorUndo
                                    | keybindings::Action::ExitMulticursor
                            ) || t.vim.multi.active())
                    });
                    if ui.add_enabled_ui(enabled,|ui| design::menu_item(ui,action.label(),&self.shortcut_label(action),"Standard editing keys apply to every cursor. Esc returns to one cursor.")).inner.clicked() {
                    self.run_shortcut_action(action); ui.close();
                }
                }
            },
        );
        design::menu(ui, "View", "Navigate and arrange your workspace", |ui| {
            if design::menu_item(ui, "For agent", &self.shortcut_label(keybindings::Action::AgentMarks), "Open selected blocks across all projects, add notes, or restore the last collection").clicked() {
                self.run_shortcut_action(keybindings::Action::AgentMarks);
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
                self.run_shortcut_action(keybindings::Action::AgentNotifications);
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
                self.run_shortcut_action(keybindings::Action::Bookmarks);
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
                self.run_shortcut_action(keybindings::Action::ToggleBookmark);
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
                self.run_shortcut_action(keybindings::Action::ScreenJump);
                ui.close();
            }
            if design::menu_item(
                ui,
                "Go back",
                &self.shortcut_label(keybindings::Action::NavigateBack),
                "Return to the previous location in this editor pane",
            )
            .clicked()
            {
                self.run_shortcut_action(keybindings::Action::NavigateBack);
                ui.close();
            }
            if design::menu_item(
                ui,
                "Go forward",
                &self.shortcut_label(keybindings::Action::NavigateForward),
                "Return to the location you came back from",
            )
            .clicked()
            {
                self.run_shortcut_action(keybindings::Action::NavigateForward);
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
                self.run_shortcut_action(keybindings::Action::SearchProject);
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
                self.run_shortcut_action(keybindings::Action::SourceControl);
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
                self.run_shortcut_action(keybindings::Action::Problems);
                ui.close();
            }
            if design::menu_item(
                ui,
                "Next problem",
                &self.shortcut_label(keybindings::Action::NextProblem),
                "Next error or warning, then the next file with problems",
            )
            .clicked()
            {
                self.run_shortcut_action(keybindings::Action::NextProblem);
                ui.close();
            }
            if design::menu_item(
                ui,
                "Previous problem",
                &self.shortcut_label(keybindings::Action::PreviousProblem),
                "Previous error or warning, then the previous file with problems",
            )
            .clicked()
            {
                self.run_shortcut_action(keybindings::Action::PreviousProblem);
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
                self.run_shortcut_action(keybindings::Action::MarkdownPreview);
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
            if self.active_folder().is_none() {
                ui.label("Open a project folder to run commands.");
                return;
            }
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
                    self.run_shortcut_action(action);
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
                self.run_shortcut_action(keybindings::Action::RestartLanguage);
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
                self.run_shortcut_action(keybindings::Action::Terminal);
                ui.close();
            }
            ui.separator();
            for (name, action) in [
                ("Check", keybindings::Action::CheckProject),
                ("Test", keybindings::Action::TestProject),
                ("Build", keybindings::Action::BuildProject),
                ("Run active file", keybindings::Action::RunFile),
            ] {
                if design::menu_item(
                    ui,
                    name,
                    &self.shortcut_label(action),
                    "Use this project's configured command",
                )
                .clicked()
                {
                    self.run_shortcut_action(action);
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
                self.run_shortcut_action(keybindings::Action::ConfigureTasks);
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

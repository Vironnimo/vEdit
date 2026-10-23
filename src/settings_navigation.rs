//! Short, keyboard-reachable settings pages instead of one unbounded form.
use super::*;

const SECTIONS: &[&str] = &[
    "Appearance",
    "Editing",
    "Keyboard",
    "Languages",
    "Tasks",
    "Layout",
    "Vim",
];

impl VEditApp {
    pub(super) fn settings_section(&self) -> String {
        self.ctx
            .data(|data| data.get_temp::<String>(egui::Id::new("settings_section")))
            .filter(|value| SECTIONS.contains(&value.as_str()))
            .unwrap_or_else(|| "Appearance".into())
    }

    pub(super) fn open_settings_section(&mut self, section: &str) {
        self.interface_zoom_draft = None;
        if SECTIONS.contains(&section) {
            self.ctx.data_mut(|data| {
                data.insert_temp(egui::Id::new("settings_section"), section.to_owned())
            });
        }
        self.settings_open = true;
    }

    pub(super) fn settings_navigation_ui(&mut self, ui: &mut egui::Ui) {
        let previous = self.settings_section();
        let mut selected = previous.clone();
        ui.horizontal_wrapped(|ui| {
            for section in SECTIONS {
                ui.selectable_value(&mut selected, (*section).to_owned(), *section);
            }
        });
        if selected != previous {
            self.interface_zoom_draft = None;
            if previous == "Tasks" {
                self.apply_pending_project_interpreter();
            }
            self.keybinding_ui = keybindings::State::default();
            self.ctx
                .data_mut(|data| data.insert_temp(egui::Id::new("settings_section"), selected));
        }
        ui.separator();
    }

    pub(super) fn settings_contents_ui(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;
        match self.settings_section().as_str() {
            "Keyboard" => changed |= self.keyboard_settings_ui(ui),
            "Languages" => self.language_settings_ui(ui),
            "Tasks" => self.project_tasks_settings_ui(ui),
            "Editing" => {
                editor_preferences::preference_heading(ui, "EDITING & SAVING");
                changed |= ui.checkbox(&mut self.settings.autosave, "Save files automatically after a pause")
                    .on_hover_text("Save edits after you stop typing. Agent drafts remain manual until explicitly saved.").changed();
                changed |= ui
                    .add_enabled(
                        self.settings.autosave,
                        egui::Slider::new(&mut self.settings.autosave_delay_ms, 300..=5000)
                            .text("Auto save delay")
                            .suffix(" ms"),
                    )
                    .changed();
                ui.label(
                    RichText::new("Recovery copies are kept even with auto save off.")
                        .size(11.0)
                        .color(MUTED),
                );
                changed |= self.indentation_settings_ui(ui);
                changed |= self.multicursor_settings_ui(ui);
            }
            "Layout" => {
                editor_preferences::preference_heading(ui, "LAYOUT & EXPLORER");
                let mut new_window = self.settings.external_open_new_window;
                if ui
                    .checkbox(&mut new_window, "Open external files in a new window")
                    .changed()
                {
                    self.set_external_open_new_window(new_window);
                    changed = true;
                }
                ui.label(RichText::new("For files opened from Windows or vedit. Off: use the last active window. Open dialogs and agent commands stay in their selected window.").small().color(MUTED));
                ui.add_space(12.0);
                self.sidebar_settings_ui(ui);
                ui.add_space(12.0);
                let mut preview = self.settings.explorer_preview_tabs;
                if ui
                    .checkbox(&mut preview, "Preview files from Explorer")
                    .changed()
                {
                    self.set_explorer_preview_tabs(preview);
                    changed = true;
                }
                ui.label(RichText::new("Single-click reuses a preview tab in this pane. Double-click a file or its tab, or edit it, to keep it open.").small().color(MUTED));
            }
            "Vim" => {
                editor_preferences::preference_heading(ui, "VIM");
                changed |= ui.checkbox(&mut self.settings.vim, "Vim motions (default)")
                    .on_hover_text("Use Normal, Insert, Visual and Visual Line modes. Changes apply immediately.").changed();
                changed |= ui.checkbox(&mut self.settings.swap_jk, "Swap J and K")
                    .on_hover_text("Swap vertical Vim navigation. Inserted text and literal search targets stay unchanged.").changed();
                changed |= egui::ComboBox::from_label("Leader key")
                    .selected_text(if self.settings.leader_key == " " {
                        "Space"
                    } else {
                        &self.settings.leader_key
                    })
                    .show_ui(ui, |ui| {
                        let mut changed = ui
                            .selectable_value(
                                &mut self.settings.leader_key,
                                " ".into(),
                                "Space (default)",
                            )
                            .changed();
                        changed |= ui
                            .selectable_value(&mut self.settings.leader_key, ",".into(), "Comma")
                            .changed();
                        changed |= ui
                            .selectable_value(
                                &mut self.settings.leader_key,
                                "\\".into(),
                                "Backslash",
                            )
                            .changed();
                        changed
                    })
                    .inner
                    .unwrap_or(false);
                ui.label(
                    RichText::new(
                        "Leader m: bookmark · Leader b: bookmarks · Leader Leader f: jump",
                    )
                    .size(10.5)
                    .color(MUTED),
                );
                ui.add_space(12.0);
                editor_preferences::preference_heading(ui, "VIM REFERENCE");
                for text in [
                    "h j k l · w b e · 0 ^ $ · gg G · %",
                    "i a I A · o O · r · x · dd · cw · cc · D C",
                    "v V · y d c p · > < · u · Ctrl R",
                    "Counts: 3w · 2dd · d2w     Find: f F t T ; ,",
                    "Search: / ? · n N · * #     Pages: Ctrl F B D U",
                    "Commands: :w :wa :q :wq :e path :42",
                ] {
                    ui.label(RichText::new(text).monospace().size(11.5).color(TEXT));
                }
            }
            _ => changed |= self.reading_settings_ui(ui),
        }
        changed
    }

    pub(super) fn set_external_open_new_window(&mut self, enabled: bool) {
        self.settings.external_open_new_window = enabled;
        // Use the existing ordered recovery writer and error reporting. Only an
        // explicit preference change writes this shared setting; closing an older
        // window cannot silently revert it.
        let base = self.external_open_directory();
        let writer = self.recovery_writer.clone();
        let stamp = writer.stamp();
        self.external_open_revision += 1;
        self.external_open_save_pending = true;
        let revision = self.external_open_revision;
        let sender = self.sender.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let result = writer.write(
                &base.join("external-open.json"),
                stamp,
                &serde_json::to_vec(&enabled).unwrap(),
            );
            let _ = sender.send(WorkerResult::ExternalOpenSaved { revision, result });
            ctx.request_repaint();
        });
        self.session_dirty = true;
    }

    pub(super) fn external_open_directory(&self) -> PathBuf {
        #[cfg(not(test))]
        {
            crate::profile::base_recovery_directory()
        }
        #[cfg(test)]
        {
            self.recovery_session_path.parent().unwrap().to_owned()
        }
    }

    pub(super) fn refresh_external_open_preference(&mut self) {
        if self.external_open_save_pending {
            return;
        }
        self.external_open_revision += 1;
        let revision = self.external_open_revision;
        let base = self.external_open_directory();
        let sender = self.sender.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let enabled = crate::profile::read_open_mode(&base);
            let _ = sender.send(WorkerResult::ExternalOpenRead { revision, enabled });
            ctx.request_repaint();
        });
    }

    pub(super) fn settings_save_hint(&self) -> &'static str {
        if self.settings_section() == "Languages" {
            "Server edits need Apply and restart language · Esc closes"
        } else {
            "Applied & saved automatically · Esc closes"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable;

    #[test]
    fn external_open_preference_is_available_and_leaves_documents_in_place() {
        let mut app = VEditApp::empty(egui::Context::default());
        app.recovery_session_path = recovery_directory().join("external-open-test/session.json");
        visual_tests::workspace(&mut app);
        let before: Vec<_> = app
            .projects
            .iter()
            .flat_map(|project| &project.tabs)
            .map(|tab| (tab.id, tab.text.clone(), tab.dirty))
            .collect();
        let mut screen = visual_tests::harness(app);
        screen.state_mut().open_settings_section("Layout");
        screen.run_steps(3);
        screen
            .get_by_label("Open external files in a new window")
            .click();
        screen.run_steps(3);
        assert!(screen.state().settings.external_open_new_window);
        let stale_revision = screen.state().external_open_revision - 1;
        screen
            .state()
            .sender
            .send(WorkerResult::ExternalOpenRead {
                revision: stale_revision,
                enabled: false,
            })
            .unwrap();
        screen.state_mut().poll();
        assert!(screen.state().settings.external_open_new_window);
        let after: Vec<_> = screen
            .state()
            .projects
            .iter()
            .flat_map(|project| &project.tabs)
            .map(|tab| (tab.id, tab.text.clone(), tab.dirty))
            .collect();
        assert_eq!(before, after);
        screen.get_by_label("Done").click();
        screen.run_steps(3);
        let response = screen
            .state_mut()
            .control_request(crate::cli::Request::Settings {
                patch: Some(crate::cli::SettingsPatch {
                    external_open_new_window: Some(false),
                    ..Default::default()
                }),
            });
        assert!(response.ok);
        assert_eq!(
            response.result["settings"]["external_open_new_window"],
            false
        );
    }

    #[test]
    fn settings_categories_are_reachable_and_zoomed_pages_fit() {
        let mut app = VEditApp::empty(egui::Context::default());
        visual_tests::workspace(&mut app);
        // This test only renders settings; no task runs. Show stable project
        // metadata rather than the visual fixture's unique temporary path.
        let project = &mut app.projects[0];
        let stable_root = PathBuf::from("C:/Development/projects/tiny-project");
        let index = project.index.take().unwrap();
        project.index = Some(Arc::new(
            (*index).clone().at_fixture_root(stable_root.clone()),
        ));
        for tab in &mut project.tabs {
            if let Some(path) = &mut tab.path {
                *path = stable_root.join(path.strip_prefix(&project.root).unwrap());
            }
        }
        project.root = stable_root;
        let mut screen = visual_tests::harness(app);
        screen.run_steps(3);
        screen.state_mut().open_settings_section("Languages");
        screen.run_steps(3);
        assert_eq!(screen.state().settings_section(), "Languages");
        assert!(screen.query_by_label("Add language server…").is_some());
        screen.get_by_label("Keyboard").click();
        screen.run_steps(3);
        assert_eq!(screen.state().settings_section(), "Keyboard");
        assert!(screen.query_by_label("Reset all").is_some());
        visual_tests::snapshot(&mut screen, "vedit_keyboard_settings");
        screen.get_by_label("Tasks").click();
        screen.run_steps(3);
        assert_eq!(screen.state().settings_section(), "Tasks");
        visual_tests::snapshot(&mut screen, "vedit_task_settings");
        screen.get_by_label("Languages").click();
        screen.run_steps(3);
        visual_tests::snapshot(&mut screen, "vedit_language_settings");
        screen.set_size(Vec2::new(960.0, 640.0));
        screen.state_mut().settings.ui_scale = 2.0;
        screen.ctx.set_zoom_factor(2.0);
        screen.run_steps(4);
        for label in [
            "Appearance",
            "Editing",
            "Keyboard",
            "Languages",
            "Tasks",
            "Layout",
            "Vim",
            "Done",
        ] {
            let bounds = screen.get_by_label(label).rect();
            assert!(
                screen.ctx.content_rect().contains_rect(bounds),
                "{label} remains reachable at 200%: {bounds:?}"
            );
        }
        visual_tests::snapshot(&mut screen, "vedit_zoomed_language_settings");
        screen.key_press(Key::Escape);
        screen.run_steps(3);
        assert!(!screen.state().settings_open);
    }
}

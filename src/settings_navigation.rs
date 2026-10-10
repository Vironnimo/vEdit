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
            }
            "Layout" => {
                editor_preferences::preference_heading(ui, "LAYOUT & EXPLORER");
                self.sidebar_settings_ui(ui);
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
    fn settings_categories_are_reachable_and_zoomed_pages_fit() {
        let mut app = VEditApp::empty(egui::Context::default());
        visual_tests::workspace(&mut app);
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

//! Personal reading preferences, independent of documents and pane layout.
use super::*;

impl Settings {
    pub(super) fn normalize_appearance(&mut self) {
        fn bounded(value: f32, min: f32, max: f32, fallback: f32) -> f32 {
            if value.is_finite() {
                value.clamp(min, max)
            } else {
                fallback
            }
        }
        self.ui_scale = bounded(self.ui_scale, 0.75, 2.0, 1.0);
        self.font_size = bounded(self.font_size, 10.0, 32.0, 14.0);
        self.line_height = bounded(self.line_height, 1.0, 2.0, 1.35);
        self.autosave_delay_ms = self.autosave_delay_ms.clamp(300, 5000);
        self.tab_width = self.tab_width.clamp(1, 8);
        for width in self.language_indent_widths.values_mut() {
            *width = (*width).clamp(1, 8);
        }
    }
}

impl VEditApp {
    /// Configured zoom commands also work in Settings, except during recording.
    pub(super) fn preference_shortcuts(&mut self) {
        use keybindings::Action;
        for action in [Action::ZoomReset, Action::ZoomIn, Action::ZoomOut] {
            if self.take_shortcut(action) {
                self.run_shortcut_action(action);
                break;
            }
        }
    }

    pub(super) fn reading_settings_ui(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;
        preference_heading(ui, "READING & APPEARANCE");
        let cancelling = ui.input(|input| input.key_pressed(Key::Escape));
        let mut percent = self
            .interface_zoom_draft
            .filter(|_| !cancelling)
            .unwrap_or(self.settings.ui_scale * 100.0);
        let zoom = ui
            .add(
                egui::Slider::new(&mut percent, 75.0..=200.0)
                    .text("Interface zoom")
                    .suffix("%"),
            )
            .on_hover_text("Release the mouse to apply the zoom. You can also enter a percentage.");
        // Resizing a held slider moves its track beneath the pointer and feeds
        // that new position back into the value. Keep its geometry fixed until
        // release, including when dragging the numeric value beside the track.
        if cancelling {
            self.interface_zoom_draft = None;
        } else if zoom.is_pointer_button_down_on() {
            self.interface_zoom_draft = Some(percent);
        } else {
            let finishing_drag = self.interface_zoom_draft.take().is_some();
            if (zoom.changed() || finishing_drag) && self.settings.ui_scale != percent / 100.0 {
                self.settings.ui_scale = percent / 100.0;
                changed = true;
            }
        }
        ui.label(
            RichText::new(format!(
                "Zoom: {} / {} · Reset: {}",
                self.shortcut_label(keybindings::Action::ZoomIn),
                self.shortcut_label(keybindings::Action::ZoomOut),
                self.shortcut_label(keybindings::Action::ZoomReset)
            ))
            .size(10.5)
            .color(MUTED),
        );
        ui.label(
            RichText::new("Zoom changes the entire interface, including Git and previews.")
                .size(10.5)
                .color(MUTED),
        );
        let font_choice = egui::ComboBox::from_label("Editor font")
            .selected_text(self.settings.editor_font.label())
            .show_ui(ui, |ui| {
                let mut changed = false;
                for font in [
                    crate::cli::EditorFont::JetBrainsMono,
                    crate::cli::EditorFont::Consolas,
                    crate::cli::EditorFont::CascadiaMono,
                ] {
                    changed |= ui
                        .selectable_value(&mut self.settings.editor_font, font, font.label())
                        .changed();
                }
                changed
            });
        changed |= font_choice.inner.unwrap_or(false);
        font_choice.response.on_hover_text("Regular code fonts. JetBrains Mono is included; unavailable Windows fonts use it as a fallback.");
        changed |= ui
            .add(
                egui::Slider::new(&mut self.settings.font_size, 10.0..=32.0)
                    .text("Editor font size"),
            )
            .changed();
        changed |= ui
            .add(
                egui::Slider::new(&mut self.settings.line_height, 1.0..=2.0)
                    .text("Code line spacing")
                    .suffix("×"),
            )
            .changed();
        changed |= egui::ComboBox::from_label("Line numbers")
            .selected_text(match self.settings.line_numbers {
                crate::cli::LineNumbers::Absolute => "Absolute",
                crate::cli::LineNumbers::Relative => "Relative",
                crate::cli::LineNumbers::Off => "Off",
            })
            .show_ui(ui, |ui| {
                let mut changed = false;
                for (value, label) in [
                    (crate::cli::LineNumbers::Absolute, "Absolute"),
                    (crate::cli::LineNumbers::Relative, "Relative"),
                    (crate::cli::LineNumbers::Off, "Off"),
                ] {
                    changed |= ui
                        .selectable_value(&mut self.settings.line_numbers, value, label)
                        .changed();
                }
                changed
            })
            .inner
            .unwrap_or(false);
        changed |= ui
            .checkbox(
                &mut self.settings.highlight_current_line,
                "Highlight cursor line",
            )
            .changed();
        changed |= ui
            .checkbox(&mut self.settings.word_wrap, "Wrap long lines")
            .on_hover_text(format!(
                "Fit long source lines to the editor pane · {}",
                self.shortcut_label(keybindings::Action::ToggleWrap)
            ))
            .changed();
        changed |= ui
            .checkbox(&mut self.settings.smooth_scroll, "Smooth scrolling")
            .on_hover_text(
                "Animate source and Markdown scrolling; turn off for immediate movement.",
            )
            .changed();
        changed
    }
}

pub(super) fn preference_heading(ui: &mut egui::Ui, title: &str) {
    ui.add_space(8.0);
    ui.label(RichText::new(title).size(10.5).strong().color(ACCENT));
    ui.add_space(6.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn older_settings_use_readable_defaults_and_restored_values_are_bounded() {
        let mut settings: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings.ui_scale, 1.0);
        assert_eq!(settings.line_height, 1.35);
        assert_eq!(settings.line_numbers, crate::cli::LineNumbers::Absolute);
        assert!(settings.highlight_current_line);
        assert!(settings.show_ignored_files);
        assert_eq!(settings.autosave_delay_ms, 850);
        settings.ui_scale = f32::NAN;
        settings.font_size = 100.0;
        settings.line_height = -1.0;
        settings.autosave_delay_ms = 0;
        settings.normalize_appearance();
        assert_eq!(settings.ui_scale, 1.0);
        assert_eq!(settings.font_size, 32.0);
        assert_eq!(settings.line_height, 1.0);
        assert_eq!(settings.autosave_delay_ms, 300);
    }
}

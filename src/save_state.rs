//! Visible save state is independent of transient status messages.
use super::*;

impl VEditApp {
    pub(super) fn save_status_ui(&mut self, ui: &mut egui::Ui, show_autosave: bool) {
        let autosave = self.settings.autosave;
        let save_binding = self.shortcut_label(keybindings::Action::Save);
        let save = if save_binding.is_empty() {
            "File > Save"
        } else {
            &save_binding
        };
        if let Some(tab) = self.tab() {
            let (label, color, hint) = if tab.loading {
                (
                    "Loading…",
                    MUTED,
                    "The document is still loading.".to_owned(),
                )
            } else if let Some(error) = &tab.error {
                (
                    if tab.dirty {
                        "Save blocked"
                    } else {
                        "File error"
                    },
                    RED,
                    if tab.dirty {
                        format!(
                            "{error}\nYour buffer is retained. Use Save as… in the document banner to preserve your edits in another file. {save} retries saving."
                        )
                    } else {
                        format!(
                            "{error}\nUse Reload from disk in the document banner to retry loading."
                        )
                    },
                )
            } else if tab.saving {
                ("Saving…", MUTED, "Writing the document to disk.".to_owned())
            } else if tab.dirty && tab.agent_unsaved {
                (
                    "Agent draft",
                    ORANGE,
                    format!(
                        "These agent edits are unsaved. Auto save is paused for this document until you save with {save}."
                    ),
                )
            } else if tab.path.is_none() {
                (
                    "New file",
                    ORANGE,
                    format!(
                        "Choose a file name with {save}. New files are not automatically saved."
                    ),
                )
            } else if tab.dirty {
                (
                    "Unsaved",
                    ORANGE,
                    if autosave {
                        format!(
                            "Changes will be saved after {} ms without typing. {save} saves now.",
                            self.settings.autosave_delay_ms
                        )
                    } else {
                        format!("Changes are in memory. {save} saves them to disk.")
                    },
                )
            } else {
                (
                    "Saved",
                    MUTED,
                    "The buffer matches the last saved document.".to_owned(),
                )
            };
            ui.label(RichText::new(label).size(10.5).color(color))
                .on_hover_text(hint);
        }
        if !show_autosave {
            return;
        }
        let label = if autosave {
            "Auto save on"
        } else {
            "Auto save off"
        };
        let hint = format!(
            "Save named files {} ms after you stop typing. Applies to all projects.\nNew files and unsaved agent drafts wait for {save}.\nClick to turn auto save {}.",
            self.settings.autosave_delay_ms,
            if autosave { "off" } else { "on" }
        );
        let response = ui
            .add(
                egui::Button::new(RichText::new(label).size(10.5).color(if autosave {
                    ACCENT
                } else {
                    MUTED
                }))
                .frame_when_inactive(false)
                .corner_radius(3.0),
            )
            .on_hover_text(hint)
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if response.clicked() {
            self.settings.autosave = !autosave;
            self.session_dirty = true;
            if let Some(tab) = self.tab_mut() {
                tab.focus = tab.agent_view == crate::cli::DocumentView::Source;
            }
            self.ctx.request_repaint();
        }
    }
}

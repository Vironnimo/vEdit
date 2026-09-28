//! Visible save state is independent of transient status messages.
use super::*;

impl MorrowApp {
    pub(super) fn save_status_ui(&mut self, ui: &mut egui::Ui) {
        let autosave = self.settings.autosave;
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
                            "{error}\nYour buffer is retained. Ctrl+S retries saving; reload from the File menu."
                        )
                    } else {
                        format!("{error}\nReload from disk in the File menu.")
                    },
                )
            } else if tab.saving {
                ("Saving…", MUTED, "Writing the document to disk.".to_owned())
            } else if tab.dirty && tab.agent_unsaved {
                ("Agent draft", ORANGE, "These agent edits are unsaved. Auto save is paused for this document until you save with Ctrl+S.".to_owned())
            } else if tab.path.is_none() {
                (
                    "New file",
                    ORANGE,
                    "Choose a file name with Ctrl+S. New files are not automatically saved."
                        .to_owned(),
                )
            } else if tab.dirty {
                (
                    "Unsaved",
                    ORANGE,
                    if autosave {
                        "Changes will be saved after 850 ms without typing. Ctrl+S saves now."
                    } else {
                        "Changes are in memory. Ctrl+S saves them to disk."
                    }
                    .to_owned(),
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
        let label = if autosave {
            "Auto save on"
        } else {
            "Auto save off"
        };
        let hint = format!(
            "Save named files 850 ms after you stop typing. Applies to all projects.\nNew files and unsaved agent drafts wait for Ctrl+S.\nClick to turn auto save {}.",
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

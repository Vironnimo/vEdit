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
        let undo = self.shortcut_label(keybindings::Action::Undo);
        let undo = if undo.is_empty() { "Undo" } else { &undo };
        let mut compare = None;
        if let Some(tab) = self.tab() {
            // `Some(kind)` makes the state a button that opens that comparison.
            let (label, color, hint, action) = if tab.loading {
                (
                    "Loading…",
                    MUTED,
                    "The document is still loading.".to_owned(),
                    None,
                )
            } else if tab.disk_conflict.is_some() {
                (
                    "Disk conflict",
                    ORANGE,
                    "The file changed on disk while this buffer has unsaved edits. Your edits are kept; only saving to this file waits until you choose a version.
Click to compare the disk version with your edits."
                        .to_owned(),
                    Some(disk_sync::ComparisonKind::DiskConflict),
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
                            "{error}
Your edits are kept. Save as… in the document banner preserves them in another file."
                        )
                    } else {
                        format!(
                            "{error}
Use Reload from disk in the document banner to retry loading."
                        )
                    },
                    None,
                )
            } else if tab.saving {
                (
                    "Saving…",
                    MUTED,
                    "Writing the document to disk.".to_owned(),
                    None,
                )
            } else if tab.dirty && tab.agent_unsaved {
                (
                    "Agent draft",
                    ORANGE,
                    format!(
                        "These agent edits are unsaved. Auto save is paused for this document until you save with {save}.
Click to compare them with the saved version."
                    ),
                    Some(disk_sync::ComparisonKind::Saved),
                )
            } else if tab.path.is_none() {
                (
                    "New file",
                    ORANGE,
                    format!(
                        "Choose a file name with {save}. New files are not automatically saved."
                    ),
                    None,
                )
            } else if tab.dirty {
                (
                    "Unsaved",
                    ORANGE,
                    if autosave {
                        format!(
                            "Changes will be saved after {} ms without typing. {save} saves now.
Click to compare them with the saved version.",
                            self.settings.autosave_delay_ms
                        )
                    } else {
                        format!(
                            "Changes are in memory. {save} saves them to disk.
Click to compare them with the saved version."
                        )
                    },
                    Some(disk_sync::ComparisonKind::Saved),
                )
            } else if let Some(reload) = tab
                .disk_reload
                .as_ref()
                .filter(|reload| reload.revision == tab.revision)
            {
                let seconds = reload.at.elapsed().as_secs();
                let when = if seconds < 60 {
                    "just now".to_owned()
                } else if seconds < 3600 {
                    format!("{} min ago", seconds / 60)
                } else {
                    format!("{} h ago", seconds / 3600)
                };
                let changed = match reload.changed_lines {
                    Some(1) => ": 1 line changed".to_owned(),
                    Some(lines) => format!(": {lines} lines changed"),
                    None => String::new(),
                };
                (
                    "Reloaded",
                    MUTED,
                    format!(
                        "The file changed on disk and the buffer had no unsaved edits, so it was reloaded {when}{changed}.
{undo} restores the previous text."
                    ),
                    None,
                )
            } else {
                (
                    "Saved",
                    MUTED,
                    "The buffer matches the last saved document.".to_owned(),
                    None,
                )
            };
            match action {
                Some(kind) => {
                    if design::status_details(ui, label, color, &hint).clicked() {
                        compare = Some((tab.id, kind));
                    }
                }
                None => {
                    ui.label(RichText::new(label).size(11.0).color(color))
                        .on_hover_details(hint);
                }
            }
        }
        match compare {
            Some((id, disk_sync::ComparisonKind::DiskConflict)) => self.compare_disk_conflict(id),
            Some((id, disk_sync::ComparisonKind::Saved)) => self.compare_with_saved(id),
            None => {}
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
        let response = design::status_item(ui, label, MUTED, &hint);
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

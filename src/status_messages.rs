//! Inspect and copy a stable snapshot of a status message.
use super::*;

pub(super) struct Details {
    message: String,
    error: bool,
    restore_focus: Option<egui::Id>,
}

impl VEditApp {
    pub(super) fn message_details_json(&self) -> Option<serde_json::Value> {
        self.status_details
            .as_ref()
            .map(|details| serde_json::json!({"text": details.message, "error": details.error}))
    }

    pub(super) fn status_message_ui(&mut self, ui: &mut egui::Ui, width: f32) {
        let Some((message, error, _)) = &self.status else {
            return;
        };
        let message = message.clone();
        let error = *error;
        let summary = message.lines().next().unwrap_or("Message details");
        let response = ui
            .add_sized(
                [width, 20.0],
                egui::Button::new(RichText::new(summary).size(11.0).color(if error {
                    RED
                } else {
                    MUTED
                }))
                .truncate()
                .frame_when_inactive(false)
                .corner_radius(3.0),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(format!(
                "{}\n\nClick for details · Right-click to copy",
                short_preview(summary)
            ));
        if response.clicked() {
            self.open_message_details(message.clone(), error);
        }
        let snapshot_id = response.id.with("message_snapshot");
        if response.secondary_clicked() {
            ui.ctx()
                .data_mut(|data| data.insert_temp(snapshot_id, (message.clone(), error)));
        }
        response.context_menu(|ui| {
            let (message, error) = ui
                .ctx()
                .data(|data| data.get_temp::<(String, bool)>(snapshot_id))
                .unwrap_or_else(|| (message.clone(), error));
            if ui.button("Copy message").clicked() {
                ui.ctx().copy_text(message.clone());
                ui.close();
            }
            if ui.button("Show details…").clicked() {
                self.open_message_details(message.clone(), error);
                ui.close();
            }
        });
    }

    pub(super) fn open_message_details(&mut self, message: String, error: bool) {
        let restore_focus = self.ctx.memory(|memory| memory.focused());
        if let Some(id) = restore_focus {
            self.ctx.memory_mut(|memory| memory.surrender_focus(id));
        }
        if let Some(tab) = self.tab_mut() {
            tab.focus = false;
        }
        self.completions = None;
        self.status_details = Some(Details {
            message,
            error,
            restore_focus,
        });
    }

    pub(super) fn status_details_ui(&mut self) {
        let Some(details) = &self.status_details else {
            return;
        };
        let ctx = self.ctx.clone();
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::new("message_details"))
            .backdrop_color(Color32::from_black_alpha(80))
            .show(&ctx, |ui| {
                ui.set_width((ctx.content_rect().width() - 48.0).clamp(260.0, 760.0));
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(if details.error {
                            "Error details"
                        } else {
                            "Message details"
                        })
                        .size(20.0)
                        .strong(),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        close |= ui.button("Close").clicked();
                        if ui.button("Copy all").clicked() {
                            ui.ctx().copy_text(details.message.clone());
                        }
                    });
                });
                ui.separator();
                egui::ScrollArea::vertical()
                    .id_salt("message_details_scroll")
                    .max_height((ctx.content_rect().height() - 160.0).clamp(100.0, 540.0))
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        // A borrowed str is a read-only TextBuffer: selection,
                        // Ctrl+A and Ctrl+C work, editing cannot change the log.
                        let mut text = details.message.as_str();
                        ui.add(
                            egui::TextEdit::multiline(&mut text)
                                .id_salt("message_details_text")
                                .font(egui::TextStyle::Monospace)
                                .desired_width(f32::INFINITY)
                                .desired_rows(8),
                        );
                    });
            });
        if close || modal.should_close() {
            let details = self.status_details.take().unwrap();
            if let Some(id) = details.restore_focus {
                ctx.memory_mut(|memory| memory.request_focus(id));
            }
        }
    }
}

fn short_preview(text: &str) -> String {
    let mut chars = text.chars();
    let mut preview: String = chars.by_ref().take(180).collect();
    if chars.next().is_some() {
        preview.push('…');
    }
    preview
}

#[cfg(test)]
#[path = "status_message_tests.rs"]
mod tests;

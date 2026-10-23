//! Keyboard preferences: alias recording, conflict reassignment and command filters.
use super::*;

impl VEditApp {
    fn start_shortcut_recording(&mut self, action: Action, index: Option<usize>) {
        self.keybinding_ui.capture = Some(action);
        self.keybinding_ui.capture_add = index.is_none();
        self.keybinding_ui.capture_index = index.unwrap_or_default();
        self.keybinding_ui.search_capture = false;
        self.keybinding_ui.pending_conflict = None;
        self.keybinding_ui.feedback = None;
        if let Some(id) = self.ctx.memory(|memory| memory.focused()) {
            self.ctx.memory_mut(|memory| memory.surrender_focus(id));
        }
    }
    pub(in crate::app) fn keyboard_settings_ui(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;
        editor_preferences::preference_heading(ui, "KEYBOARD SHORTCUTS");
        ui.label(RichText::new("Choose a command, then record or add a shortcut. Changes apply immediately and are saved.").size(11.0).color(MUTED));
        ui.label(RichText::new("Context shows where a command works. Separate contexts may share a key. Text navigation, clipboard and Vim motions retain their input meaning.").size(10.5).color(MUTED));
        if ui.link("Vim key forwarding…").clicked() {
            self.open_settings_section("Vim");
        }
        ui.horizontal_wrapped(|ui| {
            if ui
                .add(
                    egui::TextEdit::singleline(&mut self.keybinding_ui.query)
                        .id(egui::Id::new("keyboard_shortcut_search"))
                        .hint_text("Search commands, shortcuts or contexts")
                        .desired_width((ui.available_width() - 135.0).max(100.0)),
                )
                .changed()
            {
                self.keybinding_ui.search_stroke = None;
            }
            if ui
                .button("Record search")
                .on_hover_text("Press a key combination to find its commands")
                .clicked()
            {
                self.keybinding_ui.capture = None;
                self.keybinding_ui.search_capture = true;
                self.keybinding_ui.pending_conflict = None;
                self.keybinding_ui.feedback = None;
                if let Some(id) = self.ctx.memory(|memory| memory.focused()) {
                    self.ctx.memory_mut(|memory| memory.surrender_focus(id));
                }
            }
        });
        ui.horizontal_wrapped(|ui| {
            egui::ComboBox::from_id_salt("shortcut_filter")
                .selected_text(self.keybinding_ui.filter.label())
                .show_ui(ui, |ui| {
                    for filter in [
                        Filter::All,
                        Filter::Modified,
                        Filter::Disabled,
                        Filter::Default,
                    ] {
                        ui.selectable_value(&mut self.keybinding_ui.filter, filter, filter.label());
                    }
                });
            if ui
                .add_enabled(
                    !self.settings.keybindings.is_empty(),
                    egui::Button::new("Reset all"),
                )
                .clicked()
            {
                self.settings.keybindings.clear();
                self.keybinding_ui.capture = None;
                self.keybinding_ui.pending_conflict = None;
                self.keybinding_ui.feedback =
                    Some(("All shortcuts restored to defaults.".into(), false));
                changed = true;
            }
            if let Some(stroke) = self.keybinding_ui.search_stroke {
                ui.label(format!("Key: {}", stroke.label()));
                if ui.small_button("Clear key search").clicked() {
                    self.keybinding_ui.search_stroke = None;
                }
            }
        });
        if self.keybinding_ui.capture.is_some() || self.keybinding_ui.search_capture {
            ui.horizontal_wrapped(|ui| {
                let prompt = self.keybinding_ui.capture.map_or_else(
                    || "Press a shortcut to search - Esc cancels".into(),
                    |action| format!("Press shortcut for {} - Esc cancels", action.label()),
                );
                ui.label(RichText::new(prompt).color(ACCENT));
                if ui.button("Cancel recording").clicked() {
                    self.keybinding_ui.capture = None;
                    self.keybinding_ui.search_capture = false;
                }
            });
        }
        if let Some(pending) = &self.keybinding_ui.pending_conflict {
            let PendingConflict {
                action,
                index,
                stroke,
            } = *pending;
            let owners = conflicts(&self.settings.keybindings, action, stroke)
                .iter()
                .map(|action| action.label())
                .collect::<Vec<_>>()
                .join(", ");
            ui.label(
                RichText::new(format!(
                    "{} is used by {}. Reassign it to {}? Other shortcuts stay available.",
                    stroke.label(),
                    owners,
                    action.label()
                ))
                .color(ORANGE),
            );
            ui.horizontal_wrapped(|ui| {
                if ui.button("Reassign shortcut").clicked() {
                    match assign_alias(&self.settings.keybindings, action, index, stroke, true) {
                        Ok(candidate) => {
                            self.settings.keybindings = candidate;
                            self.keybinding_ui.pending_conflict = None;
                            changed = true;
                        }
                        Err(error) => self.keybinding_ui.feedback = Some((error, true)),
                    }
                }
                if ui.button("Keep existing").clicked() {
                    self.keybinding_ui.pending_conflict = None;
                }
            });
        }
        if let Some((message, error)) = &self.keybinding_ui.feedback {
            ui.label(
                RichText::new(message)
                    .size(11.0)
                    .color(if *error { RED } else { MUTED }),
            );
        }
        let query = self.keybinding_ui.query.trim().to_lowercase();
        let rows: Vec<_> = Action::ALL
            .iter()
            .copied()
            .filter(|action| {
                let customized = self.settings.keybindings.contains_key(action.id());
                let keys = strokes(&self.settings.keybindings, *action);
                let visible = match self.keybinding_ui.filter {
                    Filter::All => true,
                    Filter::Modified => customized,
                    Filter::Disabled => customized && keys.is_empty(),
                    Filter::Default => !customized,
                };
                visible
                    && self
                        .keybinding_ui
                        .search_stroke
                        .is_none_or(|stroke| keys.contains(&stroke))
                    && format!(
                        "{} {} {} {} {}",
                        action.label(),
                        action.id(),
                        action.group(),
                        action.scope().label(),
                        self.shortcut_label(*action)
                    )
                    .to_lowercase()
                    .contains(&query)
            })
            .collect();
        ui.label(
            RichText::new(format!("{} actions", rows.len()))
                .size(10.0)
                .color(MUTED),
        );
        if rows.is_empty() {
            ui.label("No commands match this search.");
        }
        for action in rows {
            ui.push_id(action.id(), |ui| {
                changed |= self.keyboard_shortcut_row(ui, action);
            });
        }
        changed
    }
    fn keyboard_shortcut_row(&mut self, ui: &mut egui::Ui, action: Action) -> bool {
        let mut changed = false;
        let keys = strokes(&self.settings.keybindings, action);
        let customized = self.settings.keybindings.contains_key(action.id());
        let status = if customized && keys.is_empty() {
            "Disabled"
        } else if customized {
            "Modified"
        } else {
            "Default"
        };
        let recording = self.keybinding_ui.capture == Some(action);
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(6, 4))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(action.label()).size(12.0).color(TEXT))
                        .on_hover_text(action.context_hint());
                    ui.label(
                        RichText::new(format!("{} · {status}", action.scope().label()))
                            .size(10.0)
                            .color(if customized { ACCENT } else { MUTED }),
                    );
                });
                ui.horizontal_wrapped(|ui| {
                    for (index, stroke) in keys.iter().enumerate() {
                        let text = if recording
                            && !self.keybinding_ui.capture_add
                            && index == self.keybinding_ui.capture_index
                        {
                            "Press a shortcut...".into()
                        } else {
                            stroke.label()
                        };
                        if ui
                            .button(text)
                            .on_hover_text(format!(
                                "Change {} for {}",
                                stroke.label(),
                                action.label()
                            ))
                            .clicked()
                        {
                            self.start_shortcut_recording(action, Some(index));
                        }
                        if ui
                            .small_button("Remove")
                            .on_hover_text(format!(
                                "Remove {} from {}",
                                stroke.label(),
                                action.label()
                            ))
                            .clicked()
                        {
                            let mut retained = keys.clone();
                            retained.remove(index);
                            set_strokes(&mut self.settings.keybindings, action, &retained);
                            self.keybinding_ui.capture = None;
                            self.keybinding_ui.pending_conflict = None;
                            changed = true;
                        }
                    }
                    let add_label = if recording && self.keybinding_ui.capture_add {
                        "Press a shortcut..."
                    } else if keys.is_empty() {
                        "Unassigned"
                    } else {
                        "Add shortcut"
                    };
                    if ui
                        .add_enabled(
                            keys.len() < crate::cli::MAX_SHORTCUT_ALIASES,
                            egui::Button::new(add_label),
                        )
                        .on_hover_text(
                            "Add an alternative shortcut without replacing existing ones",
                        )
                        .clicked()
                    {
                        self.start_shortcut_recording(action, None);
                    }
                    if !keys.is_empty()
                        && ui
                            .small_button("Disable")
                            .on_hover_text("Remove every shortcut for this command")
                            .clicked()
                    {
                        self.settings.keybindings.insert(action.id().into(), None);
                        self.keybinding_ui.capture = None;
                        self.keybinding_ui.pending_conflict = None;
                        changed = true;
                    }
                    if customized
                        && ui
                            .small_button("Reset")
                            .on_hover_text(format!(
                                "Restore defaults: {}",
                                action.defaults().join(" / ")
                            ))
                            .clicked()
                    {
                        let mut candidate = self.settings.keybindings.clone();
                        candidate.remove(action.id());
                        match validate_inner(&candidate, true) {
                            Ok(()) => {
                                self.settings.keybindings = candidate;
                                self.keybinding_ui.capture = None;
                                self.keybinding_ui.pending_conflict = None;
                                changed = true;
                            }
                            Err(error) => self.keybinding_ui.feedback = Some((error, true)),
                        }
                    }
                });
            });
        ui.separator();
        changed
    }
}

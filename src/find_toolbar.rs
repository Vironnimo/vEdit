//! In-document search and replacement toolbar presentation.
use super::*;

impl VEditApp {
    pub(super) fn find_ui(&mut self, ui: &mut egui::Ui) {
        let find_id = ui.make_persistent_id("find_query");
        let replace_id = ui.make_persistent_id("replace_query");
        let interactive = !self.settings_open
            && self.status_details.is_none()
            && !self.agent_marks_modal_open()
            && self.palette.is_none()
            && self.picker.is_none()
            && self.close_action.is_none()
            && self.save_as.is_none()
            && self.diff.is_none()
            && !self.git_detail_open()
            && !self.navigation_modal_open()
            && !self.navigation_pending()
            && self.screen_jump.is_none()
            && self.vim_command.is_none()
            && self.find.prompt.is_none()
            && self.context_file_dialog.is_none();
        let mut close = false;
        let mut replace_one = false;
        let mut replace_all = false;
        let controls = |app: &mut Self, ui: &mut egui::Ui| {
            let case = ui
                .toggle_value(&mut app.find_case, "Aa")
                .on_hover_text("Match upper and lower case exactly");
            case.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::WidgetType::Checkbox,
                    true,
                    app.find_case,
                    "Match case",
                )
            });
            let word = ui
                .toggle_value(&mut app.find_word, "ab")
                .on_hover_text("Match whole words only");
            word.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::WidgetType::Checkbox,
                    true,
                    app.find_word,
                    "Whole word",
                )
            });
            let regex = ui.toggle_value(&mut app.find_regex, ".*").on_hover_text(
                "Use a regular expression (Rust syntax); Replace inserts groups with $1",
            );
            regex.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::WidgetType::Checkbox,
                    true,
                    app.find_regex,
                    "Use regular expression",
                )
            });
            if regex.changed() {
                app.session_dirty = true;
            }
            if case.changed() || word.changed() || regex.changed() {
                app.find_match = None;
                if !app.find_text.is_empty() {
                    app.find_next(false);
                }
                if let Some(tab) = app.tab_mut() {
                    tab.focus = false;
                }
            }
            let matches = app.editor_search_matches();
            ui.add_enabled_ui(!matches.is_empty(), |ui| {
                let previous = small_button(
                    ui,
                    "↑",
                    &format!(
                        "Previous match · {}",
                        app.shortcut_label(keybindings::Action::FindPrevious)
                    ),
                );
                previous.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::WidgetType::Button,
                        ui.is_enabled(),
                        "Previous match",
                    )
                });
                let next = small_button(
                    ui,
                    "↓",
                    &format!(
                        "Next match · {} or Enter",
                        app.shortcut_label(keybindings::Action::FindNext)
                    ),
                );
                next.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::WidgetType::Button,
                        ui.is_enabled(),
                        "Next match",
                    )
                });
                if previous.clicked() || next.clicked() {
                    app.find_next(previous.clicked());
                    if let Some(tab) = app.tab_mut() {
                        tab.focus = false;
                    }
                }
            });
            let current = app
                .tab()
                .and_then(|tab| {
                    let cursor = app.find.active_cursor(tab, app.current_pane_id());
                    let index = matches.partition_point(|range| range.start < cursor);
                    matches
                        .get(index)
                        .filter(|range| range.start == cursor)
                        .map(|_| index)
                })
                .map_or(0, |index| index + 1);
            let searching = app.find_search_running();
            if let Some(error) = app.editor_search_error() {
                let invalid = ui
                    .label(RichText::new("Invalid pattern").size(11.0).color(RED))
                    .on_hover_text(format!("This regular expression cannot search: {error}"));
                invalid.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Label, true, "Invalid pattern")
                });
                return;
            }
            ui.label(
                RichText::new(if searching {
                    "Searching…".into()
                } else {
                    format!("{current} of {}", matches.len())
                })
                .size(11.0)
                .color(
                    if !searching && !app.find_text.is_empty() && matches.is_empty() {
                        ORANGE
                    } else {
                        MUTED
                    },
                ),
            )
            .on_hover_text(if searching {
                "Searching this document in the background; keep typing to refine the query"
            } else if matches.is_empty() {
                "No matching text in this document"
            } else {
                "Current match and total matches · Use the next / previous buttons or their configured shortcuts"
            });
        };
        egui::Frame::new()
            .fill(SURFACE)
            .inner_margin(Margin::symmetric(12, 7))
            .show(ui, |ui| {
                if !interactive { ui.disable(); }
                ui.spacing_mut().item_spacing.y = 5.0;
                let compact = ui.available_width() < 600.0;
                ui.horizontal(|ui| {
                    let available = ui.available_width();
                    let toggle = small_button(
                        ui,
                        if self.replace_visible { "▾" } else { "▸" },
                        "Toggle replace",
                    );
                    toggle.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, self.replace_visible, "Show replace"));
                    if toggle.clicked() {
                        self.replace_visible = !self.replace_visible;
                    }
                    let query_width = (available - if compact { 72.0 } else { 320.0 }).clamp(30.0, 360.0);
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut self.find_text)
                            .id(find_id)
                            .hint_text("Find in this file")
                            .desired_width(query_width),
                    ).on_hover_text(format!("Find text, or a regular expression with .* · Enter / {}: next · {}: previous · Esc: close. Vim Normal: / searches; the default Ctrl+F scrolls a page.", self.shortcut_label(keybindings::Action::FindNext), self.shortcut_label(keybindings::Action::FindPrevious)));
                    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Find in file"));
                    if self.find_focus {
                        response.request_focus();
                        self.find_focus = false;
                        if let Some(tab) = self.tab_mut() {
                            tab.focus = false;
                        }
                    }
                    if response.changed() {
                        self.find_match = None;
                        if !self.find_text.is_empty() { self.find_next(false); }
                        response.request_focus();
                        if let Some(tab) = self.tab_mut() {
                            tab.focus = false;
                        }
                    }
                    let previous = (response.has_focus() || response.lost_focus())
                        && ui.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, Key::Enter));
                    if previous || ((response.has_focus() || response.lost_focus())
                        && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter)))
                    {
                        if !self.find_text.is_empty() { self.find_next(previous); }
                        response.request_focus();
                        if let Some(tab) = self.tab_mut() {
                            tab.focus = false;
                        }
                    }
                    if !compact { controls(self, ui); }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let response = small_button(ui, "×", "Close find · Esc");
                        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Close find"));
                        close |= response.clicked();
                    });
                });
                if compact { ui.horizontal_wrapped(|ui| { controls(self, ui); }); }
                if self.replace_visible {
                    ui.horizontal(|ui| {
                        let width = (ui.available_width() - if compact { 12.0 } else { 174.0 }).clamp(30.0, 360.0);
                        let response = ui.add(
                            egui::TextEdit::singleline(&mut self.replace_text)
                                .id(replace_id)
                                .hint_text("Replace with")
                                .desired_width(width),
                        ).on_hover_text("Replacement text · Enter: replace this match · Esc: close Find and Replace");
                        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Replace with"));
                        if (response.has_focus() || response.lost_focus()) && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Enter)) {
                            replace_one = true;
                        }
                        if !compact {
                            let enabled = self.tab().is_some_and(|tab| !tab.loading && !tab.saving && tab.error.is_none()) && !self.editor_search_matches().is_empty();
                            ui.add_enabled_ui(enabled, |ui| {
                                replace_one |= design::ghost_button(ui, "Replace", "Replace the current match and move to the next").clicked();
                                replace_all |= design::ghost_button(ui, "Replace all", "Replace every match in this document").clicked();
                            });
                        }
                    });
                    if compact {
                        let enabled = self.tab().is_some_and(|tab| !tab.loading && !tab.saving && tab.error.is_none()) && !self.editor_search_matches().is_empty();
                        ui.add_enabled_ui(enabled, |ui| { ui.horizontal_wrapped(|ui| {
                            replace_one |= design::ghost_button(ui, "Replace", "Replace the current match and move to the next").clicked();
                            replace_all |= design::ghost_button(ui, "Replace all", "Replace every match in this document").clicked();
                        }); });
                    }
                }
            });
        // Escape belongs to the whole toolbar, including Replace and option buttons.
        close |= interactive
            && !egui::Popup::is_any_open(ui.ctx())
            && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Escape));
        if close {
            self.find_visible = false;
            ui.ctx().memory_mut(|memory| {
                if let Some(id) = memory.focused() {
                    memory.surrender_focus(id);
                }
            });
            if let Some(tab) = self.tab_mut() {
                tab.focus = tab.agent_view == crate::cli::DocumentView::Source;
            }
            return;
        }
        let editable = self
            .tab()
            .is_some_and(|tab| !tab.loading && !tab.saving && tab.error.is_none());
        if editable && replace_one {
            let matches = self.editor_search_matches();
            let cursor = self.tab().map_or(0, |tab| {
                self.find.active_cursor(tab, self.current_pane_id())
            });
            if let Some(range) = matches
                .iter()
                .find(|range| range.start >= cursor)
                .or_else(|| matches.first())
                .cloned()
            {
                let options = self.find_options();
                let replacement = if options.regex {
                    // Expand `$1` for this match exactly as Replace all would.
                    let query = self.find_text.clone();
                    let template = self.replace_text.clone();
                    self.tab()
                        .and_then(|tab| {
                            editor::replacements(&tab.text, &query, &template, options)
                                .into_iter()
                                .find(|(found, _)| *found == range)
                        })
                        .map_or(template, |(_, expanded)| expanded)
                } else {
                    self.replace_text.clone()
                };
                // Without Vim, the text widget owns Undo: record the text
                // before this programmatic edit so Ctrl+Z restores it exactly.
                if let Some(id) = self.tab().map(|tab| tab.id) {
                    self.context_native_undo_checkpoint(id);
                }
                if let Some(tab) = self.tab_mut() {
                    let edit = editor::replace_selection(&tab.text, range, &replacement);
                    let cursor = edit.selection.end;
                    tab.apply(edit);
                    tab.vim.cursor = cursor;
                }
                self.find_match = None;
                // Start at the replacement's end so removing a match cannot skip
                // the next match that has just moved into that same position.
                let no_matches = self.editor_search_matches().is_empty();
                if no_matches && !self.find_search_running() {
                    self.status("Replaced the last match", false);
                } else {
                    self.find_next(false);
                }
                if let Some(tab) = self.tab_mut() {
                    tab.focus = false;
                }
                ui.ctx()
                    .memory_mut(|memory| memory.request_focus(replace_id));
            }
        }
        if editable && replace_all {
            let query = self.find_text.clone();
            let replacement = self.replace_text.clone();
            let options = self.find_options();
            if let Some(id) = self.tab().map(|tab| tab.id) {
                self.context_native_undo_checkpoint(id);
            }
            if let Some(tab) = self.tab_mut() {
                let (edit, count) = editor::replace_all(
                    &tab.text,
                    tab.selection.clone(),
                    &query,
                    &replacement,
                    options,
                );
                if count > 0 {
                    tab.apply(edit);
                }
                tab.focus = false;
                self.find_match = None;
                self.status(format!("Replaced {count} matches"), false);
                ui.ctx()
                    .memory_mut(|memory| memory.request_focus(replace_id));
            }
        }
    }
}

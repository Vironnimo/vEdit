//! Pane, tab and document-view presentation over shared documents.
use super::*;

impl VEditApp {
    pub(super) fn editor_ui(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(Margin::ZERO))
            .show(ui, |ui| {
                if self.projects.is_empty() {
                    self.welcome(ui);
                } else if self.change_review_visible() {
                    self.change_review_ui(ui);
                } else {
                    self.split_editor_ui(ui);
                }
            });
    }
    pub(super) fn editor_pane_ui(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing.y = 0.0;
        self.tabs_ui(ui);
        ui.separator();
        if self.render_pane_is_focused() && self.find_visible {
            self.find_ui(ui);
        }
        if self.render_pane_is_focused() && self.vim_command.is_some() {
            self.ex_ui(ui);
        }
        if self.render_pane_is_focused() && self.vim_search_active() {
            self.vim_search_ui(ui);
        }
        self.disk_conflict_banner(ui);
        let error = self.tab().and_then(|t| t.error.clone());
        if let Some(error) = error {
            egui::Frame::new()
                .fill(PANEL.lerp_to_gamma(RED, 0.12))
                .inner_margin(10)
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(error).size(12.0).color(RED));
                        if ui.button("Save as…").clicked()
                            && let Some(id) = self.tab().map(|tab| tab.id)
                        {
                            self.request_save_as(id);
                        }
                        let can_reload = self.tab().is_some_and(|tab| !tab.dirty && !tab.saving && !tab.loading && tab.path.is_some());
                        if ui.add_enabled(can_reload, egui::Button::new("Reload from disk"))
                            .on_disabled_hover_text("Keep your edits with Save as… first. To discard them, close this tab, choose Discard, then reopen the original file.")
                            .clicked() {
                            self.reload_current();
                        }
                        if self.tab().is_some_and(|tab| tab.dirty) {
                            ui.label(RichText::new("Your edits are kept. Save as… preserves them in another file.").size(11.0).color(TEXT));
                        }
                    });
                });
        }
        if self.tab().is_none() {
            ui.vertical_centered(|ui| {
                ui.add_space((ui.available_height() * 0.28).max(40.0));
                brand(ui, 45.0);
                ui.add_space(20.0);
                ui.label(
                    RichText::new("Your workspace is ready.")
                        .size(23.0)
                        .color(TEXT),
                );
                ui.add_space(8.0);
                ui.label(RichText::new("Find a file and make it yours.").color(MUTED));
                ui.add_space(24.0);
                if accent_button(
                    ui,
                    &format!(
                        "Open a file   {}",
                        self.shortcut_label(keybindings::Action::QuickOpen)
                    ),
                )
                .clicked()
                {
                    self.open_palette(Palette::Files);
                }
                ui.add_space(8.0);
                if ui.button("Create a new file").clicked() {
                    self.new_file();
                }
            });
            return;
        }
        if self.tab().is_some_and(|t| t.loading) {
            ui.vertical_centered(|ui| {
                ui.add_space(60.0);
                ui.spinner();
                ui.label("Opening file…");
            });
            return;
        }
        if self
            .tab()
            .is_some_and(|t| t.agent_view == crate::cli::DocumentView::Preview)
        {
            self.markdown_preview(ui);
        } else {
            self.text_editor(ui);
        }
    }
    pub(super) fn set_document_view(&mut self, view: crate::cli::DocumentView) {
        self.cancel_vim_search(false);
        let ctx = self.ctx.clone();
        if let Some(tab) = self.tab_mut() {
            if view == crate::cli::DocumentView::Preview && !is_markdown(tab.path.as_deref()) {
                return;
            }
            let action = tab.vim.finish_insert(&mut tab.text);
            if action.text_changed {
                tab.changed();
            }
            tab.agent_view = view;
            tab.focus = view == crate::cli::DocumentView::Source;
            if !tab.focus {
                ctx.memory_mut(|memory| {
                    if let Some(id) = memory.focused() {
                        memory.surrender_focus(id);
                    }
                });
            }
        }
        self.completions = None;
        self.completion_request = None;
        self.find_visible = false;
        self.editor_focused = false;
        self.session_dirty = true;
        self.capture_pane_navigation();
    }
    pub(super) fn markdown_preview(&mut self, ui: &mut egui::Ui) {
        self.editor_focused = false;
        self.editor_rect = None;
        self.editor_cursor_rect = None;
        let keyboard_active = self.render_pane_is_focused() && !self.explorer_keyboard_focused();
        let Some(project) = self.projects.get(self.active_project) else {
            return;
        };
        let Some(tab) = project.tabs.get(project.active_tab) else {
            return;
        };
        let path = tab
            .path
            .clone()
            .unwrap_or_else(|| self.document_directory().join("Untitled.md"));
        if tab.scroll_to_cursor {
            let line = tab
                .lines
                .line_col(&tab.text, tab.revision, tab.vim.cursor)
                .0;
            self.markdown.reveal_line(tab.id, line);
        }
        let response = self.markdown.show(
            ui,
            tab.id,
            tab.revision,
            &tab.text,
            &path,
            self.settings.font_size,
            self.settings.smooth_scroll,
            keyboard_active,
            self.settings.swap_jk,
        );
        if let Some(tab) = self.tab_mut() {
            tab.scroll_to_cursor = false;
        }
        if let Some(path) = response.open_file {
            self.open_argument(path);
        }
        if let Some(line) = response.source_line {
            self.set_document_view(crate::cli::DocumentView::Source);
            if let Some(tab) = self.tab_mut() {
                let at = editor::go_to_line(&tab.text, line);
                tab.set_cursor(at..at);
            }
        }
    }
    pub(super) fn tabs_ui(&mut self, ui: &mut egui::Ui) {
        let tabs: Vec<_> = self
            .current_pane_tab_ids()
            .into_iter()
            .filter_map(|id| {
                self.projects.iter().enumerate().find_map(|(pi, project)| {
                    project
                        .tabs
                        .iter()
                        .enumerate()
                        .find(|(_, tab)| tab.id == id)
                        .map(|(ti, tab)| (pi, ti, tab))
                })
            })
            .map(|(pi, ti, t)| {
                (
                    t.id,
                    t.title.clone(),
                    t.dirty,
                    t.loading,
                    t.path.clone(),
                    t.annotations.len(),
                    t.saving,
                    pi,
                    ti,
                    t.preview_tab,
                )
            })
            .collect();
        let tab_labels = crate::file_identity::disambiguate_labels(
            &tabs
                .iter()
                .map(|tab| (tab.1.as_str(), tab.4.as_deref()))
                .collect::<Vec<_>>(),
        );
        let active_id = self.tab().map(|tab| tab.id);
        let mut tab_drop_rects = Vec::new();
        let reveal_active = self.tab_strip_active != active_id;
        self.tab_strip_active = active_id;
        let mut select = None;
        let mut close = None;
        let mut context_action = None;
        let markdown = self
            .tab()
            .is_some_and(|tab| is_markdown(tab.path.as_deref()));
        let view = self.tab().map(|tab| tab.agent_view).unwrap_or_default();
        ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let controls_width = 44.0 + if markdown { 38.0 } else { 0.0 };
        let tab_width = (ui.available_width() - controls_width).max(20.0);
        ui.allocate_ui_with_layout(Vec2::new(tab_width, 28.0), Layout::left_to_right(Align::Min), |ui| {
        let tab_scroll = egui::ScrollArea::horizontal()
            .id_salt("document_tabs")
            .auto_shrink([false, true])
            .scroll_source(egui::scroll_area::ScrollSource { drag: egui::scroll_area::DragScroll::Never, ..Default::default() })
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.horizontal(|ui| {
                    for ((id, original_title, dirty, loading, path, marks, saving, project_index, tab_index, preview), title) in tabs.iter().zip(&tab_labels) {
                        let selected = Some(*id) == active_id;
                        let font = FontId::proportional(12.0);
                        let title_galley = design::document_tab_galley(
                            ui, title,
                            font.clone(),
                            if selected { TEXT } else { MUTED },
                            f32::INFINITY, *preview,
                        );
                        let width = (title_galley.size().x + 59.0).clamp(96.0, if title == original_title { 220.0 } else { 290.0 });
                        let (rect, response) =
                            ui.allocate_exact_size(Vec2::new(width, 28.0), egui::Sense::click_and_drag());
                        tab_drop_rects.push((*id, rect));
                        response.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::WidgetType::Button, true, title)
                        });
                        if let Some(action) = context_actions::tab_menu(&response, path.is_some(), !loading && !saving, path.is_some() && !loading, &self.settings.keybindings) {
                            context_action = Some((*id, action));
                        }
                        if selected && reveal_active {
                            response.scroll_to_me(None);
                        }
                        let hovered = response.hovered();
                        if hovered || response.context_menu_opened() {
                            ui.painter()
                                .rect_filled(rect, 5.0, HOVER);
                        }
                        if selected {
                            ui.painter().line_segment(
                                [rect.left_bottom() + Vec2::new(8.0, -0.5), rect.right_bottom() + Vec2::new(-8.0, -0.5)],
                                Stroke::new(1.5, ACCENT.gamma_multiply(0.85)),
                            );
                        }
                        let badge = file_badge(path.as_deref().unwrap_or(Path::new("")));
                        ui.painter().text(
                            rect.left_center() + Vec2::new(9.0, 0.0),
                            egui::Align2::LEFT_CENTER,
                            badge,
                            FontId::monospace(9.5),
                            file_color(path.as_deref()),
                        );
                        let clip = egui::Rect::from_min_max(
                            rect.min + Vec2::new(30.0, 0.0),
                            rect.max - Vec2::new(29.0, 0.0),
                        );
                        let painter = ui.painter().with_clip_rect(clip);
                        if title != original_title && title_galley.size().x > clip.width() {
                            // Keep the distinguishing folder visible even when
                            // a long shared filename must be shortened.
                            let suffix = title.strip_prefix(original_title).unwrap_or_default();
                            let suffix = design::truncated_galley(ui, suffix, FontId::proportional(10.5), MUTED, clip.width() * 0.7);
                            let filename = design::document_tab_galley(ui, original_title, font, if selected { TEXT } else { MUTED }, (clip.width() - suffix.size().x - 4.0).max(1.0), *preview);
                            painter.galley(egui::pos2(clip.left(), rect.center().y - filename.size().y * 0.5), filename, TEXT);
                            painter.galley(egui::pos2(clip.right() - suffix.size().x, rect.center().y - suffix.size().y * 0.5), suffix, MUTED);
                        } else {
                            painter.galley(
                                egui::pos2(
                                    rect.left() + 30.0,
                                    rect.center().y - title_galley.size().y * 0.5,
                                ),
                                title_galley,
                                TEXT,
                            );
                        }
                        let close_rect = egui::Rect::from_center_size(
                            egui::pos2(rect.right() - 16.0, rect.center().y),
                            Vec2::splat(20.0),
                        );
                        let close_response = ui.interact(
                            close_rect,
                            ui.make_persistent_id(("tab_close", id)),
                            egui::Sense::click(),
                        );
                        if !close_rect.contains(ui.input(|input| input.pointer.press_origin().unwrap_or_default())) {
                            self.start_tab_drag(&response, *id, title);
                        }
                        if hovered || close_response.hovered() || (selected && !*dirty) {
                            if close_response.hovered() {
                                ui.painter().rect_filled(close_rect, 4.0, SURFACE);
                            }
                            let c = close_rect.center();
                            ui.painter().line_segment(
                                [c - Vec2::splat(3.0), c + Vec2::splat(3.0)],
                                Stroke::new(1.2, MUTED),
                            );
                            ui.painter().line_segment(
                                [c + Vec2::new(-3.0, 3.0), c + Vec2::new(3.0, -3.0)],
                                Stroke::new(1.2, MUTED),
                            );
                            if close_response.clicked() {
                                close = Some(*id);
                            }
                            close_response
                                .on_hover_text(format!("Close tab in this pane · {} · Middle-click also closes", self.shortcut_label(keybindings::Action::CloseTab)))
                                .on_hover_cursor(egui::CursorIcon::PointingHand);
                        } else if *dirty {
                            ui.painter().circle_filled(close_rect.center(), 3.0, TEXT.gamma_multiply(0.8));
                        }
                        if *marks > 0 {
                            ui.painter().circle_filled(
                                egui::pos2(rect.left() + 22.0, rect.bottom() - 5.0),
                                2.2,
                                BLUE,
                            );
                        }
                        if *loading || *saving {
                            ui.painter().circle_filled(
                                egui::pos2(rect.left() + 24.0, rect.center().y),
                                2.0,
                                MUTED,
                            );
                        }
                        if response.clicked_by(egui::PointerButton::Middle) {
                            close = Some(*id);
                        }
                        if response.clicked()
                            && !self.tab_drag_active()
                            && !close_rect.contains(
                                ui.input(|input| input.pointer.interact_pos().unwrap_or_default()),
                            )
                        {
                            select = Some((*project_index, *tab_index));
                            if response.double_clicked() {
                                self.keep_tab_open(*id);
                            }
                        }
                        let mut tooltip = path.as_ref()
                            .map(|p| crate::paths::display(p))
                            .unwrap_or_else(|| "Untitled document".into());
                        if *dirty {
                            tooltip.push_str("\nUnsaved changes");
                        }
                        if *preview {
                            tooltip.push_str("\nPreview — double-click this tab to keep it open");
                        }
                        if *marks > 0 {
                            tooltip.push_str(&format!("\n{marks} review note{}", if *marks == 1 { "" } else { "s" }));
                        }
                        if *loading {
                            tooltip.push_str("\nLoading…");
                        } else if *saving {
                            tooltip.push_str("\nSaving…");
                        }
                        tooltip.push_str("\nDrag to reorder or move to another editor pane");
                        response
                            .on_hover_text(tooltip)
                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                    }
                    ui.add_space(7.0);
                    if small_button(ui, "+", &format!("New file · {}", self.shortcut_label(keybindings::Action::NewFile))).clicked() {
                        self.new_file();
                    }
                });
            });
        self.register_tab_drop_strip(&tab_scroll, std::mem::take(&mut tab_drop_rects));
        });
        ui.allocate_ui_with_layout(Vec2::new(controls_width, 28.0), Layout::right_to_left(Align::Center), |ui| {
            if self.tab_pointer_interaction_active() { ui.disable(); }
            self.agent_marks_toolbar(ui);
            if markdown {
                ui.add_space(5.0);
                self.markdown_preview_toggle(ui, view == crate::cli::DocumentView::Preview);
            }
        });
        });
        if let Some((project, index)) = select {
            self.activate_tab(project, index);
        }
        if let Some(id) = close {
            self.request_close(CloseAction::PaneTab {
                pane_id: self.current_pane_id(),
                id,
            });
        }
        if let Some((id, action)) = context_action {
            self.apply_tab_context_action(id, action);
        }
    }
}

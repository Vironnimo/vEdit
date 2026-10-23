//! Modal ordering, settings, close confirmations and diff presentation.
use super::*;

impl VEditApp {
    pub(super) fn overlays(&mut self) {
        let ctx = self.ctx.clone();
        self.context_file_dialog_ui(&ctx);
        self.palette_ui(&ctx);
        let settings_was_open = ctx
            .data(|data| data.get_temp::<bool>(egui::Id::new("external_open_settings_seen")))
            .unwrap_or(false);
        if self.settings_open && !settings_was_open {
            self.refresh_external_open_preference();
        }
        ctx.data_mut(|data| {
            data.insert_temp(
                egui::Id::new("external_open_settings_seen"),
                self.settings_open,
            )
        });
        if self.settings_open {
            let mut done = false;
            let mut changed = false;
            let modal = egui::Modal::new(egui::Id::new("settings_dialog")).show(&ctx, |ui| {
                ui.set_width((ctx.content_rect().width() - 32.0).clamp(220.0, 560.0));
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Settings").size(15.0).strong());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let close = small_button(ui, "×", "Close settings · Esc");
                        close.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::WidgetType::Button,
                                true,
                                "Close settings",
                            )
                        });
                        done |= close.clicked();
                    });
                });
                ui.add_space(10.0);
                self.settings_navigation_ui(ui);
                egui::ScrollArea::vertical()
                    .id_salt(("settings_contents", self.settings_section()))
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
                    .max_height((ctx.content_rect().height() - 200.0).clamp(65.0, 540.0))
                    .show(ui, |ui| {
                        changed |= self.settings_contents_ui(ui);
                    });
                ui.add_space(12.0);
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    // Settings apply immediately, so closing is not a primary action.
                    done |= ui
                        .add(egui::Button::new("Done").min_size(Vec2::new(70.0, 26.0)))
                        .clicked();
                    ui.label(
                        RichText::new(self.settings_save_hint())
                            .size(10.5)
                            .color(MUTED),
                    );
                });
            });
            self.settings_open = !(done || modal.should_close());
            self.session_dirty |= changed;
            self.ctx.set_zoom_factor(self.settings.ui_scale);
            self.apply_editor_font();
            if !self.settings_open {
                self.interface_zoom_draft = None;
                self.apply_pending_project_interpreter();
                self.keybinding_ui = keybindings::State::default();
                ctx.memory_mut(|memory| {
                    if let Some(id) = memory.focused() {
                        memory.surrender_focus(id);
                    }
                });
                if let Some(tab) = self.tab_mut() {
                    tab.focus = tab.agent_view == crate::cli::DocumentView::Source;
                }
                ctx.request_repaint();
            }
        }
        if let Some(action) = self.close_action
            && self.save_as.is_none()
        {
            let documents: Vec<_> = self
                .projects
                .iter()
                .enumerate()
                .flat_map(|(index, project)| {
                    project.tabs.iter().filter_map(move |tab| {
                        let affected = match action {
                            CloseAction::Tab(id) | CloseAction::PaneTab { id, .. } => tab.id == id,
                            CloseAction::Project(project_index) => index == project_index,
                            CloseAction::Exit => true,
                        };
                        if !affected || !(tab.dirty || tab.saving) {
                            return None;
                        }
                        let relative = tab.path.as_ref().map_or_else(
                            || tab.title.clone(),
                            |path| {
                                crate::paths::display(
                                    path.strip_prefix(&project.root).unwrap_or(path),
                                )
                            },
                        );
                        let full_path = tab
                            .path
                            .as_deref()
                            .map(crate::paths::display)
                            .unwrap_or_else(|| project.display_name());
                        Some((
                            format!("{} · {relative}", project.display_name()),
                            full_path,
                        ))
                    })
                })
                .collect();
            let mut save = false;
            let mut discard = false;
            let mut cancel = false;
            let modal = egui::Modal::new(egui::Id::new("unsaved_changes")).show(&ctx, |ui| {
                ui.set_width(390.0);
                ui.label(
                    RichText::new("Keep your changes?")
                        .size(20.0)
                        .strong()
                        .color(TEXT),
                );
                ui.add_space(10.0);
                ui.label(
                    RichText::new(format!(
                        "{} {} with unsaved changes.",
                        documents.len(),
                        if documents.len() == 1 {
                            "file"
                        } else {
                            "files"
                        },
                    ))
                    .color(MUTED),
                );
                ui.add_space(6.0);
                egui::ScrollArea::vertical()
                    .id_salt("unsaved_documents")
                    .max_height((ctx.content_rect().height() * 0.25).clamp(70.0, 180.0))
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for (label, path) in &documents {
                            ui.add(egui::Label::new(RichText::new(label).size(12.0)).truncate())
                                .on_hover_details(path);
                        }
                    });
                if self.close_after_save {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Saving…");
                    });
                }
                ui.add_space(20.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                    if ui
                        .add_enabled(!self.close_after_save, egui::Button::new("Discard"))
                        .clicked()
                    {
                        discard = true;
                    }
                    if ui
                        .add_enabled_ui(!self.close_after_save, |ui| {
                            design::primary_button(ui, "Save & close", Vec2::new(110.0, 28.0))
                        })
                        .inner
                        .clicked()
                    {
                        save = true;
                    }
                });
            });
            if cancel || modal.should_close() {
                self.close_action = None;
                self.close_after_save = false;
                if let Some(tab) = self.tab_mut() {
                    tab.focus = tab.agent_view == crate::cli::DocumentView::Source;
                }
                if let Some(tab) = self.tab()
                    && tab.agent_view == crate::cli::DocumentView::Source
                {
                    ctx.memory_mut(|memory| memory.request_focus(self.document_widget_id(tab.id)));
                }
            }
            if discard {
                self.perform_close(action);
            }
            if save {
                self.close_after_save = true;
                match action {
                    CloseAction::Tab(id) | CloseAction::PaneTab { id, .. } => {
                        self.save_tab(id, None)
                    }
                    CloseAction::Project(index) => {
                        let ids: Vec<_> = self
                            .projects
                            .get(index)
                            .map(|p| p.tabs.iter().filter(|t| t.dirty).map(|t| t.id).collect())
                            .unwrap_or_default();
                        for id in ids {
                            self.save_tab(id, None);
                        }
                    }
                    CloseAction::Exit => self.save_all(),
                }
            }
        }
        self.git_detail_ui();
        if let Some((title, text)) = &self.diff {
            let mut choice = None;
            let mut open = None;
            let modal = crate::git_diff_view::dialog(
                &ctx,
                egui::Id::new("git_patch_window"),
                title,
                "Close diff",
                Vec2::new(950.0, 620.0),
                true,
                |ui| {
                    if let Some(comparison) = &self.comparison {
                        choice = disk_sync::comparison_header(ui, comparison, &self.projects);
                    }
                    if self.diff_loading {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Loading changes…");
                        });
                    } else if let Some(error) = &self.diff_error {
                        ui.colored_label(RED, error);
                    } else {
                        open = self.diff_view.show(ui, title, text);
                    }
                },
            );
            if let Some(open) = open {
                self.open_diff_location(open);
            } else if modal.inner || modal.should_close() {
                self.close_diff();
            }
            if let Some(choice) = choice {
                self.close_diff();
                self.resolve_disk_conflict(choice);
            }
        }
    }
    /// Open the file of the diff dialog at the chosen line, as the change
    /// review's o and Enter do.
    fn open_diff_location(&mut self, open: crate::git_diff_view::DiffOpen) {
        self.record_jump();
        let Some(repository) = self.diff_repository.clone() else {
            return;
        };
        if open.deleted {
            self.status(
                format!(
                    "{} is deleted in this diff; there is no file to open",
                    open.path
                ),
                true,
            );
            return;
        }
        self.close_diff();
        if let Some(project) = self.projects.get_mut(self.active_project) {
            project.git_view.detail_open = false;
        }
        let path = open
            .path
            .split('/')
            .fold(repository, |path, part| path.join(part));
        self.open_file(path);
        if let Some(line) = open.line
            && let Some(tab) = self.tab_mut()
        {
            tab.pending_agent_location = Some(crate::cli::Location { line, column: 1 });
            tab.agent_location_error = None;
            self.apply_pending_agent_locations();
        }
    }
    pub(super) fn close_diff(&mut self) {
        let comparison = self.comparison.take().is_some();
        self.diff = None;
        self.diff_repository = None;
        self.diff_loading = false;
        self.diff_error = None;
        self.diff_view.reset_focus();
        if comparison {
            // A document comparison returns to the document it was opened from.
            self.restore_document_focus();
        } else {
            self.restore_git_overlay_focus();
        }
    }
    pub(super) fn restore_git_overlay_focus(&mut self) {
        self.ctx.memory_mut(|memory| {
            if let Some(id) = memory.focused() {
                memory.surrender_focus(id);
            }
        });
        if self.git_detail_open() {
            if let Some(tab) = self.tab_mut() {
                tab.focus = false;
            }
            if self.restore_git_detail_content_focus() {
                return;
            }
            let modal_id = egui::Id::new(("git-commit-details", self.active_project));
            if let Some(id) = self
                .ctx
                .data(|data| data.get_temp::<egui::Id>(modal_id.with("close_focus")))
            {
                self.ctx.memory_mut(|memory| memory.request_focus(id));
            }
            self.ctx.request_repaint();
            return;
        }
        if self.sidebar_view_visible(SidebarView::SourceControl) {
            self.focus_git_content();
            return;
        }
        self.restore_document_focus();
    }
    fn restore_document_focus(&mut self) {
        self.ctx.memory_mut(|memory| {
            if let Some(id) = memory.focused() {
                memory.surrender_focus(id);
            }
        });
        if let Some(tab) = self.tab_mut() {
            tab.focus = tab.agent_view == crate::cli::DocumentView::Source;
        }
        if let Some(tab) = self.tab()
            && tab.agent_view == crate::cli::DocumentView::Source
        {
            let id = self.document_widget_id(tab.id);
            self.ctx.memory_mut(|memory| memory.request_focus(id));
        }
        self.ctx.request_repaint();
    }
    pub(super) fn apply_editor_font(&mut self) {
        if self.applied_editor_font != self.settings.editor_font {
            self.ctx
                .set_fonts(crate::typography::fonts(self.settings.editor_font));
            self.applied_editor_font = self.settings.editor_font;
            self.ctx.request_repaint();
        }
    }
}

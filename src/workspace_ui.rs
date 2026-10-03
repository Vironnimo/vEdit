//! Fast project navigation without rebuilding or reloading open documents.
use super::*;

const PROJECT_ROW_HEIGHT: f32 = 52.0;

#[derive(Default)]
pub(super) struct WorkspaceUiState {
    query: String,
    selected_root: Option<PathBuf>,
    focus: bool,
    pending: Option<WorkspaceAction>,
}

#[derive(Clone, Copy)]
enum WorkspaceAction {
    TogglePicker,
    Cycle { next: bool },
}

struct ProjectRow {
    root: PathBuf,
    name: String,
    path: String,
    short_path: String,
    open: usize,
    dirty: usize,
    current: bool,
}

fn picker_id() -> egui::Id {
    egui::Id::new("workspace_picker")
}

fn query_id() -> egui::Id {
    egui::Id::new("workspace_picker_query")
}

impl MorrowApp {
    pub(super) fn workspace_picker_open(&self) -> bool {
        egui::Popup::is_id_open(&self.ctx, picker_id())
    }

    pub(super) fn open_workspace_picker(&mut self) {
        if self.projects.is_empty() {
            self.pick_project();
            return;
        }
        self.palette = None;
        self.completions = None;
        self.cancel_navigation();
        self.cancel_vim_search(false);
        if let Some(tab) = self.tab_mut() {
            tab.focus = false;
        }
        self.workspace_ui.query.clear();
        self.workspace_ui.selected_root = self.project().map(|project| project.root.clone());
        self.workspace_ui.focus = true;
        egui::Popup::open_id(&self.ctx, picker_id());
        // Keyboard opening is deferred until after the header has rendered.
        // The query does not exist in this frame's accessibility tree yet.
        if let Some(focused) = self.ctx.memory(|memory| memory.focused()) {
            self.ctx
                .memory_mut(|memory| memory.surrender_focus(focused));
        }
        self.ctx.request_repaint();
    }

    fn close_workspace_picker(&mut self) {
        egui::Popup::close_id(&self.ctx, picker_id());
        self.ctx
            .memory_mut(|memory| memory.surrender_focus(query_id()));
        if let Some(tab) = self.tab_mut() {
            tab.focus = tab.agent_view == crate::cli::DocumentView::Source;
        }
    }

    /// Only project shortcuts are intercepted. While the popup is open, its
    /// query owns text events and editor commands wait for the next frame.
    pub(super) fn workspace_keys(&mut self) -> bool {
        if self.picker.is_some()
            || self.settings_open
            || self.agent_marks_modal_open()
            || self.save_as.is_some()
            || self.close_action.is_some()
            || self.context_file_dialog.is_some()
            || self.diff.is_some()
            || self.git_detail_open()
            || self.navigation_modal_open()
            || self.screen_jump.is_some()
            || self.vim_command.is_some()
        {
            return false;
        }
        let shortcut = self.ctx.input(|input| {
            input.events.iter().enumerate().find_map(|(index, event)| {
                let action = if self.shortcut_matches(keybindings::Action::SwitchProject, event) {
                    WorkspaceAction::TogglePicker
                } else if self.shortcut_matches(keybindings::Action::NextProject, event) {
                    WorkspaceAction::Cycle { next: true }
                } else if self.shortcut_matches(keybindings::Action::PreviousProject, event) {
                    WorkspaceAction::Cycle { next: false }
                } else {
                    return None;
                };
                Some((index, action))
            })
        });
        if let Some((index, action)) = shortcut {
            if !self.workspace_picker_open() && self.earlier_shortcut_boundary(index) {
                return false;
            }
            if self.workspace_picker_open() {
                // Prefix input belongs to the query being dismissed, never
                // to the source that regains focus afterwards.
                self.ctx.input_mut(|input| {
                    input.events.retain(|event| {
                        ![
                            keybindings::Action::SwitchProject,
                            keybindings::Action::NextProject,
                            keybindings::Action::PreviousProject,
                        ]
                        .into_iter()
                        .any(|action| self.shortcut_matches(action, event))
                    });
                });
                self.workspace_discard_trailing_text();
                self.apply_workspace_action(action);
            } else {
                self.ctx.input_mut(|input| input.events.truncate(index));
                // Modal motions run in order; native Insert events continue
                // into the old source widget, including its real undo history.
                let navigating = self.navigation_keys();
                let jumping = self.screen_jump_keys();
                if !navigating && !jumping {
                    self.shortcuts();
                    self.handle_vim();
                }
                self.workspace_ui.pending = Some(action);
            }
            return true;
        }
        self.workspace_picker_open()
    }

    pub(super) fn finish_workspace_action(&mut self) {
        if let Some(action) = self.workspace_ui.pending.take()
            && self.picker.is_none()
            && !self.settings_open
            && !self.agent_marks_modal_open()
            && self.save_as.is_none()
            && self.close_action.is_none()
            && self.context_file_dialog.is_none()
            && self.diff.is_none()
            && !self.git_detail_open()
            && !self.navigation_modal_open()
            && self.screen_jump.is_none()
            && self.vim_command.is_none()
            && !self.tab_drag_active()
        {
            self.apply_workspace_action(action);
        }
    }

    fn apply_workspace_action(&mut self, action: WorkspaceAction) {
        match action {
            WorkspaceAction::TogglePicker => {
                if self.workspace_picker_open() {
                    self.close_workspace_picker();
                } else {
                    self.open_workspace_picker();
                }
            }
            WorkspaceAction::Cycle { next } => {
                if !self.projects.is_empty() {
                    let count = self.projects.len();
                    let index = if next {
                        (self.active_project + 1) % count
                    } else {
                        (self.active_project + count - 1) % count
                    };
                    self.switch_project(index);
                }
                if self.workspace_picker_open() {
                    self.close_workspace_picker();
                }
            }
        }
    }

    fn workspace_discard_trailing_text(&self) {
        self.ctx.input_mut(|input| {
            input.events.retain(|event| {
                !matches!(
                    event,
                    egui::Event::Text(_) | egui::Event::Paste(_) | egui::Event::Ime(_)
                )
            });
        });
    }

    pub(super) fn workspace_header_button(&mut self, ui: &mut egui::Ui) {
        let Some(project) = self.project() else {
            if design::ghost_button(
                ui,
                "Add project",
                &format!(
                    "Open a project folder · {}",
                    self.shortcut_label(keybindings::Action::OpenProject)
                ),
            )
            .clicked()
            {
                self.pick_project();
            }
            return;
        };
        let names: Vec<_> = self
            .projects
            .iter()
            .map(|project| file_title(&project.root))
            .collect();
        let identities: Vec<_> = names
            .iter()
            .zip(&self.projects)
            .map(|(name, project)| (name.as_str(), Some(project.root.as_path())))
            .collect();
        let name =
            crate::file_identity::disambiguate_labels(&identities)[self.active_project].clone();
        let path = crate::paths::display(&project.root);
        let dirty = project.tabs.iter().filter(|tab| tab.dirty).count();
        let tooltip = format!(
            "{path}\n{} open files · {dirty} unsaved\nSwitch project: {}\nPrevious / next: {} / {}",
            project.tabs.len(),
            self.shortcut_label(keybindings::Action::SwitchProject),
            self.shortcut_label(keybindings::Action::PreviousProject),
            self.shortcut_label(keybindings::Action::NextProject),
        );
        let max_width = (ui.ctx().content_rect().width() - 530.0).clamp(100.0, 245.0);
        let width = (ui
            .fonts_mut(|fonts| fonts.layout_no_wrap(name.clone(), FontId::proportional(12.5), TEXT))
            .size()
            .x
            + 38.0)
            .clamp(100.0, max_width);
        let response = ui.add_sized(
            [width, 28.0],
            egui::Button::new(RichText::new(format!("{name}  ▾")).size(12.5).strong())
                .truncate()
                .frame_when_inactive(false)
                .corner_radius(5.0),
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), "Switch project")
        });
        let response = response
            .on_hover_text(tooltip)
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if response.clicked() {
            if self.workspace_picker_open() {
                self.close_workspace_picker();
            } else {
                self.open_workspace_picker();
            }
        }
        self.workspace_popup(ui, &response);
    }

    fn workspace_popup(&mut self, _ui: &mut egui::Ui, anchor: &egui::Response) {
        if !self.workspace_picker_open() {
            return;
        }
        let rows: Vec<_> = self
            .projects
            .iter()
            .enumerate()
            .map(|(index, project)| {
                let path = crate::paths::display(&project.root);
                ProjectRow {
                    root: project.root.clone(),
                    name: file_title(&project.root),
                    short_path: shortened_project_path(&path),
                    path,
                    open: project.tabs.len(),
                    dirty: project.tabs.iter().filter(|tab| tab.dirty).count(),
                    current: index == self.active_project,
                }
            })
            .collect();
        let ctx = self.ctx.clone();
        let mut state = std::mem::take(&mut self.workspace_ui);
        let mut chosen = None;
        let mut close = false;
        let mut add = false;
        let mut close_project = None;
        let mut keyboard_close = false;
        let width = (ctx.content_rect().width() - 30.0).clamp(300.0, 470.0);
        egui::Popup::new(picker_id(), ctx.clone(), anchor, anchor.layer_id)
            .open_memory(None)
            .width(width)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .layout(Layout::top_down_justified(Align::Min))
            .show(|ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Projects").size(14.0).strong());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(self.shortcut_label(keybindings::Action::SwitchProject))
                                .size(10.5)
                                .color(MUTED),
                        );
                    });
                });
                ui.add_space(6.0);
                // List navigation takes precedence over moving the search
                // field's caret. Apply it after filtering this frame's text.
                let down =
                    ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::ArrowDown));
                let up =
                    ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::ArrowUp));
                let first =
                    ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Home));
                let last =
                    ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::End));
                let enter =
                    ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Enter));
                let escape =
                    ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Escape));
                let query = ui.add(
                    egui::TextEdit::singleline(&mut state.query)
                        .id(query_id())
                        .hint_text("Find an open project…")
                        .desired_width(f32::INFINITY),
                );
                query.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::WidgetType::TextEdit,
                        ui.is_enabled(),
                        "Find project",
                    )
                });
                if state.focus && !ui.is_sizing_pass() {
                    query.request_focus();
                    state.focus = false;
                }
                let mut matching: Vec<_> = rows
                    .iter()
                    .filter_map(|row| {
                        project_match_score(&state.query, &row.name, &row.path)
                            .map(|score| (row, score))
                    })
                    .collect();
                if !state.query.trim().is_empty() {
                    matching.sort_by_key(|(_, score)| *score);
                }
                let visible: Vec<_> = matching.iter().map(|(row, _)| *row).collect();
                let mut selected = state
                    .selected_root
                    .as_ref()
                    .and_then(|root| visible.iter().position(|row| &row.root == root))
                    .unwrap_or(0);
                let mut moved = false;
                for (pressed, destination) in [
                    (down, selected.saturating_add(1)),
                    (up, selected.saturating_sub(1)),
                    (first, 0),
                    (last, visible.len().saturating_sub(1)),
                ] {
                    if pressed {
                        selected = destination.min(visible.len().saturating_sub(1));
                        moved = true;
                    }
                }
                state.selected_root = visible.get(selected).map(|row| row.root.clone());
                if enter {
                    chosen = state.selected_root.clone();
                    keyboard_close = chosen.is_some();
                }
                if escape {
                    close = true;
                    keyboard_close = true;
                }
                ui.add_space(6.0);
                let max_height = (ctx.content_rect().height() - 225.0).clamp(100.0, 390.0);
                if visible.is_empty() {
                    ui.add_space(14.0);
                    ui.label(RichText::new("No matching open project.").color(MUTED));
                    ui.add_space(14.0);
                } else {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    let mut scroll = egui::ScrollArea::vertical()
                        .id_salt("workspace_projects")
                        .max_height(max_height)
                        .auto_shrink([false, true]);
                    if moved {
                        scroll = scroll.vertical_scroll_offset(
                            (selected as f32 * PROJECT_ROW_HEIGHT - max_height * 0.5).max(0.0),
                        );
                    }
                    scroll.show_rows(ui, PROJECT_ROW_HEIGHT, visible.len(), |ui, range| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        for index in range {
                            let row = visible[index];
                            ui.push_id(&row.root, |ui| {
                                if project_row(ui, row, index == selected).clicked() {
                                    chosen = Some(row.root.clone());
                                }
                            });
                        }
                    });
                }
                ui.separator();
                ui.horizontal(|ui| {
                    add = design::ghost_button(
                        ui,
                        "Add project…",
                        &format!(
                            "Choose project folders with the Windows picker · {}",
                            self.shortcut_label(keybindings::Action::OpenProject)
                        ),
                    )
                    .clicked();
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.add_enabled_ui(state.selected_root.is_some(), |ui| {
                            if design::ghost_button(
                                ui,
                                "Close project",
                                "Close the selected project; unsaved changes are confirmed",
                            )
                            .clicked()
                            {
                                close_project = state.selected_root.clone();
                            }
                            if design::ghost_button(
                                ui,
                                "Copy path",
                                "Copy the selected project's full path",
                            )
                            .clicked()
                                && let Some(root) = &state.selected_root
                            {
                                ctx.copy_text(crate::paths::display(root));
                            }
                        });
                    });
                });
                ui.label(
                    RichText::new("↑ ↓ select · Enter switch · Esc close")
                        .size(10.5)
                        .color(MUTED),
                );
            });
        self.workspace_ui = state;
        if keyboard_close {
            self.workspace_discard_trailing_text();
        }
        if let Some(root) = chosen {
            if let Some(index) = self
                .projects
                .iter()
                .position(|project| project.root == root)
            {
                self.switch_project(index);
            }
            self.close_workspace_picker();
        } else if let Some(root) = close_project {
            self.close_workspace_picker();
            if let Some(index) = self
                .projects
                .iter()
                .position(|project| project.root == root)
            {
                self.request_close(CloseAction::Project(index));
            }
        } else if add {
            self.close_workspace_picker();
            self.pick_project();
        } else if close || !self.workspace_picker_open() {
            self.close_workspace_picker();
        }
    }

    pub(super) fn explorer_project_heading(&mut self, ui: &mut egui::Ui, root: &Path) {
        let title = file_title(root);
        let (rect, response) = ui.allocate_exact_size(
            Vec2::new(ui.available_width().max(1.0), 30.0),
            egui::Sense::hover(),
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Label, ui.is_enabled(), &title)
        });
        let galley = project_text(
            ui,
            &title,
            FontId::proportional(12.5),
            TEXT,
            rect.width() - 18.0,
        );
        ui.painter().galley(
            egui::pos2(rect.left() + 8.0, rect.center().y - galley.size().y * 0.5),
            galley,
            TEXT,
        );
        response.on_hover_text(crate::paths::display(root));
        self.explorer_background_menu(ui, root, rect);
        ui.add_space(3.0);
    }
}

fn shortened_project_path(path: &str) -> String {
    let components: Vec<_> = path
        .split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .collect();
    if components.len() <= 3 {
        path.to_owned()
    } else {
        format!("…\\{}", components[components.len() - 3..].join("\\"))
    }
}

fn project_match_score(query: &str, name: &str, path: &str) -> Option<usize> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Some(0);
    }
    let name = name.to_lowercase();
    if let Some(position) = name.find(&query) {
        return Some(position);
    }
    let path = path.to_lowercase();
    if let Some(position) = path.find(&query) {
        return Some(100 + position);
    }
    let mut offset = 0;
    let mut gaps = 0;
    for character in query.chars() {
        let position = path[offset..].find(character)?;
        gaps += position;
        offset += position + character.len_utf8();
    }
    Some(1000 + gaps)
}

fn project_text(
    ui: &egui::Ui,
    text: &str,
    font: FontId,
    color: Color32,
    width: f32,
) -> Arc<egui::Galley> {
    ui.fonts_mut(|fonts| {
        let mut job = egui::text::LayoutJob::simple(text.to_owned(), font, color, width.max(1.0));
        job.wrap.max_rows = 1;
        job.wrap.break_anywhere = true;
        fonts.layout_job(job)
    })
}

fn project_row(ui: &mut egui::Ui, row: &ProjectRow, selected: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width().max(1.0), PROJECT_ROW_HEIGHT),
        egui::Sense::click(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            ui.is_enabled(),
            selected,
            format!(
                "{} · {} open · {} unsaved · {}",
                row.name, row.open, row.dirty, row.short_path
            ),
        )
    });
    if ui.is_rect_visible(rect) {
        if selected || response.hovered() || response.has_focus() {
            let color = if selected {
                ui.visuals().selection.bg_fill
            } else {
                HOVER
            };
            ui.painter()
                .rect_filled(rect.shrink2(Vec2::new(0.0, 1.0)), 5.0, color);
        }
        let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(rect));
        let badge = if row.dirty > 0 {
            format!("{} unsaved", row.dirty)
        } else if row.current {
            "current".into()
        } else {
            String::new()
        };
        let badge_color = if row.dirty > 0 { ORANGE } else { ACCENT };
        let badge = project_text(ui, &badge, FontId::proportional(10.5), badge_color, 105.0);
        let title_width = rect.width() - 30.0 - badge.size().x;
        let title = project_text(ui, &row.name, FontId::proportional(13.0), TEXT, title_width);
        painter.galley(rect.left_top() + Vec2::new(12.0, 8.0), title, TEXT);
        painter.galley(
            egui::pos2(rect.right() - 12.0 - badge.size().x, rect.top() + 9.0),
            badge,
            badge_color,
        );
        let detail = format!("{} · {} open", row.short_path, row.open);
        let detail = project_text(
            ui,
            &detail,
            FontId::proportional(10.5),
            MUTED,
            rect.width() - 24.0,
        );
        painter.galley(rect.left_top() + Vec2::new(12.0, 29.0), detail, MUTED);
    }
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(format!(
            "{}\n{} open files · {} unsaved{}",
            row.path,
            row.open,
            row.dirty,
            if row.current { "\nCurrent project" } else { "" }
        ))
}

#[cfg(test)]
#[path = "workspace_ui_tests.rs"]
mod tests;

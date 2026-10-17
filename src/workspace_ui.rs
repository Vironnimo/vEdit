//! Fast project navigation without rebuilding or reloading open documents.
use super::*;

const PROJECT_ROW_HEIGHT: f32 = 52.0;
const PROJECT_TAB_HEIGHT: f32 = 28.0;
const PROJECT_TAB_GAP: f32 = 4.0;
const PROJECT_LIST_WIDTH: f32 = 28.0;

#[derive(Default)]
pub(super) struct WorkspaceUiState {
    query: String,
    selected_root: Option<PathBuf>,
    focus: bool,
    pending: Option<WorkspaceAction>,
    header_root: Option<PathBuf>,
    header_width: f32,
    header_count: usize,
    prefix_focus_policy: Option<egui::SurrenderFocusOn>,
}

#[derive(Clone, Copy)]
enum WorkspaceAction {
    TogglePicker,
    Cycle { next: bool },
    Select { index: usize },
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

struct ProjectHeaderTab {
    root: PathBuf,
    label: String,
    path: String,
    open: usize,
    dirty: usize,
    changes: Option<usize>,
    current: bool,
}

fn picker_id() -> egui::Id {
    egui::Id::new("workspace_picker")
}

fn query_id() -> egui::Id {
    egui::Id::new("workspace_picker_query")
}

impl VEditApp {
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
        if let Some(policy) = self.workspace_ui.prefix_focus_policy.take() {
            self.ctx.options_mut(|options| {
                options.input_options.surrender_focus_on = policy;
            });
        }
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
            WorkspaceAction::Select { index } => {
                self.switch_project(index);
                // The source was rendered before this deferred switch.
                self.ctx.request_repaint();
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
        if self.projects.is_empty() {
            self.workspace_ui.header_root = None;
            self.workspace_ui.header_count = 0;
            let tooltip = format!(
                "Open a project folder · {}",
                self.shortcut_label(keybindings::Action::OpenProject),
            );
            let response = if ui.available_width() < 110.0 {
                compact_project_action(ui, "+", "Add project", &tooltip)
            } else {
                design::ghost_button(ui, "Add project", &tooltip)
            };
            if response.clicked() {
                self.pick_project();
            }
            return;
        }
        let tooltip = format!(
            "Projects · {}\nAdd project folders · {}",
            self.shortcut_label(keybindings::Action::SwitchProject),
            self.shortcut_label(keybindings::Action::OpenProject),
        );
        if ui.available_width() < 80.0 {
            self.workspace_ui.header_width = 0.0;
            let response = project_list_button(ui, &tooltip);
            self.workspace_header_list(ui, &response);
            return;
        }
        let source_owner = self.tab().and_then(|tab| {
            let id = self.document_widget_id(tab.id);
            (tab.agent_view == crate::cli::DocumentView::Source
                && self
                    .ctx
                    .memory(|memory| memory.has_focus(id) || memory.had_focus_last_frame(id)))
            .then_some(id)
        });
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
        let labels = crate::file_identity::disambiguate_labels(&identities);
        let tabs: Vec<_> = self
            .projects
            .iter()
            .zip(labels)
            .enumerate()
            .map(|(index, (project, label))| ProjectHeaderTab {
                root: project.root.clone(),
                label,
                path: crate::paths::display(&project.root),
                open: project.tabs.len(),
                dirty: project.tabs.iter().filter(|tab| tab.dirty).count(),
                changes: project.uncommitted_change_count(),
                current: index == self.active_project,
            })
            .collect();
        let available_tabs = (ui.available_width() - PROJECT_LIST_WIDTH - PROJECT_TAB_GAP).max(1.0);
        let max_tab_width = available_tabs.min(220.0);
        let min_tab_width = 88.0_f32.min(max_tab_width);
        let widths: Vec<_> = tabs
            .iter()
            .map(|tab| {
                let label_width = ui
                    .fonts_mut(|fonts| {
                        fonts.layout_no_wrap(tab.label.clone(), FontId::proportional(12.5), TEXT)
                    })
                    .size()
                    .x;
                // Reserve the status-dot space even while clean, so a Git
                // refresh cannot shift another project's click target.
                (label_width + 36.0).clamp(min_tab_width, max_tab_width)
            })
            .collect();
        let content_width =
            widths.iter().sum::<f32>() + PROJECT_TAB_GAP * tabs.len().saturating_sub(1) as f32;
        let width = available_tabs.min(content_width);
        let current_root = self.project().map(|project| project.root.clone());
        let reveal = self.workspace_ui.header_root != current_root
            || (self.workspace_ui.header_width - width).abs() > 0.5
            || self.workspace_ui.header_count != tabs.len();
        self.workspace_ui.header_root = current_root;
        self.workspace_ui.header_width = width;
        self.workspace_ui.header_count = tabs.len();
        let mut chosen = None;
        let mut list = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = PROJECT_TAB_GAP;
            ui.allocate_ui_with_layout(
                Vec2::new(width, PROJECT_TAB_HEIGHT),
                Layout::left_to_right(Align::Center),
                |ui| {
                    let scroll_id = ui.make_persistent_id("workspace_header_tabs");
                    let mut scroll = egui::ScrollArea::horizontal()
                        .id_salt("workspace_header_tabs")
                        .max_width(width)
                        .auto_shrink([false, true])
                        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                        .scroll_source(egui::scroll_area::ScrollSource {
                            drag: egui::scroll_area::DragScroll::Never,
                            ..Default::default()
                        });
                    if reveal {
                        let mut offset = egui::scroll_area::State::load(ui.ctx(), scroll_id)
                            .map(|state| state.offset.x)
                            .unwrap_or(0.0);
                        let left = widths.iter().take(self.active_project).sum::<f32>()
                            + PROJECT_TAB_GAP * self.active_project as f32;
                        let right = left + widths[self.active_project.min(widths.len() - 1)];
                        if right > offset + width {
                            offset = right - width;
                        }
                        if left < offset {
                            offset = left;
                        }
                        scroll = scroll.horizontal_scroll_offset(
                            offset.clamp(0.0, (content_width - width).max(0.0)),
                        );
                    }
                    scroll.show(ui, |ui| {
                        ui.spacing_mut().item_spacing.x = PROJECT_TAB_GAP;
                        ui.horizontal(|ui| {
                            for (index, (tab, width)) in tabs.iter().zip(&widths).enumerate() {
                                ui.push_id(&tab.root, |ui| {
                                    let response = project_header_tab(ui, tab, *width);
                                    if response.clicked() {
                                        chosen = Some((index, response.rect));
                                    }
                                });
                            }
                        });
                    });
                },
            );
            let response = project_list_button(ui, &tooltip);
            list = Some(response);
        });
        if let Some((index, clicked_rect)) = chosen {
            // Source input earlier in this frame still belongs to the old
            // document. Switch only after its TextEdit and undo state render.
            self.workspace_ui.pending = Some(WorkspaceAction::Select { index });
            let prefix_boundary = self.ctx.input(|input| {
                input
                    .events
                    .iter()
                    .position(|event| {
                        matches!(event, egui::Event::PointerButton {
                        pos, button: egui::PointerButton::Primary, pressed: true, ..
                    } if clicked_rect.contains(*pos))
                    })
                    .filter(|boundary| {
                        input.events[..*boundary]
                            .iter()
                            .any(editor_input::native_input)
                    })
            });
            if let Some(boundary) = prefix_boundary
                && let Some(owner) = source_owner
            {
                // Events after the ownership boundary must not be interpreted
                // by either project's document, as with keyboard switching.
                self.ctx.input_mut(|input| input.events.truncate(boundary));
                // egui can clear the source's focus at the start of a pointer
                // frame, before this button is rendered. Restore the previous
                // source owner so TextEdit records its prefix in native undo.
                self.ctx.memory_mut(|memory| memory.request_focus(owner));
                // TextEdit also surrenders focus when it renders under an
                // outside click. Keep this prefix owner only until its source
                // has rendered; finish_workspace_action restores the policy.
                self.workspace_ui.prefix_focus_policy = Some(self.ctx.options_mut(|options| {
                    std::mem::replace(
                        &mut options.input_options.surrender_focus_on,
                        egui::SurrenderFocusOn::Never,
                    )
                }));
            }
        }
        let response = list.expect("project list button is always drawn");
        self.workspace_header_list(ui, &response);
    }

    fn workspace_header_list(&mut self, ui: &mut egui::Ui, response: &egui::Response) {
        if response.clicked() {
            if self.workspace_picker_open() {
                self.close_workspace_picker();
            } else {
                self.open_workspace_picker();
            }
        }
        self.workspace_popup(ui, response);
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
        let width = (ctx.content_rect().width() - 30.0).clamp(120.0, 470.0);
        let compact = width < 350.0;
        let short = ctx.content_rect().height() < 300.0;
        let mut popup = egui::Popup::new(picker_id(), ctx.clone(), anchor, anchor.layer_id)
            .open_memory(None)
            .width(width)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .layout(Layout::top_down_justified(Align::Min));
        if short {
            // Area constrains its position before rounding it to pixels, which
            // can put a fractional bottom edge back outside the viewport. Keep
            // the measured popup inside an inset before Area rounds it.
            let bounds = ctx
                .content_rect()
                .shrink(1.0 / ctx.pixels_per_point() + 0.1);
            let size = ctx
                .memory(|memory| memory.area_rect(picker_id()))
                .map_or(Vec2::new(width, 120.0), |rect| rect.size());
            let position = egui::pos2(
                anchor
                    .rect
                    .left()
                    .clamp(bounds.left(), (bounds.right() - size.x).max(bounds.left())),
                anchor
                    .rect
                    .bottom()
                    .clamp(bounds.top(), (bounds.bottom() - size.y).max(bounds.top())),
            );
            popup = popup.at_position(position).align_alternatives(&[]);
        }
        popup.show(|ui| {
            if !short {
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
            }
            // List navigation takes precedence over moving the search
            // field's caret. Apply it after filtering this frame's text.
            let down =
                ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::ArrowDown));
            let up = ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::ArrowUp));
            let first = ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Home));
            let last = ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::End));
            let enter = ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Enter));
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
            let opening = state.focus;
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
            let max_height = if short {
                // At 200% interface zoom the native 500×360 minimum is
                // 250×180 points. Keep search, one row and footer in view.
                (ctx.content_rect().height() - anchor.rect.bottom() - 90.0).clamp(26.0, 390.0)
            } else {
                (ctx.content_rect().height() - 225.0).clamp(100.0, 390.0)
            };
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
                // Reveal the current project when opening, including when
                // its row is below the viewport. Filtering must also reveal
                // the selection after a previously scrolled list shrinks.
                if moved || opening || query.changed() {
                    // Clamp before painting, rather than relying on the
                    // ScrollArea's end-of-frame correction. An overshoot
                    // near the last rows otherwise moves their hit targets
                    // between the first pointer press and release.
                    let content_height = visible.len() as f32 * PROJECT_ROW_HEIGHT;
                    let offset = (selected as f32 * PROJECT_ROW_HEIGHT + PROJECT_ROW_HEIGHT * 0.5
                        - max_height * 0.5)
                        .clamp(0.0, (content_height - max_height).max(0.0));
                    scroll = scroll.vertical_scroll_offset(offset);
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
                let tooltip = format!(
                    "Choose project folders with the Windows picker · {}",
                    self.shortcut_label(keybindings::Action::OpenProject),
                );
                add = if compact {
                    compact_project_action(ui, "+", "Add project…", &tooltip)
                } else {
                    design::ghost_button(ui, "Add project…", &tooltip)
                }
                .clicked();
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_enabled_ui(state.selected_root.is_some(), |ui| {
                        let tooltip = "Close the selected project; unsaved changes are confirmed";
                        let close = if compact {
                            compact_project_action(ui, "×", "Close project", tooltip)
                        } else {
                            design::ghost_button(ui, "Close project", tooltip)
                        };
                        if close.clicked() {
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
            if !short {
                ui.label(
                    RichText::new("↑ ↓ select · Enter switch · Esc close")
                        .size(10.5)
                        .color(MUTED),
                );
            }
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
    let query = query.trim().replace('\\', "/").to_lowercase();
    if query.is_empty() {
        return Some(0);
    }
    let name = name.to_lowercase();
    if let Some(position) = name.find(&query) {
        return Some(position);
    }
    let path = path.replace('\\', "/").to_lowercase();
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

fn project_header_tab(ui: &mut egui::Ui, tab: &ProjectHeaderTab, width: f32) -> egui::Response {
    let response = ui.add_sized(
        Vec2::new(width, PROJECT_TAB_HEIGHT),
        egui::Button::new("").frame(false),
    );
    let rect = response.rect;
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            ui.is_enabled(),
            tab.current,
            format!("Project: {}", tab.label),
        )
    });
    let changes = tab.changes.filter(|count| *count > 0);
    let git_description = changes.map(|count| {
        format!(
            "{count} uncommitted {}",
            if count == 1 { "change" } else { "changes" },
        )
    });
    if ui.is_rect_visible(rect) {
        let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(rect));
        if tab.current || response.hovered() || crate::design::keyboard_focus(&response) {
            painter.rect_filled(rect, 5.0, if response.hovered() { HOVER } else { SURFACE });
        }
        if tab.current {
            painter.line_segment(
                [
                    rect.left_bottom() + Vec2::new(8.0, -0.5),
                    rect.right_bottom() + Vec2::new(-8.0, -0.5),
                ],
                Stroke::new(1.5, ACCENT.gamma_multiply(0.85)),
            );
        }
        if crate::design::keyboard_focus(&response) {
            painter.rect_stroke(
                rect.shrink(1.0),
                4.0,
                ui.visuals().selection.stroke,
                egui::StrokeKind::Inside,
            );
        }
        let color = if tab.current { TEXT } else { MUTED };
        let font = FontId::proportional(12.5);
        let title_width = (rect.width() - 32.0).max(1.0);
        let shortened = ui
            .fonts_mut(|fonts| fonts.layout_no_wrap(tab.label.clone(), font.clone(), color))
            .size()
            .x
            > title_width;
        let (title, suffix) = if let Some((name, context)) = tab.label.rsplit_once(" — ")
            && shortened
        {
            // Keep the distinguishing parent visible even when a shared long
            // project basename is shortened. The full identity stays accessible.
            let suffix = design::truncated_galley(
                ui,
                &format!(" — {context}"),
                font.clone(),
                MUTED,
                title_width * 0.65,
            );
            (
                design::truncated_galley(ui, name, font, color, title_width - suffix.size().x),
                Some(suffix),
            )
        } else {
            (
                design::truncated_galley(ui, &tab.label, font, color, title_width),
                None,
            )
        };
        let title_right = rect.left() + 10.0 + title.size().x;
        painter.galley(
            rect.left_center() + Vec2::new(10.0, -title.size().y * 0.5),
            title,
            color,
        );
        if let Some(suffix) = suffix {
            painter.galley(
                egui::pos2(title_right, rect.center().y - suffix.size().y * 0.5),
                suffix,
                MUTED,
            );
        }
        if let Some(description) = &git_description {
            let center = egui::pos2(rect.right() - 11.0, rect.center().y);
            painter.circle_filled(center, 2.8, ORANGE.gamma_multiply(0.85));
            let status = ui.interact(
                egui::Rect::from_center_size(center, Vec2::new(12.0, 18.0)),
                response.id.with("git_changes"),
                egui::Sense::hover(),
            );
            status.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::Label,
                    ui.is_enabled(),
                    format!("{description} in {}", tab.label),
                )
            });
            status.on_hover_text(description);
        }
    }
    let mut tooltip = format!(
        "{}\n{} open files · {} unsaved",
        tab.path, tab.open, tab.dirty,
    );
    if let Some(description) = git_description {
        tooltip.push('\n');
        tooltip.push_str(&description);
    }
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(tooltip)
}

fn compact_project_action(
    ui: &mut egui::Ui,
    glyph: &str,
    label: &str,
    tooltip: &str,
) -> egui::Response {
    let response = design::ghost_button(ui, glyph, tooltip);
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    response
}

fn project_list_button(ui: &mut egui::Ui, tooltip: &str) -> egui::Response {
    let response = ui.add_sized(
        Vec2::new(PROJECT_LIST_WIDTH, PROJECT_TAB_HEIGHT),
        egui::Button::new("").frame(false),
    );
    let rect = response.rect;
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), "Switch project")
    });
    if response.hovered() || crate::design::keyboard_focus(&response) {
        ui.painter().rect_filled(rect, 5.0, HOVER);
    }
    if crate::design::keyboard_focus(&response) {
        ui.painter().rect_stroke(
            rect.shrink(1.0),
            4.0,
            ui.visuals().selection.stroke,
            egui::StrokeKind::Inside,
        );
    }
    let color = if response.hovered() { TEXT } else { MUTED };
    let center = rect.center() + Vec2::new(-3.0, 0.0);
    let stroke = Stroke::new(1.25, color);
    ui.painter().line_segment(
        [center + Vec2::new(-3.5, 0.0), center + Vec2::new(3.5, 0.0)],
        stroke,
    );
    ui.painter().line_segment(
        [center + Vec2::new(0.0, -3.5), center + Vec2::new(0.0, 3.5)],
        stroke,
    );
    let arrow = rect.center() + Vec2::new(7.0, 0.0);
    ui.painter().add(egui::Shape::line(
        vec![
            arrow + Vec2::new(-2.0, -1.0),
            arrow + Vec2::new(0.0, 1.0),
            arrow + Vec2::new(2.0, -1.0),
        ],
        stroke,
    ));
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(tooltip)
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
        if selected || response.hovered() || crate::design::keyboard_focus(&response) {
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

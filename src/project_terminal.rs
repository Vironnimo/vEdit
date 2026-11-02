//! Project-owned terminal sessions, independent of their panel or Pane placement.
use super::*;

static NEXT_TERMINAL: AtomicU64 = AtomicU64::new(1 << 63);

#[derive(Default)]
pub(super) struct ProjectTerminals {
    entries: Vec<ProjectTerminal>,
    selected: Option<u64>,
    pub(super) show_panel: bool,
}

struct ProjectTerminal {
    id: u64,
    title: String,
    session: Option<crate::terminal::TerminalSession>,
    error: Option<String>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct SessionTerminals {
    entries: Vec<SessionTerminal>,
    selected: Option<u64>,
    show_panel: bool,
}

#[derive(Clone, Serialize, Deserialize)]
struct SessionTerminal {
    id: u64,
    title: String,
}

impl ProjectTerminals {
    pub(super) fn session(&self) -> SessionTerminals {
        SessionTerminals {
            entries: self
                .entries
                .iter()
                .map(|entry| SessionTerminal {
                    id: entry.id,
                    title: entry.title.clone(),
                })
                .collect(),
            selected: self.selected,
            show_panel: self.show_panel,
        }
    }
}

impl VEditApp {
    pub(super) fn restore_terminals(&self, saved: SessionTerminals) -> ProjectTerminals {
        let mut seen = std::collections::HashSet::new();
        let entries: Vec<_> = saved
            .entries
            .into_iter()
            .filter(|entry| {
                entry.id >= 1 << 63
                    && entry.id < u64::MAX
                    && seen.insert(entry.id)
                    && !self.terminal_exists(entry.id)
            })
            .map(|entry| {
                NEXT_TERMINAL.fetch_max(entry.id + 1, Ordering::Relaxed);
                ProjectTerminal {
                    id: entry.id,
                    title: entry.title,
                    session: None,
                    error: None,
                }
            })
            .collect();
        let selected = saved
            .selected
            .filter(|id| entries.iter().any(|entry| entry.id == *id))
            .or_else(|| entries.first().map(|entry| entry.id));
        ProjectTerminals {
            entries,
            selected,
            show_panel: saved.show_panel,
        }
    }

    pub(super) fn terminal_exists(&self, id: u64) -> bool {
        self.projects
            .iter()
            .any(|project| project.terminals.entries.iter().any(|entry| entry.id == id))
    }

    pub(super) fn terminal_belongs_to_project(&self, id: u64, project: usize) -> bool {
        self.projects
            .get(project)
            .is_some_and(|project| project.terminals.entries.iter().any(|entry| entry.id == id))
    }

    pub(super) fn terminal_title(&self, id: u64) -> String {
        self.projects
            .iter()
            .flat_map(|project| &project.terminals.entries)
            .find(|entry| entry.id == id)
            .map_or_else(|| "Terminal".into(), |entry| entry.title.clone())
    }

    pub(super) fn terminal_widget_id(id: u64) -> egui::Id {
        egui::Id::new(("project_terminal", id))
    }

    pub(super) fn panel_terminal_id(&self) -> Option<u64> {
        if !self.output_visible || self.problems_visible {
            return None;
        }
        let project = self.project()?;
        if !project.terminals.show_panel || project.launch.show_output {
            return None;
        }
        project
            .terminals
            .selected
            .filter(|id| self.pane_terminal(*id).is_none())
            .or_else(|| {
                project
                    .terminals
                    .entries
                    .iter()
                    .find(|entry| self.pane_terminal(entry.id).is_none())
                    .map(|entry| entry.id)
            })
    }

    pub(super) fn terminal_keyboard_focused(&self) -> bool {
        !self.chrome_blocked() && self.focused_terminal_id().is_some()
    }

    pub(super) fn focused_terminal_id(&self) -> Option<u64> {
        self.project()?
            .terminals
            .entries
            .iter()
            .find(|entry| {
                self.terminal_visible(entry.id)
                    && self
                        .ctx
                        .memory(|memory| memory.has_focus(Self::terminal_widget_id(entry.id)))
            })
            .map(|entry| entry.id)
    }

    pub(super) fn terminal_input_active(&self) -> bool {
        self.terminal_keyboard_focused()
            || (!self.chrome_blocked()
                && self
                    .terminal_focus
                    .is_some_and(|id| self.terminal_visible(id)))
    }

    pub(super) fn terminal_visible(&self, id: u64) -> bool {
        self.panel_terminal_id() == Some(id)
            || (!self.change_review_visible() && self.active_pane_terminal() == Some(id))
    }

    pub(super) fn new_terminal(&mut self) {
        let Some(project) = self
            .projects
            .get_mut(self.active_project)
            .filter(|project| !project.is_other_files())
        else {
            return;
        };
        let id = NEXT_TERMINAL.fetch_add(1, Ordering::Relaxed);
        let number = project
            .terminals
            .entries
            .iter()
            .filter_map(|entry| entry.title.strip_prefix("Terminal ")?.parse::<u64>().ok())
            .max()
            .unwrap_or(0)
            + 1;
        project.terminals.entries.push(ProjectTerminal {
            id,
            title: format!("Terminal {number}"),
            session: None,
            error: None,
        });
        self.focus_terminal(id);
        self.session_dirty = true;
    }

    pub(super) fn open_terminal(&mut self) {
        let id = self.project().and_then(|project| {
            project
                .terminals
                .selected
                .or_else(|| project.terminals.entries.first().map(|entry| entry.id))
        });
        let Some(id) = id else {
            self.new_terminal();
            return;
        };
        if self.panel_terminal_id() == Some(id)
            && self
                .ctx
                .memory(|memory| memory.has_focus(Self::terminal_widget_id(id)))
        {
            self.output_visible = false;
            self.terminal_focus = None;
            self.focus_editor();
            self.session_dirty = true;
        } else {
            self.focus_terminal(id);
        }
    }

    pub(super) fn focus_terminal(&mut self, id: u64) {
        let Some(project_index) = self
            .projects
            .iter()
            .position(|project| project.terminals.entries.iter().any(|entry| entry.id == id))
        else {
            return;
        };
        if let Some(tab) = self.tab_mut() {
            tab.focus = false;
        }
        if project_index != self.active_project {
            self.switch_project(project_index);
        }
        self.projects[project_index].terminals.selected = Some(id);
        if self.pane_terminal(id).is_some() {
            self.park_change_review(project_index);
            let _ = self.focus_pane_terminal(id);
        } else {
            self.problems_visible = false;
            self.projects[project_index].launch.show_output = false;
            self.projects[project_index].terminals.show_panel = true;
            self.output_visible = true;
        }
        self.terminal_focus = Some(id);
        self.ctx
            .memory_mut(|memory| memory.request_focus(Self::terminal_widget_id(id)));
        self.editor_focused = false;
        self.completions = None;
        self.session_dirty = true;
        self.ctx.request_repaint();
    }

    pub(super) fn close_terminal(&mut self, id: u64) {
        let focused = self
            .ctx
            .memory(|memory| memory.has_focus(Self::terminal_widget_id(id)))
            || self.terminal_focus == Some(id);
        self.undock_terminal(id);
        for project in &mut self.projects {
            let terminals = &mut project.terminals;
            terminals.entries.retain(|entry| entry.id != id);
            if terminals.selected == Some(id) {
                terminals.selected = terminals.entries.last().map(|entry| entry.id);
            }
        }
        self.ctx
            .memory_mut(|memory| memory.surrender_focus(Self::terminal_widget_id(id)));
        if self.terminal_focus == Some(id) {
            self.terminal_focus = None;
        }
        if focused {
            if let Some(next) = self
                .project()
                .and_then(|project| project.terminals.selected)
            {
                self.focus_terminal(next);
            } else {
                self.focus_editor();
            }
        }
        self.session_dirty = true;
    }

    pub(super) fn select_task_output(&mut self) {
        if let Some(project) = self.projects.get_mut(self.active_project) {
            project.terminals.show_panel = false;
            project.launch.show_output = false;
        }
        self.problems_visible = false;
    }

    pub(super) fn cycle_panel_terminal(&mut self, previous: bool) {
        let ids: Vec<_> = self
            .project()
            .into_iter()
            .flat_map(|project| &project.terminals.entries)
            .filter(|entry| self.pane_terminal(entry.id).is_none())
            .map(|entry| entry.id)
            .collect();
        if ids.is_empty() {
            return;
        }
        let active = self
            .panel_terminal_id()
            .and_then(|id| ids.iter().position(|candidate| *candidate == id))
            .unwrap_or(0);
        let next = if previous {
            (active + ids.len() - 1) % ids.len()
        } else {
            (active + 1) % ids.len()
        };
        self.focus_terminal(ids[next]);
    }

    pub(super) fn render_terminal(&mut self, ui: &mut egui::Ui, id: u64) {
        let Some(project_index) = self
            .projects
            .iter()
            .position(|project| project.terminals.entries.iter().any(|entry| entry.id == id))
        else {
            return;
        };
        let root = self.projects[project_index].root.clone();
        let history = self
            .recovery_session_path
            .parent()
            .unwrap_or(Path::new("."))
            .join("terminal-history");
        #[cfg(not(test))]
        let configuration = self.projects[project_index]
            .terminals
            .entries
            .iter()
            .find(|entry| entry.id == id)
            .filter(|entry| entry.session.is_none() && entry.error.is_none())
            .and_then(|_| self.project_task_configuration_for(project_index));
        let blocked = self.chrome_blocked()
            || self.navigation_pending()
            || self.screen_jump.is_some()
            || self.vim_command.is_some();
        let focus = !blocked && self.terminal_focus == Some(id);
        let widget = Self::terminal_widget_id(id);
        let entry = self.projects[project_index]
            .terminals
            .entries
            .iter_mut()
            .find(|entry| entry.id == id)
            .unwrap();
        if entry.session.is_none() && entry.error.is_none() {
            #[cfg(not(test))]
            match crate::terminal::TerminalSession::spawn(
                &root,
                &history,
                {
                    let environment_root = root.clone();
                    let configuration = configuration.unwrap();
                    move || configuration.environment(&environment_root, None)
                },
                self.ctx.clone(),
            ) {
                Ok(session) => entry.session = Some(session),
                Err(error) => entry.error = Some(error),
            }
            #[cfg(test)]
            {
                let _ = (&root, &history);
                entry.session = Some(crate::terminal::TerminalSession::in_memory());
            }
        }
        if let Some(error) = entry.error.clone() {
            ui.colored_label(RED, error);
            if ui.button("Retry terminal").clicked() {
                entry.error = None;
            }
            return;
        }
        let session = entry.session.as_mut().unwrap();
        let running = session.is_running();
        let status = session.status();
        let title = session.title();
        let restart = ui
            .horizontal(|ui| {
                ui.label(RichText::new(title).size(11.0).color(MUTED));
                ui.label(RichText::new(status).size(11.0).color(if running {
                    MUTED
                } else {
                    ORANGE
                }));
                !running && ui.small_button("Restart").clicked()
            })
            .inner;
        if restart {
            entry.session = None;
            self.terminal_focus = Some(id);
            self.ctx.request_repaint();
            return;
        }
        let response = ui
            .add_enabled_ui(!blocked, |ui| {
                entry
                    .session
                    .as_mut()
                    .unwrap()
                    .show(ui, widget, focus, self.settings.font_size)
            })
            .inner;
        if focus {
            self.terminal_focus = None;
        }
        self.register_region_focus_widget(widget);
        if response.has_focus() {
            self.editor_focused = false;
            if let Some(tab) = self.tab_mut() {
                tab.focus = false;
            }
            self.projects[project_index].terminals.selected = Some(id);
        }
    }

    pub(super) fn terminal_panel_tab(&mut self, ui: &mut egui::Ui) {
        let active = self.panel_terminal_id().is_some();
        let response = design::ghost_button(
            ui,
            "Terminal",
            &format!(
                "Project terminals · {}",
                self.shortcut_label(keybindings::Action::Terminal)
            ),
        );
        if active {
            ui.painter().line_segment(
                [response.rect.left_bottom(), response.rect.right_bottom()],
                Stroke::new(1.5, ACCENT),
            );
        }
        if response.clicked() {
            let bottom = self.project().and_then(|project| {
                project
                    .terminals
                    .entries
                    .iter()
                    .find(|entry| self.pane_terminal(entry.id).is_none())
                    .map(|entry| entry.id)
            });
            if let Some(id) = bottom {
                self.focus_terminal(id);
            } else {
                self.new_terminal();
            }
        }
    }

    pub(super) fn terminal_panel_ui(&mut self, ui: &mut egui::Ui) -> bool {
        let Some(id) = self.panel_terminal_id() else {
            return false;
        };
        let entries: Vec<_> = self
            .project()
            .unwrap()
            .terminals
            .entries
            .iter()
            .filter(|entry| self.pane_terminal(entry.id).is_none())
            .map(|entry| (entry.id, entry.title.clone()))
            .collect();
        let mut focus = None;
        let mut close = None;
        ui.horizontal_wrapped(|ui| {
            for (terminal, title) in entries {
                let response = ui.add(
                    egui::Button::new(&title)
                        .selected(terminal == id)
                        .sense(egui::Sense::click_and_drag()),
                );
                self.start_terminal_tab_drag(&response, terminal, &title);
                response.context_menu(|ui| {
                    if ui.button("Move to editor").clicked() {
                        let pane = self.current_pane_id();
                        let _ = self.dock_terminal_in_pane(terminal, pane, usize::MAX);
                        focus = Some(terminal);
                        ui.close();
                    }
                    if ui.button("Close terminal").clicked() {
                        close = Some(terminal);
                        ui.close();
                    }
                });
                if response.clicked() {
                    focus = Some(terminal);
                }
                if response.clicked_by(egui::PointerButton::Middle) {
                    close = Some(terminal);
                }
            }
            if ui.button("+").on_hover_text("New terminal").clicked() {
                self.new_terminal();
            }
            if ui
                .button("×")
                .on_hover_text("Close terminal and its processes")
                .clicked()
            {
                close = Some(id);
            }
        });
        if let Some(close) = close {
            self.close_terminal(close);
        }
        if let Some(focus) = focus {
            self.focus_terminal(focus);
        }
        if self.terminal_exists(id) && self.pane_terminal(id).is_none() {
            self.render_terminal(ui, id);
        }
        true
    }

    #[cfg(test)]
    pub(super) fn add_test_terminal(&mut self) -> u64 {
        let id = NEXT_TERMINAL.fetch_add(1, Ordering::Relaxed);
        self.projects[self.active_project]
            .terminals
            .entries
            .push(ProjectTerminal {
                id,
                title: "Terminal 1".into(),
                session: Some(crate::terminal::TerminalSession::in_memory()),
                error: None,
            });
        self.focus_terminal(id);
        id
    }

    #[cfg(test)]
    pub(super) fn terminal_input_bytes(&mut self, id: u64) -> Vec<u8> {
        self.projects
            .iter_mut()
            .flat_map(|project| &mut project.terminals.entries)
            .find(|entry| entry.id == id)
            .and_then(|entry| entry.session.as_mut())
            .unwrap()
            .take_input_bytes()
    }

    #[cfg(test)]
    pub(super) fn feed_test_terminal_output(&mut self, id: u64, bytes: &[u8]) {
        self.projects
            .iter_mut()
            .flat_map(|project| &mut project.terminals.entries)
            .find(|entry| entry.id == id)
            .and_then(|entry| entry.session.as_mut())
            .unwrap()
            .feed_output(bytes);
    }
}

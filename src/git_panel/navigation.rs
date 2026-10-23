//! Keyboard traversal and popup/overlay focus within the Git panel.

use super::*;

impl GitViewState {
    pub(super) fn update_keyboard_rows(
        &mut self,
        root: &Path,
        repository: bool,
        history_generation: u64,
        commits: &[git::CommitSummary],
    ) -> bool {
        let key = (
            root.to_path_buf(),
            repository,
            self.section,
            history_generation,
            commits.len(),
        );
        if self.keyboard_rows_key.as_ref() == Some(&key) && self.keyboard_expanded == self.expanded
        {
            return false;
        }
        self.keyboard_rows.clear();
        if repository {
            match self.section {
                GitSection::Changes => {
                    if let Some(cache) = &self.working_cache {
                        for (group, tree) in &cache.groups {
                            if tree.summary.total == 0 {
                                continue;
                            }
                            let key = format!("working:{}", group.key());
                            self.keyboard_rows.push(git_row_id(root, &key));
                            if self.expanded.get(&key).copied().unwrap_or(true) {
                                tree_keyboard_rows(
                                    tree,
                                    "",
                                    &key,
                                    root,
                                    &self.expanded,
                                    &mut self.keyboard_rows,
                                );
                            }
                        }
                    }
                }
                GitSection::History => self
                    .keyboard_rows
                    .extend(commits.iter().map(|commit| git_row_id(root, &commit.hash))),
            }
        } else {
            self.keyboard_header = None;
        }
        self.keyboard_expanded.clone_from(&self.expanded);
        self.keyboard_rows_key = Some(key);
        true
    }
}

pub(super) fn git_row_id(root: &Path, key: &str) -> egui::Id {
    egui::Id::new(("git-content", root, key))
}

pub(super) fn tree_keyboard_rows(
    tree: &FileTree,
    prefix: &str,
    namespace: &str,
    root: &Path,
    expanded: &HashMap<String, bool>,
    rows: &mut Vec<egui::Id>,
) {
    for (name, folder) in &tree.folders {
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        let key = format!("{namespace}:folder:{path}");
        rows.push(git_row_id(root, &key));
        if expanded.get(&key).copied().unwrap_or(true) {
            tree_keyboard_rows(folder, &path, namespace, root, expanded, rows);
        }
    }
    rows.extend(
        tree.files
            .iter()
            .map(|file| git_row_id(root, &format!("{namespace}:file:{}", file.path))),
    );
}

fn git_keyboard_target(rows: &[egui::Id], focused: egui::Id, key: Key) -> Option<egui::Id> {
    let index = rows
        .iter()
        .position(|id| *id == focused || id.with("action") == focused)?;
    let index = match key {
        Key::ArrowUp => index.saturating_sub(1),
        Key::ArrowDown => (index + 1).min(rows.len().saturating_sub(1)),
        Key::Home => 0,
        Key::End => rows.len().saturating_sub(1),
        _ => return None,
    };
    rows.get(index).copied()
}

#[derive(Clone, Copy)]
enum GitMovement {
    Up(usize),
    Down(usize),
    First,
    Last,
}

fn git_movement(
    key: Key,
    modifiers: egui::Modifiers,
    page: usize,
    swap_jk: bool,
) -> Option<GitMovement> {
    let page = page.max(1);
    if modifiers == egui::Modifiers::CTRL {
        return match key {
            Key::U => Some(GitMovement::Up((page / 2).max(1))),
            Key::D => Some(GitMovement::Down((page / 2).max(1))),
            _ => None,
        };
    }
    if modifiers != egui::Modifiers::NONE {
        return None;
    }
    match key {
        Key::ArrowUp => Some(GitMovement::Up(1)),
        Key::ArrowDown => Some(GitMovement::Down(1)),
        Key::J if swap_jk => Some(GitMovement::Up(1)),
        Key::K if swap_jk => Some(GitMovement::Down(1)),
        Key::K => Some(GitMovement::Up(1)),
        Key::J => Some(GitMovement::Down(1)),
        Key::PageUp => Some(GitMovement::Up(page)),
        Key::PageDown => Some(GitMovement::Down(page)),
        Key::Home => Some(GitMovement::First),
        Key::End => Some(GitMovement::Last),
        _ => None,
    }
}

/// Consume navigation in event order, including repeated keys within one frame.
/// Only rows own these motions; composer and popup text input stays untouched.
pub(super) fn take_git_navigation(
    ctx: &egui::Context,
    rows: &[egui::Id],
    focused: egui::Id,
    page: usize,
    swap_jk: bool,
) -> Option<egui::Id> {
    let mut index = rows
        .iter()
        .position(|id| *id == focused || id.with("action") == focused)?;
    let mut moved = false;
    ctx.input_mut(|input| {
        let keyed_letters: Vec<_> = input
            .events
            .iter()
            .filter_map(|event| match event {
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } if matches!(key, Key::J | Key::K) && *modifiers == egui::Modifiers::NONE => {
                    Some(*key)
                }
                _ => None,
            })
            .collect();
        let mut apply = |movement| {
            index = match movement {
                GitMovement::Up(count) => index.saturating_sub(count),
                GitMovement::Down(count) => index.saturating_add(count).min(rows.len() - 1),
                GitMovement::First => 0,
                GitMovement::Last => rows.len() - 1,
            };
            moved = true;
        };
        input.events.retain(|event| match event {
            egui::Event::Key {
                key,
                pressed,
                modifiers,
                ..
            } => {
                if let Some(movement) = git_movement(*key, *modifiers, page, swap_jk) {
                    if *pressed {
                        apply(movement);
                    }
                    false
                } else {
                    true
                }
            }
            egui::Event::Text(text)
                if text.chars().all(|character| matches!(character, 'j' | 'k')) =>
            {
                for character in text.chars() {
                    let key = if character == 'j' { Key::J } else { Key::K };
                    if !keyed_letters.contains(&key) {
                        apply(git_movement(key, egui::Modifiers::NONE, page, swap_jk).unwrap());
                    }
                }
                false
            }
            _ => true,
        });
    });
    if moved {
        ctx.memory_mut(|memory| memory.move_focus(egui::FocusDirection::None));
        Some(rows[index])
    } else {
        None
    }
}

pub(super) fn request_git_row_focus(ctx: &egui::Context, id: egui::Id) {
    ctx.memory_mut(|memory| memory.request_focus(id));
    ctx.data_mut(|data| data.insert_temp(id.with("reveal"), true));
}

pub(super) fn git_row_keyboard(ui: &egui::Ui, response: &egui::Response) {
    if response.has_focus() {
        ui.memory_mut(|memory| {
            memory.set_focus_lock_filter(
                response.id,
                egui::EventFilter {
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    ..Default::default()
                },
            )
        });
        if response.gained_focus()
            || ui
                .data_mut(|data| data.remove_temp::<bool>(response.id.with("reveal")))
                .unwrap_or(false)
        {
            response.scroll_to_me(Some(Align::Center));
        }
        // Same quiet focus ring as the Explorer's selected row.
        if crate::design::focus_visible(ui.ctx()) {
            ui.painter().rect_stroke(
                response.rect.shrink(1.0),
                4.0,
                Stroke::new(1.0, ACCENT.gamma_multiply(0.7)),
                egui::StrokeKind::Inside,
            );
        }
    }
}

pub(super) fn git_folder_key(ui: &egui::Ui, row: &egui::Response, open: &mut bool) {
    if row.has_focus() && !egui::Popup::is_any_open(ui.ctx()) {
        ui.input_mut(|input| {
            input.events.retain(|event| match event {
                egui::Event::Key {
                    key,
                    pressed,
                    modifiers,
                    ..
                } if *modifiers == egui::Modifiers::NONE
                    && matches!(key, Key::ArrowLeft | Key::ArrowRight | Key::H | Key::L) =>
                {
                    if *pressed {
                        *open = matches!(key, Key::ArrowRight | Key::L);
                    }
                    false
                }
                egui::Event::Text(text)
                    if text.chars().all(|character| matches!(character, 'h' | 'l')) =>
                {
                    for character in text.chars() {
                        *open = character == 'l';
                    }
                    false
                }
                _ => true,
            });
        });
    }
}

pub(super) fn git_popup_focus(response: &egui::Response, popup_id: egui::Id) {
    let ctx = &response.ctx;
    let remembered = popup_id.with("git-was-open");
    if egui::Popup::is_id_open(ctx, popup_id) {
        ctx.data_mut(|data| data.insert_temp(remembered, true));
    } else if ctx
        .data_mut(|data| data.remove_temp::<bool>(remembered))
        .unwrap_or(false)
    {
        let escape = ctx.input(|input| input.key_pressed(Key::Escape));
        if escape {
            ctx.data_mut(|data| data.insert_temp(egui::Id::new("git-popup-return"), response.id));
        }
        if escape
            || ctx.input(|input| input.key_pressed(Key::Enter) || input.key_pressed(Key::Space))
        {
            request_git_row_focus(ctx, response.id);
        }
    }
}

pub(super) fn take_git_popup_return(ctx: &egui::Context) -> Option<egui::Id> {
    ctx.data_mut(|data| {
        let key = egui::Id::new("git-popup-return");
        let id = data.get_temp::<egui::Id>(key);
        data.remove::<egui::Id>(key);
        id
    })
}

pub(super) fn benchmark_previous_git_frame(ctx: &egui::Context) -> bool {
    #[cfg(test)]
    {
        ctx.data(|data| {
            data.get_temp::<bool>(egui::Id::new("git-panel-benchmark-uncached"))
                .unwrap_or(false)
        })
    }
    #[cfg(not(test))]
    {
        let _ = ctx;
        false
    }
}

pub(super) fn git_context_menu(
    response: &egui::Response,
    bindings: &keybindings::Bindings,
    contents: impl FnOnce(&mut egui::Ui, bool),
) {
    let keyboard = response.has_focus()
        && keybindings::consume(bindings, keybindings::Action::ContextMenu, &response.ctx);
    let id = response.id.with("git-context-menu");
    // Closed menus have no UI to build. Keep the previous-open marker in this
    // check so Escape still restores row focus on the frame after closing.
    if !benchmark_previous_git_frame(&response.ctx)
        && !keyboard
        && !response.secondary_clicked()
        && !egui::Popup::is_id_open(&response.ctx, id)
        && !response.ctx.data(|data| {
            data.get_temp::<bool>(id.with("git-was-open"))
                .unwrap_or(false)
        })
    {
        return;
    }
    if keyboard {
        egui::Popup::open_id(&response.ctx, id);
        response.ctx.data_mut(|data| {
            data.insert_temp(id.with("keyboard-position"), response.rect.left_bottom())
        });
        response
            .ctx
            .data_mut(|data| data.insert_temp(id.with("focus-first"), true));
    } else if response.secondary_clicked() {
        response
            .ctx
            .data_mut(|data| data.remove::<egui::Pos2>(id.with("keyboard-position")));
    }
    let mut popup = egui::Popup::context_menu(response).id(id);
    if let Some(position) = response
        .ctx
        .data(|data| data.get_temp::<egui::Pos2>(id.with("keyboard-position")))
    {
        popup = popup.at_position(position);
    }
    popup.show(|ui| {
        let items_id = id.with("git-menu-items");
        let items = ui
            .data(|data| data.get_temp::<Vec<egui::Id>>(items_id))
            .unwrap_or_default();
        if let Some(focused) = ui.memory(|memory| memory.focused()) {
            for key in [Key::ArrowUp, Key::ArrowDown, Key::Home, Key::End] {
                if let Some(target) = git_keyboard_target(&items, focused, key)
                    && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, key))
                {
                    ui.memory_mut(|memory| memory.request_focus(target));
                    break;
                }
            }
        }
        ui.data_mut(|data| {
            data.insert_temp(egui::Id::new("git-active-menu"), items_id);
            data.insert_temp(items_id, Vec::<egui::Id>::new());
        });
        let focus = !ui.is_sizing_pass()
            && ui
                .data(|data| data.get_temp::<bool>(id.with("focus-first")))
                .unwrap_or(false);
        contents(ui, focus);
        ui.data_mut(|data| data.remove::<egui::Id>(egui::Id::new("git-active-menu")));
        if focus {
            ui.data_mut(|data| data.remove::<bool>(id.with("focus-first")));
        }
    });
    git_popup_focus(response, id);
}

pub(super) fn git_menu_item(
    ui: &mut egui::Ui,
    label: &str,
    shortcut: &str,
    tooltip: &str,
    focus: bool,
) -> egui::Response {
    let response = design::menu_item(ui, label, shortcut, tooltip);
    if response.enabled() {
        ui.data_mut(|data| {
            if let Some(id) = data.get_temp::<egui::Id>(egui::Id::new("git-active-menu")) {
                data.get_temp_mut_or_default::<Vec<egui::Id>>(id)
                    .push(response.id);
            }
        });
        if response.has_focus() {
            ui.memory_mut(|memory| {
                memory.set_focus_lock_filter(
                    response.id,
                    egui::EventFilter {
                        vertical_arrows: true,
                        ..Default::default()
                    },
                )
            });
        }
    }
    if focus && response.enabled() {
        response.request_focus();
    }
    response
}

impl VEditApp {
    pub(in crate::app) fn git_content_has_focus(&self) -> bool {
        let Some(focused) = self.ctx.memory(|memory| memory.focused()) else {
            return false;
        };
        self.project().is_some_and(|project| {
            let state = &project.git_view;
            state
                .keyboard_rows
                .iter()
                .any(|id| *id == focused || id.with("action") == focused)
                || state.keyboard_controls.contains(&focused)
        })
    }

    pub(in crate::app) fn restore_git_detail_content_focus(&mut self) -> bool {
        let Some(project) = self
            .projects
            .get_mut(self.active_project)
            .filter(|project| {
                project.git_view.detail_open && project.git_view.detail_keyboard_focus.is_some()
            })
        else {
            return false;
        };
        project.git_view.detail_keyboard_focus_pending = true;
        self.ctx.request_repaint();
        true
    }

    pub(in crate::app) fn restore_git_popup_focus(&mut self) -> bool {
        let Some(id) = take_git_popup_return(&self.ctx) else {
            return false;
        };
        if let Some(tab) = self.tab_mut() {
            tab.focus = false;
        }
        request_git_row_focus(&self.ctx, id);
        self.ctx.request_repaint();
        true
    }

    /// Enter Git without activating a row or stealing arrows from the composer.
    pub(in crate::app) fn focus_git_content(&mut self) {
        if let Some(tab) = self.tab() {
            let id = self.document_widget_id(tab.id);
            self.ctx.memory_mut(|memory| {
                if memory.focused() == Some(id) {
                    memory.surrender_focus(id);
                }
            });
        }
        if let Some(tab) = self.tab_mut() {
            tab.focus = false;
        }
        if let Some(project) = self.projects.get_mut(self.active_project) {
            project.git_view.keyboard_focus_pending = true;
        }
        self.ctx.request_repaint();
    }
}

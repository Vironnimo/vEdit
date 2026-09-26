//! Vim leader discovery and a searchable, persistent bookmark picker.

use super::*;
use crate::bookmarks::Bookmark;

const LEADER_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LeaderStage {
    Actions,
    Jump,
}

pub(super) struct LeaderState {
    pub(super) stage: LeaderStage,
    started: Instant,
}

#[derive(Default)]
pub(super) struct NavigationState {
    pub(super) leader: Option<LeaderState>,
    leader_rect: Option<egui::Rect>,
    picker: Option<BookmarkPicker>,
}

struct BookmarkPicker {
    query: String,
    selected: Option<u64>,
    project_only: bool,
    focus: bool,
    editing: Option<BookmarkLabel>,
    clearing: bool,
}

struct BookmarkLabel {
    id: u64,
    name: String,
    focus: bool,
}

#[derive(Clone, Copy)]
enum NavigationAction {
    ToggleBookmark,
    OpenBookmarks,
    ScreenJump,
}

impl MorrowApp {
    pub(super) fn navigation_modal_open(&self) -> bool {
        self.navigation.picker.is_some()
    }

    pub(super) fn navigation_pending(&self) -> bool {
        self.navigation.leader.is_some()
    }

    pub(super) fn open_bookmarks(&mut self) {
        self.navigation.leader = None;
        self.completions = None;
        self.palette = None;
        self.navigation.picker = Some(BookmarkPicker {
            query: String::new(),
            selected: None,
            project_only: self.project().is_some(),
            focus: true,
            editing: None,
            clearing: false,
        });
        self.ctx
            .memory_mut(|memory| memory.request_focus(egui::Id::new("bookmark_query")));
        self.ctx.request_repaint();
    }

    pub(super) fn cancel_navigation(&mut self) {
        let leader = self.navigation.leader.take().is_some();
        let picker = self.navigation.picker.take().is_some();
        if leader || picker {
            self.navigation_restore_editor_focus();
        }
    }

    /// Run before Vim dispatch. Known sequences consume only their own events;
    /// an unknown suffix returns to Vim in the same frame instead of vanishing.
    pub(super) fn navigation_keys(&mut self) -> bool {
        if self.navigation_modal_open() {
            return true;
        }
        let ctx = self.ctx.clone();
        let eligible = self.settings.vim
            && !self.vim_search_active()
            && self.screen_jump.is_none()
            && !egui::Popup::is_any_open(&ctx)
            && self.palette.is_none()
            && self.picker.is_none()
            && !self.settings_open
            && self.save_as.is_none()
            && self.close_action.is_none()
            && self.vim_command.is_none()
            && self.diff.is_none()
            && !self.git_detail_open()
            && self.context_file_dialog.is_none()
            && self.tab().is_some_and(|tab| {
                tab.vim.mode != VimMode::Insert
                    && tab.agent_view == crate::cli::DocumentView::Source
                    && ctx.memory(|memory| memory.has_focus(self.document_widget_id(tab.id)))
                    && (self.navigation_pending() || tab.vim.pending_command().is_empty())
                    && !tab.vim.expects_literal_character()
            });
        if !eligible {
            self.navigation.leader = None;
            return false;
        }
        if self
            .navigation
            .leader
            .as_ref()
            .is_some_and(|leader| leader.started.elapsed() >= LEADER_TIMEOUT)
        {
            self.navigation.leader = None;
        }
        if self.navigation.leader.is_some()
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Escape))
        {
            self.cancel_navigation();
            return true;
        }
        if ctx.input(|input| {
            input.pointer.any_pressed()
                && input.pointer.interact_pos().is_none_or(|position| {
                    !self
                        .navigation
                        .leader_rect
                        .is_some_and(|rect| rect.contains(position))
                })
        }) {
            self.navigation.leader = None;
            return false;
        }

        let leader_key = self.settings.leader_key.chars().next().unwrap_or(' ');
        let events = ctx.input(|input| input.events.clone());
        let text_keys: Vec<char> = events
            .iter()
            .filter_map(|event| match event {
                egui::Event::Text(text) => Some(text.chars()),
                _ => None,
            })
            .flatten()
            .collect();
        let mut retained = Vec::with_capacity(events.len());
        let mut consumed_keys = Vec::new();
        let mut actions = Vec::new();
        let mut consumed = false;
        let mut passthrough = false;
        for event in events {
            match event {
                egui::Event::Text(text) => {
                    let mut remainder = String::new();
                    for ch in text.chars() {
                        if !passthrough && self.navigation_character(ch, leader_key, &mut actions) {
                            consumed = true;
                            consumed_keys.push(ch);
                        } else {
                            remainder.push(ch);
                            // Vim must see this key before interpreting the
                            // following ones: `i ` inserts a space and `f `
                            // finds one even when both arrive in one frame.
                            passthrough = true;
                        }
                    }
                    if !remainder.is_empty() {
                        retained.push(egui::Event::Text(remainder));
                    }
                }
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    repeat,
                    ..
                } if !modifiers.command && !modifiers.ctrl && !modifiers.alt => {
                    let ch = key_character(key, modifiers.shift);
                    // Desktop input includes both Key and Text. Text owns that
                    // character, with a Key fallback for synthetic inputs and
                    // platforms that omit the Space text event.
                    if let Some(ch) = ch
                        && !passthrough
                        && !text_keys.contains(&ch)
                        && !repeat
                        && self.navigation_character(ch, leader_key, &mut actions)
                    {
                        consumed = true;
                        consumed_keys.push(ch);
                    } else {
                        if !passthrough && ch.is_none() && self.navigation.leader.is_some() {
                            self.navigation.leader = None;
                            passthrough = true;
                        }
                        retained.push(event);
                    }
                }
                egui::Event::Key { pressed: true, .. } => {
                    if self.navigation.leader.take().is_some() {
                        passthrough = true;
                    }
                    retained.push(event);
                }
                other => retained.push(other),
            }
        }
        if consumed {
            retained.retain(|event| match event {
                egui::Event::Key { key, modifiers, .. }
                    if !modifiers.command && !modifiers.ctrl && !modifiers.alt =>
                {
                    !key_character(*key, modifiers.shift)
                        .is_some_and(|ch| consumed_keys.contains(&ch))
                }
                _ => true,
            });
            ctx.input_mut(|input| input.events = retained);
        }
        for action in actions {
            self.navigation_action(action);
        }
        if self.navigation.leader.is_some() {
            ctx.request_repaint_after(LEADER_TIMEOUT);
        }
        self.navigation.leader.is_some() || self.navigation_modal_open()
    }

    fn navigation_character(
        &mut self,
        ch: char,
        leader_key: char,
        actions: &mut Vec<NavigationAction>,
    ) -> bool {
        if let Some(leader) = self.navigation.leader.as_mut() {
            let action = match (leader.stage, ch) {
                (LeaderStage::Actions, 'm') => Some(NavigationAction::ToggleBookmark),
                (LeaderStage::Actions, 'b') => Some(NavigationAction::OpenBookmarks),
                (LeaderStage::Actions, ch) if ch == leader_key => {
                    leader.stage = LeaderStage::Jump;
                    leader.started = Instant::now();
                    return true;
                }
                (LeaderStage::Jump, 'f') => Some(NavigationAction::ScreenJump),
                _ => None,
            };
            self.navigation.leader = None;
            if let Some(action) = action {
                actions.push(action);
                return true;
            }
            self.status(
                "Unknown leader command; press the leader key to see available actions",
                false,
            );
            return false;
        }
        if ch == leader_key {
            self.navigation.leader = Some(LeaderState {
                stage: LeaderStage::Actions,
                started: Instant::now(),
            });
            return true;
        }
        false
    }

    fn navigation_action(&mut self, action: NavigationAction) {
        match action {
            NavigationAction::ToggleBookmark => {
                self.toggle_bookmark();
                self.navigation_restore_editor_focus();
            }
            NavigationAction::OpenBookmarks => self.open_bookmarks(),
            NavigationAction::ScreenJump => {
                self.start_screen_jump();
                self.navigation_restore_editor_focus();
            }
        }
    }

    pub(super) fn navigation_ui(&mut self) {
        self.leader_ui();
        self.bookmarks_ui();
    }

    fn leader_ui(&mut self) {
        let Some(leader) = &self.navigation.leader else {
            return;
        };
        let stage = leader.stage;
        let label = leader_label(&self.settings.leader_key);
        let ctx = self.ctx.clone();
        let mut action = None;
        let mut more = false;
        let mut cancel = false;
        let area = egui::Area::new(egui::Id::new("navigation_leader_help"))
            .anchor(egui::Align2::RIGHT_BOTTOM, Vec2::new(-20.0, -38.0))
            .order(egui::Order::Foreground)
            .show(&ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .show(ui, |ui| {
                        ui.set_width(245.0);
                        // Keep both leader levels at the same size. Shrinking
                        // an anchored card otherwise moves its pointer targets
                        // between the hover and press frames.
                        ui.set_min_height(160.0);
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(if stage == LeaderStage::Actions {
                                    format!("Leader · {label}")
                                } else {
                                    format!("Jump · {label} {label}")
                                })
                                .strong(),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                cancel = design::ghost_button(
                                    ui,
                                    "×",
                                    "Cancel the leader sequence (Esc)",
                                )
                                .clicked();
                            });
                        });
                        ui.separator();
                        if stage == LeaderStage::Actions {
                            if design::menu_item(
                                ui,
                                "Toggle bookmark",
                                "m",
                                "Mark or unmark the current file and line",
                            )
                            .clicked()
                            {
                                action = Some(NavigationAction::ToggleBookmark);
                            }
                            if design::menu_item(
                                ui,
                                "Bookmarks",
                                "b",
                                "Search saved places across your projects",
                            )
                            .clicked()
                            {
                                action = Some(NavigationAction::OpenBookmarks);
                            }
                            more = design::menu_item(
                                ui,
                                "Jump to visible text",
                                &label,
                                "Show the jump key: then press f",
                            )
                            .clicked();
                        } else if design::menu_item(
                            ui,
                            "Jump to visible text",
                            "f",
                            "Pick an on-screen target without moving the mouse",
                        )
                        .clicked()
                        {
                            action = Some(NavigationAction::ScreenJump);
                        }
                        ui.label(RichText::new("Esc cancels").size(10.5).color(MUTED));
                    })
                    .response
                    .rect
            });
        // Clicking a visible action must not cancel the sequence as an outside click.
        self.navigation.leader_rect = Some(area.inner);
        if cancel {
            self.cancel_navigation();
        } else if more {
            self.navigation.leader = Some(LeaderState {
                stage: LeaderStage::Jump,
                started: Instant::now(),
            });
            if let Some(tab) = self.tab() {
                let id = self.document_widget_id(tab.id);
                ctx.memory_mut(|memory| memory.request_focus(id));
            }
        } else if let Some(action) = action {
            self.navigation.leader = None;
            self.navigation_action(action);
        }
    }

    fn bookmarks_ui(&mut self) {
        let Some(mut picker) = self.navigation.picker.take() else {
            return;
        };
        let ctx = self.ctx.clone();
        let root = self.project().map(|project| project.root.clone());
        let count = self.bookmarks.items.len();
        let mut jump = None;
        let mut remove = None;
        let mut save_label = None;
        let mut clear = false;
        let mut add = false;
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::new("bookmark_picker")).show(&ctx, |ui| {
            ui.set_width((ctx.content_rect().width() - 70.0).clamp(280.0, 620.0));
            ui.horizontal(|ui| {
                ui.label(RichText::new("Bookmarks").size(20.0).strong());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    close = design::ghost_button(ui, "×", "Close bookmarks (Esc)").clicked();
                });
            });
            ui.add_space(10.0);
            ui.add_enabled_ui(picker.editing.is_none() && !picker.clearing, |ui| {
                if picker.focus {
                    ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("bookmark_query")));
                    picker.focus = false;
                }
                let response = ui.add(
                    egui::TextEdit::singleline(&mut picker.query)
                        .id(egui::Id::new("bookmark_query"))
                        .hint_text("Find a file, label, or bookmarked text…")
                        .desired_width(f32::INFINITY),
                );
                if response.changed() {
                    picker.selected = None;
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            root.is_some(),
                            egui::Button::new("Current project").selected(picker.project_only),
                        )
                        .clicked()
                    {
                        picker.project_only = true;
                        picker.selected = None;
                        picker.focus = true;
                    }
                    if ui
                        .add(egui::Button::new("All projects").selected(!picker.project_only))
                        .clicked()
                    {
                        picker.project_only = false;
                        picker.selected = None;
                        picker.focus = true;
                    }
                });
            });
            let matches = bookmark_matches(
                &self.bookmarks.items,
                &picker.query,
                if picker.project_only {
                    root.as_deref()
                } else {
                    None
                },
            );
            let selected = picker
                .selected
                .and_then(|id| matches.iter().position(|bookmark| bookmark.id == id))
                .unwrap_or(0);
            picker.selected = matches.get(selected).map(|bookmark| bookmark.id);
            let keys_enabled =
                !egui::Popup::is_any_open(&ctx) && picker.editing.is_none() && !picker.clearing;
            let up = keys_enabled
                && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::ArrowUp));
            let down = keys_enabled
                && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::ArrowDown));
            if !matches.is_empty() && (up || down) {
                let next = if up {
                    selected.saturating_sub(1)
                } else {
                    (selected + 1).min(matches.len() - 1)
                };
                picker.selected = Some(matches[next].id);
            }
            if keys_enabled
                && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Enter))
            {
                jump = picker.selected;
            }
            ui.add_space(10.0);
            if matches.is_empty() {
                ui.label(
                    RichText::new(if count == 0 {
                        "No bookmarks yet. Mark the current line with the leader key, then m."
                    } else {
                        "No matching bookmarks. Try another search or select All projects."
                    })
                    .color(MUTED),
                );
                ui.add_space(30.0);
            } else {
                ui.add_enabled_ui(picker.editing.is_none() && !picker.clearing, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    let height = (ctx.content_rect().height() - 250.0).clamp(120.0, 430.0);
                    let mut scroll = egui::ScrollArea::vertical()
                        .id_salt("bookmark_results")
                        .max_height(height);
                    if (up || down)
                        && let Some(index) = matches
                            .iter()
                            .position(|bookmark| Some(bookmark.id) == picker.selected)
                    {
                        scroll = scroll
                            .vertical_scroll_offset((index as f32 * 48.0 - height * 0.5).max(0.0));
                    }
                    scroll.show_rows(ui, 48.0, matches.len(), |ui, rows| {
                        for index in rows {
                            let bookmark = matches[index];
                            ui.push_id(bookmark.id, |ui| {
                                let title = if bookmark.label.is_empty() {
                                    format!("{}:{}", file_title(&bookmark.path), bookmark.line)
                                } else {
                                    bookmark.label.clone()
                                };
                                let path = root
                                    .as_deref()
                                    .and_then(|root| bookmark.path.strip_prefix(root).ok())
                                    .unwrap_or(&bookmark.path);
                                let detail = format!(
                                    "{} · {}:{}",
                                    crate::paths::display(path),
                                    bookmark.line,
                                    bookmark.column
                                );
                                let tooltip = format!(
                                    "{}:{}:{}\n{}",
                                    crate::paths::display(&bookmark.path),
                                    bookmark.line,
                                    bookmark.column,
                                    bookmark.context_line
                                );
                                let response = design::detail_row(
                                    ui,
                                    &title,
                                    &detail,
                                    picker.selected == Some(bookmark.id),
                                    &tooltip,
                                );
                                if response.clicked() {
                                    jump = Some(bookmark.id);
                                }
                                if (up || down) && picker.selected == Some(bookmark.id) {
                                    response.scroll_to_me(Some(Align::Center));
                                }
                                response.context_menu(|ui| {
                                    if design::menu_item(
                                        ui,
                                        "Edit label…",
                                        "",
                                        "Give this bookmark an optional name",
                                    )
                                    .clicked()
                                    {
                                        picker.editing = Some(BookmarkLabel {
                                            id: bookmark.id,
                                            name: bookmark.label.clone(),
                                            focus: true,
                                        });
                                        ui.close();
                                    }
                                    if design::menu_item(
                                        ui,
                                        "Remove bookmark",
                                        "",
                                        "Remove this saved place; the document stays unchanged",
                                    )
                                    .clicked()
                                    {
                                        remove = Some(bookmark.id);
                                        ui.close();
                                    }
                                });
                            });
                        }
                    });
                });
            }
            ui.add_space(12.0);
            ui.separator();
            if let Some(label) = picker.editing.as_mut() {
                ui.label(RichText::new("Bookmark label").strong());
                if label.focus {
                    ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("bookmark_label")));
                    label.focus = false;
                }
                ui.add(
                    egui::TextEdit::singleline(&mut label.name)
                        .id(egui::Id::new("bookmark_label"))
                        .char_limit(256)
                        .hint_text("Optional label")
                        .desired_width(f32::INFINITY),
                );
                let enter =
                    ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Enter));
                let escape =
                    ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Escape));
                let mut cancel = escape;
                ui.horizontal(|ui| {
                    if design::primary_button(ui, "Save label", Vec2::new(100.0, 28.0)).clicked()
                        || enter
                    {
                        save_label = Some((label.id, label.name.clone()));
                    }
                    if design::ghost_button(ui, "Cancel label", "Keep the previous bookmark label")
                        .clicked()
                    {
                        cancel = true;
                    }
                });
                if cancel {
                    picker.editing = None;
                    picker.focus = true;
                }
            } else if picker.clearing {
                ui.label(
                    RichText::new(format!(
                        "Remove all {count} bookmarks across every project?"
                    ))
                    .color(ORANGE),
                );
                ui.horizontal(|ui| {
                    if design::ghost_button(
                        ui,
                        "Keep bookmarks",
                        "Cancel and preserve every saved place",
                    )
                    .clicked()
                    {
                        picker.clearing = false;
                        picker.focus = true;
                    }
                    clear = design::ghost_button(
                        ui,
                        "Remove all",
                        "Remove every bookmark after this confirmation",
                    )
                    .clicked();
                });
                if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Escape)) {
                    picker.clearing = false;
                    picker.focus = true;
                }
            } else {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("{} shown · {count} total", matches.len()))
                            .size(10.5)
                            .color(MUTED),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .add_enabled_ui(count > 0, |ui| {
                                design::ghost_button(
                                    ui,
                                    "Remove all…",
                                    "Review a confirmation before removing all bookmarks",
                                )
                            })
                            .inner
                            .clicked()
                        {
                            picker.clearing = true;
                        }
                        add = ui
                            .add_enabled_ui(
                                self.tab().is_some_and(|tab| tab.path.is_some()),
                                |ui| {
                                    design::ghost_button(
                                        ui,
                                        "Toggle current line",
                                        "Mark or unmark the current editor line",
                                    )
                                },
                            )
                            .inner
                            .clicked();
                    });
                });
                ui.label(
                    RichText::new("↑ ↓ choose · Enter opens · Right-click edits · Esc closes")
                        .size(10.5)
                        .color(MUTED),
                );
            }
        });
        close |= modal.should_close();
        if let Some(id) = remove
            && self.bookmarks.remove(id)
        {
            self.bookmark_lines.clear();
            self.session_dirty = true;
            picker.selected = None;
            picker.focus = true;
        }
        if let Some((id, label)) = save_label {
            if self.bookmarks.set_label(id, &label) {
                self.bookmark_lines.clear();
                self.session_dirty = true;
            }
            picker.editing = None;
            picker.focus = true;
        }
        if clear {
            self.bookmarks.clear();
            self.bookmark_lines.clear();
            self.session_dirty = true;
            picker.clearing = false;
            picker.selected = None;
            picker.focus = true;
        }
        if add {
            self.toggle_bookmark();
        }
        if let Some(id) = jump {
            self.navigation_restore_editor_focus();
            self.jump_to_bookmark(id);
        } else if close {
            self.navigation_restore_editor_focus();
        } else {
            self.navigation.picker = Some(picker);
        }
    }

    fn navigation_restore_editor_focus(&mut self) {
        if let Some(id) = self.ctx.memory(|memory| memory.focused()) {
            self.ctx.memory_mut(|memory| memory.surrender_focus(id));
        }
        if let Some(tab) = self.tab_mut() {
            tab.focus = tab.agent_view == crate::cli::DocumentView::Source;
        }
        if let Some(tab) = self.tab()
            && tab.agent_view == crate::cli::DocumentView::Source
        {
            let id = self.document_widget_id(tab.id);
            self.ctx.memory_mut(|memory| memory.request_focus(id));
        }
    }
}

fn leader_label(key: &str) -> String {
    if key == " " || key.is_empty() {
        "Space".into()
    } else {
        key.into()
    }
}

fn key_character(key: Key, shift: bool) -> Option<char> {
    let ch = match key {
        Key::Space => ' ',
        Key::Comma => {
            if shift {
                '<'
            } else {
                ','
            }
        }
        Key::Semicolon => {
            if shift {
                ':'
            } else {
                ';'
            }
        }
        Key::Backslash => {
            if shift {
                '|'
            } else {
                '\\'
            }
        }
        Key::A => 'a',
        Key::B => 'b',
        Key::C => 'c',
        Key::D => 'd',
        Key::E => 'e',
        Key::F => 'f',
        Key::G => 'g',
        Key::H => 'h',
        Key::I => 'i',
        Key::J => 'j',
        Key::K => 'k',
        Key::L => 'l',
        Key::M => 'm',
        Key::N => 'n',
        Key::O => 'o',
        Key::P => 'p',
        Key::Q => 'q',
        Key::R => 'r',
        Key::S => 's',
        Key::T => 't',
        Key::U => 'u',
        Key::V => 'v',
        Key::W => 'w',
        Key::X => 'x',
        Key::Y => 'y',
        Key::Z => 'z',
        _ => return None,
    };
    Some(if shift { ch.to_ascii_uppercase() } else { ch })
}

fn bookmark_matches<'a>(
    items: &'a [Bookmark],
    query: &str,
    root: Option<&Path>,
) -> Vec<&'a Bookmark> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return items
            .iter()
            .filter(|bookmark| root.is_none_or(|root| bookmark.path.starts_with(root)))
            .collect();
    }
    let mut matches: Vec<_> = items
        .iter()
        .enumerate()
        .filter_map(|(order, bookmark)| {
            if root.is_some_and(|root| !bookmark.path.starts_with(root)) {
                return None;
            }
            let searchable = format!(
                "{} {} {}:{} {}",
                bookmark.label,
                crate::paths::display(&bookmark.path),
                bookmark.line,
                bookmark.column,
                bookmark.context_line
            )
            .to_lowercase();
            fuzzy_score(&searchable, &query).map(|score| (score, order, bookmark))
        })
        .collect();
    matches.sort_by_key(|(score, order, _)| (*score, *order));
    matches
        .into_iter()
        .map(|(_, _, bookmark)| bookmark)
        .collect()
}

fn fuzzy_score(text: &str, query: &str) -> Option<usize> {
    let mut score = 0;
    for token in query.split_whitespace() {
        if let Some(offset) = text.find(token) {
            score += offset;
            continue;
        }
        let mut chars = text.chars().enumerate();
        let mut previous = 0;
        for wanted in token.chars() {
            let (position, _) = chars.find(|(_, ch)| *ch == wanted)?;
            score += position.saturating_sub(previous) + 20;
            previous = position;
        }
    }
    Some(score)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};

    const DOCUMENT: &str = "alpha first\nbeta second\ngamma third\n";

    fn fixture() -> MorrowApp {
        let mut app = MorrowApp::empty(egui::Context::default());
        app.settings.autosave = false;
        let root = std::env::temp_dir().join("morrow-navigation-in-memory");
        let mut project = Project::new(root.clone());
        project.last_git_refresh = Instant::now();
        let mut tab = EditorTab::new(Some(root.join("notes.txt")), "notes.txt".into());
        tab.text = DOCUMENT.into();
        tab.saved_text = DOCUMENT.into();
        project.tabs.push(tab);
        app.projects.push(project);
        app
    }

    fn seeded_fixture() -> MorrowApp {
        let mut app = fixture();
        let path = app.tab().unwrap().path.clone().unwrap();
        app.bookmarks = crate::bookmarks::BookmarkStore::from_items(vec![
            Bookmark {
                id: 9,
                path: path.clone(),
                line: 2,
                column: 3,
                label: "Second place".into(),
                context_line: "beta second".into(),
            },
            Bookmark {
                id: 10,
                path,
                line: 3,
                column: 1,
                label: "Third place".into(),
                context_line: "gamma third".into(),
            },
            Bookmark {
                id: 11,
                path: std::env::temp_dir().join("morrow-other-project/plan.md"),
                line: 8,
                column: 2,
                label: "Other project".into(),
                context_line: "Deployment checklist".into(),
            },
        ]);
        app
    }

    fn harness(app: MorrowApp) -> Harness<'static, MorrowApp> {
        let mut screen = Harness::builder()
            .with_size([1080.0, 740.0])
            .with_step_dt(1.0 / 60.0)
            .with_os(egui::os::OperatingSystem::Windows)
            .build_ui_state(
                |ui, app: &mut MorrowApp| {
                    app.ctx = ui.ctx().clone();
                    if !app.navigation_keys() {
                        app.handle_vim();
                    }
                    app.editor_ui(ui);
                    app.navigation_ui();
                },
                app,
            );
        screen.run_steps(3);
        screen
    }

    fn character(screen: &mut Harness<'_, MorrowApp>, key: Key, text: &str) {
        let key_event = |pressed| egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        screen.input_mut().events.extend([
            key_event(true),
            egui::Event::Text(text.into()),
            key_event(false),
        ]);
        screen.run_steps(2);
    }

    #[test]
    fn leader_discovers_actions_without_changing_the_document_or_cursor() {
        let mut screen = harness(fixture());
        character(&mut screen, Key::Space, " ");
        assert!(screen.state().navigation_pending());
        assert_eq!(
            screen.state().navigation.leader.as_ref().unwrap().stage,
            LeaderStage::Actions
        );
        assert!(screen.query_by_label_contains("Toggle bookmark").is_some());
        assert_eq!(screen.state().tab().unwrap().text, DOCUMENT);
        assert_eq!(screen.state().tab().unwrap().vim.cursor, 0);
        screen.key_press(Key::Escape);
        screen.run_steps(2);
        assert!(!screen.state().navigation_pending());
        assert_eq!(screen.state().tab().unwrap().text, DOCUMENT);
    }

    #[test]
    fn leader_bookmark_toggle_preserves_visual_line_selection() {
        let mut screen = harness(fixture());
        let tab = screen.state_mut().tab_mut().unwrap();
        tab.vim.handle_key(&mut tab.text, VimKey::Char('V'));
        let selection = tab.vim.selection(&tab.text).unwrap();
        tab.selection = selection.clone();
        tab.pending_selection = Some(selection.clone());
        screen.run_steps(2);
        for _ in 0..2 {
            character(&mut screen, Key::Space, " ");
            character(&mut screen, Key::M, "m");
            assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::VisualLine);
            assert_eq!(screen.state().tab().unwrap().selection, selection);
            assert_eq!(screen.state().tab().unwrap().text, DOCUMENT);
        }
        assert!(screen.state().bookmarks.items.is_empty());
        assert!(screen.state().session_dirty);
    }

    #[test]
    fn doubled_leader_f_starts_the_visible_jump_without_editing_text() {
        let mut screen = harness(fixture());
        character(&mut screen, Key::Space, " ");
        character(&mut screen, Key::Space, " ");
        assert_eq!(
            screen.state().navigation.leader.as_ref().unwrap().stage,
            LeaderStage::Jump
        );
        character(&mut screen, Key::F, "f");
        assert!(screen.state().screen_jump.is_some());
        assert!(!screen.state().navigation_pending());
        assert_eq!(screen.state().tab().unwrap().text, DOCUMENT);
    }

    #[test]
    fn leader_overlay_pointer_actions_keep_the_editor_ready_for_vim() {
        let mut screen = harness(fixture());
        character(&mut screen, Key::Space, " ");
        screen.get_by_label_contains("Toggle bookmark").click();
        screen.run_steps(2);
        assert_eq!(screen.state().bookmarks.items.len(), 1);
        let id = screen
            .state()
            .document_widget_id(screen.state().tab().unwrap().id);
        assert!(screen.ctx.memory(|memory| memory.has_focus(id)));
        character(&mut screen, Key::Space, " ");
        screen.get_by_label_contains("Jump to visible text").click();
        screen.run_steps(2);
        assert_eq!(
            screen.state().navigation.leader.as_ref().unwrap().stage,
            LeaderStage::Jump
        );
        screen.get_by_label_contains("Jump to visible text").click();
        screen.run_steps(2);
        assert!(
            screen.state().screen_jump.is_some(),
            "pointer jump did not start: pending={:?}, focus={:?}, help_rect={:?}, pointer={:?}, status={:?}",
            screen
                .state()
                .navigation
                .leader
                .as_ref()
                .map(|leader| leader.stage),
            screen.ctx.memory(|memory| memory.focused()),
            screen.state().navigation.leader_rect,
            screen.ctx.pointer_hover_pos(),
            screen
                .state()
                .status
                .as_ref()
                .map(|(message, _, _)| message)
        );
        assert_eq!(screen.state().tab().unwrap().text, DOCUMENT);
    }

    #[test]
    fn custom_leader_and_unknown_suffix_keep_ordinary_vim_motions() {
        let mut app = fixture();
        app.settings.leader_key = ",".into();
        let mut screen = harness(app);
        character(&mut screen, Key::Comma, ",");
        character(&mut screen, Key::M, "m");
        assert_eq!(screen.state().bookmarks.items.len(), 1);
        character(&mut screen, Key::Comma, ",");
        character(&mut screen, Key::J, "j");
        assert!(!screen.state().navigation_pending());
        assert!(screen.state().tab().unwrap().vim.cursor >= "alpha first\n".len());
        assert_eq!(screen.state().tab().unwrap().text, DOCUMENT);
    }

    #[test]
    fn insert_and_literal_find_spaces_remain_vim_input_even_in_one_frame() {
        let mut screen = harness(fixture());
        screen
            .input_mut()
            .events
            .push(egui::Event::Text("f ".into()));
        screen.run_steps(2);
        assert!(!screen.state().navigation_pending());
        assert_eq!(screen.state().tab().unwrap().vim.cursor, 5);
        screen
            .input_mut()
            .events
            .push(egui::Event::Text("i ".into()));
        screen.run_steps(2);
        assert!(!screen.state().navigation_pending());
        assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::Insert);
        assert_eq!(
            screen.state().tab().unwrap().text,
            "alpha  first\nbeta second\ngamma third\n"
        );
        character(&mut screen, Key::Space, " ");
        assert_eq!(
            screen.state().tab().unwrap().text,
            "alpha   first\nbeta second\ngamma third\n"
        );
        assert!(!screen.state().navigation_pending());
    }

    #[test]
    fn expired_leader_leaves_the_next_key_available_to_vim() {
        let mut screen = harness(fixture());
        character(&mut screen, Key::Space, " ");
        screen
            .state_mut()
            .navigation
            .leader
            .as_mut()
            .unwrap()
            .started = Instant::now() - LEADER_TIMEOUT;
        character(&mut screen, Key::J, "j");
        assert!(!screen.state().navigation_pending());
        assert!(screen.state().tab().unwrap().vim.cursor >= "alpha first\n".len());
    }

    #[test]
    fn bookmark_picker_keyboard_selects_and_restores_editor_focus() {
        let mut screen = harness(seeded_fixture());
        character(&mut screen, Key::Space, " ");
        character(&mut screen, Key::B, "b");
        assert!(screen.state().navigation_modal_open());
        screen.key_press(Key::ArrowDown);
        screen.run_steps(2);
        assert_eq!(
            screen.state().navigation.picker.as_ref().unwrap().selected,
            Some(10)
        );
        assert_eq!(screen.state().tab().unwrap().vim.cursor, 0);
        screen.key_press(Key::Enter);
        screen.run_steps(2);
        assert!(!screen.state().navigation_modal_open());
        let tab = screen.state().tab().unwrap();
        assert_eq!(tab.vim.cursor, "alpha first\nbeta second\n".len());
        assert_eq!(tab.text, DOCUMENT);
        assert!(
            screen
                .ctx
                .memory(|memory| memory.has_focus(screen.state().document_widget_id(tab.id)))
        );
    }

    #[test]
    fn bookmark_rows_jump_and_scope_filters_other_projects() {
        let mut screen = harness(seeded_fixture());
        screen.state_mut().open_bookmarks();
        screen.run_steps(3);
        assert!(screen.query_by_label_contains("Other project:").is_none());
        screen.get_by_label("All projects").click();
        screen.run_steps(2);
        assert!(screen.query_by_label_contains("Other project:").is_some());
        screen.get_by_label_contains("Second place:").click();
        screen.run_steps(2);
        assert!(!screen.state().navigation_modal_open());
        assert_eq!(
            screen.state().tab().unwrap().vim.cursor,
            "alpha first\n".len() + 2
        );
        assert_eq!(screen.state().tab().unwrap().text, DOCUMENT);
    }

    #[test]
    fn bookmark_context_label_and_remove_update_persistent_records() {
        let mut screen = harness(seeded_fixture());
        screen.state_mut().open_bookmarks();
        screen.run_steps(3);
        screen
            .get_by_label_contains("Second place:")
            .click_secondary();
        screen.run_steps(3);
        screen.get_by_label("Edit label…").click();
        screen.run_steps(3);
        screen
            .state_mut()
            .navigation
            .picker
            .as_mut()
            .unwrap()
            .editing
            .as_mut()
            .unwrap()
            .name = "Review target".into();
        screen.get_by_label("Save label").click();
        screen.run_steps(3);
        assert_eq!(
            screen.state().bookmarks.get(9).unwrap().label,
            "Review target"
        );
        screen
            .get_by_label_contains("Review target:")
            .click_secondary();
        screen.run_steps(3);
        screen.get_by_label("Remove bookmark").click();
        screen.run_steps(3);
        assert!(screen.state().bookmarks.get(9).is_none());
        assert!(screen.state().session_dirty);
        assert_eq!(screen.state().tab().unwrap().text, DOCUMENT);
    }

    #[test]
    fn clearing_all_bookmarks_requires_the_visible_confirmation() {
        let mut screen = harness(seeded_fixture());
        screen.state_mut().open_bookmarks();
        screen.run_steps(3);
        screen.get_by_label("Remove all…").click();
        screen.run_steps(2);
        assert_eq!(screen.state().bookmarks.items.len(), 3);
        screen.get_by_label("Keep bookmarks").click();
        screen.run_steps(2);
        assert_eq!(screen.state().bookmarks.items.len(), 3);
        screen.get_by_label("Remove all…").click();
        screen.run_steps(2);
        screen.get_by_label("Remove all").click();
        screen.run_steps(2);
        assert!(screen.state().bookmarks.items.is_empty());
        assert!(screen.state().navigation_modal_open());
        assert_eq!(screen.state().tab().unwrap().text, DOCUMENT);
    }

    #[test]
    fn bookmark_matching_is_fuzzy_and_searches_labels_paths_and_context() {
        let app = seeded_fixture();
        let root = &app.projects[0].root;
        assert_eq!(
            bookmark_matches(&app.bookmarks.items, "s c n d", Some(root))[0].id,
            9
        );
        assert_eq!(
            bookmark_matches(&app.bookmarks.items, "gamma", Some(root))[0].id,
            10
        );
        assert_eq!(
            bookmark_matches(&app.bookmarks.items, "deployment", None)[0].id,
            11
        );
        assert!(bookmark_matches(&app.bookmarks.items, "deployment", Some(root)).is_empty());
    }
}

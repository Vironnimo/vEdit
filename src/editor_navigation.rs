//! Bookmark navigation and visible-text jumps.
use super::*;

pub(super) struct ActiveScreenJump {
    pub(super) tab_id: u64,
    pub(super) pane_id: u64,
    pub(super) revision: u64,
    pub(super) state: crate::screen_jump::ScreenJump,
    pub(super) glyphs: Vec<crate::screen_jump::VisibleGlyph>,
}

type BookmarkMarks = Vec<(usize, u64, String)>;

pub(super) type BookmarkLineCache = HashMap<u64, (u64, BookmarkMarks)>;

impl VEditApp {
    pub(super) fn toggle_bookmark(&mut self) {
        match self.try_toggle_bookmark(self.tab().map(|tab| tab.id)) {
            Ok(crate::bookmarks::ToggleResult::Added(_)) => self.status("Bookmark saved", false),
            Ok(crate::bookmarks::ToggleResult::Removed(_)) => {
                self.status("Bookmark removed", false)
            }
            Err(error) => self.status(error, true),
        }
    }
    pub(super) fn try_toggle_bookmark(
        &mut self,
        tab_id: Option<u64>,
    ) -> Result<crate::bookmarks::ToggleResult, String> {
        let id = tab_id.ok_or_else(|| "Open a file before saving a bookmark".to_owned())?;
        let tab = self
            .projects
            .iter()
            .flat_map(|p| &p.tabs)
            .find(|tab| tab.id == id)
            .ok_or_else(|| "The document is no longer open".to_owned())?;
        if tab.loading || tab.error.is_some() {
            return Err("Wait until the file has loaded before saving a bookmark".into());
        }
        let path = tab
            .path
            .clone()
            .ok_or_else(|| "Save this file before creating a bookmark".to_owned())?;
        let (line, column) = tab.lines.line_col(&tab.text, tab.revision, tab.vim.cursor);
        let context = tab
            .text
            .lines()
            .nth(line.saturating_sub(1))
            .filter(|line| line.len() <= 4096)
            .unwrap_or("")
            .to_owned();
        let existing = self
            .bookmarks
            .for_path(&path)
            .find(|bookmark| crate::bookmarks::locate(&tab.text, bookmark).line == line)
            .map(|bookmark| bookmark.id);
        let result = if let Some(id) = existing {
            self.bookmarks.remove(id);
            crate::bookmarks::ToggleResult::Removed(id)
        } else {
            self.bookmarks.toggle(&path, line, column, Some(&context))
        };
        self.bookmark_lines.clear();
        self.session_dirty = true;
        Ok(result)
    }
    pub(super) fn jump_to_bookmark(&mut self, id: u64) {
        if let Err(error) = self.try_jump_to_bookmark(id) {
            self.status(error, true);
        }
    }
    pub(super) fn try_jump_to_bookmark(&mut self, id: u64) -> Result<u64, String> {
        self.record_jump();
        let bookmark = self
            .bookmarks
            .get(id)
            .cloned()
            .ok_or_else(|| "The bookmark no longer exists".to_owned())?;
        let existing = self.projects.iter().enumerate().find_map(|(project, p)| {
            p.tabs
                .iter()
                .position(|t| t.path.as_ref() == Some(&bookmark.path))
                .map(|tab| (project, tab))
        });
        if let Some((project, tab)) = existing {
            if let Some(error) = &self.projects[project].tabs[tab].error {
                return Err(format!("The bookmarked file is unavailable: {error}"));
            }
            self.activate_tab(project, tab);
        } else {
            self.open_routed_file(bookmark.path.clone());
        }
        let tab = self
            .tab_mut()
            .ok_or_else(|| "The bookmarked file could not be opened".to_owned())?;
        let id = tab.id;
        tab.agent_view = crate::cli::DocumentView::Source;
        self.pending_bookmark_jump = Some((id, bookmark));
        self.apply_pending_bookmark_jump();
        self.session_dirty = true;
        Ok(id)
    }
    pub(super) fn apply_pending_bookmark_jump(&mut self) {
        let Some((id, bookmark)) = self.pending_bookmark_jump.take() else {
            return;
        };
        let Some(tab) = self.tab_by_id_mut(id) else {
            return;
        };
        if tab.loading {
            self.pending_bookmark_jump = Some((id, bookmark));
            return;
        }
        if let Some(error) = tab.error.clone() {
            self.status(format!("Could not open bookmark: {error}"), true);
            return;
        }
        let location = crate::bookmarks::locate(&tab.text, &bookmark);
        Self::move_navigation_cursor(tab, location.char_index);
        if let Some(stored) = self
            .bookmarks
            .items
            .iter_mut()
            .find(|item| item.id == bookmark.id)
        {
            stored.line = location.line;
            stored.column = location.column;
        }
        self.bookmark_lines.clear();
        self.session_dirty = true;
    }
    fn move_navigation_cursor(tab: &mut EditorTab, cursor: usize) {
        let action = tab.vim.finish_insert(&mut tab.text);
        if action.text_changed {
            tab.changed();
        }
        tab.vim.set_cursor(&tab.text, cursor);
        let selection = tab
            .vim
            .selection(&tab.text)
            .unwrap_or(tab.vim.cursor..tab.vim.cursor);
        tab.selection = selection.clone();
        tab.pending_selection = Some(selection);
        tab.scroll_to_cursor = true;
        tab.focus = true;
    }
    pub(super) fn start_screen_jump(&mut self) {
        if let Some(tab) = self.tab_mut() {
            let action = tab.vim.finish_insert(&mut tab.text);
            if action.text_changed {
                tab.changed();
            }
        }
        let Some(tab) = self.tab() else { return };
        if tab.loading || tab.error.is_some() || tab.agent_view != crate::cli::DocumentView::Source
        {
            self.status(
                "Open a source document to jump between visible matches",
                true,
            );
            return;
        }
        self.screen_jump = Some(ActiveScreenJump {
            tab_id: tab.id,
            pane_id: self.current_pane_id(),
            revision: tab.revision,
            state: crate::screen_jump::ScreenJump::new(),
            glyphs: Vec::new(),
        });
        self.completions = None;
    }
    pub(super) fn screen_jump_keys(&mut self) -> bool {
        let Some(mut jump) = self.screen_jump.take() else {
            return false;
        };
        let valid = self.current_pane_id() == jump.pane_id
            && self.tab().is_some_and(|tab| {
                tab.id == jump.tab_id
                    && tab.revision == jump.revision
                    && tab.agent_view == crate::cli::DocumentView::Source
            })
            && self.palette.is_none()
            && !self.settings_open
            && self.status_details.is_none()
            && !self.agent_marks_modal_open()
            && self.picker.is_none()
            && self.save_as.is_none()
            && self.close_action.is_none()
            && self.vim_command.is_none()
            && self.context_file_dialog.is_none()
            && !egui::Popup::is_any_open(&self.ctx)
            && self.diff.is_none()
            && !self.git_detail_open()
            && !self.navigation_modal_open();
        if !valid || self.ctx.input(|input| input.pointer.any_pressed()) {
            jump.state.cancel();
            return false;
        }
        let events = self
            .ctx
            .input_mut(|input| std::mem::take(&mut input.events));
        // A translated Text event owns its preceding key press, including
        // Shift punctuation and non-US layouts. Comparing character values
        // would process those keys twice and could trigger a jump prematurely.
        let mut keys_with_text = vec![false; events.len()];
        let mut preceding_key = None;
        for (index, event) in events.iter().enumerate() {
            match event {
                egui::Event::Key { pressed: true, .. } => preceding_key = Some(index),
                egui::Event::Text(_)
                | egui::Event::Ime(egui::ImeEvent::Preedit { .. } | egui::ImeEvent::Commit(_)) => {
                    if let Some(key_index) = preceding_key.take() {
                        keys_with_text[key_index] = true;
                    }
                }
                _ => {}
            }
        }
        let mut keep = Vec::new();
        let mut action = crate::screen_jump::JumpAction::None;
        for (event_index, event) in events.into_iter().enumerate() {
            if !matches!(action, crate::screen_jump::JumpAction::None) {
                // A completed jump owns the entire input batch; trailing query
                // characters must never become an editing command.
                if !editor_input::native_input(&event) && !matches!(event, egui::Event::Key { .. })
                {
                    keep.push(event);
                }
                continue;
            }
            match event {
                egui::Event::Text(text)
                | egui::Event::Paste(text)
                | egui::Event::Ime(egui::ImeEvent::Commit(text)) => {
                    for ch in text.chars() {
                        action = jump.state.input_char(ch);
                        jump.state.recompute(&jump.glyphs);
                        if !matches!(action, crate::screen_jump::JumpAction::None) {
                            break;
                        }
                    }
                }
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    if modifiers.command || modifiers.ctrl || modifiers.alt {
                        continue;
                    }
                    match key {
                        Key::Escape => action = crate::screen_jump::JumpAction::Cancel,
                        Key::Enter => {
                            action = jump.state.enter();
                            jump.state.recompute(&jump.glyphs);
                        }
                        Key::Backspace => {
                            jump.state.backspace();
                            jump.state.recompute(&jump.glyphs);
                        }
                        _ => {
                            let name = key.name();
                            let ch = if key == Key::Space {
                                Some(' ')
                            } else if name.chars().count() == 1 {
                                name.chars().next().map(|ch| {
                                    if modifiers.shift {
                                        ch
                                    } else {
                                        ch.to_ascii_lowercase()
                                    }
                                })
                            } else {
                                None
                            };
                            if let Some(ch) = ch.filter(|_| !keys_with_text[event_index]) {
                                action = jump.state.input_char(ch);
                                jump.state.recompute(&jump.glyphs);
                            }
                        }
                    }
                }
                egui::Event::Key { .. } => {}
                egui::Event::Copy | egui::Event::Cut | egui::Event::Ime(_) => {}
                other => keep.push(other),
            }
        }
        self.ctx.input_mut(|input| input.events = keep);
        match action {
            crate::screen_jump::JumpAction::Jump(cursor) => {
                if let Some(tab) = self.tab_mut() {
                    Self::move_navigation_cursor(tab, cursor);
                }
                self.session_dirty = true;
            }
            crate::screen_jump::JumpAction::Cancel => {}
            crate::screen_jump::JumpAction::None => self.screen_jump = Some(jump),
        }
        true
    }
}

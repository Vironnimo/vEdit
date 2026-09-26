//! Cached file search, shared by the native Vim command line and Find toolbar.
use super::*;

const MAX_QUERY_CHARS: usize = 4096;
const ASYNC_DOCUMENT_BYTES: usize = 256 * 1024;

#[path = "find_worker.rs"]
mod worker;

struct PinnedMatches {
    key: worker::Key,
    matches: Arc<[Range<usize>]>,
    error: Option<String>,
    frame: u64,
}

struct PendingNavigation {
    key: worker::Key,
    pane: u64,
    backwards: bool,
    count: usize,
    include_current: bool,
    modal: bool,
    origin: usize,
    navigation: crate::vim::VimNavigation,
    selection: Range<usize>,
    motion_epoch: u64,
    preserve_focus: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Pattern {
    query: String,
    case_sensitive: bool,
    whole_word: bool,
    regex: bool,
}

impl Pattern {
    fn vim(query: String) -> Self {
        Self {
            query,
            case_sensitive: true,
            whole_word: false,
            regex: true,
        }
    }
    fn literal(query: String, options: editor::SearchOptions) -> Self {
        Self {
            query,
            case_sensitive: options.case_sensitive,
            whole_word: options.whole_word,
            regex: false,
        }
    }
}

#[derive(Clone)]
struct SearchIdentity {
    tab_id: u64,
    pane_id: u64,
    revision: u64,
    pattern: Pattern,
    logical_start: usize,
    physical_cursor: usize,
    motion_epoch: u64,
}

pub(super) struct VimSearch {
    tab_id: u64,
    pane_id: u64,
    revision: u64,
    origin: crate::vim::VimNavigation,
    origin_selection: Range<usize>,
    query: String,
    query_cursor: usize,
    backwards: bool,
    count: usize,
    original_scroll: Option<(egui::Id, Vec2)>,
    original_match: Option<SearchIdentity>,
    preview_match: Option<usize>,
    horizontal_scroll: f32,
    confirming: bool,
    searching: bool,
    pub(super) error: Option<String>,
}

#[derive(Default)]
pub(super) struct FindState {
    pub(super) prompt: Option<VimSearch>,
    last: Option<Pattern>,
    backwards: bool,
    pub(super) highlighted: bool,
    last_escape: Option<f64>,
    cache: crate::search::MatchCache<Pattern>,
    pub(super) view_scroll: HashMap<(u64, u64), (egui::Id, Vec2)>,
    pub(super) restore_scroll: HashMap<(u64, u64), Vec2>,
    message: Option<(String, bool)>,
    current_pane: u64,
    current_match: Option<SearchIdentity>,
    pending_yank: Option<crate::vim::Yank>,
    worker: Option<worker::Worker>,
    context: Option<egui::Context>,
    pinned: HashMap<u64, PinnedMatches>,
    pending_navigation: Option<PendingNavigation>,
    deferred_events: Vec<egui::Event>,
    dispatch_poll_frame: Option<u64>,
    toolbar_navigation: bool,
}

impl FindState {
    /// A repeated held key is not a second Escape. Only consecutive key presses
    /// suppress highlights; the last pattern and logical match remain intact.
    pub(super) fn observe_escape(&mut self, event: &egui::Event, time: f64) {
        match event {
            egui::Event::Key {
                key: Key::Escape,
                pressed: true,
                repeat: false,
                ..
            } => {
                if self
                    .last_escape
                    .is_some_and(|previous| (0.0..=0.75).contains(&(time - previous)))
                {
                    self.highlighted = false;
                    self.last_escape = None;
                } else {
                    self.last_escape = Some(time);
                }
            }
            egui::Event::Key {
                key: Key::Escape, ..
            } => {}
            egui::Event::Key { pressed: true, .. }
            | egui::Event::Text(_)
            | egui::Event::Paste(_)
            | egui::Event::Copy
            | egui::Event::Cut
            | egui::Event::Ime(egui::ImeEvent::Commit(_))
            | egui::Event::PointerButton { pressed: true, .. } => self.last_escape = None,
            _ => {}
        }
    }

    pub(super) fn break_escape_sequence(&mut self) {
        self.last_escape = None;
    }

    fn matches(
        &mut self,
        tab: &EditorTab,
        pattern: &Pattern,
    ) -> Option<crate::search::MatchResult> {
        let key = worker::Key {
            document: tab.id,
            revision: tab.revision,
            byte_len: tab.text.len(),
            pattern: pattern.clone(),
        };
        self.collect_results();
        if let Some(entry) = self.pinned.get(&tab.id).filter(|entry| entry.key == key) {
            return Some((entry.matches.clone(), entry.error.clone()));
        }
        if let Some(result) = self
            .cache
            .get(tab.id, tab.revision, tab.text.len(), pattern)
        {
            return Some(result);
        }
        if tab.text.len() > ASYNC_DOCUMENT_BYTES && !pattern.query.is_empty() {
            let context = self
                .context
                .get_or_insert_with(egui::Context::default)
                .clone();
            self.worker
                .get_or_insert_with(|| worker::Worker::new(context))
                .request(key, &tab.text);
            return None;
        }
        if let Some(worker) = &self.worker {
            worker.cancel(tab.id);
        }
        let (matches, error): crate::search::MatchResult = match crate::search::search(
            &tab.text,
            &pattern.query,
            pattern.case_sensitive,
            pattern.whole_word,
            pattern.regex,
        ) {
            Ok(matches) => (Arc::from(matches), None),
            Err(error) => (Arc::from(Vec::<Range<usize>>::new()), Some(error)),
        };
        self.cache.insert(
            tab.id,
            tab.revision,
            tab.text.len(),
            pattern.clone(),
            matches.clone(),
            error.clone(),
        );
        Some((matches, error))
    }

    fn collect_results(&mut self) {
        while let Some(result) = self.worker.as_ref().and_then(worker::Worker::take_result) {
            self.cache.insert(
                result.key.document,
                result.key.revision,
                result.key.byte_len,
                result.key.pattern,
                result.matches,
                result.error,
            );
        }
    }

    pub(super) fn waiting(&self) -> bool {
        self.prompt.as_ref().is_some_and(|prompt| prompt.confirming)
            || self
                .pending_navigation
                .as_ref()
                .is_some_and(|pending| pending.modal)
    }

    fn poll_deferred(&mut self, tab: &mut EditorTab) {
        self.collect_results();
        if let Some(pending) = self.pending_navigation.take() {
            if pending.key.document == tab.id
                && pending.pane == self.current_pane
                && pending.key.revision == tab.revision
                && pending.navigation.cursor == tab.vim.cursor
                && pending.motion_epoch == tab.vim.motion_epoch()
                && pending.selection == tab.selection
            {
                if self.matches(tab, &pending.key.pattern).is_some() {
                    tab.vim.restore_navigation(&tab.text, &pending.navigation);
                    self.navigate_from(
                        tab,
                        &pending.key.pattern,
                        pending.backwards,
                        pending.count,
                        pending.include_current,
                        pending.modal,
                        pending.origin,
                    );
                    if pending.preserve_focus {
                        tab.focus = false;
                    }
                } else {
                    self.pending_navigation = Some(pending);
                }
            } else {
                self.deferred_events.clear();
            }
        }
        if let Some((confirming, pattern)) = self
            .prompt
            .as_ref()
            .filter(|prompt| prompt.searching)
            .map(|prompt| {
                (
                    prompt.confirming,
                    if prompt.query.is_empty() {
                        self.last.clone()
                    } else {
                        Some(Pattern::vim(prompt.query.clone()))
                    },
                )
            })
            && pattern
                .as_ref()
                .is_some_and(|pattern| self.matches(tab, pattern).is_some())
        {
            if confirming {
                self.confirm(tab);
            } else {
                self.preview(tab);
            }
        }
    }

    fn pattern(&self) -> Option<Pattern> {
        self.prompt
            .as_ref()
            .filter(|prompt| !prompt.query.is_empty())
            .map(|prompt| Pattern::vim(prompt.query.clone()))
            .or_else(|| self.last.clone())
    }

    pub(super) fn invalidate(&mut self, document_id: u64) {
        self.cache.invalidate(document_id);
        self.pinned.remove(&document_id);
        if let Some(worker) = &self.worker {
            worker.cancel(document_id);
        }
        if self
            .current_match
            .as_ref()
            .is_some_and(|matched| matched.tab_id == document_id)
        {
            self.current_match = None;
        }
    }

    fn logical_cursor(&self, tab: &EditorTab, pattern: &Pattern) -> Option<usize> {
        self.current_match
            .as_ref()
            .filter(|matched| {
                matched.tab_id == tab.id
                    && matched.pane_id == self.current_pane
                    && matched.revision == tab.revision
                    && matched.pattern == *pattern
                    && matched.physical_cursor == tab.vim.cursor
                    && matched.motion_epoch == tab.vim.motion_epoch()
            })
            .map(|matched| matched.logical_start)
    }

    pub(super) fn take_yank(&mut self) -> Option<crate::vim::Yank> {
        self.pending_yank.take()
    }

    pub(super) fn active_cursor(&self, tab: &EditorTab, pane_id: u64) -> usize {
        if let Some(prompt) = &self.prompt
            && prompt.tab_id == tab.id
            && prompt.pane_id == pane_id
            && prompt.revision == tab.revision
        {
            return prompt.preview_match.unwrap_or(tab.vim.cursor);
        }
        if pane_id == self.current_pane
            && let Some(pattern) = &self.last
        {
            return self.logical_cursor(tab, pattern).unwrap_or(tab.vim.cursor);
        }
        tab.vim.cursor
    }

    pub(super) fn start(&mut self, tab: &mut EditorTab, pane_id: u64, backwards: bool) {
        self.current_pane = pane_id;
        let count = tab.vim.take_search_count();
        self.prompt = Some(VimSearch {
            tab_id: tab.id,
            pane_id,
            revision: tab.revision,
            origin: tab.vim.navigation(),
            origin_selection: tab.selection.clone(),
            query: String::new(),
            query_cursor: 0,
            backwards,
            count,
            original_scroll: self.view_scroll.get(&(pane_id, tab.id)).copied(),
            original_match: self.current_match.clone(),
            preview_match: None,
            horizontal_scroll: 0.0,
            confirming: false,
            searching: false,
            error: None,
        });
    }

    pub(super) fn cancel(&mut self, tab: &mut EditorTab, focus: bool) {
        if self.waiting() || self.pending_navigation.is_some() {
            self.message = Some(("Search cancelled".into(), false));
        }
        let pending = self.pending_navigation.take();
        self.deferred_events.clear();
        if let Some(worker) = &self.worker {
            worker.cancel(tab.id);
        }
        let Some(prompt) = self.prompt.take() else {
            if pending.is_some_and(|pending| pending.modal && pending.key.document == tab.id) {
                tab.vim.cancel_pending_command();
            }
            return;
        };
        if prompt.tab_id != tab.id {
            return;
        }
        tab.vim.restore_navigation(&tab.text, &prompt.origin);
        tab.vim.cancel_pending_command();
        self.current_match = if tab.revision == prompt.revision {
            prompt.original_match
        } else {
            None
        };
        tab.selection = if tab.revision == prompt.revision {
            prompt.origin_selection
        } else {
            tab.vim
                .selection(&tab.text)
                .unwrap_or(tab.vim.cursor..tab.vim.cursor)
        };
        tab.pending_selection = Some(tab.selection.clone());
        tab.scroll_to_cursor = prompt.original_scroll.is_none();
        if let Some((_, offset)) = prompt.original_scroll {
            self.restore_scroll.insert((prompt.pane_id, tab.id), offset);
        }
        tab.focus = focus;
    }

    fn move_cursor(tab: &mut EditorTab, range: &Range<usize>, modal: bool) {
        if modal {
            tab.vim.set_cursor(&tab.text, range.start);
            tab.selection = tab
                .vim
                .selection(&tab.text)
                .unwrap_or(tab.vim.cursor..tab.vim.cursor);
            tab.pending_selection = Some(tab.selection.clone());
            tab.scroll_to_cursor = true;
        } else {
            tab.set_cursor(range.clone());
        }
    }

    fn preview(&mut self, tab: &mut EditorTab) {
        let Some(prompt) = self.prompt.as_ref() else {
            return;
        };
        let origin = prompt.origin.clone();
        let query = prompt.query.clone();
        let backwards = prompt.backwards;
        let count = prompt.count;
        tab.vim.restore_navigation(&tab.text, &origin);
        tab.selection = tab
            .vim
            .selection(&tab.text)
            .unwrap_or(tab.vim.cursor..tab.vim.cursor);
        tab.pending_selection = Some(tab.selection.clone());
        if query.is_empty() {
            tab.scroll_to_cursor = true;
            if let Some(prompt) = self.prompt.as_mut() {
                prompt.error = None;
                prompt.searching = false;
                prompt.preview_match = None;
            }
            if let Some(worker) = &self.worker {
                worker.cancel(tab.id);
            }
            return;
        }
        let pattern = Pattern::vim(query);
        let search_origin = self.logical_cursor(tab, &pattern).unwrap_or(origin.cursor);
        let Some((matches, error)) = self.matches(tab, &pattern) else {
            if let Some(prompt) = self.prompt.as_mut() {
                prompt.searching = true;
                prompt.error = None;
                prompt.preview_match = None;
            }
            return;
        };
        let error = if error.is_some() {
            error
        } else if matches.is_empty() {
            Some("Pattern not found".into())
        } else {
            None
        };
        let matched = crate::search::next_match(&matches, search_origin, backwards, count, false);
        if let Some((range, _)) = &matched {
            Self::move_cursor(tab, range, true);
        }
        if let Some(prompt) = self.prompt.as_mut() {
            prompt.error = error;
            prompt.searching = false;
            prompt.preview_match = matched.map(|(range, _)| range.start);
        }
    }

    #[cfg(test)]
    pub(super) fn character(&mut self, tab: &mut EditorTab, ch: char) {
        let mut buffer = [0; 4];
        self.text(tab, ch.encode_utf8(&mut buffer));
    }

    /// Paste/IME/text events are one query update and one scan, even when their
    /// contents contain thousands of characters.
    pub(super) fn text(&mut self, tab: &mut EditorTab, text: &str) {
        if self.waiting() {
            self.deferred_events.push(egui::Event::Text(text.into()));
            return;
        }
        let Some(prompt) = self.prompt.as_mut() else {
            return;
        };
        let inserted: String = text
            .chars()
            .filter(|ch| !ch.is_control())
            .take(MAX_QUERY_CHARS.saturating_sub(prompt.query.chars().count()))
            .collect();
        if inserted.is_empty() {
            return;
        }
        let byte = prompt
            .query
            .char_indices()
            .nth(prompt.query_cursor)
            .map_or(prompt.query.len(), |(byte, _)| byte);
        prompt.query.insert_str(byte, &inserted);
        prompt.query_cursor += inserted.chars().count();
        self.preview(tab);
    }

    pub(super) fn key(&mut self, tab: &mut EditorTab, key: Key, modifiers: egui::Modifiers) {
        if key == Key::Escape || modifiers.ctrl && key == Key::C {
            self.cancel(tab, true);
            return;
        }
        if self.waiting() {
            self.deferred_events.push(egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            });
            return;
        }
        if key == Key::Enter {
            self.confirm(tab);
            return;
        }
        let Some(prompt) = self.prompt.as_mut() else {
            return;
        };
        let len = prompt.query.chars().count();
        match key {
            Key::Backspace if prompt.query_cursor > 0 => {
                let start = prompt
                    .query
                    .char_indices()
                    .nth(prompt.query_cursor - 1)
                    .unwrap()
                    .0;
                let end = prompt
                    .query
                    .char_indices()
                    .nth(prompt.query_cursor)
                    .map_or(prompt.query.len(), |(byte, _)| byte);
                prompt.query.replace_range(start..end, "");
                prompt.query_cursor -= 1;
            }
            Key::Delete if prompt.query_cursor < len => {
                let start = prompt
                    .query
                    .char_indices()
                    .nth(prompt.query_cursor)
                    .unwrap()
                    .0;
                let end = prompt
                    .query
                    .char_indices()
                    .nth(prompt.query_cursor + 1)
                    .map_or(prompt.query.len(), |(byte, _)| byte);
                prompt.query.replace_range(start..end, "");
            }
            Key::ArrowLeft => prompt.query_cursor = prompt.query_cursor.saturating_sub(1),
            Key::ArrowRight => prompt.query_cursor = (prompt.query_cursor + 1).min(len),
            Key::Home => prompt.query_cursor = 0,
            Key::End => prompt.query_cursor = len,
            Key::U if modifiers.ctrl => {
                prompt.query.clear();
                prompt.query_cursor = 0;
            }
            _ => return,
        }
        self.preview(tab);
    }

    fn confirm(&mut self, tab: &mut EditorTab) {
        let pattern = self.prompt.as_ref().and_then(|prompt| {
            if prompt.query.is_empty() {
                self.last.clone()
            } else {
                Some(Pattern::vim(prompt.query.clone()))
            }
        });
        if let Some(pattern) = &pattern
            && self.matches(tab, pattern).is_none()
        {
            if let Some(prompt) = self.prompt.as_mut() {
                prompt.confirming = true;
                prompt.searching = true;
            }
            self.message = Some(("Searching… · Esc cancels".into(), false));
            return;
        }
        let Some(prompt) = self.prompt.take() else {
            return;
        };
        let pattern = if prompt.query.is_empty() {
            self.last.clone()
        } else {
            Some(Pattern::vim(prompt.query))
        };
        tab.vim.restore_navigation(&tab.text, &prompt.origin);
        if let Some(pattern) = pattern {
            self.last = Some(pattern.clone());
            self.backwards = prompt.backwards;
            self.highlighted = true;
            self.navigate(tab, &pattern, prompt.backwards, prompt.count, false, true);
        } else {
            tab.vim.cancel_pending_command();
            self.message = Some(("No previous search pattern".into(), true));
        }
        tab.selection = tab
            .vim
            .selection(&tab.text)
            .unwrap_or(tab.vim.cursor..tab.vim.cursor);
        tab.pending_selection = Some(tab.selection.clone());
        tab.focus = true;
    }

    fn navigate(
        &mut self,
        tab: &mut EditorTab,
        pattern: &Pattern,
        backwards: bool,
        count: usize,
        include_current: bool,
        modal: bool,
    ) -> Option<usize> {
        let origin = self.logical_cursor(tab, pattern).unwrap_or(tab.vim.cursor);
        self.navigate_from(
            tab,
            pattern,
            backwards,
            count,
            include_current,
            modal,
            origin,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn navigate_from(
        &mut self,
        tab: &mut EditorTab,
        pattern: &Pattern,
        backwards: bool,
        count: usize,
        include_current: bool,
        modal: bool,
        origin: usize,
    ) -> Option<usize> {
        let Some((matches, error)) = self.matches(tab, pattern) else {
            self.pending_navigation = Some(PendingNavigation {
                key: worker::Key {
                    document: tab.id,
                    revision: tab.revision,
                    byte_len: tab.text.len(),
                    pattern: pattern.clone(),
                },
                pane: self.current_pane,
                backwards,
                count,
                include_current,
                modal,
                origin,
                navigation: tab.vim.navigation(),
                selection: tab.selection.clone(),
                motion_epoch: tab.vim.motion_epoch(),
                preserve_focus: self.toolbar_navigation,
            });
            self.message = Some(("Searching… · Esc cancels".into(), false));
            return None;
        };
        if let Some(error) = error {
            tab.vim.cancel_pending_command();
            self.message = Some((error, true));
            return None;
        }
        let Some((range, wrapped)) =
            crate::search::next_match(&matches, origin, backwards, count, include_current)
        else {
            tab.vim.cancel_pending_command();
            self.message = Some((
                if pattern.query.is_empty() {
                    "No previous search pattern".into()
                } else {
                    format!("Pattern not found: {}", pattern.query)
                },
                true,
            ));
            return None;
        };
        let operator = modal && tab.vim.search_operator_pending();
        if modal {
            let action = tab.vim.apply_search_motion(
                &mut tab.text,
                range.start,
                crate::vim::SearchMotion {
                    query: pattern.query.clone(),
                    case_sensitive: pattern.case_sensitive,
                    whole_word: pattern.whole_word,
                    regex: pattern.regex,
                    backwards,
                    count,
                },
            );
            if action.text_changed {
                tab.changed();
            }
            if action.yanked.is_some() {
                if !action.text_changed {
                    if action.yanked.as_ref().is_some_and(|yank| yank.linewise) {
                        tab.flash_line_yank(action.yanked_ranges.clone());
                    } else {
                        tab.flash_yank(action.yanked_ranges.clone());
                    }
                }
                self.pending_yank = action.yanked;
            }
            tab.selection = tab
                .vim
                .selection(&tab.text)
                .unwrap_or(tab.vim.cursor..tab.vim.cursor);
            tab.pending_selection = Some(tab.selection.clone());
            tab.scroll_to_cursor = true;
        } else {
            Self::move_cursor(tab, &range, false);
        }
        self.current_match = if operator {
            None
        } else {
            Some(SearchIdentity {
                tab_id: tab.id,
                pane_id: self.current_pane,
                revision: tab.revision,
                pattern: pattern.clone(),
                logical_start: range.start,
                physical_cursor: tab.vim.cursor,
                motion_epoch: tab.vim.motion_epoch(),
            })
        };
        let index = matches.partition_point(|candidate| candidate.start < range.start) + 1;
        self.message = Some((
            format!(
                "{index}/{}{}",
                matches.len(),
                if wrapped {
                    if backwards {
                        " · wrapped to bottom"
                    } else {
                        " · wrapped to top"
                    }
                } else {
                    ""
                }
            ),
            false,
        ));
        Some(range.start)
    }

    pub(super) fn repeat(&mut self, tab: &mut EditorTab, opposite: bool) {
        let count = tab.vim.take_search_count();
        let Some(pattern) = self.last.clone() else {
            tab.vim.cancel_pending_command();
            self.message = Some(("No previous search pattern".into(), true));
            return;
        };
        self.highlighted = true;
        self.navigate(tab, &pattern, self.backwards ^ opposite, count, false, true);
    }

    pub(super) fn word(&mut self, tab: &mut EditorTab, backwards: bool) {
        let count = tab.vim.take_search_count();
        let chars: Vec<_> = tab.text.chars().collect();
        let word = crate::search::word_char;
        let mut start = tab.vim.cursor.min(chars.len());
        if chars.get(start).is_none_or(|ch| !word(*ch)) {
            while start < chars.len() && chars[start] != '\n' && !word(chars[start]) {
                start += 1;
            }
        }
        if chars.get(start).is_none_or(|ch| !word(*ch)) {
            tab.vim.cancel_pending_command();
            self.message = Some(("No word under cursor".into(), true));
            return;
        }
        let mut end = start;
        while start > 0 && word(chars[start - 1]) {
            start -= 1;
        }
        while end < chars.len() && word(chars[end]) {
            end += 1;
        }
        let pattern = Pattern {
            query: chars[start..end].iter().collect(),
            case_sensitive: true,
            whole_word: true,
            regex: false,
        };
        self.last = Some(pattern.clone());
        self.backwards = backwards;
        self.highlighted = true;
        // A cursor inside a word searches beyond that word, not its start again.
        self.navigate_from(tab, &pattern, backwards, count, false, true, start);
    }
}

impl MorrowApp {
    pub(super) fn find_search_running(&self) -> bool {
        self.tab().is_some_and(|tab| {
            self.find
                .worker
                .as_ref()
                .is_some_and(|worker| worker.pending(tab.id))
        })
    }
    pub(super) fn vim_search_active(&self) -> bool {
        self.find.prompt.is_some() || self.find.waiting()
    }

    pub(super) fn cancel_vim_search(&mut self, focus: bool) {
        let mut state = std::mem::take(&mut self.find);
        let pending_work = state.waiting() || state.pending_navigation.is_some();
        if let Some(id) = state
            .prompt
            .as_ref()
            .map(|prompt| prompt.tab_id)
            .or_else(|| {
                state
                    .pending_navigation
                    .as_ref()
                    .map(|pending| pending.key.document)
            })
        {
            for project in &mut self.projects {
                if let Some(tab) = project.tabs.iter_mut().find(|tab| tab.id == id) {
                    state.cancel(tab, focus);
                    break;
                }
            }
            // A tab may have been closed by its mouse button while work was pending.
            state.prompt = None;
            state.pending_navigation = None;
            state.deferred_events.clear();
            if let Some(worker) = &state.worker {
                worker.cancel(id);
            }
        }
        state.current_match = None;
        if pending_work {
            state.message = Some(("Search cancelled".into(), false));
        }
        self.find = state;
        self.find_message();
    }

    pub(super) fn validate_vim_search(&mut self) {
        let was_modal_waiting = self.find.waiting();
        let pane = self.current_pane_id();
        let tab_id = self.tab().map(|tab| tab.id);
        if self.find.current_pane != pane
            || self
                .find
                .current_match
                .as_ref()
                .is_some_and(|matched| Some(matched.tab_id) != tab_id)
        {
            self.find.current_match = None;
        }
        self.find.current_pane = pane;
        self.find.context = Some(self.ctx.clone());
        let frame = self.ctx.cumulative_pass_nr();
        self.find
            .pinned
            .retain(|_, entry| entry.frame.saturating_add(1) >= frame);
        let valid = self.find.prompt.as_ref().is_none_or(|prompt| {
            self.settings.vim
                && self.current_pane_id() == prompt.pane_id
                && self.tab().is_some_and(|tab| {
                    tab.id == prompt.tab_id
                        && tab.revision == prompt.revision
                        && tab.agent_view == crate::cli::DocumentView::Source
                })
        });
        let pending_valid = self.find.pending_navigation.as_ref().is_none_or(|pending| {
            (!pending.modal || self.settings.vim)
                && (!self.find_visible
                    || pending.key.pattern.regex
                    || pending.key.pattern
                        == Pattern::literal(self.find_text.clone(), self.find_options()))
                && pane == pending.pane
                && self.tab().is_some_and(|tab| {
                    tab.id == pending.key.document
                        && tab.revision == pending.key.revision
                        && tab.vim.cursor == pending.navigation.cursor
                        && tab.vim.motion_epoch() == pending.motion_epoch
                        && tab.selection == pending.selection
                        && tab.agent_view == crate::cli::DocumentView::Source
                })
        });
        if !valid || !pending_valid {
            self.cancel_vim_search(false);
        }
        let blocked = egui::Popup::is_any_open(&self.ctx)
            || self.palette.is_some()
            || self.picker.is_some()
            || self.settings_open
            || self.save_as.is_some()
            || self.close_action.is_some()
            || self.vim_command.is_some()
            || self.diff.is_some()
            || self.git_detail_open()
            || self.navigation_modal_open()
            || self.navigation_pending()
            || self.screen_jump.is_some()
            || self.context_file_dialog.is_some();
        if blocked {
            return;
        }
        let modal_waiting = self.find.waiting();
        let has_pending =
            was_modal_waiting || modal_waiting || self.find.pending_navigation.is_some();
        let escape = has_pending
            && self.ctx.input(|input| {
                input.events.iter().any(|event| {
                    matches!(
                        event,
                        egui::Event::Copy
                            | egui::Event::Key {
                                key: Key::Escape,
                                pressed: true,
                                ..
                            }
                            | egui::Event::Key {
                                key: Key::C,
                                pressed: true,
                                modifiers: egui::Modifiers { ctrl: true, .. },
                                ..
                            }
                    )
                })
            });
        if has_pending && (escape || self.ctx.input(|input| input.pointer.any_pressed())) {
            self.cancel_vim_search(false);
            if was_modal_waiting || modal_waiting {
                // The first cancellation belongs to this search. Preserve any
                // following Escape so the ordinary dispatcher can recognize a
                // second press, including two presses in the same input batch.
                let consumed = self.ctx.input_mut(|input| {
                    let index = input.events.iter().position(|event| {
                        matches!(
                            event,
                            egui::Event::Copy
                                | egui::Event::PointerButton { pressed: true, .. }
                                | egui::Event::Key {
                                    key: Key::Escape,
                                    pressed: true,
                                    ..
                                }
                                | egui::Event::Key {
                                    key: Key::C,
                                    pressed: true,
                                    modifiers: egui::Modifiers { ctrl: true, .. },
                                    ..
                                }
                        )
                    })?;
                    // Input before cancellation belonged to the waiting query,
                    // and must never become source commands after it closes.
                    let prefix: Vec<_> = input.events.drain(..=index).collect();
                    let (consumed, retained): (Vec<_>, Vec<_>) =
                        prefix.into_iter().partition(|event| {
                            matches!(
                                event,
                                egui::Event::Text(_)
                                    | egui::Event::Paste(_)
                                    | egui::Event::Ime(_)
                                    | egui::Event::Key { .. }
                                    | egui::Event::Copy
                                    | egui::Event::Cut
                            )
                        });
                    drop(input.events.splice(0..0, retained));
                    Some((consumed, input.time))
                });
                if let Some((events, time)) = consumed {
                    for event in events {
                        self.find.observe_escape(&event, time);
                    }
                }
            }
            return;
        }
        // The second validation in handle_vim still cancels invalid context,
        // but may not publish events after the shortcut dispatcher has run.
        let frame = self.ctx.cumulative_pass_nr();
        if self.find.dispatch_poll_frame == Some(frame) {
            return;
        }
        self.find.dispatch_poll_frame = Some(frame);
        let mut state = std::mem::take(&mut self.find);
        if let Some(tab) = self.tab_mut() {
            state.poll_deferred(tab);
        }
        if let Some(yank) = state.take_yank() {
            self.register = Some(yank);
        }
        if !state.waiting() && !state.deferred_events.is_empty() {
            let mut queued = std::mem::take(&mut state.deferred_events);
            self.ctx.input_mut(|input| {
                queued.append(&mut input.events);
                input.events = queued;
            });
        }
        self.find = state;
        self.find_message();
    }

    pub(super) fn find_options(&self) -> editor::SearchOptions {
        editor::SearchOptions {
            case_sensitive: self.find_case,
            whole_word: self.find_word,
        }
    }

    pub(super) fn find_next(&mut self, backwards: bool) {
        let pane = self.current_pane_id();
        if self.find.current_pane != pane {
            self.find.current_match = None;
        }
        self.find.current_pane = pane;
        self.find.context = Some(self.ctx.clone());
        let pattern = if self.find_visible {
            Pattern::literal(self.find_text.clone(), self.find_options())
        } else {
            self.find
                .last
                .clone()
                .unwrap_or_else(|| Pattern::literal(self.find_text.clone(), self.find_options()))
        };
        let include_current = self
            .tab()
            .is_none_or(|tab| self.find.logical_cursor(tab, &pattern).is_none())
            && (self.find_match.is_none()
                || self
                    .find
                    .last
                    .as_ref()
                    .is_some_and(|previous| previous != &pattern));
        let modal = !self.find_visible
            && self.settings.vim
            && self
                .tab()
                .is_some_and(|tab| tab.vim.mode != VimMode::Insert);
        let mut state = std::mem::take(&mut self.find);
        state.last = Some(pattern.clone());
        state.backwards = backwards;
        state.highlighted = true;
        state.toolbar_navigation = self.find_visible;
        if self.find_visible
            && let Some(tab) = self.tab_mut()
        {
            tab.vim.cancel_pending_command();
        }
        self.find_match = self
            .tab_mut()
            .and_then(|tab| state.navigate(tab, &pattern, backwards, 1, include_current, modal));
        if self.find_visible
            && let Some(tab) = self.tab_mut()
        {
            tab.focus = false;
        }
        state.toolbar_navigation = false;
        self.find = state;
        self.find_message();
    }

    pub(super) fn find_message(&mut self) {
        if let Some((message, error)) = self.find.message.take() {
            self.status(message, error);
        }
    }

    pub(super) fn editor_search_matches(&mut self) -> Arc<[Range<usize>]> {
        let pattern = if self.find_visible {
            Some(Pattern::literal(
                self.find_text.clone(),
                self.find_options(),
            ))
        } else if self.find.highlighted || self.find.prompt.is_some() {
            self.find.pattern()
        } else {
            None
        };
        let Some(pattern) = pattern else {
            return Arc::from([]);
        };
        let mut state = std::mem::take(&mut self.find);
        state.context = Some(self.ctx.clone());
        let frame = self.ctx.cumulative_pass_nr();
        state
            .pinned
            .retain(|_, entry| entry.frame.saturating_add(1) >= frame);
        let matches = self.tab().map_or_else(
            || Arc::from([]),
            |tab| {
                if let Some((matches, error)) = state.matches(tab, &pattern) {
                    state.pinned.insert(
                        tab.id,
                        PinnedMatches {
                            key: worker::Key {
                                document: tab.id,
                                revision: tab.revision,
                                byte_len: tab.text.len(),
                                pattern: pattern.clone(),
                            },
                            matches: matches.clone(),
                            error,
                            frame,
                        },
                    );
                    matches
                } else {
                    Arc::from([])
                }
            },
        );
        self.find = state;
        matches
    }

    pub(super) fn vim_search_ui(&mut self, ui: &mut egui::Ui) {
        let Some(prompt) = self.find.prompt.as_ref() else {
            return;
        };
        let query = prompt.query.clone();
        let cursor = prompt.query_cursor;
        let backwards = prompt.backwards;
        let error = prompt.error.clone();
        let active_match = prompt.preview_match;
        let searching = prompt.searching;
        let mut horizontal_scroll = prompt.horizontal_scroll;
        let matches = self.editor_search_matches();
        let current = active_match.map_or(0, |cursor| {
            let index = matches.partition_point(|range| range.start < cursor);
            if matches
                .get(index)
                .is_some_and(|range| range.start == cursor)
            {
                index + 1
            } else {
                0
            }
        });
        egui::Panel::bottom("vim_search_command")
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(Margin::symmetric(12, 7)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(if backwards { "?" } else { "/" })
                            .monospace()
                            .color(ACCENT),
                    );
                    let show_hint = ui.available_width() > 560.0;
                    let available =
                        (ui.available_width() - if show_hint { 280.0 } else { 110.0 }).max(40.0);
                    let before: String = query.chars().take(cursor).collect();
                    let after: String = query.chars().skip(cursor).collect();
                    let font = egui::TextStyle::Monospace.resolve(ui.style());
                    let galley = ui.painter().layout_no_wrap(
                        format!("{before}▏{after}"),
                        font.clone(),
                        TEXT,
                    );
                    let caret = ui
                        .painter()
                        .layout_no_wrap(format!("{before}▏"), font, TEXT)
                        .size()
                        .x;
                    let (rect, response) =
                        ui.allocate_exact_size(Vec2::new(available, 18.0), egui::Sense::hover());
                    horizontal_scroll =
                        prompt_scroll(horizontal_scroll, caret, available, galley.size().x);
                    let position = egui::pos2(
                        rect.left() - horizontal_scroll,
                        rect.center().y - galley.size().y / 2.0,
                    );
                    ui.painter()
                        .with_clip_rect(rect.intersect(ui.clip_rect()))
                        .galley(position, galley, TEXT);
                    if !self.find.waiting() {
                        ui.output_mut(|output| {
                            output.ime = Some(egui::output::IMEOutput {
                                purpose: egui::IMEPurpose::Normal,
                                rect,
                                cursor_rect: egui::Rect::from_min_size(
                                    egui::pos2(position.x + caret, rect.top()),
                                    egui::vec2(1.0, rect.height()),
                                )
                                .intersect(rect),
                                should_interrupt_composition: false,
                            })
                        });
                    }
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::TextEdit,
                            true,
                            "Vim search pattern",
                        )
                    });
                    let label = error
                        .as_deref()
                        .map(|error| {
                            if error.len() > 80 {
                                "Invalid search pattern"
                            } else {
                                error
                            }
                        })
                        .map(str::to_owned)
                        .unwrap_or_else(|| {
                            if searching {
                                "Searching…".into()
                            } else {
                                format!("{current}/{}", matches.len())
                            }
                        });
                    ui.label(RichText::new(label).size(11.0).color(if error.is_some() {
                        RED
                    } else {
                        MUTED
                    }))
                    .on_hover_text(error.unwrap_or_else(|| {
                        "Regex search · case sensitive · \\c ignores case, \\C matches case".into()
                    }));
                    if show_hint {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(
                                RichText::new("Enter confirm · Esc cancel")
                                    .size(11.0)
                                    .color(MUTED),
                            );
                        });
                    }
                });
            });
        if let Some(prompt) = self.find.prompt.as_mut() {
            prompt.horizontal_scroll = horizontal_scroll;
        }
    }
}

/// Keep the insertion marker visible during both editing and Home/End motion.
fn prompt_scroll(previous: f32, caret: f32, width: f32, text_width: f32) -> f32 {
    let margin = 10.0_f32.min(width / 4.0);
    let mut scroll = previous;
    if caret > scroll + width - margin {
        scroll = caret - width + margin;
    } else if caret < scroll + margin {
        scroll = caret - margin;
    }
    scroll.clamp(0.0, (text_width - width + margin).max(0.0))
}

pub(super) fn paint_matches(
    ui: &egui::Ui,
    galley: &egui::Galley,
    position: egui::Pos2,
    visible: &[(usize, crate::render::VisualRow)],
    matches: &[Range<usize>],
    cursor: usize,
) {
    let clip = ui.clip_rect();
    let painter = ui.painter().with_clip_rect(clip);
    for &(row, mapped) in visible {
        let columns = visible_match_columns(galley, row, mapped, position, clip);
        let begin = matches.partition_point(|range| {
            range.end < columns.start || range.end == columns.start && !range.is_empty()
        });
        for range in matches[begin..]
            .iter()
            .take_while(|range| range.start <= columns.end)
        {
            if let Some(segment) =
                crate::render::range_on_visual_row(galley, row, mapped, range.start, range.end)
            {
                let rect = segment.translate(position.to_vec2());
                if !rect.intersects(clip) {
                    continue;
                }
                let current = range.start == cursor;
                painter.rect_filled(
                    rect,
                    1.5,
                    if current {
                        Color32::from_rgba_unmultiplied(237, 191, 126, 85)
                    } else {
                        Color32::from_rgba_unmultiplied(147, 174, 255, 38)
                    },
                );
                if current {
                    painter.rect_stroke(
                        rect,
                        1.5,
                        Stroke::new(1.0, ORANGE.linear_multiply(0.8)),
                        egui::StrokeKind::Inside,
                    );
                }
            }
        }
    }
}

/// Bound painting even when a single unwrapped row has millions of hits.
/// Glyph positions are row-relative and include whitespace. The extra left
/// margin preserves the minimum six-pixel mark used for zero-width matches.
fn visible_match_columns(
    galley: &egui::Galley,
    row_index: usize,
    mapped: crate::render::VisualRow,
    position: egui::Pos2,
    clip: egui::Rect,
) -> Range<usize> {
    let row = &galley.rows[row_index];
    let left = clip.left() - position.x - row.pos.x - 6.0;
    let right = clip.right() - position.x - row.pos.x;
    let first = row
        .glyphs
        .partition_point(|glyph| glyph.pos.x < left)
        .saturating_sub(1);
    let last = row.glyphs.partition_point(|glyph| glyph.pos.x <= right);
    (mapped.start + first).min(mapped.end)..(mapped.start + last).min(mapped.end)
}

#[cfg(test)]
#[path = "find_tests.rs"]
mod tests;

#[cfg(test)]
mod escape_tests {
    use super::*;

    fn escape(repeat: bool) -> egui::Event {
        egui::Event::Key {
            key: Key::Escape,
            physical_key: None,
            pressed: true,
            repeat,
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn only_two_consecutive_escape_presses_hide_the_retained_pattern() {
        let mut state = FindState {
            highlighted: true,
            last: Some(Pattern::vim("🦀".into())),
            ..Default::default()
        };
        state.observe_escape(&escape(false), 1.0);
        state.observe_escape(&escape(true), 1.1);
        assert!(state.highlighted);
        state.observe_escape(&egui::Event::Text("j".into()), 1.2);
        state.observe_escape(&escape(false), 1.3);
        assert!(state.highlighted);
        state.observe_escape(&escape(false), 2.2);
        assert!(
            state.highlighted,
            "widely separated Escape presses are independent"
        );
        state.observe_escape(&escape(false), 2.3);
        assert!(!state.highlighted);
        assert_eq!(state.pattern(), Some(Pattern::vim("🦀".into())));
        assert!(state.prompt.is_none());
    }

    #[test]
    fn async_query_cancellation_discards_query_input_and_preserves_second_escape() {
        let ctx = egui::Context::default();
        let mut app = MorrowApp::empty(ctx.clone());
        let mut project = Project::new(std::env::temp_dir().join("morrow-async-escape-unit"));
        project.last_git_refresh = Instant::now();
        let mut tab = EditorTab::new(None, "search.txt".into());
        tab.text = format!("start foo {}", "a".repeat(ASYNC_DOCUMENT_BYTES));
        tab.saved_text = tab.text.clone();
        project.tabs.push(tab);
        app.projects.push(project);
        app.bind_current_pane();
        let pane = app.current_pane_id();
        let original = app.tab().unwrap().text.clone();
        let mut state = std::mem::take(&mut app.find);
        state.worker = Some(worker::Worker::manual());
        state.context = Some(ctx.clone());
        state.last = Some(Pattern::vim("start".into()));
        state.highlighted = true;
        let tab = app.tab_mut().unwrap();
        state.start(tab, pane, false);
        state.text(tab, "foo");
        state.key(tab, Key::Enter, egui::Modifiers::NONE);
        assert!(state.waiting());
        app.find = state;
        let mut output = ctx.run_ui(
            egui::RawInput {
                time: Some(1.0),
                events: vec![
                    egui::Event::Text("dd".into()),
                    egui::Event::Paste("must not edit".into()),
                    escape(false),
                    egui::Event::Key {
                        key: Key::Escape,
                        physical_key: None,
                        pressed: false,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    },
                    escape(false),
                ],
                ..Default::default()
            },
            |_| {
                app.validate_vim_search();
                assert!(!app.vim_search_active());
                assert!(app.find.highlighted);
                let remaining = ctx.input(|input| input.events.clone());
                assert_eq!(remaining.len(), 2);
                assert!(matches!(
                    remaining[0],
                    egui::Event::Key {
                        key: Key::Escape,
                        ..
                    }
                ));
                for event in remaining {
                    app.find.observe_escape(&event, 1.0);
                }
                assert!(!app.find.highlighted);
                assert_eq!(app.find.pattern(), Some(Pattern::vim("start".into())));
                assert_eq!(app.tab().unwrap().text, original);
                assert!(!app.tab().unwrap().vim.can_undo());
            },
        );
        output.textures_delta.clear();
    }

    #[test]
    fn async_query_pointer_cancel_discards_preceding_query_commands() {
        let ctx = egui::Context::default();
        let mut app = MorrowApp::empty(ctx.clone());
        let mut project =
            Project::new(std::env::temp_dir().join("morrow-async-pointer-cancel-unit"));
        project.last_git_refresh = Instant::now();
        let mut tab = EditorTab::new(None, "search.txt".into());
        tab.text = format!("start foo {}", "a".repeat(ASYNC_DOCUMENT_BYTES));
        tab.saved_text = tab.text.clone();
        project.tabs.push(tab);
        app.projects.push(project);
        app.bind_current_pane();
        let pane = app.current_pane_id();
        let original = app.tab().unwrap().text.clone();
        let mut state = std::mem::take(&mut app.find);
        state.worker = Some(worker::Worker::manual());
        state.context = Some(ctx.clone());
        let tab = app.tab_mut().unwrap();
        state.start(tab, pane, false);
        state.text(tab, "foo");
        state.key(tab, Key::Enter, egui::Modifiers::NONE);
        assert!(state.waiting());
        app.find = state;
        let pointer = egui::Event::PointerButton {
            pos: egui::pos2(10.0, 10.0),
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        };
        let mut output = ctx.run_ui(
            egui::RawInput {
                events: vec![egui::Event::Text("dd".into()), pointer.clone()],
                ..Default::default()
            },
            |_| {
                app.validate_vim_search();
                assert!(!app.vim_search_active());
                ctx.input(|input| assert_eq!(input.events, vec![pointer.clone()]));
                assert_eq!(app.tab().unwrap().text, original);
                assert!(!app.tab().unwrap().vim.can_undo());
            },
        );
        output.textures_delta.clear();
    }
}

#[cfg(test)]
#[path = "find_async_tests.rs"]
mod async_tests;

#[cfg(test)]
#[path = "find_paint_tests.rs"]
mod paint_tests;

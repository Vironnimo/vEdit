//! Insert transactions, shared Undo/Redo and repeat of completed changes.
use super::*;

/// Opaque history boundary for a macro spanning native input and navigation.
#[derive(Debug)]
pub struct MacroHistory {
    origin: Snapshot,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
}

impl VimState {
    pub fn take_command_failure(&mut self) -> bool {
        std::mem::take(&mut self.command_failed)
    }
    pub fn macro_checkpoint(&self, text: &str) -> MacroHistory {
        MacroHistory {
            origin: self.snapshot(text),
            undo: self.undo.clone(),
            redo: self.redo.clone(),
        }
    }

    /// Individual commands already moved marks and built dot recipes. Merge
    /// only their history after the macro has finished in this Document.
    pub fn finish_macro(&mut self, text: &str, checkpoint: MacroHistory) {
        self.undo = checkpoint.undo;
        if checkpoint.origin.text == text {
            self.redo = checkpoint.redo;
        } else {
            self.redo.clear();
            self.undo.push(checkpoint.origin);
            trim_history(&mut self.undo);
        }
    }
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty() || self.insert_origin.is_some()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Call immediately before an external operation changes the document.
    /// Insert-mode edits stay in the current insert transaction; other modes
    /// get a bounded undo snapshot and return to normal mode.
    pub fn begin_external_edit(&mut self, text: &str) {
        if self.mode == VimMode::Insert {
            if self.insert_origin.is_none() {
                self.insert_origin = Some(self.snapshot(text));
            } else if let Some(origin) = &mut self.insert_origin
                && origin.text == text
            {
                origin.multi = self.multi.clone();
            }
            return;
        }
        let before = self.snapshot(text);
        self.multi_cursors.clear();
        self.leave_visual();
        self.cancel_pending_command();
        self.insert_origin = None;
        self.insert_replay = None;
        self.insert_count = 1;
        self.record(before, text);
        self.external_edit = true;
    }

    pub(super) fn visual_repeat(&self, text: &str, range: &Range<usize>) -> VisualRepeat {
        let chars: Vec<_> = text.chars().collect();
        let rows = lines(&chars);
        let first = row_for_cursor(&rows, range.start);
        let last = if self.mode == VimMode::VisualLine {
            row_for_cursor(
                &rows,
                self.cursor
                    .max(self.visual_anchor.unwrap_or(self.cursor))
                    .min(chars.len()),
            )
        } else {
            row_for_cursor(&rows, range.end.saturating_sub(1).max(range.start))
        };
        VisualRepeat {
            linewise: self.mode == VimMode::VisualLine,
            blockwise: false,
            rows: last.saturating_sub(first) + 1,
            columns: if first == last {
                range.len()
            } else {
                range.end - rows[last].start
            },
            includes_newline: range.end > range.start && chars.get(range.end - 1) == Some(&'\n'),
            to_line_end: self.visual_to_line_end,
        }
    }

    fn select_repeat_visual(&mut self, text: &str, visual: &VisualRepeat) {
        if visual.blockwise {
            self.select_repeat_block(text, visual);
            return;
        }
        let chars: Vec<_> = text.chars().collect();
        let rows = lines(&chars);
        let first = row_for_cursor(&rows, self.cursor);
        let last = first.saturating_add(visual.rows - 1).min(rows.len() - 1);
        let line = rows[last];
        self.cancel_pending_command();
        self.visual_to_line_end = visual.to_line_end;
        self.visual_anchor = Some(self.cursor);
        self.mode = if visual.linewise {
            VimMode::VisualLine
        } else {
            VimMode::Visual
        };
        self.cursor = if visual.linewise {
            line.start
        } else {
            let start = if visual.rows == 1 {
                self.cursor
            } else {
                line.start
            };
            let limit = if visual.includes_newline
                && let Some(newline) = line.newline
            {
                newline
            } else {
                line.end.saturating_sub(1).max(line.start)
            };
            if visual.to_line_end {
                limit
            } else {
                start
                    .saturating_add(visual.columns.saturating_sub(1))
                    .min(limit)
            }
        };
    }

    /// Commit the insert session before e.g. switching documents.
    pub fn finish_insert(&mut self, text: &mut String) -> Action {
        if self.mode == VimMode::Insert {
            self.handle_key(text, VimKey::Escape)
        } else {
            Action::default()
        }
    }

    /// Windows undo/redo uses the same history as Vim, while keeping an Insert
    /// caret at its insertion boundary. Committing with Escape here would move
    /// the caret left and turn later text in the same input batch into motions.
    pub fn history_edit(&mut self, text: &mut String, redo: bool) -> Action {
        let old_cursor = self.cursor;
        let old_mode = self.mode;
        let replacing = self.is_replacing();
        let replace_keys = self.replace_change_keys();
        let inserting = old_mode == VimMode::Insert;
        let mut insert_expanded =
            inserting && self.multi.active() && self.finish_multi_insert_history(text).text_changed;
        if inserting {
            let inserted_delta = self
                .insert_replay
                .as_ref()
                .map(|replay| insert_delta(&replay.baseline, text, replay.cursor));
            insert_expanded |= self.finish_insert_repetitions(text);
            let changed = self
                .insert_origin
                .as_ref()
                .is_some_and(|origin| origin.text != *text);
            if let Some(origin) = self.insert_origin.take()
                && changed
            {
                self.record(origin, text);
            }
            if let Some(replay) = self.insert_replay.take()
                && changed
            {
                self.last_change = Some(RepeatChange {
                    keys: replace_keys.unwrap_or(replay.keys),
                    insert: if replacing { None } else { inserted_delta },
                    search: replay.search,
                    visual: replay.visual,
                });
            }
            self.insert_count = 1;
        }
        self.leave_visual();
        self.cancel_pending_command();
        let text_changed = self.restore(text, redo) || insert_expanded;
        self.preferred_column = None;
        if inserting {
            if replacing {
                self.start_replace(text, 1);
            } else {
                self.start_insert(text, self.cursor);
            }
            self.insert_replay = Some(InsertReplay {
                keys: vec![VimKey::Char(if replacing { 'R' } else { 'i' })],
                baseline: text.clone(),
                cursor: self.cursor,
                search: None,
                visual: None,
            });
        }
        self.clamp_cursor(text);
        if text_changed || self.cursor != old_cursor {
            self.motion_epoch = self.motion_epoch.wrapping_add(1);
        }
        Action {
            consumed: true,
            text_changed,
            cursor_changed: self.cursor != old_cursor,
            mode_changed: self.mode != old_mode,
            ..Action::default()
        }
    }

    pub fn insert_text(&mut self, text: &mut String, inserted: &str) -> Action {
        if self.multi.active() && self.is_replacing() {
            return self.multi_replace_text(text, inserted);
        }
        if self.is_replacing() {
            return self.replace_text(text, inserted);
        }
        if self.mode != VimMode::Insert || inserted.is_empty() {
            return Action::default();
        }
        let old_cursor = self.cursor;
        let at = self.cursor.min(text.chars().count());
        text.insert_str(char_to_byte(text, at), inserted);
        self.cursor = at + inserted.chars().count();
        self.preferred_column = None;
        Action {
            consumed: true,
            text_changed: true,
            cursor_changed: self.cursor != old_cursor,
            ..Action::default()
        }
    }

    pub(super) fn handle_insert_key(
        &mut self,
        text: &mut String,
        key: VimKey,
    ) -> (bool, bool, Option<Yank>) {
        if self.insert_register_pending() {
            self.pending = None;
            if let VimKey::Char(name) = key {
                let action = self.insert_register(text, name);
                self.message = action.message;
                return (true, action.text_changed, None);
            }
            return (true, false, None);
        }
        if self.is_replacing() {
            return self.handle_replace_key(text, key);
        }
        match key {
            VimKey::Escape => {
                let block_inserting = self.block_insert.is_some();
                let changed = self.finish_insert_repetitions(text);
                let insert_boundary = self.cursor;
                let chars: Vec<char> = text.chars().collect();
                let rows = lines(&chars);
                if self.cursor > rows[row_for_cursor(&rows, self.cursor)].start {
                    self.cursor -= 1;
                }
                if block_inserting {
                    self.snap_block_position(text);
                }
                if let Some(before) = self.insert_origin.take()
                    && before.text != *text
                {
                    self.record(before, text);
                }
                self.marks.insert = Some(insert_boundary);
                self.mode = VimMode::Normal;
                self.clear_command();
                (true, changed, None)
            }
            VimKey::Char(ch) => (
                true,
                self.insert_text(text, &ch.to_string()).text_changed,
                None,
            ),
            VimKey::Enter => {
                let ending = line_ending(text);
                (true, self.insert_text(text, ending).text_changed, None)
            }
            VimKey::Tab => (true, self.insert_text(text, "\t").text_changed, None),
            VimKey::Backspace => {
                if self.cursor == 0 {
                    return (true, false, None);
                }
                let end = char_to_byte(text, self.cursor);
                let count = if text[..end].ends_with("\r\n") { 2 } else { 1 };
                let range = char_to_byte(text, self.cursor - count)..end;
                text.replace_range(range, "");
                self.cursor -= count;
                (true, true, None)
            }
            VimKey::Redo
            | VimKey::Increment
            | VimKey::Decrement
            | VimKey::VisualBlock
            | VimKey::Left
            | VimKey::Right
            | VimKey::Up
            | VimKey::Down
            | VimKey::Home
            | VimKey::End
            | VimKey::Delete => (false, false, None),
        }
    }

    /// Finalize a counted insertion once for Escape and Windows history alike.
    /// The repeat delta is captured by callers before this expands the text.
    fn finish_insert_repetitions(&mut self, text: &mut String) -> bool {
        if self.is_replacing() {
            return self.finish_replace_repetitions(text);
        }
        let mut changed = self.finish_block_insert(text);
        let line_prefix = self.insert_line_prefix.take();
        if self.insert_count > 1
            && let Some(replay) = &self.insert_replay
        {
            let delta = insert_delta(&replay.baseline, text, replay.cursor);
            // A counted o/O opens one line per copy, as in Vim.
            let unit = format!("{}{}", line_prefix.unwrap_or_default(), delta.inserted);
            if delta.removed == 0
                && !unit.is_empty()
                && let Some(repeated) = repeated_text(&unit, self.insert_count - 1)
                && text.len().saturating_add(repeated.len()) <= MAX_PASTE_BYTES
            {
                text.insert_str(char_to_byte(text, self.cursor), &repeated);
                self.cursor += repeated.chars().count();
                changed = true;
            }
        }
        self.insert_count = 1;
        changed
    }

    pub(super) fn snapshot(&self, text: &str) -> Snapshot {
        Snapshot {
            text: text.to_owned(),
            cursor: self.cursor,
            multi: self.multi.clone(),
            marks: self.marks.named,
        }
    }

    pub(super) fn record(&mut self, before: Snapshot, text: &str) {
        if self.replaying {
            return;
        }
        self.track_marks(&before.text, text, true, before.cursor);
        self.redo.clear();
        self.undo.push(before);
        trim_history(&mut self.undo);
    }

    pub(super) fn start_insert(&mut self, text: &str, cursor: usize) {
        self.replace = None;
        self.insert_origin = Some(self.snapshot(text));
        self.mode = VimMode::Insert;
        self.cursor = cursor.min(text.chars().count());
        self.visual_anchor = None;
        self.insert_count = 1;
        self.insert_line_prefix = None;
        self.clear_command();
    }

    pub(super) fn open_line(&mut self, text: &mut String, above: bool) {
        let chars: Vec<char> = text.chars().collect();
        let rows = lines(&chars);
        let line = rows[row_for_cursor(&rows, self.cursor)];
        let before = self.snapshot(text);
        if above {
            let indent: String = chars[line.start..line.end]
                .iter()
                .copied()
                .take_while(|ch| *ch == ' ' || *ch == '\t')
                .collect();
            let ending = line_ending(text);
            text.insert_str(char_to_byte(text, line.start), &format!("{indent}{ending}"));
            self.cursor = line.start + indent.chars().count();
        } else {
            // Insert before the complete line break, using the same smart
            // indentation as normal Enter.
            let at = line.end;
            let unit = if self.indent_with_tabs {
                "\t".into()
            } else {
                " ".repeat(self.indent_width)
            };
            let edit = crate::editor::smart_newline(text, at..at, &unit);
            *text = edit.text;
            self.cursor = edit.selection.start;
        }
        let indent = text[..char_to_byte(text, self.cursor)]
            .rsplit('\n')
            .next()
            .unwrap_or_default();
        self.insert_line_prefix = Some(format!("{}{indent}", line_ending(text)));
        self.mode = VimMode::Insert;
        self.insert_origin = Some(before);
        self.insert_count = 1;
        self.clear_command();
    }

    pub(super) fn restore(&mut self, text: &mut String, redo: bool) -> bool {
        let source = if redo { &mut self.redo } else { &mut self.undo };
        let Some(snapshot) = source.pop() else {
            return false;
        };
        let current = self.snapshot(text);
        self.track_marks(text, &snapshot.text, false, self.cursor);
        self.marks.named = snapshot.marks;
        let target = if redo { &mut self.undo } else { &mut self.redo };
        target.push(current);
        trim_history(target);
        *text = snapshot.text;
        self.cursor = snapshot.cursor;
        self.multi = snapshot.multi;
        self.multi_cursors.clear();
        self.insert_origin = None;
        self.replace = None;
        self.clear_command();
        true
    }

    pub(super) fn repeat_change(
        &mut self,
        text: &mut String,
        count: usize,
        explicit: bool,
    ) -> (bool, bool, Option<Yank>) {
        let Some(mut change) = self.last_change.clone() else {
            return (true, false, None);
        };
        if change.keys.is_empty() {
            return (true, false, None);
        }
        if let Some(search) = change.search.as_mut().filter(|_| explicit) {
            search.count = count;
        } else if explicit && change.visual.is_none() {
            // A count on dot replaces the original command count, as in Vim.
            let digits = change
                .keys
                .iter()
                .take_while(|key| matches!(key, VimKey::Char(ch) if ch.is_ascii_digit()))
                .count();
            change.keys.drain(..digits);
            let register_prefix = usize::from(matches!(
                change.keys.as_slice(),
                [VimKey::Char('"'), VimKey::Char(_), ..]
            )) * 2;
            let after_register_digits = change.keys[register_prefix..]
                .iter()
                .take_while(|key| matches!(key, VimKey::Char(ch) if ch.is_ascii_digit()))
                .count();
            if after_register_digits > 0 {
                change
                    .keys
                    .drain(register_prefix..register_prefix + after_register_digits);
            }
            let motion_offset = match &change.keys[register_prefix..] {
                [VimKey::Char('y'), VimKey::Char('s'), ..]
                | [VimKey::Char('g'), VimKey::Char('u' | 'U' | '~'), ..] => {
                    Some(register_prefix + 2)
                }
                [VimKey::Char('d' | 'c' | 'y' | '>' | '<'), ..] => Some(register_prefix + 1),
                _ => None,
            };
            if let Some(offset) = motion_offset {
                let motion_digits = if change
                    .keys
                    .get(offset)
                    .is_some_and(|key| matches!(key, VimKey::Char('1'..='9')))
                {
                    change
                        .keys
                        .iter()
                        .skip(offset)
                        .take_while(|key| matches!(key, VimKey::Char(ch) if ch.is_ascii_digit()))
                        .count()
                } else {
                    0
                };
                change.keys.drain(offset..offset + motion_digits);
            }
            let mut keys: Vec<VimKey> = count.to_string().chars().map(VimKey::Char).collect();
            keys.extend(change.keys);
            change.keys = keys;
        }
        let before = self.snapshot(text);
        self.command_keys.clear();
        if let Some(visual) = &change.visual {
            // Visual changes repeat their completed area, independent of mouse
            // selection and selection-building keys. A dot count is ignored.
            self.select_repeat_visual(text, visual);
        }
        self.replaying = true;
        // The insert replay supplies a baseline even during command replay, so
        // counted i/a commands can repeat the inserted text on Escape.
        let mut yank = None;
        let surround_desired = if change.search.is_some()
            && change
                .keys
                .starts_with(&[VimKey::Char('y'), VimKey::Char('s')])
        {
            change.keys.last().copied()
        } else {
            None
        };
        let key_count = change.keys.len() - usize::from(surround_desired.is_some());
        for key in &change.keys[..key_count] {
            let action = self.handle_key(text, *key);
            if action.yanked.is_some() {
                yank = action.yanked;
            }
            if self.global_mark_request.is_some() {
                self.replaying = false;
                self.pending_global_repeat = Some(change);
                return (true, false, yank);
            }
        }
        if let Some(motion) = &change.search {
            let target = crate::search::search(
                text,
                &motion.query,
                motion.case_sensitive,
                motion.whole_word,
                motion.regex,
            )
            .ok()
            .and_then(|matches| {
                crate::search::next_match(
                    &matches,
                    self.cursor,
                    motion.backwards,
                    motion.count,
                    false,
                )
            })
            .map(|(range, _)| range.start);
            if let Some(target) = target {
                let action = self.apply_search_motion(text, target, motion.clone());
                yank = action.yanked;
            } else {
                self.cancel_pending_command();
            }
        }
        if let Some(desired) = surround_desired
            && matches!(self.pending, Some(Pending::SurroundAdd(..)))
        {
            self.handle_key(text, desired);
        }
        self.surround_search = None;
        if let Some(delta) = &change.insert
            && self.mode == VimMode::Insert
        {
            self.insert_replay = Some(InsertReplay {
                keys: change.keys.clone(),
                baseline: text.clone(),
                cursor: self.cursor,
                search: change.search.clone(),
                visual: change.visual.clone(),
            });
            let len = text.chars().count();
            let start = self
                .cursor
                .saturating_add_signed(delta.relative_start)
                .min(len);
            let end = start.saturating_add(delta.removed).min(len);
            let range = char_to_byte(text, start)..char_to_byte(text, end);
            // Guard replayed paste/insert sizes before allocating or mutating.
            if text
                .len()
                .saturating_sub(range.len())
                .saturating_add(delta.inserted.len())
                <= MAX_PASTE_BYTES
            {
                text.replace_range(range, &delta.inserted);
                self.cursor = start + delta.inserted.chars().count();
            }
            self.finish_insert(text);
        }
        self.replaying = false;
        self.insert_replay = None;
        self.clear_command();
        let changed = *text != before.text;
        if changed {
            self.record(before, text);
            self.last_change = Some(change);
        }
        (true, changed, yank)
    }
}

fn trim_history(history: &mut Vec<Snapshot>) {
    let mut bytes: usize = history.iter().map(|snapshot| snapshot.text.len()).sum();
    let mut remove = 0;
    while history.len() - remove > 1
        && (history.len() - remove > MAX_HISTORY || bytes > MAX_HISTORY_BYTES)
    {
        bytes -= history[remove].text.len();
        remove += 1;
    }
    if remove > 0 {
        history.drain(..remove);
    }
}

pub(super) fn insert_delta(before: &str, after: &str, cursor: usize) -> InsertDelta {
    let mut prefix_bytes = 0;
    let start = before
        .chars()
        .zip(after.chars())
        .take(cursor)
        .take_while(|(old, new)| old == new)
        .inspect(|(ch, _)| prefix_bytes += ch.len_utf8())
        .count();
    let mut suffix_bytes = 0;
    before[prefix_bytes..]
        .chars()
        .rev()
        .zip(after[prefix_bytes..].chars().rev())
        .take_while(|(old, new)| old == new)
        .for_each(|(ch, _)| suffix_bytes += ch.len_utf8());
    let old = &before[prefix_bytes..before.len() - suffix_bytes];
    let new = &after[prefix_bytes..after.len() - suffix_bytes];
    InsertDelta {
        relative_start: start as isize - cursor as isize,
        removed: old.chars().count(),
        inserted: new.to_owned(),
    }
}

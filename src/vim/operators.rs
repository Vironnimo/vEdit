//! Operator motions and edits over character or line ranges.
use super::*;

impl VimState {
    /// Cut the completed Visual area, discarding only unfinished command input.
    /// Geometry makes dot reliable for keyboard, mouse and external selections,
    /// even when a pending r/i/g/count or the bounded key history is incomplete.
    pub fn cut_visual(&mut self, text: &mut String) -> Action {
        if self.mode == VimMode::VisualBlock {
            return self.cut_block(text);
        }
        self.yanked_ranges.clear();
        if !matches!(
            self.mode,
            VimMode::Visual | VimMode::VisualLine | VimMode::VisualBlock
        ) {
            return Action::default();
        }
        let Some(range) = self.selection(text).filter(|range| !range.is_empty()) else {
            self.leave_visual();
            self.cancel_pending_command();
            return Action {
                consumed: true,
                mode_changed: true,
                ..Action::default()
            };
        };
        let old_cursor = self.cursor;
        let old_mode = self.mode;
        let visual = self.visual_repeat(text, &range);
        self.cancel_pending_command();
        let (consumed, text_changed, yanked) = self.apply_visual(text, 'd', 1);
        self.clamp_cursor(text);
        if text_changed && !self.replaying {
            self.last_change = Some(RepeatChange {
                keys: vec![VimKey::Char('d')],
                insert: None,
                search: None,
                visual: Some(visual),
            });
        }
        self.motion_epoch = self.motion_epoch.wrapping_add(1);
        Action {
            consumed,
            text_changed,
            yanked,
            yanked_ranges: std::mem::take(&mut self.yanked_ranges),
            cursor_changed: self.cursor != old_cursor,
            mode_changed: self.mode != old_mode,
            ..Action::default()
        }
    }

    /// System Cut shares Visual undo/repeat behavior without replacing the Vim
    /// unnamed register. The returned clipboard text is deliberately separate.
    pub fn cut_visual_to_clipboard(&mut self, text: &mut String) -> (Action, Option<String>) {
        let register = self.register.clone();
        let mut action = self.cut_visual(text);
        self.register = register;
        let copied = action.yanked.take().map(|yank| yank.text);
        action.yanked_ranges.clear();
        (action, copied)
    }

    /// Apply an exclusive logical search target before Normal mode clamps its
    /// visible caret away from newlines/EOF. This shares registers and history
    /// with ordinary Vim operators.
    pub fn apply_search_motion(
        &mut self,
        text: &mut String,
        target: usize,
        motion: SearchMotion,
    ) -> Action {
        self.yanked_ranges.clear();
        let old_cursor = self.cursor;
        let old_mode = self.mode;
        let operator = match self.pending {
            Some(Pending::Operator(op, _, _)) => Some(op),
            _ => None,
        };
        self.cancel_pending_command();
        let (consumed, text_changed, yanked) = if let Some(op) = operator {
            let result =
                self.apply_exclusive(text, op, old_cursor.min(target), old_cursor.max(target));
            if op == Operator::Surround && self.pending.is_some() {
                self.command_keys = vec![VimKey::Char('y'), VimKey::Char('s')];
                self.surround_search = Some(motion);
            } else if !self.replaying {
                let keys = op.command().chars().map(VimKey::Char).collect();
                if self.mode == VimMode::Insert {
                    self.insert_replay = Some(InsertReplay {
                        keys,
                        baseline: text.clone(),
                        cursor: self.cursor,
                        search: Some(motion),
                        visual: None,
                    });
                } else if result.1 {
                    self.last_change = Some(RepeatChange {
                        keys,
                        insert: None,
                        search: Some(motion),
                        visual: None,
                    });
                }
            }
            result
        } else {
            self.set_cursor(text, target);
            (true, false, None)
        };
        self.clamp_cursor(text);
        self.motion_epoch = self.motion_epoch.wrapping_add(1);
        Action {
            consumed,
            text_changed,
            cursor_changed: self.cursor != old_cursor,
            mode_changed: self.mode != old_mode,
            yanked,
            yanked_ranges: std::mem::take(&mut self.yanked_ranges),
            ..Action::default()
        }
    }

    /// Ctrl+A / Ctrl+X: change the decimal number under or after the cursor on
    /// the current line. A directly preceding `-` makes it negative; leading
    /// zeros keep their width.
    pub(super) fn add_to_number(
        &mut self,
        text: &mut String,
        count: usize,
        increment: bool,
    ) -> bool {
        let chars: Vec<char> = text.chars().collect();
        let line = line_at_cursor(&chars, self.cursor);
        let cursor = self.cursor.clamp(line.start, line.end);
        let Some(mut start) = (cursor..line.end).find(|index| chars[*index].is_ascii_digit())
        else {
            return false;
        };
        while start > line.start && chars[start - 1].is_ascii_digit() {
            start -= 1;
        }
        let mut end = start;
        while end < line.end && chars[end].is_ascii_digit() {
            end += 1;
        }
        let digits: String = chars[start..end].iter().collect();
        let Ok(magnitude) = digits.parse::<i128>() else {
            return false;
        };
        let negative = start > line.start && chars[start - 1] == '-';
        let value = if negative { -magnitude } else { magnitude };
        let delta = i128::try_from(count).unwrap_or(i128::MAX);
        let value = if increment {
            value.saturating_add(delta)
        } else {
            value.saturating_sub(delta)
        };
        let width = if digits.len() > 1 && digits.starts_with('0') {
            digits.len()
        } else {
            0
        };
        let replacement = format!(
            "{}{:0width$}",
            if value < 0 { "-" } else { "" },
            value.unsigned_abs()
        );
        let start = start - usize::from(negative);
        let before = self.snapshot(text);
        text.replace_range(
            char_to_byte(text, start)..char_to_byte(text, end),
            &replacement,
        );
        self.cursor = start + replacement.chars().count() - 1;
        self.preferred_column = None;
        self.record(before);
        true
    }

    pub(super) fn apply_operator_motion(
        &mut self,
        text: &mut String,
        op: Operator,
        key: char,
        count: usize,
        explicit: bool,
    ) -> (bool, bool, Option<Yank>) {
        let chars: Vec<char> = text.chars().collect();
        let rows = lines(&chars);
        let row = row_for_cursor(&rows, self.cursor);
        let line = rows[row];
        // A final line break ends the last line; it does not start another.
        let last = last_line_row(&rows).max(row);
        if matches!(key, 'j' | 'k' | 'G' | '+' | '-' | '_' | 'H' | 'M' | 'L') {
            let target = match key {
                'j' | '+' => row.saturating_add(count).min(last),
                'k' | '-' => row.saturating_sub(count),
                '_' => row.saturating_add(count - 1).min(last),
                'H' | 'M' | 'L' => self.screen_row(&chars, &rows, key, count),
                _ => {
                    if explicit {
                        count.saturating_sub(1).min(last)
                    } else {
                        last
                    }
                }
            };
            return self.apply_lines(text, op, row, target);
        }
        let range = match key {
            'h' => self.cursor.saturating_sub(count).max(line.start)..self.cursor,
            'l' => self.cursor..self.cursor.saturating_add(count).min(line.end),
            '0' => line.start..self.cursor,
            '^' => {
                let target = first_nonblank(&chars, line);
                self.cursor.min(target)..self.cursor.max(target)
            }
            '|' => {
                let target = line.start
                    + count
                        .saturating_sub(1)
                        .min(line.end.saturating_sub(line.start + 1));
                self.cursor.min(target)..self.cursor.max(target)
            }
            '$' => self.cursor..rows[row.saturating_add(count - 1).min(last)].end,
            'w' | 'W' => {
                if op == Operator::Change
                    && chars.get(self.cursor).is_some_and(|ch| !ch.is_whitespace())
                {
                    let mut end = self.cursor;
                    for step in 0..count {
                        end = if step == 0 {
                            change_word_end(&chars, end, key == 'W')
                        } else {
                            word_end(&chars, end, key == 'W')
                        };
                    }
                    self.cursor..end.saturating_add(1).min(chars.len())
                } else {
                    let mut target = self.cursor;
                    let mut last_start = self.cursor;
                    for _ in 0..count {
                        last_start = target;
                        let next = word_forward(&chars, target, key == 'W');
                        if next == target {
                            break;
                        }
                        target = next;
                    }
                    let last_row = row_for_cursor(&rows, last_start);
                    let last_line = rows[last_row];
                    if target > last_line.end
                        && last_line.start < last_line.end
                        && (op == Operator::Yank || last_row == row)
                    {
                        target = last_line.end;
                    }
                    self.cursor..target
                }
            }
            'b' | 'B' => {
                repeat_motion(&chars, self.cursor, count, key == 'B', word_backward)..self.cursor
            }
            'e' | 'E' => {
                self.cursor
                    ..repeat_motion(&chars, self.cursor, count, key == 'E', word_end)
                        .saturating_add(1)
                        .min(chars.len())
            }
            '%' => {
                let Some(target) = matching_bracket(&chars, self.cursor) else {
                    return (true, false, None);
                };
                self.cursor.min(target)..self.cursor.max(target) + 1
            }
            '{' | '}' => {
                let target = paragraph(&chars, self.cursor, count, key == '}');
                return self.apply_exclusive(
                    text,
                    op,
                    self.cursor.min(target),
                    self.cursor.max(target),
                );
            }
            _ => {
                if !key.is_whitespace() {
                    self.unsupported(&format!("{}{key}", op.command()));
                }
                if matches!(key, '\'' | '`') {
                    // Mark motions take a mark name; it must not run as a command.
                    self.pending = Some(Pending::Ignore);
                }
                return (true, false, None);
            }
        };
        if matches!(op, Operator::Indent | Operator::Outdent) {
            let last_char = range.end.saturating_sub(1).max(range.start);
            return self.apply_lines(
                text,
                op,
                row_for_cursor(&rows, range.start),
                row_for_cursor(&rows, last_char),
            );
        }
        let start = range.start;
        let result = self.apply_range(text, op, range);
        if op == Operator::Yank {
            // Like Vim, a yank leaves the cursor at the start of the text.
            self.cursor = start;
        }
        result
    }

    /// Apply an exclusive motion with Vim's column-one rule: a target at the
    /// start of a later line does not include that line. From the indentation
    /// the motion becomes linewise; otherwise the preceding newline remains.
    fn apply_exclusive(
        &mut self,
        text: &mut String,
        op: Operator,
        start: usize,
        end: usize,
    ) -> (bool, bool, Option<Yank>) {
        let chars: Vec<_> = text.chars().collect();
        let rows = lines(&chars);
        let start = start.min(chars.len());
        let mut end = end.min(chars.len()).max(start);
        let first_row = row_for_cursor(&rows, start);
        let last_row = row_for_cursor(&rows, end);
        let linewise = last_row > first_row
            && end == rows[last_row].start
            && start <= first_nonblank(&chars, rows[first_row]);
        let result = if linewise {
            self.apply_lines(text, op, first_row, last_row - 1)
        } else {
            if last_row > first_row && end == rows[last_row].start {
                end = rows[last_row - 1].end;
            }
            if matches!(op, Operator::Indent | Operator::Outdent) {
                self.apply_lines(
                    text,
                    op,
                    first_row,
                    row_for_cursor(&rows, end.saturating_sub(1).max(start)),
                )
            } else {
                self.apply_range(text, op, start..end)
            }
        };
        if op == Operator::Yank {
            self.cursor = if linewise {
                first_nonblank(&chars, rows[first_row])
            } else {
                start
            };
        }
        result
    }

    pub(super) fn apply_lines(
        &mut self,
        text: &mut String,
        op: Operator,
        first: usize,
        last: usize,
    ) -> (bool, bool, Option<Yank>) {
        let (first, last) = (first.min(last), first.max(last));
        if matches!(op, Operator::Indent | Operator::Outdent) {
            return (
                true,
                self.shift_lines(text, first, last, op == Operator::Indent, 1),
                None,
            );
        }
        let chars: Vec<char> = text.chars().collect();
        let rows = lines(&chars);
        // The empty row after a final line break is not a line of its own,
        // unless the cursor is on it.
        let last = last.min(last_line_row(&rows).max(first));
        let first_line = rows[first];
        let last_line = rows[last];
        if op.changes_case() {
            return self.apply_range(text, op, first_line.start..last_line.end);
        }
        if op == Operator::Surround {
            let start = first_line.start
                + chars[first_line.start..first_line.end]
                    .iter()
                    .take_while(|ch| matches!(ch, ' ' | '\t'))
                    .count();
            return self.apply_range(text, op, start..last_line.end);
        }
        let mut register: String = chars
            [first_line.start..last_line.newline.map_or(last_line.end, |index| index + 1)]
            .iter()
            .collect();
        if last_line.newline.is_none() {
            register.push_str(line_ending(text));
        }
        let yank = Yank {
            text: register,
            linewise: true,
            block_width: None,
        };
        self.register = Some(yank.clone());
        self.yanked_ranges.clear();
        self.yanked_ranges
            .push(first_line.start..last_line.newline.map_or(last_line.end, |index| index + 1));
        if op == Operator::Yank {
            self.yank_register = Some(yank.clone());
            return (true, false, Some(yank));
        }
        let before = self.snapshot(text);
        let mut start = first_line.start;
        let end = if op == Operator::Change {
            last_line.end
        } else if let Some(newline) = last_line.newline {
            newline + 1
        } else {
            start = if first > 0 { rows[first - 1].end } else { 0 };
            last_line.end
        };
        let indentation: String = if op == Operator::Change {
            chars[first_line.start..first_line.end]
                .iter()
                .copied()
                .take_while(|ch| matches!(ch, ' ' | '\t'))
                .collect()
        } else {
            String::new()
        };
        let bytes = char_to_byte(text, start)..char_to_byte(text, end);
        let changed = text[bytes.clone()] != indentation;
        if changed {
            text.replace_range(bytes, &indentation);
        }
        self.cursor = if op == Operator::Change {
            first_line.start + indentation.chars().count()
        } else {
            start
        };
        self.visual_anchor = None;
        if op == Operator::Change {
            self.mode = VimMode::Insert;
            self.insert_origin = Some(before);
        } else {
            self.mode = VimMode::Normal;
            if changed {
                self.record(before);
            }
            let chars: Vec<char> = text.chars().collect();
            let rows = lines(&chars);
            self.cursor = first_nonblank(&chars, rows[row_for_cursor(&rows, self.cursor)]);
        }
        self.preferred_column = None;
        (true, changed, Some(yank))
    }

    pub(super) fn apply_range(
        &mut self,
        text: &mut String,
        op: Operator,
        range: Range<usize>,
    ) -> (bool, bool, Option<Yank>) {
        if op.changes_case() {
            let len = text.chars().count();
            let start = range.start.min(len);
            let end = range.end.min(len).max(start);
            let bytes = char_to_byte(text, start)..char_to_byte(text, end);
            let selected = &text[bytes.clone()];
            let replacement = match op {
                Operator::Lowercase => selected.to_lowercase(),
                Operator::Uppercase => selected.to_uppercase(),
                Operator::ToggleCase => {
                    let mut toggled = String::new();
                    for ch in selected.chars() {
                        if ch.is_lowercase() {
                            toggled.extend(ch.to_uppercase());
                        } else {
                            toggled.extend(ch.to_lowercase());
                        }
                    }
                    toggled
                }
                _ => unreachable!(),
            };
            let changed = selected != replacement;
            if changed {
                let before = self.snapshot(text);
                text.replace_range(bytes, &replacement);
                self.record(before);
            }
            self.leave_visual();
            self.cursor = start;
            self.preferred_column = None;
            return (true, changed, None);
        }
        if op == Operator::Surround {
            let chars: Vec<_> = text.chars().collect();
            let start = range.start.min(chars.len());
            let mut end = range.end.min(chars.len()).max(start);
            while end > start && chars[end - 1].is_whitespace() {
                end -= 1;
            }
            if start < end {
                self.pending = Some(Pending::SurroundAdd(start, end));
            }
            return (true, false, None);
        }
        let len = text.chars().count();
        let start = range.start.min(len);
        let end = range.end.min(len).max(start);
        let start_byte = char_to_byte(text, start);
        let end_byte = char_to_byte(text, end);
        let yank = if start < end {
            let yank = Yank {
                text: text[start_byte..end_byte].to_owned(),
                linewise: false,
                block_width: None,
            };
            self.register = Some(yank.clone());
            self.yanked_ranges.clear();
            self.yanked_ranges.push(start..end);
            Some(yank)
        } else {
            None
        };
        if op == Operator::Yank {
            if yank.is_some() {
                self.yank_register = yank.clone();
            }
            return (true, false, yank);
        }
        let before = self.snapshot(text);
        if start < end {
            text.replace_range(start_byte..end_byte, "");
        }
        self.cursor = start;
        self.visual_anchor = None;
        if op == Operator::Change {
            self.mode = VimMode::Insert;
            self.insert_origin = Some(before);
        } else {
            self.mode = VimMode::Normal;
            if start < end {
                self.record(before);
            }
        }
        self.preferred_column = None;
        (true, start < end, yank)
    }

    pub(super) fn apply_visual(
        &mut self,
        text: &mut String,
        key: char,
        count: usize,
    ) -> (bool, bool, Option<Yank>) {
        if self.mode == VimMode::VisualBlock {
            return self.apply_block(text, key, count);
        }
        let Some(range) = self.selection(text) else {
            return (true, false, None);
        };
        let chars: Vec<char> = text.chars().collect();
        let rows = lines(&chars);
        let first = row_for_cursor(&rows, range.start);
        let last = row_for_cursor(
            &rows,
            self.cursor.max(self.visual_anchor.unwrap_or(self.cursor)),
        );
        if matches!(key, '>' | '<') {
            self.leave_visual();
            return (
                true,
                self.shift_lines(text, first, last, key == '>', count),
                None,
            );
        }
        let linewise = self.mode == VimMode::VisualLine;
        self.leave_visual();
        let op = match key {
            'y' => Operator::Yank,
            'c' | 's' => Operator::Change,
            'u' => Operator::Lowercase,
            'U' => Operator::Uppercase,
            '~' => Operator::ToggleCase,
            _ => Operator::Delete,
        };
        let result = if linewise {
            self.apply_lines(text, op, first, last)
        } else {
            self.apply_range(text, op, range.clone())
        };
        if op == Operator::Yank {
            self.cursor = range.start;
        }
        result
    }

    pub(super) fn shift_lines(
        &mut self,
        text: &mut String,
        first: usize,
        last: usize,
        right: bool,
        count: usize,
    ) -> bool {
        let before = self.snapshot(text);
        let chars: Vec<char> = text.chars().collect();
        let rows = lines(&chars);
        let last = last.min(rows.len() - 1);
        let added = if right {
            let width = if self.indent_with_tabs {
                count
            } else {
                self.indent_width.saturating_mul(count)
            };
            let affected = rows[first..=last]
                .iter()
                .filter(|line| line.start != line.end)
                .count();
            let Some(added) = width
                .checked_mul(affected)
                .filter(|added| text.len().saturating_add(*added) <= MAX_PASTE_BYTES)
            else {
                return false;
            };
            added
        } else {
            0
        };
        if right && added == 0 {
            return false;
        }
        let indent = if !right {
            String::new()
        } else if self.indent_with_tabs {
            "\t".repeat(count)
        } else {
            " ".repeat(self.indent_width.saturating_mul(count))
        };
        // Copy each source character once. Inserting/removing each line in
        // place repeatedly scans UTF-8 prefixes and moves the document tail.
        let mut result = String::with_capacity(text.len() + added);
        let mut copied = 0;
        for line in &rows[first..=last] {
            result.extend(chars[copied..line.start].iter());
            copied = line.start;
            if right {
                if line.start != line.end {
                    result.push_str(&indent);
                }
            } else {
                let mut at = line.start;
                let mut columns = 0;
                let wanted = self.indent_width.saturating_mul(count);
                while at < line.end && columns < wanted {
                    match chars[at] {
                        ' ' => columns += 1,
                        '\t' => columns += self.indent_width - columns % self.indent_width,
                        _ => break,
                    }
                    at += 1;
                }
                copied = at;
            }
        }
        result.extend(chars[copied..].iter());
        if result == *text {
            return false;
        }
        *text = result;
        let chars: Vec<char> = text.chars().collect();
        let rows = lines(&chars);
        self.cursor = first_nonblank(&chars, rows[first]);
        self.record(before);
        self.preferred_column = None;
        true
    }

    pub(super) fn join_lines(&mut self, text: &mut String, count: usize, spaces: bool) -> bool {
        let chars: Vec<char> = text.chars().collect();
        let rows = lines(&chars);
        let row = row_for_cursor(&rows, self.cursor);
        let last = row
            .saturating_add(count - 1)
            .min(last_line_row(&rows).max(row));
        if last == row {
            return false;
        }
        let snapshot = self.snapshot(text);
        let first = rows[row];
        // Keep the final row's complete line break in the original document.
        let mut joined: String = chars[first.start..first.end].iter().collect();
        let cursor = first.end;
        for line in &rows[row + 1..=last] {
            let content: String = chars[line.start..line.end].iter().collect();
            let content = if spaces {
                content.trim_start_matches([' ', '\t'])
            } else {
                &content
            };
            if spaces
                && !joined.is_empty()
                && !joined.ends_with([' ', '\t'])
                && !content.is_empty()
                && !content.starts_with(')')
            {
                joined.push(' ');
            }
            joined.push_str(content);
        }
        text.replace_range(
            char_to_byte(text, first.start)..char_to_byte(text, rows[last].end),
            &joined,
        );
        self.cursor = cursor;
        self.record(snapshot);
        self.preferred_column = None;
        true
    }
}

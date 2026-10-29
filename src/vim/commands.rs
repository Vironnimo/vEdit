//! Pending command decoding and Normal/Visual key dispatch.
use super::*;

impl VimState {
    /// When true, punctuation belongs to a command such as r/ or df/ and must
    /// reach this model rather than opening a search or command field.
    pub fn expects_literal_character(&self) -> bool {
        matches!(
            self.pending,
            Some(
                Pending::Replace(_)
                    | Pending::Register(_)
                    | Pending::SurroundAdd(..)
                    | Pending::SurroundTarget { .. }
                    | Pending::SurroundChange { .. }
                    | Pending::Find { .. }
                    | Pending::TextObject { .. }
                    | Pending::Ignore
                    | Pending::Bracket(_)
            )
        )
    }

    /// Cancel an unfinished operator/count without leaving a visual selection.
    pub fn cancel_pending_command(&mut self) {
        self.clear_command();
        self.command_keys.clear();
        self.surround_search = None;
    }

    /// Consume a search count while retaining a waiting operator. Search
    /// previews may move the caret, but the operator commits only on Enter/n.
    pub fn take_search_count(&mut self) -> usize {
        let count = self.take_count().clamp(1, MAX_COUNT);
        match self.pending {
            Some(Pending::Operator(_, prefix, _)) => count.saturating_mul(prefix).min(MAX_COUNT),
            None => count,
            _ => {
                self.cancel_pending_command();
                count
            }
        }
    }

    pub fn search_operator_pending(&self) -> bool {
        matches!(self.pending, Some(Pending::Operator(_, _, _)))
    }

    pub fn pending_command(&self) -> String {
        if matches!(
            self.pending,
            Some(
                Pending::Operator(Operator::Surround, ..)
                    | Pending::Go {
                        operator: Some(Operator::Surround),
                        ..
                    }
                    | Pending::Find {
                        operator: Some(Operator::Surround),
                        ..
                    }
                    | Pending::TextObject {
                        operator: Some(Operator::Surround),
                        ..
                    }
                    | Pending::SurroundAdd(..)
                    | Pending::SurroundTarget { .. }
                    | Pending::SurroundChange { .. }
            )
        ) {
            return self
                .command_keys
                .iter()
                .filter_map(|key| match key {
                    VimKey::Char(ch) => Some(*ch),
                    _ => None,
                })
                .collect();
        }
        let prefix = |count: usize| {
            if count > 1 {
                count.to_string()
            } else {
                String::new()
            }
        };
        let mut command = match self.pending {
            Some(Pending::Operator(op, count, explicit)) => format!(
                "{}{}",
                if explicit {
                    count.to_string()
                } else {
                    String::new()
                },
                op.command(),
            ),
            Some(Pending::Go {
                operator,
                count,
                explicit,
            }) => format!(
                "{}{}g",
                if explicit {
                    count.to_string()
                } else {
                    String::new()
                },
                operator.map(|op| op.command()).unwrap_or_default()
            ),
            Some(Pending::Replace(count)) => format!("{}r", prefix(count)),
            Some(Pending::Register(count)) => format!("{}\"", prefix(count)),
            Some(Pending::RegisterPaste(count)) => format!("{}\"0", prefix(count)),
            Some(Pending::SurroundAdd(..)) => self
                .command_keys
                .iter()
                .filter_map(|key| match key {
                    VimKey::Char(ch) => Some(*ch),
                    _ => None,
                })
                .collect(),
            Some(Pending::SurroundTarget { change, count }) => {
                format!("{}{}s", prefix(count), if change { 'c' } else { 'd' })
            }
            Some(Pending::SurroundChange { target, .. }) => format!("cs{target}"),
            Some(Pending::Find {
                operator,
                count,
                forward,
                till,
            }) => format!(
                "{}{}{}",
                prefix(count),
                operator.map(|op| op.command()).unwrap_or_default(),
                match (forward, till) {
                    (true, false) => 'f',
                    (false, false) => 'F',
                    (true, true) => 't',
                    (false, true) => 'T',
                }
            ),
            Some(Pending::TextObject {
                operator,
                count,
                around,
            }) => format!(
                "{}{}{}",
                prefix(count),
                operator.map(|op| op.command()).unwrap_or_default(),
                if around { 'a' } else { 'i' }
            ),
            Some(Pending::Scroll { count, explicit }) => format!(
                "{}z",
                if explicit {
                    count.to_string()
                } else {
                    String::new()
                }
            ),
            Some(Pending::Bracket(bracket)) => bracket.to_string(),
            Some(Pending::Ignore) | None => String::new(),
        };
        if self.count > 0 {
            command.push_str(&self.count.to_string());
        }
        command
    }

    pub(super) fn handle_normal_key(
        &mut self,
        text: &mut String,
        key: VimKey,
    ) -> (bool, bool, Option<Yank>) {
        if key == VimKey::VisualBlock {
            self.toggle_visual_block(text);
            return (true, false, None);
        }
        if key == VimKey::Escape {
            self.leave_visual();
            self.clear_command();
            return (true, false, None);
        }
        if key == VimKey::Redo {
            let count = self.take_count();
            self.leave_visual();
            self.clear_command();
            let mut changed = false;
            for _ in 0..count {
                if !self.restore(text, true) {
                    break;
                }
                changed = true;
            }
            return (true, changed, None);
        }
        if let Some(Pending::Ignore) = self.pending {
            self.clear_command();
            return (true, false, None);
        }
        if let Some(Pending::Bracket(bracket)) = self.pending {
            self.clear_command();
            if let VimKey::Char(ch) = key {
                self.unsupported(&format!("{bracket}{ch}"));
            }
            return (true, false, None);
        }
        if let Some(Pending::Scroll { count, explicit }) = self.pending {
            self.clear_command();
            let (align, line_start) = match key {
                VimKey::Char('z') => (ScrollAlign::Center, false),
                VimKey::Char('t') => (ScrollAlign::Top, false),
                VimKey::Char('b') => (ScrollAlign::Bottom, false),
                VimKey::Enter => (ScrollAlign::Top, true),
                VimKey::Char('.') => (ScrollAlign::Center, true),
                VimKey::Char('-') => (ScrollAlign::Bottom, true),
                VimKey::Char(other) => {
                    self.unsupported(&format!("z{other}"));
                    return (true, false, None);
                }
                _ => return (true, false, None),
            };
            if explicit || line_start {
                let chars: Vec<char> = text.chars().collect();
                let rows = lines(&chars);
                let current = rows[row_for_cursor(&rows, self.cursor)];
                let line = if explicit {
                    rows[count.saturating_sub(1).min(last_line_row(&rows))]
                } else {
                    current
                };
                self.cursor = if line_start {
                    first_nonblank(&chars, line)
                } else {
                    line.start
                        + self
                            .cursor
                            .saturating_sub(current.start)
                            .min(line.end.saturating_sub(line.start + 1))
                };
                self.preferred_column = None;
            }
            self.scroll_request = Some(align);
            return (true, false, None);
        }
        if let Some(Pending::Replace(count)) = self.pending {
            self.clear_command();
            let replacement = match key {
                VimKey::Char(ch) => Some(ch),
                VimKey::Enter => Some('\n'),
                VimKey::Tab => Some('\t'),
                _ => None,
            };
            return (
                true,
                replacement.is_some_and(|ch| self.replace_characters(text, count, ch)),
                None,
            );
        }
        if let Some(Pending::Register(count)) = self.pending {
            self.pending = if key == VimKey::Char('0') {
                Some(Pending::RegisterPaste(count))
            } else {
                None
            };
            self.count = 0;
            return (true, false, None);
        }
        if let Some(Pending::SurroundAdd(start, end)) = self.pending {
            self.pending = None;
            let VimKey::Char(desired) = key else {
                return (true, false, None);
            };
            return (true, self.add_surround(text, start..end, desired), None);
        }
        if let Some(Pending::SurroundTarget { change, count }) = self.pending {
            self.pending = None;
            let VimKey::Char(target) = key else {
                return (true, false, None);
            };
            let chars: Vec<_> = text.chars().collect();
            let Some(range) = surround_range(&chars, self.cursor, target, count) else {
                return (true, false, None);
            };
            if change {
                self.pending = Some(Pending::SurroundChange {
                    start: range.start,
                    end: range.end,
                    target,
                });
                return (true, false, None);
            }
            return (true, self.change_surround(text, range, target, None), None);
        }
        if let Some(Pending::SurroundChange { start, end, target }) = self.pending {
            self.pending = None;
            let VimKey::Char(desired) = key else {
                return (true, false, None);
            };
            return (
                true,
                self.change_surround(text, start..end, target, Some(desired)),
                None,
            );
        }
        if let Some(Pending::Find {
            operator,
            count,
            forward,
            till,
        }) = self.pending
        {
            self.pending = None;
            self.count = 0;
            let character = match key {
                VimKey::Char(character) => character,
                VimKey::Tab => '\t',
                _ => return (true, false, None),
            };
            let find = Find {
                character,
                forward,
                till,
            };
            self.last_find = Some(find);
            return self.find(text, find, count, operator, false);
        }
        if let Some(Pending::TextObject {
            operator,
            count,
            around,
        }) = self.pending
        {
            self.pending = None;
            self.count = 0;
            let VimKey::Char(object) = key else {
                return (true, false, None);
            };
            let chars: Vec<char> = text.chars().collect();
            if object == 'p' {
                let (first, last) = paragraph_object(&chars, self.cursor, count, around);
                let rows = lines(&chars);
                if let Some(op) = operator {
                    let result = self.apply_lines(text, op, first, last);
                    if op == Operator::Yank {
                        self.cursor = rows[first].start;
                    }
                    return result;
                }
                self.mode = VimMode::VisualLine;
                self.visual_to_line_end = false;
                self.visual_anchor = Some(rows[first].start);
                self.cursor = rows[last].start;
                return (true, false, None);
            }
            if !matches!(
                object,
                'w' | 'W'
                    | '\''
                    | '"'
                    | '`'
                    | '('
                    | ')'
                    | 'b'
                    | '['
                    | ']'
                    | '{'
                    | '}'
                    | 'B'
                    | '<'
                    | '>'
            ) {
                self.unsupported(&format!(
                    "{}{}{object}",
                    operator.map(|op| op.command()).unwrap_or_default(),
                    if around { 'a' } else { 'i' }
                ));
                return (true, false, None);
            }
            let Some(range) = text_object(&chars, self.cursor, object, count, around) else {
                return (true, false, None);
            };
            if let Some(op) = operator {
                if matches!(op, Operator::Indent | Operator::Outdent) {
                    let rows = lines(&chars);
                    return self.apply_lines(
                        text,
                        op,
                        row_for_cursor(&rows, range.start),
                        row_for_cursor(&rows, range.end.saturating_sub(1).max(range.start)),
                    );
                }
                return self.apply_range(text, op, range);
            }
            self.mode = VimMode::Visual;
            self.visual_to_line_end = false;
            self.visual_anchor = Some(range.start);
            self.cursor = range.end.saturating_sub(1).max(range.start);
            return (true, false, None);
        }
        if matches!(key, VimKey::Increment | VimKey::Decrement) {
            let count = self.take_count();
            self.pending = None;
            if self.mode != VimMode::Normal {
                return (true, false, None);
            }
            return (
                true,
                self.add_to_number(text, count, key == VimKey::Increment),
                None,
            );
        }
        let ch = match key {
            VimKey::Char(ch) => ch,
            VimKey::Backspace => 'h',
            VimKey::Enter => '+',
            _ => {
                self.clear_command();
                return (false, false, None);
            }
        };
        if ch.is_ascii_digit() && (ch != '0' || self.count > 0) {
            self.count = self
                .count
                .saturating_mul(10)
                .saturating_add(ch as usize - '0' as usize)
                .min(MAX_COUNT);
            return (true, false, None);
        }
        let explicit = self.count > 0;
        let count = self.take_count();
        let chars: Vec<char> = text.chars().collect();
        if let Some(pending) = self.pending.take() {
            match pending {
                Pending::Go {
                    operator,
                    count: prefix,
                    explicit: explicit_prefix,
                } => {
                    let count = prefix.saturating_mul(count).min(MAX_COUNT);
                    let rows = lines(&chars);
                    if operator.is_none() && ch == 'J' {
                        let count = if matches!(
                            self.mode,
                            VimMode::Visual | VimMode::VisualLine | VimMode::VisualBlock
                        ) {
                            let anchor = self.visual_anchor.unwrap_or(self.cursor);
                            let first = row_for_cursor(&rows, self.cursor.min(anchor));
                            let last = row_for_cursor(&rows, self.cursor.max(anchor));
                            self.cursor = rows[first].start;
                            (last - first + 1).max(2)
                        } else {
                            count.max(2)
                        };
                        self.leave_visual();
                        return (true, self.join_lines(text, count, false), None);
                    }
                    if operator.is_none() && matches!(ch, 'u' | 'U' | '~') {
                        let op = match ch {
                            'u' => Operator::Lowercase,
                            'U' => Operator::Uppercase,
                            _ => Operator::ToggleCase,
                        };
                        if self.mode == VimMode::VisualBlock {
                            return self.apply_block(text, ch, count);
                        }
                        if self.mode != VimMode::Normal {
                            let range = self.selection(text).unwrap_or(self.cursor..self.cursor);
                            return self.apply_range(text, op, range);
                        }
                        self.pending =
                            Some(Pending::Operator(op, count, explicit || explicit_prefix));
                        return (true, false, None);
                    }
                    // gugu, gUgU and g~g~ are the long forms of guu, gUU and g~~.
                    if let Some(op) = operator
                        && op.changes_case()
                        && ch == op.key()
                    {
                        let row = row_for_cursor(&rows, self.cursor);
                        return self.apply_lines(text, op, row, row.saturating_add(count - 1));
                    }
                    if operator.is_none() && ch == 'v' {
                        self.reselect_visual(text);
                        return (true, false, None);
                    }
                    if matches!(ch, 'e' | 'E' | '_') {
                        let target = if ch == '_' {
                            let row = row_for_cursor(&rows, self.cursor);
                            last_nonblank(
                                &chars,
                                rows[row
                                    .saturating_add(count - 1)
                                    .min(last_line_row(&rows).max(row))],
                            )
                        } else {
                            repeat_motion(&chars, self.cursor, count, ch == 'E', word_end_backward)
                        };
                        if let Some(op) = operator {
                            if matches!(op, Operator::Indent | Operator::Outdent) {
                                return self.apply_lines(
                                    text,
                                    op,
                                    row_for_cursor(&rows, self.cursor),
                                    row_for_cursor(&rows, target),
                                );
                            }
                            return self.apply_range(
                                text,
                                op,
                                self.cursor.min(target)..self.cursor.max(target).saturating_add(1),
                            );
                        }
                        self.cursor = target;
                        self.visual_to_line_end = false;
                        self.preferred_column = None;
                        return (true, false, None);
                    }
                    if ch != 'g' {
                        if !ch.is_whitespace() {
                            self.unsupported(&format!(
                                "{}g{ch}",
                                operator.map(|op| op.command()).unwrap_or_default()
                            ));
                        }
                        return (true, false, None);
                    }
                    let row = if explicit_prefix || explicit {
                        count.saturating_sub(1).min(last_line_row(&rows))
                    } else {
                        0
                    };
                    if let Some(op) = operator {
                        return self.apply_lines(text, op, row_for_cursor(&rows, self.cursor), row);
                    }
                    self.cursor = first_nonblank(&chars, rows[row]);
                    self.visual_to_line_end = false;
                    self.preferred_column = None;
                    return (true, false, None);
                }
                Pending::Operator(op, prefix, explicit_prefix) => {
                    let count = prefix.saturating_mul(count).min(MAX_COUNT);
                    if ch == 's'
                        && matches!(op, Operator::Yank | Operator::Delete | Operator::Change)
                    {
                        self.pending = Some(if op == Operator::Yank {
                            Pending::Operator(
                                Operator::Surround,
                                count,
                                explicit || explicit_prefix,
                            )
                        } else {
                            Pending::SurroundTarget {
                                change: op == Operator::Change,
                                count,
                            }
                        });
                        return (true, false, None);
                    }
                    if ch == op.key() {
                        let rows = lines(&chars);
                        let row = row_for_cursor(&rows, self.cursor);
                        return self.apply_lines(
                            text,
                            op,
                            row,
                            row.saturating_add(count - 1).min(rows.len() - 1),
                        );
                    }
                    if ch == 'g' {
                        self.pending = Some(Pending::Go {
                            operator: Some(op),
                            count,
                            explicit: explicit || explicit_prefix,
                        });
                        return (true, false, None);
                    }
                    if matches!(ch, 'i' | 'a') {
                        self.pending = Some(Pending::TextObject {
                            operator: Some(op),
                            count,
                            around: ch == 'a',
                        });
                        return (true, false, None);
                    }
                    if matches!(ch, 'f' | 'F' | 't' | 'T') {
                        self.pending = Some(Pending::Find {
                            operator: Some(op),
                            count,
                            forward: ch.is_lowercase(),
                            till: matches!(ch, 't' | 'T'),
                        });
                        return (true, false, None);
                    }
                    if matches!(ch, ';' | ',') {
                        if let Some(mut find) = self.last_find {
                            if ch == ',' {
                                find.forward = !find.forward;
                            }
                            return self.find(text, find, count, Some(op), true);
                        }
                        return (true, false, None);
                    }
                    return self.apply_operator_motion(
                        text,
                        op,
                        ch,
                        count,
                        explicit || explicit_prefix,
                    );
                }
                Pending::RegisterPaste(prefix) => {
                    if !matches!(ch, 'p' | 'P') {
                        return (true, false, None);
                    }
                    let count = prefix.saturating_mul(count).min(MAX_COUNT);
                    let register = self.register.take();
                    self.register = self.yank_register.clone();
                    let result = if self.mode == VimMode::Normal {
                        (true, self.paste(text, ch == 'P', count), None)
                    } else {
                        self.paste_visual(text, count, ch == 'P')
                    };
                    if result.2.is_none() {
                        self.register = register;
                    }
                    return result;
                }
                Pending::Replace(_)
                | Pending::Register(_)
                | Pending::SurroundAdd(..)
                | Pending::SurroundTarget { .. }
                | Pending::SurroundChange { .. }
                | Pending::Find { .. }
                | Pending::TextObject { .. }
                | Pending::Scroll { .. }
                | Pending::Ignore
                | Pending::Bracket(_) => {
                    unreachable!()
                }
            }
        }

        if matches!(
            self.mode,
            VimMode::Visual | VimMode::VisualLine | VimMode::VisualBlock
        ) {
            match ch {
                'y' | 'd' | 'x' | 'c' | 's' | '>' | '<' | 'u' | 'U' | '~' => {
                    return self.apply_visual(text, ch, count);
                }
                'p' | 'P' => return self.paste_visual(text, count, ch == 'P'),
                'I' | 'A' if self.mode == VimMode::VisualBlock => {
                    return self.apply_block(text, ch, count);
                }
                'S' => {
                    if self.mode == VimMode::VisualBlock {
                        self.unsupported("Visual Block S");
                        return (true, false, None);
                    }
                    if let Some(range) = self.selection(text) {
                        self.pending = Some(Pending::SurroundAdd(range.start, range.end));
                    }
                    return (true, false, None);
                }
                'o' => {
                    if let Some((anchor, head)) = self.block_columns.as_mut() {
                        std::mem::swap(anchor, head);
                    }
                    if let Some(anchor) = self.visual_anchor.as_mut() {
                        std::mem::swap(anchor, &mut self.cursor);
                    }
                    return (true, false, None);
                }
                'r' => {
                    self.pending = Some(Pending::Replace(count));
                    return (true, false, None);
                }
                'i' | 'a' => {
                    self.pending = Some(Pending::TextObject {
                        operator: None,
                        count,
                        around: ch == 'a',
                    });
                    return (true, false, None);
                }
                'J' => {
                    let rows = lines(&chars);
                    let anchor = self.visual_anchor.unwrap_or(self.cursor);
                    let first = row_for_cursor(&rows, self.cursor.min(anchor));
                    let last = row_for_cursor(&rows, self.cursor.max(anchor));
                    self.cursor = rows[first].start;
                    self.leave_visual();
                    return (
                        true,
                        self.join_lines(text, (last - first + 1).max(2), true),
                        None,
                    );
                }
                _ => {}
            }
        }

        match ch {
            '"' => {
                self.pending = Some(Pending::Register(count));
                (true, false, None)
            }
            '.' if self.mode == VimMode::Normal => self.repeat_change(text, count, explicit),
            ch if super::navigation::direct_motion(ch) => {
                self.move_cursor(&chars, ch, count, explicit);
                (true, false, None)
            }
            'z' => {
                self.pending = Some(Pending::Scroll { count, explicit });
                (true, false, None)
            }
            'g' => {
                self.pending = Some(Pending::Go {
                    operator: None,
                    count,
                    explicit,
                });
                (true, false, None)
            }
            'v' | 'V' => {
                let target = if ch == 'v' {
                    VimMode::Visual
                } else {
                    VimMode::VisualLine
                };
                if self.mode == target {
                    self.leave_visual();
                } else {
                    if self.visual_anchor.is_none() {
                        self.visual_anchor = Some(self.cursor);
                        self.visual_to_line_end = false;
                    }
                    self.mode = target;
                    self.block_columns = None;
                }
                (true, false, None)
            }
            'f' | 'F' | 't' | 'T' => {
                self.pending = Some(Pending::Find {
                    operator: None,
                    count,
                    forward: ch.is_lowercase(),
                    till: matches!(ch, 't' | 'T'),
                });
                (true, false, None)
            }
            ';' | ',' => {
                if let Some(mut find) = self.last_find {
                    if ch == ',' {
                        find.forward = !find.forward;
                    }
                    self.find(text, find, count, None, true)
                } else {
                    (true, false, None)
                }
            }
            'i' | 'I' | 'a' | 'A' if self.mode == VimMode::Normal => {
                let rows = lines(&chars);
                let line = rows[row_for_cursor(&rows, self.cursor)];
                let at = match ch {
                    'I' => first_nonblank(&chars, line),
                    'a' => (self.cursor + usize::from(line.end > line.start)).min(line.end),
                    'A' => line.end,
                    _ => self.cursor,
                };
                self.start_insert(text, at);
                self.insert_count = count;
                (true, false, None)
            }
            'o' | 'O' if self.mode == VimMode::Normal => {
                self.open_line(text, ch == 'O');
                self.insert_count = count;
                (true, true, None)
            }
            'd' | 'c' | 'y' | '>' | '<' if self.mode == VimMode::Normal => {
                let op = operator_for(ch);
                self.pending = Some(Pending::Operator(op, count, explicit));
                (true, false, None)
            }
            'D' | 'C' if self.mode == VimMode::Normal => self.apply_operator_motion(
                text,
                if ch == 'D' {
                    Operator::Delete
                } else {
                    Operator::Change
                },
                '$',
                count,
                explicit,
            ),
            'Y' | 'S' if self.mode == VimMode::Normal => {
                let rows = lines(&chars);
                let row = row_for_cursor(&rows, self.cursor);
                self.apply_lines(
                    text,
                    if ch == 'S' {
                        Operator::Change
                    } else {
                        Operator::Yank
                    },
                    row,
                    row.saturating_add(count - 1).min(rows.len() - 1),
                )
            }
            'x' | 's' if self.mode == VimMode::Normal => {
                let rows = lines(&chars);
                let line = rows[row_for_cursor(&rows, self.cursor)];
                self.apply_range(
                    text,
                    if ch == 's' {
                        Operator::Change
                    } else {
                        Operator::Delete
                    },
                    self.cursor..self.cursor.saturating_add(count).min(line.end),
                )
            }
            'X' if self.mode == VimMode::Normal => {
                let rows = lines(&chars);
                let line = rows[row_for_cursor(&rows, self.cursor)];
                self.apply_range(
                    text,
                    Operator::Delete,
                    self.cursor.saturating_sub(count).max(line.start)..self.cursor,
                )
            }
            '~' if self.mode == VimMode::Normal => {
                let rows = lines(&chars);
                let line = rows[row_for_cursor(&rows, self.cursor)];
                let end = self.cursor.saturating_add(count).min(line.end);
                let old_len = chars.len();
                let result = self.apply_range(text, Operator::ToggleCase, self.cursor..end);
                if end > self.cursor {
                    self.cursor =
                        end.saturating_add_signed(text.chars().count() as isize - old_len as isize);
                }
                result
            }
            'p' | 'P' if self.mode == VimMode::Normal => {
                (true, self.paste(text, ch == 'P', count), None)
            }
            'r' => {
                self.pending = Some(Pending::Replace(count));
                (true, false, None)
            }
            'J' => {
                self.leave_visual();
                (
                    true,
                    self.join_lines(text, if explicit { count.max(2) } else { 2 }, true),
                    None,
                )
            }
            'u' => {
                self.leave_visual();
                let mut changed = false;
                for _ in 0..count {
                    if !self.restore(text, false) {
                        break;
                    }
                    changed = true;
                }
                (true, changed, None)
            }
            // Commands vEdit does not implement. Swallow their argument so
            // that it cannot run as a command of its own.
            'q' | '@' | 'm' | '`' | '\'' | 'Z' => {
                self.unsupported(&ch.to_string());
                self.pending = Some(Pending::Ignore);
                (true, false, None)
            }
            '[' | ']' => {
                self.pending = Some(Pending::Bracket(ch));
                (true, false, None)
            }
            _ => {
                if !ch.is_whitespace() && !ch.is_control() {
                    self.unsupported(&ch.to_string());
                }
                (true, false, None)
            }
        }
    }

    pub(super) fn unsupported(&mut self, command: &str) {
        self.message = Some(format!("Not supported in vEdit: {command}"));
    }

    pub(super) fn take_count(&mut self) -> usize {
        let count = self.count.max(1);
        self.count = 0;
        count
    }

    pub(super) fn clear_command(&mut self) {
        self.pending = None;
        self.count = 0;
        self.preferred_column = None;
    }
}

fn operator_for(ch: char) -> Operator {
    match ch {
        'd' | 'x' => Operator::Delete,
        'c' | 's' => Operator::Change,
        '>' => Operator::Indent,
        '<' => Operator::Outdent,
        _ => Operator::Yank,
    }
}

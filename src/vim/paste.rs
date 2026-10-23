//! Register paste, character replacement and surrounding edits.
use super::*;

impl VimState {
    pub(super) fn add_surround(
        &mut self,
        text: &mut String,
        range: Range<usize>,
        desired: char,
    ) -> bool {
        let bytes = char_to_byte(text, range.start)..char_to_byte(text, range.end);
        let Some(replacement) = surrounded(&text[bytes.clone()], desired) else {
            return false;
        };
        self.replace_surround(text, range.start, bytes, &replacement)
    }

    pub(super) fn change_surround(
        &mut self,
        text: &mut String,
        range: Range<usize>,
        target: char,
        desired: Option<char>,
    ) -> bool {
        let contents = char_to_byte(text, range.start + 1)..char_to_byte(text, range.end - 1);
        let mut contents = &text[contents];
        if matches!(target, '(' | '[' | '{' | '<') {
            contents = contents.trim_matches([' ', '\t']);
        }
        let replacement = if let Some(desired) = desired {
            let Some(replacement) = surrounded(contents, desired) else {
                return false;
            };
            replacement
        } else {
            contents.to_owned()
        };
        let bytes = char_to_byte(text, range.start)..char_to_byte(text, range.end);
        self.replace_surround(text, range.start, bytes, &replacement)
    }

    fn replace_surround(
        &mut self,
        text: &mut String,
        cursor: usize,
        bytes: Range<usize>,
        replacement: &str,
    ) -> bool {
        let changed = &text[bytes.clone()] != replacement;
        let before = self.snapshot(text);
        text.replace_range(bytes, replacement);
        self.cursor = cursor;
        self.leave_visual();
        self.clear_command();
        if changed {
            self.record(before);
        }
        changed
    }

    pub(super) fn paste_visual(
        &mut self,
        text: &mut String,
        count: usize,
        preserve_register: bool,
    ) -> (bool, bool, Option<Yank>) {
        if self.mode == VimMode::VisualBlock {
            return self.paste_visual_block(text, count, preserve_register);
        }
        let Some(register) = self.register.clone() else {
            self.leave_visual();
            return (true, false, None);
        };
        let Some(range) = self.selection(text) else {
            return (true, false, None);
        };
        let linewise = self.mode == VimMode::VisualLine;
        let before = self.snapshot(text);
        let bytes = char_to_byte(text, range.start)..char_to_byte(text, range.end);
        let mut removed = text[bytes.clone()].to_owned();
        if linewise && !removed.ends_with('\n') {
            removed.push_str(line_ending(text));
        }
        let deleted = Yank {
            text: removed,
            linewise,
            block_width: None,
        };
        let Some(mut pasted) = repeated_text(&register.text, count) else {
            return (true, false, None);
        };
        if !linewise && register.linewise {
            if !pasted.ends_with('\n') {
                pasted.push_str(line_ending(text));
            }
            pasted.insert_str(0, line_ending(text));
            if range.end == text.chars().count() && !text.ends_with('\n') {
                remove_last_line_ending(&mut pasted);
            }
        }
        if linewise {
            if !pasted.ends_with('\n') {
                pasted.push_str(line_ending(text));
            }
            if range.end == text.chars().count() && !text.ends_with('\n') {
                remove_last_line_ending(&mut pasted);
            }
        }
        let changed = text[bytes.clone()] != pasted;
        text.replace_range(bytes, &pasted);
        self.cursor = if linewise {
            range.start
        } else {
            range.start + pasted.chars().count().saturating_sub(1)
        };
        self.leave_visual();
        let yank = if preserve_register {
            None
        } else {
            self.register = Some(deleted.clone());
            self.yanked_ranges.clear();
            self.yanked_ranges.push(range);
            Some(deleted)
        };
        if changed {
            self.record(before);
        }
        (true, changed, yank)
    }

    pub(super) fn paste(&mut self, text: &mut String, before_cursor: bool, count: usize) -> bool {
        if self
            .register
            .as_ref()
            .is_some_and(|yank| yank.block_width.is_some())
        {
            return self.paste_block(text, before_cursor, count);
        }
        let Some(yank) = self.register.as_ref() else {
            return false;
        };
        if yank.text.is_empty() {
            return false;
        }
        let chars: Vec<char> = text.chars().collect();
        let rows = lines(&chars);
        let line = rows[row_for_cursor(&rows, self.cursor)];
        let Some(mut pasted) = repeated_text(&yank.text, count) else {
            return false;
        };
        let (at, cursor) = if yank.linewise {
            if !pasted.ends_with('\n') {
                pasted.push_str(line_ending(text));
            }
            if text.is_empty() {
                remove_last_line_ending(&mut pasted);
                (0, 0)
            } else if before_cursor {
                (line.start, line.start)
            } else if let Some(newline) = line.newline {
                (newline + 1, newline + 1)
            } else {
                remove_last_line_ending(&mut pasted);
                let ending = line_ending(text);
                pasted.insert_str(0, ending);
                (line.end, line.end + ending.chars().count())
            }
        } else {
            let at = if before_cursor {
                self.cursor
            } else {
                (self.cursor + usize::from(line.end > line.start)).min(line.end)
            };
            let cursor = if pasted.contains('\n') {
                at
            } else {
                at + pasted.chars().count().saturating_sub(1)
            };
            (at, cursor)
        };
        if pasted.is_empty() {
            return false;
        }
        if text.len().saturating_add(pasted.len()) > MAX_PASTE_BYTES {
            return false;
        }
        let snapshot = self.snapshot(text);
        text.insert_str(char_to_byte(text, at), &pasted);
        self.cursor = cursor;
        if yank.linewise {
            let chars: Vec<char> = text.chars().collect();
            let rows = lines(&chars);
            self.cursor = first_nonblank(&chars, rows[row_for_cursor(&rows, self.cursor)]);
        }
        self.record(snapshot);
        self.preferred_column = None;
        true
    }

    pub(super) fn replace_characters(
        &mut self,
        text: &mut String,
        count: usize,
        replacement: char,
    ) -> bool {
        if self.mode == VimMode::VisualBlock {
            return self.replace_block_characters(text, replacement);
        }
        let visual = self.selection(text);
        let chars: Vec<char> = text.chars().collect();
        let rows = lines(&chars);
        let line = rows[row_for_cursor(&rows, self.cursor)];
        let selection = if let Some(range) = visual {
            range
        } else {
            if self.cursor.saturating_add(count) > line.end {
                return false;
            }
            self.cursor..self.cursor + count
        };
        let mut incoming = String::new();
        for index in selection.clone() {
            let ch = chars[index];
            incoming.push(
                if ch == '\n' || (ch == '\r' && chars.get(index + 1) == Some(&'\n')) {
                    ch
                } else {
                    replacement
                },
            );
        }
        let replacing_with_line_break = self.mode == VimMode::Normal && replacement == '\n';
        if replacing_with_line_break {
            incoming = line_ending(text).to_owned();
        }
        let range = char_to_byte(text, selection.start)..char_to_byte(text, selection.end);
        let changed = text[range.clone()] != incoming;
        let snapshot = self.snapshot(text);
        text.replace_range(range, &incoming);
        self.cursor = if replacing_with_line_break {
            selection.start + incoming.chars().count()
        } else if replacement == '\n' {
            selection.start + 1
        } else {
            selection.start + incoming.chars().count().saturating_sub(1)
        };
        self.leave_visual();
        if changed {
            self.record(snapshot);
        }
        changed
    }
}

pub(super) fn surround_range(
    chars: &[char],
    cursor: usize,
    target: char,
    count: usize,
) -> Option<Range<usize>> {
    let target = match target {
        'r' => '[',
        'a' => '<',
        target => target,
    };
    if matches!(target, '\'' | '"' | '`') {
        let inner = text_object(chars, cursor, target, 1, false)?;
        Some(inner.start - 1..inner.end + 1)
    } else if matches!(
        target,
        '(' | ')' | 'b' | '[' | ']' | '{' | '}' | 'B' | '<' | '>'
    ) {
        text_object(chars, cursor, target, count, true)
    } else {
        None
    }
}

fn surrounded(contents: &str, desired: char) -> Option<String> {
    let (open, close, padded) = match desired {
        '(' => ('(', ')', true),
        ')' | 'b' => ('(', ')', false),
        '[' => ('[', ']', true),
        ']' | 'r' => ('[', ']', false),
        '{' => ('{', '}', true),
        '}' | 'B' => ('{', '}', false),
        '<' => ('<', '>', true),
        '>' | 'a' => ('<', '>', false),
        '\'' | '"' | '`' => (desired, desired, false),
        _ => return None,
    };
    Some(if padded {
        format!("{open} {contents} {close}")
    } else {
        format!("{open}{contents}{close}")
    })
}

pub(super) fn repeated_text(text: &str, count: usize) -> Option<String> {
    if text.len().checked_mul(count)? > MAX_PASTE_BYTES {
        return None;
    }
    Some(text.repeat(count))
}

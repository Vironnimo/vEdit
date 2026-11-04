//! Replace-mode input within the existing Insert transaction and history.
use super::*;

#[derive(Clone, Debug)]
struct Step {
    at: usize,
    original: Option<char>,
    written: char,
    crlf: bool,
    layout: Option<Box<Layout>>,
}

#[derive(Clone, Debug)]
struct Layout {
    original: String,
    written: String,
    cursor: usize,
}

impl Step {
    fn written_len(&self) -> usize {
        if let Some(layout) = &self.layout {
            return layout.cursor;
        }
        if self.crlf { 2 } else { 1 }
    }

    fn written_text(&self) -> String {
        if let Some(layout) = &self.layout {
            return layout.written.clone();
        }
        if self.crlf {
            "\r\n".into()
        } else {
            self.written.to_string()
        }
    }

    fn original_text(&self) -> String {
        self.layout.as_ref().map_or_else(
            || self.original.map_or_else(String::new, |ch| ch.to_string()),
            |layout| layout.original.clone(),
        )
    }

    fn changed(&self) -> bool {
        self.layout
            .as_ref()
            .map_or(self.original != Some(self.written), |layout| {
                layout.original != layout.written
            })
    }
}

#[derive(Clone, Debug)]
pub(super) struct Session {
    steps: Vec<Step>,
    keys: Vec<VimKey>,
    last_segment: Vec<VimKey>,
    count: usize,
    prefix: Vec<VimKey>,
    changed: bool,
}

impl Session {
    pub(super) fn new(count: usize) -> Self {
        let count = count.clamp(1, MAX_COUNT);
        let mut prefix: Vec<_> = if count > 1 {
            count.to_string().chars().map(VimKey::Char).collect()
        } else {
            Vec::new()
        };
        prefix.push(VimKey::Char('R'));
        Self {
            steps: Vec::new(),
            keys: Vec::new(),
            last_segment: Vec::new(),
            count,
            prefix,
            changed: false,
        }
    }

    pub(super) fn changed(&self) -> bool {
        self.changed || self.steps.iter().any(Step::changed)
    }

    pub(super) fn remap(&mut self, map: &impl Fn(usize) -> usize) {
        for step in &mut self.steps {
            step.at = map(step.at);
        }
    }
}

impl VimState {
    pub fn is_replacing(&self) -> bool {
        self.mode == VimMode::Insert && self.replace.is_some()
    }

    pub fn start_replace(&mut self, text: &str, count: usize) {
        self.start_insert(text, self.cursor);
        self.replace = Some(Session::new(count));
    }

    /// A mouse or external cursor move ends Backspace restoration at the old
    /// position. Subsequent dot repeat starts with the next replacement there.
    pub(super) fn break_replace_restoration(&mut self) {
        if let Some(session) = &mut self.replace {
            session.changed = session.changed();
            session.steps.clear();
            if !session.keys.is_empty() {
                session.last_segment = std::mem::take(&mut session.keys);
            }
            session.count = 1;
            session.prefix = vec![VimKey::Char('R')];
        }
    }

    pub(super) fn replace_change_keys(&self) -> Option<Vec<VimKey>> {
        let session = self.replace.as_ref()?;
        let mut keys = session.prefix.clone();
        keys.extend_from_slice(if session.keys.is_empty() {
            &session.last_segment
        } else {
            &session.keys
        });
        keys.push(VimKey::Escape);
        Some(keys)
    }

    /// Overwrite scalar characters without crossing an existing line ending;
    /// text after the end of a line extends that line. Newlines split it.
    pub(super) fn replace_text(&mut self, text: &mut String, inserted: &str) -> Action {
        if !self.is_replacing() || inserted.is_empty() {
            return Action::default();
        }
        let chars: Vec<_> = text.chars().collect();
        let start = self.cursor.min(chars.len());
        let mut old = start;
        let mut cursor = start;
        let mut written = String::with_capacity(inserted.len());
        let mut steps = Vec::new();
        let ending = line_ending(text);
        let mut input = inserted.chars().peekable();
        while let Some(ch) = input.next() {
            let newline = ch == '\n' || ch == '\r' && input.peek() == Some(&'\n');
            if ch == '\r' && newline {
                input.next();
            }
            let replacement = if newline {
                ending.to_owned()
            } else {
                ch.to_string()
            };
            let original = if !newline
                && old < chars.len()
                && chars[old] != '\n'
                && !(chars[old] == '\r' && chars.get(old + 1) == Some(&'\n'))
            {
                let original = chars[old];
                old += 1;
                Some(original)
            } else {
                None
            };
            steps.push(Step {
                at: cursor,
                original,
                written: if newline { '\n' } else { ch },
                crlf: newline && ending == "\r\n",
                layout: None,
            });
            cursor += replacement.chars().count();
            written.push_str(&replacement);
        }
        let bytes = char_to_byte(text, start)..char_to_byte(text, old);
        if written.len() > bytes.len()
            && text
                .len()
                .saturating_sub(bytes.len())
                .saturating_add(written.len())
                > MAX_PASTE_BYTES
        {
            return Action {
                consumed: true,
                message: Some("Replacement exceeds the text size limit".into()),
                ..Action::default()
            };
        }
        let changed = text[bytes.clone()] != written;
        text.replace_range(bytes, &written);
        self.cursor = cursor;
        self.preferred_column = None;
        let session = self.replace.as_mut().unwrap();
        for step in &steps {
            // A paste/register/IME payload is literal. Physical Tab/Enter are
            // separate keys and regenerate indentation when dot is replayed.
            session.keys.push(VimKey::Char(step.written));
        }
        session.steps.extend(steps);
        Action {
            consumed: true,
            text_changed: changed,
            cursor_changed: cursor != start,
            ..Action::default()
        }
    }

    pub(super) fn finish_replace_repetitions(&mut self, text: &mut String) -> bool {
        let Some(session) = self.replace.as_mut() else {
            return false;
        };
        let count = std::mem::replace(&mut session.count, 1);
        if count <= 1 || session.keys.is_empty() {
            return false;
        }
        let keys = session.keys.clone();
        let mut changed = false;
        if keys.iter().all(|key| matches!(key, VimKey::Char(_))) {
            let unit: String = keys
                .iter()
                .map(|key| match key {
                    VimKey::Char(ch) => *ch,
                    VimKey::Enter => '\n',
                    VimKey::Tab => '\t',
                    _ => unreachable!(),
                })
                .collect();
            if let Some(repeated) = repeated_text(&unit, count - 1) {
                changed = self.replace_text(text, &repeated).text_changed;
            }
        } else {
            for _ in 1..count {
                for key in &keys {
                    changed |= self.apply_replace_key(text, *key).1;
                }
            }
        }
        // Count expansion is derived input, not a larger dot recipe.
        self.replace.as_mut().unwrap().keys = keys;
        changed
    }

    pub(super) fn handle_replace_key(
        &mut self,
        text: &mut String,
        key: VimKey,
    ) -> (bool, bool, Option<Yank>) {
        if key == VimKey::Escape {
            let expanded = self.finish_replace_repetitions(text);
            self.replace = None;
            let mut result = self.handle_insert_key(text, key);
            result.1 |= expanded;
            return result;
        }
        self.apply_replace_key(text, key)
    }

    fn apply_replace_key(&mut self, text: &mut String, key: VimKey) -> (bool, bool, Option<Yank>) {
        if matches!(key, VimKey::Tab | VimKey::Enter) {
            let at = self.cursor;
            let edit = if key == VimKey::Enter {
                let indent = if self.indent_with_tabs {
                    "\t".into()
                } else {
                    " ".repeat(self.indent_width)
                };
                crate::editor::smart_newline(text, at..at, &indent)
            } else {
                let inserted = crate::editor::insert_tab(
                    text,
                    at..at,
                    self.indent_width,
                    self.indent_with_tabs,
                );
                let value: String = inserted
                    .text
                    .chars()
                    .skip(at)
                    .take(inserted.selection.start - at)
                    .collect();
                let chars: Vec<_> = text.chars().collect();
                let line = line_at_cursor(&chars, at);
                crate::editor::replace_selection(
                    text,
                    at..(at + value.chars().count()).min(line.end),
                    &value,
                )
            };
            if edit.text.len() > text.len() && edit.text.len() > MAX_PASTE_BYTES {
                self.message = Some("Replacement exceeds the text size limit".into());
                return (true, false, None);
            }
            let changed = edit.text != *text;
            let delta = insert_delta(text, &edit.text, at);
            let start = at.saturating_add_signed(delta.relative_start);
            let original: String = text.chars().skip(start).take(delta.removed).collect();
            let session = self.replace.as_mut().unwrap();
            session.keys.push(key);
            session.steps.push(Step {
                at: start,
                original: None,
                written: if key == VimKey::Enter { '\n' } else { '\t' },
                crlf: false,
                layout: Some(Box::new(Layout {
                    original,
                    written: delta.inserted,
                    cursor: edit.selection.start - start,
                })),
            });
            *text = edit.text;
            self.cursor = edit.selection.start;
            self.preferred_column = None;
            return (true, changed, None);
        }
        let inserted = match key {
            VimKey::Char(ch) => Some(ch.to_string()),
            _ => None,
        };
        if let Some(inserted) = inserted {
            let action = self.replace_text(text, &inserted);
            self.message = action.message;
            return (true, action.text_changed, None);
        }
        if key == VimKey::Backspace {
            let session = self.replace.as_mut().unwrap();
            session.keys.push(key);
            if let Some(step) = session
                .steps
                .pop_if(|step| step.at + step.written_len() == self.cursor)
            {
                let written = step.written_text();
                let end = if step.layout.is_some() {
                    step.at + written.chars().count()
                } else {
                    self.cursor
                };
                let range = char_to_byte(text, step.at)..char_to_byte(text, end);
                if text[range.clone()] == written {
                    let changed = step.changed();
                    text.replace_range(range, &step.original_text());
                    self.cursor = step.at;
                    return (true, changed, None);
                }
                session.steps.clear();
            }
            let chars: Vec<_> = text.chars().collect();
            let line = line_at_cursor(&chars, self.cursor);
            self.cursor = self.cursor.saturating_sub(1).max(line.start);
            return (true, false, None);
        }
        let session = self.replace.as_mut().unwrap();
        session.changed = session.changed();
        session.steps.clear();
        if key == VimKey::Delete {
            session.keys.push(key);
        } else if matches!(
            key,
            VimKey::Left | VimKey::Right | VimKey::Up | VimKey::Down | VimKey::Home | VimKey::End
        ) {
            if !session.keys.is_empty() {
                session.last_segment = std::mem::take(&mut session.keys);
            }
            session.count = 1;
            session.prefix = vec![VimKey::Char('R')];
        }
        let chars: Vec<_> = text.chars().collect();
        let line = line_at_cursor(&chars, self.cursor);
        self.cursor = match key {
            VimKey::Left => self.cursor.saturating_sub(1).max(line.start),
            VimKey::Right => self.cursor.saturating_add(1).min(line.end),
            VimKey::Home => line.start,
            VimKey::End => line.end,
            VimKey::Up | VimKey::Down => {
                let column = *self
                    .preferred_column
                    .get_or_insert(self.cursor - line.start);
                let target = relative_line(&chars, line, 1, key == VimKey::Down);
                target.start + column.min(target.end - target.start)
            }
            VimKey::Delete if self.cursor < chars.len() => {
                let removed =
                    if chars[self.cursor] == '\r' && chars.get(self.cursor + 1) == Some(&'\n') {
                        2
                    } else {
                        1
                    };
                text.replace_range(
                    char_to_byte(text, self.cursor)..char_to_byte(text, self.cursor + removed),
                    "",
                );
                self.replace.as_mut().unwrap().changed = true;
                return (true, true, None);
            }
            _ => self.cursor,
        };
        if !matches!(key, VimKey::Up | VimKey::Down) {
            self.preferred_column = None;
        }
        (true, false, None)
    }
}

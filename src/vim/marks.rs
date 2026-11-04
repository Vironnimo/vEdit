//! Document marks and change locations, with pane-local jump context.
use super::*;

const MAX_MARK_DIFF_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, Default)]
pub(super) struct Marks {
    pub named: [Option<usize>; 26],
    pub insert: Option<usize>,
    change: Option<usize>,
    affected: Option<(usize, usize)>,
    visual: Option<(usize, usize)>,
    changes: Vec<usize>,
}

impl VimState {
    /// Application navigation uses the same context mark as Vim jumps.
    pub fn set_previous_context(&mut self, cursor: usize) {
        self.previous_context = Some(cursor);
    }

    /// Origins of explicit Vim jumps, including short moves, for pane history.
    pub fn take_jump_origins(&mut self) -> Vec<usize> {
        std::mem::take(&mut self.jump_origins)
    }

    pub(super) fn remember_jump(&mut self) {
        self.previous_context = Some(self.cursor);
        self.jump_origins.push(self.cursor);
        if self.jump_origins.len() > MAX_HISTORY {
            self.jump_origins.remove(0);
        }
    }

    pub(super) fn set_mark(&mut self, name: char) {
        match name {
            'a'..='z' => self.marks.named[name as usize - 'a' as usize] = Some(self.cursor),
            '\'' | '`' => self.previous_context = Some(self.cursor),
            _ => self.unsupported(&format!("m{name}")),
        }
    }

    fn mark(&self, name: char) -> Option<usize> {
        match name {
            'a'..='z' => self.marks.named[name as usize - 'a' as usize],
            '\'' | '`' => self.previous_context,
            '^' => self.marks.insert,
            '.' => self.marks.change,
            '[' => self.marks.affected.map(|range| range.0),
            ']' => self.marks.affected.map(|range| range.1),
            '<' => self.marks.visual.map(|range| range.0),
            '>' => self.marks.visual.map(|range| range.1),
            _ => None,
        }
    }

    pub(super) fn jump_to_mark(
        &mut self,
        text: &mut String,
        name: char,
        operator: Option<Operator>,
        linewise: bool,
        keep_jumps: bool,
    ) -> (bool, bool, Option<Yank>) {
        let Some(target) = self.mark(name) else {
            self.message = Some(format!("Mark not set: {name}"));
            return (true, false, None);
        };
        let chars: Vec<_> = text.chars().collect();
        let rows = lines(&chars);
        let target = target.min(chars.len());
        if let Some(op) = operator {
            return if linewise || matches!(op, Operator::Indent | Operator::Outdent) {
                self.apply_lines(
                    text,
                    op,
                    row_for_cursor(&rows, self.cursor),
                    row_for_cursor(&rows, target),
                )
            } else {
                self.apply_exclusive(text, op, self.cursor.min(target), self.cursor.max(target))
            };
        }
        if !keep_jumps {
            self.remember_jump();
        }
        self.cursor = if linewise {
            first_nonblank(&chars, rows[row_for_cursor(&rows, target)])
        } else {
            target
        };
        self.preferred_column = None;
        self.visual_to_line_end = false;
        (true, false, None)
    }

    pub(super) fn change_location(&mut self, text: &str, count: usize, backwards: bool) {
        let len = self.marks.changes.len();
        if len == 0 {
            self.message = Some("No change locations in this document".into());
            return;
        }
        let current = self.change_index.unwrap_or(len).min(len);
        if !backwards && current >= len - 1 {
            self.message = Some("No newer change location".into());
            return;
        }
        let next = if backwards {
            current.saturating_sub(count)
        } else {
            current.saturating_add(count).min(len - 1)
        };
        if next == current {
            self.message = Some(
                if backwards {
                    "No older change location"
                } else {
                    "No newer change location"
                }
                .into(),
            );
            return;
        }
        self.change_index = Some(next);
        self.cursor = normal_cursor(text, self.marks.changes[next]);
        self.preferred_column = None;
    }

    pub(super) fn remember_yank(&mut self) {
        if let (Some(first), Some(last)) = (self.yanked_ranges.first(), self.yanked_ranges.last()) {
            self.marks.affected = Some((first.start, last.end.saturating_sub(1).max(first.start)));
        }
    }

    pub(super) fn remember_visual(&mut self, anchor: usize) {
        self.marks.visual = Some((anchor.min(self.cursor), anchor.max(self.cursor)));
    }

    /// Map marks once per completed transaction, including native edits and
    /// disjoint multicursor replacements. Pure navigation never copies text.
    pub(super) fn track_marks(&mut self, before: &str, after: &str, remember: bool, origin: usize) {
        if before == after {
            return;
        }
        // Character diffing an enormous replacement (e.g. recovery of a long
        // generated line) is unbounded work. Use a linear prefix/suffix span;
        // existing marks in that span retain their logical line and column.
        let large = before.len().saturating_add(after.len()) > MAX_MARK_DIFF_BYTES;
        let mut edits = if large {
            let length = before.chars().count();
            let delta = insert_delta(before, after, length);
            let start = length.saturating_add_signed(delta.relative_start);
            vec![crate::multicursor::Replacement {
                range: start..start + delta.removed,
                text: delta.inserted,
            }]
        } else {
            multicursor::replacements(before, after)
        };
        // A diff alone cannot tell where an identical character was inserted
        // or deleted in a run of equal characters. The transaction can.
        if remember && !large && edits.len() == 1 {
            let hint = self
                .insert_replay
                .as_ref()
                .filter(|replay| self.mode == VimMode::Insert && replay.baseline == before)
                .map_or(origin.min(self.cursor), |replay| replay.cursor);
            let delta = insert_delta(before, after, hint);
            if delta.removed == 0 || delta.inserted.is_empty() || self.mode == VimMode::Insert {
                let start = hint.saturating_add_signed(delta.relative_start);
                edits[0] = crate::multicursor::Replacement {
                    range: start..start + delta.removed,
                    text: delta.inserted,
                };
            }
        }
        let map = |position| crate::multicursor::map_position(position, &edits);
        let chars: Vec<_> = before.chars().collect();
        let old_rows = lines(&chars);
        let after_chars: Vec<_> = after.chars().collect();
        let rows = lines(&after_chars);
        // Vim marks keep their column during ordinary text insertion/deletion;
        // their line follows inserted/deleted lines. Clamp to that line only.
        let map_mark = |position| {
            let line = line_at_cursor(&chars, position);
            let target = if large && edits[0].range.contains(&position) && !edits[0].text.is_empty()
            {
                rows[row_for_cursor(&old_rows, position).min(rows.len() - 1)]
            } else {
                rows[row_for_cursor(&rows, map(line.start))]
            };
            target.start
                + position
                    .saturating_sub(line.start)
                    .min(target.end - target.start)
        };
        for mark in &mut self.marks.named {
            if let Some(position) = *mark {
                let line = line_at_cursor(&chars, position);
                let end = line.newline.map_or(line.end, |newline| newline + 1);
                let deleted = edits.iter().any(|edit| {
                    edit.text.is_empty() && edit.range.start <= line.start && edit.range.end >= end
                });
                *mark = (!deleted).then(|| map_mark(position));
            }
        }
        self.marks.insert = self.marks.insert.map(map_mark);
        self.marks.change = self.marks.change.map(map_mark);
        self.marks.affected = self
            .marks
            .affected
            .map(|(start, end)| (map_mark(start), map_mark(end)));
        self.previous_context = self.previous_context.map(map_mark);
        self.marks.visual = self
            .marks
            .visual
            .map(|(start, end)| (map_mark(start), map_mark(end)));
        for position in &mut self.marks.changes {
            *position = map_mark(*position);
        }
        for position in &mut self.jump_origins {
            *position = map(*position);
        }
        if !remember {
            return;
        }
        let first = edits.first().unwrap();
        let last = edits.last().unwrap();
        let start = map(first.range.start).saturating_sub(first.text.chars().count());
        let end = map(last.range.start)
            .saturating_sub(usize::from(!last.text.is_empty()))
            .max(start);
        self.marks.affected = Some((start, end));
        let changed = if self.mode == VimMode::Insert {
            end
        } else if edits.iter().any(|edit| {
            let end = map(edit.range.start);
            let start = end.saturating_sub(edit.text.chars().count());
            (start..=end).contains(&self.cursor)
        }) {
            self.cursor
        } else {
            start
        };
        self.marks.change = Some(changed);
        if let Some(previous) = self.marks.changes.last_mut()
            && row_for_cursor(&rows, *previous) == row_for_cursor(&rows, changed)
            && previous.abs_diff(changed) < 79
        {
            *previous = changed;
        } else {
            self.marks.changes.push(changed);
            if self.marks.changes.len() > MAX_HISTORY {
                self.marks.changes.remove(0);
            }
        }
        self.change_index = None;
    }
}

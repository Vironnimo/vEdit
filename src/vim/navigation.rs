//! Pane-local navigation, selections and modal cursor motions.
use super::*;

impl VimNavigation {
    /// Move a view with its source text while keeping its visual selection's
    /// direction and modal state. The delta uses Unicode character indices.
    pub fn shifted(&self, delta: isize) -> Self {
        let mut navigation = self.remapped(|position| position.saturating_add_signed(delta));
        // Moving or duplicating complete lines preserves their virtual columns,
        // including block endpoints beyond a short line's physical end.
        navigation.block_columns = self.block_columns;
        if let Some(last) = navigation.last_visual.as_mut() {
            last.block_columns = self.last_visual.and_then(|last| last.block_columns);
        }
        navigation
    }

    /// Map every stored position of this view, e.g. after another view edited
    /// the shared document. Modal state and pending commands stay unchanged.
    pub fn remapped(&self, map: impl Fn(usize) -> usize) -> Self {
        let mut navigation = self.clone();
        navigation.cursor = map(navigation.cursor);
        navigation.visual_anchor = navigation.visual_anchor.map(&map);
        navigation.multi.remap(&map);
        navigation.block_columns = None;
        if let Some(last) = navigation.last_visual.as_mut() {
            last.anchor = map(last.anchor);
            last.cursor = map(last.cursor);
            last.block_columns = None;
        }
        navigation
    }

    /// Move a view's cursor and visual anchor through a replacement of the whole
    /// text, e.g. an external reload. Modal state and pending commands remain.
    pub fn mapped(&self, map: impl Fn(usize) -> usize) -> Self {
        let mut navigation = self.clone();
        navigation.cursor = map(navigation.cursor);
        navigation.visual_anchor = navigation.visual_anchor.map(&map);
        navigation.preferred_column = None;
        navigation.multi.remap(&map);
        navigation.block_columns = None;
        navigation
    }
}

impl VimState {
    pub fn navigation(&self) -> VimNavigation {
        VimNavigation {
            mode: self.mode,
            multi: self.multi.clone(),
            cursor: self.cursor,
            visual_anchor: self.visual_anchor,
            pending: self.pending,
            count: self.count,
            preferred_column: self.preferred_column,
            last_find: self.last_find,
            command_keys: self.command_keys.clone(),
            surround_search: self.surround_search.clone(),
            motion_epoch: self.motion_epoch,
            visual_to_line_end: self.visual_to_line_end,
            last_visual: self.last_visual,
            block_columns: self.block_columns,
        }
    }

    /// Restore a view without replacing this document's shared editing history.
    /// Finish an active insert session before switching to another view. An
    /// Insert snapshot starts a new transaction if its original one was finished.
    pub fn restore_navigation(&mut self, text: &str, navigation: &VimNavigation) {
        let len = text.chars().count();
        self.mode = navigation.mode;
        self.multi = navigation.multi.clone();
        self.cursor = navigation.cursor.min(len);
        self.visual_anchor = navigation.visual_anchor.map(|anchor| anchor.min(len));
        self.pending = navigation.pending;
        self.count = navigation.count.min(MAX_COUNT);
        self.preferred_column = navigation.preferred_column;
        self.last_find = navigation.last_find;
        self.command_keys.clone_from(&navigation.command_keys);
        self.surround_search.clone_from(&navigation.surround_search);
        self.motion_epoch = navigation.motion_epoch;
        self.visual_to_line_end = navigation.visual_to_line_end;
        self.last_visual = navigation.last_visual;
        self.block_columns = navigation.block_columns;
        if self.mode == VimMode::Insert && self.insert_origin.is_none() {
            self.insert_origin = Some(self.snapshot(text));
            self.insert_count = 1;
            self.insert_replay = Some(InsertReplay {
                keys: vec![VimKey::Char('i')],
                baseline: text.to_owned(),
                cursor: self.cursor,
                search: None,
                visual: None,
            });
        }
    }

    /// Set an exact half-open range of Unicode scalar indices. Finish an active
    /// insert session first. A nonempty range becomes Visual mode; an empty range
    /// becomes Normal mode at the requested boundary, including end of file.
    pub fn set_selection(&mut self, text: &str, range: Range<usize>) {
        self.set_selection_boundaries(text, range.start, range.end);
    }

    /// Like `set_selection`, preserving which end of the selection is active.
    /// `anchor` and `cursor` are half-open boundaries, as used by a text widget.
    pub fn set_selection_boundaries(&mut self, text: &str, anchor: usize, cursor: usize) {
        self.multi.clear();
        self.motion_epoch = self.motion_epoch.wrapping_add(1);
        self.visual_to_line_end = false;
        self.block_columns = None;
        let len = text.chars().count();
        let anchor = anchor.min(len);
        let cursor = cursor.min(len);
        self.cancel_pending_command();
        if anchor == cursor {
            self.mode = VimMode::Normal;
            self.visual_anchor = None;
            self.cursor = cursor;
        } else {
            self.mode = VimMode::Visual;
            if cursor > anchor {
                self.visual_anchor = Some(anchor);
                self.cursor = cursor - 1;
            } else {
                self.visual_anchor = Some(anchor - 1);
                self.cursor = cursor;
            }
        }
    }

    /// Accept native mouse selection boundaries. Insert mode keeps its current
    /// transaction and native widget selection; modal selections use exact Vim
    /// Visual ranges. A collapsed modal click selects a valid character on its
    /// logical line rather than moving to an insertion boundary past that line.
    pub fn set_mouse_selection(
        &mut self,
        text: &str,
        primary: usize,
        anchor: usize,
        linewise: bool,
    ) {
        if self.mode == VimMode::Insert {
            self.motion_epoch = self.motion_epoch.wrapping_add(1);
            self.cursor = primary.min(text.chars().count());
            self.visual_anchor = None;
            self.cancel_pending_command();
        } else if primary == anchor {
            self.mode = VimMode::Normal;
            self.visual_anchor = None;
            self.set_cursor(text, primary);
        } else {
            self.set_selection_boundaries(text, anchor, primary);
            if linewise && self.mode == VimMode::Visual {
                self.mode = VimMode::VisualLine;
            }
        }
    }

    pub fn set_cursor(&mut self, text: &str, cursor: usize) {
        self.multi.clear();
        self.motion_epoch = self.motion_epoch.wrapping_add(1);
        self.visual_to_line_end = false;
        self.block_columns = None;
        self.cursor = if self.mode == VimMode::Insert {
            char_boundary(text, cursor).0
        } else {
            normal_cursor(text, cursor)
        };
        self.preferred_column = None;
        self.pending = None;
        self.count = 0;
        if self.mode != VimMode::Insert {
            self.command_keys.clear();
        }
    }

    /// The half-open character range that the active view shows completely.
    /// H, M and L use it; without a view the whole document counts.
    pub fn set_visible_range(&mut self, range: Option<Range<usize>>) {
        self.visible = range;
    }

    pub fn motion_epoch(&self) -> u64 {
        self.motion_epoch
    }

    /// Selected characters; the end is exclusive. Line selections include their
    /// newline when one exists. Returns None outside either visual mode.
    pub fn selection(&self, text: &str) -> Option<Range<usize>> {
        let anchor = self.visual_anchor?;
        let len = text.chars().count();
        match self.mode {
            VimMode::Visual => Some(
                anchor.min(self.cursor).min(len)
                    ..anchor.max(self.cursor).saturating_add(1).min(len),
            ),
            VimMode::VisualLine => {
                let chars: Vec<char> = text.chars().collect();
                let rows = lines(&chars);
                let first = rows[row_for_cursor(&rows, anchor.min(self.cursor).min(len))];
                let last = rows[row_for_cursor(&rows, anchor.max(self.cursor).min(len))];
                Some(first.start..last.newline.map_or(last.end, |index| index + 1))
            }
            _ => None,
        }
    }

    /// Move by visible-page rows supplied by the UI, preserving visual mode.
    pub fn scroll_lines(&mut self, text: &mut String, delta: isize) -> Action {
        let prefix = self.take_count();
        self.pending = None;
        let count = delta
            .unsigned_abs()
            .saturating_mul(prefix)
            .clamp(1, MAX_COUNT);
        self.count = count;
        self.handle_key(text, VimKey::Char(if delta < 0 { 'k' } else { 'j' }))
    }

    pub(super) fn clamp_cursor(&mut self, text: &str) {
        self.cursor = if self.mode != VimMode::Normal || self.multi.active() {
            char_boundary(text, self.cursor).0
        } else {
            normal_cursor(text, self.cursor)
        };
    }

    /// `gv`: select the previous Visual area again. In Visual mode the current
    /// and the previous area are exchanged.
    pub(super) fn reselect_visual(&mut self, text: &str) {
        let Some(last) = self.last_visual else {
            return;
        };
        let current = self.visual_anchor.and_then(|anchor| {
            matches!(
                self.mode,
                VimMode::Visual | VimMode::VisualLine | VimMode::VisualBlock
            )
            .then_some(LastVisual {
                mode: self.mode,
                anchor,
                cursor: self.cursor,
                to_line_end: self.visual_to_line_end,
                block_columns: self.block_columns,
            })
        });
        let len = text.chars().count();
        self.mode = last.mode;
        self.visual_anchor = Some(last.anchor.min(len));
        self.cursor = last.cursor.min(len);
        self.visual_to_line_end = last.to_line_end;
        self.block_columns = last.block_columns;
        self.preferred_column = None;
        if current.is_some() {
            self.last_visual = current;
        }
    }

    pub(super) fn leave_visual(&mut self) {
        if matches!(
            self.mode,
            VimMode::Visual | VimMode::VisualLine | VimMode::VisualBlock
        ) && let Some(anchor) = self.visual_anchor
        {
            self.last_visual = Some(LastVisual {
                mode: self.mode,
                anchor,
                cursor: self.cursor,
                to_line_end: self.visual_to_line_end,
                block_columns: self.block_columns,
            });
        }
        self.visual_anchor = None;
        self.visual_to_line_end = false;
        self.block_columns = None;
        if matches!(
            self.mode,
            VimMode::Visual | VimMode::VisualLine | VimMode::VisualBlock
        ) {
            self.mode = VimMode::Normal;
        }
    }

    pub(super) fn move_cursor(&mut self, chars: &[char], key: char, count: usize, explicit: bool) {
        if self.mode == VimMode::VisualBlock && self.move_block_cursor(chars, key, count) {
            return;
        }
        if matches!(
            self.mode,
            VimMode::Visual | VimMode::VisualLine | VimMode::VisualBlock
        ) && !matches!(key, 'j' | 'k')
        {
            self.visual_to_line_end = key == '$';
        }
        // Ordinary motions only need the nearby rows, not a new index of every
        // line in the document for each held navigation key.
        let word_target = match key {
            'w' | 'W' => Some(repeat_motion(
                chars,
                self.cursor,
                count,
                key == 'W',
                word_forward,
            )),
            'b' | 'B' => Some(repeat_motion(
                chars,
                self.cursor,
                count,
                key == 'B',
                word_backward,
            )),
            'e' | 'E' => Some(repeat_motion(
                chars,
                self.cursor,
                count,
                key == 'E',
                word_end,
            )),
            _ => None,
        };
        if let Some(target) = word_target {
            self.cursor = if self.mode == VimMode::Normal && target == chars.len() {
                let line = last_line(chars);
                line.end.saturating_sub(1).max(line.start)
            } else {
                target
            };
            self.preferred_column = None;
            return;
        }
        let line = line_at_cursor(chars, self.cursor);
        if key == 'j' || key == 'k' {
            let col = *self
                .preferred_column
                .get_or_insert(self.cursor.saturating_sub(line.start));
            let target = relative_line(chars, line, count, key == 'j');
            self.cursor = target.start + col.min(target.end.saturating_sub(target.start + 1));
            return;
        }
        self.cursor = match key {
            'h' => self.cursor.saturating_sub(count).max(line.start),
            'l' => self
                .cursor
                .saturating_add(count)
                .min(line.end.saturating_sub(1).max(line.start)),
            '0' => line.start,
            '^' => first_nonblank(chars, line),
            '+' | '-' | '_' => {
                let target =
                    relative_line(chars, line, count - usize::from(key == '_'), key != '-');
                first_nonblank(chars, target)
            }
            '|' => {
                line.start
                    + count
                        .saturating_sub(1)
                        .min(line.end.saturating_sub(line.start + 1))
            }
            '$' => {
                let target = relative_line(chars, line, count - 1, true);
                target.end.saturating_sub(1).max(target.start)
            }
            'G' => {
                let target = if explicit {
                    relative_line(chars, line_at_cursor(chars, 0), count - 1, true)
                } else {
                    last_line(chars)
                };
                // A final line break ends the last line; it starts no other.
                let target = if target.start == chars.len() && target.start > 0 {
                    last_line(chars)
                } else {
                    target
                };
                first_nonblank(chars, target)
            }
            'H' | 'M' | 'L' => {
                let rows = lines(chars);
                first_nonblank(chars, rows[self.screen_row(chars, &rows, key, count)])
            }
            '%' => matching_bracket(chars, self.cursor).unwrap_or(self.cursor),
            '{' | '}' => paragraph(chars, self.cursor, count, key == '}'),
            _ => self.cursor,
        };
        self.preferred_column = if key == '$' { Some(usize::MAX) } else { None };
    }

    /// The line H, M or L selects in the visible range, never the empty row
    /// after a final line break.
    pub(super) fn screen_row(
        &self,
        chars: &[char],
        rows: &[Line],
        key: char,
        count: usize,
    ) -> usize {
        let last = last_line_row(rows);
        let (top, bottom) = match &self.visible {
            Some(range) if range.start < range.end && range.start < chars.len() => {
                let mut top = row_for_cursor(rows, range.start);
                // A wrapped line that starts above the view is not the top line.
                if rows[top].start < range.start && top < last {
                    top += 1;
                }
                let top = top.min(last);
                // The range ends before the newline of its last row, also
                // when that row is empty.
                let bottom = row_for_cursor(rows, range.end.min(chars.len()))
                    .min(last)
                    .max(top);
                (top, bottom)
            }
            _ => (0, last),
        };
        match key {
            'H' => top.saturating_add(count - 1).min(bottom),
            'L' => bottom.saturating_sub(count - 1).max(top),
            _ => top + (bottom - top) / 2,
        }
    }

    pub(super) fn find(
        &mut self,
        text: &mut String,
        find: Find,
        count: usize,
        operator: Option<Operator>,
        repeated: bool,
    ) -> (bool, bool, Option<Yank>) {
        let chars: Vec<char> = text.chars().collect();
        let rows = lines(&chars);
        let cursor_for_match = |at: usize| {
            if !find.till {
                return at;
            }
            let mut target = if find.forward {
                at.saturating_sub(1)
            } else {
                at + 1
            };
            if self.mode == VimMode::Normal {
                // t/T stop on a visible character when the adjacent position
                // is a line ending. Operator ranges still use the exact span.
                if find.forward {
                    let line = rows[row_for_cursor(&rows, target)];
                    target = target.min(line.end.saturating_sub(1).max(line.start));
                } else {
                    if chars.get(target) == Some(&'\r') && chars.get(target + 1) == Some(&'\n') {
                        target += 1;
                    }
                    if chars.get(target) == Some(&'\n') {
                        target += 1;
                    }
                }
            }
            target
        };
        let mut at = self.cursor;
        for step in 0..count {
            let matches = |index: &usize| {
                chars[*index] == find.character
                    && !(repeated
                        && find.till
                        && step == 0
                        && cursor_for_match(*index) == self.cursor)
            };
            let target = if find.forward {
                (at.saturating_add(1)..chars.len()).find(matches)
            } else {
                (0..at.min(chars.len())).rev().find(matches)
            };
            let Some(target) = target else {
                return (true, false, None);
            };
            at = target;
        }
        let target = if find.till {
            if find.forward {
                at.saturating_sub(1)
            } else {
                at + 1
            }
        } else {
            at
        };
        if let Some(op) = operator {
            let range = if find.forward {
                self.cursor..target + 1
            } else {
                target..self.cursor
            };
            if matches!(op, Operator::Indent | Operator::Outdent) {
                return self.apply_lines(
                    text,
                    op,
                    row_for_cursor(&rows, range.start),
                    row_for_cursor(&rows, range.end.saturating_sub(1)),
                );
            }
            let yank_cursor = cursor_for_match(at);
            let result = self.apply_range(text, op, range);
            if op == Operator::Yank && !find.forward {
                self.cursor = yank_cursor;
            }
            return result;
        }
        self.cursor = cursor_for_match(at);
        self.visual_to_line_end = false;
        self.preferred_column = None;
        (true, false, None)
    }
}

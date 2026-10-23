//! Rectangular selection and edits, owned by the Vim state and its history.
//! Columns count graphemes; tabs advance to the configured indentation stop.
use super::*;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Debug)]
pub(super) struct BlockInsert {
    unprepared: String,
    baseline: String,
    cursor: usize,
    /// Scalar insertion boundaries in the baseline, excluding the first row.
    targets: Vec<usize>,
}

#[derive(Clone, Copy)]
struct Geometry {
    first: usize,
    last: usize,
    left: usize,
    right: usize,
    to_end: bool,
}

#[derive(Clone)]
struct Cell<'a> {
    text: &'a str,
    scalars: Range<usize>,
    columns: Range<usize>,
}

/// A source row, its rectangular source span, and replacement-safe fragments.
struct Row {
    range: Range<usize>,
    prefix: String,
    selected: String,
    suffix: String,
    width: usize,
}

fn cells(text: &str, line: Line, tab_width: usize) -> Vec<Cell<'_>> {
    let body = &text[char_to_byte(text, line.start)..char_to_byte(text, line.end)];
    let mut at = line.start;
    let mut column = 0;
    body.graphemes(true)
        .map(|grapheme| {
            let length = grapheme.chars().count();
            let width = if grapheme == "\t" {
                tab_width - column % tab_width
            } else {
                1
            };
            let cell = Cell {
                text: grapheme,
                scalars: at..at + length,
                columns: column..column + width,
            };
            at += length;
            column += width;
            cell
        })
        .collect()
}

fn column_at(text: &str, line: Line, cursor: usize, tab_width: usize) -> usize {
    let cells = cells(text, line, tab_width);
    cells
        .iter()
        .find(|cell| cursor < cell.scalars.end)
        .map_or_else(
            || cells.last().map_or(0, |cell| cell.columns.end),
            |cell| cell.columns.start,
        )
}

fn position_at(text: &str, line: Line, column: usize, tab_width: usize) -> usize {
    cells(text, line, tab_width)
        .iter()
        .find(|cell| column < cell.columns.end)
        .map_or(line.end, |cell| cell.scalars.start)
}

fn row(text: &str, line: Line, left: usize, right: usize, tab_width: usize) -> Row {
    let mut prefix = String::new();
    let mut selected = String::new();
    let mut suffix = String::new();
    let mut start = line.end;
    let mut end = line.end;
    let mut width = 0;
    for cell in cells(text, line, tab_width) {
        width = cell.columns.end;
        if cell.columns.end <= left {
            prefix.push_str(cell.text);
        } else if cell.columns.start >= right {
            suffix.push_str(cell.text);
        } else {
            start = start.min(cell.scalars.start);
            end = if start == cell.scalars.start {
                cell.scalars.end
            } else {
                end.max(cell.scalars.end)
            };
            if cell.text == "\t" {
                prefix.push_str(&" ".repeat(left.saturating_sub(cell.columns.start)));
                selected.push_str(
                    &" ".repeat(right.min(cell.columns.end) - left.max(cell.columns.start)),
                );
                suffix.push_str(&" ".repeat(cell.columns.end.saturating_sub(right)));
            } else {
                selected.push_str(cell.text);
            }
        }
    }
    Row {
        range: start..end,
        prefix,
        selected,
        suffix,
        width,
    }
}

/// Split a row at a text column, expanding a tab only when the boundary is
/// inside it. Padding is opt-in because Vim's block I skips short rows.
fn insertion_row(
    text: &str,
    line: Line,
    column: usize,
    tab_width: usize,
    pad: bool,
) -> Option<(String, String)> {
    let mut prefix = String::new();
    let mut suffix = String::new();
    let mut width = 0;
    for cell in cells(text, line, tab_width) {
        width = cell.columns.end;
        if cell.columns.end <= column {
            prefix.push_str(cell.text);
        } else if cell.columns.start >= column {
            suffix.push_str(cell.text);
        } else {
            prefix.push_str(&" ".repeat(column - cell.columns.start));
            suffix.push_str(&" ".repeat(cell.columns.end - column));
        }
    }
    if width < column {
        if !pad {
            return None;
        }
        prefix.push_str(&" ".repeat(column - width));
    }
    Some((prefix, suffix))
}

impl VimState {
    fn block_geometry(&self, text: &str) -> Option<Geometry> {
        if self.mode != VimMode::VisualBlock {
            return None;
        }
        let chars: Vec<_> = text.chars().collect();
        let rows = lines(&chars);
        let anchor = self.visual_anchor.unwrap_or(self.cursor).min(chars.len());
        let cursor = self.cursor.min(chars.len());
        let anchor_row = row_for_cursor(&rows, anchor);
        let cursor_row = row_for_cursor(&rows, cursor);
        let (a, b) = self.block_columns.unwrap_or_else(|| {
            (
                column_at(text, rows[anchor_row], anchor, self.indent_width),
                column_at(text, rows[cursor_row], cursor, self.indent_width),
            )
        });
        Some(Geometry {
            first: anchor_row.min(cursor_row),
            last: anchor_row.max(cursor_row),
            left: if self.visual_to_line_end { a } else { a.min(b) },
            right: a.max(b).saturating_add(1),
            to_end: self.visual_to_line_end,
        })
    }

    /// Disjoint scalar ranges, one per logical row, excluding line breaks.
    /// Short rows produce an empty range. A selected tab is included as a whole
    /// source grapheme; block editing expands only its selected columns.
    pub fn block_selection(&self, text: &str) -> Option<Vec<Range<usize>>> {
        let geometry = self.block_geometry(text)?;
        let chars: Vec<_> = text.chars().collect();
        let rows = lines(&chars);
        Some(
            rows[geometry.first..=geometry.last]
                .iter()
                .map(|line| {
                    row(
                        text,
                        *line,
                        geometry.left,
                        if geometry.to_end {
                            usize::MAX
                        } else {
                            geometry.right
                        },
                        self.indent_width,
                    )
                    .range
                })
                .collect(),
        )
    }

    /// Virtual column bounds matching each row from `block_selection`. Drawing
    /// can clip partial tabs and extend the highlight beyond a short line.
    pub fn block_selection_columns(&self, text: &str) -> Option<Vec<Range<usize>>> {
        let geometry = self.block_geometry(text)?;
        let rows = lines(&text.chars().collect::<Vec<_>>());
        Some(
            rows[geometry.first..=geometry.last]
                .iter()
                .map(|line| {
                    let right = if geometry.to_end {
                        column_at(text, *line, line.end, self.indent_width).max(geometry.left + 1)
                    } else {
                        geometry.right
                    };
                    geometry.left..right
                })
                .collect(),
        )
    }

    /// Rectangular clipboard/register payload with tabs expanded and short
    /// rows padded to the recorded width. It never contains source line breaks.
    /// Returns None outside Visual Block or above the 16 MiB payload limit.
    pub fn block_yank(&self, text: &str) -> Option<Yank> {
        let geometry = self.block_geometry(text)?;
        if !geometry.to_end
            && (geometry.right - geometry.left).saturating_mul(geometry.last - geometry.first + 1)
                > MAX_PASTE_BYTES
        {
            return None;
        }
        let chars: Vec<_> = text.chars().collect();
        let rows = lines(&chars);
        let selected: Vec<_> = rows[geometry.first..=geometry.last]
            .iter()
            .map(|line| {
                row(
                    text,
                    *line,
                    geometry.left,
                    if geometry.to_end {
                        usize::MAX
                    } else {
                        geometry.right
                    },
                    self.indent_width,
                )
            })
            .collect();
        let width = if geometry.to_end {
            selected
                .iter()
                .map(|row| row.width.saturating_sub(geometry.left))
                .max()
                .unwrap_or(1)
                .max(1)
        } else {
            geometry.right - geometry.left
        };
        let mut payload = String::new();
        for (index, row) in selected.into_iter().enumerate() {
            if index > 0 {
                payload.push('\n');
            }
            let actual = row.selected.graphemes(true).count();
            if payload
                .len()
                .saturating_add(row.selected.len())
                .saturating_add(width.saturating_sub(actual))
                > MAX_PASTE_BYTES
            {
                return None;
            }
            payload.push_str(&row.selected);
            payload.push_str(&" ".repeat(width.saturating_sub(actual)));
        }
        Some(Yank {
            text: payload,
            linewise: false,
            block_width: Some(width),
        })
    }

    pub(super) fn visual_change_geometry(&self, text: &str) -> Option<VisualRepeat> {
        if let Some(geometry) = self.block_geometry(text) {
            return Some(VisualRepeat {
                linewise: false,
                blockwise: true,
                rows: geometry.last - geometry.first + 1,
                columns: geometry.right - geometry.left,
                includes_newline: false,
                to_line_end: geometry.to_end,
            });
        }
        self.selection(text)
            .map(|range| self.visual_repeat(text, &range))
    }

    pub(super) fn select_repeat_block(&mut self, text: &str, visual: &VisualRepeat) {
        let chars: Vec<_> = text.chars().collect();
        let rows = lines(&chars);
        let first = row_for_cursor(&rows, self.cursor);
        let last = first
            .saturating_add(visual.rows - 1)
            .min(last_line_row(&rows));
        let column = column_at(text, rows[first], self.cursor, self.indent_width);
        self.cancel_pending_command();
        self.mode = VimMode::VisualBlock;
        self.visual_anchor = Some(self.cursor);
        self.visual_to_line_end = visual.to_line_end;
        let right = column.saturating_add(visual.columns.saturating_sub(1));
        self.block_columns = Some((column, right));
        self.cursor = position_at(text, rows[last], right, self.indent_width);
    }

    pub(super) fn toggle_visual_block(&mut self, text: &str) {
        if self.mode == VimMode::VisualBlock {
            self.leave_visual();
        } else {
            self.multi.clear();
            self.visual_anchor.get_or_insert(self.cursor);
            let chars: Vec<_> = text.chars().collect();
            let anchor = self.visual_anchor.unwrap_or(self.cursor);
            self.block_columns = Some((
                column_at(
                    text,
                    line_at_cursor(&chars, anchor),
                    anchor,
                    self.indent_width,
                ),
                column_at(
                    text,
                    line_at_cursor(&chars, self.cursor),
                    self.cursor,
                    self.indent_width,
                ),
            ));
            self.mode = VimMode::VisualBlock;
            self.visual_to_line_end = false;
            self.sync_block_cursor(text, usize::MAX);
        }
        self.clear_command();
    }

    /// Non-rectangular motions (word/search/find/gg) update the head's column.
    /// Dedicated grid motions have already moved the virtual column themselves.
    pub(super) fn sync_block_cursor(&mut self, text: &str, old_cursor: usize) {
        let chars: Vec<_> = text.chars().collect();
        let line = line_at_cursor(&chars, self.cursor);
        let actual = column_at(text, line, self.cursor, self.indent_width);
        if self.block_columns.is_none() {
            let anchor = self.visual_anchor.unwrap_or(self.cursor);
            self.block_columns = Some((
                column_at(
                    text,
                    line_at_cursor(&chars, anchor),
                    anchor,
                    self.indent_width,
                ),
                actual,
            ));
        }
        if let Some((_, head)) = self.block_columns.as_mut() {
            let target = position_at(text, line, *head, self.indent_width);
            if self.cursor != old_cursor && target != self.cursor {
                *head = actual;
            }
            self.cursor = position_at(text, line, *head, self.indent_width);
        }
    }

    pub(super) fn snap_block_position(&mut self, text: &str) {
        let chars: Vec<_> = text.chars().collect();
        let line = line_at_cursor(&chars, self.cursor);
        let column = column_at(text, line, self.cursor, self.indent_width);
        self.cursor = position_at(text, line, column, self.indent_width);
    }

    pub(super) fn move_block_cursor(&mut self, chars: &[char], key: char, count: usize) -> bool {
        if !matches!(key, 'h' | 'j' | 'k' | 'l' | '0' | '|' | '$') {
            return false;
        }
        let text: String = chars.iter().collect();
        self.sync_block_cursor(&text, self.cursor);
        let rows = lines(chars);
        let current = row_for_cursor(&rows, self.cursor);
        let (anchor, mut column) = self.block_columns.unwrap_or((0, 0));
        let mut target = current;
        match key {
            'j' => {
                target = current
                    .saturating_add(count)
                    .min(last_line_row(&rows).max(current))
            }
            'k' => target = current.saturating_sub(count),
            'h' => column = column.saturating_sub(count),
            'l' => column = column.saturating_add(count).min(MAX_PASTE_BYTES),
            '0' => column = 0,
            '|' => column = count.saturating_sub(1),
            '$' => {
                target = current.saturating_add(count - 1).min(last_line_row(&rows));
                column = cells(&text, rows[target], self.indent_width)
                    .last()
                    .map_or(0, |cell| cell.columns.start);
            }
            _ => unreachable!(),
        }
        if self.visual_to_line_end && matches!(key, 'j' | 'k') {
            column = cells(&text, rows[target], self.indent_width)
                .last()
                .map_or(0, |cell| cell.columns.start);
        }
        if !matches!(key, 'j' | 'k') {
            self.visual_to_line_end = key == '$';
        }
        self.block_columns = Some((anchor, column));
        self.cursor = position_at(&text, rows[target], column, self.indent_width);
        true
    }

    pub(super) fn cut_block(&mut self, text: &mut String) -> Action {
        let old_cursor = self.cursor;
        let visual = self.visual_change_geometry(text);
        self.cancel_pending_command();
        let (consumed, text_changed, yanked) = self.apply_block(text, 'd', 1);
        if text_changed && !self.replaying {
            self.last_change = Some(RepeatChange {
                keys: vec![VimKey::Char('d')],
                insert: None,
                search: None,
                visual,
            });
        }
        self.clamp_cursor(text);
        self.motion_epoch = self.motion_epoch.wrapping_add(1);
        Action {
            consumed,
            text_changed,
            cursor_changed: self.cursor != old_cursor,
            mode_changed: self.mode != VimMode::VisualBlock,
            yanked,
            yanked_ranges: std::mem::take(&mut self.yanked_ranges),
            message: self.message.take(),
            ..Action::default()
        }
    }

    pub(super) fn apply_block(
        &mut self,
        text: &mut String,
        key: char,
        count: usize,
    ) -> (bool, bool, Option<Yank>) {
        let Some(geometry) = self.block_geometry(text) else {
            return (true, false, None);
        };
        let chars: Vec<_> = text.chars().collect();
        let rows = lines(&chars);
        if matches!(key, '>' | '<') {
            self.leave_visual();
            return (
                true,
                self.shift_lines(text, geometry.first, geometry.last, key == '>', count),
                None,
            );
        }
        if matches!(key, 'I' | 'A') {
            return (
                true,
                self.start_block_insert(text, geometry, key == 'A', None),
                None,
            );
        }
        let copying = matches!(key, 'y' | 'd' | 'x' | 'c' | 's');
        let yank = copying.then(|| self.block_yank(text)).flatten();
        if copying && yank.is_none() {
            self.message = Some("Block selection exceeds the 16 MiB clipboard limit".into());
            return (true, false, None);
        }
        if let Some(yank) = &yank {
            self.register = Some(yank.clone());
            if key == 'y' {
                self.yank_register = Some(yank.clone());
            }
            self.yanked_ranges = self.block_selection(text).unwrap_or_default();
        }
        if key == 'y' {
            let cursor = position_at(text, rows[geometry.first], geometry.left, self.indent_width);
            self.leave_visual();
            self.cursor = cursor;
            return (true, false, yank);
        }
        let before = self.snapshot(text);
        let mut replacements = Vec::new();
        for line in &rows[geometry.first..=geometry.last] {
            let selected = row(
                text,
                *line,
                geometry.left,
                if geometry.to_end {
                    usize::MAX
                } else {
                    geometry.right
                },
                self.indent_width,
            );
            let replacement = match key {
                'u' => selected.selected.to_lowercase(),
                'U' => selected.selected.to_uppercase(),
                '~' => selected
                    .selected
                    .chars()
                    .flat_map(|ch| {
                        if ch.is_lowercase() {
                            ch.to_uppercase().collect::<Vec<_>>()
                        } else {
                            ch.to_lowercase().collect::<Vec<_>>()
                        }
                    })
                    .collect(),
                _ => String::new(),
            };
            replacements.push((
                line.start..line.end,
                format!("{}{}{}", selected.prefix, replacement, selected.suffix),
            ));
        }
        apply_replacements(text, &replacements);
        let changed = *text != before.text;
        if matches!(key, 'c' | 's') {
            self.start_block_insert(text, geometry, false, Some(before));
        } else {
            let rows = lines(&text.chars().collect::<Vec<_>>());
            let cursor = position_at(text, rows[geometry.first], geometry.left, self.indent_width);
            self.leave_visual();
            self.cursor = cursor;
            if changed {
                self.record(before);
            }
        }
        (true, changed, yank)
    }

    fn start_block_insert(
        &mut self,
        text: &mut String,
        geometry: Geometry,
        append: bool,
        origin: Option<Snapshot>,
    ) -> bool {
        let before = origin.unwrap_or_else(|| self.snapshot(text));
        let original = text.clone();
        let rows = lines(&text.chars().collect::<Vec<_>>());
        let mut positions = Vec::new();
        let mut replacements = Vec::new();
        let mut delta = 0isize;
        for (index, line) in rows[geometry.first..=geometry.last].iter().enumerate() {
            let column = if append && geometry.to_end {
                column_at(text, *line, line.end, self.indent_width)
            } else if append {
                geometry.right
            } else {
                geometry.left
            };
            let Some((prefix, suffix)) =
                insertion_row(text, *line, column, self.indent_width, append)
            else {
                continue;
            };
            let content = format!("{prefix}{suffix}");
            positions.push((
                geometry.first + index,
                line.start.saturating_add_signed(delta) + prefix.chars().count(),
            ));
            delta += content.chars().count() as isize - (line.end - line.start) as isize;
            replacements.push((line.start..line.end, content));
        }
        if positions.is_empty() {
            self.leave_visual();
            return false;
        }
        apply_replacements(text, &replacements);
        self.leave_visual();
        let first = positions.first().map_or(self.cursor, |(_, at)| *at);
        self.start_insert(text, first);
        self.insert_origin = Some(before);
        self.block_insert = Some(BlockInsert {
            unprepared: original.clone(),
            baseline: text.clone(),
            cursor: first,
            targets: positions.into_iter().skip(1).map(|(_, at)| at).collect(),
        });
        *text != original
    }

    pub(super) fn finish_block_insert(&mut self, text: &mut String) -> bool {
        let Some(insert) = self.block_insert.take() else {
            return false;
        };
        let delta = insert_delta(&insert.baseline, text, insert.cursor);
        if delta.inserted.is_empty() && delta.removed == 0 {
            let changed = *text != insert.unprepared;
            *text = insert.unprepared;
            self.cursor = self.cursor.min(text.chars().count());
            return changed;
        }
        // Cursor motion, deleting existing text and multiline insertion end the
        // first-row edit, as Vim does; they must not damage unrelated rows.
        if delta.relative_start != 0 || delta.removed != 0 || delta.inserted.contains(['\r', '\n'])
        {
            return false;
        }
        let added = delta.inserted.chars().count();
        if self.cursor != insert.cursor + added
            || added == 0
            || text
                .len()
                .saturating_add(delta.inserted.len().saturating_mul(insert.targets.len()))
                > MAX_PASTE_BYTES
        {
            return false;
        }
        for at in insert.targets.into_iter().rev() {
            text.insert_str(char_to_byte(text, at + added), &delta.inserted);
        }
        true
    }

    pub(super) fn replace_block_characters(
        &mut self,
        text: &mut String,
        replacement: char,
    ) -> bool {
        let Some(geometry) = self.block_geometry(text) else {
            return false;
        };
        if matches!(replacement, '\r' | '\n') {
            self.message = Some("A block replacement must stay on its source rows".into());
            return false;
        }
        let before = self.snapshot(text);
        let rows = lines(&text.chars().collect::<Vec<_>>());
        let replacements: Vec<_> = rows[geometry.first..=geometry.last]
            .iter()
            .map(|line| {
                let selected = row(
                    text,
                    *line,
                    geometry.left,
                    if geometry.to_end {
                        usize::MAX
                    } else {
                        geometry.right
                    },
                    self.indent_width,
                );
                let count = selected.selected.graphemes(true).count();
                (
                    line.start..line.end,
                    format!(
                        "{}{}{}",
                        selected.prefix,
                        replacement.to_string().repeat(count),
                        selected.suffix
                    ),
                )
            })
            .collect();
        apply_replacements(text, &replacements);
        let changed = *text != before.text;
        self.leave_visual();
        let rows = lines(&text.chars().collect::<Vec<_>>());
        self.cursor = position_at(text, rows[geometry.first], geometry.left, self.indent_width);
        if changed {
            self.record(before);
        }
        changed
    }

    pub(super) fn paste_block(
        &mut self,
        text: &mut String,
        before_cursor: bool,
        count: usize,
    ) -> bool {
        let Some(register) = self.register.clone() else {
            return false;
        };
        let rows = lines(&text.chars().collect::<Vec<_>>());
        let first = row_for_cursor(&rows, self.cursor);
        let line = rows[first];
        let mut column = column_at(text, line, self.cursor, self.indent_width);
        if !before_cursor && self.cursor < line.end {
            column = cells(text, line, self.indent_width)
                .iter()
                .find(|cell| cell.scalars.contains(&self.cursor))
                .map_or(column + 1, |cell| cell.columns.end);
        }
        let payload: Vec<_> = register
            .text
            .split('\n')
            .map(|part| part.trim_end_matches('\r'))
            .collect();
        let required = payload.iter().fold(0usize, |total, part| {
            let padding = register
                .block_width
                .unwrap_or(0)
                .saturating_sub(part.graphemes(true).count());
            total.saturating_add(part.len().saturating_add(padding).saturating_mul(count))
        });
        if required
            .saturating_add(column.saturating_mul(payload.len()))
            .saturating_add(text.len())
            > MAX_PASTE_BYTES
        {
            return false;
        }
        let before = self.snapshot(text);
        let ending = line_ending(text).to_owned();
        let existing = rows.len() - usize::from(text.ends_with('\n'));
        let missing = (first + payload.len()).saturating_sub(existing);
        text.push_str(&ending.repeat(missing));
        let rows = lines(&text.chars().collect::<Vec<_>>());
        let mut replacements = Vec::new();
        for (index, part) in payload.iter().enumerate() {
            let line = rows[first + index];
            let (prefix, suffix) =
                insertion_row(text, line, column, self.indent_width, true).unwrap();
            let unit_width = part.graphemes(true).count();
            let padded = format!(
                "{}{}",
                part,
                " ".repeat(
                    register
                        .block_width
                        .unwrap_or(unit_width)
                        .saturating_sub(unit_width)
                )
            );
            replacements.push((
                line.start..line.end,
                format!("{}{}{}", prefix, padded.repeat(count), suffix),
            ));
        }
        apply_replacements(text, &replacements);
        self.cursor = position_at(
            text,
            lines(&text.chars().collect::<Vec<_>>())[first],
            column,
            self.indent_width,
        );
        let changed = *text != before.text;
        if changed {
            self.record(before);
        }
        changed
    }

    pub(super) fn paste_visual_block(
        &mut self,
        text: &mut String,
        count: usize,
        preserve_register: bool,
    ) -> (bool, bool, Option<Yank>) {
        let Some(geometry) = self.block_geometry(text) else {
            return (true, false, None);
        };
        let Some(register) = self.register.clone() else {
            return (true, false, None);
        };
        let deleted = self.block_yank(text);
        if !preserve_register && deleted.is_none() {
            self.message = Some("Block selection exceeds the 16 MiB clipboard limit".into());
            return (true, false, None);
        }
        let ranges = self.block_selection(text).unwrap_or_default();
        let before = self.snapshot(text);
        let rows = lines(&text.chars().collect::<Vec<_>>());
        let payload = if register.linewise {
            register
                .text
                .strip_suffix("\r\n")
                .or_else(|| register.text.strip_suffix('\n'))
                .unwrap_or(&register.text)
        } else {
            &register.text
        };
        let parts: Vec<_> = payload
            .split('\n')
            .map(|part| part.trim_end_matches('\r'))
            .collect();
        let row_count = (geometry.last - geometry.first + 1).max(parts.len());
        if text
            .len()
            .saturating_add(register.text.len().saturating_mul(count))
            .saturating_add(row_count.saturating_mul(geometry.left))
            > MAX_PASTE_BYTES
        {
            return (true, false, None);
        }
        let mut candidate = text.clone();
        let existing = rows.len() - usize::from(text.ends_with('\n'));
        let missing = (geometry.first + row_count).saturating_sub(existing);
        candidate.push_str(&line_ending(text).repeat(missing));
        let rows = lines(&candidate.chars().collect::<Vec<_>>());
        let mut replacements = Vec::new();
        let mut replacement_bytes = 0usize;
        for (index, line) in rows[geometry.first..geometry.first + row_count]
            .iter()
            .enumerate()
        {
            let right = if geometry.first + index > geometry.last {
                geometry.left
            } else if geometry.to_end {
                usize::MAX
            } else {
                geometry.right
            };
            let selected = if right == geometry.left {
                let (prefix, suffix) =
                    insertion_row(&candidate, *line, geometry.left, self.indent_width, true)
                        .unwrap();
                Row {
                    range: line.start..line.start,
                    prefix,
                    selected: String::new(),
                    suffix,
                    width: geometry.left,
                }
            } else {
                row(&candidate, *line, geometry.left, right, self.indent_width)
            };
            let part = if parts.len() == 1 {
                parts[0]
            } else {
                parts.get(index).copied().unwrap_or("")
            };
            let Some(pasted) = repeated_text(part, count) else {
                return (true, false, None);
            };
            let padding = " ".repeat(geometry.left.saturating_sub(selected.width));
            let replacement = format!(
                "{}{}{}{}",
                selected.prefix, padding, pasted, selected.suffix
            );
            replacement_bytes = replacement_bytes.saturating_add(replacement.len());
            if replacement_bytes.saturating_add(text.len()) > MAX_PASTE_BYTES {
                return (true, false, None);
            }
            replacements.push((line.start..line.end, replacement));
        }
        apply_replacements(&mut candidate, &replacements);
        *text = candidate;
        self.leave_visual();
        self.cursor = position_at(
            text,
            lines(&text.chars().collect::<Vec<_>>())[geometry.first],
            geometry.left,
            self.indent_width,
        );
        let changed = *text != before.text;
        if changed {
            self.record(before);
        }
        if preserve_register {
            return (true, changed, None);
        }
        self.register = deleted.clone();
        self.yanked_ranges = ranges;
        (true, changed, deleted)
    }

    /// Native paste replaces the rectangle but preserves Vim's registers.
    pub fn paste_block_clipboard(&mut self, text: &mut String, clipboard: &str) -> Action {
        if self.mode != VimMode::VisualBlock || clipboard.is_empty() {
            return Action::default();
        }
        let old_cursor = self.cursor;
        let register = self.register.take();
        self.register = Some(Yank {
            text: clipboard.to_owned(),
            linewise: false,
            block_width: None,
        });
        self.cancel_pending_command();
        let (consumed, text_changed, _) = self.paste_visual_block(text, 1, true);
        self.register = register;
        Action {
            consumed,
            text_changed,
            cursor_changed: old_cursor != self.cursor,
            mode_changed: self.mode != VimMode::VisualBlock,
            ..Action::default()
        }
    }
}

fn apply_replacements(text: &mut String, replacements: &[(Range<usize>, String)]) {
    // All source coordinates refer to the same snapshot; replacing bottom-up
    // keeps them valid when Unicode case mapping expands a grapheme.
    let mut remaining = text.chars();
    let mut scalar = 0;
    let mut byte = 0;
    let mut boundary = |target| {
        for _ in scalar..target {
            byte += remaining.next().expect("valid source range").len_utf8();
        }
        scalar = target;
        byte
    };
    let bytes: Vec<_> = replacements
        .iter()
        .map(|(range, replacement)| (boundary(range.start)..boundary(range.end), replacement))
        .collect();
    for (range, replacement) in bytes.into_iter().rev() {
        text.replace_range(range, replacement);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(state: &mut VimState, text: &mut String, keys: &str) {
        for key in keys.chars() {
            state.handle_key(text, VimKey::Char(key));
        }
    }

    fn block(state: &mut VimState, text: &mut String, cursor: usize, motions: &str) {
        state.set_cursor(text, cursor);
        state.handle_key(text, VimKey::VisualBlock);
        keys(state, text, motions);
    }

    #[test]
    fn rectangles_preserve_graphemes_short_rows_crlf_and_pane_navigation() {
        let mut text = "αe\u{301}x\r\n👩\u{200d}💻中z\r\nq\r\n".to_owned();
        let original = text.clone();
        let mut state = VimState::new();
        block(&mut state, &mut text, 1, "jlj");
        let ranges = vec![1..4, 9..11, 14..14];
        assert_eq!(state.block_selection(&text), Some(ranges.clone()));
        assert_eq!(state.selection(&text), None);
        assert!(!state.multi.active());
        assert_eq!(state.block_yank(&text).unwrap().text, "e\u{301}x\n中z\n  ");
        let navigation = state.navigation();
        state.handle_key(&mut text, VimKey::Escape);
        state.set_cursor(&text, 0);
        state.restore_navigation(&text, &navigation);
        keys(&mut state, &mut text, "o");
        assert_eq!(state.block_selection(&text), Some(ranges));
        let action = state.handle_key(&mut text, VimKey::Char('d'));
        assert!(action.text_changed);
        assert_eq!(action.yanked_ranges, vec![1..4, 9..11, 14..14]);
        assert_eq!(text, "α\r\n👩\u{200d}💻\r\nq\r\n");
        keys(&mut state, &mut text, "u");
        assert_eq!(text, original);
        state.handle_key(&mut text, VimKey::Redo);
        assert_eq!(text, "α\r\n👩\u{200d}💻\r\nq\r\n");
        state.set_cursor(&text, 0);
        keys(&mut state, &mut text, ".");
        assert_eq!(text, "\r\n\r\n\r\n");
    }

    #[test]
    fn partial_tabs_keep_unselected_columns_and_clipboard_cuts_keep_registers() {
        let original = "\tA\r\n12\tB\r\nxy\r\n";
        let mut text = original.to_owned();
        let mut state = VimState::new();
        let saved = Yank {
            text: "keep".into(),
            linewise: false,
            block_width: None,
        };
        state.set_register(saved.clone());
        block(&mut state, &mut text, 5, "kl");
        assert_eq!(state.block_yank(&text).unwrap().text, "  \n2 ");
        let (action, copied) = state.cut_visual_to_clipboard(&mut text);
        assert!(action.text_changed);
        assert_eq!(copied.as_deref(), Some("  \n2 "));
        assert_eq!(text, "  A\r\n1 B\r\nxy\r\n");
        assert_eq!(state.register(), Some(&saved));
        keys(&mut state, &mut text, "u");
        assert_eq!(text, original);
    }

    #[test]
    fn block_insert_append_and_change_repeat_as_single_transactions() {
        for (command, expected) in [
            ('I', "a!bcd\na!bcd\nabcd"),
            ('A', "abc!d\nabc!d\nabcd"),
            ('c', "a!d\na!d\nabcd"),
        ] {
            let mut text = "abcd\nabcd\nabcd".to_owned();
            let original = text.clone();
            let mut state = VimState::new();
            block(&mut state, &mut text, 1, "jl");
            state.handle_key(&mut text, VimKey::Char(command));
            assert_eq!(state.mode, VimMode::Insert);
            assert!(!state.multi.active());
            // TextEdit edits the first row; finish_insert repeats that delta.
            state.insert_text(&mut text, "!");
            state.finish_insert(&mut text);
            assert_eq!(text, expected, "{command}");
            state.history_edit(&mut text, false);
            assert_eq!(text, original, "{command}");
            state.history_edit(&mut text, true);
            assert_eq!(text, expected, "{command}");
            state.set_cursor(&text, text.rfind('\n').unwrap() + 2);
            keys(&mut state, &mut text, ".");
            assert!(
                text.ends_with(match command {
                    'I' => "a!bcd",
                    'A' => "abc!d",
                    _ => "a!d",
                }),
                "{command}: {text:?}"
            );
            keys(&mut state, &mut text, "u");
            assert_eq!(text, expected, "{command}");
        }
    }

    #[test]
    fn short_rows_and_canceled_block_insertion_keep_existing_content() {
        for (command, expected) in [('I', "abc!d\nx\nabc!d"), ('A', "abcd!\nx   !\nabcd!")] {
            let mut text = "abcd\nx\nabcd".to_owned();
            let mut state = VimState::new();
            block(&mut state, &mut text, 3, "jj");
            state.handle_key(&mut text, VimKey::Char(command));
            state.insert_text(&mut text, "!");
            state.finish_insert(&mut text);
            assert_eq!(text, expected);
        }
        for command in ['I', 'A'] {
            let original = "12\tB\n\tA\nx";
            let mut text = original.to_owned();
            let mut state = VimState::new();
            block(&mut state, &mut text, 1, "jj");
            state.handle_key(&mut text, VimKey::Char(command));
            state.finish_insert(&mut text);
            assert_eq!(text, original, "{command}");
            assert!(!state.can_undo());
        }
    }

    #[test]
    fn rectangular_case_replace_and_indent_leave_other_columns_intact() {
        for (command, replacement, expected) in [
            ("U", None, "aSSCd\neFGh"),
            ("~", None, "aSScd\neFgh"),
            ("r", Some('🦀'), "a🦀🦀d\ne🦀🦀h"),
            (">", None, "    aßCd\n    efGh"),
        ] {
            let mut text = "aßCd\nefGh".to_owned();
            let original = text.clone();
            let mut state = VimState::new();
            block(&mut state, &mut text, 1, "jl");
            keys(&mut state, &mut text, command);
            if let Some(replacement) = replacement {
                state.handle_key(&mut text, VimKey::Char(replacement));
            }
            assert_eq!(text, expected, "{command}");
            keys(&mut state, &mut text, "u");
            assert_eq!(text, original);
        }
    }

    #[test]
    fn block_register_paste_keeps_shape_counts_and_line_endings() {
        for ending in ["\n", "\r\n"] {
            let mut text = format!("abcd{ending}efgh");
            let mut state = VimState::new();
            block(&mut state, &mut text, 1, "jl");
            keys(&mut state, &mut text, "y");
            let yank = state.register().unwrap();
            assert_eq!(yank.text, "bc\nfg");
            assert_eq!(yank.block_width, Some(2));
            text = format!("x{ending}y");
            state.set_cursor(&text, 0);
            keys(&mut state, &mut text, "2p");
            assert_eq!(text, format!("xbcbc{ending}yfgfg"));
            keys(&mut state, &mut text, "u");
            assert_eq!(text, format!("x{ending}y"));
            state.set_cursor(&text, text.chars().count() - 1);
            keys(&mut state, &mut text, "p");
            assert_eq!(text, format!("x{ending}ybc{ending} fg"));
            text = format!("y{ending}");
            state.set_cursor(&text, 0);
            keys(&mut state, &mut text, "p");
            assert_eq!(text, format!("ybc{ending} fg{ending}"));
        }
    }

    #[test]
    fn native_rectangular_paste_preserves_register_and_all_payload_rows() {
        let mut text = "abcd\r\nefgh".to_owned();
        let mut state = VimState::new();
        let saved = Yank {
            text: "saved".into(),
            linewise: true,
            block_width: None,
        };
        state.set_register(saved.clone());
        block(&mut state, &mut text, 1, "jl");
        let action = state.paste_block_clipboard(&mut text, "A\nB\nC");
        assert!(action.text_changed && action.mode_changed);
        assert_eq!(text, "aAd\r\neBh\r\n C");
        assert_eq!(state.register(), Some(&saved));
        keys(&mut state, &mut text, "u");
        assert_eq!(text, "abcd\r\nefgh");
    }

    #[test]
    fn oversized_virtual_blocks_reject_yank_and_cut_without_losing_text() {
        let mut text = "x\n".repeat(2_000);
        let original = text.clone();
        let mut state = VimState::new();
        let saved = Yank {
            text: "keep".into(),
            linewise: false,
            block_width: None,
        };
        state.set_register(saved.clone());
        block(&mut state, &mut text, 0, "1999j9999l");
        assert!(state.block_yank(&text).is_none());
        let action = state.cut_visual(&mut text);
        assert!(!action.text_changed);
        assert!(!action.mode_changed);
        assert!(action.message.is_some());
        assert_eq!(text, original);
        assert_eq!(state.register(), Some(&saved));
        assert_eq!(state.mode, VimMode::VisualBlock);
        assert!(!state.can_undo());
    }

    #[test]
    fn block_line_end_intent_reselection_and_mode_switches_keep_geometry() {
        let mut text = "abcd\nx\nabcdef".to_owned();
        let mut state = VimState::new();
        block(&mut state, &mut text, 1, "$jj");
        assert_eq!(state.block_selection(&text), Some(vec![1..4, 6..6, 8..13]));
        state.handle_key(&mut text, VimKey::Escape);
        keys(&mut state, &mut text, "gv");
        assert_eq!(state.mode, VimMode::VisualBlock);
        assert_eq!(state.block_selection(&text), Some(vec![1..4, 6..6, 8..13]));
        keys(&mut state, &mut text, "v");
        assert_eq!(state.mode, VimMode::Visual);
        state.handle_key(&mut text, VimKey::VisualBlock);
        assert_eq!(state.mode, VimMode::VisualBlock);
        keys(&mut state, &mut text, "V");
        assert_eq!(state.mode, VimMode::VisualLine);
    }
}

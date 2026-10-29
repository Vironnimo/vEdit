//! Multiple directed selections, in Unicode scalar coordinates.
//!
//! A command computes all replacements against one original document and commits
//! them together. Sorting and merging happens before editing, never halfway through.
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub anchor: usize,
    pub head: usize,
    pub column: Option<usize>,
    pub visual_x: Option<i32>,
    pub prefer_next_row: bool,
}

impl Selection {
    pub fn new(anchor: usize, head: usize) -> Self {
        Self {
            anchor,
            head,
            column: None,
            visual_x: None,
            prefer_next_row: true,
        }
    }
    pub fn range(self) -> Range<usize> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    /// Empty means ordinary single-cursor editing. The last entry is primary.
    pub selections: Vec<Selection>,
    /// During placement the last selection moves independently of marked ones.
    placing: bool,
    history: Vec<(Vec<Selection>, bool)>,
}

impl State {
    pub fn active(&self) -> bool {
        !self.selections.is_empty()
    }
    pub fn primary(&self) -> Option<Selection> {
        self.selections.last().copied()
    }
    pub fn placing(&self) -> bool {
        self.placing
    }
    pub fn moving_selections(&mut self) -> &mut [Selection] {
        let start = if self.placing {
            self.selections.len().saturating_sub(1)
        } else {
            0
        };
        &mut self.selections[start..]
    }
    pub fn mark_cursor(&mut self, limit: usize) -> bool {
        let Some(primary) = self.primary() else {
            return true;
        };
        let mut next = self.clone();
        if !next.finish_placement(limit) {
            return false;
        }
        self.checkpoint();
        self.selections = next.selections;
        self.selections.push(primary);
        self.placing = true;
        true
    }
    pub fn finish_placement(&mut self, limit: usize) -> bool {
        if !self.placing {
            return true;
        }
        let mut next = self.clone();
        next.placing = false;
        next.normalize(usize::MAX);
        if next.selections.len() > limit {
            return false;
        }
        self.placing = false;
        self.selections = next.selections;
        true
    }
    pub fn start(&mut self, selection: Selection) {
        if !self.active() {
            self.selections.push(selection);
        }
    }
    pub fn clear(&mut self) {
        *self = Self::default();
    }
    pub fn checkpoint(&mut self) {
        if self.history.last().is_none_or(|(selections, placing)| {
            selections != &self.selections || *placing != self.placing
        }) {
            self.history.push((self.selections.clone(), self.placing));
            if self.history.len() > 100 {
                self.history.remove(0);
            }
        }
    }
    pub fn undo_selection(&mut self) {
        if let Some((selections, placing)) = self.history.pop() {
            self.selections = selections;
            self.placing = placing;
        }
    }
    pub fn remap(&mut self, map: impl Fn(usize) -> usize) {
        for selection in &mut self.selections {
            selection.anchor = map(selection.anchor);
            selection.head = map(selection.head);
            selection.column = None;
            selection.visual_x = None;
        }
        self.history.clear();
        self.normalize(usize::MAX);
    }
    pub fn normalize(&mut self, len: usize) {
        // The movable cursor may coincide with a marked one. Keep both until
        // placement finishes so moving away does not erase the marked position.
        let moving = if self.placing {
            self.selections.pop()
        } else {
            None
        };
        // Keep creation order (and the newest primary) while merging overlaps.
        let mut indexed: Vec<_> = self
            .selections
            .iter()
            .copied()
            .enumerate()
            .map(|(i, mut s)| {
                s.anchor = s.anchor.min(len);
                s.head = s.head.min(len);
                (i, s)
            })
            .collect();
        indexed.sort_by_key(|(_, s)| (s.range().start, s.range().end));
        let mut merged: Vec<(usize, Selection)> = Vec::new();
        for (index, selection) in indexed {
            if let Some((old_index, old)) = merged.last_mut() {
                let a = old.range();
                let b = selection.range();
                if b.start < a.end || b.start == a.end && (a.is_empty() || b.is_empty()) {
                    let start = a.start.min(b.start);
                    let end = a.end.max(b.end);
                    let direction = if index > *old_index { selection } else { *old };
                    *old_index = index.max(*old_index);
                    *old = if direction.head < direction.anchor {
                        Selection::new(end, start)
                    } else {
                        Selection::new(start, end)
                    };
                    continue;
                }
            }
            merged.push((index, selection));
        }
        merged.sort_by_key(|(index, _)| *index);
        self.selections = merged.into_iter().map(|(_, s)| s).collect();
        if let Some(mut moving) = moving {
            moving.anchor = moving.anchor.min(len);
            moving.head = moving.head.min(len);
            self.selections.push(moving);
        }
    }
    pub fn toggle(&mut self, position: usize, limit: usize) -> bool {
        if !self.finish_placement(limit) {
            return false;
        }
        self.checkpoint();
        if let Some(index) = self
            .selections
            .iter()
            .position(|s| s.anchor == position && s.head == position)
        {
            if self.selections.len() > 1 {
                self.selections.remove(index);
            }
        } else {
            if self.selections.len() >= limit {
                return false;
            }
            self.selections.push(Selection::new(position, position));
        }
        self.normalize(usize::MAX);
        true
    }
    pub fn add_vertical(&mut self, text: &str, down: bool, limit: usize) -> bool {
        let Some(primary) = self.primary() else {
            return true;
        };
        let chars: Vec<_> = text.chars().collect();
        let rows = lines(&chars);
        let row = row_at(&rows, primary.head);
        let column = primary.column.unwrap_or(primary.head - rows[row].start);
        let boundaries = grapheme_boundaries(text);
        let mut target = row;
        let at = loop {
            target = if down {
                target + 1
            } else if target > 0 {
                target - 1
            } else {
                return true;
            };
            let Some(line) = rows.get(target) else {
                return true;
            };
            let at = grapheme_at_or_after(&boundaries, (line.start + column).min(line.end));
            if !self
                .selections
                .iter()
                .any(|s| s.range().contains(&at) || s.head == at)
            {
                break at;
            }
        };
        if self.selections.len() >= limit {
            return false;
        }
        self.checkpoint();
        self.selections.push(Selection {
            column: Some(column),
            ..Selection::new(at, at)
        });
        true
    }
    pub fn line_ends(&mut self, text: &str, limit: usize) -> bool {
        let chars: Vec<_> = text.chars().collect();
        let rows = lines(&chars);
        let mut ends = std::collections::BTreeSet::new();
        for selection in &self.selections {
            let range = selection.range();
            let last = if range.is_empty() {
                range.end
            } else {
                range.end - 1
            };
            for line in &rows[row_at(&rows, range.start)..=row_at(&rows, last)] {
                ends.insert(line.end);
            }
        }
        if ends.len() > limit {
            return false;
        }
        self.checkpoint();
        self.selections = ends.into_iter().map(|at| Selection::new(at, at)).collect();
        true
    }
    pub fn occurrences(
        &mut self,
        text: &str,
        all: bool,
        skip: bool,
        case: bool,
        whole_word: bool,
        limit: usize,
    ) -> bool {
        let Some(primary) = self.primary() else {
            return true;
        };
        let chars: Vec<_> = text.chars().collect();
        let mut range = primary.range();
        if range.is_empty() {
            let at = range.start.min(chars.len());
            if at == chars.len() || !word(chars[at]) {
                return true;
            }
            range = at..at;
            while range.start > 0 && word(chars[range.start - 1]) {
                range.start -= 1;
            }
            while range.end < chars.len() && word(chars[range.end]) {
                range.end += 1;
            }
            self.checkpoint();
            *self.selections.last_mut().unwrap() = Selection::new(range.start, range.end);
            if !all {
                return true;
            }
        } else {
            self.checkpoint();
        }
        let query: String = chars[range.clone()].iter().collect();
        let matches = crate::editor::find_matches(
            text,
            &query,
            crate::editor::SearchOptions {
                case_sensitive: case,
                whole_word: whole_word && query.chars().all(word),
                regex: false,
            },
        );
        if all {
            if matches.is_empty() {
                return true;
            }
            if matches.len() > limit {
                return false;
            }
            self.selections = matches
                .into_iter()
                .map(|r| Selection::new(r.start, r.end))
                .collect();
        } else {
            let next = matches
                .iter()
                .filter(|r| r.start >= range.end)
                .chain(matches.iter().filter(|r| r.start < range.end))
                .find(|r| !self.selections.iter().any(|s| overlaps(&s.range(), r)))
                .cloned();
            if let Some(next) = next {
                if !skip && self.selections.len() >= limit {
                    return false;
                }
                if skip {
                    self.selections.pop();
                }
                self.selections.push(Selection::new(next.start, next.end));
            }
        }
        self.normalize(chars.len());
        true
    }
}

fn overlaps(a: &Range<usize>, b: &Range<usize>) -> bool {
    a.start < b.end && b.start < a.end || a == b
}
fn word(c: char) -> bool {
    crate::search::word_char(c)
}

#[derive(Clone, Debug)]
pub struct Line {
    pub start: usize,
    pub end: usize,
    pub break_end: usize,
}
pub fn lines(chars: &[char]) -> Vec<Line> {
    let mut rows = Vec::new();
    let mut start = 0;
    for (i, &c) in chars.iter().enumerate() {
        if c == '\n' {
            rows.push(Line {
                start,
                end: if i > start && chars[i - 1] == '\r' {
                    i - 1
                } else {
                    i
                },
                break_end: i + 1,
            });
            start = i + 1;
        }
    }
    rows.push(Line {
        start,
        end: chars.len(),
        break_end: chars.len(),
    });
    rows
}
pub fn row_at(rows: &[Line], at: usize) -> usize {
    rows.partition_point(|r| r.start <= at).saturating_sub(1)
}

#[derive(Clone, Copy, Debug)]
pub enum Motion {
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    DocumentStart,
    DocumentEnd,
}

pub fn move_selections(state: &mut State, text: &str, motion: Motion, extend: bool, by_word: bool) {
    let chars: Vec<_> = text.chars().collect();
    let boundaries = grapheme_boundaries(text);
    let rows = lines(&chars);
    for s in state.moving_selections() {
        let row = row_at(&rows, s.head);
        let range = s.range();
        let head = match motion {
            Motion::Left if !extend && !range.is_empty() => range.start,
            Motion::Right if !extend && !range.is_empty() => range.end,
            Motion::Left => previous(&chars, &boundaries, s.head, by_word),
            Motion::Right => next(&chars, &boundaries, s.head, by_word),
            Motion::Up | Motion::Down => {
                let col = s.column.unwrap_or(s.head - rows[row].start);
                s.column = Some(col);
                let target = if matches!(motion, Motion::Up) {
                    row.saturating_sub(1)
                } else {
                    (row + 1).min(rows.len() - 1)
                };
                grapheme_at_or_after(
                    &boundaries,
                    (rows[target].start + col).min(rows[target].end),
                )
            }
            Motion::Home => {
                let first = rows[row].start
                    + chars[rows[row].start..rows[row].end]
                        .iter()
                        .take_while(|c| **c == ' ' || **c == '\t')
                        .count();
                if first == s.head {
                    rows[row].start
                } else {
                    first
                }
            }
            Motion::End => rows[row].end,
            Motion::DocumentStart => 0,
            Motion::DocumentEnd => chars.len(),
        };
        if !matches!(motion, Motion::Up | Motion::Down) {
            s.column = None;
            s.visual_x = None;
            s.prefer_next_row = !matches!(motion, Motion::End);
        }
        s.head = head;
        if !extend {
            s.anchor = head;
        }
    }
    state.normalize(chars.len());
}

fn grapheme_boundaries(text: &str) -> Vec<usize> {
    let mut at = 0;
    std::iter::once(0)
        .chain(text.graphemes(true).map(|g| {
            at += g.chars().count();
            at
        }))
        .collect()
}

fn grapheme_at_or_after(boundaries: &[usize], at: usize) -> usize {
    boundaries[boundaries.partition_point(|boundary| *boundary < at)]
}

fn previous(chars: &[char], boundaries: &[usize], mut at: usize, by_word: bool) -> usize {
    if by_word {
        while at > 0 && chars[at - 1].is_whitespace() {
            at -= 1;
        }
        if at > 0 {
            let kind = word(chars[at - 1]);
            while at > 0 && !chars[at - 1].is_whitespace() && word(chars[at - 1]) == kind {
                at -= 1;
            }
        }
    } else {
        at = boundaries[boundaries.partition_point(|b| *b < at).saturating_sub(1)];
    }
    at
}
fn next(chars: &[char], boundaries: &[usize], mut at: usize, by_word: bool) -> usize {
    if by_word {
        if at < chars.len() {
            let kind = word(chars[at]);
            while at < chars.len() && !chars[at].is_whitespace() && word(chars[at]) == kind {
                at += 1;
            }
        }
        while at < chars.len() && chars[at].is_whitespace() {
            at += 1;
        }
    } else if at < chars.len() {
        at = boundaries[boundaries
            .partition_point(|b| *b <= at)
            .min(boundaries.len() - 1)];
    }
    at
}

#[derive(Clone, Debug)]
pub struct Replacement {
    pub range: Range<usize>,
    pub text: String,
}

pub fn map_position(position: usize, edits: &[Replacement]) -> usize {
    let mut delta = 0isize;
    for edit in edits {
        if position < edit.range.start {
            break;
        }
        let inserted = edit.text.chars().count();
        if position <= edit.range.end {
            return edit.range.start.saturating_add_signed(delta) + inserted;
        }
        delta += inserted as isize - edit.range.len() as isize;
    }
    position.saturating_add_signed(delta)
}

/// Apply non-overlapping replacements in one UTF-8 pass. Selection endpoints
/// on a replacement map to its end; untouched endpoints track the same text.
pub fn apply(text: &str, state: &mut State, mut edits: Vec<Replacement>) -> String {
    edits.sort_by_key(|e| (e.range.start, e.range.end));
    edits.dedup_by(|b, a| a.range == b.range && a.text == b.text);
    let bytes: Vec<_> = text
        .char_indices()
        .map(|(b, _)| b)
        .chain(std::iter::once(text.len()))
        .collect();
    let mut output = String::with_capacity(text.len());
    let mut at = 0;
    for edit in &edits {
        assert!(edit.range.start >= at && edit.range.end < bytes.len());
        output.push_str(&text[bytes[at]..bytes[edit.range.start]]);
        output.push_str(&edit.text);
        at = edit.range.end;
    }
    output.push_str(&text[bytes[at]..]);
    // Resolve every endpoint in logarithmic time. Walking all edits for each
    // cursor becomes quadratic at the configurable 10,000-cursor limit.
    let mut delta = 0isize;
    let mapped: Vec<_> = edits
        .iter()
        .map(|edit| {
            let inserted = edit.text.chars().count();
            let end = edit.range.start.saturating_add_signed(delta) + inserted;
            delta += inserted as isize - edit.range.len() as isize;
            (end, delta)
        })
        .collect();
    state.remap(|position| {
        let index = edits.partition_point(|edit| edit.range.end < position);
        if edits
            .get(index)
            .is_some_and(|edit| edit.range.start <= position)
        {
            mapped[index].0
        } else {
            position.saturating_add_signed(index.checked_sub(1).map_or(0, |i| mapped[i].1))
        }
    });
    output
}

pub fn replace(text: &str, state: &mut State, values: &[String]) -> String {
    state.normalize(text.chars().count());
    let mut ranges: Vec<_> = state.selections.iter().map(|s| s.range()).collect();
    ranges.sort_by_key(|r| r.start);
    let edits = ranges
        .into_iter()
        .enumerate()
        .map(|(i, range)| Replacement {
            range,
            text: values[if values.len() == state.selections.len() {
                i
            } else {
                0
            }]
            .clone(),
        })
        .collect();
    let result = apply(text, state, edits);
    for s in &mut state.selections {
        s.anchor = s.head;
    }
    result
}

pub fn delete(text: &str, state: &mut State, backwards: bool, by_word: bool) -> String {
    let chars: Vec<_> = text.chars().collect();
    let boundaries = grapheme_boundaries(text);
    for s in &mut state.selections {
        if s.anchor == s.head {
            s.anchor = if backwards {
                previous(&chars, &boundaries, s.head, by_word)
            } else {
                next(&chars, &boundaries, s.head, by_word)
            };
        }
    }
    state.normalize(chars.len());
    replace(text, state, &[String::new()])
}

pub fn clipboard(text: &str, state: &State) -> (String, Vec<Range<usize>>) {
    let chars: Vec<_> = text.chars().collect();
    let rows = lines(&chars);
    let linewise = state.selections.iter().all(|s| s.anchor == s.head);
    let mut ranges: Vec<_> = state
        .selections
        .iter()
        .filter_map(|s| {
            if linewise {
                let row = &rows[row_at(&rows, s.head)];
                Some(row.start..row.break_end)
            } else {
                let range = s.range();
                (!range.is_empty()).then_some(range)
            }
        })
        .collect();
    ranges.sort_by_key(|r| r.start);
    ranges.dedup();
    let parts: Vec<String> = ranges
        .iter()
        .map(|r| chars[r.clone()].iter().collect())
        .collect();
    (
        if linewise {
            parts.concat()
        } else {
            parts.join("\n")
        },
        ranges,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state(ranges: &[(usize, usize)]) -> State {
        State {
            selections: ranges.iter().map(|&(a, h)| Selection::new(a, h)).collect(),
            ..State::default()
        }
    }
    #[test]
    fn simultaneous_unicode_replacement_and_primary() {
        let mut s = state(&[(5, 7), (0, 2)]);
        assert_eq!(replace("🦀a / 🦀a", &mut s, &["é".into()]), "é / é");
        assert_eq!(s.primary().unwrap().head, 1);
        assert_eq!(s.selections[0].head, 5);
    }
    #[test]
    fn overlap_merges_but_adjacent_selections_stay_separate() {
        let mut s = state(&[(0, 3), (2, 5), (5, 7), (4, 4)]);
        s.normalize(7);
        assert_eq!(s.selections.len(), 2);
        assert_eq!(replace("abcdefg", &mut s, &["X".into()]), "XX");
    }
    #[test]
    fn crlf_deleted_atomically_and_overlaps_only_once() {
        let mut s = state(&[(3, 3), (4, 4)]);
        assert_eq!(delete("a\r\nb", &mut s, true, false), "a");
        assert_eq!(s.selections.len(), 1);
    }
    #[test]
    fn vertical_cursors_keep_column_through_short_lines() {
        let mut s = state(&[(4, 4)]);
        s.add_vertical("abcdef\nx\nabcdef", true, 100);
        s.add_vertical("abcdef\nx\nabcdef", true, 100);
        assert_eq!(s.primary().unwrap().head, 13);
        let mut both = state(&[(6, 6)]);
        assert!(both.add_vertical("aaaa\nbbbb\ncccc", false, 3));
        assert!(both.add_vertical("aaaa\nbbbb\ncccc", true, 3));
        assert_eq!(
            both.selections.iter().map(|s| s.head).collect::<Vec<_>>(),
            vec![6, 1, 11]
        );
        assert!(both.add_vertical("aaaa\nbbbb\ncccc", false, 3));
        assert_eq!(both.selections.len(), 3);
        both.undo_selection();
        assert_eq!(
            both.selections.iter().map(|s| s.head).collect::<Vec<_>>(),
            vec![6, 1]
        );
        for ending in ["\n", "\r\n"] {
            for grapheme in ["👩‍💻", "e\u{301}"] {
                let text = ["ab", &format!("{grapheme}x"), "cd"].join(ending);
                let after_grapheme = 2 + ending.len() + grapheme.chars().count();
                let mut s = state(&[(1, 1)]);
                s.add_vertical(&text, true, 100);
                assert_eq!(s.primary().unwrap().head, after_grapheme);
                assert_eq!(
                    replace(&text, &mut s, &["X".into()]),
                    ["aXb", &format!("{grapheme}Xx"), "cd"].join(ending)
                );
                let mut s = state(&[(1, 1)]);
                s.mark_cursor(100);
                move_selections(&mut s, &text, Motion::Down, false, false);
                assert_eq!(s.primary().unwrap().head, after_grapheme);
                move_selections(&mut s, &text, Motion::Down, false, false);
                assert_eq!(s.primary().unwrap().head, text.chars().count() - 1);
            }
        }
    }
    #[test]
    fn placement_limit_is_atomic_and_cursor_undo_restores_marking() {
        let mut s = state(&[(0, 0)]);
        assert!(s.mark_cursor(2));
        move_selections(&mut s, "abcd", Motion::Right, false, false);
        assert!(s.mark_cursor(2));
        move_selections(&mut s, "abcd", Motion::Right, false, false);
        let before = s.clone();
        assert!(!s.mark_cursor(2));
        assert!(!s.finish_placement(2));
        assert_eq!(s, before);
        s.undo_selection();
        assert!(s.placing());
        assert_eq!(
            s.selections,
            vec![Selection::new(0, 0), Selection::new(1, 1)]
        );
        assert!(s.finish_placement(2));
        assert!(!s.placing());
        assert_eq!(replace("abcd", &mut s, &["X".into()]), "XaXbcd");
    }
    #[test]
    fn paste_distributes_in_document_order() {
        let mut s = state(&[(2, 2), (0, 0)]);
        assert_eq!(replace("a\nb", &mut s, &["1".into(), "2".into()]), "1a\n2b");
    }
    #[test]
    fn occurrences_skip_wrap_and_cursor_undo() {
        let mut s = state(&[(0, 0)]);
        s.occurrences("foo foo foo", false, false, true, true, 100);
        s.occurrences("foo foo foo", false, false, true, true, 100);
        s.occurrences("foo foo foo", false, true, true, true, 100);
        assert_eq!(s.primary().unwrap().range(), 8..11);
        s.undo_selection();
        assert_eq!(s.primary().unwrap().range(), 4..7);

        // A decomposed accent is part of the word, even when the cursor is
        // positioned directly on its combining mark.
        for cursor in [0, 4] {
            let mut s = state(&[(cursor, cursor)]);
            let text = "cafe\u{301} cafe\u{301}";
            s.occurrences(text, false, false, true, true, 100);
            assert_eq!(s.primary().unwrap().range(), 0..5);
            s.occurrences(text, false, false, true, true, 100);
            assert_eq!(s.primary().unwrap().range(), 6..11);
        }
    }
}

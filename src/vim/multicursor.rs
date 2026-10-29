//! Fan modal commands out through the existing Vim decoder, then commit their
//! edits against one source snapshot. Only navigation/repeat state is per cursor;
//! document history remains on the parent VimState.
use super::*;
use crate::multicursor::{Replacement, Selection, State, apply, map_position};

#[derive(Clone, Debug)]
pub(super) struct Cursor {
    selection: Selection,
    navigation: VimNavigation,
    change: Option<RepeatChange>,
    insert: Option<Insertion>,
}

#[derive(Clone, Debug)]
struct Insertion {
    keys: Vec<VimKey>,
    count: usize,
    line_prefix: Option<String>,
    visual: Option<VisualRepeat>,
    // A compact, composed local edit; no document/history copy per cursor.
    edit: Option<Replacement>,
    original: String,
    relative_start: isize,
    changed: bool,
}

impl Cursor {
    pub(super) fn remap(&mut self, map: &dyn Fn(usize) -> usize) {
        self.selection.anchor = map(self.selection.anchor);
        self.selection.head = map(self.selection.head);
        self.navigation = self.navigation.remapped(map);
        self.insert = None;
    }
}

fn slice(text: &str, range: Range<usize>) -> String {
    text.chars().skip(range.start).take(range.len()).collect()
}

pub(super) fn replacements(before: &str, after: &str) -> Vec<Replacement> {
    let mut edits: Vec<Replacement> = Vec::new();
    let mut at = 0;
    let mut pending: Option<Replacement> = None;
    for change in similar::TextDiff::from_chars(before, after).iter_all_changes() {
        let count = change.value().chars().count();
        match change.tag() {
            similar::ChangeTag::Equal => {
                if let Some(edit) = pending.take() {
                    edits.push(edit);
                }
                at += count;
            }
            similar::ChangeTag::Delete => {
                pending
                    .get_or_insert_with(|| Replacement {
                        range: at..at,
                        text: String::new(),
                    })
                    .range
                    .end += count;
                at += count;
            }
            similar::ChangeTag::Insert => {
                pending
                    .get_or_insert_with(|| Replacement {
                        range: at..at,
                        text: String::new(),
                    })
                    .text
                    .push_str(change.value());
            }
        }
    }
    if let Some(edit) = pending {
        edits.push(edit);
    }
    edits
}

// Map a cursor in one command's result into the result of all commands. Keep
// positions inside inserted text rather than snapping them to its far boundary.
fn result_position(position: usize, own: &[Replacement], all: &[Replacement]) -> usize {
    let mut delta = 0isize;
    for edit in own {
        let start = edit.range.start.saturating_add_signed(delta);
        let end = start + edit.text.chars().count();
        if position < start {
            break;
        }
        if position <= end {
            if edit.text.is_empty() {
                return map_position(edit.range.start, all);
            }
            let before: isize = all
                .iter()
                .take_while(|other| {
                    other.range.end <= edit.range.start && other.range.start < edit.range.start
                })
                .map(|other| other.text.chars().count() as isize - other.range.len() as isize)
                .sum();
            return edit.range.start.saturating_add_signed(before) + position - start;
        }
        delta += edit.text.chars().count() as isize - edit.range.len() as isize;
    }
    map_position(position.saturating_add_signed(-delta), all)
}

impl VimState {
    fn multi_cursor(&self, selection: Selection) -> Cursor {
        if let Some(cursor) = self
            .multi_cursors
            .iter()
            .find(|cursor| cursor.selection == selection && cursor.navigation.mode == self.mode)
        {
            return cursor.clone();
        }
        let mut single = VimState::new();
        if self.mode == VimMode::Insert {
            single.mode = VimMode::Insert;
            single.cursor = selection.head;
        } else if matches!(self.mode, VimMode::Visual | VimMode::VisualLine)
            && !selection.range().is_empty()
        {
            // Directed selection heads are exclusive; Vim cursors are inclusive.
            single.visual_anchor = Some(if selection.head >= selection.anchor {
                selection.anchor
            } else {
                selection.anchor - 1
            });
            single.cursor = if selection.head > selection.anchor {
                selection.head - 1
            } else {
                selection.head
            };
            single.mode = self.mode;
        } else {
            single.cursor = if selection.head > selection.anchor {
                selection.head - 1
            } else {
                selection.head
            };
        }
        Cursor {
            selection,
            navigation: single.navigation(),
            change: self.last_change.clone(),
            insert: (self.mode == VimMode::Insert).then(|| Insertion {
                keys: vec![VimKey::Char('i')],
                count: 1,
                line_prefix: None,
                visual: None,
                edit: None,
                original: String::new(),
                relative_start: 0,
                changed: false,
            }),
        }
    }

    /// Track native multicursor Insert edits for dot/count replay without retaining
    /// a full copy of the document for each insertion point.
    pub fn track_multi_insert(
        &mut self,
        before: &str,
        after: &str,
        state: &State,
        replacements: Option<Vec<Replacement>>,
    ) {
        if self.mode != VimMode::Insert || before == after {
            return;
        }
        let edits = replacements.unwrap_or_else(|| self::replacements(before, after));
        let mut cursors = Vec::new();
        for &selection in &self.multi.selections {
            let mut cursor = self.multi_cursor(selection);
            let owned: Vec<_> = edits
                .iter()
                .filter(|edit| {
                    edit.range.start <= selection.range().end
                        && edit.range.end >= selection.range().start
                })
                .collect();
            if let Some(insert) = cursor.insert.as_mut() {
                for edit in owned.iter().copied() {
                    let (range, relative_start, original) = if let Some(old) = &insert.edit {
                        let range = old.range.start.min(edit.range.start)
                            ..(old.range.start + old.text.chars().count()).max(edit.range.end);
                        let mut original = slice(before, range.clone());
                        let start = char_to_byte(&original, old.range.start - range.start);
                        let end = char_to_byte(
                            &original,
                            old.range.start - range.start + old.text.chars().count(),
                        );
                        original.replace_range(start..end, &insert.original);
                        let relative_start =
                            insert.relative_start - (old.range.start - range.start) as isize;
                        (range, relative_start, original)
                    } else {
                        (
                            edit.range.clone(),
                            edit.range.start as isize - cursor.navigation.cursor as isize,
                            slice(before, edit.range.clone()),
                        )
                    };
                    let mut inserted = slice(before, range.clone());
                    let start = char_to_byte(&inserted, edit.range.start - range.start);
                    let end = char_to_byte(&inserted, edit.range.end - range.start);
                    inserted.replace_range(start..end, &edit.text);
                    insert.original = original;
                    insert.relative_start = relative_start;
                    insert.changed = true;
                    let other: Vec<_> = edits
                        .iter()
                        .filter(|other| !std::ptr::eq(*other, edit))
                        .cloned()
                        .collect();
                    let start = map_position(range.start, &other);
                    insert.edit = Some(Replacement {
                        range: start..start + insert.original.chars().count(),
                        text: inserted,
                    });
                }
                if owned.is_empty()
                    && let Some(edit) = &mut insert.edit
                {
                    let start = map_position(edit.range.start, &edits);
                    edit.range = start..start + insert.original.chars().count();
                }
            }
            let head = map_position(selection.head, &edits);
            cursor.navigation.cursor = head;
            cursor.selection = Selection::new(head, head);
            cursors.push(cursor);
        }
        self.multi_cursors = cursors;
        // A merged cursor no longer has an independent insert transaction.
        self.multi_cursors
            .retain(|cursor| state.selections.contains(&cursor.selection));
    }

    pub(super) fn handle_multi_key(&mut self, text: &mut String, key: VimKey) -> Action {
        if self.mode == VimMode::Normal
            && self.pending.is_none()
            && let VimKey::Char(ch) = key
            && super::navigation::direct_motion(ch)
            && !(ch == '0' && self.count > 0)
        {
            return self.multi_normal_motion(text, ch);
        }
        if key == VimKey::Redo
            || key == VimKey::Char('u') && self.mode == VimMode::Normal && self.pending.is_none()
        {
            self.multi_cursors.clear();
            let count = self.take_count();
            let mut action = Action {
                consumed: true,
                ..Action::default()
            };
            for _ in 0..count {
                let step = self.history_edit(text, key == VimKey::Redo);
                action.text_changed |= step.text_changed;
                action.cursor_changed |= step.cursor_changed;
                action.mode_changed |= step.mode_changed;
            }
            return action;
        }
        if key == VimKey::VisualBlock {
            return Action {
                consumed: true,
                message: Some("Finish multicursor editing before using Visual Block".into()),
                ..Action::default()
            };
        }
        self.multi_action(text, |single, local| single.handle_key(local, key))
    }

    // Normal motions share one character buffer and the existing motion owner.
    // Avoid decoding/indexing the entire document once for every cursor/key.
    fn multi_normal_motion(&mut self, text: &str, ch: char) -> Action {
        let chars: Vec<_> = text.chars().collect();
        let clamp = |position: usize| {
            let at = position.min(chars.len());
            let line = line_at_cursor(&chars, at);
            at.min(line.end.saturating_sub(1).max(line.start))
        };
        let old_cursor = self.cursor;
        let mut cursors = Vec::with_capacity(self.multi.selections.len());
        for (index, &selection) in self.multi.selections.iter().enumerate() {
            let mut cursor = self.multi_cursor(selection);
            if !self.multi.placing() || index + 1 == self.multi.selections.len() {
                let mut single = VimState {
                    cursor: clamp(cursor.navigation.cursor),
                    preferred_column: cursor.navigation.preferred_column,
                    visible: self.visible.clone(),
                    ..VimState::default()
                };
                let count = cursor.navigation.count;
                single.move_cursor(&chars, ch, count.max(1), count > 0);
                cursor.navigation.cursor = clamp(single.cursor);
                cursor.navigation.preferred_column = single.preferred_column;
                cursor.navigation.count = 0;
                cursor.navigation.command_keys.clear();
                cursor.navigation.surround_search = None;
                cursor.navigation.motion_epoch = cursor.navigation.motion_epoch.wrapping_add(1);
                cursor.selection =
                    Selection::new(cursor.navigation.cursor, cursor.navigation.cursor);
            }
            cursors.push(cursor);
        }
        self.multi.selections = cursors.iter().map(|cursor| cursor.selection).collect();
        self.multi.normalize(chars.len());
        self.multi_cursors = cursors;
        self.cursor = self.multi.primary().unwrap().head;
        self.count = 0;
        self.command_keys.clear();
        self.surround_search = None;
        self.motion_epoch = self.motion_epoch.wrapping_add(1);
        Action {
            consumed: true,
            cursor_changed: self.cursor != old_cursor,
            ..Action::default()
        }
    }

    pub(super) fn multi_scroll_lines(&mut self, text: &mut String, delta: isize) -> Action {
        self.multi_action(text, |single, local| single.scroll_lines(local, delta))
    }

    pub(super) fn finish_multi_insert_history(&mut self, text: &mut String) -> Action {
        let action = self.multi_action(text, |single, local| {
            let boundary = single.cursor;
            let before = local.clone();
            let action = single.handle_key(local, VimKey::Escape);
            single.cursor = if action.text_changed {
                map_position(boundary, &replacements(&before, local))
            } else {
                boundary
            };
            single.mode = VimMode::Insert;
            action
        });
        if let Some(origin) = self.insert_origin.take()
            && origin.text != *text
        {
            self.record(origin);
        }
        self.insert_replay = None;
        self.insert_count = 1;
        action
    }

    fn multi_action(
        &mut self,
        text: &mut String,
        mut run: impl FnMut(&mut VimState, &mut String) -> Action,
    ) -> Action {
        let before = self.snapshot(text);
        let old_mode = self.mode;
        let old_cursor = self.cursor;
        let mut results = Vec::new();
        let mut edits = Vec::new();
        let mut yank_register = self.yank_register.clone();
        let mut local = text.clone();
        let mut action = Action {
            consumed: true,
            ..Action::default()
        };
        for (index, &selection) in self.multi.selections.iter().enumerate() {
            let cursor = self.multi_cursor(selection);
            if self.multi.placing() && index + 1 != self.multi.selections.len() {
                results.push((cursor, Vec::new()));
                continue;
            }
            let mut single = VimState::new();
            single.restore_navigation(text, &cursor.navigation);
            single.multi_cursors.clear();
            single.set_indentation(self.indent_width, self.indent_with_tabs);
            single.register = self.register.clone();
            single.yank_register = self.yank_register.clone();
            single.last_change = cursor.change;
            single.visible = self.visible.clone();
            if old_mode == VimMode::Insert {
                single.insert_origin = None;
                single.insert_replay = None;
                if let Some(insert) = &cursor.insert {
                    let mut baseline = text.clone();
                    let mut origin_cursor = single.cursor;
                    if let Some(edit) = &insert.edit {
                        let start = char_to_byte(&baseline, edit.range.start);
                        let end =
                            char_to_byte(&baseline, edit.range.start + edit.text.chars().count());
                        baseline.replace_range(start..end, &insert.original);
                        origin_cursor = edit
                            .range
                            .start
                            .saturating_add_signed(-insert.relative_start);
                    }
                    single.insert_origin = Some(single.snapshot(&baseline));
                    single.insert_replay = Some(InsertReplay {
                        keys: insert.keys.clone(),
                        baseline,
                        cursor: origin_cursor,
                        search: None,
                        visual: insert.visual.clone(),
                    });
                    single.insert_count = insert.count;
                    single.insert_line_prefix = insert.line_prefix.clone();
                }
            }
            let result = run(&mut single, &mut local);
            yank_register = single.yank_register.clone();
            if old_mode == VimMode::Insert
                && single.insert_replay.is_none()
                && let Some(insert) = &cursor.insert
                && insert.changed
                && insert.edit.is_none()
            {
                single.last_change = Some(RepeatChange {
                    keys: insert.keys.clone(),
                    insert: Some(InsertDelta {
                        relative_start: 0,
                        removed: 0,
                        inserted: String::new(),
                    }),
                    search: None,
                    visual: insert.visual.clone(),
                });
            }
            let own = if result.text_changed {
                replacements(text, &local)
            } else {
                Vec::new()
            };
            edits.extend(own.iter().cloned());
            action.yanked_ranges.extend(result.yanked_ranges);
            if result.yanked.is_some() {
                action.yanked = result.yanked;
            }
            action.scroll = result.scroll.or(action.scroll);
            action.message = result.message.or(action.message);
            let insert = if old_mode != VimMode::Insert && single.mode == VimMode::Insert {
                single.insert_replay.as_ref().map(|replay| Insertion {
                    keys: replay.keys.clone(),
                    count: single.insert_count,
                    line_prefix: single.insert_line_prefix.clone(),
                    visual: replay.visual.clone(),
                    edit: None,
                    original: String::new(),
                    relative_start: 0,
                    changed: result.text_changed,
                })
            } else {
                None
            };
            let range = single
                .selection(&local)
                .unwrap_or(single.cursor..single.cursor);
            let selection = if single.cursor == range.start && !range.is_empty() {
                Selection::new(range.end, range.start)
            } else {
                Selection::new(range.start, range.end)
            };
            results.push((
                Cursor {
                    selection,
                    navigation: single.navigation(),
                    change: single.last_change,
                    insert,
                },
                own,
            ));
            if result.text_changed {
                local.clone_from(text);
            }
        }
        edits.sort_by_key(|edit| (edit.range.start, edit.range.end));
        let mut merged: Vec<Replacement> = Vec::new();
        for edit in edits {
            if let Some(last) = merged.last_mut() {
                if last.range == edit.range && last.text == edit.text {
                    continue;
                }
                if edit.range.start < last.range.end
                    || edit.range.is_empty() && last.range == edit.range
                {
                    if last.text.is_empty() && edit.text.is_empty() {
                        last.range.end = last.range.end.max(edit.range.end);
                        continue;
                    }
                    return Action {
                        consumed: true,
                        message: Some(
                            "Overlapping Vim edits: refine the cursors before retrying".into(),
                        ),
                        ..Action::default()
                    };
                }
            }
            merged.push(edit);
        }
        if old_mode != VimMode::Insert
            && results
                .last()
                .is_some_and(|(cursor, _)| cursor.navigation.mode == VimMode::Insert)
        {
            self.insert_origin = Some(before.clone());
        } else if old_mode != VimMode::Insert && !merged.is_empty() {
            self.record(before);
        }
        let output = if merged.is_empty() {
            None
        } else {
            Some(apply(text, &mut State::default(), merged.clone()))
        };
        let mut state = self.multi.clone();
        state.selections.clear();
        self.multi_cursors.clear();
        for (mut cursor, own) in results {
            let map = |position| result_position(position, &own, &merged);
            cursor.selection.anchor = map(cursor.selection.anchor);
            cursor.selection.head = map(cursor.selection.head);
            cursor.navigation = cursor.navigation.remapped(map);
            state.selections.push(cursor.selection);
            self.multi_cursors.push(cursor);
        }
        if let Some(output) = output {
            *text = output;
        }
        state.normalize(text.chars().count());
        if let Some(primary) = self.multi_cursors.last().cloned() {
            self.mode = primary.navigation.mode;
            self.cursor = primary.navigation.cursor;
            self.pending = primary.navigation.pending;
            self.count = primary.navigation.count;
            self.command_keys = primary.navigation.command_keys;
            self.last_change = primary.change;
        }
        self.multi = state;
        self.yank_register = yank_register;
        if old_mode == VimMode::Insert
            && self.mode != VimMode::Insert
            && let Some(origin) = self.insert_origin.take()
            && origin.text != *text
        {
            self.record(origin);
        }
        if let Some(yank) = &action.yanked {
            self.register = Some(yank.clone());
        }
        action.text_changed = !merged.is_empty();
        action.cursor_changed = old_cursor != self.cursor;
        action.mode_changed = old_mode != self.mode;
        self.motion_epoch = self.motion_epoch.wrapping_add(1);
        action
    }

    pub fn multicursor_head(&self, selection: Selection) -> usize {
        self.multi_cursors
            .iter()
            .find(|cursor| cursor.selection == selection && cursor.navigation.mode == self.mode)
            .map_or_else(
                || {
                    if matches!(self.mode, VimMode::Visual | VimMode::VisualLine)
                        && selection.head > selection.anchor
                    {
                        selection.head - 1
                    } else {
                        selection.head
                    }
                },
                |cursor| cursor.navigation.cursor,
            )
    }

    pub fn track_multi_motion(&mut self) {
        if self.mode == VimMode::Insert && self.multi_cursors.len() == self.multi.selections.len() {
            for (cursor, &selection) in self.multi_cursors.iter_mut().zip(&self.multi.selections) {
                cursor.selection = selection;
                cursor.navigation.cursor = selection.head;
            }
        }
    }
}

//! Unicode-safe modal editing. Cursors and selection ranges use Unicode
//! character indices, matching egui's text cursor. Keep a state per document.
use std::ops::Range;

const MAX_COUNT: usize = 9_999;
const MAX_HISTORY: usize = 100;
const MAX_HISTORY_BYTES: usize = 16 * 1024 * 1024;
const MAX_PASTE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VimMode {
    #[default]
    Normal,
    Insert,
    Visual,
    VisualLine,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VimKey {
    Char(char),
    Escape,
    Enter,
    Backspace,
    Tab,
    /// Map normal-mode Ctrl+R to this key.
    Redo,
    /// Vim's Ctrl+A: add the count to the number at or after the cursor.
    Increment,
    /// Vim's Ctrl+X: subtract the count from the number at or after the cursor.
    Decrement,
}

/// Where the view should place the cursor line after zt, zz or zb.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollAlign {
    Top,
    Center,
    Bottom,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Yank {
    /// Linewise register values end in a newline.
    pub text: String,
    pub linewise: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Action {
    pub consumed: bool,
    pub text_changed: bool,
    pub cursor_changed: bool,
    pub mode_changed: bool,
    /// Vim yanks and deletions, for sharing the unnamed register across tabs.
    pub yanked: Option<Yank>,
    /// Actual source spans in character indices, before any destructive edit.
    /// A linewise register's synthetic final newline has no source span.
    pub yanked_ranges: Vec<Range<usize>>,
    /// A view request from zt, zz or zb; the UI owns the scroll position.
    pub scroll: Option<ScrollAlign>,
    /// Short feedback for the status bar, e.g. for an unsupported command.
    pub message: Option<String>,
}

/// Remap physical Vim navigation keys. Callers leave literal targets and
/// inserted text untouched; arrows and scrolling already express a direction.
pub fn map_jk(character: char, swap_jk: bool) -> char {
    match (character, swap_jk) {
        ('j', true) => 'k',
        ('k', true) => 'j',
        _ => character,
    }
}

/// Native Copy/Cut use the selection, or the current line when it is empty.
/// The final newline is included only when it exists in the source document.
pub fn clipboard_range(text: &str, selection: Range<usize>, cursor: usize) -> Range<usize> {
    let chars: Vec<_> = text.chars().collect();
    let start = selection.start.min(chars.len());
    let end = selection.end.min(chars.len()).max(start);
    if start != end {
        return start..end;
    }
    let rows = lines(&chars);
    let line = rows[row_for_cursor(&rows, cursor.min(chars.len()))];
    line.start..line.newline.map_or(line.end, |index| index + 1)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Operator {
    Delete,
    Change,
    Yank,
    Indent,
    Outdent,
    Surround,
    Lowercase,
    Uppercase,
    ToggleCase,
}

impl Operator {
    fn key(self) -> char {
        match self {
            Self::Delete => 'd',
            Self::Change => 'c',
            Self::Yank => 'y',
            Self::Indent => '>',
            Self::Outdent => '<',
            Self::Surround => 's',
            Self::Lowercase => 'u',
            Self::Uppercase => 'U',
            Self::ToggleCase => '~',
        }
    }

    fn command(self) -> &'static str {
        match self {
            Self::Delete => "d",
            Self::Change => "c",
            Self::Yank => "y",
            Self::Indent => ">",
            Self::Outdent => "<",
            Self::Surround => "s",
            Self::Lowercase => "gu",
            Self::Uppercase => "gU",
            Self::ToggleCase => "g~",
        }
    }

    fn changes_case(self) -> bool {
        matches!(self, Self::Lowercase | Self::Uppercase | Self::ToggleCase)
    }
}

#[derive(Clone, Copy, Debug)]
struct Find {
    character: char,
    forward: bool,
    till: bool,
}

#[derive(Clone, Copy, Debug)]
enum Pending {
    Operator(Operator, usize, bool),
    Go {
        operator: Option<Operator>,
        count: usize,
        explicit: bool,
    },
    Replace(usize),
    Register(usize),
    RegisterPaste(usize),
    SurroundAdd(usize, usize),
    SurroundTarget {
        change: bool,
        count: usize,
    },
    SurroundChange {
        start: usize,
        end: usize,
        target: char,
    },
    Find {
        operator: Option<Operator>,
        count: usize,
        forward: bool,
        till: bool,
    },
    TextObject {
        operator: Option<Operator>,
        count: usize,
        around: bool,
    },
    /// `z` waits for a view command such as `zz`.
    Scroll {
        count: usize,
        explicit: bool,
    },
    /// The argument of an unsupported command such as `q` or `m`. It is
    /// swallowed so that the user's next keys do not run as commands.
    Ignore,
    /// `[` or `]`: the editor handles `[d` / `]d` (previous / next problem);
    /// other bracket commands are unsupported and swallow their argument.
    Bracket(char),
}

/// The most recent Visual area, for `gv`.
#[derive(Clone, Copy, Debug)]
struct LastVisual {
    mode: VimMode,
    anchor: usize,
    cursor: usize,
    to_line_end: bool,
}

#[derive(Clone, Debug)]
struct Snapshot {
    text: String,
    cursor: usize,
}

#[derive(Clone, Debug)]
struct InsertDelta {
    relative_start: isize,
    removed: usize,
    inserted: String,
}

#[derive(Clone, Debug)]
struct RepeatChange {
    keys: Vec<VimKey>,
    insert: Option<InsertDelta>,
    search: Option<SearchMotion>,
    visual: Option<VisualRepeat>,
}

/// Native selections have no keystroke history. Store their area rather than
/// replaying an incomplete prefix or attempting to reconstruct mouse motions.
#[derive(Clone, Debug)]
struct VisualRepeat {
    linewise: bool,
    rows: usize,
    columns: usize,
    includes_newline: bool,
    to_line_end: bool,
}

#[derive(Clone, Debug)]
struct InsertReplay {
    keys: Vec<VimKey>,
    baseline: String,
    cursor: usize,
    search: Option<SearchMotion>,
    visual: Option<VisualRepeat>,
}

/// An application-level search motion. Keeping the pattern with a change lets
/// dot repeat resolve the same exclusive motion at the new cursor.
#[derive(Clone, Debug)]
pub struct SearchMotion {
    pub query: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
    pub backwards: bool,
    pub count: usize,
}

#[derive(Clone, Copy, Debug)]
struct Line {
    start: usize,
    end: usize,
    newline: Option<usize>,
}

/// The navigation belonging to one view of a document. Undo history, registers,
/// repeat commands and the current insert transaction stay on `VimState`.
#[derive(Clone, Debug, Default)]
pub struct VimNavigation {
    pub mode: VimMode,
    pub cursor: usize,
    visual_anchor: Option<usize>,
    pending: Option<Pending>,
    count: usize,
    preferred_column: Option<usize>,
    last_find: Option<Find>,
    command_keys: Vec<VimKey>,
    surround_search: Option<SearchMotion>,
    motion_epoch: u64,
    visual_to_line_end: bool,
    last_visual: Option<LastVisual>,
}

impl VimNavigation {
    /// Move a view with its source text while keeping its visual selection's
    /// direction and modal state. The delta uses Unicode character indices.
    pub fn shifted(&self, delta: isize) -> Self {
        self.remapped(|position| position.saturating_add_signed(delta))
    }

    /// Map every stored position of this view, e.g. after another view edited
    /// the shared document. Modal state and pending commands stay unchanged.
    pub fn remapped(&self, map: impl Fn(usize) -> usize) -> Self {
        let mut navigation = self.clone();
        navigation.cursor = map(navigation.cursor);
        navigation.visual_anchor = navigation.visual_anchor.map(&map);
        if let Some(last) = navigation.last_visual.as_mut() {
            last.anchor = map(last.anchor);
            last.cursor = map(last.cursor);
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
        navigation
    }
}

#[derive(Clone, Debug)]
pub struct VimState {
    pub mode: VimMode,
    /// Insert mode: insertion boundary. Other modes: current character.
    pub cursor: usize,
    register: Option<Yank>,
    yank_register: Option<Yank>,
    yanked_ranges: Vec<Range<usize>>,
    pending: Option<Pending>,
    count: usize,
    preferred_column: Option<usize>,
    visual_anchor: Option<usize>,
    last_find: Option<Find>,
    indent_width: usize,
    indent_with_tabs: bool,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    insert_origin: Option<Snapshot>,
    insert_count: usize,
    command_keys: Vec<VimKey>,
    surround_search: Option<SearchMotion>,
    last_change: Option<RepeatChange>,
    insert_replay: Option<InsertReplay>,
    replaying: bool,
    motion_epoch: u64,
    visual_to_line_end: bool,
    last_visual: Option<LastVisual>,
    /// Line break and indentation that separate the copies of a counted `o`.
    insert_line_prefix: Option<String>,
    /// Character range of the lines the view currently shows, for H/M/L.
    visible: Option<Range<usize>>,
    scroll_request: Option<ScrollAlign>,
    message: Option<String>,
}

impl Default for VimState {
    fn default() -> Self {
        Self {
            mode: VimMode::Normal,
            cursor: 0,
            register: None,
            yank_register: None,
            yanked_ranges: Vec::new(),
            pending: None,
            count: 0,
            preferred_column: None,
            visual_anchor: None,
            last_find: None,
            indent_width: 4,
            indent_with_tabs: false,
            undo: Vec::new(),
            redo: Vec::new(),
            insert_origin: None,
            insert_count: 1,
            command_keys: Vec::new(),
            surround_search: None,
            last_change: None,
            insert_replay: None,
            replaying: false,
            motion_epoch: 0,
            visual_to_line_end: false,
            last_visual: None,
            insert_line_prefix: None,
            visible: None,
            scroll_request: None,
            message: None,
        }
    }
}

impl VimState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn navigation(&self) -> VimNavigation {
        VimNavigation {
            mode: self.mode,
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
        }
    }

    /// Restore a view without replacing this document's shared editing history.
    /// Finish an active insert session before switching to another view. An
    /// Insert snapshot starts a new transaction if its original one was finished.
    pub fn restore_navigation(&mut self, text: &str, navigation: &VimNavigation) {
        let len = text.chars().count();
        self.mode = navigation.mode;
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
        self.motion_epoch = self.motion_epoch.wrapping_add(1);
        self.visual_to_line_end = false;
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
        self.motion_epoch = self.motion_epoch.wrapping_add(1);
        self.visual_to_line_end = false;
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

    #[cfg(test)]
    pub fn register(&self) -> Option<&Yank> {
        self.register.as_ref()
    }
    pub fn set_register(&mut self, yank: Yank) {
        self.register = Some(yank);
    }
    pub fn yank_register(&self) -> Option<&Yank> {
        self.yank_register.as_ref()
    }
    pub fn set_yank_register(&mut self, yank: Option<Yank>) {
        self.yank_register = yank;
    }
    pub fn set_indentation(&mut self, width: usize, tabs: bool) {
        self.indent_width = width.clamp(1, 16);
        self.indent_with_tabs = tabs;
    }
    /// The half-open character range that the active view shows completely.
    /// H, M and L use it; without a view the whole document counts.
    pub fn set_visible_range(&mut self, range: Option<Range<usize>>) {
        self.visible = range;
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
            }
            return;
        }
        let before = self.snapshot(text);
        self.leave_visual();
        self.cancel_pending_command();
        self.insert_origin = None;
        self.insert_replay = None;
        self.insert_count = 1;
        self.record(before);
    }
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

    /// Cut the completed Visual area, discarding only unfinished command input.
    /// Geometry makes dot reliable for keyboard, mouse and external selections,
    /// even when a pending r/i/g/count or the bounded key history is incomplete.
    pub fn cut_visual(&mut self, text: &mut String) -> Action {
        self.yanked_ranges.clear();
        if !matches!(self.mode, VimMode::Visual | VimMode::VisualLine) {
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

    fn visual_repeat(&self, text: &str, range: &Range<usize>) -> VisualRepeat {
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

    pub fn motion_epoch(&self) -> u64 {
        self.motion_epoch
    }

    pub fn search_operator_pending(&self) -> bool {
        matches!(self.pending, Some(Pending::Operator(_, _, _)))
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

    /// The insert-session snapshot also covers edits performed by the widget.
    pub fn notify_external_text_change(&mut self) {}

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
        let inserting = old_mode == VimMode::Insert;
        let mut insert_expanded = false;
        if inserting {
            let inserted_delta = self
                .insert_replay
                .as_ref()
                .map(|replay| insert_delta(&replay.baseline, text, replay.cursor));
            insert_expanded = self.finish_insert_repetitions(text);
            let changed = self
                .insert_origin
                .as_ref()
                .is_some_and(|origin| origin.text != *text);
            if let Some(origin) = self.insert_origin.take()
                && changed
            {
                self.record(origin);
            }
            if let Some(replay) = self.insert_replay.take()
                && changed
            {
                self.last_change = Some(RepeatChange {
                    keys: replay.keys,
                    insert: inserted_delta,
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
            self.start_insert(text, self.cursor);
            self.insert_replay = Some(InsertReplay {
                keys: vec![VimKey::Char('i')],
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

    pub fn handle_key(&mut self, text: &mut String, key: VimKey) -> Action {
        self.yanked_ranges.clear();
        if matches!(self.mode, VimMode::Visual | VimMode::VisualLine)
            && self.pending.is_none()
            && matches!(key, VimKey::Char('d' | 'x'))
        {
            return self.cut_visual(text);
        }
        let old_cursor = self.cursor;
        let old_mode = self.mode;
        let old_count = self.count;
        let visual_change = self.selection(text).and_then(|range| {
            let keys = match (self.pending, key) {
                (Some(Pending::Replace(_)), VimKey::Char(_) | VimKey::Enter | VimKey::Tab) => {
                    vec![VimKey::Char('r'), key]
                }
                (None, VimKey::Char('c' | 's' | '>' | '<' | 'p' | 'P' | 'J' | 'u' | 'U' | '~')) => {
                    let mut keys = Vec::new();
                    if old_count > 0 {
                        keys.extend(old_count.to_string().chars().map(VimKey::Char));
                    }
                    keys.push(key);
                    keys
                }
                (Some(Pending::Go { operator: None, .. }), VimKey::Char('u' | 'U' | '~')) => {
                    vec![VimKey::Char('g'), key]
                }
                (Some(Pending::Go { operator: None, .. }), VimKey::Char('J')) => {
                    vec![VimKey::Char('g'), key]
                }
                (Some(Pending::RegisterPaste(prefix)), VimKey::Char('p' | 'P')) => {
                    let count = prefix.saturating_mul(old_count.max(1)).min(MAX_COUNT);
                    let mut keys: Vec<_> = count.to_string().chars().map(VimKey::Char).collect();
                    keys.extend([VimKey::Char('"'), VimKey::Char('0'), key]);
                    keys
                }
                (Some(Pending::SurroundAdd(..)), VimKey::Char(_)) => {
                    vec![VimKey::Char('S'), key]
                }
                _ => return None,
            };
            Some((self.visual_repeat(text, &range), keys))
        });
        let insert_changed = old_mode == VimMode::Insert
            && self
                .insert_origin
                .as_ref()
                .is_some_and(|origin| origin.text != *text);
        let inserted_delta = if old_mode == VimMode::Insert && key == VimKey::Escape {
            self.insert_replay
                .as_ref()
                .map(|replay| insert_delta(&replay.baseline, text, replay.cursor))
        } else {
            None
        };
        let history_command = key == VimKey::Redo
            || (key == VimKey::Char('u') && old_mode == VimMode::Normal && self.pending.is_none());
        let dot_command =
            key == VimKey::Char('.') && old_mode == VimMode::Normal && self.pending.is_none();
        if !self.replaying && old_mode != VimMode::Insert && !history_command && !dot_command {
            // A command itself is short even when the supplied count overflows.
            if self.command_keys.len() < 64 {
                self.command_keys.push(key);
            }
        }
        self.clamp_cursor(text);
        let (consumed, text_changed, yanked) = match self.mode {
            VimMode::Insert => self.handle_insert_key(text, key),
            VimMode::Normal | VimMode::Visual | VimMode::VisualLine => {
                self.handle_normal_key(text, key)
            }
        };
        self.clamp_cursor(text);
        if !self.replaying {
            if old_mode != VimMode::Insert && self.mode == VimMode::Insert {
                let (visual, keys) = if let Some((visual, keys)) = visual_change {
                    self.command_keys.clear();
                    (Some(visual), keys)
                } else {
                    (None, std::mem::take(&mut self.command_keys))
                };
                self.insert_replay = Some(InsertReplay {
                    keys,
                    baseline: text.clone(),
                    cursor: self.cursor,
                    search: None,
                    visual,
                });
            } else if old_mode == VimMode::Insert && self.mode != VimMode::Insert {
                if let Some(replay) = self.insert_replay.take()
                    && insert_changed
                {
                    self.last_change = Some(RepeatChange {
                        keys: replay.keys,
                        insert: inserted_delta
                            .or_else(|| Some(insert_delta(&replay.baseline, text, replay.cursor))),
                        search: replay.search,
                        visual: replay.visual,
                    });
                }
            } else if text_changed && !history_command && !dot_command {
                let (visual, keys) = if let Some((visual, keys)) = visual_change {
                    self.command_keys.clear();
                    (Some(visual), keys)
                } else {
                    (None, std::mem::take(&mut self.command_keys))
                };
                self.last_change = Some(RepeatChange {
                    keys,
                    insert: None,
                    search: self.surround_search.take(),
                    visual,
                });
            }
            if self.mode == VimMode::Normal && self.pending.is_none() && self.count == 0 {
                self.command_keys.clear();
                self.surround_search = None;
            }
        }
        if self.cursor != old_cursor
            || text_changed
            || matches!(
                key,
                VimKey::Char(
                    'h' | 'l'
                        | 'j'
                        | 'k'
                        | 'w'
                        | 'b'
                        | 'e'
                        | 'W'
                        | 'B'
                        | 'E'
                        | '^'
                        | '$'
                        | 'G'
                        | '%'
                        | '{'
                        | '}'
                        | ';'
                        | ','
                        | '+'
                        | '-'
                        | '_'
                        | '|'
                        | 'H'
                        | 'M'
                        | 'L'
                ) | VimKey::Enter
                    | VimKey::Backspace
            )
            || key == VimKey::Char('0') && old_count == 0
        {
            self.motion_epoch = self.motion_epoch.wrapping_add(1);
        }
        Action {
            consumed,
            text_changed,
            cursor_changed: self.cursor != old_cursor,
            mode_changed: self.mode != old_mode,
            yanked,
            yanked_ranges: std::mem::take(&mut self.yanked_ranges),
            scroll: self.scroll_request.take(),
            message: self.message.take(),
        }
    }

    pub fn insert_text(&mut self, text: &mut String, inserted: &str) -> Action {
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

    fn clamp_cursor(&mut self, text: &str) {
        self.cursor = if self.mode != VimMode::Normal {
            char_boundary(text, self.cursor).0
        } else {
            normal_cursor(text, self.cursor)
        };
    }

    fn handle_insert_key(&mut self, text: &mut String, key: VimKey) -> (bool, bool, Option<Yank>) {
        match key {
            VimKey::Escape => {
                let changed = self.finish_insert_repetitions(text);
                let chars: Vec<char> = text.chars().collect();
                let rows = lines(&chars);
                if self.cursor > rows[row_for_cursor(&rows, self.cursor)].start {
                    self.cursor -= 1;
                }
                if let Some(before) = self.insert_origin.take()
                    && before.text != *text
                {
                    self.record(before);
                }
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
            VimKey::Redo | VimKey::Increment | VimKey::Decrement => (false, false, None),
        }
    }

    /// Finalize a counted insertion once for Escape and Windows history alike.
    /// The repeat delta is captured by callers before this expands the text.
    fn finish_insert_repetitions(&mut self, text: &mut String) -> bool {
        let mut changed = false;
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

    fn handle_normal_key(&mut self, text: &mut String, key: VimKey) -> (bool, bool, Option<Yank>) {
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
                        let count = if matches!(self.mode, VimMode::Visual | VimMode::VisualLine) {
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

        if matches!(self.mode, VimMode::Visual | VimMode::VisualLine) {
            match ch {
                'y' | 'd' | 'x' | 'c' | 's' | '>' | '<' | 'u' | 'U' | '~' => {
                    return self.apply_visual(text, ch, count);
                }
                'p' | 'P' => return self.paste_visual(text, count, ch == 'P'),
                'S' => {
                    if let Some(range) = self.selection(text) {
                        self.pending = Some(Pending::SurroundAdd(range.start, range.end));
                    }
                    return (true, false, None);
                }
                'o' => {
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
            'h' | 'j' | 'k' | 'l' | 'w' | 'W' | 'b' | 'B' | 'e' | 'E' | '0' | '^' | '$' | 'G'
            | '%' | '{' | '}' | '+' | '-' | '_' | '|' | 'H' | 'M' | 'L' => {
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

    fn unsupported(&mut self, command: &str) {
        self.message = Some(format!("Not supported in vEdit: {command}"));
    }

    /// `gv`: select the previous Visual area again. In Visual mode the current
    /// and the previous area are exchanged.
    fn reselect_visual(&mut self, text: &str) {
        let Some(last) = self.last_visual else {
            return;
        };
        let current = self.visual_anchor.and_then(|anchor| {
            matches!(self.mode, VimMode::Visual | VimMode::VisualLine).then_some(LastVisual {
                mode: self.mode,
                anchor,
                cursor: self.cursor,
                to_line_end: self.visual_to_line_end,
            })
        });
        let len = text.chars().count();
        self.mode = last.mode;
        self.visual_anchor = Some(last.anchor.min(len));
        self.cursor = last.cursor.min(len);
        self.visual_to_line_end = last.to_line_end;
        self.preferred_column = None;
        if current.is_some() {
            self.last_visual = current;
        }
    }

    /// Ctrl+A / Ctrl+X: change the decimal number under or after the cursor on
    /// the current line. A directly preceding `-` makes it negative; leading
    /// zeros keep their width.
    fn add_to_number(&mut self, text: &mut String, count: usize, increment: bool) -> bool {
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

    fn take_count(&mut self) -> usize {
        let count = self.count.max(1);
        self.count = 0;
        count
    }
    fn clear_command(&mut self) {
        self.pending = None;
        self.count = 0;
        self.preferred_column = None;
    }
    fn leave_visual(&mut self) {
        if matches!(self.mode, VimMode::Visual | VimMode::VisualLine)
            && let Some(anchor) = self.visual_anchor
        {
            self.last_visual = Some(LastVisual {
                mode: self.mode,
                anchor,
                cursor: self.cursor,
                to_line_end: self.visual_to_line_end,
            });
        }
        self.visual_anchor = None;
        self.visual_to_line_end = false;
        if matches!(self.mode, VimMode::Visual | VimMode::VisualLine) {
            self.mode = VimMode::Normal;
        }
    }
    fn snapshot(&self, text: &str) -> Snapshot {
        Snapshot {
            text: text.to_owned(),
            cursor: self.cursor,
        }
    }
    fn record(&mut self, before: Snapshot) {
        if self.replaying {
            return;
        }
        self.redo.clear();
        self.undo.push(before);
        trim_history(&mut self.undo);
    }

    fn start_insert(&mut self, text: &str, cursor: usize) {
        self.insert_origin = Some(self.snapshot(text));
        self.mode = VimMode::Insert;
        self.cursor = cursor.min(text.chars().count());
        self.visual_anchor = None;
        self.insert_count = 1;
        self.insert_line_prefix = None;
        self.clear_command();
    }

    fn open_line(&mut self, text: &mut String, above: bool) {
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

    fn move_cursor(&mut self, chars: &[char], key: char, count: usize, explicit: bool) {
        if matches!(self.mode, VimMode::Visual | VimMode::VisualLine) && !matches!(key, 'j' | 'k') {
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
            self.cursor = target;
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

    fn apply_operator_motion(
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

    /// The line H, M or L selects in the visible range, never the empty row
    /// after a final line break.
    fn screen_row(&self, chars: &[char], rows: &[Line], key: char, count: usize) -> usize {
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

    fn apply_lines(
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

    fn apply_range(
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

    fn apply_visual(
        &mut self,
        text: &mut String,
        key: char,
        count: usize,
    ) -> (bool, bool, Option<Yank>) {
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

    fn add_surround(&mut self, text: &mut String, range: Range<usize>, desired: char) -> bool {
        let bytes = char_to_byte(text, range.start)..char_to_byte(text, range.end);
        let Some(replacement) = surrounded(&text[bytes.clone()], desired) else {
            return false;
        };
        self.replace_surround(text, range.start, bytes, &replacement)
    }

    fn change_surround(
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

    fn paste_visual(
        &mut self,
        text: &mut String,
        count: usize,
        preserve_register: bool,
    ) -> (bool, bool, Option<Yank>) {
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

    fn shift_lines(
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
        if right {
            let width = if self.indent_with_tabs {
                count
            } else {
                self.indent_width.saturating_mul(count)
            };
            let affected = rows[first..=last]
                .iter()
                .filter(|line| line.start != line.end)
                .count();
            if width
                .checked_mul(affected)
                .and_then(|added| text.len().checked_add(added))
                .is_none_or(|size| size > MAX_PASTE_BYTES)
            {
                return false;
            }
        }
        for line in rows[first..=last].iter().rev() {
            if right {
                if line.start == line.end {
                    continue;
                }
                text.insert_str(
                    char_to_byte(text, line.start),
                    &if self.indent_with_tabs {
                        "\t".repeat(count)
                    } else {
                        " ".repeat(self.indent_width.saturating_mul(count))
                    },
                );
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
                text.replace_range(char_to_byte(text, line.start)..char_to_byte(text, at), "");
            }
        }
        if *text == before.text {
            return false;
        }
        let chars: Vec<char> = text.chars().collect();
        let rows = lines(&chars);
        self.cursor = first_nonblank(&chars, rows[first]);
        self.record(before);
        self.preferred_column = None;
        true
    }

    fn paste(&mut self, text: &mut String, before_cursor: bool, count: usize) -> bool {
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

    fn replace_characters(&mut self, text: &mut String, count: usize, replacement: char) -> bool {
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

    fn find(
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

    fn join_lines(&mut self, text: &mut String, count: usize, spaces: bool) -> bool {
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

    fn restore(&mut self, text: &mut String, redo: bool) -> bool {
        let source = if redo { &mut self.redo } else { &mut self.undo };
        let Some(snapshot) = source.pop() else {
            return false;
        };
        let current = self.snapshot(text);
        let target = if redo { &mut self.undo } else { &mut self.redo };
        target.push(current);
        trim_history(target);
        *text = snapshot.text;
        self.cursor = snapshot.cursor;
        self.insert_origin = None;
        self.clear_command();
        true
    }

    fn repeat_change(
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
            let motion_offset = match change.keys.as_slice() {
                [VimKey::Char('y'), VimKey::Char('s'), ..]
                | [VimKey::Char('"'), VimKey::Char('0'), ..] => Some(2),
                [VimKey::Char('g'), VimKey::Char('u' | 'U' | '~'), ..] => Some(2),
                [VimKey::Char('d' | 'c' | 'y' | '>' | '<'), ..] => Some(1),
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
            self.record(before);
            self.last_change = Some(change);
        }
        (true, changed, yank)
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

fn surround_range(
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
fn char_to_byte(text: &str, cursor: usize) -> usize {
    text.char_indices()
        .nth(cursor)
        .map_or(text.len(), |(index, _)| index)
}

fn char_boundary(text: &str, cursor: usize) -> (usize, usize) {
    let mut count = 0;
    for (position, (byte, _)) in text.char_indices().enumerate() {
        if position == cursor {
            return (position, byte);
        }
        count = position + 1;
    }
    (count, text.len())
}
fn repeated_text(text: &str, count: usize) -> Option<String> {
    if text.len().checked_mul(count)? > MAX_PASTE_BYTES {
        return None;
    }
    Some(text.repeat(count))
}
fn insert_delta(before: &str, after: &str, cursor: usize) -> InsertDelta {
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
fn word_class(ch: char, big: bool) -> u8 {
    if ch.is_whitespace() {
        0
    } else if big || ch.is_alphanumeric() || ch == '_' {
        1
    } else {
        2
    }
}

/// Whether an empty line, LF or CRLF, starts at this index.
fn empty_line_at(chars: &[char], index: usize) -> bool {
    (index == 0 || chars[index - 1] == '\n')
        && match chars.get(index) {
            Some('\n') => true,
            Some('\r') => chars.get(index + 1) == Some(&'\n'),
            _ => false,
        }
}

fn word_forward(chars: &[char], cursor: usize, big: bool) -> usize {
    let mut at = cursor.min(chars.len());
    if at == chars.len() {
        return at;
    }
    let empty_line_at = |index: usize| empty_line_at(chars, index);
    if empty_line_at(at) {
        at += 1;
    } else {
        let class = word_class(chars[at], big);
        while at < chars.len() && word_class(chars[at], big) == class && !empty_line_at(at) {
            at += 1;
        }
    }
    while at < chars.len() && chars[at].is_whitespace() {
        if empty_line_at(at) {
            break;
        }
        at += 1;
    }
    at
}
fn word_backward(chars: &[char], cursor: usize, big: bool) -> usize {
    if chars.is_empty() || cursor == 0 {
        return 0;
    }
    let mut at = cursor.min(chars.len()).saturating_sub(1);
    let empty_line_at = |index: usize| empty_line_at(chars, index);
    while at > 0 && chars[at].is_whitespace() {
        if empty_line_at(at) {
            return at;
        }
        at -= 1;
    }
    let class = word_class(chars[at], big);
    while at > 0 && word_class(chars[at - 1], big) == class {
        at -= 1;
    }
    at
}
fn word_end(chars: &[char], cursor: usize, big: bool) -> usize {
    if chars.is_empty() {
        return 0;
    }
    let mut at = cursor.min(chars.len() - 1);
    if at + 1 >= chars.len() {
        return at;
    }
    let class = word_class(chars[at], big);
    at += 1;
    if class == 0 || word_class(chars[at], big) != class {
        while at < chars.len() && chars[at].is_whitespace() {
            at += 1;
        }
    }
    if at == chars.len() {
        return chars.len() - 1;
    }
    let class = word_class(chars[at], big);
    while at + 1 < chars.len() && word_class(chars[at + 1], big) == class {
        at += 1;
    }
    at
}
fn word_end_backward(chars: &[char], cursor: usize, big: bool) -> usize {
    if chars.is_empty() || cursor == 0 {
        return 0;
    }
    let mut at = cursor.min(chars.len()).saturating_sub(1);
    // From inside a word, ge skips the current word before looking for the
    // preceding end. At its start or on whitespace, the previous end is next.
    if cursor < chars.len() && word_class(chars[cursor], big) != 0 {
        let class = word_class(chars[cursor], big);
        while word_class(chars[at], big) == class {
            if at == 0 {
                return 0;
            }
            at -= 1;
        }
    }
    while at > 0 && chars[at].is_whitespace() {
        at -= 1;
    }
    at
}
fn change_word_end(chars: &[char], cursor: usize, big: bool) -> usize {
    if chars.is_empty() {
        return 0;
    }
    let mut at = cursor.min(chars.len() - 1);
    let class = word_class(chars[at], big);
    while at + 1 < chars.len() && word_class(chars[at + 1], big) == class {
        at += 1;
    }
    at
}
fn repeat_motion(
    chars: &[char],
    cursor: usize,
    count: usize,
    big: bool,
    motion: fn(&[char], usize, bool) -> usize,
) -> usize {
    let mut at = cursor;
    for _ in 0..count {
        let next = motion(chars, at, big);
        if next == at {
            break;
        }
        at = next;
    }
    at
}
fn text_object(
    chars: &[char],
    cursor: usize,
    object: char,
    count: usize,
    around: bool,
) -> Option<Range<usize>> {
    if chars.is_empty() {
        return None;
    }
    let cursor = cursor.min(chars.len() - 1);
    if matches!(object, 'w' | 'W') {
        // Words, blanks and their counts stay on the cursor's line. An empty
        // line has no word to select.
        let line = line_at_cursor(chars, cursor);
        if line.start == line.end {
            return None;
        }
        let cursor = cursor.clamp(line.start, line.end - 1);
        let big = object == 'W';
        let class = |index: usize| word_class(chars[index], big);
        let run_end = |from: usize| {
            let kind = class(from);
            let mut end = from;
            while end < line.end && class(end) == kind {
                end += 1;
            }
            end
        };
        let mut start = cursor;
        while start > line.start && class(start - 1) == class(cursor) {
            start -= 1;
        }
        let mut end = run_end(cursor);
        if around {
            // On whitespace, "a word" includes the following word. Otherwise
            // each count adds another word and the whitespace between them.
            let words = if class(cursor) == 0 {
                count
            } else {
                count.saturating_sub(1)
            };
            for _ in 0..words {
                while end < line.end && class(end) == 0 {
                    end += 1;
                }
                if end == line.end {
                    break;
                }
                end = run_end(end);
            }
            if class(cursor) != 0 {
                let content_end = end;
                while end < line.end && class(end) == 0 {
                    end += 1;
                }
                // Without trailing blanks, take the leading ones instead.
                if end == content_end {
                    while start > line.start && class(start - 1) == 0 {
                        start -= 1;
                    }
                }
            }
        } else {
            // Inner-word counts include whitespace as its own text object.
            for _ in 1..count {
                if end == line.end {
                    break;
                }
                end = run_end(end);
            }
        }
        return Some(start..end);
    }
    if matches!(object, '\'' | '"' | '`') {
        let rows = lines(chars);
        let line = rows[row_for_cursor(&rows, cursor)];
        let quotes: Vec<usize> = (line.start..line.end)
            .filter(|index| {
                if chars[*index] != object {
                    return false;
                }
                let backslashes = chars[line.start..*index]
                    .iter()
                    .rev()
                    .take_while(|ch| **ch == '\\')
                    .count();
                backslashes % 2 == 0
            })
            .collect();
        let preceding = quotes.iter().rposition(|index| *index <= cursor);
        let opening = match preceding {
            Some(index) if quotes[index] == cursor && index % 2 == 1 => index - 1,
            Some(index) => index,
            None => 0,
        };
        let pair = (*quotes.get(opening)?, *quotes.get(opening + 1)?);
        let mut range = if around || count > 1 {
            pair.0..pair.1 + 1
        } else {
            pair.0 + 1..pair.1
        };
        if around {
            let content_end = range.end;
            while range.end < line.end && matches!(chars[range.end], ' ' | '\t') {
                range.end += 1;
            }
            if range.end == content_end {
                while range.start > line.start && matches!(chars[range.start - 1], ' ' | '\t') {
                    range.start -= 1;
                }
            }
        }
        return Some(range);
    }
    let (open, close) = match object {
        '(' | ')' | 'b' => ('(', ')'),
        '[' | ']' => ('[', ']'),
        '{' | '}' | 'B' => ('{', '}'),
        '<' | '>' => ('<', '>'),
        _ => return None,
    };
    let mut pairs = Vec::new();
    let mut stack = Vec::new();
    for (index, ch) in chars.iter().enumerate() {
        if *ch == open {
            stack.push(index);
        } else if *ch == close
            && let Some(start) = stack.pop()
        {
            pairs.push((start, index));
        }
    }
    // Closing delimiters encounter enclosing pairs from the inside out. From
    // outside a block, Vim searches forward by opening delimiter instead.
    let enclosing: Vec<_> = pairs
        .iter()
        .copied()
        .filter(|pair| pair.0 <= cursor && cursor <= pair.1)
        .collect();
    let pair = if enclosing.is_empty() {
        pairs.sort_unstable_by_key(|pair| pair.0);
        let mut pair = *pairs.iter().find(|pair| pair.0 > cursor)?;
        for _ in 1..count {
            pair = *pairs
                .iter()
                .find(|nested| pair.0 < nested.0 && nested.1 < pair.1)?;
        }
        pair
    } else {
        *enclosing.get(count.saturating_sub(1))?
    };
    Some(if around {
        pair.0..pair.1 + 1
    } else {
        inner_block(chars, pair)
    })
}

/// The inside of a delimiter pair. As in Vim, an opening delimiter that ends
/// its line starts the inner text on the next line, and a closing delimiter
/// preceded only by indentation leaves its own line out, so a block on its own
/// lines yields whole lines.
fn inner_block(chars: &[char], (open, close): (usize, usize)) -> Range<usize> {
    let mut start = open + 1;
    if chars.get(start) == Some(&'\n') {
        start += 1;
    } else if chars.get(start) == Some(&'\r') && chars.get(start + 1) == Some(&'\n') {
        start += 2;
    }
    let line_start = chars[..close]
        .iter()
        .rposition(|ch| *ch == '\n')
        .map_or(0, |at| at + 1);
    let end = if line_start > open
        && chars[line_start..close]
            .iter()
            .all(|ch| matches!(ch, ' ' | '\t'))
    {
        line_start
    } else {
        close
    };
    start.min(end)..end
}

/// `ip` / `ap` as whole rows. Blank lines, also those containing only
/// whitespace, form blocks of their own; `ap` adds the following blank lines,
/// or the preceding ones when none follow.
fn paragraph_object(chars: &[char], cursor: usize, count: usize, around: bool) -> (usize, usize) {
    let rows = lines(chars);
    let last = last_line_row(&rows);
    let blank = |row: usize| {
        chars[rows[row].start..rows[row].end]
            .iter()
            .all(|ch| ch.is_whitespace())
    };
    let run_end = |from: usize| {
        let kind = blank(from);
        let mut end = from;
        while end < last && blank(end + 1) == kind {
            end += 1;
        }
        end
    };
    let row = row_for_cursor(&rows, cursor).min(last);
    let mut first = row;
    while first > 0 && blank(first - 1) == blank(row) {
        first -= 1;
    }
    let mut end = run_end(row);
    let runs = if around {
        count.saturating_mul(2)
    } else {
        count
    };
    let mut taken = 1;
    while taken < runs && end < last {
        end = run_end(end + 1);
        taken += 1;
    }
    if around && !blank(row) && !blank(end) {
        while first > 0 && blank(first - 1) {
            first -= 1;
        }
    }
    (first, end)
}
fn line_ending(text: &str) -> &'static str {
    if text.contains("\r\n") { "\r\n" } else { "\n" }
}

fn remove_last_line_ending(text: &mut String) {
    if text.ends_with("\r\n") {
        text.truncate(text.len() - 2);
    } else if text.ends_with('\n') {
        text.pop();
    }
}

fn lines(chars: &[char]) -> Vec<Line> {
    let mut result = Vec::new();
    let mut start = 0;
    for (index, ch) in chars.iter().enumerate() {
        if *ch == '\n' {
            result.push(Line {
                start,
                end: index - usize::from(index > start && chars[index - 1] == '\r'),
                newline: Some(index),
            });
            start = index + 1;
        }
    }
    result.push(Line {
        start,
        end: chars.len(),
        newline: None,
    });
    result
}
fn row_for_cursor(rows: &[Line], cursor: usize) -> usize {
    rows.partition_point(|line| line.start <= cursor)
        .saturating_sub(1)
}

fn line_at_cursor(chars: &[char], cursor: usize) -> Line {
    let cursor = cursor.min(chars.len());
    let start = chars[..cursor]
        .iter()
        .rposition(|&ch| ch == '\n')
        .map_or(0, |at| at + 1);
    let newline = chars[cursor..]
        .iter()
        .position(|&ch| ch == '\n')
        .map(|at| cursor + at);
    let end = newline.map_or(chars.len(), |at| {
        at - usize::from(at > start && chars[at - 1] == '\r')
    });
    Line {
        start,
        end,
        newline,
    }
}

fn relative_line(chars: &[char], mut line: Line, count: usize, down: bool) -> Line {
    for _ in 0..count {
        let next = if down {
            line.newline.map(|at| at + 1)
        } else {
            line.start.checked_sub(1)
        };
        let Some(next) = next else { break };
        line = line_at_cursor(chars, next);
    }
    line
}
fn first_nonblank(chars: &[char], line: Line) -> usize {
    let mut at = line.start;
    while at < line.end && (chars[at] == ' ' || chars[at] == '\t') {
        at += 1;
    }
    at.min(line.end.saturating_sub(1).max(line.start))
}

fn last_nonblank(chars: &[char], line: Line) -> usize {
    chars[line.start..line.end]
        .iter()
        .rposition(|ch| !ch.is_whitespace())
        .map_or(line.start, |column| line.start + column)
}
fn normal_cursor(text: &str, cursor: usize) -> usize {
    let (mut cursor, byte) = char_boundary(text, cursor);
    let mut prefix = &text[..byte];
    let suffix = &text[byte..];
    match suffix.chars().next() {
        Some('\n') => {
            if prefix.ends_with('\r') {
                prefix = &prefix[..prefix.len() - 1];
                cursor -= 1;
            }
        }
        Some('\r') if suffix.starts_with("\r\n") => {}
        None => {}
        _ => return cursor,
    }
    // Only a line ending or EOF needs moving back. Empty lines keep their
    // boundary, including CRLF and the empty row after a final newline.
    if !prefix.is_empty() && !prefix.ends_with('\n') {
        cursor -= 1;
    }
    cursor
}
fn matching_bracket(chars: &[char], cursor: usize) -> Option<usize> {
    let rows = lines(chars);
    let line = rows[row_for_cursor(&rows, cursor)];
    let at = (cursor..line.end).find(|index| is_bracket(chars[*index]))?;
    bracket_partner(chars, at, chars.len())
}

fn is_bracket(ch: char) -> bool {
    matches!(ch, '(' | ')' | '[' | ']' | '{' | '}')
}

/// The bracket that pairs with the one at `at`, looking at most `limit`
/// characters away.
fn bracket_partner(chars: &[char], at: usize, limit: usize) -> Option<usize> {
    let (open, close, forward) = match chars[at] {
        '(' => ('(', ')', true),
        ')' => ('(', ')', false),
        '[' => ('[', ']', true),
        ']' => ('[', ']', false),
        '{' => ('{', '}', true),
        '}' => ('{', '}', false),
        _ => return None,
    };
    let mut depth: usize = 1;
    if forward {
        for (index, ch) in chars.iter().enumerate().skip(at + 1).take(limit) {
            if *ch == open {
                depth += 1;
            } else if *ch == close {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
        }
    } else {
        for index in (at.saturating_sub(limit)..at).rev() {
            if chars[index] == close {
                depth += 1;
            } else if chars[index] == open {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
        }
    }
    None
}

/// The bracket at the caret and its partner, for highlighting: the bracket
/// under the cursor, else, for a caret between characters as in Insert mode,
/// the one directly before it. `text` is a window of the document starting
/// at character `offset`; the search stays inside it.
pub fn bracket_pair(
    text: &[char],
    offset: usize,
    cursor: usize,
    between: bool,
) -> Option<(usize, usize)> {
    let local = cursor.checked_sub(offset)?;
    let before = local.checked_sub(1).filter(|_| between);
    let at = [Some(local), before]
        .into_iter()
        .flatten()
        .find(|index| text.get(*index).copied().is_some_and(is_bracket))?;
    bracket_partner(text, at, text.len()).map(|partner| (offset + at, offset + partner))
}
/// `{` / `}`: the next empty line after at least one nonempty line, counting
/// the cursor's own line. Without one, the motion ends at the start or end of
/// the document.
fn paragraph(chars: &[char], cursor: usize, count: usize, forward: bool) -> usize {
    let rows = lines(chars);
    let empty = |row: usize| rows[row].start == rows[row].end;
    let mut row = row_for_cursor(&rows, cursor);
    for _ in 0..count {
        let mut content = !empty(row);
        loop {
            let next = if forward {
                Some(row + 1).filter(|next| *next < rows.len())
            } else {
                row.checked_sub(1)
            };
            let Some(next) = next else {
                return if forward { chars.len() } else { 0 };
            };
            row = next;
            if content && empty(row) {
                break;
            }
            content |= !empty(row);
        }
    }
    rows[row].start
}

/// The last line in Vim's model, where a final line break terminates the
/// last line instead of starting an empty one.
fn last_line_row(rows: &[Line]) -> usize {
    let last = rows.len() - 1;
    if last > 0 && rows[last].start == rows[last].end {
        last - 1
    } else {
        last
    }
}

fn last_line(chars: &[char]) -> Line {
    line_at_cursor(
        chars,
        chars.len() - usize::from(chars.last() == Some(&'\n')),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_clamping_handles_unicode_crlf_empty_lines_and_stale_positions() {
        for (text, expected) in [
            ("", vec![0]),
            ("α🦀", vec![0, 1, 1]),
            ("α\r\n\r\nβ\n", vec![0, 0, 0, 3, 3, 5, 5, 7]),
            ("\r\n", vec![0, 0, 2]),
            ("α\rβ\r", vec![0, 1, 2, 3, 3]),
        ] {
            for (cursor, &clamped) in expected.iter().enumerate() {
                assert_eq!(normal_cursor(text, cursor), clamped, "{text:?}, {cursor}");
            }
            assert_eq!(normal_cursor(text, usize::MAX), *expected.last().unwrap());
        }
    }

    #[test]
    fn nearby_line_motions_preserve_crlf_unicode_and_empty_rows() {
        let mut text = "α🦀\r\n\r\n  βγ\r\nlast\r\n".to_owned();
        let mut state = VimState::new();
        keys(&mut state, &mut text, "lj");
        assert_eq!(state.cursor, 4);
        keys(&mut state, &mut text, "j");
        assert_eq!(state.cursor, 7);
        keys(&mut state, &mut text, "2k");
        assert_eq!(state.cursor, 1);
        keys(&mut state, &mut text, "2+");
        assert_eq!(state.cursor, 8);
        keys(&mut state, &mut text, "2$");
        assert_eq!(state.cursor, 15);
        // The final CRLF terminates "last"; G does not enter the empty row after it.
        keys(&mut state, &mut text, "G");
        assert_eq!(state.cursor, 12);
        keys(&mut state, &mut text, "2G");
        assert_eq!(state.cursor, 4);
    }

    #[test]
    fn insert_replay_delta_retains_scalar_boundaries_and_cursor_limited_prefix() {
        for (before, after, cursor, start, removed, inserted) in [
            ("α🦀\r\nβ", "α中\r\nβ", 1, 0, 1, "中"),
            ("α🦀", "α🦀🦀", 1, 0, 0, "🦀"),
            ("α\r\nβ", "αβ", 3, -2, 2, ""),
            ("", "中", 0, 0, 0, "中"),
            ("α🦀", "α🦀", 0, 0, 0, ""),
        ] {
            let delta = insert_delta(before, after, cursor);
            assert_eq!(delta.relative_start, start);
            assert_eq!(delta.removed, removed);
            assert_eq!(delta.inserted, inserted);
        }
    }

    #[test]
    fn yank_feedback_reports_unicode_source_ranges_and_real_newlines() {
        let mut text = "α🦀 one\nlast".to_owned();
        let mut state = VimState::new();
        keys(&mut state, &mut text, "vl");
        let action = state.handle_key(&mut text, VimKey::Char('y'));
        assert_eq!(action.yanked.as_ref().unwrap().text, "α🦀");
        assert_eq!(action.yanked_ranges.len(), 1);
        assert_eq!(action.yanked_ranges[0], 0..2);
        assert!(!action.text_changed);
        assert!(
            state
                .handle_key(&mut text, VimKey::Char('j'))
                .yanked_ranges
                .is_empty()
        );

        keys(&mut state, &mut text, "y");
        let action = state.handle_key(&mut text, VimKey::Char('y'));
        assert_eq!(action.yanked.as_ref().unwrap().text, "last\n");
        assert_eq!(action.yanked_ranges.len(), 1);
        assert_eq!(action.yanked_ranges[0], 7..11);
        keys(&mut state, &mut text, "ggV");
        let action = state.handle_key(&mut text, VimKey::Char('y'));
        assert_eq!(action.yanked_ranges.len(), 1);
        assert_eq!(action.yanked_ranges[0], 0..7);
    }

    #[test]
    fn clipboard_visual_cut_preserves_register_and_geometry_for_undo_and_dot() {
        for register in [
            None,
            Some(Yank {
                text: "KEEP🦀".into(),
                linewise: false,
            }),
        ] {
            let mut text = "α🦀βabcdef".to_owned();
            let mut state = VimState::new();
            state.register = register.clone();
            state.set_selection(&text, 0..3);
            keys(&mut state, &mut text, "2r");
            let (action, copied) = state.cut_visual_to_clipboard(&mut text);
            assert_eq!(copied.as_deref(), Some("α🦀β"));
            assert!(action.text_changed && action.yanked.is_none());
            assert!(action.yanked_ranges.is_empty());
            assert_eq!(state.register, register);
            assert_eq!(text, "abcdef");
            keys(&mut state, &mut text, ".");
            assert_eq!(text, "def");
            assert_eq!(state.register.as_ref().unwrap().text, "abc");
            keys(&mut state, &mut text, "uu");
            assert_eq!(text, "α🦀βabcdef");
        }
    }

    #[test]
    fn native_clipboard_ranges_use_unicode_selection_or_the_complete_source_line() {
        let text = "α🦀\nβγ";
        assert_eq!(clipboard_range(text, 1..2, 0), 1..2);
        assert_eq!(clipboard_range(text, 1..1, 1), 0..3);
        assert_eq!(clipboard_range(text, 4..4, 4), 3..5);
        assert_eq!(clipboard_range(text, 99..99, 99), 3..5);
        assert_eq!(clipboard_range("", 0..0, 0), 0..0);
        assert_eq!(clipboard_range("α\n", 2..2, 2), 2..2);
    }

    #[test]
    fn native_visual_cut_discards_unfinished_prefixes_and_repeats_completed_area() {
        for prefix in ["", "r", "i", "a", "f", "g", "23", "2f", "g2"] {
            let original = "ä🙂中defghi";
            let mut text = original.to_owned();
            let mut state = VimState::new();
            keys(&mut state, &mut text, &format!("vll{prefix}"));
            assert_eq!(state.selection(&text), Some(0..3), "{prefix}");
            let action = state.cut_visual(&mut text);
            assert_eq!(text, "defghi", "{prefix}");
            assert_eq!(
                action.yanked,
                Some(Yank {
                    text: "ä🙂中".into(),
                    linewise: false
                })
            );
            assert!(action.text_changed && action.mode_changed);
            assert_eq!(state.pending_command(), "");
            keys(&mut state, &mut text, ".w");
            assert_eq!(text, "ghi", "{prefix}");
            assert_eq!(state.pending_command(), "");
            assert_eq!(state.register().unwrap().text, "def");
            keys(&mut state, &mut text, "u");
            assert_eq!(text, "defghi");
            keys(&mut state, &mut text, "u");
            assert_eq!(text, original);
        }
    }

    #[test]
    fn native_linewise_cut_ignores_pending_counts_and_dot_counts() {
        let original = "one\ntwo\nthree\nfour\nfive\nsix\n";
        let mut text = original.to_owned();
        let mut state = VimState::new();
        keys(&mut state, &mut text, "Vj23r");
        let action = state.cut_visual(&mut text);
        assert_eq!(text, "three\nfour\nfive\nsix\n");
        assert_eq!(
            action.yanked,
            Some(Yank {
                text: "one\ntwo\n".into(),
                linewise: true
            })
        );
        keys(&mut state, &mut text, "9.");
        assert_eq!(text, "five\nsix\n");
        assert_eq!(state.register().unwrap().text, "three\nfour\n");
        assert!(state.register().unwrap().linewise);
        keys(&mut state, &mut text, "u");
        assert_eq!(text, "three\nfour\nfive\nsix\n");
        keys(&mut state, &mut text, "u");
        assert_eq!(text, original);
    }

    #[test]
    fn mouse_and_external_visual_cuts_repeat_without_any_typed_motion_history() {
        for mouse in [false, true] {
            let mut text = "abcdefghi".to_owned();
            let mut state = VimState::new();
            if mouse {
                state.set_mouse_selection(&text, 3, 0, false);
            } else {
                state.set_selection_boundaries(&text, 3, 0);
            }
            keys(&mut state, &mut text, "i");
            state.cut_visual(&mut text);
            keys(&mut state, &mut text, ".w");
            assert_eq!(text, "ghi");
            assert_eq!(state.pending_command(), "");
            keys(&mut state, &mut text, "uu");
            assert_eq!(text, "abcdefghi");
        }
        let mut text = "abcdefghi".to_owned();
        let mut state = VimState::new();
        state.set_mouse_selection(&text, 3, 0, false);
        keys(&mut state, &mut text, "d.w");
        assert_eq!(text, "ghi");
        assert_eq!(state.pending_command(), "");
    }

    #[test]
    fn multiline_native_visual_cut_repeats_line_count_and_last_line_column() {
        let original = "ab\ncdef\nXYZuvw\n";
        let mut text = original.to_owned();
        let mut state = VimState::new();
        state.set_selection(&text, 1..5);
        state.cut_visual(&mut text);
        assert_eq!(text, "aef\nXYZuvw\n");
        keys(&mut state, &mut text, ".");
        assert_eq!(text, "aZuvw\n");
        assert_eq!(state.register().unwrap().text, "ef\nXY");
        assert!(!state.register().unwrap().linewise);
        keys(&mut state, &mut text, "u");
        assert_eq!(text, "aef\nXYZuvw\n");
        keys(&mut state, &mut text, "u");
        assert_eq!(text, original);
    }

    #[test]
    fn visual_cut_replay_is_independent_of_key_history_and_motion_count_limits() {
        let area = MAX_COUNT + 2;
        let mut text = format!("{}{}", "a".repeat(area), "z".repeat(area));
        let mut state = VimState::new();
        state.set_selection(&text, 0..area);
        state.cut_visual(&mut text);
        assert_eq!(text.len(), area);
        keys(&mut state, &mut text, ".");
        assert!(text.is_empty());
        assert_eq!(state.register().unwrap().text.len(), area);

        text = "a".repeat(213);
        state = VimState::new();
        keys(&mut state, &mut text, &format!("v{}d.", "l".repeat(70)));
        assert_eq!(text.len(), 71);
        assert_eq!(state.pending_command(), "");
    }

    #[test]
    fn native_cut_does_not_change_normal_mode_or_literal_visual_replacement() {
        let mut text = "abcdefghi".to_owned();
        let mut state = VimState::new();
        keys(&mut state, &mut text, "d");
        assert_eq!(state.cut_visual(&mut text), Action::default());
        assert_eq!(state.pending_command(), "d");
        state.handle_key(&mut text, VimKey::Escape);
        keys(&mut state, &mut text, "vllrd");
        assert_eq!(text, "ddddefghi");
        keys(&mut state, &mut text, "u");
        assert_eq!(text, "abcdefghi");
    }

    #[test]
    fn visual_cut_of_empty_document_exits_visual_without_arming_a_command() {
        for mode in ['v', 'V'] {
            for operator in ['d', 'x'] {
                let mut text = String::new();
                let mut state = VimState::new();
                state.handle_key(&mut text, VimKey::Char(mode));
                let action = state.handle_key(&mut text, VimKey::Char(operator));
                assert!(action.consumed && action.mode_changed);
                assert!(!action.text_changed);
                assert_eq!(state.mode, VimMode::Normal);
                assert_eq!(state.pending_command(), "");
                assert!(!state.can_undo());
            }
        }
    }

    #[test]
    fn native_visual_cut_preserves_explicit_line_end_intent_not_mouse_coincidence() {
        let original = "one\nlonger\nend";
        for explicit in [false, true] {
            let mut text = original.to_owned();
            let mut state = VimState::new();
            if explicit {
                keys(&mut state, &mut text, "v$r");
            } else {
                state.set_mouse_selection(&text, 3, 0, false);
            }
            state.cut_visual(&mut text);
            assert_eq!(text, "\nlonger\nend");
            keys(&mut state, &mut text, "j.");
            assert_eq!(text, if explicit { "\n\nend" } else { "\nger\nend" });
            keys(&mut state, &mut text, "u");
            assert_eq!(text, "\nlonger\nend");
            keys(&mut state, &mut text, "u");
            assert_eq!(text, original);
        }
    }

    fn search_motion(query: &str, backwards: bool, count: usize) -> SearchMotion {
        SearchMotion {
            query: query.into(),
            case_sensitive: true,
            whole_word: false,
            regex: true,
            backwards,
            count,
        }
    }

    #[test]
    fn external_search_motion_preserves_operator_count_and_exact_newline_target() {
        let mut state = VimState::new();
        let mut text = "one\ntwo\n".to_owned();
        keys(&mut state, &mut text, "2d3");
        assert_eq!(state.take_search_count(), 6);
        assert!(state.search_operator_pending());
        let action = state.apply_search_motion(&mut text, 3, search_motion(r"\n", false, 6));
        assert!(action.text_changed);
        assert_eq!(text, "\ntwo\n");
        assert_eq!(action.yanked.unwrap().text, "one");
        keys(&mut state, &mut text, "u");
        assert_eq!(text, "one\ntwo\n");
    }

    #[test]
    fn exclusive_search_at_column_one_adjusts_to_linewise_or_previous_line_end() {
        for (cursor, expected, linewise, register) in [
            (0, "two\n", true, "  one\n"),
            (2, "two\n", true, "  one\n"),
            (3, "  o\ntwo\n", false, "ne"),
        ] {
            let mut state = VimState::new();
            let mut text = "  one\ntwo\n".to_owned();
            state.set_cursor(&text, cursor);
            keys(&mut state, &mut text, "d");
            state.take_search_count();
            let action = state.apply_search_motion(&mut text, 6, search_motion("two", false, 1));
            assert_eq!(text, expected);
            assert_eq!(
                action.yanked,
                Some(Yank {
                    text: register.into(),
                    linewise
                })
            );
            keys(&mut state, &mut text, "u");
            assert_eq!(text, "  one\ntwo\n");
        }
    }

    #[test]
    fn backwards_search_yank_moves_to_start_without_changing_text() {
        let mut state = VimState::new();
        let mut text = "one foo two".to_owned();
        state.set_cursor(&text, 8);
        keys(&mut state, &mut text, "y");
        let action = state.apply_search_motion(&mut text, 4, search_motion("foo", true, 1));
        assert_eq!(text, "one foo two");
        assert!(!state.can_undo());
        assert_eq!(state.cursor, 4);
        assert_eq!(action.yanked.unwrap().text, "foo ");
    }

    #[test]
    fn dot_replays_search_delete_pattern_and_replacement_count_at_new_cursor() {
        let mut state = VimState::new();
        let original = "one foo two foo three foo four foo end";
        let mut text = original.to_owned();
        keys(&mut state, &mut text, "d");
        state.apply_search_motion(&mut text, 4, search_motion("foo", false, 1));
        assert_eq!(text, "foo two foo three foo four foo end");
        keys(&mut state, &mut text, ".");
        assert_eq!(text, "foo three foo four foo end");
        keys(&mut state, &mut text, "u2.");
        assert_eq!(text, "foo four foo end");
        keys(&mut state, &mut text, "u");
        assert_eq!(text, "foo two foo three foo four foo end");
    }

    #[test]
    fn search_change_dot_repeats_insert_and_each_operation_has_one_undo() {
        let mut state = VimState::new();
        let mut text = "one foo two foo end".to_owned();
        keys(&mut state, &mut text, "c");
        state.apply_search_motion(&mut text, 4, search_motion("foo", false, 1));
        assert_eq!(state.mode, VimMode::Insert);
        state.insert_text(&mut text, "X ");
        state.finish_insert(&mut text);
        assert_eq!(text, "X foo two foo end");
        keys(&mut state, &mut text, "w.");
        assert_eq!(text, "X X foo end");
        keys(&mut state, &mut text, "u");
        assert_eq!(text, "X foo two foo end");
        keys(&mut state, &mut text, "u");
        assert_eq!(text, "one foo two foo end");
    }
    fn position(state: &VimState, text: &str) -> (usize, usize) {
        text.chars()
            .take(state.cursor)
            .fold((0, 0), |(row, column), ch| {
                if ch == '\n' {
                    (row + 1, 0)
                } else {
                    (row, column + 1)
                }
            })
    }
    fn keys(state: &mut VimState, text: &mut String, value: &str) {
        for ch in value.chars() {
            state.handle_key(text, VimKey::Char(ch));
        }
    }
    #[test]
    fn exact_external_selection_keeps_unicode_and_final_newline() {
        let mut state = VimState::new();
        let mut text = "ä🙂\n中".to_owned();
        state.set_selection(&text, 1..3);
        assert_eq!(state.selection(&text), Some(1..3));
        let action = state.handle_key(&mut text, VimKey::Char('y'));
        assert_eq!(action.yanked.unwrap().text, "🙂\n");
        assert_eq!(text, "ä🙂\n中");
        assert_eq!(state.mode, VimMode::Normal);

        text = "ä🙂\n".to_owned();
        state.set_selection_boundaries(&text, 3, 1);
        assert_eq!(state.cursor, 1);
        assert_eq!(state.selection(&text), Some(1..3));
        state.handle_key(&mut text, VimKey::Char('o'));
        assert_eq!(state.cursor, 2);
        assert_eq!(state.selection(&text), Some(1..3));
        state.handle_key(&mut text, VimKey::Char('d'));
        assert_eq!(text, "ä");
        assert_eq!(state.register().unwrap().text, "🙂\n");
        state.handle_key(&mut text, VimKey::Char('u'));
        assert_eq!(text, "ä🙂\n");
    }

    #[test]
    fn external_boundaries_and_mouse_clicks_have_distinct_eof_rules() {
        let mut state = VimState::new();
        let text = "ä🙂\n中";
        state.set_selection(text, usize::MAX..usize::MAX);
        assert_eq!(state.mode, VimMode::Normal);
        assert_eq!(state.cursor, 4);
        assert_eq!(state.selection(text), None);
        state.set_mouse_selection(text, 2, 2, false);
        assert_eq!(state.cursor, 1);
        assert_eq!(state.mode, VimMode::Normal);
        state.set_mouse_selection(text, 999, 999, false);
        assert_eq!(state.cursor, 3);
        state.set_selection("", 0..999);
        assert_eq!(state.mode, VimMode::Normal);
        assert_eq!(state.cursor, 0);
        assert!(!state.can_undo());
    }

    #[test]
    fn mouse_line_selection_does_not_add_the_end_exclusive_line() {
        for (primary, anchor) in [(2, 0), (0, 2)] {
            let mut state = VimState::new();
            let mut text = "a\nb\n".to_owned();
            state.set_mouse_selection(&text, primary, anchor, true);
            assert_eq!(state.mode, VimMode::VisualLine);
            assert_eq!(state.selection(&text), Some(0..2));
            let action = state.handle_key(&mut text, VimKey::Char('y'));
            let yank = action.yanked.unwrap();
            assert!(yank.linewise);
            assert_eq!(yank.text, "a\n");
        }
    }

    #[test]
    fn mouse_navigation_in_insert_keeps_one_undo_transaction() {
        let mut state = VimState::new();
        let mut text = "ab".to_owned();
        keys(&mut state, &mut text, "i");
        state.insert_text(&mut text, "ä");
        state.set_mouse_selection(&text, 3, 1, false);
        assert_eq!(state.mode, VimMode::Insert);
        assert_eq!(state.cursor, 3);
        state.insert_text(&mut text, "🙂");
        state.finish_insert(&mut text);
        assert_eq!(text, "äab🙂");
        assert_eq!(state.undo.len(), 1);
        state.handle_key(&mut text, VimKey::Char('u'));
        assert_eq!(text, "ab");
        state.handle_key(&mut text, VimKey::Redo);
        assert_eq!(text, "äab🙂");
    }

    #[test]
    fn pane_navigation_restores_selection_without_forking_history_or_registers() {
        let mut state = VimState::new();
        let mut text = "ä🙂\n中xy".to_owned();
        state.set_selection_boundaries(&text, 3, 1);
        let first_view = state.navigation();
        let second_view = VimNavigation::default();
        state.restore_navigation(&text, &second_view);
        assert_eq!(state.selection(&text), None);
        keys(&mut state, &mut text, "x");
        assert_eq!(text, "🙂\n中xy");
        let register = state.register().unwrap().clone();
        state.restore_navigation(&text, &first_view);
        assert_eq!(state.selection(&text), Some(1..3));
        assert_eq!(state.cursor, 1);
        assert_eq!(state.register(), Some(&register));
        assert!(state.can_undo());
        // Native undo remains available in a restored visual pane; Vim's
        // visual u now correctly means lowercase the selected characters.
        state.history_edit(&mut text, false);
        assert_eq!(text, "ä🙂\n中xy");
        state.restore_navigation(&text, &second_view);
        assert!(state.can_redo());
        state.handle_key(&mut text, VimKey::Redo);
        assert_eq!(text, "🙂\n中xy");
        keys(&mut state, &mut text, ".");
        assert_eq!(text, "\n中xy");
    }

    #[test]
    fn shifted_navigation_preserves_reverse_visual_lines_and_modal_state() {
        let mut state = VimState::new();
        let mut text = "ä🙂\n中xy\nthird\nlast".to_owned();
        state.set_selection_boundaries(&text, 6, 3);
        keys(&mut state, &mut text, "V");
        let navigation = state.navigation();
        let selection = state.selection(&text).unwrap();
        let edit = crate::editor::move_lines(&text, selection.clone(), true);
        let delta = edit.selection.start as isize - selection.start as isize;
        state.begin_external_edit(&text);
        text = edit.text;
        state.restore_navigation(&text, &navigation.shifted(delta));
        assert_eq!(text, "ä🙂\nthird\n中xy\nlast");
        assert_eq!(state.mode, VimMode::VisualLine);
        assert_eq!(state.cursor, 9);
        assert_eq!(state.visual_anchor, Some(11));
        assert_eq!(state.selection(&text), Some(9..13));
        assert_eq!(state.undo.len(), 1);

        let mut pending = VimState::new();
        keys(&mut pending, &mut text, "2d3");
        let shifted = pending.navigation().shifted(5);
        assert_eq!(shifted.cursor, 5);
        assert!(matches!(
            shifted.pending,
            Some(Pending::Operator(Operator::Delete, 2, true))
        ));
        assert_eq!(shifted.count, 3);
        assert_eq!(shifted.command_keys, pending.command_keys);
        let saturated = navigation.shifted(isize::MIN);
        assert_eq!(saturated.cursor, 0);
        assert_eq!(saturated.visual_anchor, Some(0));
    }

    #[test]
    fn pane_navigation_restores_pending_commands_and_preferred_columns() {
        let mut state = VimState::new();
        let mut text = "one two three".to_owned();
        keys(&mut state, &mut text, "2d");
        let operator_view = state.navigation();
        state.restore_navigation(&text, &VimNavigation::default());
        keys(&mut state, &mut text, "l");
        assert_eq!(text, "one two three");
        state.restore_navigation(&text, &operator_view);
        assert_eq!(state.pending_command(), "2d");
        keys(&mut state, &mut text, "w");
        assert_eq!(text, "three");
        keys(&mut state, &mut text, "u");
        assert_eq!(text, "one two three");

        text = "abcdef\nx\nabcdef".to_owned();
        state = VimState::new();
        keys(&mut state, &mut text, "4lj");
        let short_line_view = state.navigation();
        state.restore_navigation(&text, &VimNavigation::default());
        keys(&mut state, &mut text, "l");
        state.restore_navigation(&text, &short_line_view);
        keys(&mut state, &mut text, "j");
        assert_eq!(position(&state, &text), (2, 4));
    }

    #[test]
    fn pane_navigation_clamps_after_another_view_shortens_the_document() {
        let mut state = VimState::new();
        state.set_selection_boundaries("ä🙂\n中", 4, 1);
        let view = state.navigation();
        state.restore_navigation("é", &view);
        assert_eq!(state.cursor, 1);
        assert_eq!(state.selection("é"), Some(1..1));
        state.restore_navigation("", &view);
        assert_eq!(state.cursor, 0);
        assert_eq!(state.selection(""), Some(0..0));
    }

    #[test]
    fn unicode_offsets_and_redo() {
        let mut state = VimState::new();
        let mut text = "ä🙂中\nß".to_owned();
        assert_eq!(state.mode, VimMode::Normal);
        keys(&mut state, &mut text, "lx");
        assert_eq!(text, "ä中\nß");
        assert_eq!(state.register().unwrap().text, "🙂");
        keys(&mut state, &mut text, "u");
        assert_eq!(text, "ä🙂中\nß");
        assert_eq!(state.cursor, 1);
        state.handle_key(&mut text, VimKey::Redo);
        assert_eq!(text, "ä中\nß");
    }
    #[test]
    fn crlf_line_motions_and_edits_never_treat_the_break_as_content() {
        for command in ["$", "99l", "99|"] {
            let mut state = VimState::new();
            let mut text = "α🙂\r\nβ\r\n".to_owned();
            keys(&mut state, &mut text, command);
            assert_eq!(state.cursor, 1, "{command}");
        }
        for (initial, command, expected) in [
            ("α🙂\r\nβ", "$x", "α\r\nβ"),
            ("α🙂\r\nβ", "D", "\r\nβ"),
            ("αβ\r\nγ", "dw", "\r\nγ"),
            ("\r\nβ", "x", "\r\nβ"),
            ("α\r\nβ", "Gdd", "α"),
            ("α\r\nβ\r\nγ", "Gdk", "α"),
        ] {
            let mut state = VimState::new();
            let mut text = initial.to_owned();
            keys(&mut state, &mut text, command);
            assert_eq!(text, expected, "{command}");
            if text != initial {
                keys(&mut state, &mut text, "u");
                assert_eq!(text, initial, "undo {command}");
            }
        }
        let mut state = VimState::new();
        let mut text = "α\r\nβ".to_owned();
        keys(&mut state, &mut text, "A!");
        state.finish_insert(&mut text);
        assert_eq!(text, "α!\r\nβ");
        keys(&mut state, &mut text, "u");
        assert_eq!(text, "α\r\nβ");
    }

    #[test]
    fn crlf_linewise_registers_and_pastes_preserve_complete_breaks() {
        for (initial, command, expected) in [
            ("α\r\nβ", "yyp", "α\r\nα\r\nβ"),
            ("α\r\nβ", "yyGp", "α\r\nβ\r\nα"),
            ("α\r\nβ", "GyyP", "α\r\nβ\r\nβ"),
            ("α\r\nβ", "yyGVP", "α\r\nα"),
        ] {
            let mut state = VimState::new();
            let mut text = initial.to_owned();
            keys(&mut state, &mut text, command);
            assert_eq!(text, expected, "{command}");
            keys(&mut state, &mut text, "u");
            assert_eq!(text, initial, "undo {command}");
        }
        let mut state = VimState::new();
        let mut text = "α\r\nβ".to_owned();
        keys(&mut state, &mut text, "yy");
        assert_eq!(state.register().unwrap().text, "α\r\n");
    }

    #[test]
    fn crlf_visual_replace_keeps_line_breaks_and_paragraphs_see_empty_lines() {
        let mut state = VimState::new();
        let mut text = "α\r\n\r\nβ\r\n".to_owned();
        keys(&mut state, &mut text, "}");
        assert_eq!(state.cursor, 3);
        keys(&mut state, &mut text, "ggVjrx");
        assert_eq!(text, "x\r\n\r\nβ\r\n");
    }

    #[test]
    fn crlf_insert_and_character_replacement_use_complete_line_breaks() {
        let mut state = VimState::new();
        let mut text = "αβ\r\nγ".to_owned();
        keys(&mut state, &mut text, "i");
        state.handle_key(&mut text, VimKey::Enter);
        assert_eq!(text, "\r\nαβ\r\nγ");
        assert_eq!(state.cursor, 2);
        state.handle_key(&mut text, VimKey::Backspace);
        assert_eq!(text, "αβ\r\nγ");
        assert_eq!(state.cursor, 0);
        state.finish_insert(&mut text);
        keys(&mut state, &mut text, "r");
        state.handle_key(&mut text, VimKey::Enter);
        assert_eq!(text, "\r\nβ\r\nγ");
        assert_eq!(state.cursor, 2);
        keys(&mut state, &mut text, "u");
        assert_eq!(text, "αβ\r\nγ");
    }
    #[test]
    fn counts_multiply_and_line_boundaries_work() {
        let mut state = VimState::new();
        let mut text = "one two three four five six seven".to_owned();
        keys(&mut state, &mut text, "2d3w");
        assert_eq!(text, "seven");
        for (command, expected) in [
            ("2dd", "c\nd"),
            ("d2d", "c\nd"),
            ("2d2d", ""),
            ("Gdd", "a\nb\nc"),
            ("Gdk", "a\nb"),
        ] {
            let mut state = VimState::new();
            let mut text = "a\nb\nc\nd".to_owned();
            keys(&mut state, &mut text, command);
            assert_eq!(text, expected, "{command}");
        }
    }
    #[test]
    fn column_and_punctuation_motions() {
        let mut state = VimState::new();
        let mut text = "abcdef\nx\nabcdef".to_owned();
        keys(&mut state, &mut text, "4ljj");
        assert_eq!(position(&state, &text), (2, 4));
        text = "foo.bar baz".to_owned();
        state.set_cursor(&text, 0);
        keys(&mut state, &mut text, "w");
        assert_eq!(state.cursor, 3);
        keys(&mut state, &mut text, "we");
        assert_eq!(state.cursor, 6);
    }
    #[test]
    fn change_word_and_insert_are_one_undo_step() {
        let mut state = VimState::new();
        let mut text = "hello world".to_owned();
        keys(&mut state, &mut text, "cw");
        assert_eq!(text, " world");
        state.insert_text(&mut text, "é🙂");
        assert_eq!(state.cursor, 2);
        state.finish_insert(&mut text);
        keys(&mut state, &mut text, "u");
        assert_eq!(text, "hello world");
        state.handle_key(&mut text, VimKey::Redo);
        assert_eq!(text, "é🙂 world");
        state = VimState::new();
        text = "a b c".to_owned();
        keys(&mut state, &mut text, "2cw");
        assert_eq!(text, " c");
    }
    #[test]
    fn operator_inclusivity_and_empty_lines() {
        for (initial, command, expected) in [
            ("abc", "dw", ""),
            ("abc", "d$", ""),
            ("abc", "$dl", "ab"),
            ("\nabc", "x", "\nabc"),
        ] {
            let mut state = VimState::new();
            let mut text = initial.to_owned();
            keys(&mut state, &mut text, command);
            assert_eq!(text, expected, "{command}");
        }
    }
    #[test]
    fn paste_blank_line_cursor_and_final_newlines() {
        let mut state = VimState::new();
        let mut text = "a\nb".to_owned();
        state.set_register(Yank {
            text: "\n".to_owned(),
            linewise: true,
        });
        keys(&mut state, &mut text, "p");
        assert_eq!(text, "a\n\nb");
        assert_eq!(position(&state, &text), (1, 0));
        for (initial, command, expected) in [
            ("a", "yyp", "a\na"),
            ("a\n", "yyp", "a\na\n"),
            ("a\nb", "yyGp", "a\nb\na"),
        ] {
            let mut state = VimState::new();
            let mut text = initial.to_owned();
            keys(&mut state, &mut text, command);
            assert_eq!(text, expected, "{command}");
        }
    }
    #[test]
    fn redo_survives_noop_commands_but_not_new_edits() {
        let mut state = VimState::new();
        let mut text = "abc".to_owned();
        keys(&mut state, &mut text, "xu0yyi");
        state.finish_insert(&mut text);
        assert!(state.can_redo());
        state.handle_key(&mut text, VimKey::Redo);
        assert_eq!(text, "bc");
        keys(&mut state, &mut text, "ux");
        assert!(!state.can_redo());
    }
    #[test]
    fn open_line_indent_and_external_edits_group() {
        let mut state = VimState::new();
        let mut text = "  one\ntwo".to_owned();
        keys(&mut state, &mut text, "o");
        assert_eq!(text, "  one\n  \ntwo");
        state.insert_text(&mut text, "three");
        state.finish_insert(&mut text);
        keys(&mut state, &mut text, "u");
        assert_eq!(text, "  one\ntwo");
        keys(&mut state, &mut text, "i");
        text.insert_str(0, "external");
        state.notify_external_text_change();
        state.finish_insert(&mut text);
        keys(&mut state, &mut text, "u");
        assert_eq!(text, "  one\ntwo");
    }
    #[test]
    fn pending_commands_cancel_and_history_is_bounded() {
        let mut state = VimState::new();
        let mut text = "one two three".to_owned();
        keys(&mut state, &mut text, "3d");
        state.handle_key(&mut text, VimKey::Escape);
        keys(&mut state, &mut text, "wdx");
        assert_eq!(state.cursor, 4);
        assert_eq!(text, "one two three");
        assert!(state.pending_command().is_empty());
        text = "x".repeat(200);
        state = VimState::new();
        keys(&mut state, &mut text, &"x".repeat(150));
        assert_eq!(state.undo.len(), MAX_HISTORY);
    }
    #[test]
    fn open_line_indents_python_and_preserves_windows_endings() {
        let mut state = VimState::new();
        let mut text = "if ready:\r\n    run()\r\n".to_owned();
        keys(&mut state, &mut text, "o");
        assert_eq!(text, "if ready:\r\n    \r\n    run()\r\n");
        assert_eq!(state.cursor, 15);
        state.insert_text(&mut text, "next()");
        state.finish_insert(&mut text);
        keys(&mut state, &mut text, "u");
        assert_eq!(text, "if ready:\r\n    run()\r\n");
        state.cursor = 11;
        keys(&mut state, &mut text, "O");
        assert_eq!(text, "if ready:\r\n    \r\n    run()\r\n");
        assert_eq!(state.cursor, 15);
    }

    #[test]
    fn opening_and_shifting_lines_honor_tab_indentation() {
        let mut state = VimState::new();
        state.set_indentation(4, true);
        let mut text = "if ready:\n\trun()\n".to_owned();
        keys(&mut state, &mut text, "o");
        assert_eq!(text, "if ready:\n\t\n\trun()\n");
        state.finish_insert(&mut text);
        keys(&mut state, &mut text, "u>>");
        assert_eq!(text, "\tif ready:\n\trun()\n");
        keys(&mut state, &mut text, "<<");
        assert_eq!(text, "if ready:\n\trun()\n");
    }
    #[test]
    fn visual_character_selection_yank_cut_and_change() {
        let mut state = VimState::new();
        let mut text = "ä🙂中 xyz".to_owned();
        keys(&mut state, &mut text, "vl");
        assert_eq!(state.selection(&text), Some(0..2));
        let action = state.handle_key(&mut text, VimKey::Char('y'));
        assert_eq!(action.yanked.unwrap().text, "ä🙂");
        assert_eq!(state.mode, VimMode::Normal);
        assert!(state.selection(&text).is_none());
        keys(&mut state, &mut text, "vld");
        assert_eq!(text, "中 xyz");
        keys(&mut state, &mut text, "u0vlc");
        state.insert_text(&mut text, "new");
        state.finish_insert(&mut text);
        assert_eq!(text, "new中 xyz");
        keys(&mut state, &mut text, "u");
        assert_eq!(text, "ä🙂中 xyz");
    }
    #[test]
    fn visual_line_selection_down_and_up() {
        for command in ["Vjd", "jVkd"] {
            let mut state = VimState::new();
            let mut text = "one\ntwo\nthree".to_owned();
            keys(&mut state, &mut text, command);
            assert_eq!(text, "three", "{command}");
            assert!(state.register().unwrap().linewise);
            assert_eq!(state.register().unwrap().text, "one\ntwo\n");
        }
        let mut state = VimState::new();
        let mut text = "one\ntwo\nthree".to_owned();
        keys(&mut state, &mut text, "Vjc");
        assert_eq!(text, "\nthree");
        assert_eq!(state.mode, VimMode::Insert);
        state.insert_text(&mut text, "replacement");
        state.finish_insert(&mut text);
        keys(&mut state, &mut text, "u");
        assert_eq!(text, "one\ntwo\nthree");
    }
    #[test]
    fn visual_paste_and_indent_are_undoable() {
        let mut state = VimState::new();
        let mut text = "one\ntwo\nthree".to_owned();
        state.set_register(Yank {
            text: "other\n".to_owned(),
            linewise: true,
        });
        keys(&mut state, &mut text, "Vjp");
        assert_eq!(text, "other\nthree");
        keys(&mut state, &mut text, "uggVj>");
        assert_eq!(text, "    one\n    two\nthree");
        assert_eq!(state.mode, VimMode::Normal);
        keys(&mut state, &mut text, "ugg2>>");
        assert_eq!(text, "    one\n    two\nthree");
        keys(&mut state, &mut text, "2<<");
        assert_eq!(text, "one\ntwo\nthree");
    }
    #[test]
    fn find_replace_join_and_matching_bracket() {
        let mut state = VimState::new();
        let mut text = "one, two, three".to_owned();
        keys(&mut state, &mut text, "f,;");
        assert_eq!(state.cursor, 8);
        keys(&mut state, &mut text, "0dt,");
        assert_eq!(text, ", two, three");
        keys(&mut state, &mut text, "u0d2f,");
        assert_eq!(text, " three");
        text = "abc\ndef".to_owned();
        state = VimState::new();
        keys(&mut state, &mut text, "2r🙂J");
        assert_eq!(text, "🙂🙂c def");
        text = "(a(b)c)".to_owned();
        state = VimState::new();
        keys(&mut state, &mut text, "%");
        assert_eq!(state.cursor, 6);
    }

    #[test]
    fn character_find_counts_and_repeats_cross_lines_in_both_directions() {
        let mut state = VimState::new();
        let mut text = "a🦀x\n\n中x\r\n🙂x\n".to_owned();
        let original = text.clone();
        for (command, cursor) in [
            ("fx", 2),
            (";", 6),
            (";", 10),
            (";", 10),
            (",", 6),
            (";", 10),
            ("G$2Fx", 2),
            ("2;", 2),
            ("2,", 10),
            ("3Fx", 10),
            ("fz", 10),
        ] {
            keys(&mut state, &mut text, command);
            assert_eq!(state.cursor, cursor, "{command}");
            assert_eq!(state.pending_command(), "");
        }
        assert_eq!(text, original);
        assert!(!state.can_undo());
    }

    #[test]
    fn surround_add_change_delete_preserve_registers_and_repeat_as_one_edit() {
        for (initial, cursor, command, expected) in [
            ("one two", 0, "ysiw)", "(one) two"),
            ("one two", 0, "ysw]", "[one] two"),
            ("one two", 0, "ysiw(", "( one ) two"),
            ("  α🦀\r\nnext", 2, "yss\"", "  \"α🦀\"\r\nnext"),
            ("one\nx tail", 0, "ysfx]", "[one\nx] tail"),
            ("foo \"bar\" baz", 6, "cs\"'", "foo 'bar' baz"),
            ("foo \"bar\" baz", 6, "ds\"", "foo bar baz"),
            ("( α🦀 ) tail", 3, "cs(]", "[α🦀] tail"),
            ("( α🦀 ) tail", 3, "ds)", " α🦀  tail"),
            ("( α🦀 ) tail", 3, "ds(", "α🦀 tail"),
            ("a((b))c", 3, "2ds)", "a(b)c"),
        ] {
            let mut state = VimState::new();
            let mut text = initial.to_owned();
            state.cursor = cursor;
            let register = Yank {
                text: "KEEP".into(),
                linewise: false,
            };
            state.set_register(register.clone());
            keys(&mut state, &mut text, command);
            assert_eq!(text, expected, "{command}");
            assert_eq!(state.register(), Some(&register));
            assert_eq!(state.undo.len(), 1, "{command}");
            assert_eq!(state.mode, VimMode::Normal);
            keys(&mut state, &mut text, "u");
            assert_eq!(text, initial, "{command}");
        }
        let mut state = VimState::new();
        let mut text = "one two three".to_owned();
        keys(&mut state, &mut text, "ysiw)www.");
        assert_eq!(text, "(one) (two) three");
        keys(&mut state, &mut text, "u");
        assert_eq!(text, "(one) two three");
        keys(&mut state, &mut text, "gglds)");
        assert_eq!(text, "one two three");
    }

    #[test]
    fn visual_surround_repeats_native_selection_geometry_and_can_cancel() {
        let mut state = VimState::new();
        let mut text = "one two three".to_owned();
        state.set_selection(&text, 0..3);
        keys(&mut state, &mut text, "S]");
        assert_eq!(text, "[one] two three");
        keys(&mut state, &mut text, "www.");
        assert_eq!(text, "[one] [two] three");
        keys(&mut state, &mut text, "uu");
        assert_eq!(text, "one two three");
        keys(&mut state, &mut text, "ysiw");
        assert!(state.expects_literal_character());
        state.handle_key(&mut text, VimKey::Escape);
        keys(&mut state, &mut text, "w");
        assert_eq!(text, "one two three");
        assert_eq!(state.pending_command(), "");
        keys(&mut state, &mut text, "dsw");
        assert_eq!(text, "one two three");
    }

    #[test]
    fn surround_search_dot_resolves_the_pattern_at_the_new_cursor() {
        let mut state = VimState::new();
        let mut text = "a x b x c x".to_owned();
        keys(&mut state, &mut text, "ys");
        state.apply_search_motion(&mut text, 2, search_motion("x", false, 1));
        keys(&mut state, &mut text, "]");
        assert_eq!(text, "[a] x b x c x");
        keys(&mut state, &mut text, "www.");
        assert_eq!(text, "[a] [x b] x c x");
        keys(&mut state, &mut text, "u");
        assert_eq!(text, "[a] x b x c x");
    }

    #[test]
    fn dot_count_replaces_counts_after_surround_and_register_prefixes() {
        let mut state = VimState::new();
        let mut text = "one two three four five six seven".to_owned();
        keys(&mut state, &mut text, "ys2w)");
        assert_eq!(text, "(one two) three four five six seven");
        state.set_cursor(&text, 10);
        keys(&mut state, &mut text, "3.");
        assert_eq!(text, "(one two) (three four five) six seven");

        state = VimState::new();
        text = "abcdef".to_owned();
        keys(&mut state, &mut text, "yl\"02p3.");
        assert_eq!(text, "aaaaaabcdef");
    }

    #[test]
    fn yank_register_zero_survives_deletes_changes_and_visual_pastes() {
        let mut state = VimState::new();
        let mut text = "one two three".to_owned();
        keys(&mut state, &mut text, "yiwwdiw");
        assert_eq!(text, "one  three");
        assert_eq!(state.register().unwrap().text, "two");
        assert_eq!(state.yank_register().unwrap().text, "one");
        keys(&mut state, &mut text, "\"0p");
        assert_eq!(text, "one  onethree");
        assert_eq!(state.yank_register().unwrap().text, "one");
        keys(&mut state, &mut text, "u");
        assert_eq!(text, "one  three");
        keys(&mut state, &mut text, "wviw\"0p");
        assert_eq!(text, "one  one");
        assert_eq!(state.register().unwrap().text, "three");
        assert_eq!(state.yank_register().unwrap().text, "one");
        keys(&mut state, &mut text, "u");
        assert_eq!(text, "one  three");
    }

    #[test]
    fn yank_register_zero_counts_and_dot_use_linewise_or_characterwise_content() {
        let mut state = VimState::new();
        let mut text = "one\ntwo\nthree".to_owned();
        keys(&mut state, &mut text, "yyjdd\"0p");
        assert_eq!(text, "one\nthree\none");
        keys(&mut state, &mut text, ".");
        assert_eq!(text, "one\nthree\none\none");
        assert_eq!(state.yank_register().unwrap().text, "one\n");
        keys(&mut state, &mut text, "u");
        assert_eq!(text, "one\nthree\none");

        state = VimState::new();
        text = "🦀abc".to_owned();
        keys(&mut state, &mut text, "yl2\"0p");
        assert_eq!(text, "🦀🦀🦀abc");
        keys(&mut state, &mut text, "u\"02p");
        assert_eq!(text, "🦀🦀🦀abc");
        keys(&mut state, &mut text, "u\"0p3.");
        assert_eq!(text, "🦀🦀🦀🦀🦀abc");
    }

    #[test]
    fn till_repeats_advance_past_line_endings_and_keep_the_original_direction() {
        for ending in ["\n", "\r\n"] {
            let mut state = VimState::new();
            let mut text = format!("ab{ending}x cd{ending}x ef");
            let first_x = 2 + ending.chars().count();
            let second_x = first_x + 4 + ending.chars().count();
            for (command, cursor) in [
                ("tx", 1),
                (";", first_x + 3),
                (",", first_x + 1),
                (";", first_x + 3),
                (";", first_x + 3),
            ] {
                keys(&mut state, &mut text, command);
                assert_eq!(state.cursor, cursor, "{command} with {ending:?}");
            }
            state.set_cursor(&text, 0);
            keys(&mut state, &mut text, "2tx");
            assert_eq!(state.cursor, second_x - ending.chars().count() - 1);

            state = VimState::new();
            text = format!("x{ending}ab{ending}x{ending}cd");
            let second_x = 3 + 2 * ending.chars().count();
            keys(&mut state, &mut text, "G$Tx");
            assert_eq!(state.cursor, second_x + 1 + ending.chars().count());
            keys(&mut state, &mut text, ";");
            assert_eq!(state.cursor, 1 + ending.chars().count());
            keys(&mut state, &mut text, ",");
            assert_eq!(state.cursor, second_x - ending.chars().count() - 1);
        }
    }

    #[test]
    fn till_repeats_skip_adjacent_matches_in_both_directions() {
        for (initial, cursor, command, expected) in [
            ("a,b,c,d", 0, "t,;", 2),
            ("a,b,c,d", 0, "t,2;", 4),
            (",,a,b,c", 2, "t,,", 1),
            ("a,b,c,d", 6, "T,;", 4),
            ("a,b,c,,", 2, "T,,", 4),
        ] {
            let mut state = VimState::new();
            let mut text = initial.to_owned();
            state.cursor = cursor;
            keys(&mut state, &mut text, command);
            assert_eq!(state.cursor, expected, "{command}");
        }
    }

    #[test]
    fn character_find_operators_keep_exact_multiline_spans() {
        for (initial, command, expected, yanked, cursor) in [
            ("a🦀\nxz\nbx!", "dfx", "z\nbx!", "a🦀\nx", 0),
            ("a🦀\nxz\nbx!", "dtx", "xz\nbx!", "a🦀\n", 0),
            ("a🦀x\nmid\nlast!", "G$dF🦀", "a!", "🦀x\nmid\nlast", 1),
            ("a🦀x\nmid\nlast!", "G$dT🦀", "a🦀!", "x\nmid\nlast", 2),
            (
                "a🦀x\nmid\nlast!",
                "G$yF🦀",
                "a🦀x\nmid\nlast!",
                "🦀x\nmid\nlast",
                1,
            ),
        ] {
            let mut state = VimState::new();
            let mut text = initial.to_owned();
            keys(&mut state, &mut text, command);
            assert_eq!(text, expected, "{command}");
            assert_eq!(state.cursor, cursor, "{command}");
            assert_eq!(state.register().unwrap().text, yanked, "{command}");
            assert!(!state.register().unwrap().linewise);
        }
        for (command, expected) in [(">fx", "    ab\n    x\nnext"), (">tx", "    ab\nx\nnext")] {
            let mut state = VimState::new();
            let mut text = "ab\nx\nnext".to_owned();
            keys(&mut state, &mut text, command);
            assert_eq!(text, expected, "{command}");
        }
    }

    #[test]
    fn dot_replays_multiline_find_deletes_counts_and_insert_changes() {
        let original = "a\nx b\nx c\nx tail";
        let mut state = VimState::new();
        let mut text = original.to_owned();
        keys(&mut state, &mut text, "dfx");
        assert_eq!(text, " b\nx c\nx tail");
        keys(&mut state, &mut text, ".");
        assert_eq!(text, " c\nx tail");
        keys(&mut state, &mut text, "u");
        assert_eq!(text, " b\nx c\nx tail");
        keys(&mut state, &mut text, "2.");
        assert_eq!(text, " tail");
        keys(&mut state, &mut text, "uu");
        assert_eq!(text, original);
        keys(&mut state, &mut text, "2d1fx");
        assert_eq!(text, " c\nx tail");

        state = VimState::new();
        text = original.to_owned();
        keys(&mut state, &mut text, "cfxZ");
        state.finish_insert(&mut text);
        assert_eq!(text, "Z b\nx c\nx tail");
        keys(&mut state, &mut text, "l.");
        assert_eq!(text, "ZZ c\nx tail");
        keys(&mut state, &mut text, "u");
        assert_eq!(text, "Z b\nx c\nx tail");
        keys(&mut state, &mut text, "u");
        assert_eq!(text, original);
    }

    #[test]
    fn repeated_find_operators_and_visual_find_share_multiline_targets() {
        let mut state = VimState::new();
        let mut text = "ax\nbx\ncx\ndx".to_owned();
        keys(&mut state, &mut text, "fx0d;.");
        assert_eq!(text, "\ncx\ndx");
        keys(&mut state, &mut text, "uu");
        assert_eq!(text, "ax\nbx\ncx\ndx");
        keys(&mut state, &mut text, "G$2d,");
        assert_eq!(text, "ax\nbx");

        state = VimState::new();
        text = "a\n🙂b\n🙂c".to_owned();
        keys(&mut state, &mut text, "vf🙂");
        assert_eq!(state.selection(&text), Some(0..3));
        keys(&mut state, &mut text, ";");
        assert_eq!(state.selection(&text), Some(0..6));
        keys(&mut state, &mut text, ",");
        assert_eq!(state.selection(&text), Some(0..3));
        keys(&mut state, &mut text, "d");
        assert_eq!(text, "b\n🙂c");
        keys(&mut state, &mut text, "uggVf🙂;");
        assert_eq!(state.selection(&text), Some(0..7));
    }

    #[test]
    fn character_find_accepts_literal_tab_and_repeats_across_lines() {
        let mut state = VimState::new();
        let mut text = "a\n\tb\n\tc".to_owned();
        keys(&mut state, &mut text, "f");
        state.handle_key(&mut text, VimKey::Tab);
        assert_eq!(state.cursor, 2);
        keys(&mut state, &mut text, ";,");
        assert_eq!(state.cursor, 2);
        keys(&mut state, &mut text, "0d;");
        assert_eq!(text, "a\nc");
    }

    #[test]
    fn multiline_character_paste_starts_at_inserted_text_and_dot_is_undoable() {
        for (command, repeated) in [("p", "a🦀🦀\n中\n中bc"), ("lP", "a🦀\n中🦀\n中bc")]
        {
            let mut state = VimState::new();
            let mut text = "abc".to_owned();
            state.set_register(Yank {
                text: "🦀\n中".into(),
                linewise: false,
            });
            keys(&mut state, &mut text, command);
            assert_eq!(text, "a🦀\n中bc", "{command}");
            assert_eq!(state.cursor, 1, "{command}");
            keys(&mut state, &mut text, ".");
            assert_eq!(text, repeated, "{command}");
            keys(&mut state, &mut text, "u");
            assert_eq!(text, "a🦀\n中bc", "{command}");
        }
    }

    #[test]
    fn visual_uppercase_p_preserves_register_for_further_pastes_and_dot() {
        for (command, register) in [("p", "abc"), ("P", "XY")] {
            let mut state = VimState::new();
            let mut text = "abcdefghi".to_owned();
            state.set_register(Yank {
                text: "XY".into(),
                linewise: false,
            });
            keys(&mut state, &mut text, "v2l");
            let action = state.handle_key(&mut text, VimKey::Char(command.chars().next().unwrap()));
            assert_eq!(text, "XYdefghi");
            assert_eq!(state.register().unwrap().text, register);
            assert_eq!(action.yanked.is_none(), command == "P");
            keys(&mut state, &mut text, "u");
            assert_eq!(text, "abcdefghi");
        }
        let mut state = VimState::new();
        let mut text = "abcdefghi".to_owned();
        state.set_register(Yank {
            text: "XY".into(),
            linewise: false,
        });
        keys(&mut state, &mut text, "v2lPl.");
        assert_eq!(text, "XYXYghi");
        assert_eq!(state.register().unwrap().text, "XY");
        keys(&mut state, &mut text, "u");
        assert_eq!(text, "XYdefghi");
    }
    #[test]
    fn text_objects_select_words_pairs_and_quotes() {
        let mut state = VimState::new();
        let mut text = "one two three".to_owned();
        keys(&mut state, &mut text, "wviw");
        assert_eq!(state.selection(&text), Some(4..7));
        keys(&mut state, &mut text, "d");
        assert_eq!(text, "one  three");
        text = "call(first, nested(value))".to_owned();
        state = VimState::new();
        state.set_cursor(&text, 20);
        keys(&mut state, &mut text, "ci(");
        assert_eq!(text, "call(first, nested())");
        state.insert_text(&mut text, "arg");
        state.finish_insert(&mut text);
        keys(&mut state, &mut text, "u");
        assert_eq!(text, "call(first, nested(value))");
        text = "let s = \"some \\\"quoted\\\" text\";".to_owned();
        state = VimState::new();
        state.set_cursor(&text, 12);
        keys(&mut state, &mut text, "di\"");
        assert_eq!(text, "let s = \"\";");
    }
    #[test]
    fn oversized_counted_paste_is_rejected_before_allocation() {
        let mut state = VimState::new();
        let mut text = "a".to_owned();
        state.set_register(Yank {
            text: "b".repeat(2_000),
            linewise: false,
        });
        keys(&mut state, &mut text, "9999p");
        assert_eq!(text, "a");
        assert!(state.undo.is_empty());
    }
    #[test]
    fn huge_numeric_counts_saturate_and_motions_stop_at_boundaries() {
        let mut state = VimState::new();
        let mut text = "ä🙂\none two\nlast".to_owned();
        keys(&mut state, &mut text, "999999999999999999999999j");
        assert_eq!(position(&state, &text), (2, 0));
        keys(&mut state, &mut text, "gg9999w");
        assert_eq!(state.cursor, text.chars().count() - 1);
        keys(&mut state, &mut text, "9999b");
        assert_eq!(state.cursor, 0);
        keys(&mut state, &mut text, "9999dd");
        assert!(text.is_empty());
        assert_eq!(state.undo.len(), 1);
    }
    #[test]
    fn dot_replays_operators_and_replaces_the_original_count() {
        let mut state = VimState::new();
        let mut text = "one two three".to_owned();
        keys(&mut state, &mut text, "dw.");
        assert_eq!(text, "three");
        assert_eq!(state.undo.len(), 2);
        keys(&mut state, &mut text, "u");
        assert_eq!(text, "two three");
        keys(&mut state, &mut text, ".");
        assert_eq!(text, "three");
        text = "abcdefghi".to_owned();
        state = VimState::new();
        keys(&mut state, &mut text, "2x3.");
        assert_eq!(text, "fghi");
        keys(&mut state, &mut text, ".");
        assert_eq!(text, "i");
        text = "one two three four five six seven eight nine ten".to_owned();
        state = VimState::new();
        keys(&mut state, &mut text, "d2w3.");
        assert_eq!(text, "six seven eight nine ten");
    }

    #[test]
    fn windows_history_discards_visual_operator_and_count_prefixes_before_dot() {
        for prefix in ["vll", "V", "d", "3"] {
            for redo in [false, true] {
                let mut state = VimState::new();
                let mut text = "abcdefghi".to_owned();
                keys(&mut state, &mut text, "x");
                if redo {
                    keys(&mut state, &mut text, "u");
                }
                keys(&mut state, &mut text, prefix);
                state.history_edit(&mut text, redo);
                assert!(
                    state.command_keys.is_empty(),
                    "prefix={prefix}, redo={redo}"
                );
                keys(&mut state, &mut text, "x.");
                assert_eq!(text, if redo { "defghi" } else { "cdefghi" });
            }
        }
    }

    #[test]
    fn mouse_visual_changes_repeat_completed_geometry_and_one_undo_step() {
        for (operation, first, repeated) in [("rX", "XXX def", "XXX XXX"), ("cZ", "Z def", "Z Z")] {
            let mut state = VimState::new();
            let mut text = "abc def".to_owned();
            state.set_mouse_selection(&text, 3, 0, false);
            keys(&mut state, &mut text, operation);
            state.finish_insert(&mut text);
            assert_eq!(text, first);
            keys(&mut state, &mut text, "w.");
            assert_eq!(text, repeated, "operation={operation}");
            keys(&mut state, &mut text, "u");
            assert_eq!(text, first);
            state.handle_key(&mut text, VimKey::Redo);
            assert_eq!(text, repeated);
        }

        let mut state = VimState::new();
        let mut text = "a\nb\nc\nd".to_owned();
        state.set_mouse_selection(&text, 4, 0, true);
        keys(&mut state, &mut text, ">jj.");
        assert_eq!(text, "    a\n    b\n    c\n    d");
        keys(&mut state, &mut text, "u");
        assert_eq!(text, "    a\n    b\nc\nd");
    }

    #[test]
    fn explicit_one_before_operator_is_retained_for_absolute_line_motion() {
        for command in ["1dG", "d1G"] {
            let mut state = VimState::new();
            let mut text = "one\ntwo\nthree\nfour".to_owned();
            keys(&mut state, &mut text, "3G");
            keys(&mut state, &mut text, command);
            assert_eq!(text, "four", "command={command}");
        }
        let mut state = VimState::new();
        let mut text = "one\ntwo\nthree\nfour".to_owned();
        keys(&mut state, &mut text, "3G1yG");
        assert_eq!(state.register().unwrap().text, "one\ntwo\nthree\n");
        keys(&mut state, &mut text, "3G1>G");
        assert_eq!(text, "    one\n    two\n    three\nfour");
    }

    #[test]
    fn dollar_keeps_end_of_line_intent_across_vertical_motion_and_empty_lines() {
        let mut state = VimState::new();
        let mut text = "a\nlong\n\nlast".to_owned();
        keys(&mut state, &mut text, "$j");
        assert_eq!(state.cursor, 5);
        keys(&mut state, &mut text, "jj");
        assert_eq!(state.cursor, 11);
        keys(&mut state, &mut text, "kk");
        assert_eq!(state.cursor, 5);
        keys(&mut state, &mut text, "0j");
        assert_eq!(state.cursor, 7);

        state = VimState::new();
        text = "a\nlong".into();
        keys(&mut state, &mut text, "v$jd");
        assert_eq!(text, "");
        keys(&mut state, &mut text, "u");
        assert_eq!(text, "a\nlong");
    }
    #[test]
    fn dot_replays_external_widget_change_as_one_transaction() {
        let mut state = VimState::new();
        let mut text = "one two three".to_owned();
        keys(&mut state, &mut text, "ciw");
        text.insert_str(0, "é🙂");
        state.set_cursor(&text, 2);
        state.notify_external_text_change();
        state.finish_insert(&mut text);
        keys(&mut state, &mut text, "w.");
        assert_eq!(text, "é🙂 é🙂 three");
        keys(&mut state, &mut text, "u");
        assert_eq!(text, "é🙂 two three");
        state.handle_key(&mut text, VimKey::Redo);
        assert_eq!(text, "é🙂 é🙂 three");
    }
    #[test]
    fn dot_replays_external_multiline_insert_and_adjacent_identical_text() {
        let mut state = VimState::new();
        let mut text = "abc\nxyz".to_owned();
        keys(&mut state, &mut text, "A");
        text.insert_str(3, "!\n  next");
        state.set_cursor(&text, 11);
        state.finish_insert(&mut text);
        keys(&mut state, &mut text, "G.");
        assert_eq!(text, "abc!\n  next\nxyz!\n  next");
        text = "abc".to_owned();
        state = VimState::new();
        keys(&mut state, &mut text, "ia");
        state.finish_insert(&mut text);
        keys(&mut state, &mut text, "$.");
        assert_eq!(text, "aabac");
    }
    #[test]
    fn counted_insert_and_dot_count_repeat_the_original_typed_text() {
        let mut state = VimState::new();
        let mut text = "x".to_owned();
        keys(&mut state, &mut text, "3iab");
        state.finish_insert(&mut text);
        assert_eq!(text, "abababx");
        keys(&mut state, &mut text, "0.");
        assert_eq!(text, "ababababababx");
        keys(&mut state, &mut text, "02.");
        assert_eq!(text, "ababababababababx");
    }
    #[test]
    fn dot_repeats_visual_selection_and_indent() {
        let mut state = VimState::new();
        let mut text = "one\ntwo\nthree\nfour".to_owned();
        keys(&mut state, &mut text, "Vj>jj.");
        assert_eq!(text, "    one\n    two\n    three\n    four");
        keys(&mut state, &mut text, "u");
        assert_eq!(text, "    one\n    two\nthree\nfour");
        text = "abcdefghi".to_owned();
        state = VimState::new();
        keys(&mut state, &mut text, "v2ld.");
        assert_eq!(text, "ghi");
    }
    #[test]
    fn word_delete_preserves_current_line_break_when_next_word_is_on_next_line() {
        for (initial, command, expected) in [
            ("one\ntwo", "dw", "\ntwo"),
            ("one two\nthree", "d2w", "\nthree"),
            ("one\ntwo\nthree", "d2w", "three"),
            ("one\ntwo three", "d2w", "three"),
        ] {
            let mut state = VimState::new();
            let mut text = initial.to_owned();
            keys(&mut state, &mut text, command);
            assert_eq!(text, expected, "{command} in {initial:?}");
        }
    }
    #[test]
    fn backward_find_operator_excludes_the_original_cursor_character() {
        for (command, expected) in [("$dF,", "foo, barz"), ("$dT,", "foo, bar,z")] {
            let mut state = VimState::new();
            let mut text = "foo, bar, baz".to_owned();
            keys(&mut state, &mut text, command);
            assert_eq!(text, expected, "{command}");
        }
    }
    #[test]
    fn visual_character_paste_of_a_line_register_adds_line_boundaries() {
        let mut state = VimState::new();
        let mut text = "foo bar baz".to_owned();
        state.set_register(Yank {
            text: "qux\n".to_owned(),
            linewise: true,
        });
        keys(&mut state, &mut text, "wviwp");
        assert_eq!(text, "foo \nqux\n baz");
        keys(&mut state, &mut text, "u");
        assert_eq!(text, "foo bar baz");
    }
    #[test]
    fn line_change_keeps_code_indent_and_large_visual_indent_is_bounded() {
        let mut state = VimState::new();
        let mut text = "    old\n    next".to_owned();
        keys(&mut state, &mut text, "cc");
        assert_eq!(text, "    \n    next");
        assert_eq!(state.cursor, 4);
        state.insert_text(&mut text, "new");
        state.finish_insert(&mut text);
        keys(&mut state, &mut text, "u");
        assert_eq!(text, "    old\n    next");
        text = "a\n".repeat(1_000);
        state = VimState::new();
        let initial = text.clone();
        keys(&mut state, &mut text, "VG9999>");
        assert_eq!(text, initial);
    }
    #[test]
    fn external_normal_edit_is_undoable_and_clears_visual_and_pending_state() {
        let mut state = VimState::new();
        let mut text = "original".to_owned();
        keys(&mut state, &mut text, "v2l");
        assert!(state.selection(&text).is_some());
        state.begin_external_edit(&text);
        text = "replaced".to_owned();
        assert_eq!(state.mode, VimMode::Normal);
        assert!(state.selection(&text).is_none());
        assert!(state.pending_command().is_empty());
        assert!(state.can_undo());
        keys(&mut state, &mut text, "u");
        assert_eq!(text, "original");
        state.handle_key(&mut text, VimKey::Redo);
        assert_eq!(text, "replaced");
        keys(&mut state, &mut text, "u3d");
        state.begin_external_edit(&text);
        text = "another".to_owned();
        assert!(!state.can_redo());
        assert!(state.pending_command().is_empty());
        keys(&mut state, &mut text, "u");
        assert_eq!(text, "original");
    }
    #[test]
    fn external_insert_edit_stays_in_one_insert_undo_transaction() {
        let mut state = VimState::new();
        let mut text = "original".to_owned();
        keys(&mut state, &mut text, "iX");
        state.begin_external_edit(&text);
        text.insert_str(1, "completion");
        state.set_cursor(&text, 11);
        state.notify_external_text_change();
        assert!(state.undo.is_empty());
        state.finish_insert(&mut text);
        assert_eq!(state.undo.len(), 1);
        keys(&mut state, &mut text, "u");
        assert_eq!(text, "original");
        state.handle_key(&mut text, VimKey::Redo);
        assert_eq!(text, "Xcompletionoriginal");
    }
    #[test]
    fn word_motions_visit_empty_lines_but_word_end_skips_them() {
        for (initial, command, expected) in [
            ("one\n\n\nthree", "w", (1, 0)),
            ("one\n\n\nthree", "2w", (2, 0)),
            ("one\n\n\nthree", "3w", (3, 0)),
            ("one\n\n\nthree", "Gb", (2, 0)),
            ("one\n\n\nthree", "G2b", (1, 0)),
            ("one\n\n\nthree", "G3b", (0, 0)),
            ("one\n\n\nthree", "2e", (3, 4)),
            ("one\n   \nthree", "w", (2, 0)),
            ("one\n   \nthree", "Gb", (0, 0)),
            ("\n\nthree", "w", (1, 0)),
            ("\n\nthree", "G2b", (0, 0)),
        ] {
            let mut state = VimState::new();
            let mut text = initial.to_owned();
            keys(&mut state, &mut text, command);
            assert_eq!(position(&state, &text), expected, "{initial:?}, {command}");
        }
    }
    #[test]
    fn inner_word_counts_include_whitespace_and_around_word_includes_content() {
        for (initial, command, expected) in [
            ("one two three", "d2iw", "two three"),
            ("one two three", "d3iw", " three"),
            ("one two three", "d2aw", "three"),
            ("one  two three", "3ldiw", "onetwo three"),
            ("one  two three", "3ld2iw", "one three"),
            ("one  two three", "3ldaw", "one three"),
            ("one  two three", "3ld2aw", "one"),
            ("foo.bar baz", "d2iW", "baz"),
        ] {
            let mut state = VimState::new();
            let mut text = initial.to_owned();
            keys(&mut state, &mut text, command);
            assert_eq!(text, expected, "{initial:?}, {command}");
        }
    }
    #[test]
    fn quoted_objects_find_forward_pairs_and_adjacent_quote_spans() {
        for (initial, cursor, command, expected) in [
            ("let s = \"hello\";", 0, "di\"", "let s = \"\";"),
            ("\"one\" middle \"two\"", 7, "di\"", "\"one\"\"two\""),
            ("\"one\" middle \"two\"", 0, "di\"", "\"\" middle \"two\""),
            ("\"one\" middle \"two\"", 4, "di\"", "\"\" middle \"two\""),
            ("pre \"one\" tail", 5, "2di\"", "pre  tail"),
            ("pre \"one\" tail", 5, "2da\"", "pre tail"),
            ("pre \"one\" tail", 13, "di\"", "pre \"one\" tail"),
            ("pre\n\"one\"", 0, "di\"", "pre\n\"one\""),
        ] {
            let mut state = VimState::new();
            let mut text = initial.to_owned();
            state.set_cursor(&text, cursor);
            keys(&mut state, &mut text, command);
            assert_eq!(text, expected, "{initial:?}, {cursor}, {command}");
        }
    }
    #[test]
    fn block_objects_search_forward_and_respect_available_nesting() {
        for (initial, cursor, command, expected) in [
            ("call(value)", 0, "di(", "call()"),
            ("pre\n{value}", 0, "di{", "pre\n{}"),
            ("pre (a (b) c) post", 0, "2di(", "pre (a () c) post"),
            ("pre (a (b) c) post", 8, "2di(", "pre () post"),
            ("pre (a (b) c) post", 8, "3di(", "pre (a (b) c) post"),
            ("pre (a) gap (b)", 0, "2di(", "pre (a) gap (b)"),
            ("pre (a (b) (c) d)", 0, "3di(", "pre (a (b) (c) d)"),
            ("(a) gap (b)", 4, "di(", "(a) gap ()"),
            ("(a) tail", 6, "di(", "(a) tail"),
        ] {
            let mut state = VimState::new();
            let mut text = initial.to_owned();
            state.set_cursor(&text, cursor);
            keys(&mut state, &mut text, command);
            assert_eq!(text, expected, "{initial:?}, {cursor}, {command}");
        }
    }

    #[test]
    fn case_operators_transform_unicode_motions_objects_and_lines_without_yanking() {
        for (initial, command, expected) in [
            ("hello WORLD", "gUw", "HELLO WORLD"),
            ("ÄÖ ßeta", "guw", "äö ßeta"),
            ("aBc DEF", "g~iw", "AbC DEF"),
            ("call(äöß)", "gUi(", "call(ÄÖSS)"),
            ("one\n  two\nthree", "2gUU", "ONE\n  TWO\nthree"),
            ("ONE\n  TWO\nTHREE", "gu2j", "one\n  two\nthree"),
            ("aBc\nDeF", "g~~", "AbC\nDeF"),
            ("one TWO three", "gU2w", "ONE TWO three"),
        ] {
            let mut state = VimState::new();
            let mut text = initial.to_owned();
            let register = Yank {
                text: "keep".into(),
                linewise: false,
            };
            state.set_register(register.clone());
            state.set_yank_register(Some(register.clone()));
            keys(&mut state, &mut text, command);
            assert_eq!(text, expected, "{command}");
            assert_eq!(state.register(), Some(&register), "{command}");
            assert_eq!(state.yank_register(), Some(&register), "{command}");
            assert_eq!(state.mode, VimMode::Normal);
            assert_eq!(state.undo.len(), 1, "{command}");
            keys(&mut state, &mut text, "u");
            assert_eq!(text, initial, "{command}");
            state.handle_key(&mut text, VimKey::Redo);
            assert_eq!(text, expected, "{command}");
        }
    }

    #[test]
    fn case_operator_dot_replaces_motion_counts_and_noop_preserves_redo() {
        let mut state = VimState::new();
        let mut text = "one two three four five".to_owned();
        keys(&mut state, &mut text, "gU2w2w1.");
        assert_eq!(text, "ONE TWO THREE four five");
        assert_eq!(state.undo.len(), 2);
        keys(&mut state, &mut text, "u");
        assert_eq!(text, "ONE TWO three four five");
        keys(&mut state, &mut text, "0gUiw");
        assert!(state.can_redo());
        state.handle_key(&mut text, VimKey::Redo);
        assert_eq!(text, "ONE TWO THREE four five");
    }

    #[test]
    fn case_search_operator_dot_replays_its_prefix_and_keeps_registers() {
        let mut state = VimState::new();
        let mut text = "one END two END three END".to_owned();
        let register = Yank {
            text: "keep".into(),
            linewise: false,
        };
        state.set_register(register.clone());
        keys(&mut state, &mut text, "gU");
        assert!(state.search_operator_pending());
        assert_eq!(state.pending_command(), "gU");
        let action = state.apply_search_motion(&mut text, 4, search_motion("END", false, 1));
        assert!(action.text_changed);
        assert!(action.yanked.is_none());
        assert_eq!(text, "ONE END two END three END");
        keys(&mut state, &mut text, "2w.");
        assert_eq!(text, "ONE END TWO END three END");
        assert_eq!(state.register(), Some(&register));
        assert_eq!(state.undo.len(), 2);
        keys(&mut state, &mut text, "u");
        assert_eq!(text, "ONE END two END three END");
    }

    #[test]
    fn visual_case_and_dot_use_completed_selection_geometry() {
        for command in ["U", "gU"] {
            let mut state = VimState::new();
            let mut text = "äö hello world".to_owned();
            state.set_selection(&text, 0..2);
            keys(&mut state, &mut text, command);
            assert_eq!(text, "ÄÖ hello world", "{command}");
            assert_eq!(state.mode, VimMode::Normal);
            keys(&mut state, &mut text, "w.");
            assert_eq!(text, "ÄÖ HEllo world", "{command}");
            keys(&mut state, &mut text, "u");
            assert_eq!(text, "ÄÖ hello world", "{command}");
        }
        let mut state = VimState::new();
        let mut text = "ONE\nTWO\nTHREE".to_owned();
        keys(&mut state, &mut text, "Vju");
        assert_eq!(text, "one\ntwo\nTHREE");
        assert_eq!(state.undo.len(), 1);
        keys(&mut state, &mut text, "u");
        assert_eq!(text, "ONE\nTWO\nTHREE");
    }

    #[test]
    fn normal_toggle_case_advances_by_transformed_unicode_characters() {
        let mut state = VimState::new();
        let mut text = "ßaB!\ntail".to_owned();
        keys(&mut state, &mut text, "~");
        assert_eq!(text, "SSaB!\ntail");
        assert_eq!(state.cursor, 2);
        keys(&mut state, &mut text, "2.");
        assert_eq!(text, "SSAb!\ntail");
        assert_eq!(state.cursor, 4);
        keys(&mut state, &mut text, "9~");
        assert_eq!(text, "SSAb!\ntail");
        assert_eq!(state.cursor, 4);
        assert_eq!(state.undo.len(), 2);
    }

    #[test]
    fn backward_word_end_and_last_nonblank_motions_support_counts_and_operators() {
        for (initial, cursor, command, expected) in [
            ("one two three", 12, "ge", 6),
            ("one two three", 12, "2ge", 2),
            ("foo.bar baz", 10, "ge", 6),
            ("foo.bar baz", 10, "2ge", 3),
            ("foo.bar baz", 10, "2gE", 0),
            ("one\n\nthree", 9, "ge", 2),
            ("  one  \n two \t", 0, "g_", 4),
            ("  one  \n two \t", 0, "2g_", 11),
        ] {
            let mut state = VimState::new();
            let mut text = initial.to_owned();
            state.set_cursor(&text, cursor);
            keys(&mut state, &mut text, command);
            assert_eq!(state.cursor, expected, "{initial:?}, {command}");
        }
        let mut state = VimState::new();
        let mut text = "one two three".to_owned();
        state.set_cursor(&text, 12);
        keys(&mut state, &mut text, "dge");
        assert_eq!(text, "one tw");
        assert_eq!(state.register().unwrap().text, "o three");
        keys(&mut state, &mut text, "u0dg_");
        assert_eq!(text, "");
    }

    #[test]
    fn line_start_and_absolute_column_motions_handle_counts_unicode_and_empty_rows() {
        let mut state = VimState::new();
        let mut text = "  αβ\n\n   second\n  third".to_owned();
        keys(&mut state, &mut text, "3|");
        assert_eq!(state.cursor, 2);
        state.handle_key(&mut text, VimKey::Enter);
        assert_eq!(position(&state, &text), (1, 0));
        keys(&mut state, &mut text, "+");
        assert_eq!(position(&state, &text), (2, 3));
        keys(&mut state, &mut text, "2-");
        assert_eq!(position(&state, &text), (0, 2));
        keys(&mut state, &mut text, "3_");
        assert_eq!(position(&state, &text), (2, 3));
        keys(&mut state, &mut text, "999|");
        assert_eq!(position(&state, &text), (2, 8));
        keys(&mut state, &mut text, "d-");
        assert_eq!(text, "  αβ\n  third");
        assert!(state.register().unwrap().linewise);
        keys(&mut state, &mut text, "u0d");
        state.handle_key(&mut text, VimKey::Enter);
        assert_eq!(text, "  αβ\n");
    }

    #[test]
    fn joining_crlf_lines_preserves_final_break_and_raw_join_preserves_indentation() {
        for (initial, command, expected) in [
            ("one\r\n  two\r\nthree\r\n", "J", "one two\r\nthree\r\n"),
            ("one\r\n  two\r\nthree\r\n", "3J", "one two three\r\n"),
            ("one\r\n  two\r\nthree", "3gJ", "one  twothree"),
            ("one\n  two\nthree\n", "gJ", "one  two\nthree\n"),
            ("one\r\n\r\ntwo\r\n", "3J", "one two\r\n"),
            ("one\ntwo\r", "J", "one two\r"),
        ] {
            let mut state = VimState::new();
            let mut text = initial.to_owned();
            keys(&mut state, &mut text, command);
            assert_eq!(text, expected, "{initial:?}, {command}");
            assert_eq!(state.cursor, 3, "{command}");
            keys(&mut state, &mut text, "u");
            assert_eq!(text, initial, "{command}");
            state.handle_key(&mut text, VimKey::Redo);
            assert_eq!(text, expected, "{command}");
        }
    }

    #[test]
    fn raw_join_dot_and_visual_selection_join_all_selected_lines() {
        let mut state = VimState::new();
        let mut text = "a\n b\nc\n d\ne\n f".to_owned();
        keys(&mut state, &mut text, "gJj.");
        assert_eq!(text, "a b\nc d\ne\n f");
        assert_eq!(state.undo.len(), 2);
        keys(&mut state, &mut text, "u");
        assert_eq!(text, "a b\nc\n d\ne\n f");

        let mut state = VimState::new();
        let mut text = "a\r\n b\r\n c\r\nd\r\n e\r\n f\r\n".to_owned();
        keys(&mut state, &mut text, "V2jgJ");
        assert_eq!(text, "a b c\r\nd\r\n e\r\n f\r\n");
        keys(&mut state, &mut text, "j.");
        assert_eq!(text, "a b c\r\nd e f\r\n");
        assert_eq!(state.undo.len(), 2);
    }

    fn run(initial: &str, cursor: usize, value: &str) -> (String, usize, VimState) {
        let mut state = VimState::new();
        let mut text = initial.to_owned();
        state.set_cursor(&text, cursor);
        for ch in value.chars() {
            let key = match ch {
                '\u{1b}' => VimKey::Escape,
                '\n' => VimKey::Enter,
                ch => VimKey::Char(ch),
            };
            state.handle_key(&mut text, key);
        }
        let cursor = state.cursor;
        (text, cursor, state)
    }

    #[test]
    fn word_objects_never_cross_line_breaks() {
        for (initial, cursor, command, expected) in [
            ("foo bar\n    baz\n", 4, "daw", "foo\n    baz\n"),
            ("foo\nbar\n", 0, "daw", "\nbar\n"),
            ("a\n\n\nb\n", 2, "diw", "a\n\n\nb\n"),
            ("a\n    foo\n", 2, "diw", "a\nfoo\n"),
            ("a\r\n    foo\r\n", 3, "diw", "a\r\nfoo\r\n"),
            ("foo  \nbar", 0, "daw", "\nbar"),
            ("one two\nthree", 4, "d2aw", "one\nthree"),
            ("one two\nthree", 0, "d5iw", "\nthree"),
            ("x foo\n", 2, "ciwy\u{1b}", "x y\n"),
        ] {
            let (text, _, _) = run(initial, cursor, command);
            assert_eq!(text, expected, "{initial:?}, {cursor}, {command}");
        }
    }

    #[test]
    fn crlf_word_motions_stop_on_empty_lines() {
        for (initial, cursor, command, expected) in [
            ("a\r\n\r\nb", 0, "w", 3),
            ("a\r\n\r\n\r\nb", 0, "2w", 5),
            ("a\r\n\r\nb", 5, "b", 3),
            ("a\n\nb", 0, "w", 2),
        ] {
            let (_, cursor, _) = run(initial, cursor, command);
            assert_eq!(cursor, expected, "{initial:?}, {command}");
        }
    }

    #[test]
    fn final_line_break_terminates_the_last_line_for_line_commands() {
        for (initial, cursor, command, expected) in [
            ("a\nb\n", 2, "J", "a\nb\n"),
            ("a\nb\n", 0, "3J", "a b\n"),
            ("a\nb\n", 2, "dj", "a\n"),
            ("a\nb\n", 2, "2dd", "a\n"),
            ("a\nb\nc\n", 4, "dG", "a\nb\n"),
            ("a\nb\n", 0, "yGP", "a\nb\na\nb\n"),
            ("a\nb\n", 0, "dG", ""),
            ("a\nb\n", 0, "ggVGd", ""),
            ("a\r\nb\r\n", 3, "J", "a\r\nb\r\n"),
            ("a\r\nb\r\n", 0, "3J", "a b\r\n"),
            ("a\r\nb\r\n", 3, "2dd", "a\r\n"),
            ("a\nb", 2, "dd", "a"),
            ("a\nb\n", 0, "5yyjp", "a\nb\na\nb\n"),
        ] {
            let (text, _, _) = run(initial, cursor, command);
            assert_eq!(text, expected, "{initial:?}, {cursor}, {command}");
        }
        let (_, cursor, _) = run("a\n  b\n", 0, "G");
        assert_eq!(cursor, 4);
    }

    #[test]
    fn block_objects_on_their_own_lines_are_linewise_and_paragraph_motions_are_exclusive() {
        for (initial, cursor, command, expected) in [
            ("if x {\n    a;\n}\n", 0, "di{", "if x {\n}\n"),
            ("if x {\n    a;\n}\n", 9, "di{", "if x {\n}\n"),
            ("if x {\n    a;\n}\n", 9, "ci{y\u{1b}", "if x {\ny}\n"),
            ("if x {\r\n    a;\r\n}\r\n", 9, "di{", "if x {\r\n}\r\n"),
            ("  f(\n    a,\n    b\n  )\n", 7, "di(", "  f(\n  )\n"),
            ("if x { a; }\n", 0, "di{", "if x {}\n"),
            ("a b\nc\n\nd", 2, "d}", "a \n\nd"),
            ("a b\nc\n\nd", 0, "d}", "\nd"),
            ("a\nb", 0, "d}", ""),
            ("a\n\nb c\nd", 7, "d{", "a\nd"),
        ] {
            let (text, _, _) = run(initial, cursor, command);
            assert_eq!(text, expected, "{initial:?}, {cursor}, {command}");
        }
        for (initial, cursor, command, expected) in [
            ("a\n\nb c\nd", 5, "{", 2),
            ("a\n\nb\nc\n\nd", 3, "}", 7),
            ("a\nb", 0, "}", 2),
        ] {
            let (_, cursor, _) = run(initial, cursor, command);
            assert_eq!(cursor, expected, "{initial:?}, {command}");
        }
    }

    #[test]
    fn paragraph_objects_select_blocks_and_their_blank_lines() {
        for (initial, cursor, command, expected) in [
            ("a\nb\n\nc\n", 0, "dap", "c\n"),
            ("a\nb\n\nc\n", 0, "dip", "\nc\n"),
            ("a\nb\n\nc\n", 4, "dip", "a\nb\nc\n"),
            ("a\nb\n\nc\n", 4, "dap", "a\nb\n"),
            ("a\n\nc\nd\n", 4, "dap", "a\n"),
            ("a\n\nb\n\nc\n", 0, "d2ap", "c\n"),
            ("a\n\nb\n\nc\n", 0, "d3ip", "\nc\n"),
            ("a\nb\n  \nc", 0, "yipP", "a\nb\na\nb\n  \nc"),
            ("a\nb\n\nc\n", 0, ">ip", "    a\n    b\n\nc\n"),
            ("one\ntwo\n\nx\n", 0, "cipnew\u{1b}", "new\n\nx\n"),
        ] {
            let (text, _, _) = run(initial, cursor, command);
            assert_eq!(text, expected, "{initial:?}, {cursor}, {command}");
        }
        let (_, _, state) = run("a\nb\n\nc\n", 0, "vip");
        assert_eq!(state.mode, VimMode::VisualLine);
        assert_eq!(state.selection("a\nb\n\nc\n"), Some(0..4));
    }

    #[test]
    fn small_vim_divergences_follow_vim() {
        for (initial, cursor, command, expected) in [
            ("f(\n)", 0, "J", "f()"),
            ("f(a,\n  b)", 0, "J", "f(a, b)"),
            ("a\n", 0, "3ox\u{1b}", "a\nx\nx\nx\n"),
            ("a\n", 0, "2Ox\u{1b}", "x\nx\na\n"),
            ("  a\n", 0, "2ox\u{1b}", "  a\n  x\n  x\n"),
            ("a\r\n", 0, "2ox\u{1b}", "a\r\nx\r\nx\r\n"),
            ("Abc dEF\nx", 0, "gugu", "abc def\nx"),
            ("Abc dEF\nx", 0, "gUgU", "ABC DEF\nx"),
            ("Abc dEF\nx", 0, "g~g~", "aBC Def\nx"),
            ("Ab\ncD\nx", 0, "2gUgU", "AB\nCD\nx"),
        ] {
            let (text, _, _) = run(initial, cursor, command);
            assert_eq!(text, expected, "{initial:?}, {cursor}, {command}");
        }
        for (initial, cursor, command, expected) in [
            ("foo bar", 4, "yb", 0),
            ("foo bar", 4, "y0", 0),
            ("foo bar", 4, "2yh", 2),
            ("a\n\nb c", 5, "y{", 2),
            ("foo bar", 0, "yw", 0),
        ] {
            let (text, cursor, _) = run(initial, cursor, command);
            assert_eq!(text, initial);
            assert_eq!(cursor, expected, "{initial:?}, {command}");
        }
        let (text, _, _) = run("a\n", 0, "3ox\u{1b}u");
        assert_eq!(text, "a\n");
        let (text, _, _) = run("a\nb\n", 0, "2ox\u{1b}j.");
        assert_eq!(text, "a\nx\nx\nb\nx\nx\n");
    }

    #[test]
    fn substitute_line_keeps_indentation_and_repeats() {
        let (text, cursor, state) = run("    old\nnext\n", 6, "Snew\u{1b}");
        assert_eq!(text, "    new\nnext\n");
        assert_eq!(cursor, 6);
        assert_eq!(state.mode, VimMode::Normal);
        let (text, _, _) = run("a\nb\nc\n", 0, "2Sx\u{1b}j.");
        assert_eq!(text, "x\nx\n");
        let (text, _, _) = run("a\nb\n", 0, "Sx\u{1b}u");
        assert_eq!(text, "a\nb\n");
    }

    #[test]
    fn reselect_last_visual_area() {
        let (_, _, state) = run("one two three", 4, "viw\u{1b}0gv");
        assert_eq!(state.mode, VimMode::Visual);
        assert_eq!(state.selection("one two three"), Some(4..7));
        let (text, _, _) = run("a\nb\nc\n", 0, "Vjy\u{1b}Ggvd");
        assert_eq!(text, "c\n");
        let (_, _, state) = run("abc", 0, "gv");
        assert_eq!(state.mode, VimMode::Normal);
        let (_, _, state) = run("abcdef", 0, "vl\u{1b}3lvlgv");
        assert_eq!(state.selection("abcdef"), Some(0..2));
        let (_, _, state) = run("abcdef", 0, "vl\u{1b}3lvlgvgv");
        assert_eq!(state.selection("abcdef"), Some(4..6));
    }

    #[test]
    fn scroll_commands_request_alignment_and_keep_the_text() {
        let mut state = VimState::new();
        let mut text = "a\n  b\nc\n".to_owned();
        state.set_cursor(&text, 3);
        for (command, align, cursor) in [
            ("zz", ScrollAlign::Center, 3),
            ("zt", ScrollAlign::Top, 3),
            ("zb", ScrollAlign::Bottom, 3),
            ("z\n", ScrollAlign::Top, 4),
            ("1z.", ScrollAlign::Center, 0),
            ("3z-", ScrollAlign::Bottom, 6),
        ] {
            let mut last = Action::default();
            for ch in command.chars() {
                last = state.handle_key(
                    &mut text,
                    if ch == '\n' {
                        VimKey::Enter
                    } else {
                        VimKey::Char(ch)
                    },
                );
            }
            assert_eq!(last.scroll, Some(align), "{command}");
            assert_eq!(state.cursor, cursor, "{command}");
            assert_eq!(text, "a\n  b\nc\n");
        }
    }

    #[test]
    fn screen_line_motions_use_the_visible_range() {
        let text = "l0\nl1\n  l2\nl3\nl4\nl5\nl6\n";
        // Lines l1 to l4 are fully visible; like the editor's visual rows,
        // the range ends before the last row's newline.
        let visible = 3..16;
        for (command, expected) in [
            ("H", 3),
            ("2H", 8),
            ("L", 14),
            ("2L", 11),
            ("M", 8),
            ("9H", 14),
            ("9L", 3),
        ] {
            let mut state = VimState::new();
            let mut source = text.to_owned();
            state.set_visible_range(Some(visible.clone()));
            keys(&mut state, &mut source, command);
            assert_eq!(state.cursor, expected, "{command}");
        }
        let mut state = VimState::new();
        let mut source = text.to_owned();
        state.set_visible_range(Some(visible));
        state.set_cursor(&source, 3);
        keys(&mut state, &mut source, "dL");
        assert_eq!(source, "l0\nl5\nl6\n");
        // Without a known view, the whole document counts as visible.
        let mut state = VimState::new();
        let mut source = text.to_owned();
        keys(&mut state, &mut source, "L");
        assert_eq!(state.cursor, 20);
        // An empty last visible row is still the bottom screen line.
        let mut state = VimState::new();
        let mut source = "a\n\nb\n".to_owned();
        state.set_visible_range(Some(0..2));
        keys(&mut state, &mut source, "L");
        assert_eq!(state.cursor, 2);
    }

    #[test]
    fn increment_and_decrement_numbers_at_or_after_the_cursor() {
        use VimKey::{Char, Decrement, Increment};
        for (initial, cursor, keys, expected, expected_cursor) in [
            ("x 9 y", 0, vec![Increment], "x 10 y", 3),
            ("x 9 y", 0, vec![Char('5'), Increment], "x 14 y", 3),
            ("x 1 y", 2, vec![Decrement, Decrement], "x -1 y", 3),
            ("a -5", 0, vec![Char('7'), Increment], "a 2", 2),
            ("a-5", 0, vec![Increment], "a-4", 2),
            ("v123", 3, vec![Increment], "v124", 3),
            ("007", 0, vec![Increment], "008", 2),
            ("no digits\n42", 0, vec![Increment], "no digits\n42", 0),
            ("x 1 y", 0, vec![Increment, Char('.')], "x 3 y", 2),
            (
                "x 1 y",
                0,
                vec![Char('3'), Decrement, Char('u')],
                "x 1 y",
                0,
            ),
            ("é 1\r\n", 0, vec![Increment], "é 2\r\n", 2),
        ] {
            let mut state = VimState::new();
            let mut text = initial.to_owned();
            state.set_cursor(&text, cursor);
            for key in keys {
                state.handle_key(&mut text, key);
            }
            assert_eq!(text, expected, "{initial:?}");
            assert_eq!(state.cursor, expected_cursor, "{initial:?}");
        }
    }

    #[test]
    fn unsupported_commands_consume_their_argument_and_report_it() {
        for (command, message) in [
            ("qa", "q"),
            ("@a", "@"),
            ("ma", "m"),
            ("`a", "`"),
            ("'a", "'"),
            ("R", "R"),
            ("ZZ", "Z"),
            ("zf", "zf"),
            ("gq", "gq"),
            ("dq", "dq"),
            ("d'a", "d'"),
        ] {
            let mut state = VimState::new();
            let mut text = "abc def".to_owned();
            let mut messages = Vec::new();
            for ch in command.chars() {
                let action = state.handle_key(&mut text, VimKey::Char(ch));
                assert!(action.consumed, "{command}");
                messages.extend(action.message);
            }
            assert_eq!(
                messages,
                vec![format!("Not supported in vEdit: {message}")],
                "{command}"
            );
            assert_eq!(text, "abc def");
            assert_eq!(state.pending_command(), "");
            keys(&mut state, &mut text, "x");
            assert_eq!(
                text, "bc def",
                "{command} must not swallow the next command"
            );
        }
        let mut state = VimState::new();
        state.handle_key(&mut "abc".to_owned(), VimKey::Char('m'));
        assert!(state.expects_literal_character());
    }

    #[test]
    fn bracket_pairs_for_highlighting_respect_the_caret_kind_and_window() {
        let text: Vec<char> = "f(a, [b], {c})🦀)".chars().collect();
        assert_eq!(bracket_pair(&text, 0, 1, false), Some((1, 13)));
        assert_eq!(bracket_pair(&text, 0, 13, false), Some((13, 1)));
        assert_eq!(bracket_pair(&text, 0, 5, false), Some((5, 7)));
        // A block cursor after a bracket shows nothing; a caret between
        // characters pairs the bracket before it.
        assert_eq!(bracket_pair(&text, 0, 2, false), None);
        assert_eq!(bracket_pair(&text, 0, 2, true), Some((1, 13)));
        assert_eq!(bracket_pair(&text, 0, 14, true), Some((13, 1)));
        // An unmatched bracket and one outside the window have no partner.
        assert_eq!(bracket_pair(&text, 0, 15, false), None);
        assert_eq!(bracket_pair(&text[5..], 5, 5, false), Some((5, 7)));
        assert_eq!(bracket_pair(&text[3..], 3, 13, false), None);
        assert_eq!(bracket_pair(&text[3..], 3, 1, true), None);
        // `%` still finds the next bracket on the line and its partner.
        let mut state = VimState::new();
        let mut source = "f(a, [b])".to_owned();
        keys(&mut state, &mut source, "%");
        assert_eq!(state.cursor, 8);
    }
}

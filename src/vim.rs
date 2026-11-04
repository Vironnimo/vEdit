//! Unicode-safe modal editing. Cursors and selection ranges use Unicode
//! character indices, matching egui's text cursor. Keep a state per document.
use std::ops::Range;

mod block;
mod commands;
mod geometry;
mod history;
mod marks;
mod multicursor;
mod navigation;
mod operators;
mod paste;
mod registers;

pub use registers::RegisterBank;
mod replace;

use geometry::{
    bracket_partner, change_word_end, char_boundary, char_to_byte, first_nonblank, is_bracket,
    last_line, last_line_row, last_nonblank, line_at_cursor, line_ending, lines, matching_bracket,
    normal_cursor, paragraph, paragraph_object, relative_line, remove_last_line_ending,
    repeat_motion, row_for_cursor, text_object, word_backward, word_end, word_end_backward,
    word_forward,
};
pub use history::MacroHistory;
use history::insert_delta;
use paste::{repeated_text, surround_range};
pub use registers::RegisterValue;

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
    VisualBlock,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VimKey {
    Char(char),
    Escape,
    Enter,
    Backspace,
    Tab,
    /// Toggle a rectangular visual selection.
    VisualBlock,
    /// Map normal-mode Ctrl+R to this key.
    Redo,
    /// Vim's Ctrl+A: add the count to the number at or after the cursor.
    Increment,
    /// Vim's Ctrl+X: subtract the count from the number at or after the cursor.
    Decrement,
    /// Native editing motions, used while replacing text.
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    Delete,
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
    /// Width in text columns for a blockwise register; rows are newline-separated.
    pub block_width: Option<usize>,
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

/// Window-owned file-mark identity is resolved outside the editing model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlobalMarkRequest {
    Set(char),
    Jump {
        name: char,
        linewise: bool,
        keep_jumps: bool,
        operator: bool,
    },
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
    Register {
        count: usize,
        explicit: bool,
    },
    InsertRegister,
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
    /// The argument of an unsupported command such as `q`. It is
    /// swallowed so that the user's next keys do not run as commands.
    Ignore,
    /// `[` or `]`: the editor handles `[d` / `]d` (previous / next problem);
    /// other bracket commands are unsupported and swallow their argument.
    Bracket(char),
    SetMark,
    Mark {
        operator: Option<Operator>,
        linewise: bool,
        keep_jumps: bool,
    },
}

/// The most recent Visual area, for `gv`.
#[derive(Clone, Copy, Debug)]
struct LastVisual {
    mode: VimMode,
    anchor: usize,
    cursor: usize,
    to_line_end: bool,
    block_columns: Option<(usize, usize)>,
}

#[derive(Clone, Debug)]
struct Snapshot {
    text: String,
    cursor: usize,
    multi: crate::multicursor::State,
    marks: [Option<usize>; 26],
    global_marks: [Option<usize>; 26],
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
    blockwise: bool,
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
    pub multi: crate::multicursor::State,
    multi_cursors: Vec<multicursor::Cursor>,
    visual_anchor: Option<usize>,
    pending: Option<Pending>,
    count: usize,
    preferred_column: Option<usize>,
    last_find: Option<Find>,
    selected_register: Option<char>,
    register_count: Option<(usize, bool)>,
    command_keys: Vec<VimKey>,
    surround_search: Option<SearchMotion>,
    motion_epoch: u64,
    visual_to_line_end: bool,
    last_visual: Option<LastVisual>,
    block_columns: Option<(usize, usize)>,
    previous_context: Option<usize>,
    change_index: Option<usize>,
    replace: Option<replace::Session>,
}

#[derive(Clone, Debug)]
pub struct VimState {
    pub mode: VimMode,
    pub multi: crate::multicursor::State,
    multi_cursors: Vec<multicursor::Cursor>,
    /// Insert mode: insertion boundary. Other modes: current character.
    pub cursor: usize,
    register: Option<Yank>,
    yank_register: Option<Yank>,
    register_bank: RegisterBank,
    selected_register: Option<char>,
    register_count: Option<(usize, bool)>,
    collecting_registers: bool,
    register_capture: Option<(Option<char>, bool, bool)>,
    numbered_delete: bool,
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
    pending_global_repeat: Option<RepeatChange>,
    insert_replay: Option<InsertReplay>,
    replaying: bool,
    motion_epoch: u64,
    visual_to_line_end: bool,
    last_visual: Option<LastVisual>,
    block_columns: Option<(usize, usize)>,
    block_insert: Option<block::BlockInsert>,
    /// Line break and indentation that separate the copies of a counted `o`.
    insert_line_prefix: Option<String>,
    replace: Option<replace::Session>,
    /// Character range of the lines the view currently shows, for H/M/L.
    visible: Option<Range<usize>>,
    scroll_request: Option<ScrollAlign>,
    message: Option<String>,
    marks: marks::Marks,
    global_mark_request: Option<GlobalMarkRequest>,
    resolving_global_mark: bool,
    previous_context: Option<usize>,
    change_index: Option<usize>,
    jump_origins: Vec<usize>,
    external_edit: bool,
    command_failed: bool,
}

impl Default for VimState {
    fn default() -> Self {
        Self {
            mode: VimMode::Normal,
            multi: crate::multicursor::State::default(),
            multi_cursors: Vec::new(),
            cursor: 0,
            register: None,
            yank_register: None,
            register_bank: RegisterBank::default(),
            selected_register: None,
            register_count: None,
            collecting_registers: false,
            register_capture: None,
            numbered_delete: false,
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
            pending_global_repeat: None,
            insert_replay: None,
            replaying: false,
            motion_epoch: 0,
            visual_to_line_end: false,
            last_visual: None,
            block_columns: None,
            block_insert: None,
            insert_line_prefix: None,
            replace: None,
            visible: None,
            scroll_request: None,
            message: None,
            marks: marks::Marks::default(),
            global_mark_request: None,
            resolving_global_mark: false,
            previous_context: None,
            change_index: None,
            jump_origins: Vec::new(),
            external_edit: false,
            command_failed: false,
        }
    }
}

impl VimState {
    pub fn new() -> Self {
        Self::default()
    }

    #[cfg(test)]
    pub fn register(&self) -> Option<&Yank> {
        self.register.as_ref()
    }

    pub fn set_register(&mut self, yank: Yank) {
        self.register_bank.set_text('"', yank.clone());
        self.register = Some(yank);
    }

    pub fn yank_register(&self) -> Option<&Yank> {
        self.yank_register.as_ref()
    }

    pub fn set_yank_register(&mut self, yank: Option<Yank>) {
        if let Some(yank) = &yank {
            self.register_bank.set_text('0', yank.clone());
        } else {
            self.register_bank.clear('0');
        }
        self.yank_register = yank;
    }

    pub fn set_indentation(&mut self, width: usize, tabs: bool) {
        self.indent_width = width.clamp(1, 16);
        self.indent_with_tabs = tabs;
    }

    /// Native Insert edits are mapped together when their transaction ends.
    pub fn notify_external_text_change(&mut self, text: &str) {
        if std::mem::take(&mut self.external_edit)
            && let Some(before) = self.undo.last()
        {
            let (before, origin) = (before.text.clone(), before.cursor);
            self.track_marks(&before, text, true, origin);
        }
    }

    pub fn handle_key(&mut self, text: &mut String, key: VimKey) -> Action {
        self.numbered_delete = false;
        self.notify_external_text_change(text);
        if self.multi.active() {
            return self.handle_multi_key(text, key);
        }
        self.yanked_ranges.clear();
        if matches!(
            self.mode,
            VimMode::Visual | VimMode::VisualLine | VimMode::VisualBlock
        ) && self.pending.is_none()
            && matches!(key, VimKey::Char('d' | 'x'))
        {
            return self.cut_visual(text);
        }
        let old_cursor = self.cursor;
        let old_mode = self.mode;
        let old_pending = self.pending;
        let old_count = self.count;
        let replace_keys = (self.is_replacing() && key == VimKey::Escape)
            .then(|| self.replace_change_keys())
            .flatten();
        let selecting_register = matches!(self.pending, Some(Pending::Register { .. }));
        let visual_change = self.visual_change_geometry(text).and_then(|visual| {
            let keys = match (self.pending, key) {
                (Some(Pending::Replace(_)), VimKey::Char(_) | VimKey::Enter | VimKey::Tab) => {
                    vec![VimKey::Char('r'), key]
                }
                (
                    None,
                    VimKey::Char(
                        'c' | 's' | 'I' | 'A' | '>' | '<' | 'p' | 'P' | 'J' | 'u' | 'U' | '~',
                    ),
                ) => {
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
                (Some(Pending::SurroundAdd(..)), VimKey::Char(_)) => {
                    vec![VimKey::Char('S'), key]
                }
                _ => return None,
            };
            let mut keys = keys;
            if let Some(name) = self.selected_register {
                keys.splice(0..0, [VimKey::Char('"'), VimKey::Char(name)]);
            }
            Some((visual, keys))
        });
        let insert_changed = old_mode == VimMode::Insert
            && (self
                .insert_origin
                .as_ref()
                .is_some_and(|origin| origin.text != *text)
                || self
                    .replace
                    .as_ref()
                    .is_some_and(|session| session.changed()));
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
            VimMode::Normal | VimMode::Visual | VimMode::VisualLine | VimMode::VisualBlock => {
                self.handle_normal_key(text, key)
            }
        };
        if self.mode == VimMode::VisualBlock {
            self.sync_block_cursor(text, old_cursor);
        }
        self.clamp_cursor(text);
        if !self.yanked_ranges.is_empty() && !text_changed {
            self.remember_yank();
        }
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
                if let Some(keys) = replace_keys.filter(|_| insert_changed || text_changed) {
                    self.insert_replay = None;
                    self.last_change = Some(RepeatChange {
                        keys,
                        insert: None,
                        search: None,
                        visual: None,
                    });
                } else if let Some(replay) = self.insert_replay.take()
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
            if self.mode == VimMode::Normal
                && self.pending.is_none()
                && self.count == 0
                && !selecting_register
            {
                self.command_keys.clear();
                self.surround_search = None;
                self.selected_register = None;
                self.register_count = None;
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
        let targeted_motion = matches!(old_pending, Some(Pending::Find { .. }))
            || old_pending.is_none()
                && matches!(
                    key,
                    VimKey::Char(
                        'h' | 'j'
                            | 'k'
                            | 'l'
                            | 'w'
                            | 'W'
                            | 'b'
                            | 'B'
                            | 'e'
                            | 'E'
                            | '%'
                            | ';'
                            | ','
                            | '{'
                            | '}'
                    )
                );
        if old_mode != VimMode::Insert
            && targeted_motion
            && self.cursor == old_cursor
            && !text_changed
        {
            self.command_failed = true;
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

#[cfg(test)]
mod tests;

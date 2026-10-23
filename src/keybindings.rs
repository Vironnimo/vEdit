//! One source of truth for user shortcuts, dispatch, persistence and visible hints.
use super::*;
use std::collections::BTreeMap;

pub(super) type Bindings = BTreeMap<String, Option<crate::cli::ShortcutBinding>>;

/// Contexts that can own the same chord. Source subcontexts overlap except for
/// ordinary text entry and cursor placement, where Enter has distinct meaning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Scope {
    Global,
    Source,
    SourceEditing,
    CursorPlacement,
    Multicursor,
    Explorer,
    ContextMenu,
    Review,
    ReviewComment,
}

impl Scope {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Global => "Workspace",
            Self::Source => "Source editor",
            Self::SourceEditing => "Source editing · outside cursor placement",
            Self::CursorPlacement => "Cursor placement",
            Self::Multicursor => "Multicursor editing",
            Self::Explorer => "Explorer",
            Self::ContextMenu => "Explorer, Git and Project tabs",
            Self::Review => "Change review",
            Self::ReviewComment => "Review comment editor",
        }
    }

    fn source(self) -> bool {
        matches!(
            self,
            Self::Source | Self::SourceEditing | Self::CursorPlacement | Self::Multicursor
        )
    }

    fn overlaps(self, other: Self) -> bool {
        self == Self::Global
            || other == Self::Global
            || self == other
            || matches!(
                (self, other),
                (Self::ContextMenu, Self::Explorer) | (Self::Explorer, Self::ContextMenu)
            )
            || self.source()
                && other.source()
                && !matches!(
                    (self, other),
                    (Self::SourceEditing, Self::CursorPlacement)
                        | (Self::CursorPlacement, Self::SourceEditing)
                )
    }
}

macro_rules! actions {
    ($($name:ident, $id:literal, $label:literal, $group:literal, [$($key:literal),*];)*) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub(super) enum Action { $($name,)* }
        pub(super) const COMMANDS: &[(&str, &str)] = &[$(($label, ""),)*];
        impl Action {
            pub(super) const ALL: &'static [Self] = &[$(Self::$name,)*];
            pub(super) fn id(self) -> &'static str { match self { $(Self::$name => $id,)* } }
            pub(super) fn label(self) -> &'static str { match self { $(Self::$name => $label,)* } }
            fn group(self) -> &'static str { match self { $(Self::$name => $group,)* } }
            fn defaults(self) -> &'static [&'static str] { match self { $(Self::$name => &[$($key),*],)* } }
        }
    }
}

actions! {
    CommandPalette, "command_palette", "Command palette", "Navigation", ["Ctrl+Shift+P"];
    QuickOpen, "quick_open", "Go to file", "Navigation", ["Ctrl+P"];
    SwitchProject, "switch_project", "Switch project", "Navigation", ["Ctrl+Alt+P"];
    PreviousProject, "previous_project", "Previous project", "Navigation", ["Ctrl+Alt+PageUp"];
    NextProject, "next_project", "Next project", "Navigation", ["Ctrl+Alt+PageDown"];
    OpenProject, "open_project", "Add project folder", "Files", ["Ctrl+Shift+O"];
    OpenFile, "open_file", "Open file", "Files", ["Ctrl+O"];
    OpenRecent, "open_recent", "Open recent", "Files", [];
    NewFile, "new_file", "New file", "Files", ["Ctrl+N"];
    Save, "save", "Save current file", "Files", ["Ctrl+S"];
    SaveAll, "save_all", "Save all files", "Files", ["Ctrl+Shift+S"];
    SaveAs, "save_as", "Save as...", "Files", [];
    CloseTab, "close_tab", "Close current tab", "Files", ["Ctrl+W"];
    ReopenTab, "reopen_tab", "Reopen closed tab", "Files", ["Ctrl+Shift+T"];
    PreviousTab, "previous_tab", "Previous tab", "Navigation", ["Ctrl+Shift+Tab"];
    NextTab, "next_tab", "Next tab", "Navigation", ["Ctrl+Tab"];
    Find, "find", "Find in file", "Search", ["Ctrl+F"];
    Replace, "replace", "Replace in file", "Search", ["Ctrl+H"];
    SearchProject, "search_project", "Search project", "Search", ["Ctrl+Shift+F"];
    FindPrevious, "find_previous", "Find previous", "Search", ["Shift+F3"];
    FindNext, "find_next", "Find next", "Search", ["F3"];
    GoToLine, "go_to_line", "Go to line", "Navigation", ["Ctrl+G"];
    Definition, "definition", "Go to definition", "Language", ["F12"];
    Completion, "completion", "Show completions", "Language", ["Ctrl+Space"];
    Hover, "show_hover", "Show hover", "Language", [];
    FindReferences, "find_references", "Find references", "Language", ["Shift+F12"];
    DocumentSymbol, "go_to_symbol", "Go to symbol in file", "Language", [];
    WorkspaceSymbol, "go_to_workspace_symbol", "Go to symbol in workspace", "Language", ["Ctrl+T"];
    RestartLanguage, "restart_language", "Restart language analysis", "Language", [];
    LanguageSupport, "language_support", "Language support", "Language", [];
    Problems, "problems", "Show problems", "Layout", ["Ctrl+Shift+M"];
    AgentMark, "agent_mark", "Mark selection for agent", "Navigation", ["Ctrl+Shift+A"];
    AgentMarks, "agent_marks", "Agent marks", "Navigation", ["Ctrl+Alt+Shift+A"];
    AgentNotifications, "agent_notifications", "Agent notifications", "Navigation", [];
    MarkdownPreview, "markdown_preview", "Markdown source / preview", "Layout", ["Ctrl+Alt+M"];
    SplitRight, "split_right", "Split right", "Layout", ["Ctrl+Backslash"];
    SplitDown, "split_down", "Split down", "Layout", ["Ctrl+Shift+Backslash"];
    ClosePane, "close_pane", "Close pane", "Layout", ["Ctrl+Alt+W"];
    FocusExplorer, "focus_explorer", "Focus Explorer", "Focus", ["Ctrl+Shift+E"];
    FocusEditor, "focus_editor", "Focus editor", "Focus", ["Ctrl+1"];
    FocusGit, "focus_git", "Focus Git", "Focus", ["Ctrl+Shift+G"];
    FocusOutput, "focus_output", "Focus output", "Focus", ["Ctrl+Alt+O"];
    FocusNextRegion, "focus_next_region", "Next region", "Focus", ["F6"];
    FocusPreviousRegion, "focus_previous_region", "Previous region", "Focus", ["Shift+F6"];
    FocusLeft, "focus_left", "Focus left", "Focus", ["Ctrl+Alt+ArrowLeft"];
    FocusRight, "focus_right", "Focus right", "Focus", ["Ctrl+Alt+ArrowRight"];
    FocusUp, "focus_up", "Focus above", "Focus", ["Ctrl+Alt+ArrowUp"];
    FocusDown, "focus_down", "Focus below", "Focus", ["Ctrl+Alt+ArrowDown"];
    FocusNextPane, "focus_next_pane", "Next editor pane", "Focus", [];
    FocusPreviousPane, "focus_previous_pane", "Previous editor pane", "Focus", [];
    MoveTabLeft, "move_tab_left", "Move tab left", "Layout", ["Ctrl+Alt+Shift+ArrowLeft"];
    MoveTabRight, "move_tab_right", "Move tab right", "Layout", ["Ctrl+Alt+Shift+ArrowRight"];
    MoveTabUp, "move_tab_up", "Move tab up", "Layout", ["Ctrl+Alt+Shift+ArrowUp"];
    MoveTabDown, "move_tab_down", "Move tab down", "Layout", ["Ctrl+Alt+Shift+ArrowDown"];
    Review, "review", "Show agent review", "Layout", [];
    SourceControl, "source_control", "Show source control", "Layout", [];
    ReviewWorkingTree, "review_working_tree", "Review working tree changes", "Git", [];
    ReviewBranch, "review_branch", "Review branch against main", "Git", [];
    Explorer, "explorer", "Show explorer", "Layout", [];
    ToggleLeftSidebar, "toggle_left_sidebar", "Toggle left sidebar", "Layout", ["Ctrl+Alt+E", "Ctrl+B"];
    ToggleRightSidebar, "toggle_right_sidebar", "Toggle right sidebar", "Layout", ["Ctrl+Alt+B"];
    ExplorerGitLayout, "explorer_git_layout", "Explorer left, Git right", "Layout", [];
    ToggleOutput, "toggle_output", "Toggle output", "Layout", ["Ctrl+Backtick"];
    CheckProject, "check_project", "Check project", "Run", ["F5"];
    TestProject, "test_project", "Run tests", "Run", ["Alt+F6"];
    BuildProject, "build_project", "Build project", "Run", [];
    RunFile, "run_file", "Run active file", "Run", ["Ctrl+F5"];
    RunApplication, "run_application", "Run application", "Run", [];
    StopApplication, "stop_application", "Stop application", "Run", [];
    ConfigureTasks, "configure_tasks", "Configure project tasks", "Run", [];
    Terminal, "terminal", "Open terminal", "Run", [];
    ToggleWrap, "toggle_wrap", "Toggle word wrap", "Editing", ["Alt+Z"];
    ToggleVim, "toggle_vim", "Toggle Vim motions", "Editing", [];
    ToggleComment, "toggle_comment", "Toggle line comment", "Editing", ["Ctrl+Slash", "Ctrl+Shift+Slash"];
    DuplicateLine, "duplicate_line", "Duplicate line / selection", "Editing", ["Alt+Shift+D"];
    DeleteLine, "delete_line", "Delete line / selection", "Editing", ["Ctrl+Shift+K"];
    InsertLineAbove, "insert_line_above", "Insert line above", "Editing", ["Ctrl+Shift+Enter"];
    InsertLineBelow, "insert_line_below", "Insert line below", "Editing", ["Ctrl+Enter"];
    MoveLineUp, "move_line_up", "Move line / selection up", "Editing", ["Alt+ArrowUp"];
    MoveLineDown, "move_line_down", "Move line / selection down", "Editing", ["Alt+ArrowDown"];
    InsertNewline, "insert_newline", "Insert newline", "Editing", ["Enter", "Shift+Enter"];
    Indent, "indent", "Indent / insert tab", "Editing", ["Tab"];
    Outdent, "outdent", "Outdent", "Editing", ["Shift+Tab"];
    Undo, "undo", "Undo", "Editing", ["Ctrl+Z"];
    Redo, "redo", "Redo", "Editing", ["Ctrl+Y", "Ctrl+Shift+Z"];
    AddCursorAbove, "add_cursor_above", "Add cursor above", "Multicursor", ["Alt+Shift+ArrowUp"];
    AddCursorBelow, "add_cursor_below", "Add cursor below", "Multicursor", ["Alt+Shift+ArrowDown"];
    MarkCursor, "mark_cursor", "Mark cursor and move independently", "Multicursor", ["Ctrl+Alt+Enter"];
    SelectNextOccurrence, "select_next_occurrence", "Add next occurrence", "Multicursor", ["Ctrl+D"];
    SelectAllOccurrences, "select_all_occurrences", "Select all occurrences", "Multicursor", ["Ctrl+Shift+L"];
    SkipOccurrence, "skip_occurrence", "Skip current occurrence", "Multicursor", ["Ctrl+Shift+D"];
    CursorUndo, "cursor_undo", "Undo last cursor selection", "Multicursor", ["Ctrl+U"];
    CursorsAtLineEnds, "cursors_at_line_ends", "Cursors at selected line ends", "Multicursor", ["Alt+Shift+I"];
    FinishCursorPlacement, "finish_cursor_placement", "Finish cursor placement", "Multicursor", ["Enter"];
    ExitMulticursor, "exit_multicursor", "Exit multicursor editing", "Multicursor", ["Escape"];
    ExplorerRename, "explorer_rename", "Rename Explorer item", "Explorer", ["F2"];
    ExplorerDelete, "explorer_delete", "Delete Explorer item", "Explorer", ["Delete"];
    ContextMenu, "context_menu", "Open context menu", "Navigation", ["Shift+F10"];
    ReviewNextFile, "review_next_file", "Next review file", "Change review", ["N", "CloseBracket"];
    ReviewPreviousFile, "review_previous_file", "Previous review file", "Change review", ["P", "OpenBracket"];
    ReviewSeenNext, "review_seen_next", "Mark review file seen and continue", "Change review", ["Space"];
    ReviewOpenFile, "review_open_file", "Open review file", "Change review", ["O"];
    ReviewComment, "review_comment", "Comment on review selection", "Change review", ["C"];
    ReviewSubmitComment, "review_submit_comment", "Submit review comment", "Change review", ["Ctrl+Enter"];
    ToggleBookmark, "toggle_bookmark", "Toggle bookmark", "Navigation", [];
    Bookmarks, "bookmarks", "Bookmarks", "Navigation", [];
    ScreenJump, "screen_jump", "Jump to visible text", "Navigation", [];
    NavigateBack, "navigate_back", "Go back", "Navigation", ["Alt+ArrowLeft"];
    NavigateForward, "navigate_forward", "Go forward", "Navigation", ["Alt+ArrowRight"];
    NextProblem, "next_problem", "Next problem", "Language", ["F8"];
    PreviousProblem, "previous_problem", "Previous problem", "Language", ["Shift+F8"];
    Reload, "reload", "Reload file from disk", "Files", [];
    CompareWithSaved, "compare_with_saved", "Compare with saved", "Files", [];
    Settings, "settings", "Settings", "Preferences", ["Ctrl+Comma"];
    KeyboardShortcuts, "keyboard_shortcuts", "Keyboard shortcuts", "Preferences", [];
    ZoomIn, "zoom_in", "Zoom in", "Preferences", ["Ctrl+Plus", "Ctrl+Equals", "Ctrl+Shift+Plus", "Ctrl+Shift+Equals"];
    ZoomOut, "zoom_out", "Zoom out", "Preferences", ["Ctrl+Minus"];
    ZoomReset, "zoom_reset", "Reset zoom", "Preferences", ["Ctrl+0"];
}

impl Action {
    pub(super) fn palette_visible(self) -> bool {
        !matches!(self, Self::ContextMenu | Self::ReviewSubmitComment)
    }
    pub(super) fn scope(self) -> Scope {
        match self {
            Self::InsertNewline => Scope::SourceEditing,
            Self::FinishCursorPlacement => Scope::CursorPlacement,
            Self::ExitMulticursor | Self::CursorUndo => Scope::Multicursor,
            Self::ExplorerRename | Self::ExplorerDelete => Scope::Explorer,
            Self::ContextMenu => Scope::ContextMenu,
            Self::ReviewNextFile
            | Self::ReviewPreviousFile
            | Self::ReviewSeenNext
            | Self::ReviewOpenFile
            | Self::ReviewComment => Scope::Review,
            Self::ReviewSubmitComment => Scope::ReviewComment,
            action if action.multicursor() || action.editor_command().is_some() => Scope::Source,
            _ => Scope::Global,
        }
    }

    pub(super) fn editor_command(self) -> Option<editor_input::Command> {
        use editor_input::Command;
        Some(match self {
            Self::Undo => Command::Undo,
            Self::Redo => Command::Redo,
            Self::ToggleComment => Command::Comment,
            Self::DuplicateLine => Command::Duplicate,
            Self::DeleteLine => Command::DeleteLine,
            Self::InsertLineAbove => Command::InsertLine(true),
            Self::InsertLineBelow => Command::InsertLine(false),
            Self::MoveLineUp => Command::MoveLine(false),
            Self::MoveLineDown => Command::MoveLine(true),
            Self::InsertNewline => Command::Newline,
            Self::Indent => Command::Indent(false),
            Self::Outdent => Command::Indent(true),
            _ => return None,
        })
    }
    pub(super) fn multicursor(self) -> bool {
        matches!(
            self,
            Self::AddCursorAbove
                | Self::AddCursorBelow
                | Self::MarkCursor
                | Self::SelectNextOccurrence
                | Self::SelectAllOccurrences
                | Self::SkipOccurrence
                | Self::CursorUndo
                | Self::CursorsAtLineEnds
                | Self::FinishCursorPlacement
                | Self::ExitMulticursor
        )
    }
    pub(super) fn from_id(id: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|action| action.id() == id)
    }
    pub(super) fn from_label(label: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|action| action.label() == label)
    }
    fn context_hint(self) -> &'static str {
        match self {
            Self::Find => {
                "Default Ctrl+F scrolls a page in Vim Normal mode and opens Find in Insert. A custom binding opens Find in either mode."
            }
            Self::NewFile => {
                "Default Ctrl+N requests completions in Vim Insert mode. A custom binding always creates a new file."
            }
            Self::OpenFile => {
                "Default Ctrl+O goes back in Vim Normal mode, like Vim's jump list. A custom binding always opens a file."
            }
            Self::NavigateBack | Self::NavigateForward => {
                "History of this editor pane: other documents, go to definition, search results, bookmarks, problems and jumps of more than 10 lines. The mouse's back and forward buttons and, in Vim Normal mode, Ctrl+O / Ctrl+I work too."
            }
            Self::NextProblem | Self::PreviousProblem => {
                "Errors and warnings of the active file first, then the next file with problems. In Vim Normal mode also ]d / [d."
            }
            Self::ToggleLeftSidebar => {
                "Custom bindings work in every editor mode. The default Ctrl+B alias remains page-back in Vim Normal mode."
            }
            Self::MarkCursor => {
                "Keep this selection in place and move only the active cursor with arrows, Home/End or Page Up/Down. Invoke again to mark another position. Finish cursor placement starts editing at every position; Exit multicursor editing returns to one cursor."
            }
            Self::Undo
            | Self::Redo
            | Self::ToggleComment
            | Self::DuplicateLine
            | Self::DeleteLine
            | Self::InsertLineAbove
            | Self::InsertLineBelow
            | Self::MoveLineUp
            | Self::MoveLineDown => {
                "Applies to the focused source editor. Plain text fields keep their native editing controls."
            }
            Self::ZoomIn | Self::ZoomOut | Self::ZoomReset => {
                "Works throughout the interface, including Settings, except while recording another shortcut."
            }
            _ => "Works in the workspace when a dialog is not handling keyboard input.",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stroke {
    key: Key,
    ctrl: bool,
    alt: bool,
    shift: bool,
}

pub(super) fn modifier_key(key: Key) -> bool {
    matches!(
        key,
        Key::ControlLeft
            | Key::ControlRight
            | Key::ShiftLeft
            | Key::ShiftRight
            | Key::AltLeft
            | Key::AltRight
            | Key::SuperLeft
            | Key::SuperRight
    )
}

impl Stroke {
    fn parse(value: &str) -> Result<Self, String> {
        let mut stroke = Self {
            key: Key::Space,
            ctrl: false,
            alt: false,
            shift: false,
        };
        let mut key = None;
        // A literal '+' is itself a key: Ctrl++ and Ctrl+Shift++ are familiar
        // spellings of the same chords that the recorder labels with Plus.
        let value = value.trim();
        let value = if value == "+" {
            "Plus".to_owned()
        } else if let Some(prefix) = value.strip_suffix("++") {
            format!("{prefix}+Plus")
        } else {
            value.to_owned()
        };
        for part in value.split('+').map(str::trim) {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" | "cmd" | "command" => {
                    if stroke.ctrl {
                        return Err("Ctrl appears twice".into());
                    }
                    stroke.ctrl = true;
                }
                "alt" => {
                    if stroke.alt {
                        return Err("Alt appears twice".into());
                    }
                    stroke.alt = true;
                }
                "shift" => {
                    if stroke.shift {
                        return Err("Shift appears twice".into());
                    }
                    stroke.shift = true;
                }
                _ => {
                    if key.is_some() {
                        return Err("Use one key with optional Ctrl, Alt and Shift".into());
                    }
                    let normalized = if part.len() == 1 {
                        part.to_ascii_uppercase()
                    } else {
                        part.to_owned()
                    };
                    key = Key::from_name(&normalized).or_else(|| {
                        Key::ALL.iter().copied().find(|key| {
                            key.name().eq_ignore_ascii_case(part)
                                || key.symbol_or_name().eq_ignore_ascii_case(part)
                                || format!("{key:?}").eq_ignore_ascii_case(part)
                        })
                    });
                    if key.is_none() {
                        return Err(format!(
                            "Unknown key '{part}'. Record the shortcut using its button."
                        ));
                    }
                }
            }
        }
        stroke.key = key.ok_or("A shortcut needs a key, for example Ctrl+Shift+S")?;
        Ok(stroke)
    }
    fn event(event: &egui::Event) -> Option<Self> {
        match event {
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } if !modifier_key(*key) && !modifiers.mac_cmd => Some(Self {
                key: *key,
                ctrl: modifiers.ctrl || modifiers.command,
                alt: modifiers.alt,
                shift: modifiers.shift,
            }),
            _ => None,
        }
    }
    fn label(self) -> String {
        let mut words = Vec::new();
        if self.ctrl {
            words.push("Ctrl");
        }
        if self.alt {
            words.push("Alt");
        }
        if self.shift {
            words.push("Shift");
        }
        words.push(self.key.name());
        words.join("+")
    }
    fn safe(self, action: Action) -> Result<(), String> {
        if modifier_key(self.key) {
            return Err("Press a regular key together with the modifiers.".into());
        }
        if matches!(self.key, Key::Copy | Key::Cut | Key::Paste)
            || self.ctrl && matches!(self.key, Key::C | Key::X | Key::V)
        {
            return Err(
                "Ctrl+C/X/V (with any modifiers) are reserved for the system clipboard.".into(),
            );
        }
        let contextual_plain = !self.ctrl
            && !self.alt
            && (matches!(
                (action, self.key),
                (
                    Action::InsertNewline | Action::FinishCursorPlacement,
                    Key::Enter
                ) | (Action::Indent | Action::Outdent, Key::Tab)
                    | (Action::ExitMulticursor, Key::Escape)
                    | (Action::ExplorerDelete, Key::Delete)
            ) || action.scope() == Scope::Review
                && (self.key == Key::Space || self.key.symbol_or_name().chars().count() == 1));
        if self.key == Key::Escape && !contextual_plain {
            return Err("Esc is reserved for closing dialogs and leaving Vim modes.".into());
        }
        if !self.ctrl
            && !self.alt
            && !contextual_plain
            && !matches!(
                self.key,
                Key::F1
                    | Key::F2
                    | Key::F3
                    | Key::F4
                    | Key::F5
                    | Key::F6
                    | Key::F7
                    | Key::F8
                    | Key::F9
                    | Key::F10
                    | Key::F11
                    | Key::F12
                    | Key::F13
                    | Key::F14
                    | Key::F15
                    | Key::F16
                    | Key::F17
                    | Key::F18
                    | Key::F19
                    | Key::F20
                    | Key::F21
                    | Key::F22
                    | Key::F23
                    | Key::F24
                    | Key::F25
                    | Key::F26
                    | Key::F27
                    | Key::F28
                    | Key::F29
                    | Key::F30
                    | Key::F31
                    | Key::F32
                    | Key::F33
                    | Key::F34
                    | Key::F35
            )
        {
            return Err(
                "Add Ctrl or Alt so typing, Tab and arrow navigation remain available.".into(),
            );
        }
        if self.ctrl && !self.alt {
            let reserved = match self.key {
                Key::C | Key::X | Key::V | Key::A if !self.shift => true,
                Key::Z if !self.shift => action != Action::Undo,
                Key::Z if self.shift => action != Action::Redo,
                Key::Y => action != Action::Redo,
                Key::ArrowUp if self.shift && action == Action::AddCursorAbove => false,
                Key::ArrowDown if self.shift && action == Action::AddCursorBelow => false,
                Key::ArrowLeft
                | Key::ArrowRight
                | Key::ArrowUp
                | Key::ArrowDown
                | Key::Home
                | Key::End
                | Key::Backspace
                | Key::Delete => true,
                _ => false,
            };
            if reserved {
                return Err(
                    "This key is reserved for native text editing. Choose another chord.".into(),
                );
            }
        }
        if (self.alt && !self.ctrl && matches!(self.key, Key::F4 | Key::Tab | Key::Space))
            || (self.ctrl && self.alt && self.key == Key::Delete)
            || (self.key == Key::Insert && (self.shift || self.ctrl))
            || (self.key == Key::Delete && self.shift)
        {
            return Err("This shortcut is reserved by the operating system or clipboard.".into());
        }
        Ok(())
    }
}

fn strokes(bindings: &Bindings, action: Action) -> Vec<Stroke> {
    match bindings.get(action.id()) {
        Some(Some(value)) => value
            .values()
            .filter_map(|value| Stroke::parse(value).ok())
            .filter(|stroke| stroke.safe(action).is_ok())
            .collect(),
        Some(None) => Vec::new(),
        None => default_strokes(action).to_vec(),
    }
}

fn default_strokes(action: Action) -> &'static [Stroke] {
    static DEFAULTS: std::sync::LazyLock<Vec<Vec<Stroke>>> = std::sync::LazyLock::new(|| {
        Action::ALL
            .iter()
            .map(|action| {
                action
                    .defaults()
                    .iter()
                    .filter_map(|value| Stroke::parse(value).ok())
                    .collect()
            })
            .collect()
    });
    &DEFAULTS[action as usize]
}

/// Newly available defaults yield to saved custom choices in overlapping scopes.
pub(super) fn migrate_later_defaults(bindings: &mut Bindings) {
    for &action in Action::ALL {
        migrate_later_default(bindings, action);
    }
}

#[cfg(test)]
pub(super) fn migrate_reopen_shortcut(bindings: &mut Bindings) {
    migrate_later_default(bindings, Action::ReopenTab);
}

fn migrate_later_default(bindings: &mut Bindings, added: Action) {
    if bindings.contains_key(added.id()) {
        return;
    }
    let defaults = default_strokes(added);
    let available: Vec<_> = defaults
        .iter()
        .copied()
        .filter(|stroke| {
            !bindings
                .keys()
                .filter_map(|id| Action::from_id(id))
                .any(|other| {
                    added.scope().overlaps(other.scope())
                        && strokes(bindings, other).contains(stroke)
                })
        })
        .collect();
    if available.len() != defaults.len() {
        set_strokes(bindings, added, &available);
    }
}

fn set_strokes(bindings: &mut Bindings, action: Action, values: &[Stroke]) {
    bindings.insert(
        action.id().into(),
        (!values.is_empty()).then(|| {
            values
                .iter()
                .map(|stroke| stroke.label())
                .collect::<Vec<_>>()
                .into()
        }),
    );
}

/// Restore each healthy override independently. Unknown IDs remain available for
/// a newer version, but cannot match events or block editing known actions.
pub(super) fn restore_bindings(bindings: &mut Bindings) -> Vec<String> {
    let mut warnings = Vec::new();
    let mut assigned: Vec<(Stroke, Action)> = Vec::new();
    for &action in Action::ALL {
        let Some(Some(binding)) = bindings.get(action.id()) else {
            continue;
        };
        let mut valid = Vec::new();
        for value in binding.values() {
            let stroke = Stroke::parse(value)
                .ok()
                .filter(|stroke| stroke.safe(action).is_ok());
            if let Some(stroke) = stroke
                && valid.len() < crate::cli::MAX_SHORTCUT_ALIASES
                && !valid.contains(&stroke)
                && !assigned
                    .iter()
                    .any(|(old, other)| *old == stroke && action.scope().overlaps(other.scope()))
            {
                valid.push(stroke);
                assigned.push((stroke, action));
            } else {
                warnings.push(format!(
                    "{}: ignored invalid or conflicting shortcut '{}'",
                    action.label(),
                    value
                ));
            }
        }
        set_strokes(bindings, action, &valid);
    }
    migrate_later_defaults(bindings);
    warnings
}

pub(super) fn matches(bindings: &Bindings, action: Action, event: &egui::Event) -> bool {
    let Some(stroke) = Stroke::event(event) else {
        return false;
    };
    match bindings.get(action.id()) {
        Some(Some(values)) => values.values().any(|value| {
            value.len() <= crate::cli::MAX_SHORTCUT_BYTES
                && Stroke::parse(value).ok() == Some(stroke)
                && stroke.safe(action).is_ok()
        }),
        Some(None) => false,
        None => default_strokes(action).contains(&stroke),
    }
}

pub(super) fn source_matches(
    bindings: &Bindings,
    action: Action,
    event: &egui::Event,
    vim_modal: bool,
    multi: bool,
    placing: bool,
) -> bool {
    if !matches(bindings, action, event) {
        return false;
    }
    let in_scope = match action.scope() {
        Scope::Global | Scope::Source => true,
        Scope::SourceEditing => !placing,
        Scope::CursorPlacement => placing,
        Scope::Multicursor => multi,
        _ => false,
    };
    if !in_scope {
        return false;
    }
    if vim_modal && !bindings.contains_key(action.id()) {
        if matches!(
            action,
            Action::Find | Action::InsertNewline | Action::Indent | Action::Outdent
        ) {
            return false;
        }
        if action == Action::ToggleLeftSidebar
            && matches!(event, egui::Event::Key { modifiers, .. } if !modifiers.alt)
        {
            return false;
        }
    }
    true
}

/// Local widgets consume their own contextual commands after taking focus.
pub(super) fn consume(bindings: &Bindings, action: Action, ctx: &egui::Context) -> bool {
    ctx.input_mut(|input| {
        let Some(index) = input.events.iter().position(|event| matches(bindings, action, event)) else { return false; };
        let event = input.events.remove(index);
        if input.events.get(index).is_some_and(|next| shortcut_text_event(next, &event)
            || matches!((&event, next),
                (egui::Event::Key { key, modifiers, .. }, egui::Event::Text(text))
                    if !modifiers.ctrl && !modifiers.command && !modifiers.alt
                        && (text.eq_ignore_ascii_case(key.symbol_or_name()) || *key == Key::Space && text == " ")))
        {
            input.events.remove(index);
        }
        true
    })
}

pub(super) fn label(bindings: &Bindings, action: Action) -> String {
    strokes(bindings, action)
        .iter()
        .map(|stroke| stroke.label())
        .collect::<Vec<_>>()
        .join(" / ")
}

pub(super) fn shortcut_text_event(event: &egui::Event, key_event: &egui::Event) -> bool {
    matches!((key_event, event), (
        egui::Event::Key { key, modifiers, pressed: true, .. }, egui::Event::Text(text))
        if modifiers.alt && !modifiers.ctrl && !modifiers.command
            && (key.symbol_or_name().chars().count() == 1 && text.eq_ignore_ascii_case(key.symbol_or_name())
                || *key == Key::Space && text == " "))
}

/// Some input integrations report AltGr as Ctrl+Alt even while emitting a
/// printable character. That emitted text must win over application shortcuts.
/// Native Windows winit already clears those modifiers; this also protects
/// alternate integrations without changing ordinary Ctrl+Alt commands.
pub(super) fn preserve_altgr_text(ctx: &egui::Context) {
    ctx.input_mut(|input| {
        // key_down reflects the end of this input batch. Recover its initial
        // state when Right Alt is pressed/released in the same batch as text.
        let mut right_alt = input
            .events
            .iter()
            .find_map(|event| match event {
                egui::Event::Key {
                    key: Key::AltRight,
                    pressed,
                    ..
                } => Some(!pressed),
                _ => None,
            })
            .unwrap_or_else(|| input.key_down(Key::AltRight));
        let mut index = 0;
        while index + 1 < input.events.len() {
            if let egui::Event::Key {
                key: Key::AltRight,
                pressed,
                ..
            } = &input.events[index]
            {
                right_alt = *pressed;
            }
            if right_alt && altgr_text_event(&input.events[index], &input.events[index + 1]) {
                input.events.remove(index);
            } else {
                index += 1;
            }
        }
    });
}

fn altgr_text_event(key_event: &egui::Event, text_event: &egui::Event) -> bool {
    matches!((key_event, text_event), (
        egui::Event::Key { key, pressed: true, modifiers, .. }, egui::Event::Text(text))
        if modifiers.ctrl && modifiers.alt && !modifiers.mac_cmd
            && (key.symbol_or_name().chars().count() == 1 || *key == Key::Space)
            && text.chars().count() == 1 && !text.chars().any(char::is_control))
}

pub(super) fn validate(bindings: &Bindings) -> Result<(), String> {
    validate_inner(bindings, false)
}

fn validate_inner(bindings: &Bindings, allow_unknown: bool) -> Result<(), String> {
    for (id, binding) in bindings {
        let Some(action) = Action::from_id(id) else {
            if allow_unknown {
                continue;
            }
            return Err(format!("Unknown shortcut action '{id}'"));
        };
        if let Some(value) = binding {
            let values: Vec<_> = value.values().collect();
            if values.is_empty() || values.len() > crate::cli::MAX_SHORTCUT_ALIASES {
                return Err(format!(
                    "Choose 1–{} shortcuts per command, or disable it.",
                    crate::cli::MAX_SHORTCUT_ALIASES
                ));
            }
            for value in values {
                Stroke::parse(value)?.safe(action)?;
            }
        }
    }
    let mut assigned: Vec<(Stroke, Action)> = Vec::new();
    for &action in Action::ALL {
        for stroke in strokes(bindings, action) {
            if let Some((_, other)) = assigned.iter().find(|(assigned, other)| {
                *assigned == stroke && action.scope().overlaps(other.scope())
            }) {
                return Err(format!(
                    "{} is assigned to both '{}' and '{}' in overlapping contexts.",
                    stroke.label(),
                    other.label(),
                    action.label()
                ));
            }
            assigned.push((stroke, action));
        }
    }
    Ok(())
}

fn conflicts(bindings: &Bindings, action: Action, stroke: Stroke) -> Vec<Action> {
    Action::ALL
        .iter()
        .copied()
        .filter(|other| {
            *other != action
                && action.scope().overlaps(other.scope())
                && strokes(bindings, *other).contains(&stroke)
        })
        .collect()
}

fn assign_alias(
    bindings: &Bindings,
    action: Action,
    index: Option<usize>,
    stroke: Stroke,
    reassign: bool,
) -> Result<Bindings, String> {
    stroke.safe(action)?;
    let mut candidate = bindings.clone();
    if reassign {
        for other in conflicts(bindings, action, stroke) {
            let retained: Vec<_> = strokes(bindings, other)
                .into_iter()
                .filter(|other| *other != stroke)
                .collect();
            set_strokes(&mut candidate, other, &retained);
        }
    }
    let mut values = strokes(bindings, action);
    if let Some(index) = index.filter(|index| *index < values.len()) {
        values[index] = stroke;
    } else {
        values.push(stroke);
    }
    set_strokes(&mut candidate, action, &values);
    validate_inner(&candidate, true)?;
    Ok(candidate)
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Filter {
    #[default]
    All,
    Modified,
    Disabled,
    Default,
}

impl Filter {
    fn label(self) -> &'static str {
        match self {
            Self::All => "All commands",
            Self::Modified => "Modified",
            Self::Disabled => "Disabled",
            Self::Default => "Default",
        }
    }
}

struct PendingConflict {
    action: Action,
    index: Option<usize>,
    stroke: Stroke,
}

#[derive(Default)]
pub(super) struct State {
    query: String,
    capture: Option<Action>,
    capture_index: usize,
    capture_add: bool,
    search_capture: bool,
    search_stroke: Option<Stroke>,
    filter: Filter,
    pending_conflict: Option<PendingConflict>,
    feedback: Option<(String, bool)>,
    continuation: Option<WidgetContinuation>,
}

#[derive(PartialEq, Eq)]
struct WidgetOwner {
    project: usize,
    pane: u64,
    document: Option<u64>,
    widget: Option<egui::Id>,
    bindings: Bindings,
}
struct WidgetContinuation {
    pass: u64,
    events: Vec<egui::Event>,
    owner: Option<WidgetOwner>,
}

impl VEditApp {
    fn shortcut_widget_owner(&self) -> WidgetOwner {
        WidgetOwner {
            project: self.active_project,
            pane: self.current_pane_id(),
            document: self.tab().map(|tab| tab.id),
            widget: self.ctx.memory(|memory| memory.focused()),
            bindings: self.settings.keybindings.clone(),
        }
    }
    pub(super) fn defer_widget_shortcut_input(&mut self, events: Vec<egui::Event>) {
        if events.is_empty() {
            return;
        }
        self.keybinding_ui.continuation = Some(WidgetContinuation {
            pass: self.ctx.cumulative_pass_nr(),
            events,
            owner: None,
        });
        self.ctx.request_repaint();
    }
    pub(super) fn finish_shortcut_input(&mut self) {
        if self
            .keybinding_ui
            .continuation
            .as_ref()
            .is_some_and(|pending| pending.owner.is_none())
        {
            let owner = self.shortcut_widget_owner();
            self.keybinding_ui.continuation.as_mut().unwrap().owner = Some(owner);
        }
    }
    pub(super) fn resume_shortcut_input(&mut self) {
        if !self
            .keybinding_ui
            .continuation
            .as_ref()
            .is_some_and(|pending| pending.pass < self.ctx.cumulative_pass_nr())
        {
            return;
        }
        let pending = self.keybinding_ui.continuation.take().unwrap();
        if pending.owner.as_ref() != Some(&self.shortcut_widget_owner())
            || self.settings_open
            || self.palette.is_some()
            || self.save_as.is_some()
            || self.close_action.is_some()
            || self.picker.is_some()
            || self.diff.is_some()
            || self.git_detail_open()
            || self.navigation_modal_open()
            || self.navigation_pending()
            || self.status_details.is_some()
            || self.agent_marks_modal_open()
            || self.notifications.is_open()
            || self.context_file_dialog.is_some()
            || self.vim_command.is_some()
            || self.screen_jump.is_some()
            || self.vim_search_active()
            || egui::Popup::is_any_open(&self.ctx)
        {
            return;
        }
        let mut events = pending.events;
        self.ctx.input_mut(|input| {
            events.append(&mut input.events);
            input.events = events;
        });
    }
    pub(super) fn defer_widget_shortcut_tail(&mut self) {
        let context = self.shortcut_input_context();
        let boundary = self.ctx.input(|input| {
            input.events.iter().position(|event| {
                Action::ALL
                    .iter()
                    .copied()
                    .any(|action| self.shortcut_boundary(action, event, context))
            })
        });
        if let Some(index) = boundary {
            let suffix = self.ctx.input_mut(|input| input.events.split_off(index));
            self.defer_widget_shortcut_input(suffix);
        }
    }
    pub(super) fn run_shortcut_action(&mut self, action: Action) {
        self.execute_action(action);
    }
    pub(super) fn shortcut_matches(&self, action: Action, event: &egui::Event) -> bool {
        matches(&self.settings.keybindings, action, event)
    }
    pub(super) fn shortcut_input_context(&self) -> (bool, bool) {
        let source = self.tab().filter(|tab| {
            tab.agent_view == crate::cli::DocumentView::Source
                && self
                    .ctx
                    .memory(|memory| memory.has_focus(self.document_widget_id(tab.id)))
        });
        (
            source.is_some(),
            self.settings.vim
                && source
                    .is_some_and(|tab| tab.vim.mode != VimMode::Insert && !tab.vim.multi.active()),
        )
    }
    pub(super) fn shortcut_boundary(
        &self,
        action: Action,
        event: &egui::Event,
        (source, vim_modal): (bool, bool),
    ) -> bool {
        if matches!(
            action.scope(),
            Scope::Explorer | Scope::ContextMenu | Scope::Review | Scope::ReviewComment
        ) {
            return false;
        }
        if action.scope().source() && !source {
            return false;
        }
        let multi = self.tab().is_some_and(|tab| tab.vim.multi.active());
        let placing = self.tab().is_some_and(|tab| tab.vim.multi.placing());
        if !source_matches(
            &self.settings.keybindings,
            action,
            event,
            vim_modal,
            multi,
            placing,
        ) {
            return false;
        }
        if vim_modal
            && action == Action::OpenFile
            && !self.settings.keybindings.contains_key(action.id())
            && self
                .tab()
                .is_some_and(|tab| tab.vim.mode == VimMode::Normal)
        {
            return false;
        }
        true
    }
    pub(super) fn earlier_shortcut_boundary(&self, index: usize) -> bool {
        let context = self.shortcut_input_context();
        self.ctx.input(|input| {
            input.events[..index].iter().any(|event| {
                Action::ALL
                    .iter()
                    .copied()
                    .any(|action| self.shortcut_boundary(action, event, context))
            })
        })
    }
    pub(super) fn defer_shortcut_tail(&self) {
        let Some(tab) = self.tab() else { return };
        let context = self.shortcut_input_context();
        let boundary = self.ctx.input(|input| {
            input.events.iter().position(|event| {
                Action::ALL
                    .iter()
                    .copied()
                    .any(|action| self.shortcut_boundary(action, event, context))
            })
        });
        if let Some(index) = boundary {
            let suffix = self.ctx.input_mut(|input| input.events.split_off(index));
            editor_input::defer(&self.ctx, tab.id, suffix);
        }
    }
    pub(super) fn shortcut_label(&self, action: Action) -> String {
        label(&self.settings.keybindings, action)
    }
    pub(super) fn take_shortcut(&self, action: Action) -> bool {
        if self.keybinding_ui.capture.is_some() || self.keybinding_ui.search_capture {
            return false;
        }
        let context = self.shortcut_input_context();
        let index = self.ctx.input(|input| {
            input
                .events
                .iter()
                .position(|event| self.shortcut_boundary(action, event, context))
        });
        let Some(index) = index else { return false };
        if !matches!(action, Action::ZoomIn | Action::ZoomOut | Action::ZoomReset)
            && self.earlier_shortcut_boundary(index)
        {
            return false;
        }
        self.ctx.input_mut(|input| {
            let event = input.events.remove(index);
            if input
                .events
                .get(index)
                .is_some_and(|next| shortcut_text_event(next, &event))
            {
                input.events.remove(index);
            }
            true
        })
    }
    pub(super) fn keybindings_catalog(&self) -> serde_json::Value {
        serde_json::json!(
            Action::ALL
                .iter()
                .map(|action| serde_json::json!({
                    "action":action.id(), "label":action.label(), "group":action.group(),
                    "shortcut":self.shortcut_label(*action), "defaults":action.defaults(),
                    "shortcuts":strokes(&self.settings.keybindings,*action).iter().map(|stroke|stroke.label()).collect::<Vec<_>>(),
                    "context":action.scope().label(),
                    "customized":self.settings.keybindings.contains_key(action.id())
                }))
                .collect::<Vec<_>>()
        )
    }
    /// Before zoom, global commands or modal close logic sees keyboard events.
    pub(super) fn capture_shortcut_input(&mut self) {
        preserve_altgr_text(&self.ctx);
        let action = self.keybinding_ui.capture;
        if action.is_none() && !self.keybinding_ui.search_capture {
            return;
        }
        let (stroke, clipboard) = self.ctx.input_mut(|input| {
            let stroke = input.events.iter().find_map(|event| {
                if matches!(event, egui::Event::Key { repeat: true, .. }) {
                    None
                } else {
                    Stroke::event(event)
                }
            });
            let clipboard = input.events.iter().any(|event| {
                matches!(
                    event,
                    egui::Event::Copy | egui::Event::Cut | egui::Event::Paste(_)
                )
            });
            input.events.retain(|event| {
                !matches!(
                    event,
                    egui::Event::Key { .. }
                        | egui::Event::Text(_)
                        | egui::Event::Paste(_)
                        | egui::Event::Copy
                        | egui::Event::Cut
                        | egui::Event::Ime(_)
                )
            });
            (stroke, clipboard)
        });
        if clipboard {
            self.keybinding_ui.capture = None;
            self.keybinding_ui.search_capture = false;
            self.keybinding_ui.feedback = Some((
                "This shortcut is reserved for the system clipboard. Choose another combination."
                    .into(),
                true,
            ));
            return;
        }
        let Some(stroke) = stroke else { return };
        if stroke.key == Key::Escape {
            self.keybinding_ui.capture = None;
            self.keybinding_ui.search_capture = false;
            self.keybinding_ui.feedback =
                Some(("Recording cancelled; shortcuts unchanged.".into(), false));
            return;
        }
        if self.keybinding_ui.search_capture {
            self.keybinding_ui.search_capture = false;
            self.keybinding_ui.search_stroke = Some(stroke);
            self.keybinding_ui.query.clear();
            self.keybinding_ui.filter = Filter::All;
            return;
        }
        let action = action.unwrap();
        if let Err(error) = stroke.safe(action) {
            self.keybinding_ui.feedback = Some((error, true));
            return;
        }
        let index = (!self.keybinding_ui.capture_add).then_some(self.keybinding_ui.capture_index);
        if !conflicts(&self.settings.keybindings, action, stroke).is_empty() {
            self.keybinding_ui.capture = None;
            self.keybinding_ui.pending_conflict = Some(PendingConflict {
                action,
                index,
                stroke,
            });
            self.keybinding_ui.feedback = None;
            return;
        }
        match assign_alias(&self.settings.keybindings, action, index, stroke, false) {
            Ok(candidate) => {
                self.settings.keybindings = candidate;
                self.session_dirty = true;
                self.keybinding_ui.capture = None;
                self.keybinding_ui.feedback =
                    Some((format!("{}: {}", action.label(), stroke.label()), false));
            }
            Err(error) => self.keybinding_ui.feedback = Some((error, true)),
        }
    }

    fn start_shortcut_recording(&mut self, action: Action, index: Option<usize>) {
        self.keybinding_ui.capture = Some(action);
        self.keybinding_ui.capture_add = index.is_none();
        self.keybinding_ui.capture_index = index.unwrap_or_default();
        self.keybinding_ui.search_capture = false;
        self.keybinding_ui.pending_conflict = None;
        self.keybinding_ui.feedback = None;
        if let Some(id) = self.ctx.memory(|memory| memory.focused()) {
            self.ctx.memory_mut(|memory| memory.surrender_focus(id));
        }
    }

    pub(super) fn keyboard_settings_ui(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;
        editor_preferences::preference_heading(ui, "KEYBOARD SHORTCUTS");
        ui.label(RichText::new("Choose a command, then record or add a shortcut. Changes apply immediately and are saved.").size(11.0).color(MUTED));
        ui.label(RichText::new("Context shows where a command works. Separate contexts may share a key. Text navigation, clipboard and Vim motions retain their input meaning.").size(10.5).color(MUTED));
        ui.horizontal_wrapped(|ui| {
            if ui
                .add(
                    egui::TextEdit::singleline(&mut self.keybinding_ui.query)
                        .id(egui::Id::new("keyboard_shortcut_search"))
                        .hint_text("Search commands, shortcuts or contexts")
                        .desired_width((ui.available_width() - 135.0).max(100.0)),
                )
                .changed()
            {
                self.keybinding_ui.search_stroke = None;
            }
            if ui
                .button("Record search")
                .on_hover_text("Press a key combination to find its commands")
                .clicked()
            {
                self.keybinding_ui.capture = None;
                self.keybinding_ui.search_capture = true;
                self.keybinding_ui.pending_conflict = None;
                self.keybinding_ui.feedback = None;
                if let Some(id) = self.ctx.memory(|memory| memory.focused()) {
                    self.ctx.memory_mut(|memory| memory.surrender_focus(id));
                }
            }
        });
        ui.horizontal_wrapped(|ui| {
            egui::ComboBox::from_id_salt("shortcut_filter")
                .selected_text(self.keybinding_ui.filter.label())
                .show_ui(ui, |ui| {
                    for filter in [
                        Filter::All,
                        Filter::Modified,
                        Filter::Disabled,
                        Filter::Default,
                    ] {
                        ui.selectable_value(&mut self.keybinding_ui.filter, filter, filter.label());
                    }
                });
            if ui
                .add_enabled(
                    !self.settings.keybindings.is_empty(),
                    egui::Button::new("Reset all"),
                )
                .clicked()
            {
                self.settings.keybindings.clear();
                self.keybinding_ui.capture = None;
                self.keybinding_ui.pending_conflict = None;
                self.keybinding_ui.feedback =
                    Some(("All shortcuts restored to defaults.".into(), false));
                changed = true;
            }
            if let Some(stroke) = self.keybinding_ui.search_stroke {
                ui.label(format!("Key: {}", stroke.label()));
                if ui.small_button("Clear key search").clicked() {
                    self.keybinding_ui.search_stroke = None;
                }
            }
        });
        if self.keybinding_ui.capture.is_some() || self.keybinding_ui.search_capture {
            ui.horizontal_wrapped(|ui| {
                let prompt = self.keybinding_ui.capture.map_or_else(
                    || "Press a shortcut to search - Esc cancels".into(),
                    |action| format!("Press shortcut for {} - Esc cancels", action.label()),
                );
                ui.label(RichText::new(prompt).color(ACCENT));
                if ui.button("Cancel recording").clicked() {
                    self.keybinding_ui.capture = None;
                    self.keybinding_ui.search_capture = false;
                }
            });
        }
        if let Some(pending) = &self.keybinding_ui.pending_conflict {
            let PendingConflict {
                action,
                index,
                stroke,
            } = *pending;
            let owners = conflicts(&self.settings.keybindings, action, stroke)
                .iter()
                .map(|action| action.label())
                .collect::<Vec<_>>()
                .join(", ");
            ui.label(
                RichText::new(format!(
                    "{} is used by {}. Reassign it to {}? Other shortcuts stay available.",
                    stroke.label(),
                    owners,
                    action.label()
                ))
                .color(ORANGE),
            );
            ui.horizontal_wrapped(|ui| {
                if ui.button("Reassign shortcut").clicked() {
                    match assign_alias(&self.settings.keybindings, action, index, stroke, true) {
                        Ok(candidate) => {
                            self.settings.keybindings = candidate;
                            self.keybinding_ui.pending_conflict = None;
                            changed = true;
                        }
                        Err(error) => self.keybinding_ui.feedback = Some((error, true)),
                    }
                }
                if ui.button("Keep existing").clicked() {
                    self.keybinding_ui.pending_conflict = None;
                }
            });
        }
        if let Some((message, error)) = &self.keybinding_ui.feedback {
            ui.label(
                RichText::new(message)
                    .size(11.0)
                    .color(if *error { RED } else { MUTED }),
            );
        }
        let query = self.keybinding_ui.query.trim().to_lowercase();
        let rows: Vec<_> = Action::ALL
            .iter()
            .copied()
            .filter(|action| {
                let customized = self.settings.keybindings.contains_key(action.id());
                let keys = strokes(&self.settings.keybindings, *action);
                let visible = match self.keybinding_ui.filter {
                    Filter::All => true,
                    Filter::Modified => customized,
                    Filter::Disabled => customized && keys.is_empty(),
                    Filter::Default => !customized,
                };
                visible
                    && self
                        .keybinding_ui
                        .search_stroke
                        .is_none_or(|stroke| keys.contains(&stroke))
                    && format!(
                        "{} {} {} {} {}",
                        action.label(),
                        action.id(),
                        action.group(),
                        action.scope().label(),
                        self.shortcut_label(*action)
                    )
                    .to_lowercase()
                    .contains(&query)
            })
            .collect();
        ui.label(
            RichText::new(format!("{} actions", rows.len()))
                .size(10.0)
                .color(MUTED),
        );
        if rows.is_empty() {
            ui.label("No commands match this search.");
        }
        for action in rows {
            ui.push_id(action.id(), |ui| {
                changed |= self.keyboard_shortcut_row(ui, action);
            });
        }
        changed
    }

    fn keyboard_shortcut_row(&mut self, ui: &mut egui::Ui, action: Action) -> bool {
        let mut changed = false;
        let keys = strokes(&self.settings.keybindings, action);
        let customized = self.settings.keybindings.contains_key(action.id());
        let status = if customized && keys.is_empty() {
            "Disabled"
        } else if customized {
            "Modified"
        } else {
            "Default"
        };
        let recording = self.keybinding_ui.capture == Some(action);
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(6, 4))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(action.label()).size(12.0).color(TEXT))
                        .on_hover_text(action.context_hint());
                    ui.label(
                        RichText::new(format!("{} · {status}", action.scope().label()))
                            .size(10.0)
                            .color(if customized { ACCENT } else { MUTED }),
                    );
                });
                ui.horizontal_wrapped(|ui| {
                    for (index, stroke) in keys.iter().enumerate() {
                        let text = if recording
                            && !self.keybinding_ui.capture_add
                            && index == self.keybinding_ui.capture_index
                        {
                            "Press a shortcut...".into()
                        } else {
                            stroke.label()
                        };
                        if ui
                            .button(text)
                            .on_hover_text(format!(
                                "Change {} for {}",
                                stroke.label(),
                                action.label()
                            ))
                            .clicked()
                        {
                            self.start_shortcut_recording(action, Some(index));
                        }
                        if ui
                            .small_button("Remove")
                            .on_hover_text(format!(
                                "Remove {} from {}",
                                stroke.label(),
                                action.label()
                            ))
                            .clicked()
                        {
                            let mut retained = keys.clone();
                            retained.remove(index);
                            set_strokes(&mut self.settings.keybindings, action, &retained);
                            self.keybinding_ui.capture = None;
                            self.keybinding_ui.pending_conflict = None;
                            changed = true;
                        }
                    }
                    let add_label = if recording && self.keybinding_ui.capture_add {
                        "Press a shortcut..."
                    } else if keys.is_empty() {
                        "Unassigned"
                    } else {
                        "Add shortcut"
                    };
                    if ui
                        .add_enabled(
                            keys.len() < crate::cli::MAX_SHORTCUT_ALIASES,
                            egui::Button::new(add_label),
                        )
                        .on_hover_text(
                            "Add an alternative shortcut without replacing existing ones",
                        )
                        .clicked()
                    {
                        self.start_shortcut_recording(action, None);
                    }
                    if !keys.is_empty()
                        && ui
                            .small_button("Disable")
                            .on_hover_text("Remove every shortcut for this command")
                            .clicked()
                    {
                        self.settings.keybindings.insert(action.id().into(), None);
                        self.keybinding_ui.capture = None;
                        self.keybinding_ui.pending_conflict = None;
                        changed = true;
                    }
                    if customized
                        && ui
                            .small_button("Reset")
                            .on_hover_text(format!(
                                "Restore defaults: {}",
                                action.defaults().join(" / ")
                            ))
                            .clicked()
                    {
                        let mut candidate = self.settings.keybindings.clone();
                        candidate.remove(action.id());
                        match validate_inner(&candidate, true) {
                            Ok(()) => {
                                self.settings.keybindings = candidate;
                                self.keybinding_ui.capture = None;
                                self.keybinding_ui.pending_conflict = None;
                                changed = true;
                            }
                            Err(error) => self.keybinding_ui.feedback = Some((error, true)),
                        }
                    }
                });
            });
        ui.separator();
        changed
    }
}

#[cfg(test)]
#[path = "keybindings_tests.rs"]
mod tests;

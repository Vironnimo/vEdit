//! Stable command identities and display metadata, independent of UI execution.
macro_rules! actions {
    ($($name:ident, $id:literal, $label:literal, $group:literal, [$($key:literal),*];)*) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub(super) enum Action { $($name,)* }
        impl Action {
            pub(super) const ALL: &'static [Self] = &[$(Self::$name,)*];
            pub(super) fn id(self) -> &'static str { match self { $(Self::$name => $id,)* } }
            pub(super) fn label(self) -> &'static str { match self { $(Self::$name => $label,)* } }
            pub(super) fn group(self) -> &'static str { match self { $(Self::$name => $group,)* } }
            pub(super) fn defaults(self) -> &'static [&'static str] { match self { $(Self::$name => &[$($key),*],)* } }
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
    DuplicateLine, "duplicate_line", "Duplicate line / selection", "Editing", ["Alt+Shift+ArrowDown"];
    DeleteLine, "delete_line", "Delete line / selection", "Editing", ["Ctrl+Shift+K"];
    InsertLineAbove, "insert_line_above", "Insert line above", "Editing", ["Ctrl+Shift+Enter"];
    InsertLineBelow, "insert_line_below", "Insert line below", "Editing", ["Ctrl+Enter"];
    MoveLineUp, "move_line_up", "Move line / selection up", "Editing", ["Alt+ArrowUp"];
    MoveLineDown, "move_line_down", "Move line / selection down", "Editing", ["Alt+ArrowDown"];
    Undo, "undo", "Undo", "Editing", ["Ctrl+Z"];
    Redo, "redo", "Redo", "Editing", ["Ctrl+Y", "Ctrl+Shift+Z"];
    AddCursorAbove, "add_cursor_above", "Add cursor above", "Multicursor", ["Ctrl+Shift+ArrowUp"];
    AddCursorBelow, "add_cursor_below", "Add cursor below", "Multicursor", ["Ctrl+Shift+ArrowDown"];
    SelectNextOccurrence, "select_next_occurrence", "Add next occurrence", "Multicursor", ["Ctrl+D"];
    SelectAllOccurrences, "select_all_occurrences", "Select all occurrences", "Multicursor", ["Ctrl+Shift+L"];
    SkipOccurrence, "skip_occurrence", "Skip current occurrence", "Multicursor", ["Ctrl+Shift+D"];
    CursorUndo, "cursor_undo", "Undo last cursor selection", "Multicursor", ["Ctrl+U"];
    CursorsAtLineEnds, "cursors_at_line_ends", "Cursors at selected line ends", "Multicursor", ["Alt+Shift+I"];
    ExitMulticursor, "exit_multicursor", "Exit multicursor editing", "Multicursor", [];
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
    pub(super) fn multicursor(self) -> bool {
        matches!(
            self,
            Self::AddCursorAbove
                | Self::AddCursorBelow
                | Self::SelectNextOccurrence
                | Self::SelectAllOccurrences
                | Self::SkipOccurrence
                | Self::CursorUndo
                | Self::CursorsAtLineEnds
                | Self::ExitMulticursor
        )
    }
    pub(super) fn from_id(id: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|action| action.id() == id)
    }
    pub(super) fn context_hint(self) -> &'static str {
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
                "Ctrl+Alt+E works in every editor mode. The default Ctrl+B alias remains page-back in Vim Normal mode."
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

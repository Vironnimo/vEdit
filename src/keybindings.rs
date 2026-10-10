//! One source of truth for user shortcuts, dispatch, persistence and visible hints.
use super::*;
use std::collections::BTreeMap;

pub(super) type Bindings = BTreeMap<String, Option<String>>;

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
    RestartLanguage, "restart_language", "Restart language analysis", "Language", [];
    LanguageSupport, "language_support", "Language support", "Language", [];
    Problems, "problems", "Show problems", "Layout", ["Ctrl+Shift+M"];
    AgentMark, "agent_mark", "Mark selection for agent", "Navigation", ["Ctrl+Shift+A"];
    AgentMarks, "agent_marks", "Agent marks", "Navigation", ["Ctrl+Alt+Shift+A"];
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
    Explorer, "explorer", "Show explorer", "Layout", [];
    ToggleLeftSidebar, "toggle_left_sidebar", "Toggle left sidebar", "Layout", ["Ctrl+Alt+E", "Ctrl+B"];
    ToggleRightSidebar, "toggle_right_sidebar", "Toggle right sidebar", "Layout", ["Ctrl+Alt+B"];
    ExplorerGitLayout, "explorer_git_layout", "Explorer left, Git right", "Layout", [];
    ToggleOutput, "toggle_output", "Toggle output", "Layout", ["Ctrl+Backtick"];
    CheckProject, "check_project", "Check project", "Run", ["F5"];
    TestProject, "test_project", "Run tests", "Run", ["Alt+F6"];
    BuildProject, "build_project", "Build project", "Run", [];
    RunFile, "run_file", "Run active file", "Run", ["Ctrl+F5"];
    ConfigureTasks, "configure_tasks", "Configure project tasks", "Run", [];
    Terminal, "terminal", "Open terminal", "Run", [];
    ToggleWrap, "toggle_wrap", "Toggle word wrap", "Editing", ["Alt+Z"];
    ToggleVim, "toggle_vim", "Toggle Vim motions", "Editing", [];
    ToggleComment, "toggle_comment", "Toggle line comment", "Editing", ["Ctrl+Slash", "Ctrl+Shift+Slash"];
    DuplicateLine, "duplicate_line", "Duplicate line / selection", "Editing", ["Alt+Shift+ArrowDown"];
    MoveLineUp, "move_line_up", "Move line / selection up", "Editing", ["Alt+ArrowUp"];
    MoveLineDown, "move_line_down", "Move line / selection down", "Editing", ["Alt+ArrowDown"];
    Undo, "undo", "Undo", "Editing", ["Ctrl+Z"];
    Redo, "redo", "Redo", "Editing", ["Ctrl+Y", "Ctrl+Shift+Z"];
    ToggleBookmark, "toggle_bookmark", "Toggle bookmark", "Navigation", [];
    Bookmarks, "bookmarks", "Bookmarks", "Navigation", [];
    ScreenJump, "screen_jump", "Jump to visible text", "Navigation", [];
    Reload, "reload", "Reload file from disk", "Files", [];
    Settings, "settings", "Settings", "Preferences", ["Ctrl+Comma"];
    KeyboardShortcuts, "keyboard_shortcuts", "Keyboard shortcuts", "Preferences", [];
    ZoomIn, "zoom_in", "Zoom in", "Preferences", ["Ctrl+Plus", "Ctrl+Equals", "Ctrl+Shift+Plus", "Ctrl+Shift+Equals"];
    ZoomOut, "zoom_out", "Zoom out", "Preferences", ["Ctrl+Minus"];
    ZoomReset, "zoom_reset", "Reset zoom", "Preferences", ["Ctrl+0"];
}

impl Action {
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
            Self::ToggleLeftSidebar => {
                "Ctrl+Alt+E works in every editor mode. The default Ctrl+B alias remains page-back in Vim Normal mode."
            }
            Self::Undo
            | Self::Redo
            | Self::ToggleComment
            | Self::DuplicateLine
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
        if self.key == Key::Escape {
            return Err("Esc is reserved for closing dialogs and leaving Vim modes.".into());
        }
        if !self.ctrl
            && !self.alt
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
        Some(Some(value)) => Stroke::parse(value).ok().into_iter().collect(),
        Some(None) => Vec::new(),
        None => action
            .defaults()
            .iter()
            .filter_map(|value| Stroke::parse(value).ok())
            .collect(),
    }
}

/// The Reopen default was added after users could assign Ctrl+Shift+T elsewhere.
/// Preserve those saved choices when upgrading, without changing conflict rules
/// for new assignments or an explicitly configured Reopen action.
pub(super) fn migrate_reopen_shortcut(bindings: &mut Bindings) {
    if bindings.contains_key(Action::ReopenTab.id()) {
        return;
    }
    let defaults = strokes(bindings, Action::ReopenTab);
    let occupied = bindings.iter().any(|(id, value)| {
        let Some(action) = Action::from_id(id) else {
            return false;
        };
        value
            .as_deref()
            .and_then(|value| Stroke::parse(value).ok())
            .is_some_and(|stroke| stroke.safe(action).is_ok() && defaults.contains(&stroke))
    });
    if occupied {
        bindings.insert(Action::ReopenTab.id().into(), None);
    }
}

pub(super) fn matches(bindings: &Bindings, action: Action, event: &egui::Event) -> bool {
    let Some(stroke) = Stroke::event(event) else {
        return false;
    };
    strokes(bindings, action).contains(&stroke)
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
    for (id, binding) in bindings {
        let action =
            Action::from_id(id).ok_or_else(|| format!("Unknown shortcut action '{id}'"))?;
        if let Some(value) = binding {
            Stroke::parse(value)?.safe(action)?;
        }
    }
    let mut assigned: Vec<(Stroke, Action)> = Vec::new();
    for &action in Action::ALL {
        for stroke in strokes(bindings, action) {
            if let Some((_, other)) = assigned.iter().find(|(assigned, _)| *assigned == stroke) {
                return Err(format!(
                    "{} is assigned to both '{}' and '{}'. Disable or rebind one of them first.",
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

#[derive(Default)]
pub(super) struct State {
    query: String,
    capture: Option<Action>,
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
            || self.agent_marks_modal_open()
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
        match action {
            Action::CommandPalette => self.open_palette(Palette::Commands),
            Action::QuickOpen => self.open_palette(Palette::Files),
            Action::SaveAs => {
                if let Some(id) = self.tab().map(|tab| tab.id) {
                    self.request_save_as(id);
                }
            }
            Action::CloseTab => {
                if let Some(tab) = self.tab() {
                    self.request_close(CloseAction::PaneTab {
                        pane_id: self.current_pane_id(),
                        id: tab.id,
                    });
                }
            }
            Action::ReopenTab => self.reopen_closed_tab(),
            Action::NextTab | Action::PreviousTab => {
                self.bind_current_pane();
                let ids = self.current_pane_tab_ids();
                if ids.is_empty() {
                    return;
                }
                let active = self
                    .tab()
                    .and_then(|tab| ids.iter().position(|id| *id == tab.id))
                    .unwrap_or(0);
                let next = if action == Action::PreviousTab {
                    (active + ids.len() - 1) % ids.len()
                } else {
                    (active + 1) % ids.len()
                };
                if let Some((project, tab)) =
                    self.projects.iter().enumerate().find_map(|(pi, project)| {
                        project
                            .tabs
                            .iter()
                            .position(|tab| tab.id == ids[next])
                            .map(|ti| (pi, ti))
                    })
                {
                    self.activate_tab(project, tab);
                }
            }
            Action::FindNext | Action::FindPrevious => {
                self.find_next(action == Action::FindPrevious)
            }
            Action::KeyboardShortcuts => {
                self.open_settings_section("Keyboard");
            }
            Action::Undo | Action::Redo => {
                if let Some(id) = self.tab().map(|tab| tab.id) {
                    self.apply_editor_context_action(
                        id,
                        if action == Action::Undo {
                            context_actions::EditorAction::Undo
                        } else {
                            context_actions::EditorAction::Redo
                        },
                    );
                }
            }
            Action::ToggleComment
            | Action::DuplicateLine
            | Action::MoveLineUp
            | Action::MoveLineDown => {
                let Some(id) = self.tab().map(|tab| tab.id) else {
                    return;
                };
                self.context_native_undo_checkpoint(id);
                let vim = self.settings.vim;
                let Some(tab) = self.tab_mut().filter(|tab| !tab.loading) else {
                    return;
                };
                let selection = if vim {
                    tab.vim
                        .selection(&tab.text)
                        .unwrap_or_else(|| tab.selection.clone())
                } else {
                    tab.selection.clone()
                };
                let edit = match action {
                    Action::ToggleComment => editor::toggle_line_comment(
                        &tab.text,
                        selection,
                        crate::file_kind::comment_language(tab.path.as_deref()),
                    ),
                    Action::MoveLineUp | Action::MoveLineDown => {
                        editor::move_lines(&tab.text, selection, action == Action::MoveLineDown)
                    }
                    _ => editor::duplicate_lines(&tab.text, selection),
                };
                if edit.text == tab.text {
                    return;
                }
                if matches!(
                    action,
                    Action::MoveLineUp | Action::MoveLineDown | Action::DuplicateLine
                ) {
                    tab.apply_line_move(edit);
                } else {
                    tab.vim.begin_external_edit(&tab.text);
                    tab.apply(edit);
                    tab.vim.notify_external_text_change();
                }
            }
            Action::ZoomIn | Action::ZoomOut | Action::ZoomReset => {
                self.settings.ui_scale = match action {
                    Action::ZoomIn => (self.settings.ui_scale + 0.1).min(2.0),
                    Action::ZoomOut => (self.settings.ui_scale - 0.1).max(0.75),
                    _ => 1.0,
                };
                self.settings.ui_scale = (self.settings.ui_scale * 100.0).round() / 100.0;
                self.session_dirty = true;
                self.ctx.set_zoom_factor(self.settings.ui_scale);
                self.ctx.request_repaint();
            }
            _ => self.command_action(action.label()),
        }
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
            self.settings.vim && source.is_some_and(|tab| tab.vim.mode != VimMode::Insert),
        )
    }
    pub(super) fn shortcut_boundary(
        &self,
        action: Action,
        event: &egui::Event,
        (source, vim_modal): (bool, bool),
    ) -> bool {
        if !self.shortcut_matches(action, event) {
            return false;
        }
        if matches!(
            action,
            Action::Undo
                | Action::Redo
                | Action::ToggleComment
                | Action::DuplicateLine
                | Action::MoveLineUp
                | Action::MoveLineDown
        ) && !source
        {
            return false;
        }
        if vim_modal && !self.settings.keybindings.contains_key(action.id()) {
            if action == Action::Find {
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
        if self.keybinding_ui.capture.is_some() {
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
                    "customized":self.settings.keybindings.contains_key(action.id())
                }))
                .collect::<Vec<_>>()
        )
    }
    /// Before zoom, global commands or modal close logic sees keyboard events.
    pub(super) fn capture_shortcut_input(&mut self) {
        preserve_altgr_text(&self.ctx);
        let Some(action) = self.keybinding_ui.capture else {
            return;
        };
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
            self.keybinding_ui.feedback = Some((
                "This shortcut is reserved for the system clipboard. Choose another chord.".into(),
                true,
            ));
            return;
        }
        let Some(stroke) = stroke else { return };
        self.keybinding_ui.capture = None;
        if stroke.key == Key::Escape {
            self.keybinding_ui.feedback =
                Some(("Recording cancelled; shortcut unchanged.".into(), false));
            return;
        }
        let mut candidate = self.settings.keybindings.clone();
        candidate.insert(action.id().into(), Some(stroke.label()));
        match validate(&candidate) {
            Ok(()) => {
                self.settings.keybindings = candidate;
                self.session_dirty = true;
                self.keybinding_ui.feedback =
                    Some((format!("{} → {}", action.label(), stroke.label()), false));
            }
            Err(error) => {
                self.keybinding_ui.capture = Some(action);
                self.keybinding_ui.feedback = Some((error, true));
            }
        }
    }
    pub(super) fn keyboard_settings_ui(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;
        editor_preferences::preference_heading(ui, "KEYBOARD SHORTCUTS");
        ui.label(
            RichText::new(
                "Search an action or shortcut, then click its binding to record a new chord.",
            )
            .size(11.0)
            .color(MUTED),
        );
        ui.label(RichText::new("Application commands work across the workspace; editing commands require source focus. Plain typing, arrow navigation and clipboard keys are reserved. Custom bindings take priority over the default Vim exceptions.").size(10.5).color(MUTED));
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.keybinding_ui.query)
                    .id(egui::Id::new("keyboard_shortcut_search"))
                    .hint_text("Search keyboard shortcuts")
                    .desired_width((ui.available_width() - 100.0).max(80.0)),
            );
            if ui
                .add_enabled(
                    !self.settings.keybindings.is_empty(),
                    egui::Button::new("Reset all"),
                )
                .clicked()
            {
                self.settings.keybindings.clear();
                self.keybinding_ui.capture = None;
                self.keybinding_ui.feedback =
                    Some(("All shortcuts restored to defaults.".into(), false));
                changed = true;
            }
        });
        if let Some(action) = self.keybinding_ui.capture {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(format!(
                        "Press shortcut for {} · Esc cancels",
                        action.label()
                    ))
                    .color(ACCENT),
                );
                if ui.button("Cancel recording").clicked() {
                    self.keybinding_ui.capture = None;
                }
            });
        }
        if let Some((message, error)) = &self.keybinding_ui.feedback {
            ui.label(
                RichText::new(message)
                    .size(11.0)
                    .color(if *error { RED } else { ACCENT }),
            );
        }
        let query = self.keybinding_ui.query.trim().to_lowercase();
        let rows: Vec<_> = Action::ALL
            .iter()
            .copied()
            .filter(|action| {
                format!(
                    "{} {} {} {}",
                    action.label(),
                    action.id(),
                    action.group(),
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
        for action in rows {
            ui.push_id(action.id(), |ui| {
                ui.separator();
                ui.label(RichText::new(action.label()).size(12.0))
                    .on_hover_text(action.context_hint());
                ui.horizontal_wrapped(|ui| {
                    let binding = self.shortcut_label(action);
                    let recording = self.keybinding_ui.capture == Some(action);
                    let text = if recording {
                        "Press a shortcut…"
                    } else if binding.is_empty() {
                        "Unassigned — record"
                    } else {
                        &binding
                    };
                    if ui
                        .button(text)
                        .on_hover_text(format!("Record shortcut for {}", action.label()))
                        .clicked()
                    {
                        self.keybinding_ui.capture = Some(action);
                        self.keybinding_ui.feedback = None;
                        if let Some(id) = self.ctx.memory(|memory| memory.focused()) {
                            self.ctx.memory_mut(|memory| memory.surrender_focus(id));
                        }
                    }
                    if ui
                        .add_enabled(!binding.is_empty(), egui::Button::new("Disable"))
                        .clicked()
                    {
                        self.settings.keybindings.insert(action.id().into(), None);
                        self.keybinding_ui.capture = None;
                        self.keybinding_ui.feedback =
                            Some((format!("{} shortcut disabled.", action.label()), false));
                        changed = true;
                    }
                    if ui
                        .add_enabled(
                            self.settings.keybindings.contains_key(action.id()),
                            egui::Button::new("Reset"),
                        )
                        .clicked()
                    {
                        let mut candidate = self.settings.keybindings.clone();
                        candidate.remove(action.id());
                        match validate(&candidate) {
                            Ok(()) => {
                                self.settings.keybindings = candidate;
                                self.keybinding_ui.capture = None;
                                self.keybinding_ui.feedback = Some((
                                    format!("{} restored to default.", action.label()),
                                    false,
                                ));
                                changed = true;
                            }
                            Err(error) => self.keybinding_ui.feedback = Some((error, true)),
                        }
                    }
                    ui.label(RichText::new(action.group()).size(10.0).color(MUTED));
                });
            });
        }
        changed
    }
}

#[cfg(test)]
#[path = "keybindings_tests.rs"]
mod tests;

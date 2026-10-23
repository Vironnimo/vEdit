//! Shortcut parsing, persistence, input ownership and recording state.
use super::*;
use std::collections::BTreeMap;

pub(super) type Bindings = BTreeMap<String, Option<crate::cli::ShortcutBinding>>;

pub(super) use super::command_registry::{Action, Scope};

#[path = "keyboard_settings.rs"]
mod settings_ui;

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
}

#[cfg(test)]
#[path = "keybindings_tests.rs"]
mod tests;

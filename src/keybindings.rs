//! Shortcut parsing, persistence, input ownership and keyboard preferences.
use super::*;
use std::collections::BTreeMap;

pub(super) type Bindings = BTreeMap<String, Option<String>>;

pub(super) use super::command_registry::Action;

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
        Some(Some(value)) => Stroke::parse(value).ok().into_iter().collect(),
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

/// Defaults added after users could assign their keys elsewhere. Saved choices
/// keep their key; the new command starts without a shortcut instead.
const LATER_DEFAULTS: [Action; 12] = [
    Action::ReopenTab,
    Action::NavigateBack,
    Action::NavigateForward,
    Action::NextProblem,
    Action::PreviousProblem,
    Action::AddCursorAbove,
    Action::AddCursorBelow,
    Action::SelectNextOccurrence,
    Action::SelectAllOccurrences,
    Action::SkipOccurrence,
    Action::CursorUndo,
    Action::CursorsAtLineEnds,
];

pub(super) fn migrate_later_defaults(bindings: &mut Bindings) {
    for action in LATER_DEFAULTS {
        migrate_later_default(bindings, action);
    }
}

/// The Reopen default was added after users could assign Ctrl+Shift+T elsewhere.
/// Preserve those saved choices when upgrading, without changing conflict rules
/// for new assignments or an explicitly configured Reopen action.
#[cfg(test)]
pub(super) fn migrate_reopen_shortcut(bindings: &mut Bindings) {
    migrate_later_default(bindings, Action::ReopenTab);
}

fn migrate_later_default(bindings: &mut Bindings, added: Action) {
    if bindings.contains_key(added.id()) {
        return;
    }
    let defaults = strokes(bindings, added);
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
        bindings.insert(added.id().into(), None);
    }
}

pub(super) fn matches(bindings: &Bindings, action: Action, event: &egui::Event) -> bool {
    let Some(stroke) = Stroke::event(event) else {
        return false;
    };
    match bindings.get(action.id()) {
        Some(Some(value)) => Stroke::parse(value).ok() == Some(stroke),
        Some(None) => false,
        None => default_strokes(action).contains(&stroke),
    }
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
        if !self.shortcut_matches(action, event) {
            return false;
        }
        if action.multicursor()
            && (!source
                || action == Action::CursorUndo && self.tab().is_none_or(|t| !t.vim.multi.active()))
        {
            return false;
        }
        if matches!(
            action,
            Action::Undo
                | Action::Redo
                | Action::ToggleComment
                | Action::DuplicateLine
                | Action::DeleteLine
                | Action::InsertLineAbove
                | Action::InsertLineBelow
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
            if action == Action::OpenFile
                && self
                    .tab()
                    .is_some_and(|tab| tab.vim.mode == VimMode::Normal)
            {
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
                    .color(TEXT),
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
                    .color(if *error { RED } else { MUTED }),
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
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for action in rows {
                ui.push_id(action.id(), |ui| {
                    changed |= self.keyboard_shortcut_row(ui, action)
                });
            }
        });
        changed
    }

    /// One compact row per action: its name on the left and the binding chip on
    /// the right. Disable and Reset appear while the row is hovered or focused;
    /// they stay in the keyboard order after the chip.
    fn keyboard_shortcut_row(&mut self, ui: &mut egui::Ui, action: Action) -> bool {
        let mut changed = false;
        let binding = self.shortcut_label(action);
        let recording = self.keybinding_ui.capture == Some(action);
        let can_disable = !binding.is_empty();
        let can_reset = self.settings.keybindings.contains_key(action.id());
        let (row, _) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), SHORTCUT_ROW_HEIGHT),
            egui::Sense::hover(),
        );
        let chip_id = ui.id().with("binding");
        let disable_id = ui.id().with("disable");
        let reset_id = ui.id().with("reset");
        let focused = ui.ctx().memory(|memory| {
            [chip_id, disable_id, reset_id]
                .into_iter()
                .any(|id| memory.has_focus(id))
        });
        let active = recording || focused || ui.rect_contains_pointer(row);
        if active {
            ui.painter().rect_filled(row, 4.0, SURFACE);
        }

        let chip_text = if recording {
            "Press a shortcut…"
        } else if binding.is_empty() {
            "Unassigned"
        } else {
            &binding
        };
        let chip_galley =
            ui.painter()
                .layout_no_wrap(chip_text.to_owned(), FontId::proportional(11.5), TEXT);
        let chip_size = Vec2::new(chip_galley.size().x + 14.0, 20.0);
        let chip_rect = egui::Rect::from_min_size(
            egui::pos2(
                row.right() - 4.0 - chip_size.x,
                row.center().y - chip_size.y * 0.5,
            ),
            chip_size,
        );
        let action_galley = |label: &str| {
            ui.painter()
                .layout_no_wrap(label.to_owned(), FontId::proportional(11.0), TEXT)
        };
        let mut right = chip_rect.left() - 4.0;
        let mut place = |enabled: bool, galley: Arc<egui::Galley>| {
            enabled.then(|| {
                let rect = egui::Rect::from_min_max(
                    egui::pos2(right - galley.size().x - 14.0, chip_rect.top()),
                    egui::pos2(right, chip_rect.bottom()),
                );
                right = rect.left() - 2.0;
                (rect, galley)
            })
        };
        let reset_place = place(can_reset, action_galley("Reset"));
        let disable_place = place(can_disable, action_galley("Disable"));

        let label_rect = egui::Rect::from_x_y_ranges(row.left() + 8.0..=right - 6.0, row.y_range());
        let label_galley = ui.painter().layout_no_wrap(
            action.label().to_owned(),
            FontId::proportional(12.0),
            TEXT,
        );
        ui.painter()
            .with_clip_rect(label_rect.intersect(ui.clip_rect()))
            .galley(
                egui::pos2(
                    label_rect.left(),
                    row.center().y - label_galley.size().y * 0.5,
                ),
                label_galley,
                TEXT,
            );
        let label = ui.interact(label_rect, ui.id().with("label"), egui::Sense::hover());
        label.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Label, ui.is_enabled(), action.label())
        });
        label.on_hover_text(format!("{} · {}", action.group(), action.context_hint()));

        let chip = ui.interact(chip_rect, chip_id, egui::Sense::click());
        chip.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), chip_text)
        });
        let painter = ui.painter();
        if chip.hovered() {
            painter.rect_filled(chip_rect, 4.0, HOVER);
        } else if !binding.is_empty() || recording {
            painter.rect_filled(chip_rect, 4.0, SURFACE);
        }
        if recording || chip.has_focus() {
            painter.rect_stroke(
                chip_rect,
                4.0,
                egui::Stroke::new(1.5, ACCENT),
                egui::StrokeKind::Inside,
            );
        } else if !binding.is_empty() {
            painter.rect_stroke(
                chip_rect,
                4.0,
                egui::Stroke::new(1.0, crate::palette::BORDER_STRONG),
                egui::StrokeKind::Inside,
            );
        }
        painter.galley_with_override_text_color(
            chip_rect.center() - chip_galley.size() * 0.5,
            chip_galley,
            if binding.is_empty() && !recording && !chip.hovered() {
                MUTED
            } else {
                TEXT
            },
        );
        if chip
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(format!("Record shortcut for {}", action.label()))
            .clicked()
        {
            self.keybinding_ui.capture = Some(action);
            self.keybinding_ui.feedback = None;
            if let Some(id) = self.ctx.memory(|memory| memory.focused()) {
                self.ctx.memory_mut(|memory| memory.surrender_focus(id));
            }
        }

        if let Some((rect, galley)) = disable_place
            && shortcut_row_action(ui, rect, disable_id, "Disable", galley, active)
                .on_hover_text(format!("Remove the shortcut for {}", action.label()))
                .clicked()
        {
            self.settings.keybindings.insert(action.id().into(), None);
            self.keybinding_ui.capture = None;
            self.keybinding_ui.feedback =
                Some((format!("{} shortcut disabled.", action.label()), false));
            changed = true;
        }
        if let Some((rect, galley)) = reset_place
            && shortcut_row_action(ui, rect, reset_id, "Reset", galley, active)
                .on_hover_text(format!(
                    "Restore the default shortcut for {}",
                    action.label()
                ))
                .clicked()
        {
            let mut candidate = self.settings.keybindings.clone();
            candidate.remove(action.id());
            match validate(&candidate) {
                Ok(()) => {
                    self.settings.keybindings = candidate;
                    self.keybinding_ui.capture = None;
                    self.keybinding_ui.feedback =
                        Some((format!("{} restored to default.", action.label()), false));
                    changed = true;
                }
                Err(error) => self.keybinding_ui.feedback = Some((error, true)),
            }
        }
        changed
    }
}

const SHORTCUT_ROW_HEIGHT: f32 = 26.0;

/// A frameless row action that is painted only while its row is active, yet
/// remains focusable so keyboard users can reach it.
fn shortcut_row_action(
    ui: &egui::Ui,
    rect: egui::Rect,
    id: egui::Id,
    label: &str,
    galley: Arc<egui::Galley>,
    row_active: bool,
) -> egui::Response {
    let response = ui.interact(rect, id, egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    if row_active || response.has_focus() {
        let painter = ui.painter();
        if response.hovered() {
            painter.rect_filled(rect, 4.0, HOVER);
        }
        if response.has_focus() {
            painter.rect_stroke(
                rect,
                4.0,
                egui::Stroke::new(1.5, ACCENT),
                egui::StrokeKind::Inside,
            );
        }
        painter.galley_with_override_text_color(
            rect.center() - galley.size() * 0.5,
            galley,
            if response.hovered() || response.has_focus() {
                TEXT
            } else {
                MUTED
            },
        );
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

#[cfg(test)]
#[path = "keybindings_tests.rs"]
mod tests;

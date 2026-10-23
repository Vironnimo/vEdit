//! Preserve native TextEdit semantics on both sides of application commands.
//!
//! A native prefix is rendered once before its command and suffix resume on the
//! next frame. The continuation is bound to the resulting source owner, so new
//! keyboard events follow it while external navigation or edits cancel it.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Command {
    Undo,
    Redo,
    Comment,
    Duplicate,
    DeleteLine,
    InsertLine(bool),
    MoveLine(bool),
    Indent(bool),
    Newline,
}

#[derive(Clone, PartialEq)]
struct Owner {
    pane: u64,
    document: u64,
    revision: u64,
    cursor: usize,
    selection: Range<usize>,
    multi: crate::multicursor::State,
    mode: VimMode,
    widget: Option<egui::Id>,
    vim: bool,
    swap_jk: bool,
    leader: String,
    keybindings: keybindings::Bindings,
    view: crate::cli::DocumentView,
}

#[derive(Clone, Default)]
struct Continuation {
    document: u64,
    pass: u64,
    events: Vec<egui::Event>,
    owner: Option<Owner>,
}

fn continuation_id() -> egui::Id {
    egui::Id::new("source_editor_input_continuation")
}

pub(super) fn native_input(event: &egui::Event) -> bool {
    if let egui::Event::Key {
        key, pressed: true, ..
    } = event
    {
        return !keybindings::modifier_key(*key);
    }
    matches!(
        event,
        egui::Event::Text(_)
            | egui::Event::Paste(_)
            | egui::Event::Copy
            | egui::Event::Cut
            | egui::Event::Ime(_)
    )
}

pub(super) fn command(
    event: &egui::Event,
    history: bool,
    bindings: &std::collections::BTreeMap<String, Option<String>>,
) -> Option<Command> {
    for (action, command) in [
        (keybindings::Action::Undo, Command::Undo),
        (keybindings::Action::Redo, Command::Redo),
        (keybindings::Action::ToggleComment, Command::Comment),
        (keybindings::Action::DuplicateLine, Command::Duplicate),
        (keybindings::Action::DeleteLine, Command::DeleteLine),
        (
            keybindings::Action::InsertLineAbove,
            Command::InsertLine(true),
        ),
        (
            keybindings::Action::InsertLineBelow,
            Command::InsertLine(false),
        ),
        (keybindings::Action::MoveLineUp, Command::MoveLine(false)),
        (keybindings::Action::MoveLineDown, Command::MoveLine(true)),
    ] {
        if (!matches!(command, Command::Undo | Command::Redo) || history)
            && keybindings::matches(bindings, action, event)
        {
            return Some(command);
        }
    }
    let egui::Event::Key {
        key,
        pressed: true,
        modifiers,
        ..
    } = event
    else {
        return None;
    };
    let command = modifiers.command || modifiers.ctrl;
    if !command && !modifiers.alt {
        return match key {
            Key::Tab => Some(Command::Indent(modifiers.shift)),
            Key::Enter => Some(Command::Newline),
            _ => None,
        };
    }
    None
}

/// Vim must finish native editing before any action that observes the buffer.
pub(super) fn vim_barrier(
    event: &egui::Event,
    bindings: &std::collections::BTreeMap<String, Option<String>>,
) -> bool {
    if [keybindings::Action::Undo, keybindings::Action::Redo]
        .into_iter()
        .any(|action| keybindings::matches(bindings, action, event))
    {
        return true;
    }
    match event {
        egui::Event::Copy | egui::Event::Cut | egui::Event::Paste(_) => true,
        egui::Event::Key {
            key,
            pressed: true,
            modifiers,
            ..
        } => {
            *key == Key::Escape
                || (modifiers.ctrl || modifiers.command)
                    && !modifiers.alt
                    && matches!(key, Key::C | Key::X | Key::V)
        }
        _ => false,
    }
}

pub(super) fn defer(ctx: &egui::Context, document: u64, mut events: Vec<egui::Event>) {
    if events.is_empty() {
        return;
    }
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|data| {
        // TextEdit may split an earlier custom command out of the prefix that
        // Vim has just handed it. That earlier suffix precedes Vim's suffix.
        if let Some(previous) = data.remove_temp::<Continuation>(continuation_id())
            && previous.pass == pass
            && previous.document == document
        {
            events.extend(previous.events);
        }
        data.insert_temp(
            continuation_id(),
            Continuation {
                document,
                pass,
                events,
                owner: None,
            },
        );
    });
    ctx.request_repaint();
}

/// Return the first custom action only after TextEdit has seen its native
/// prefix. Any following custom action waits behind the intervening input.
pub(super) fn take_command(
    ctx: &egui::Context,
    document: u64,
    history: bool,
    bindings: &std::collections::BTreeMap<String, Option<String>>,
) -> Option<Command> {
    let mut events = ctx.input_mut(|input| std::mem::take(&mut input.events));
    // Native TextEdit also handles history and Ctrl+Shift+K (as kill-to-end).
    // Once the user changes a binding, retired aliases must stop reaching it.
    // With Vim enabled, active history commands have already been consumed by
    // the Vim handler; retired aliases must not reach native Insert mode either.
    events
        .retain(|event| command(event, history, bindings).is_some() || !native_command_key(event));
    let Some((index, action)) = events.iter().enumerate().find_map(|(index, event)| {
        command(event, history, bindings).map(|command| (index, command))
    }) else {
        ctx.input_mut(|input| input.events = events);
        return None;
    };
    if events[..index].iter().any(native_input) {
        let suffix = events.split_off(index);
        defer(ctx, document, suffix);
        ctx.input_mut(|input| input.events = events);
        return None;
    }
    let key_event = events.remove(index);
    if events
        .get(index)
        .is_some_and(|event| keybindings::shortcut_text_event(event, &key_event))
    {
        events.remove(index);
    }
    if let Some(next) = events.iter().position(|event| {
        command(event, history, bindings).is_some()
            || keybindings::Action::ALL
                .iter()
                .any(|&action| keybindings::matches(bindings, action, event))
    }) {
        let suffix = events.split_off(next);
        defer(ctx, document, suffix);
    }
    ctx.input_mut(|input| input.events = events);
    Some(action)
}

fn native_command_key(event: &egui::Event) -> bool {
    matches!(event, egui::Event::Key {
        key,
        pressed: true,
        modifiers,
        ..
    } if (modifiers.command || modifiers.ctrl) && !modifiers.alt
        && (matches!(key, Key::Z | Key::Y) || *key == Key::K && modifiers.shift))
}

impl VEditApp {
    fn editor_input_owner(&self) -> Option<Owner> {
        let tab = self.tab()?;
        Some(Owner {
            pane: self.current_pane_id(),
            document: tab.id,
            revision: tab.revision,
            cursor: tab.vim.cursor,
            selection: tab.selection.clone(),
            multi: tab.vim.multi.clone(),
            mode: tab.vim.mode,
            widget: self.ctx.memory(|memory| memory.focused()),
            vim: self.settings.vim,
            swap_jk: self.settings.swap_jk,
            leader: self.settings.leader_key.clone(),
            keybindings: self.settings.keybindings.clone(),
            view: tab.agent_view,
        })
    }

    pub(super) fn resume_editor_input(&mut self) {
        let pass = self.ctx.cumulative_pass_nr();
        let pending = self.ctx.data_mut(|data| {
            let pending = data.get_temp::<Continuation>(continuation_id())?;
            (pending.pass < pass)
                .then(|| data.remove_temp::<Continuation>(continuation_id()))
                .flatten()
        });
        let Some(pending) = pending else { return };
        let valid = pending.owner.is_some()
            && pending.owner == self.editor_input_owner()
            && self.tab().is_some_and(|tab| {
                !tab.loading
                    && tab.agent_view == crate::cli::DocumentView::Source
                    && self
                        .ctx
                        .memory(|memory| memory.has_focus(self.document_widget_id(tab.id)))
            })
            && self.palette.is_none()
            && self.picker.is_none()
            && !self.settings_open
            && !self.agent_marks_modal_open()
            && self.save_as.is_none()
            && self.close_action.is_none()
            && self.vim_command.is_none()
            && self.diff.is_none()
            && !self.git_detail_open()
            && !self.navigation_modal_open()
            && !self.navigation_pending()
            && self.screen_jump.is_none()
            && !self.vim_search_active()
            && !self.find_focus
            && self.context_file_dialog.is_none()
            && !egui::Popup::is_any_open(&self.ctx);
        if valid {
            let mut events = pending.events;
            self.ctx.input_mut(|input| {
                events.append(&mut input.events);
                input.events = events;
            });
        }
    }

    pub(super) fn finish_editor_input(&self, document: u64) {
        if !self.render_pane_is_focused() {
            return;
        }
        let owner = self.editor_input_owner();
        self.ctx.data_mut(|data| {
            if let Some(mut pending) = data.get_temp::<Continuation>(continuation_id())
                && pending.document == document
                && pending.owner.is_none()
            {
                pending.owner = owner;
                data.insert_temp(continuation_id(), pending);
            }
        });
    }

    #[cfg(test)]
    pub(super) fn editor_input_pending(&self) -> bool {
        self.ctx
            .data(|data| data.get_temp::<Continuation>(continuation_id()).is_some())
    }
}

#[cfg(test)]
mod binding_tests {
    use super::*;

    fn key(key: Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers {
                command: modifiers.ctrl,
                ..modifiers
            },
        }
    }

    #[test]
    fn default_editing_bindings_use_exact_modifiers_and_both_redo_aliases() {
        let bindings = std::collections::BTreeMap::new();
        assert_eq!(
            command(&key(Key::Z, egui::Modifiers::CTRL), true, &bindings),
            Some(Command::Undo)
        );
        for (key_code, modifiers) in [
            (Key::Y, egui::Modifiers::CTRL),
            (Key::Z, egui::Modifiers::CTRL.plus(egui::Modifiers::SHIFT)),
        ] {
            assert_eq!(
                command(&key(key_code, modifiers), true, &bindings),
                Some(Command::Redo)
            );
            assert_eq!(command(&key(key_code, modifiers), false, &bindings), None);
        }
        assert_eq!(
            command(&key(Key::Slash, egui::Modifiers::CTRL), false, &bindings),
            Some(Command::Comment)
        );
        assert_eq!(
            command(
                &key(
                    Key::Slash,
                    egui::Modifiers::CTRL
                        .plus(egui::Modifiers::SHIFT)
                        .plus(egui::Modifiers::ALT)
                ),
                false,
                &bindings
            ),
            None
        );
        assert_eq!(
            command(
                &key(
                    Key::ArrowDown,
                    egui::Modifiers::ALT.plus(egui::Modifiers::SHIFT)
                ),
                false,
                &bindings
            ),
            Some(Command::Duplicate)
        );
    }
}

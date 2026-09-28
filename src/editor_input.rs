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
    mode: VimMode,
    widget: Option<egui::Id>,
    vim: bool,
    swap_jk: bool,
    leader: String,
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
    matches!(
        event,
        egui::Event::Text(_)
            | egui::Event::Paste(_)
            | egui::Event::Copy
            | egui::Event::Cut
            | egui::Event::Ime(_)
            | egui::Event::Key { pressed: true, .. }
    )
}

pub(super) fn command(event: &egui::Event, history: bool) -> Option<Command> {
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
    if command && !modifiers.alt {
        return match key {
            Key::Z if history => Some(if modifiers.shift {
                Command::Redo
            } else {
                Command::Undo
            }),
            Key::Y if history => Some(Command::Redo),
            Key::Slash => Some(Command::Comment),
            _ => None,
        };
    }
    if !command && modifiers.alt && modifiers.shift && *key == Key::ArrowDown {
        return Some(Command::Duplicate);
    }
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
pub(super) fn vim_barrier(event: &egui::Event) -> bool {
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
                    && matches!(key, Key::C | Key::X | Key::V | Key::Z | Key::Y)
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
pub(super) fn take_command(ctx: &egui::Context, document: u64, history: bool) -> Option<Command> {
    let mut events = ctx.input_mut(|input| std::mem::take(&mut input.events));
    let Some((index, action)) = events
        .iter()
        .enumerate()
        .find_map(|(index, event)| command(event, history).map(|command| (index, command)))
    else {
        ctx.input_mut(|input| input.events = events);
        return None;
    };
    if events[..index].iter().any(native_input) {
        let suffix = events.split_off(index);
        defer(ctx, document, suffix);
        ctx.input_mut(|input| input.events = events);
        return None;
    }
    events.remove(index);
    if let Some(next) = events
        .iter()
        .position(|event| command(event, history).is_some())
    {
        let suffix = events.split_off(next);
        defer(ctx, document, suffix);
    }
    ctx.input_mut(|input| input.events = events);
    Some(action)
}

impl MorrowApp {
    fn editor_input_owner(&self) -> Option<Owner> {
        let tab = self.tab()?;
        Some(Owner {
            pane: self.current_pane_id(),
            document: tab.id,
            revision: tab.revision,
            cursor: tab.vim.cursor,
            selection: tab.selection.clone(),
            mode: tab.vim.mode,
            widget: self.ctx.memory(|memory| memory.focused()),
            vim: self.settings.vim,
            swap_jk: self.settings.swap_jk,
            leader: self.settings.leader_key.clone(),
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

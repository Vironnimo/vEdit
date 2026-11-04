//! Recorded source input reuses the ordinary editor/search/Ex input owners.
//! Playback yields between input events and waits for navigation/native edits.
use super::*;
use std::sync::Arc;

const MAX_RECORDING_BYTES: usize = 1024 * 1024;
const MAX_RECORDING_EVENTS: usize = 20_000;
const MAX_PLAYBACK_EVENTS: usize = 100_000;
const MAX_NESTING: usize = 100;
const MAX_TEXT_CHARS_PER_EVENT: usize = 64;

#[derive(Clone, Copy)]
enum Pending {
    Record,
    Play(usize),
}

struct Recording {
    register: char,
    events: Vec<egui::Event>,
    bytes: usize,
}

struct Execution {
    events: Arc<[egui::Event]>,
    next: usize,
    repetitions: usize,
}

#[derive(Default)]
pub(super) struct State {
    recording: Option<Recording>,
    pending: Option<Pending>,
    executions: Vec<Execution>,
    after: Vec<egui::Event>,
    last: Option<char>,
    steps: usize,
    synthetic: bool,
    waiting_definition: Option<u64>,
    owner: Option<editor_input::Owner>,
    owner_loading: bool,
    navigation_arrived: Option<editor_input::Owner>,
    checkpoints: Vec<(u64, crate::vim::MacroHistory)>,
    message: Option<String>,
}

pub(super) enum Input {
    Pass,
    Consumed,
    Play(char, usize),
}

pub(super) fn key(key: Key, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}

fn key_text(key: Key, modifiers: egui::Modifiers) -> String {
    if (modifiers.ctrl || modifiers.command)
        && !modifiers.alt
        && let Some(ch) = key.name().chars().next().filter(|_| key.name().len() == 1)
        && ch.is_ascii_alphabetic()
    {
        return ((ch.to_ascii_uppercase() as u8 - b'A' + 1) as char).to_string();
    }
    match key {
        Key::Escape => "\u{1b}".into(),
        Key::Enter => "\r".into(),
        Key::Backspace => "\u{8}".into(),
        Key::Tab => "\t".into(),
        Key::Delete => "\u{7f}".into(),
        Key::ArrowLeft => "<Left>".into(),
        Key::ArrowRight => "<Right>".into(),
        Key::ArrowUp => "<Up>".into(),
        Key::ArrowDown => "<Down>".into(),
        _ => format!("<{}>", key.name()),
    }
}

fn event_text(event: &egui::Event) -> String {
    match event {
        egui::Event::Text(text)
        | egui::Event::Paste(text)
        | egui::Event::Ime(egui::ImeEvent::Commit(text)) => text.clone(),
        egui::Event::Key {
            key,
            modifiers,
            pressed: true,
            ..
        } => key_text(*key, *modifiers),
        egui::Event::Copy => "\u{3}".into(),
        egui::Event::Cut => "\u{18}".into(),
        _ => String::new(),
    }
}

/// Registers yanked as text can be executed too. Recording metadata preserves
/// exact non-text input; the text form uses control characters and key names.
pub(super) fn text_events(mut text: &str) -> Vec<egui::Event> {
    let mut events = Vec::new();
    while !text.is_empty() {
        let named = [
            ("<Left>", Key::ArrowLeft),
            ("<Right>", Key::ArrowRight),
            ("<Up>", Key::ArrowUp),
            ("<Down>", Key::ArrowDown),
            ("<Home>", Key::Home),
            ("<End>", Key::End),
            ("<Escape>", Key::Escape),
            ("<Esc>", Key::Escape),
            ("<Enter>", Key::Enter),
            ("<CR>", Key::Enter),
            ("<Tab>", Key::Tab),
            ("<Backspace>", Key::Backspace),
            ("<BS>", Key::Backspace),
            ("<Delete>", Key::Delete),
        ]
        .into_iter()
        .find(|(name, _)| text.starts_with(name));
        if let Some((name, named)) = named {
            events.push(key(named, egui::Modifiers::NONE));
            text = &text[name.len()..];
            continue;
        }
        let ch = text.chars().next().unwrap();
        text = &text[ch.len_utf8()..];
        if ch == '\r' && text.starts_with('\n') {
            text = &text[1..];
        }
        let special = match ch {
            '\u{1b}' => Some(Key::Escape),
            '\r' | '\n' => Some(Key::Enter),
            '\t' => Some(Key::Tab),
            '\u{8}' => Some(Key::Backspace),
            '\u{7f}' => Some(Key::Delete),
            _ => None,
        };
        if let Some(special) = special {
            events.push(key(special, egui::Modifiers::NONE));
        } else if ('\u{1}'..='\u{1a}').contains(&ch) {
            let name = ((ch as u8 - 1 + b'A') as char).to_string();
            if let Some(special) = Key::from_name(&name) {
                events.push(key(
                    special,
                    egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
                ));
            }
        } else {
            if let Some(egui::Event::Text(previous)) = events.last_mut()
                && previous.chars().count() < MAX_TEXT_CHARS_PER_EVENT
            {
                previous.push(ch);
            } else {
                events.push(egui::Event::Text(ch.to_string()));
            }
        }
    }
    events
}

impl State {
    pub(super) fn active(&self) -> bool {
        !self.executions.is_empty()
    }

    pub(super) fn label(&self) -> Option<String> {
        self.recording
            .as_ref()
            .map(|recording| format!("recording @{}", recording.register.to_ascii_lowercase()))
            .or_else(|| self.active().then(|| "playing macro".into()))
    }

    pub(super) fn command_prefix(&self) -> Option<String> {
        self.pending.map(|pending| match pending {
            Pending::Record => "q".into(),
            Pending::Play(count) if count > 1 => format!("{count}@"),
            Pending::Play(_) => "@".into(),
        })
    }

    pub(super) fn record_event(&mut self, event: &egui::Event) {
        if self.synthetic || self.active() {
            return;
        }
        let Some(recording) = &mut self.recording else {
            return;
        };
        let event = match event {
            egui::Event::Text(_) | egui::Event::Paste(_) | egui::Event::Copy | egui::Event::Cut => {
                event.clone()
            }
            egui::Event::Ime(egui::ImeEvent::Commit(text)) => egui::Event::Text(text.clone()),
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } if !keybindings::modifier_key(*key)
                && (modifiers.ctrl
                    || modifiers.command
                    || modifiers.alt
                    || matches!(
                        key,
                        Key::Escape
                            | Key::Enter
                            | Key::Tab
                            | Key::Backspace
                            | Key::Delete
                            | Key::ArrowLeft
                            | Key::ArrowRight
                            | Key::ArrowUp
                            | Key::ArrowDown
                            | Key::Home
                            | Key::End
                            | Key::PageUp
                            | Key::PageDown
                    )) =>
            {
                event.clone()
            }
            _ => return,
        };
        recording.bytes = recording.bytes.saturating_add(event_text(&event).len());
        if recording.bytes > MAX_RECORDING_BYTES || recording.events.len() >= MAX_RECORDING_EVENTS {
            self.recording = None;
            self.message = Some("Macro recording exceeded its size limit".into());
            return;
        }
        if let egui::Event::Text(text) = &event
            && let Some(egui::Event::Text(previous)) = recording.events.last_mut()
        {
            previous.push_str(text);
        } else {
            recording.events.push(event);
        }
    }

    pub(super) fn character(&mut self, vim: &mut VimState, ch: char) -> Input {
        if let Some(pending) = self.pending.take() {
            match pending {
                Pending::Record => {
                    if ch.is_ascii_alphanumeric() || ch == '"' {
                        self.recording = Some(Recording {
                            register: ch,
                            events: Vec::new(),
                            bytes: 0,
                        });
                    } else {
                        self.message = Some(format!("Invalid macro register: {ch}"));
                    }
                    return Input::Consumed;
                }
                Pending::Play(count) => {
                    self.record_event(&egui::Event::Text(ch.to_string()));
                    let register = if ch == '@' {
                        self.last
                    } else {
                        Some(ch.to_ascii_lowercase())
                    };
                    return if let Some(register) = register {
                        Input::Play(register, count)
                    } else {
                        self.message = Some("No previously executed macro".into());
                        Input::Consumed
                    };
                }
            }
        }
        let pending = vim.pending_command();
        let idle = vim.mode == VimMode::Normal
            && !vim.expects_literal_character()
            && pending.chars().all(|ch| ch.is_ascii_digit());
        if idle && ch == 'q' {
            vim.cancel_pending_command();
            if self.active() || self.synthetic {
                return Input::Consumed;
            }
            if let Some(recording) = self.recording.take() {
                let text = recording.events.iter().map(event_text).collect();
                let mut bank = vim.register_bank().clone();
                if !bank.set_macro(
                    recording.register,
                    Yank {
                        text,
                        linewise: false,
                        block_width: None,
                    },
                    recording.events,
                ) {
                    self.message = Some("Macro register exceeds its size limit".into());
                }
                vim.set_register_bank(bank);
            } else {
                self.pending = Some(Pending::Record);
            }
            return Input::Consumed;
        }
        if idle && ch == '@' {
            self.record_event(&egui::Event::Text(ch.to_string()));
            let count = vim.take_search_count();
            vim.cancel_pending_command();
            self.pending = Some(Pending::Play(count));
            return Input::Consumed;
        }
        self.record_event(&egui::Event::Text(ch.to_string()));
        Input::Pass
    }

    pub(super) fn cancel_prefix(&mut self) -> bool {
        self.pending.take().is_some()
    }

    pub(super) fn failed_motion(&mut self, has_message: bool) {
        self.abort();
        if !has_message {
            self.message = Some("Macro stopped: motion has no target".into());
        }
    }

    /// An asynchronous source command keeps its unconsumed synthetic input
    /// on the execution stack, rather than releasing it as human input.
    pub(super) fn defer_playback_events(&mut self, events: Vec<egui::Event>) {
        if !events.is_empty() {
            self.executions.push(Execution {
                events: events.into(),
                next: 0,
                repetitions: 1,
            });
        }
    }

    pub(super) fn play(
        &mut self,
        register: char,
        count: usize,
        value: Option<crate::vim::RegisterValue>,
        suffix: Vec<egui::Event>,
    ) {
        let synthetic = self.active() || self.synthetic;
        let Some(value) = value else {
            self.abort();
            self.message = Some(format!("Macro register is empty: {register}"));
            if !synthetic {
                self.after.extend(suffix);
            }
            return;
        };
        if value.yank.text.chars().count() > MAX_PLAYBACK_EVENTS {
            self.abort();
            self.message = Some("Macro exceeds the execution size limit".into());
            if !synthetic {
                self.after.extend(suffix);
            }
            return;
        }
        let events = value
            .macro_events
            .unwrap_or_else(|| text_events(&value.yank.text));
        if events.is_empty() {
            self.abort();
            self.message = Some(format!("Macro register is empty: {register}"));
            if !synthetic {
                self.after.extend(suffix);
            }
            return;
        }
        if self.executions.len() >= MAX_NESTING {
            self.abort();
            self.message = Some("Macro nesting limit reached".into());
            return;
        }
        if self.active() {
            if !suffix.is_empty() {
                self.executions.push(Execution {
                    events: suffix.into(),
                    next: 0,
                    repetitions: 1,
                });
            }
        } else {
            self.steps = 0;
            self.after.extend(suffix);
        }
        let mut chunks = Vec::new();
        let mut cost = 0usize;
        for event in events {
            match event {
                egui::Event::Text(text) => {
                    let mut chunk = String::new();
                    for (index, ch) in text.chars().enumerate() {
                        cost += 1;
                        if cost > MAX_PLAYBACK_EVENTS {
                            break;
                        }
                        if index > 0 && index % MAX_TEXT_CHARS_PER_EVENT == 0 {
                            chunks.push(egui::Event::Text(std::mem::take(&mut chunk)));
                        }
                        chunk.push(ch);
                    }
                    if !chunk.is_empty() {
                        chunks.push(egui::Event::Text(chunk));
                    }
                }
                event => {
                    cost += 1;
                    chunks.push(event);
                }
            }
            if cost > MAX_PLAYBACK_EVENTS {
                self.abort();
                self.message = Some("Macro exceeds the execution size limit".into());
                return;
            }
        }
        self.executions.push(Execution {
            events: chunks.into(),
            next: 0,
            repetitions: count.max(1),
        });
        self.last = Some(register);
    }

    pub(super) fn play_register(
        &mut self,
        register: char,
        count: usize,
        vim: &VimState,
        suffix: Vec<egui::Event>,
    ) {
        if vim
            .register_value(register)
            .is_some_and(|yank| yank.text.chars().count() > MAX_PLAYBACK_EVENTS)
        {
            let synthetic = self.active() || self.synthetic;
            self.abort();
            self.message = Some("Macro exceeds the execution size limit".into());
            if !synthetic {
                self.after.extend(suffix);
            }
            return;
        }
        let value = vim.register_bank().get(register).cloned().or_else(|| {
            vim.register_value(register)
                .map(|yank| crate::vim::RegisterValue {
                    yank: yank.clone(),
                    macro_events: None,
                })
        });
        self.play(register, count, value, suffix);
    }

    fn next(&mut self) -> Option<egui::Event> {
        loop {
            let execution = self.executions.last_mut()?;
            if execution.next == execution.events.len() {
                if execution.repetitions > 1 {
                    execution.repetitions -= 1;
                    execution.next = 0;
                } else {
                    self.executions.pop();
                    continue;
                }
            }
            if self.steps >= MAX_PLAYBACK_EVENTS {
                self.abort();
                self.message = Some("Macro execution limit reached".into());
                return None;
            }
            let event = execution.events[execution.next].clone();
            self.steps += if let egui::Event::Text(text) = &event {
                text.chars().count()
            } else {
                1
            };
            if self.steps > MAX_PLAYBACK_EVENTS {
                self.abort();
                self.message = Some("Macro execution limit reached".into());
                return None;
            }
            execution.next += 1;
            return Some(event);
        }
    }

    fn abort(&mut self) {
        self.executions.clear();
        self.pending = None;
        self.waiting_definition = None;
    }
}

impl VEditApp {
    pub(super) fn vim_macro_navigation_arrived(&mut self) {
        self.vim_macros.navigation_arrived = self.editor_input_owner();
    }
    pub(super) fn cancel_vim_macro(&mut self) {
        if self
            .vim_macros
            .waiting_definition
            .take()
            .is_some_and(|request| {
                self.definition_request
                    .is_some_and(|(pending, _)| pending == request)
            })
        {
            self.definition_request = None;
            self.definition_origin = None;
            if self
                .status
                .as_ref()
                .is_some_and(|(message, _, _)| message == "Finding definition…")
            {
                self.status = None;
            }
        }
        self.vim_macros.abort();
        self.cancel_macro_file_mark();
    }

    pub(super) fn wait_vim_macro_definition(&mut self) {
        if self.vim_macros.active() {
            self.vim_macros.waiting_definition =
                self.definition_request.map(|(request, _)| request);
            if self.vim_macros.waiting_definition.is_none() {
                self.cancel_vim_macro();
            }
        }
    }

    pub(super) fn complete_vim_macro_definition(&mut self, request: u64, found: bool) {
        if self.vim_macros.waiting_definition == Some(request) {
            self.vim_macros.waiting_definition = None;
            if found {
                self.vim_macro_navigation_arrived();
            }
            if !found {
                self.cancel_vim_macro();
            }
        }
    }

    pub(super) fn checkpoint_vim_macro_document(&mut self) {
        if !self.vim_macros.active() {
            return;
        }
        if let Some(tab) = self.tab().filter(|tab| !tab.loading) {
            let id = tab.id;
            if !self
                .vim_macros
                .checkpoints
                .iter()
                .any(|(document, _)| *document == id)
            {
                let checkpoint = tab.vim.macro_checkpoint(&tab.text);
                self.vim_macros.checkpoints.push((id, checkpoint));
            }
        }
    }

    pub(super) fn validate_vim_macro_owner(&mut self) {
        if (self.vim_macros.active() || !self.vim_macros.checkpoints.is_empty())
            && self.vim_macros.owner.as_ref().is_some_and(|owner| {
                let current = self.editor_input_owner();
                current.as_ref() != Some(owner)
                    && !self
                        .vim_macros
                        .navigation_arrived
                        .as_ref()
                        .is_some_and(|arrival| current.as_ref() == Some(arrival))
                    && !(self.vim_macros.owner_loading
                        && current
                            .as_ref()
                            .is_some_and(|current| owner.same_view(current)))
            })
        {
            self.cancel_vim_macro();
            // Commands already have valid history. An external edit must not
            // be absorbed into the interrupted macro's history checkpoint.
            self.vim_macros.checkpoints.clear();
            self.vim_input_status("Macro stopped: the editor context changed");
        }
    }

    pub(super) fn prepare_vim_macros(&mut self) {
        self.vim_macros.synthetic = false;
        self.validate_vim_macro_owner();
        self.vim_macros.navigation_arrived = None;
        if self.vim_macros.waiting_definition.is_some_and(|request| {
            self.definition_request
                .is_none_or(|(pending, _)| pending != request)
        }) {
            self.cancel_vim_macro();
        }
        if !self.settings.vim {
            self.vim_macros.abort();
            self.vim_macros.recording = None;
        }
        if self.vim_macros.active() {
            let interrupt = self.ctx.input(|input| input.pointer.any_pressed() || input.events.iter().any(|event|
                matches!(event, egui::Event::Key { key: Key::Escape, pressed: true, .. })
                || matches!(event, egui::Event::Key { key: Key::C, pressed: true, modifiers, .. } if modifiers.ctrl || modifiers.command)));
            if interrupt
                || self.chrome_blocked()
                || self.change_review_visible()
                || self.tab().is_none()
            {
                self.cancel_vim_macro();
            } else {
                self.checkpoint_vim_macro_document();
                let waiting = self.editor_input_pending()
                    || self.navigation_pending()
                    || self.vim_file_mark_pending()
                    || self.vim_macros.waiting_definition.is_some()
                    || self.pending_location.is_some()
                    || self.pending_jump.is_some()
                    || self.tab().is_none_or(|tab| tab.loading)
                    || self.vim_command.is_none()
                        && self.tab().is_some_and(|tab| {
                            !self
                                .ctx
                                .memory(|memory| memory.has_focus(self.document_widget_id(tab.id)))
                        })
                    || self.find.waiting();
                let mut incoming = self
                    .ctx
                    .input_mut(|input| std::mem::take(&mut input.events));
                self.vim_macros
                    .after
                    .extend(incoming.extract_if(.., |event| editor_input::native_input(event)));
                self.ctx.input_mut(|input| input.events = incoming);
                if !waiting {
                    if let Some(event) = self.vim_macros.next() {
                        self.vim_macros.synthetic = true;
                        self.ctx.input_mut(|input| {
                            let release = if let egui::Event::Key {
                                key,
                                modifiers,
                                physical_key,
                                ..
                            } = &event
                            {
                                Some(egui::Event::Key {
                                    key: *key,
                                    physical_key: *physical_key,
                                    pressed: false,
                                    repeat: false,
                                    modifiers: *modifiers,
                                })
                            } else {
                                None
                            };
                            input.events.push(event);
                            if let Some(release) = release {
                                input.events.push(release);
                            }
                        });
                    } else {
                        self.finish_vim_macros();
                    }
                }
                self.ctx.request_repaint();
            }
        }
        if !self.vim_macros.active() && !self.vim_macros.after.is_empty() {
            let mut after = std::mem::take(&mut self.vim_macros.after);
            self.ctx.input_mut(|input| {
                after.append(&mut input.events);
                input.events = after;
            });
        }
        if let Some(message) = self.vim_macros.message.take() {
            self.vim_input_status(message);
        }
    }

    pub(super) fn finish_vim_macros(&mut self) {
        if let Some(message) = self.vim_macros.message.take() {
            self.vim_input_status(message);
        }
        if self.vim_macros.active() {
            self.vim_macros.owner = self.editor_input_owner();
            self.vim_macros.owner_loading = self.tab().is_some_and(|tab| tab.loading);
            return;
        }
        let checkpoints = std::mem::take(&mut self.vim_macros.checkpoints);
        for (id, checkpoint) in checkpoints {
            if let Some(tab) = self.tab_by_id_mut(id) {
                if tab.vim.mode == VimMode::Insert {
                    self.vim_macros.checkpoints.push((id, checkpoint));
                } else {
                    tab.vim.finish_macro(&tab.text, checkpoint);
                }
            }
        }
        self.vim_macros.owner = if self.vim_macros.checkpoints.is_empty() {
            None
        } else {
            self.editor_input_owner()
        };
    }
}

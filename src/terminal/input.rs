//! Translate terminal input without invoking editor commands.
use super::*;
use egui::{Event, Key, Modifiers};

impl TerminalSession {
    pub(crate) fn handle_events(&mut self, events: &[Event]) {
        let mode = *self.term.lock().unwrap().mode();
        let mut modifiers = Modifiers::NONE;
        let mut bytes = Vec::new();
        for (index, event) in events.iter().enumerate() {
            let next = events.get(index + 1);
            match event {
                Event::Text(text) => {
                    if modifiers.alt && !modifiers.ctrl {
                        bytes.push(0x1b);
                    }
                    bytes.extend_from_slice(text.as_bytes());
                }
                Event::Ime(egui::ImeEvent::Commit(text)) => {
                    self.preedit.clear();
                    bytes.extend_from_slice(text.as_bytes());
                }
                Event::Ime(egui::ImeEvent::Preedit { text, .. }) => self.preedit.clone_from(text),
                Event::Paste(text) => {
                    let text = text.replace(['\x1b', '\0'], "");
                    if mode.contains(TermMode::BRACKETED_PASTE) {
                        bytes.extend_from_slice(b"\x1b[200~");
                        bytes.extend_from_slice(text.as_bytes());
                        bytes.extend_from_slice(b"\x1b[201~");
                    } else {
                        bytes.extend_from_slice(
                            text.replace("\r\n", "\r").replace('\n', "\r").as_bytes(),
                        );
                    }
                }
                Event::Copy => {
                    let copy_only = index.checked_sub(1).and_then(|index| events.get(index)).is_some_and(|event|
                        matches!(event, Event::Key { key: Key::C, pressed: true, modifiers, .. } if (modifiers.ctrl || modifiers.command) && modifiers.shift));
                    self.copy_or_interrupt(&mut bytes, copy_only);
                }
                Event::Cut => bytes.push(0x18),
                Event::ModifiersChanged(value) => modifiers = normalize_modifiers(*value),
                Event::Key {
                    key,
                    pressed: true,
                    modifiers: value,
                    ..
                } => {
                    let value = normalize_modifiers(*value);
                    modifiers = value;
                    if value.ctrl && matches!(key, Key::C) {
                        if !matches!(next, Some(Event::Copy)) {
                            self.copy_or_interrupt(&mut bytes, value.shift);
                        }
                    } else if value.ctrl && value.shift && *key == Key::V {
                        if !matches!(next, Some(Event::Paste(_))) {
                            self.ctx
                                .send_viewport_cmd(egui::ViewportCommand::RequestPaste);
                        }
                    } else if value.ctrl && *key == Key::V && matches!(next, Some(Event::Paste(_)))
                        || value.ctrl && *key == Key::X && matches!(next, Some(Event::Cut))
                        // AltGr characters are supplied as text. Translating
                        // Ctrl+Alt+Q as a control key would corrupt '@'.
                        || value.ctrl && value.alt && matches!(next, Some(Event::Text(_)))
                    {
                        continue;
                    } else if value.shift && matches!(key, Key::PageUp | Key::PageDown) {
                        let rows = i32::from(self.size.rows);
                        let lines = if *key == Key::PageUp { rows } else { -rows };
                        self.term
                            .lock()
                            .unwrap()
                            .scroll_display(Scroll::Delta(lines));
                    } else if let Some(encoded) = key_bytes(*key, value, mode) {
                        bytes.extend(encoded);
                    }
                }
                _ => {}
            }
        }
        if !bytes.is_empty() {
            let mut term = self.term.lock().unwrap();
            term.scroll_display(Scroll::Bottom);
            term.selection = None;
            drop(term);
            self.write(bytes);
        }
    }

    fn copy_or_interrupt(&self, bytes: &mut Vec<u8>, copy_only: bool) {
        if let Some(text) = self
            .term
            .lock()
            .unwrap()
            .selection_to_string()
            .filter(|text| !text.is_empty())
        {
            self.ctx.copy_text(text);
        } else if !copy_only {
            bytes.push(0x03);
        }
    }
}

fn normalize_modifiers(mut modifiers: Modifiers) -> Modifiers {
    modifiers.ctrl |= modifiers.command;
    modifiers
}

fn key_bytes(key: Key, modifiers: Modifiers, mode: TermMode) -> Option<Vec<u8>> {
    let parameter =
        1 + u8::from(modifiers.shift) + 2 * u8::from(modifiers.alt) + 4 * u8::from(modifiers.ctrl);
    let suffix = match key {
        Key::ArrowUp => Some('A'),
        Key::ArrowDown => Some('B'),
        Key::ArrowRight => Some('C'),
        Key::ArrowLeft => Some('D'),
        Key::Home => Some('H'),
        Key::End => Some('F'),
        _ => None,
    };
    if let Some(suffix) = suffix {
        return Some(
            if parameter != 1 {
                format!("\x1b[1;{parameter}{suffix}")
            } else if mode.contains(TermMode::APP_CURSOR) {
                format!("\x1bO{suffix}")
            } else {
                format!("\x1b[{suffix}")
            }
            .into_bytes(),
        );
    }
    let number = match key {
        Key::Insert => Some(2),
        Key::Delete => Some(3),
        Key::PageUp => Some(5),
        Key::PageDown => Some(6),
        Key::F5 => Some(15),
        Key::F6 => Some(17),
        Key::F7 => Some(18),
        Key::F8 => Some(19),
        Key::F9 => Some(20),
        Key::F10 => Some(21),
        Key::F11 => Some(23),
        Key::F12 => Some(24),
        _ => None,
    };
    if let Some(number) = number {
        return Some(
            if parameter == 1 {
                format!("\x1b[{number}~")
            } else {
                format!("\x1b[{number};{parameter}~")
            }
            .into_bytes(),
        );
    }
    if matches!(key, Key::F1 | Key::F2 | Key::F3 | Key::F4) {
        let suffix = match key {
            Key::F1 => 'P',
            Key::F2 => 'Q',
            Key::F3 => 'R',
            _ => 'S',
        };
        return Some(
            if parameter == 1 {
                format!("\x1bO{suffix}")
            } else {
                format!("\x1b[1;{parameter}{suffix}")
            }
            .into_bytes(),
        );
    }
    let control = match key {
        Key::Enter => Some(b'\r'),
        Key::Escape => Some(0x1b),
        Key::Backspace => Some(if modifiers.ctrl { 0x08 } else { 0x7f }),
        Key::Tab if modifiers.shift => return Some(b"\x1b[Z".to_vec()),
        Key::Tab => Some(b'\t'),
        _ if modifiers.ctrl => match key {
            Key::A => Some(1),
            Key::B => Some(2),
            Key::C => Some(3),
            Key::D => Some(4),
            Key::E => Some(5),
            Key::F => Some(6),
            Key::G => Some(7),
            Key::H => Some(8),
            Key::I => Some(9),
            Key::J => Some(10),
            Key::K => Some(11),
            Key::L => Some(12),
            Key::M => Some(13),
            Key::N => Some(14),
            Key::O => Some(15),
            Key::P => Some(16),
            Key::Q => Some(17),
            Key::R => Some(18),
            Key::S => Some(19),
            Key::T => Some(20),
            Key::U => Some(21),
            Key::V => Some(22),
            Key::W => Some(23),
            Key::X => Some(24),
            Key::Y => Some(25),
            Key::Z => Some(26),
            Key::Space | Key::Num2 => Some(0),
            Key::OpenBracket => Some(27),
            Key::Backslash => Some(28),
            Key::CloseBracket => Some(29),
            Key::Num6 => Some(30),
            Key::Minus => Some(31),
            _ => None,
        },
        _ => None,
    }?;
    let mut bytes = Vec::with_capacity(2);
    if modifiers.alt {
        bytes.push(0x1b);
    }
    bytes.push(control);
    Some(bytes)
}

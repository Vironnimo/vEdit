//! Session-shared Vim register contents; selection of a register is pane-local.
use super::*;
use eframe::egui;
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, Debug)]
pub struct RegisterValue {
    pub yank: Yank,
    /// Physical input is retained for macros; ordinary text writes invalidate it.
    pub macro_events: Option<Vec<egui::Event>>,
}

/// Clones share immutable payloads, so switching Documents does not copy every
/// register's text or recorded input on every frame.
#[derive(Clone, Debug, Default)]
pub struct RegisterBank {
    values: BTreeMap<char, Arc<RegisterValue>>,
}

impl RegisterBank {
    pub fn get(&self, name: char) -> Option<&RegisterValue> {
        self.values
            .get(&name.to_ascii_lowercase())
            .map(AsRef::as_ref)
    }

    #[cfg(test)]
    pub fn macro_events(&self, name: char) -> Option<&[egui::Event]> {
        self.get(name)?.macro_events.as_deref()
    }

    pub(super) fn clear(&mut self, name: char) {
        self.values.remove(&name.to_ascii_lowercase());
    }

    pub fn set_text(&mut self, name: char, yank: Yank) -> bool {
        self.write(
            name,
            RegisterValue {
                yank,
                macro_events: None,
            },
        )
    }

    pub fn set_macro(&mut self, name: char, yank: Yank, events: Vec<egui::Event>) -> bool {
        self.write(
            name,
            RegisterValue {
                yank,
                macro_events: Some(events),
            },
        )
    }

    fn write(&mut self, name: char, mut value: RegisterValue) -> bool {
        let key = name.to_ascii_lowercase();
        if name.is_ascii_uppercase()
            && let Some(previous) = self.get(key)
        {
            if previous
                .yank
                .text
                .len()
                .saturating_add(value.yank.text.len())
                > MAX_PASTE_BYTES
            {
                return false;
            }
            value.yank = appended(&previous.yank, &value.yank);
            value.macro_events = match (&previous.macro_events, value.macro_events) {
                (Some(old), Some(new)) => Some(old.iter().cloned().chain(new).collect()),
                _ => None,
            };
        }
        if value.yank.text.len() <= MAX_PASTE_BYTES {
            self.values.insert(key, Arc::new(value));
            true
        } else {
            false
        }
    }

    fn push_delete(&mut self, yank: Yank) {
        for index in (2..=9).rev() {
            let previous = char::from(b'0' + index - 1);
            let key = char::from(b'0' + index);
            if let Some(value) = self.values.get(&previous).cloned() {
                self.values.insert(key, value);
            } else {
                self.values.remove(&key);
            }
        }
        self.set_text('1', yank);
    }
}

fn appended(old: &Yank, new: &Yank) -> Yank {
    let mut text = old.text.clone();
    if old.block_width.is_some() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&new.text);
    let linewise = new.linewise;
    if linewise && !text.ends_with('\n') {
        text.push('\n');
    }
    Yank {
        text,
        linewise,
        block_width: new.block_width,
    }
}

impl VimState {
    pub fn register_bank(&self) -> &RegisterBank {
        &self.register_bank
    }

    pub fn set_register_bank(&mut self, bank: RegisterBank) {
        self.register_bank = bank;
        if let Some(value) = self.register_bank.get('"') {
            self.register = Some(value.yank.clone());
        }
        if let Some(value) = self.register_bank.get('0') {
            self.yank_register = Some(value.yank.clone());
        }
    }

    pub fn register_value(&self, name: char) -> Option<&Yank> {
        match name {
            '"' => self.register.as_ref(),
            '0' => self
                .register_bank
                .get('0')
                .map(|value| &value.yank)
                .or(self.yank_register.as_ref()),
            '_' => None,
            _ => self.register_bank.get(name).map(|value| &value.yank),
        }
    }

    pub(super) fn valid_register(name: char) -> bool {
        name.is_ascii_alphanumeric() || matches!(name, '"' | '-' | '_')
    }

    /// Returns the unnamed register's new contents for app synchronization.
    /// Named appends expose the complete value, not just the latest excerpt.
    pub(super) fn capture_register(&mut self, yank: Yank, deleting: bool) -> Option<Yank> {
        let target = self.selected_register;
        let numbered = std::mem::take(&mut self.numbered_delete);
        if target == Some('_') {
            return None;
        }
        if self.collecting_registers {
            self.register_capture = Some((target, deleting, numbered));
            return Some(yank);
        }
        if deleting {
            if numbered || yank.linewise || yank.text.contains('\n') {
                self.register_bank.push_delete(yank.clone());
            }
            if !yank.linewise && !yank.text.contains('\n') && target.is_none() {
                self.register_bank.set_text('-', yank.clone());
            }
        }
        let value = match target {
            Some('"' | '0') => {
                self.set_yank_register(Some(yank.clone()));
                yank
            }
            Some(name) => {
                if self.register_bank.set_text(name, yank.clone()) {
                    self.register_bank.get(name).unwrap().yank.clone()
                } else {
                    self.message = Some("Named register exceeds the 16 MiB content limit".into());
                    yank
                }
            }
            None => {
                if !deleting {
                    self.set_yank_register(Some(yank.clone()));
                }
                yank
            }
        };
        self.set_register(value.clone());
        Some(value)
    }

    pub(super) fn paste_selected_register(
        &mut self,
        text: &mut String,
        before: bool,
        count: usize,
    ) -> (bool, bool, Option<Yank>) {
        let Some(name) = self.selected_register.take() else {
            return if self.mode == VimMode::Normal {
                (true, self.paste(text, before, count), None)
            } else {
                self.paste_visual(text, count, before)
            };
        };
        let Some(value) = self.register_value(name).cloned() else {
            return (true, false, None);
        };
        let unnamed = self.register.replace(value);
        let result = if self.mode == VimMode::Normal {
            (true, self.paste(text, before, count), None)
        } else {
            self.paste_visual(text, count, before)
        };
        if result.2.is_none() {
            self.register = unnamed;
        }
        result
    }

    pub fn start_insert_register(&mut self, text: &mut String) -> Action {
        if self.multi.active() {
            return self.multi_start_insert_register(text);
        }
        if self.mode != VimMode::Insert {
            return Action::default();
        }
        self.pending = Some(Pending::InsertRegister);
        Action {
            consumed: true,
            ..Action::default()
        }
    }

    pub fn insert_register_pending(&self) -> bool {
        matches!(self.pending, Some(Pending::InsertRegister))
    }

    pub fn insert_register(&mut self, text: &mut String, name: char) -> Action {
        if !Self::valid_register(name) {
            return Action {
                consumed: true,
                message: Some("Invalid Vim register".into()),
                ..Action::default()
            };
        }
        let Some(value) = self.register_value(name).map(|yank| yank.text.clone()) else {
            return Action {
                consumed: true,
                ..Action::default()
            };
        };
        if text.len().saturating_add(value.len()) > MAX_PASTE_BYTES {
            return Action {
                consumed: true,
                message: Some("Register insertion exceeds the 16 MiB edit limit".into()),
                ..Action::default()
            };
        }
        self.insert_text(text, &value)
    }
}

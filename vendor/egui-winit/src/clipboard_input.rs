// Keep the originating key immediately before its clipboard command so applications
// can choose to handle the key themselves, including paste with an empty clipboard.
// Returns whether the key is a clipboard command and must not also produce text.
pub(super) fn push_key(
    events: &mut Vec<egui::Event>,
    key: egui::Key,
    physical_key: Option<egui::Key>,
    pressed: bool,
    modifiers: egui::Modifiers,
    get_clipboard: impl FnOnce() -> Option<String>,
) -> bool {
    events.push(egui::Event::Key {
        key,
        physical_key,
        pressed,
        repeat: false, // egui will fill this in for us!
        modifiers,
    });
    if !pressed {
        return false;
    }

    if is_cut_command(modifiers, key) {
        events.push(egui::Event::Cut);
    } else if is_copy_command(modifiers, key) {
        events.push(egui::Event::Copy);
    } else if is_paste_command(modifiers, key) {
        if let Some(contents) = get_clipboard() {
            let contents = contents.replace("\r\n", "\n");
            if !contents.is_empty() {
                events.push(egui::Event::Paste(contents));
            }
        }
    } else {
        return false;
    }
    true
}

fn is_cut_command(modifiers: egui::Modifiers, keycode: egui::Key) -> bool {
    keycode == egui::Key::Cut
        || (modifiers.command && keycode == egui::Key::X)
        || (cfg!(target_os = "windows") && modifiers.shift && keycode == egui::Key::Delete)
}

fn is_copy_command(modifiers: egui::Modifiers, keycode: egui::Key) -> bool {
    keycode == egui::Key::Copy
        || (modifiers.command && keycode == egui::Key::C)
        || (cfg!(target_os = "windows") && modifiers.ctrl && keycode == egui::Key::Insert)
}

fn is_paste_command(modifiers: egui::Modifiers, keycode: egui::Key) -> bool {
    keycode == egui::Key::Paste
        || (modifiers.command && keycode == egui::Key::V)
        || (cfg!(target_os = "windows") && modifiers.shift && keycode == egui::Key::Insert)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, Key, Modifiers};

    fn key_event(key: Key, modifiers: Modifiers, pressed: bool) -> Event {
        Event::Key {
            key,
            physical_key: Some(key),
            pressed,
            repeat: false,
            modifiers,
        }
    }

    #[test]
    fn clipboard_commands_keep_key_provenance_and_event_order() {
        let command = Modifiers::CTRL | Modifiers::COMMAND;
        let mut cases = vec![
            (Key::C, command, Event::Copy),
            (Key::X, command, Event::Cut),
            (Key::V, command, Event::Paste("a\nb".into())),
            (Key::Copy, Modifiers::NONE, Event::Copy),
            (Key::Cut, Modifiers::NONE, Event::Cut),
            (Key::Paste, Modifiers::NONE, Event::Paste("a\nb".into())),
        ];
        if cfg!(target_os = "windows") {
            cases.extend([
                (Key::Insert, Modifiers::CTRL, Event::Copy),
                (Key::Delete, Modifiers::SHIFT, Event::Cut),
                (Key::Insert, Modifiers::SHIFT, Event::Paste("a\nb".into())),
            ]);
        }
        for (key, modifiers, semantic) in cases {
            let mut events = vec![];
            let mut reads = 0;
            assert!(push_key(
                &mut events,
                key,
                Some(key),
                true,
                modifiers,
                || {
                    reads += 1;
                    Some("a\r\nb".into())
                },
            ));
            assert_eq!(reads, usize::from(matches!(semantic, Event::Paste(_))));
            assert_eq!(events, [key_event(key, modifiers, true), semantic]);
        }
    }

    #[test]
    fn empty_or_unavailable_clipboard_still_emits_the_paste_key() {
        let modifiers = Modifiers::CTRL | Modifiers::COMMAND;
        for clipboard in [None, Some(String::new())] {
            let mut events = vec![];
            assert!(push_key(
                &mut events,
                Key::V,
                Some(Key::V),
                true,
                modifiers,
                || clipboard,
            ));
            assert_eq!(events, [key_event(Key::V, modifiers, true)]);
        }
    }

    #[test]
    fn releases_and_ordinary_keys_do_not_access_the_clipboard() {
        for (key, modifiers, pressed) in [
            (Key::V, Modifiers::CTRL | Modifiers::COMMAND, false),
            (Key::C, Modifiers::NONE, true),
        ] {
            let mut events = vec![];
            assert!(!push_key(
                &mut events,
                key,
                Some(Key::Q),
                pressed,
                modifiers,
                || panic!("non-paste input must not read the clipboard"),
            ));
            assert_eq!(
                events,
                [Event::Key {
                    key,
                    physical_key: Some(Key::Q),
                    pressed,
                    repeat: false,
                    modifiers,
                }],
            );
        }
    }
}

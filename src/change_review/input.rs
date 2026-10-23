//! Review command mapping, shortcut hints and ordered reading input.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum ReviewKey {
    Lines(f32),
    Pages(f32),
    Top,
    Bottom,
    NextFile,
    PreviousFile,
    SeenNext,
    Open,
    Close,
    /// Shift+Arrow: extend the selected diff rows by one.
    Extend(isize),
    /// Comment on the selected rows, or on the line on screen.
    Comment,
}

pub(super) fn review_action(action: keybindings::Action) -> Option<ReviewKey> {
    use keybindings::Action;
    match action {
        Action::ReviewNextFile => Some(ReviewKey::NextFile),
        Action::ReviewPreviousFile => Some(ReviewKey::PreviousFile),
        Action::ReviewSeenNext => Some(ReviewKey::SeenNext),
        Action::ReviewOpenFile => Some(ReviewKey::Open),
        Action::ReviewComment => Some(ReviewKey::Comment),
        _ => None,
    }
}

pub(super) fn review_hint(
    bindings: &keybindings::Bindings,
    action: keybindings::Action,
    description: &str,
) -> String {
    let shortcut = keybindings::label(bindings, action);
    if shortcut.is_empty() {
        description.into()
    } else {
        format!("{shortcut} · {description}")
    }
}

/// Commands use the registry; ordinary reading motions stay local to the view.
/// Consume a key and its generated text together, preserving event order.
pub(super) fn take_review_keys(
    ctx: &egui::Context,
    swap_jk: bool,
    bindings: &keybindings::Bindings,
) -> Vec<ReviewKey> {
    let actions = [
        keybindings::Action::ReviewNextFile,
        keybindings::Action::ReviewPreviousFile,
        keybindings::Action::ReviewSeenNext,
        keybindings::Action::ReviewOpenFile,
        keybindings::Action::ReviewComment,
    ];
    let command = |event: &egui::Event| {
        actions
            .into_iter()
            .find(|action| keybindings::matches(bindings, *action, event))
            .and_then(review_action)
    };
    let motion = |key: Key, modifiers: egui::Modifiers| {
        if modifiers == egui::Modifiers::SHIFT {
            return match key {
                Key::ArrowDown => Some(ReviewKey::Extend(1)),
                Key::ArrowUp => Some(ReviewKey::Extend(-1)),
                _ => None,
            };
        }
        if !modifiers.is_none() {
            return None;
        }
        match key {
            Key::J => Some(ReviewKey::Lines(if swap_jk { -1.0 } else { 1.0 })),
            Key::K => Some(ReviewKey::Lines(if swap_jk { 1.0 } else { -1.0 })),
            Key::ArrowDown => Some(ReviewKey::Lines(1.0)),
            Key::ArrowUp => Some(ReviewKey::Lines(-1.0)),
            Key::PageDown => Some(ReviewKey::Pages(1.0)),
            Key::PageUp => Some(ReviewKey::Pages(-1.0)),
            Key::Home => Some(ReviewKey::Top),
            Key::End => Some(ReviewKey::Bottom),
            Key::Enter => Some(ReviewKey::Open),
            Key::Escape => Some(ReviewKey::Close),
            _ => None,
        }
    };
    let mut keys = Vec::new();
    ctx.input_mut(|input| {
        let mut events = std::mem::take(&mut input.events).into_iter().peekable();
        while let Some(event) = events.next() {
            match &event {
                egui::Event::Key {
                    key,
                    modifiers,
                    pressed: true,
                    ..
                } => {
                    if let Some(command) = command(&event).or_else(|| motion(*key, *modifiers)) {
                        keys.push(command);
                        if events.peek().is_some_and(|next| {
                            keybindings::shortcut_text_event(next, &event)
                                || matches!(next, egui::Event::Text(text)
                                    if !modifiers.ctrl && !modifiers.command && !modifiers.alt
                                        && (text.eq_ignore_ascii_case(key.symbol_or_name())
                                            || *key == Key::Space && text == " "))
                        }) {
                            events.next();
                        }
                    } else {
                        input.events.push(event);
                    }
                }
                egui::Event::Text(text) => {
                    let mut consumed = false;
                    for character in text.chars() {
                        let key = if character == ' ' {
                            Some(Key::Space)
                        } else {
                            Key::from_name(&character.to_string())
                        };
                        if let Some(key) = key {
                            let modifiers = if character.is_uppercase() {
                                egui::Modifiers::SHIFT
                            } else {
                                egui::Modifiers::NONE
                            };
                            let event = egui::Event::Key {
                                key,
                                physical_key: None,
                                pressed: true,
                                repeat: false,
                                modifiers,
                            };
                            if let Some(command) =
                                command(&event).or_else(|| motion(key, modifiers))
                            {
                                keys.push(command);
                                consumed = true;
                            }
                        }
                    }
                    if !consumed {
                        input.events.push(event);
                    }
                }
                _ => input.events.push(event),
            }
        }
    });
    keys
}

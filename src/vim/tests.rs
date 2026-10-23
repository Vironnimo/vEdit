use super::*;

mod clipboard;
mod history;
mod motions;
mod navigation;
mod operators;

fn search_motion(query: &str, backwards: bool, count: usize) -> SearchMotion {
    SearchMotion {
        query: query.into(),
        case_sensitive: true,
        whole_word: false,
        regex: true,
        backwards,
        count,
    }
}

fn position(state: &VimState, text: &str) -> (usize, usize) {
    text.chars()
        .take(state.cursor)
        .fold((0, 0), |(row, column), ch| {
            if ch == '\n' {
                (row + 1, 0)
            } else {
                (row, column + 1)
            }
        })
}

fn keys(state: &mut VimState, text: &mut String, value: &str) {
    for ch in value.chars() {
        state.handle_key(text, VimKey::Char(ch));
    }
}

fn run(initial: &str, cursor: usize, value: &str) -> (String, usize, VimState) {
    let mut state = VimState::new();
    let mut text = initial.to_owned();
    state.set_cursor(&text, cursor);
    for ch in value.chars() {
        let key = match ch {
            '\u{1b}' => VimKey::Escape,
            '\n' => VimKey::Enter,
            ch => VimKey::Char(ch),
        };
        state.handle_key(&mut text, key);
    }
    let cursor = state.cursor;
    (text, cursor, state)
}

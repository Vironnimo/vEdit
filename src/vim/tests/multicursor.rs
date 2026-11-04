use super::*;
use crate::multicursor::Selection;

#[test]
fn replace_mode_overwrites_at_every_cursor_and_repeats_as_one_edit() {
    let mut text = "abc def\nuvw xyz".to_owned();
    let mut state = cursors(&[0, 4]);
    keys(&mut state, &mut text, "R");
    assert!(state.is_replacing());
    state.insert_text(&mut text, "XY");
    state.handle_key(&mut text, VimKey::Backspace);
    assert_eq!(text, "Xbc Xef\nuvw xyz");
    state.finish_insert(&mut text);
    assert!(!state.is_replacing());
    keys(&mut state, &mut text, "u");
    assert_eq!(text, "abc def\nuvw xyz");
    state.handle_key(&mut text, VimKey::Redo);
    assert_eq!(text, "Xbc Xef\nuvw xyz");
    keys(&mut state, &mut text, "j0.");
    assert_eq!(text, "Xbc Xef\nXvw xyz");
    let mut text = "{}\n{}".to_owned();
    let mut state = cursors(&[1, 4]);
    keys(&mut state, &mut text, "R");
    state.handle_key(&mut text, VimKey::Enter);
    assert_eq!(text, "{\n    \n}\n{\n    \n}");
    state.handle_key(&mut text, VimKey::Backspace);
    assert_eq!(
        text, "{}\n{}",
        "generated newline layout restores at every cursor"
    );
}

#[test]
fn document_marks_follow_multicursor_edits_like_the_single_cursor() {
    let original = "a\nb\nc\nd\ne";
    let mut single_text = original.to_owned();
    let mut single = VimState::new();
    single.set_cursor(&single_text, 6);
    keys(&mut single, &mut single_text, "ma");
    single.set_cursor(&single_text, 0);
    keys(&mut single, &mut single_text, "ddjdd'a");
    assert_eq!(single_text, "b\nd\ne");
    assert_eq!(position(&single, &single_text), (1, 0));

    let mut text = original.to_owned();
    let mut vim = VimState::new();
    vim.set_cursor(&text, 6);
    keys(&mut vim, &mut text, "ma");
    vim.multi.selections = vec![Selection::new(0, 0), Selection::new(4, 4)];
    vim.cursor = 4;
    keys(&mut vim, &mut text, "dd");
    assert_eq!(text, "b\nd\ne");
    vim.set_cursor(&text, 0);
    keys(&mut vim, &mut text, "'a");
    assert_eq!(
        position(&vim, &text),
        (1, 0),
        "a document mark follows simultaneous multicursor edits"
    );
}

fn cursors(positions: &[usize]) -> VimState {
    let mut vim = VimState::new();
    vim.multi.selections = positions.iter().map(|&at| Selection::new(at, at)).collect();
    vim.cursor = *positions.last().unwrap();
    vim
}

fn keys(vim: &mut VimState, text: &mut String, keys: &str) {
    for ch in keys.chars() {
        vim.handle_key(text, VimKey::Char(ch));
    }
}

#[test]
fn each_cursor_uses_vim_motion_counts_and_character_targets() {
    for command in ["ma", "`a", "'a", "g`a", "g'a", "gi", "g;", "g,"] {
        let mut text = "abc def".to_owned();
        let mut multi = cursors(&[0, 4]);
        let mut message = false;
        for ch in command.chars() {
            message |= multi
                .handle_key(&mut text, VimKey::Char(ch))
                .message
                .is_some();
        }
        assert!(message, "{command}");
        assert_eq!(multi.pending_command(), "", "{command}");
        assert_eq!(text, "abc def", "{command}");
        keys(&mut multi, &mut text, "x");
        assert_eq!(text, "bc ef", "{command} must consume only its own keys");
    }
    for ending in ["\n", "\r\n"] {
        let original = ["ä first second last", "🦀 different words here"].join(ending);
        let start = 20 + ending.chars().count() - 1;
        for command in ["w", "W", "2w", "$b", "e", "fse", "0j$", "veb"] {
            let mut text = original.clone();
            let mut multi = cursors(&[0, start]);
            keys(&mut multi, &mut text, command);
            assert_eq!(text, original);
            for at in [0, start] {
                let mut single = VimState::new();
                single.cursor = at;
                let mut local = original.clone();
                keys(&mut single, &mut local, command);
                assert!(
                    multi
                        .multi
                        .selections
                        .iter()
                        .any(|selection| multi.multicursor_head(*selection) == single.cursor),
                    "{command}, {ending:?}, {at}"
                );
            }
        }
    }
}

#[test]
fn operators_are_simultaneous_and_shared_history_restores_all_cursors() {
    for ending in ["\n", "\r\n"] {
        let original = ["first next last", "other next last"].join(ending);
        let second = 15 + ending.len();
        let mut text = original.clone();
        let mut vim = cursors(&[0, second]);
        keys(&mut vim, &mut text, "dw");
        assert_eq!(text, ["next last", "next last"].join(ending));
        keys(&mut vim, &mut text, ".");
        assert_eq!(text, ["last", "last"].join(ending));
        keys(&mut vim, &mut text, "2u");
        assert_eq!(text, original);
        assert_eq!(
            vim.multi.selections,
            vec![Selection::new(0, 0), Selection::new(second, second)]
        );
        vim.handle_key(&mut text, VimKey::Redo);
        assert_eq!(text, ["next last", "next last"].join(ending));
    }
    let mut text = "one two three\nlast".to_owned();
    let mut vim = cursors(&[0, 4]);
    keys(&mut vim, &mut text, "dd");
    assert_eq!(text, "last");
    keys(&mut vim, &mut text, "u");
    assert_eq!(text, "one two three\nlast");
    assert_eq!(vim.multi.selections.len(), 2);
    let mut vim = cursors(&[0, 1]);
    keys(&mut vim, &mut text, "dw");
    assert_eq!(text, "two three\nlast");
    assert_eq!(vim.multi.selections, vec![Selection::new(0, 0)]);
}

#[test]
fn visual_operators_and_pending_commands_survive_navigation_transfer() {
    let mut text = "one two\nred blue".to_owned();
    let mut named = cursors(&[0, 8]);
    keys(&mut named, &mut text, "\"ayiw");
    assert_eq!(named.register_value('a').unwrap().text, "red");
    keys(&mut named, &mut text, "w\"Ayiw");
    assert_eq!(named.register_value('a').unwrap().text, "redblue");
    keys(&mut named, &mut text, "i");
    named.start_insert_register(&mut text);
    named.handle_key(&mut text, VimKey::Char('a'));
    assert_eq!(text, "one redbluetwo\nred redblueblue");
    named.handle_key(&mut text, VimKey::Escape);
    keys(&mut named, &mut text, "u");
    assert_eq!(text, "one two\nred blue");
    let mut text = "one two\nred blue".to_owned();
    let mut vim = cursors(&[0, 8]);
    keys(&mut vim, &mut text, "d");
    let navigation = vim.navigation();
    vim.set_cursor(&text, 0);
    vim.restore_navigation(&text, &navigation);
    keys(&mut vim, &mut text, "iw");
    assert_eq!(text, " two\n blue");
    keys(&mut vim, &mut text, "u");
    keys(&mut vim, &mut text, "ve");
    assert_eq!(vim.mode, VimMode::Visual);
    assert_eq!(
        vim.multi
            .selections
            .iter()
            .map(|s| s.range())
            .collect::<Vec<_>>(),
        vec![0..3, 8..11]
    );
    keys(&mut vim, &mut text, "d");
    assert_eq!(text, " two\n blue");
    keys(&mut vim, &mut text, "u");
    keys(&mut vim, &mut text, "yiw");
    assert_eq!(vim.yank_register().unwrap().text, "red");
    keys(&mut vim, &mut text, "wdiw");
    assert_eq!(vim.yank_register().unwrap().text, "red");
}

#[test]
#[ignore = "manual benchmark: multicursor motion scaling"]
fn motion_scaling() {
    let line = "one two three four\n";
    for count in [16, 128, 1000] {
        let mut text = line.repeat(2000);
        let positions: Vec<_> = (0..count).map(|index| index * line.len()).collect();
        let mut vim = cursors(&positions);
        let started = std::time::Instant::now();
        vim.handle_key(&mut text, VimKey::Char('w'));
        eprintln!(
            "{count} cursors, {} bytes: {:?}",
            text.len(),
            started.elapsed()
        );
        assert_eq!(
            vim.multi
                .selections
                .iter()
                .map(|selection| selection.head)
                .collect::<Vec<_>>(),
            positions
                .iter()
                .map(|position| position + 4)
                .collect::<Vec<_>>()
        );
    }
}

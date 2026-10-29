use super::*;
use crate::multicursor::Selection;

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

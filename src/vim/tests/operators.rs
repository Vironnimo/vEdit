use super::*;

#[test]
fn external_search_motion_preserves_operator_count_and_exact_newline_target() {
    let mut state = VimState::new();
    let mut text = "one\ntwo\n".to_owned();
    keys(&mut state, &mut text, "2d3");
    assert_eq!(state.take_search_count(), 6);
    assert!(state.search_operator_pending());
    let action = state.apply_search_motion(&mut text, 3, search_motion(r"\n", false, 6));
    assert!(action.text_changed);
    assert_eq!(text, "\ntwo\n");
    assert_eq!(action.yanked.unwrap().text, "one");
    keys(&mut state, &mut text, "u");
    assert_eq!(text, "one\ntwo\n");
}

#[test]
fn operator_inclusivity_and_empty_lines() {
    for (initial, command, expected) in [
        ("abc", "dw", ""),
        ("abc", "d$", ""),
        ("abc", "$dl", "ab"),
        ("\nabc", "x", "\nabc"),
    ] {
        let mut state = VimState::new();
        let mut text = initial.to_owned();
        keys(&mut state, &mut text, command);
        assert_eq!(text, expected, "{command}");
    }
}

#[test]
fn opening_and_shifting_lines_honor_tab_indentation() {
    let mut state = VimState::new();
    state.set_indentation(4, true);
    let mut text = "if ready:\n\trun()\n".to_owned();
    keys(&mut state, &mut text, "o");
    assert_eq!(text, "if ready:\n\t\n\trun()\n");
    state.finish_insert(&mut text);
    keys(&mut state, &mut text, "u>>");
    assert_eq!(text, "\tif ready:\n\trun()\n");
    keys(&mut state, &mut text, "<<");
    assert_eq!(text, "if ready:\n\trun()\n");
}

#[test]
fn empty_lines_do_not_allocate_counted_indentation() {
    for tabs in [false, true] {
        let mut state = VimState::new();
        state.set_indentation(4, tabs);
        let mut text = "\r\n\r\n".to_owned();
        assert!(!state.shift_lines(&mut text, 0, 2, true, usize::MAX));
        assert_eq!(text, "\r\n\r\n");
        assert!(!state.can_undo());
    }
}

#[test]
fn surround_add_change_delete_preserve_registers_and_repeat_as_one_edit() {
    for (initial, cursor, command, expected) in [
        ("one two", 0, "ysiw)", "(one) two"),
        ("one two", 0, "ysw]", "[one] two"),
        ("one two", 0, "ysiw(", "( one ) two"),
        ("  α🦀\r\nnext", 2, "yss\"", "  \"α🦀\"\r\nnext"),
        ("one\nx tail", 0, "ysfx]", "[one\nx] tail"),
        ("foo \"bar\" baz", 6, "cs\"'", "foo 'bar' baz"),
        ("foo \"bar\" baz", 6, "ds\"", "foo bar baz"),
        ("( α🦀 ) tail", 3, "cs(]", "[α🦀] tail"),
        ("( α🦀 ) tail", 3, "ds)", " α🦀  tail"),
        ("( α🦀 ) tail", 3, "ds(", "α🦀 tail"),
        ("a((b))c", 3, "2ds)", "a(b)c"),
    ] {
        let mut state = VimState::new();
        let mut text = initial.to_owned();
        state.cursor = cursor;
        let register = Yank {
            text: "KEEP".into(),
            linewise: false,
            block_width: None,
        };
        state.set_register(register.clone());
        keys(&mut state, &mut text, command);
        assert_eq!(text, expected, "{command}");
        assert_eq!(state.register(), Some(&register));
        assert_eq!(state.undo.len(), 1, "{command}");
        assert_eq!(state.mode, VimMode::Normal);
        keys(&mut state, &mut text, "u");
        assert_eq!(text, initial, "{command}");
    }
    let mut state = VimState::new();
    let mut text = "one two three".to_owned();
    keys(&mut state, &mut text, "ysiw)www.");
    assert_eq!(text, "(one) (two) three");
    keys(&mut state, &mut text, "u");
    assert_eq!(text, "(one) two three");
    keys(&mut state, &mut text, "gglds)");
    assert_eq!(text, "one two three");
}

#[test]
fn dollar_keeps_end_of_line_intent_across_vertical_motion_and_empty_lines() {
    let mut state = VimState::new();
    let mut text = "a\nlong\n\nlast".to_owned();
    keys(&mut state, &mut text, "$j");
    assert_eq!(state.cursor, 5);
    keys(&mut state, &mut text, "jj");
    assert_eq!(state.cursor, 11);
    keys(&mut state, &mut text, "kk");
    assert_eq!(state.cursor, 5);
    keys(&mut state, &mut text, "0j");
    assert_eq!(state.cursor, 7);

    state = VimState::new();
    text = "a\nlong".into();
    keys(&mut state, &mut text, "v$jd");
    assert_eq!(text, "");
    keys(&mut state, &mut text, "u");
    assert_eq!(text, "a\nlong");
}

#[test]
fn line_change_keeps_code_indent_and_large_visual_indent_is_bounded() {
    let mut state = VimState::new();
    let mut text = "    old\n    next".to_owned();
    keys(&mut state, &mut text, "cc");
    assert_eq!(text, "    \n    next");
    assert_eq!(state.cursor, 4);
    state.insert_text(&mut text, "new");
    state.finish_insert(&mut text);
    keys(&mut state, &mut text, "u");
    assert_eq!(text, "    old\n    next");
    text = "a\n".repeat(1_000);
    state = VimState::new();
    let initial = text.clone();
    keys(&mut state, &mut text, "VG9999>");
    assert_eq!(text, initial);
}

#[test]
fn block_objects_search_forward_and_respect_available_nesting() {
    for (initial, cursor, command, expected) in [
        ("call(value)", 0, "di(", "call()"),
        ("pre\n{value}", 0, "di{", "pre\n{}"),
        ("pre (a (b) c) post", 0, "2di(", "pre (a () c) post"),
        ("pre (a (b) c) post", 8, "2di(", "pre () post"),
        ("pre (a (b) c) post", 8, "3di(", "pre (a (b) c) post"),
        ("pre (a) gap (b)", 0, "2di(", "pre (a) gap (b)"),
        ("pre (a (b) (c) d)", 0, "3di(", "pre (a (b) (c) d)"),
        ("(a) gap (b)", 4, "di(", "(a) gap ()"),
        ("(a) tail", 6, "di(", "(a) tail"),
    ] {
        let mut state = VimState::new();
        let mut text = initial.to_owned();
        state.set_cursor(&text, cursor);
        keys(&mut state, &mut text, command);
        assert_eq!(text, expected, "{initial:?}, {cursor}, {command}");
    }
}

#[test]
fn normal_toggle_case_advances_by_transformed_unicode_characters() {
    let mut state = VimState::new();
    let mut text = "ßaB!\ntail".to_owned();
    keys(&mut state, &mut text, "~");
    assert_eq!(text, "SSaB!\ntail");
    assert_eq!(state.cursor, 2);
    keys(&mut state, &mut text, "2.");
    assert_eq!(text, "SSAb!\ntail");
    assert_eq!(state.cursor, 4);
    keys(&mut state, &mut text, "9~");
    assert_eq!(text, "SSAb!\ntail");
    assert_eq!(state.cursor, 4);
    assert_eq!(state.undo.len(), 2);
}

#[test]
fn joining_crlf_lines_preserves_final_break_and_raw_join_preserves_indentation() {
    for (initial, command, expected) in [
        ("one\r\n  two\r\nthree\r\n", "J", "one two\r\nthree\r\n"),
        ("one\r\n  two\r\nthree\r\n", "3J", "one two three\r\n"),
        ("one\r\n  two\r\nthree", "3gJ", "one  twothree"),
        ("one\n  two\nthree\n", "gJ", "one  two\nthree\n"),
        ("one\r\n\r\ntwo\r\n", "3J", "one two\r\n"),
        ("one\ntwo\r", "J", "one two\r"),
    ] {
        let mut state = VimState::new();
        let mut text = initial.to_owned();
        keys(&mut state, &mut text, command);
        assert_eq!(text, expected, "{initial:?}, {command}");
        assert_eq!(state.cursor, 3, "{command}");
        keys(&mut state, &mut text, "u");
        assert_eq!(text, initial, "{command}");
        state.handle_key(&mut text, VimKey::Redo);
        assert_eq!(text, expected, "{command}");
    }
}

#[test]
fn final_line_break_terminates_the_last_line_for_line_commands() {
    for (initial, cursor, command, expected) in [
        ("a\nb\n", 2, "J", "a\nb\n"),
        ("a\nb\n", 0, "3J", "a b\n"),
        ("a\nb\n", 2, "dj", "a\n"),
        ("a\nb\n", 2, "2dd", "a\n"),
        ("a\nb\nc\n", 4, "dG", "a\nb\n"),
        ("a\nb\n", 0, "yGP", "a\nb\na\nb\n"),
        ("a\nb\n", 0, "dG", ""),
        ("a\nb\n", 0, "ggVGd", ""),
        ("a\r\nb\r\n", 3, "J", "a\r\nb\r\n"),
        ("a\r\nb\r\n", 0, "3J", "a b\r\n"),
        ("a\r\nb\r\n", 3, "2dd", "a\r\n"),
        ("a\nb", 2, "dd", "a"),
        ("a\nb\n", 0, "5yyjp", "a\nb\na\nb\n"),
    ] {
        let (text, _, _) = run(initial, cursor, command);
        assert_eq!(text, expected, "{initial:?}, {cursor}, {command}");
    }
    let (_, cursor, _) = run("a\n  b\n", 0, "G");
    assert_eq!(cursor, 4);
}

#[test]
fn small_vim_divergences_follow_vim() {
    for (initial, cursor, command, expected) in [
        ("f(\n)", 0, "J", "f()"),
        ("f(a,\n  b)", 0, "J", "f(a, b)"),
        ("a\n", 0, "3ox\u{1b}", "a\nx\nx\nx\n"),
        ("a\n", 0, "2Ox\u{1b}", "x\nx\na\n"),
        ("  a\n", 0, "2ox\u{1b}", "  a\n  x\n  x\n"),
        ("a\r\n", 0, "2ox\u{1b}", "a\r\nx\r\nx\r\n"),
        ("Abc dEF\nx", 0, "gugu", "abc def\nx"),
        ("Abc dEF\nx", 0, "gUgU", "ABC DEF\nx"),
        ("Abc dEF\nx", 0, "g~g~", "aBC Def\nx"),
        ("Ab\ncD\nx", 0, "2gUgU", "AB\nCD\nx"),
    ] {
        let (text, _, _) = run(initial, cursor, command);
        assert_eq!(text, expected, "{initial:?}, {cursor}, {command}");
    }
    for (initial, cursor, command, expected) in [
        ("foo bar", 4, "yb", 0),
        ("foo bar", 4, "y0", 0),
        ("foo bar", 4, "2yh", 2),
        ("a\n\nb c", 5, "y{", 2),
        ("foo bar", 0, "yw", 0),
    ] {
        let (text, cursor, _) = run(initial, cursor, command);
        assert_eq!(text, initial);
        assert_eq!(cursor, expected, "{initial:?}, {command}");
    }
    let (text, _, _) = run("a\n", 0, "3ox\u{1b}u");
    assert_eq!(text, "a\n");
    let (text, _, _) = run("a\nb\n", 0, "2ox\u{1b}j.");
    assert_eq!(text, "a\nx\nx\nb\nx\nx\n");
}

#[test]
fn substitute_line_keeps_indentation_and_repeats() {
    let (text, cursor, state) = run("    old\nnext\n", 6, "Snew\u{1b}");
    assert_eq!(text, "    new\nnext\n");
    assert_eq!(cursor, 6);
    assert_eq!(state.mode, VimMode::Normal);
    let (text, _, _) = run("a\nb\nc\n", 0, "2Sx\u{1b}j.");
    assert_eq!(text, "x\nx\n");
    let (text, _, _) = run("a\nb\n", 0, "Sx\u{1b}u");
    assert_eq!(text, "a\nb\n");
}

#[test]
fn increment_and_decrement_numbers_at_or_after_the_cursor() {
    use VimKey::{Char, Decrement, Increment};
    for (initial, cursor, keys, expected, expected_cursor) in [
        ("x 9 y", 0, vec![Increment], "x 10 y", 3),
        ("x 9 y", 0, vec![Char('5'), Increment], "x 14 y", 3),
        ("x 1 y", 2, vec![Decrement, Decrement], "x -1 y", 3),
        ("a -5", 0, vec![Char('7'), Increment], "a 2", 2),
        ("a-5", 0, vec![Increment], "a-4", 2),
        ("v123", 3, vec![Increment], "v124", 3),
        ("007", 0, vec![Increment], "008", 2),
        ("no digits\n42", 0, vec![Increment], "no digits\n42", 0),
        ("x 1 y", 0, vec![Increment, Char('.')], "x 3 y", 2),
        (
            "x 1 y",
            0,
            vec![Char('3'), Decrement, Char('u')],
            "x 1 y",
            0,
        ),
        ("é 1\r\n", 0, vec![Increment], "é 2\r\n", 2),
    ] {
        let mut state = VimState::new();
        let mut text = initial.to_owned();
        state.set_cursor(&text, cursor);
        for key in keys {
            state.handle_key(&mut text, key);
        }
        assert_eq!(text, expected, "{initial:?}");
        assert_eq!(state.cursor, expected_cursor, "{initial:?}");
    }
}

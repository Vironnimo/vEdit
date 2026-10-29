use super::*;

#[test]
fn word_motions_stop_on_the_last_real_line() {
    for (initial, command, expected) in [
        ("foo bar\n", "9w", 6),
        ("foo bar\r\n", "9W", 6),
        ("foo bar", "9w", 6),
        ("foo\n\n", "9w", 4),
        ("foo\r\n\r\n", "9w", 5),
        ("\n", "w", 0),
        ("", "w", 0),
    ] {
        let mut state = VimState::new();
        let mut text = initial.to_owned();
        keys(&mut state, &mut text, command);
        assert_eq!(state.cursor, expected, "{command} in {initial:?}");
        keys(&mut state, &mut text, "dd");
        assert_eq!(
            text,
            if initial.starts_with("foo\n\n") {
                "foo\n"
            } else if initial.starts_with("foo\r\n\r\n") {
                "foo\r\n"
            } else {
                ""
            }
        );
    }
}

#[test]
fn cursor_clamping_handles_unicode_crlf_empty_lines_and_stale_positions() {
    for (text, expected) in [
        ("", vec![0]),
        ("α🦀", vec![0, 1, 1]),
        ("α\r\n\r\nβ\n", vec![0, 0, 0, 3, 3, 5, 5, 7]),
        ("\r\n", vec![0, 0, 2]),
        ("α\rβ\r", vec![0, 1, 2, 3, 3]),
    ] {
        for (cursor, &clamped) in expected.iter().enumerate() {
            assert_eq!(normal_cursor(text, cursor), clamped, "{text:?}, {cursor}");
        }
        assert_eq!(normal_cursor(text, usize::MAX), *expected.last().unwrap());
    }
}

#[test]
fn exact_external_selection_keeps_unicode_and_final_newline() {
    let mut state = VimState::new();
    let mut text = "ä🙂\n中".to_owned();
    state.set_selection(&text, 1..3);
    assert_eq!(state.selection(&text), Some(1..3));
    let action = state.handle_key(&mut text, VimKey::Char('y'));
    assert_eq!(action.yanked.unwrap().text, "🙂\n");
    assert_eq!(text, "ä🙂\n中");
    assert_eq!(state.mode, VimMode::Normal);

    text = "ä🙂\n".to_owned();
    state.set_selection_boundaries(&text, 3, 1);
    assert_eq!(state.cursor, 1);
    assert_eq!(state.selection(&text), Some(1..3));
    state.handle_key(&mut text, VimKey::Char('o'));
    assert_eq!(state.cursor, 2);
    assert_eq!(state.selection(&text), Some(1..3));
    state.handle_key(&mut text, VimKey::Char('d'));
    assert_eq!(text, "ä");
    assert_eq!(state.register().unwrap().text, "🙂\n");
    state.handle_key(&mut text, VimKey::Char('u'));
    assert_eq!(text, "ä🙂\n");
}

#[test]
fn external_boundaries_and_mouse_clicks_have_distinct_eof_rules() {
    let mut state = VimState::new();
    let text = "ä🙂\n中";
    state.set_selection(text, usize::MAX..usize::MAX);
    assert_eq!(state.mode, VimMode::Normal);
    assert_eq!(state.cursor, 4);
    assert_eq!(state.selection(text), None);
    state.set_mouse_selection(text, 2, 2, false);
    assert_eq!(state.cursor, 1);
    assert_eq!(state.mode, VimMode::Normal);
    state.set_mouse_selection(text, 999, 999, false);
    assert_eq!(state.cursor, 3);
    state.set_selection("", 0..999);
    assert_eq!(state.mode, VimMode::Normal);
    assert_eq!(state.cursor, 0);
    assert!(!state.can_undo());
}

#[test]
fn mouse_line_selection_does_not_add_the_end_exclusive_line() {
    for (primary, anchor) in [(2, 0), (0, 2)] {
        let mut state = VimState::new();
        let mut text = "a\nb\n".to_owned();
        state.set_mouse_selection(&text, primary, anchor, true);
        assert_eq!(state.mode, VimMode::VisualLine);
        assert_eq!(state.selection(&text), Some(0..2));
        let action = state.handle_key(&mut text, VimKey::Char('y'));
        let yank = action.yanked.unwrap();
        assert!(yank.linewise);
        assert_eq!(yank.text, "a\n");
    }
}

#[test]
fn shifted_navigation_preserves_reverse_visual_lines_and_modal_state() {
    let mut state = VimState::new();
    let mut text = "ä🙂\n中xy\nthird\nlast".to_owned();
    state.set_selection_boundaries(&text, 6, 3);
    keys(&mut state, &mut text, "V");
    let navigation = state.navigation();
    let selection = state.selection(&text).unwrap();
    let edit = crate::editor::move_lines(&text, selection.clone(), true);
    let delta = edit.selection.start as isize - selection.start as isize;
    state.begin_external_edit(&text);
    text = edit.text;
    state.restore_navigation(&text, &navigation.shifted(delta));
    assert_eq!(text, "ä🙂\nthird\n中xy\nlast");
    assert_eq!(state.mode, VimMode::VisualLine);
    assert_eq!(state.cursor, 9);
    assert_eq!(state.visual_anchor, Some(11));
    assert_eq!(state.selection(&text), Some(9..13));
    assert_eq!(state.undo.len(), 1);

    let mut pending = VimState::new();
    keys(&mut pending, &mut text, "2d3");
    let shifted = pending.navigation().shifted(5);
    assert_eq!(shifted.cursor, 5);
    assert!(matches!(
        shifted.pending,
        Some(Pending::Operator(Operator::Delete, 2, true))
    ));
    assert_eq!(shifted.count, 3);
    assert_eq!(shifted.command_keys, pending.command_keys);
    let saturated = navigation.shifted(isize::MIN);
    assert_eq!(saturated.cursor, 0);
    assert_eq!(saturated.visual_anchor, Some(0));
}

#[test]
fn pane_navigation_restores_pending_commands_and_preferred_columns() {
    let mut state = VimState::new();
    let mut text = "one two three".to_owned();
    keys(&mut state, &mut text, "2d");
    let operator_view = state.navigation();
    state.restore_navigation(&text, &VimNavigation::default());
    keys(&mut state, &mut text, "l");
    assert_eq!(text, "one two three");
    state.restore_navigation(&text, &operator_view);
    assert_eq!(state.pending_command(), "2d");
    keys(&mut state, &mut text, "w");
    assert_eq!(text, "three");
    keys(&mut state, &mut text, "u");
    assert_eq!(text, "one two three");

    text = "abcdef\nx\nabcdef".to_owned();
    state = VimState::new();
    keys(&mut state, &mut text, "4lj");
    let short_line_view = state.navigation();
    state.restore_navigation(&text, &VimNavigation::default());
    keys(&mut state, &mut text, "l");
    state.restore_navigation(&text, &short_line_view);
    keys(&mut state, &mut text, "j");
    assert_eq!(position(&state, &text), (2, 4));
}

#[test]
fn pane_navigation_clamps_after_another_view_shortens_the_document() {
    let mut state = VimState::new();
    state.set_selection_boundaries("ä🙂\n中", 4, 1);
    let view = state.navigation();
    state.restore_navigation("é", &view);
    assert_eq!(state.cursor, 1);
    assert_eq!(state.selection("é"), Some(1..1));
    state.restore_navigation("", &view);
    assert_eq!(state.cursor, 0);
    assert_eq!(state.selection(""), Some(0..0));
}

#[test]
fn counts_multiply_and_line_boundaries_work() {
    let mut state = VimState::new();
    let mut text = "one two three four five six seven".to_owned();
    keys(&mut state, &mut text, "2d3w");
    assert_eq!(text, "seven");
    for (command, expected) in [
        ("2dd", "c\nd"),
        ("d2d", "c\nd"),
        ("2d2d", ""),
        ("Gdd", "a\nb\nc"),
        ("Gdk", "a\nb"),
    ] {
        let mut state = VimState::new();
        let mut text = "a\nb\nc\nd".to_owned();
        keys(&mut state, &mut text, command);
        assert_eq!(text, expected, "{command}");
    }
}

#[test]
fn visual_line_selection_down_and_up() {
    for command in ["Vjd", "jVkd"] {
        let mut state = VimState::new();
        let mut text = "one\ntwo\nthree".to_owned();
        keys(&mut state, &mut text, command);
        assert_eq!(text, "three", "{command}");
        assert!(state.register().unwrap().linewise);
        assert_eq!(state.register().unwrap().text, "one\ntwo\n");
    }
    let mut state = VimState::new();
    let mut text = "one\ntwo\nthree".to_owned();
    keys(&mut state, &mut text, "Vjc");
    assert_eq!(text, "\nthree");
    assert_eq!(state.mode, VimMode::Insert);
    state.insert_text(&mut text, "replacement");
    state.finish_insert(&mut text);
    keys(&mut state, &mut text, "u");
    assert_eq!(text, "one\ntwo\nthree");
}

#[test]
fn visual_surround_repeats_native_selection_geometry_and_can_cancel() {
    let mut state = VimState::new();
    let mut text = "one two three".to_owned();
    state.set_selection(&text, 0..3);
    keys(&mut state, &mut text, "S]");
    assert_eq!(text, "[one] two three");
    keys(&mut state, &mut text, "www.");
    assert_eq!(text, "[one] [two] three");
    keys(&mut state, &mut text, "uu");
    assert_eq!(text, "one two three");
    keys(&mut state, &mut text, "ysiw");
    assert!(state.expects_literal_character());
    state.handle_key(&mut text, VimKey::Escape);
    keys(&mut state, &mut text, "w");
    assert_eq!(text, "one two three");
    assert_eq!(state.pending_command(), "");
    keys(&mut state, &mut text, "dsw");
    assert_eq!(text, "one two three");
}

#[test]
fn huge_numeric_counts_saturate_and_motions_stop_at_boundaries() {
    let mut state = VimState::new();
    let mut text = "ä🙂\none two\nlast".to_owned();
    keys(&mut state, &mut text, "999999999999999999999999j");
    assert_eq!(position(&state, &text), (2, 0));
    keys(&mut state, &mut text, "gg9999w");
    assert_eq!(state.cursor, text.chars().count() - 1);
    keys(&mut state, &mut text, "9999b");
    assert_eq!(state.cursor, 0);
    keys(&mut state, &mut text, "9999dd");
    assert!(text.is_empty());
    assert_eq!(state.undo.len(), 1);
}

#[test]
fn reselect_last_visual_area() {
    let (_, _, state) = run("one two three", 4, "viw\u{1b}0gv");
    assert_eq!(state.mode, VimMode::Visual);
    assert_eq!(state.selection("one two three"), Some(4..7));
    let (text, _, _) = run("a\nb\nc\n", 0, "Vjy\u{1b}Ggvd");
    assert_eq!(text, "c\n");
    let (_, _, state) = run("abc", 0, "gv");
    assert_eq!(state.mode, VimMode::Normal);
    let (_, _, state) = run("abcdef", 0, "vl\u{1b}3lvlgv");
    assert_eq!(state.selection("abcdef"), Some(0..2));
    let (_, _, state) = run("abcdef", 0, "vl\u{1b}3lvlgvgv");
    assert_eq!(state.selection("abcdef"), Some(4..6));
}

#[test]
fn scroll_commands_request_alignment_and_keep_the_text() {
    let mut state = VimState::new();
    let mut text = "a\n  b\nc\n".to_owned();
    state.set_cursor(&text, 3);
    for (command, align, cursor) in [
        ("zz", ScrollAlign::Center, 3),
        ("zt", ScrollAlign::Top, 3),
        ("zb", ScrollAlign::Bottom, 3),
        ("z\n", ScrollAlign::Top, 4),
        ("1z.", ScrollAlign::Center, 0),
        ("3z-", ScrollAlign::Bottom, 6),
    ] {
        let mut last = Action::default();
        for ch in command.chars() {
            last = state.handle_key(
                &mut text,
                if ch == '\n' {
                    VimKey::Enter
                } else {
                    VimKey::Char(ch)
                },
            );
        }
        assert_eq!(last.scroll, Some(align), "{command}");
        assert_eq!(state.cursor, cursor, "{command}");
        assert_eq!(text, "a\n  b\nc\n");
    }
}

#[test]
fn screen_line_motions_use_the_visible_range() {
    let text = "l0\nl1\n  l2\nl3\nl4\nl5\nl6\n";
    // Lines l1 to l4 are fully visible; like the editor's visual rows,
    // the range ends before the last row's newline.
    let visible = 3..16;
    for (command, expected) in [
        ("H", 3),
        ("2H", 8),
        ("L", 14),
        ("2L", 11),
        ("M", 8),
        ("9H", 14),
        ("9L", 3),
    ] {
        let mut state = VimState::new();
        let mut source = text.to_owned();
        state.set_visible_range(Some(visible.clone()));
        keys(&mut state, &mut source, command);
        assert_eq!(state.cursor, expected, "{command}");
    }
    let mut state = VimState::new();
    let mut source = text.to_owned();
    state.set_visible_range(Some(visible));
    state.set_cursor(&source, 3);
    keys(&mut state, &mut source, "dL");
    assert_eq!(source, "l0\nl5\nl6\n");
    // Without a known view, the whole document counts as visible.
    let mut state = VimState::new();
    let mut source = text.to_owned();
    keys(&mut state, &mut source, "L");
    assert_eq!(state.cursor, 20);
    // An empty last visible row is still the bottom screen line.
    let mut state = VimState::new();
    let mut source = "a\n\nb\n".to_owned();
    state.set_visible_range(Some(0..2));
    keys(&mut state, &mut source, "L");
    assert_eq!(state.cursor, 2);
}

#[test]
fn unsupported_commands_consume_their_argument_and_report_it() {
    for (command, message) in [
        ("qa", "q"),
        ("@a", "@"),
        ("ma", "m"),
        ("`a", "`"),
        ("'a", "'"),
        ("R", "R"),
        ("ZZ", "Z"),
        ("zf", "zf"),
        ("gq", "gq"),
        ("dq", "dq"),
        ("d'a", "d'"),
    ] {
        let mut state = VimState::new();
        let mut text = "abc def".to_owned();
        let mut messages = Vec::new();
        for ch in command.chars() {
            let action = state.handle_key(&mut text, VimKey::Char(ch));
            assert!(action.consumed, "{command}");
            messages.extend(action.message);
        }
        assert_eq!(
            messages,
            vec![format!("Not supported in vEdit: {message}")],
            "{command}"
        );
        assert_eq!(text, "abc def");
        assert_eq!(state.pending_command(), "");
        keys(&mut state, &mut text, "x");
        assert_eq!(
            text, "bc def",
            "{command} must not swallow the next command"
        );
    }
    let mut state = VimState::new();
    state.handle_key(&mut "abc".to_owned(), VimKey::Char('m'));
    assert!(state.expects_literal_character());
}

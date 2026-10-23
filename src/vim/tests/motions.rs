use super::*;

#[test]
fn nearby_line_motions_preserve_crlf_unicode_and_empty_rows() {
    let mut text = "α🦀\r\n\r\n  βγ\r\nlast\r\n".to_owned();
    let mut state = VimState::new();
    keys(&mut state, &mut text, "lj");
    assert_eq!(state.cursor, 4);
    keys(&mut state, &mut text, "j");
    assert_eq!(state.cursor, 7);
    keys(&mut state, &mut text, "2k");
    assert_eq!(state.cursor, 1);
    keys(&mut state, &mut text, "2+");
    assert_eq!(state.cursor, 8);
    keys(&mut state, &mut text, "2$");
    assert_eq!(state.cursor, 15);
    // The final CRLF terminates "last"; G does not enter the empty row after it.
    keys(&mut state, &mut text, "G");
    assert_eq!(state.cursor, 12);
    keys(&mut state, &mut text, "2G");
    assert_eq!(state.cursor, 4);
}

#[test]
fn exclusive_search_at_column_one_adjusts_to_linewise_or_previous_line_end() {
    for (cursor, expected, linewise, register) in [
        (0, "two\n", true, "  one\n"),
        (2, "two\n", true, "  one\n"),
        (3, "  o\ntwo\n", false, "ne"),
    ] {
        let mut state = VimState::new();
        let mut text = "  one\ntwo\n".to_owned();
        state.set_cursor(&text, cursor);
        keys(&mut state, &mut text, "d");
        state.take_search_count();
        let action = state.apply_search_motion(&mut text, 6, search_motion("two", false, 1));
        assert_eq!(text, expected);
        assert_eq!(
            action.yanked,
            Some(Yank {
                text: register.into(),
                linewise
            })
        );
        keys(&mut state, &mut text, "u");
        assert_eq!(text, "  one\ntwo\n");
    }
}

#[test]
fn crlf_line_motions_and_edits_never_treat_the_break_as_content() {
    for command in ["$", "99l", "99|"] {
        let mut state = VimState::new();
        let mut text = "α🙂\r\nβ\r\n".to_owned();
        keys(&mut state, &mut text, command);
        assert_eq!(state.cursor, 1, "{command}");
    }
    for (initial, command, expected) in [
        ("α🙂\r\nβ", "$x", "α\r\nβ"),
        ("α🙂\r\nβ", "D", "\r\nβ"),
        ("αβ\r\nγ", "dw", "\r\nγ"),
        ("\r\nβ", "x", "\r\nβ"),
        ("α\r\nβ", "Gdd", "α"),
        ("α\r\nβ\r\nγ", "Gdk", "α"),
    ] {
        let mut state = VimState::new();
        let mut text = initial.to_owned();
        keys(&mut state, &mut text, command);
        assert_eq!(text, expected, "{command}");
        if text != initial {
            keys(&mut state, &mut text, "u");
            assert_eq!(text, initial, "undo {command}");
        }
    }
    let mut state = VimState::new();
    let mut text = "α\r\nβ".to_owned();
    keys(&mut state, &mut text, "A!");
    state.finish_insert(&mut text);
    assert_eq!(text, "α!\r\nβ");
    keys(&mut state, &mut text, "u");
    assert_eq!(text, "α\r\nβ");
}

#[test]
fn crlf_visual_replace_keeps_line_breaks_and_paragraphs_see_empty_lines() {
    let mut state = VimState::new();
    let mut text = "α\r\n\r\nβ\r\n".to_owned();
    keys(&mut state, &mut text, "}");
    assert_eq!(state.cursor, 3);
    keys(&mut state, &mut text, "ggVjrx");
    assert_eq!(text, "x\r\n\r\nβ\r\n");
}

#[test]
fn column_and_punctuation_motions() {
    let mut state = VimState::new();
    let mut text = "abcdef\nx\nabcdef".to_owned();
    keys(&mut state, &mut text, "4ljj");
    assert_eq!(position(&state, &text), (2, 4));
    text = "foo.bar baz".to_owned();
    state.set_cursor(&text, 0);
    keys(&mut state, &mut text, "w");
    assert_eq!(state.cursor, 3);
    keys(&mut state, &mut text, "we");
    assert_eq!(state.cursor, 6);
}

#[test]
fn find_replace_join_and_matching_bracket() {
    let mut state = VimState::new();
    let mut text = "one, two, three".to_owned();
    keys(&mut state, &mut text, "f,;");
    assert_eq!(state.cursor, 8);
    keys(&mut state, &mut text, "0dt,");
    assert_eq!(text, ", two, three");
    keys(&mut state, &mut text, "u0d2f,");
    assert_eq!(text, " three");
    text = "abc\ndef".to_owned();
    state = VimState::new();
    keys(&mut state, &mut text, "2r🙂J");
    assert_eq!(text, "🙂🙂c def");
    text = "(a(b)c)".to_owned();
    state = VimState::new();
    keys(&mut state, &mut text, "%");
    assert_eq!(state.cursor, 6);
}

#[test]
fn character_find_counts_and_repeats_cross_lines_in_both_directions() {
    let mut state = VimState::new();
    let mut text = "a🦀x\n\n中x\r\n🙂x\n".to_owned();
    let original = text.clone();
    for (command, cursor) in [
        ("fx", 2),
        (";", 6),
        (";", 10),
        (";", 10),
        (",", 6),
        (";", 10),
        ("G$2Fx", 2),
        ("2;", 2),
        ("2,", 10),
        ("3Fx", 10),
        ("fz", 10),
    ] {
        keys(&mut state, &mut text, command);
        assert_eq!(state.cursor, cursor, "{command}");
        assert_eq!(state.pending_command(), "");
    }
    assert_eq!(text, original);
    assert!(!state.can_undo());
}

#[test]
fn till_repeats_advance_past_line_endings_and_keep_the_original_direction() {
    for ending in ["\n", "\r\n"] {
        let mut state = VimState::new();
        let mut text = format!("ab{ending}x cd{ending}x ef");
        let first_x = 2 + ending.chars().count();
        let second_x = first_x + 4 + ending.chars().count();
        for (command, cursor) in [
            ("tx", 1),
            (";", first_x + 3),
            (",", first_x + 1),
            (";", first_x + 3),
            (";", first_x + 3),
        ] {
            keys(&mut state, &mut text, command);
            assert_eq!(state.cursor, cursor, "{command} with {ending:?}");
        }
        state.set_cursor(&text, 0);
        keys(&mut state, &mut text, "2tx");
        assert_eq!(state.cursor, second_x - ending.chars().count() - 1);

        state = VimState::new();
        text = format!("x{ending}ab{ending}x{ending}cd");
        let second_x = 3 + 2 * ending.chars().count();
        keys(&mut state, &mut text, "G$Tx");
        assert_eq!(state.cursor, second_x + 1 + ending.chars().count());
        keys(&mut state, &mut text, ";");
        assert_eq!(state.cursor, 1 + ending.chars().count());
        keys(&mut state, &mut text, ",");
        assert_eq!(state.cursor, second_x - ending.chars().count() - 1);
    }
}

#[test]
fn till_repeats_skip_adjacent_matches_in_both_directions() {
    for (initial, cursor, command, expected) in [
        ("a,b,c,d", 0, "t,;", 2),
        ("a,b,c,d", 0, "t,2;", 4),
        (",,a,b,c", 2, "t,,", 1),
        ("a,b,c,d", 6, "T,;", 4),
        ("a,b,c,,", 2, "T,,", 4),
    ] {
        let mut state = VimState::new();
        let mut text = initial.to_owned();
        state.cursor = cursor;
        keys(&mut state, &mut text, command);
        assert_eq!(state.cursor, expected, "{command}");
    }
}

#[test]
fn character_find_operators_keep_exact_multiline_spans() {
    for (initial, command, expected, yanked, cursor) in [
        ("a🦀\nxz\nbx!", "dfx", "z\nbx!", "a🦀\nx", 0),
        ("a🦀\nxz\nbx!", "dtx", "xz\nbx!", "a🦀\n", 0),
        ("a🦀x\nmid\nlast!", "G$dF🦀", "a!", "🦀x\nmid\nlast", 1),
        ("a🦀x\nmid\nlast!", "G$dT🦀", "a🦀!", "x\nmid\nlast", 2),
        (
            "a🦀x\nmid\nlast!",
            "G$yF🦀",
            "a🦀x\nmid\nlast!",
            "🦀x\nmid\nlast",
            1,
        ),
    ] {
        let mut state = VimState::new();
        let mut text = initial.to_owned();
        keys(&mut state, &mut text, command);
        assert_eq!(text, expected, "{command}");
        assert_eq!(state.cursor, cursor, "{command}");
        assert_eq!(state.register().unwrap().text, yanked, "{command}");
        assert!(!state.register().unwrap().linewise);
    }
    for (command, expected) in [(">fx", "    ab\n    x\nnext"), (">tx", "    ab\nx\nnext")] {
        let mut state = VimState::new();
        let mut text = "ab\nx\nnext".to_owned();
        keys(&mut state, &mut text, command);
        assert_eq!(text, expected, "{command}");
    }
}

#[test]
fn repeated_find_operators_and_visual_find_share_multiline_targets() {
    let mut state = VimState::new();
    let mut text = "ax\nbx\ncx\ndx".to_owned();
    keys(&mut state, &mut text, "fx0d;.");
    assert_eq!(text, "\ncx\ndx");
    keys(&mut state, &mut text, "uu");
    assert_eq!(text, "ax\nbx\ncx\ndx");
    keys(&mut state, &mut text, "G$2d,");
    assert_eq!(text, "ax\nbx");

    state = VimState::new();
    text = "a\n🙂b\n🙂c".to_owned();
    keys(&mut state, &mut text, "vf🙂");
    assert_eq!(state.selection(&text), Some(0..3));
    keys(&mut state, &mut text, ";");
    assert_eq!(state.selection(&text), Some(0..6));
    keys(&mut state, &mut text, ",");
    assert_eq!(state.selection(&text), Some(0..3));
    keys(&mut state, &mut text, "d");
    assert_eq!(text, "b\n🙂c");
    keys(&mut state, &mut text, "uggVf🙂;");
    assert_eq!(state.selection(&text), Some(0..7));
}

#[test]
fn character_find_accepts_literal_tab_and_repeats_across_lines() {
    let mut state = VimState::new();
    let mut text = "a\n\tb\n\tc".to_owned();
    keys(&mut state, &mut text, "f");
    state.handle_key(&mut text, VimKey::Tab);
    assert_eq!(state.cursor, 2);
    keys(&mut state, &mut text, ";,");
    assert_eq!(state.cursor, 2);
    keys(&mut state, &mut text, "0d;");
    assert_eq!(text, "a\nc");
}

#[test]
fn text_objects_select_words_pairs_and_quotes() {
    let mut state = VimState::new();
    let mut text = "one two three".to_owned();
    keys(&mut state, &mut text, "wviw");
    assert_eq!(state.selection(&text), Some(4..7));
    keys(&mut state, &mut text, "d");
    assert_eq!(text, "one  three");
    text = "call(first, nested(value))".to_owned();
    state = VimState::new();
    state.set_cursor(&text, 20);
    keys(&mut state, &mut text, "ci(");
    assert_eq!(text, "call(first, nested())");
    state.insert_text(&mut text, "arg");
    state.finish_insert(&mut text);
    keys(&mut state, &mut text, "u");
    assert_eq!(text, "call(first, nested(value))");
    text = "let s = \"some \\\"quoted\\\" text\";".to_owned();
    state = VimState::new();
    state.set_cursor(&text, 12);
    keys(&mut state, &mut text, "di\"");
    assert_eq!(text, "let s = \"\";");
}

#[test]
fn explicit_one_before_operator_is_retained_for_absolute_line_motion() {
    for command in ["1dG", "d1G"] {
        let mut state = VimState::new();
        let mut text = "one\ntwo\nthree\nfour".to_owned();
        keys(&mut state, &mut text, "3G");
        keys(&mut state, &mut text, command);
        assert_eq!(text, "four", "command={command}");
    }
    let mut state = VimState::new();
    let mut text = "one\ntwo\nthree\nfour".to_owned();
    keys(&mut state, &mut text, "3G1yG");
    assert_eq!(state.register().unwrap().text, "one\ntwo\nthree\n");
    keys(&mut state, &mut text, "3G1>G");
    assert_eq!(text, "    one\n    two\n    three\nfour");
}

#[test]
fn word_delete_preserves_current_line_break_when_next_word_is_on_next_line() {
    for (initial, command, expected) in [
        ("one\ntwo", "dw", "\ntwo"),
        ("one two\nthree", "d2w", "\nthree"),
        ("one\ntwo\nthree", "d2w", "three"),
        ("one\ntwo three", "d2w", "three"),
    ] {
        let mut state = VimState::new();
        let mut text = initial.to_owned();
        keys(&mut state, &mut text, command);
        assert_eq!(text, expected, "{command} in {initial:?}");
    }
}

#[test]
fn backward_find_operator_excludes_the_original_cursor_character() {
    for (command, expected) in [("$dF,", "foo, barz"), ("$dT,", "foo, bar,z")] {
        let mut state = VimState::new();
        let mut text = "foo, bar, baz".to_owned();
        keys(&mut state, &mut text, command);
        assert_eq!(text, expected, "{command}");
    }
}

#[test]
fn word_motions_visit_empty_lines_but_word_end_skips_them() {
    for (initial, command, expected) in [
        ("one\n\n\nthree", "w", (1, 0)),
        ("one\n\n\nthree", "2w", (2, 0)),
        ("one\n\n\nthree", "3w", (3, 0)),
        ("one\n\n\nthree", "Gb", (2, 0)),
        ("one\n\n\nthree", "G2b", (1, 0)),
        ("one\n\n\nthree", "G3b", (0, 0)),
        ("one\n\n\nthree", "2e", (3, 4)),
        ("one\n   \nthree", "w", (2, 0)),
        ("one\n   \nthree", "Gb", (0, 0)),
        ("\n\nthree", "w", (1, 0)),
        ("\n\nthree", "G2b", (0, 0)),
    ] {
        let mut state = VimState::new();
        let mut text = initial.to_owned();
        keys(&mut state, &mut text, command);
        assert_eq!(position(&state, &text), expected, "{initial:?}, {command}");
    }
}

#[test]
fn inner_word_counts_include_whitespace_and_around_word_includes_content() {
    for (initial, command, expected) in [
        ("one two three", "d2iw", "two three"),
        ("one two three", "d3iw", " three"),
        ("one two three", "d2aw", "three"),
        ("one  two three", "3ldiw", "onetwo three"),
        ("one  two three", "3ld2iw", "one three"),
        ("one  two three", "3ldaw", "one three"),
        ("one  two three", "3ld2aw", "one"),
        ("foo.bar baz", "d2iW", "baz"),
    ] {
        let mut state = VimState::new();
        let mut text = initial.to_owned();
        keys(&mut state, &mut text, command);
        assert_eq!(text, expected, "{initial:?}, {command}");
    }
}

#[test]
fn quoted_objects_find_forward_pairs_and_adjacent_quote_spans() {
    for (initial, cursor, command, expected) in [
        ("let s = \"hello\";", 0, "di\"", "let s = \"\";"),
        ("\"one\" middle \"two\"", 7, "di\"", "\"one\"\"two\""),
        ("\"one\" middle \"two\"", 0, "di\"", "\"\" middle \"two\""),
        ("\"one\" middle \"two\"", 4, "di\"", "\"\" middle \"two\""),
        ("pre \"one\" tail", 5, "2di\"", "pre  tail"),
        ("pre \"one\" tail", 5, "2da\"", "pre tail"),
        ("pre \"one\" tail", 13, "di\"", "pre \"one\" tail"),
        ("pre\n\"one\"", 0, "di\"", "pre\n\"one\""),
    ] {
        let mut state = VimState::new();
        let mut text = initial.to_owned();
        state.set_cursor(&text, cursor);
        keys(&mut state, &mut text, command);
        assert_eq!(text, expected, "{initial:?}, {cursor}, {command}");
    }
}

#[test]
fn backward_word_end_and_last_nonblank_motions_support_counts_and_operators() {
    for (initial, cursor, command, expected) in [
        ("one two three", 12, "ge", 6),
        ("one two three", 12, "2ge", 2),
        ("foo.bar baz", 10, "ge", 6),
        ("foo.bar baz", 10, "2ge", 3),
        ("foo.bar baz", 10, "2gE", 0),
        ("one\n\nthree", 9, "ge", 2),
        ("  one  \n two \t", 0, "g_", 4),
        ("  one  \n two \t", 0, "2g_", 11),
    ] {
        let mut state = VimState::new();
        let mut text = initial.to_owned();
        state.set_cursor(&text, cursor);
        keys(&mut state, &mut text, command);
        assert_eq!(state.cursor, expected, "{initial:?}, {command}");
    }
    let mut state = VimState::new();
    let mut text = "one two three".to_owned();
    state.set_cursor(&text, 12);
    keys(&mut state, &mut text, "dge");
    assert_eq!(text, "one tw");
    assert_eq!(state.register().unwrap().text, "o three");
    keys(&mut state, &mut text, "u0dg_");
    assert_eq!(text, "");
}

#[test]
fn line_start_and_absolute_column_motions_handle_counts_unicode_and_empty_rows() {
    let mut state = VimState::new();
    let mut text = "  αβ\n\n   second\n  third".to_owned();
    keys(&mut state, &mut text, "3|");
    assert_eq!(state.cursor, 2);
    state.handle_key(&mut text, VimKey::Enter);
    assert_eq!(position(&state, &text), (1, 0));
    keys(&mut state, &mut text, "+");
    assert_eq!(position(&state, &text), (2, 3));
    keys(&mut state, &mut text, "2-");
    assert_eq!(position(&state, &text), (0, 2));
    keys(&mut state, &mut text, "3_");
    assert_eq!(position(&state, &text), (2, 3));
    keys(&mut state, &mut text, "999|");
    assert_eq!(position(&state, &text), (2, 8));
    keys(&mut state, &mut text, "d-");
    assert_eq!(text, "  αβ\n  third");
    assert!(state.register().unwrap().linewise);
    keys(&mut state, &mut text, "u0d");
    state.handle_key(&mut text, VimKey::Enter);
    assert_eq!(text, "  αβ\n");
}

#[test]
fn word_objects_never_cross_line_breaks() {
    for (initial, cursor, command, expected) in [
        ("foo bar\n    baz\n", 4, "daw", "foo\n    baz\n"),
        ("foo\nbar\n", 0, "daw", "\nbar\n"),
        ("a\n\n\nb\n", 2, "diw", "a\n\n\nb\n"),
        ("a\n    foo\n", 2, "diw", "a\nfoo\n"),
        ("a\r\n    foo\r\n", 3, "diw", "a\r\nfoo\r\n"),
        ("foo  \nbar", 0, "daw", "\nbar"),
        ("one two\nthree", 4, "d2aw", "one\nthree"),
        ("one two\nthree", 0, "d5iw", "\nthree"),
        ("x foo\n", 2, "ciwy\u{1b}", "x y\n"),
    ] {
        let (text, _, _) = run(initial, cursor, command);
        assert_eq!(text, expected, "{initial:?}, {cursor}, {command}");
    }
}

#[test]
fn crlf_word_motions_stop_on_empty_lines() {
    for (initial, cursor, command, expected) in [
        ("a\r\n\r\nb", 0, "w", 3),
        ("a\r\n\r\n\r\nb", 0, "2w", 5),
        ("a\r\n\r\nb", 5, "b", 3),
        ("a\n\nb", 0, "w", 2),
    ] {
        let (_, cursor, _) = run(initial, cursor, command);
        assert_eq!(cursor, expected, "{initial:?}, {command}");
    }
}

#[test]
fn block_objects_on_their_own_lines_are_linewise_and_paragraph_motions_are_exclusive() {
    for (initial, cursor, command, expected) in [
        ("if x {\n    a;\n}\n", 0, "di{", "if x {\n}\n"),
        ("if x {\n    a;\n}\n", 9, "di{", "if x {\n}\n"),
        ("if x {\n    a;\n}\n", 9, "ci{y\u{1b}", "if x {\ny}\n"),
        ("if x {\r\n    a;\r\n}\r\n", 9, "di{", "if x {\r\n}\r\n"),
        ("  f(\n    a,\n    b\n  )\n", 7, "di(", "  f(\n  )\n"),
        ("if x { a; }\n", 0, "di{", "if x {}\n"),
        ("a b\nc\n\nd", 2, "d}", "a \n\nd"),
        ("a b\nc\n\nd", 0, "d}", "\nd"),
        ("a\nb", 0, "d}", ""),
        ("a\n\nb c\nd", 7, "d{", "a\nd"),
    ] {
        let (text, _, _) = run(initial, cursor, command);
        assert_eq!(text, expected, "{initial:?}, {cursor}, {command}");
    }
    for (initial, cursor, command, expected) in [
        ("a\n\nb c\nd", 5, "{", 2),
        ("a\n\nb\nc\n\nd", 3, "}", 7),
        ("a\nb", 0, "}", 2),
    ] {
        let (_, cursor, _) = run(initial, cursor, command);
        assert_eq!(cursor, expected, "{initial:?}, {command}");
    }
}

#[test]
fn paragraph_objects_select_blocks_and_their_blank_lines() {
    for (initial, cursor, command, expected) in [
        ("a\nb\n\nc\n", 0, "dap", "c\n"),
        ("a\nb\n\nc\n", 0, "dip", "\nc\n"),
        ("a\nb\n\nc\n", 4, "dip", "a\nb\nc\n"),
        ("a\nb\n\nc\n", 4, "dap", "a\nb\n"),
        ("a\n\nc\nd\n", 4, "dap", "a\n"),
        ("a\n\nb\n\nc\n", 0, "d2ap", "c\n"),
        ("a\n\nb\n\nc\n", 0, "d3ip", "\nc\n"),
        ("a\nb\n  \nc", 0, "yipP", "a\nb\na\nb\n  \nc"),
        ("a\nb\n\nc\n", 0, ">ip", "    a\n    b\n\nc\n"),
        ("one\ntwo\n\nx\n", 0, "cipnew\u{1b}", "new\n\nx\n"),
    ] {
        let (text, _, _) = run(initial, cursor, command);
        assert_eq!(text, expected, "{initial:?}, {cursor}, {command}");
    }
    let (_, _, state) = run("a\nb\n\nc\n", 0, "vip");
    assert_eq!(state.mode, VimMode::VisualLine);
    assert_eq!(state.selection("a\nb\n\nc\n"), Some(0..4));
}

#[test]
fn bracket_pairs_for_highlighting_respect_the_caret_kind_and_window() {
    let text: Vec<char> = "f(a, [b], {c})🦀)".chars().collect();
    assert_eq!(bracket_pair(&text, 0, 1, false), Some((1, 13)));
    assert_eq!(bracket_pair(&text, 0, 13, false), Some((13, 1)));
    assert_eq!(bracket_pair(&text, 0, 5, false), Some((5, 7)));
    // A block cursor after a bracket shows nothing; a caret between
    // characters pairs the bracket before it.
    assert_eq!(bracket_pair(&text, 0, 2, false), None);
    assert_eq!(bracket_pair(&text, 0, 2, true), Some((1, 13)));
    assert_eq!(bracket_pair(&text, 0, 14, true), Some((13, 1)));
    // An unmatched bracket and one outside the window have no partner.
    assert_eq!(bracket_pair(&text, 0, 15, false), None);
    assert_eq!(bracket_pair(&text[5..], 5, 5, false), Some((5, 7)));
    assert_eq!(bracket_pair(&text[3..], 3, 13, false), None);
    assert_eq!(bracket_pair(&text[3..], 3, 1, true), None);
    // `%` still finds the next bracket on the line and its partner.
    let mut state = VimState::new();
    let mut source = "f(a, [b])".to_owned();
    keys(&mut state, &mut source, "%");
    assert_eq!(state.cursor, 8);
}

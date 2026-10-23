use super::*;

#[test]
fn yank_feedback_reports_unicode_source_ranges_and_real_newlines() {
    let mut text = "α🦀 one\nlast".to_owned();
    let mut state = VimState::new();
    keys(&mut state, &mut text, "vl");
    let action = state.handle_key(&mut text, VimKey::Char('y'));
    assert_eq!(action.yanked.as_ref().unwrap().text, "α🦀");
    assert_eq!(action.yanked_ranges.len(), 1);
    assert_eq!(action.yanked_ranges[0], 0..2);
    assert!(!action.text_changed);
    assert!(
        state
            .handle_key(&mut text, VimKey::Char('j'))
            .yanked_ranges
            .is_empty()
    );

    keys(&mut state, &mut text, "y");
    let action = state.handle_key(&mut text, VimKey::Char('y'));
    assert_eq!(action.yanked.as_ref().unwrap().text, "last\n");
    assert_eq!(action.yanked_ranges.len(), 1);
    assert_eq!(action.yanked_ranges[0], 7..11);
    keys(&mut state, &mut text, "ggV");
    let action = state.handle_key(&mut text, VimKey::Char('y'));
    assert_eq!(action.yanked_ranges.len(), 1);
    assert_eq!(action.yanked_ranges[0], 0..7);
}

#[test]
fn clipboard_visual_cut_preserves_register_and_geometry_for_undo_and_dot() {
    for register in [
        None,
        Some(Yank {
            text: "KEEP🦀".into(),
            linewise: false,
            block_width: None,
        }),
    ] {
        let mut text = "α🦀βabcdef".to_owned();
        let mut state = VimState::new();
        state.register = register.clone();
        state.set_selection(&text, 0..3);
        keys(&mut state, &mut text, "2r");
        let (action, copied) = state.cut_visual_to_clipboard(&mut text);
        assert_eq!(copied.as_deref(), Some("α🦀β"));
        assert!(action.text_changed && action.yanked.is_none());
        assert!(action.yanked_ranges.is_empty());
        assert_eq!(state.register, register);
        assert_eq!(text, "abcdef");
        keys(&mut state, &mut text, ".");
        assert_eq!(text, "def");
        assert_eq!(state.register.as_ref().unwrap().text, "abc");
        keys(&mut state, &mut text, "uu");
        assert_eq!(text, "α🦀βabcdef");
    }
}

#[test]
fn native_clipboard_ranges_use_unicode_selection_or_the_complete_source_line() {
    let text = "α🦀\nβγ";
    assert_eq!(clipboard_range(text, 1..2, 0), 1..2);
    assert_eq!(clipboard_range(text, 1..1, 1), 0..3);
    assert_eq!(clipboard_range(text, 4..4, 4), 3..5);
    assert_eq!(clipboard_range(text, 99..99, 99), 3..5);
    assert_eq!(clipboard_range("", 0..0, 0), 0..0);
    assert_eq!(clipboard_range("α\n", 2..2, 2), 2..2);
}

#[test]
fn native_visual_cut_discards_unfinished_prefixes_and_repeats_completed_area() {
    for prefix in ["", "r", "i", "a", "f", "g", "23", "2f", "g2"] {
        let original = "ä🙂中defghi";
        let mut text = original.to_owned();
        let mut state = VimState::new();
        keys(&mut state, &mut text, &format!("vll{prefix}"));
        assert_eq!(state.selection(&text), Some(0..3), "{prefix}");
        let action = state.cut_visual(&mut text);
        assert_eq!(text, "defghi", "{prefix}");
        assert_eq!(
            action.yanked,
            Some(Yank {
                text: "ä🙂中".into(),
                linewise: false,
                block_width: None,
            })
        );
        assert!(action.text_changed && action.mode_changed);
        assert_eq!(state.pending_command(), "");
        keys(&mut state, &mut text, ".w");
        assert_eq!(text, "ghi", "{prefix}");
        assert_eq!(state.pending_command(), "");
        assert_eq!(state.register().unwrap().text, "def");
        keys(&mut state, &mut text, "u");
        assert_eq!(text, "defghi");
        keys(&mut state, &mut text, "u");
        assert_eq!(text, original);
    }
}

#[test]
fn native_linewise_cut_ignores_pending_counts_and_dot_counts() {
    let original = "one\ntwo\nthree\nfour\nfive\nsix\n";
    let mut text = original.to_owned();
    let mut state = VimState::new();
    keys(&mut state, &mut text, "Vj23r");
    let action = state.cut_visual(&mut text);
    assert_eq!(text, "three\nfour\nfive\nsix\n");
    assert_eq!(
        action.yanked,
        Some(Yank {
            text: "one\ntwo\n".into(),
            linewise: true,
            block_width: None,
        })
    );
    keys(&mut state, &mut text, "9.");
    assert_eq!(text, "five\nsix\n");
    assert_eq!(state.register().unwrap().text, "three\nfour\n");
    assert!(state.register().unwrap().linewise);
    keys(&mut state, &mut text, "u");
    assert_eq!(text, "three\nfour\nfive\nsix\n");
    keys(&mut state, &mut text, "u");
    assert_eq!(text, original);
}

#[test]
fn mouse_and_external_visual_cuts_repeat_without_any_typed_motion_history() {
    for mouse in [false, true] {
        let mut text = "abcdefghi".to_owned();
        let mut state = VimState::new();
        if mouse {
            state.set_mouse_selection(&text, 3, 0, false);
        } else {
            state.set_selection_boundaries(&text, 3, 0);
        }
        keys(&mut state, &mut text, "i");
        state.cut_visual(&mut text);
        keys(&mut state, &mut text, ".w");
        assert_eq!(text, "ghi");
        assert_eq!(state.pending_command(), "");
        keys(&mut state, &mut text, "uu");
        assert_eq!(text, "abcdefghi");
    }
    let mut text = "abcdefghi".to_owned();
    let mut state = VimState::new();
    state.set_mouse_selection(&text, 3, 0, false);
    keys(&mut state, &mut text, "d.w");
    assert_eq!(text, "ghi");
    assert_eq!(state.pending_command(), "");
}

#[test]
fn multiline_native_visual_cut_repeats_line_count_and_last_line_column() {
    let original = "ab\ncdef\nXYZuvw\n";
    let mut text = original.to_owned();
    let mut state = VimState::new();
    state.set_selection(&text, 1..5);
    state.cut_visual(&mut text);
    assert_eq!(text, "aef\nXYZuvw\n");
    keys(&mut state, &mut text, ".");
    assert_eq!(text, "aZuvw\n");
    assert_eq!(state.register().unwrap().text, "ef\nXY");
    assert!(!state.register().unwrap().linewise);
    keys(&mut state, &mut text, "u");
    assert_eq!(text, "aef\nXYZuvw\n");
    keys(&mut state, &mut text, "u");
    assert_eq!(text, original);
}

#[test]
fn visual_cut_replay_is_independent_of_key_history_and_motion_count_limits() {
    let area = MAX_COUNT + 2;
    let mut text = format!("{}{}", "a".repeat(area), "z".repeat(area));
    let mut state = VimState::new();
    state.set_selection(&text, 0..area);
    state.cut_visual(&mut text);
    assert_eq!(text.len(), area);
    keys(&mut state, &mut text, ".");
    assert!(text.is_empty());
    assert_eq!(state.register().unwrap().text.len(), area);

    text = "a".repeat(213);
    state = VimState::new();
    keys(&mut state, &mut text, &format!("v{}d.", "l".repeat(70)));
    assert_eq!(text.len(), 71);
    assert_eq!(state.pending_command(), "");
}

#[test]
fn native_cut_does_not_change_normal_mode_or_literal_visual_replacement() {
    let mut text = "abcdefghi".to_owned();
    let mut state = VimState::new();
    keys(&mut state, &mut text, "d");
    assert_eq!(state.cut_visual(&mut text), Action::default());
    assert_eq!(state.pending_command(), "d");
    state.handle_key(&mut text, VimKey::Escape);
    keys(&mut state, &mut text, "vllrd");
    assert_eq!(text, "ddddefghi");
    keys(&mut state, &mut text, "u");
    assert_eq!(text, "abcdefghi");
}

#[test]
fn visual_cut_of_empty_document_exits_visual_without_arming_a_command() {
    for mode in ['v', 'V'] {
        for operator in ['d', 'x'] {
            let mut text = String::new();
            let mut state = VimState::new();
            state.handle_key(&mut text, VimKey::Char(mode));
            let action = state.handle_key(&mut text, VimKey::Char(operator));
            assert!(action.consumed && action.mode_changed);
            assert!(!action.text_changed);
            assert_eq!(state.mode, VimMode::Normal);
            assert_eq!(state.pending_command(), "");
            assert!(!state.can_undo());
        }
    }
}

#[test]
fn native_visual_cut_preserves_explicit_line_end_intent_not_mouse_coincidence() {
    let original = "one\nlonger\nend";
    for explicit in [false, true] {
        let mut text = original.to_owned();
        let mut state = VimState::new();
        if explicit {
            keys(&mut state, &mut text, "v$r");
        } else {
            state.set_mouse_selection(&text, 3, 0, false);
        }
        state.cut_visual(&mut text);
        assert_eq!(text, "\nlonger\nend");
        keys(&mut state, &mut text, "j.");
        assert_eq!(text, if explicit { "\n\nend" } else { "\nger\nend" });
        keys(&mut state, &mut text, "u");
        assert_eq!(text, "\nlonger\nend");
        keys(&mut state, &mut text, "u");
        assert_eq!(text, original);
    }
}

#[test]
fn backwards_search_yank_moves_to_start_without_changing_text() {
    let mut state = VimState::new();
    let mut text = "one foo two".to_owned();
    state.set_cursor(&text, 8);
    keys(&mut state, &mut text, "y");
    let action = state.apply_search_motion(&mut text, 4, search_motion("foo", true, 1));
    assert_eq!(text, "one foo two");
    assert!(!state.can_undo());
    assert_eq!(state.cursor, 4);
    assert_eq!(action.yanked.unwrap().text, "foo ");
}

#[test]
fn crlf_linewise_registers_and_pastes_preserve_complete_breaks() {
    for (initial, command, expected) in [
        ("α\r\nβ", "yyp", "α\r\nα\r\nβ"),
        ("α\r\nβ", "yyGp", "α\r\nβ\r\nα"),
        ("α\r\nβ", "GyyP", "α\r\nβ\r\nβ"),
        ("α\r\nβ", "yyGVP", "α\r\nα"),
    ] {
        let mut state = VimState::new();
        let mut text = initial.to_owned();
        keys(&mut state, &mut text, command);
        assert_eq!(text, expected, "{command}");
        keys(&mut state, &mut text, "u");
        assert_eq!(text, initial, "undo {command}");
    }
    let mut state = VimState::new();
    let mut text = "α\r\nβ".to_owned();
    keys(&mut state, &mut text, "yy");
    assert_eq!(state.register().unwrap().text, "α\r\n");
}

#[test]
fn paste_blank_line_cursor_and_final_newlines() {
    let mut state = VimState::new();
    let mut text = "a\nb".to_owned();
    state.set_register(Yank {
        text: "\n".to_owned(),
        linewise: true,
        block_width: None,
    });
    keys(&mut state, &mut text, "p");
    assert_eq!(text, "a\n\nb");
    assert_eq!(position(&state, &text), (1, 0));
    for (initial, command, expected) in [
        ("a", "yyp", "a\na"),
        ("a\n", "yyp", "a\na\n"),
        ("a\nb", "yyGp", "a\nb\na"),
    ] {
        let mut state = VimState::new();
        let mut text = initial.to_owned();
        keys(&mut state, &mut text, command);
        assert_eq!(text, expected, "{command}");
    }
}

#[test]
fn visual_character_selection_yank_cut_and_change() {
    let mut state = VimState::new();
    let mut text = "ä🙂中 xyz".to_owned();
    keys(&mut state, &mut text, "vl");
    assert_eq!(state.selection(&text), Some(0..2));
    let action = state.handle_key(&mut text, VimKey::Char('y'));
    assert_eq!(action.yanked.unwrap().text, "ä🙂");
    assert_eq!(state.mode, VimMode::Normal);
    assert!(state.selection(&text).is_none());
    keys(&mut state, &mut text, "vld");
    assert_eq!(text, "中 xyz");
    keys(&mut state, &mut text, "u0vlc");
    state.insert_text(&mut text, "new");
    state.finish_insert(&mut text);
    assert_eq!(text, "new中 xyz");
    keys(&mut state, &mut text, "u");
    assert_eq!(text, "ä🙂中 xyz");
}

#[test]
fn visual_paste_and_indent_are_undoable() {
    let mut state = VimState::new();
    let mut text = "one\ntwo\nthree".to_owned();
    state.set_register(Yank {
        text: "other\n".to_owned(),
        linewise: true,
        block_width: None,
    });
    keys(&mut state, &mut text, "Vjp");
    assert_eq!(text, "other\nthree");
    keys(&mut state, &mut text, "uggVj>");
    assert_eq!(text, "    one\n    two\nthree");
    assert_eq!(state.mode, VimMode::Normal);
    keys(&mut state, &mut text, "ugg2>>");
    assert_eq!(text, "    one\n    two\nthree");
    keys(&mut state, &mut text, "2<<");
    assert_eq!(text, "one\ntwo\nthree");
}

#[test]
fn yank_register_zero_survives_deletes_changes_and_visual_pastes() {
    let mut state = VimState::new();
    let mut text = "one two three".to_owned();
    keys(&mut state, &mut text, "yiwwdiw");
    assert_eq!(text, "one  three");
    assert_eq!(state.register().unwrap().text, "two");
    assert_eq!(state.yank_register().unwrap().text, "one");
    keys(&mut state, &mut text, "\"0p");
    assert_eq!(text, "one  onethree");
    assert_eq!(state.yank_register().unwrap().text, "one");
    keys(&mut state, &mut text, "u");
    assert_eq!(text, "one  three");
    keys(&mut state, &mut text, "wviw\"0p");
    assert_eq!(text, "one  one");
    assert_eq!(state.register().unwrap().text, "three");
    assert_eq!(state.yank_register().unwrap().text, "one");
    keys(&mut state, &mut text, "u");
    assert_eq!(text, "one  three");
}

#[test]
fn yank_register_zero_counts_and_dot_use_linewise_or_characterwise_content() {
    let mut state = VimState::new();
    let mut text = "one\ntwo\nthree".to_owned();
    keys(&mut state, &mut text, "yyjdd\"0p");
    assert_eq!(text, "one\nthree\none");
    keys(&mut state, &mut text, ".");
    assert_eq!(text, "one\nthree\none\none");
    assert_eq!(state.yank_register().unwrap().text, "one\n");
    keys(&mut state, &mut text, "u");
    assert_eq!(text, "one\nthree\none");

    state = VimState::new();
    text = "🦀abc".to_owned();
    keys(&mut state, &mut text, "yl2\"0p");
    assert_eq!(text, "🦀🦀🦀abc");
    keys(&mut state, &mut text, "u\"02p");
    assert_eq!(text, "🦀🦀🦀abc");
    keys(&mut state, &mut text, "u\"0p3.");
    assert_eq!(text, "🦀🦀🦀🦀🦀abc");
}

#[test]
fn multiline_character_paste_starts_at_inserted_text_and_dot_is_undoable() {
    for (command, repeated) in [("p", "a🦀🦀\n中\n中bc"), ("lP", "a🦀\n中🦀\n中bc")] {
        let mut state = VimState::new();
        let mut text = "abc".to_owned();
        state.set_register(Yank {
            text: "🦀\n中".into(),
            linewise: false,
            block_width: None,
        });
        keys(&mut state, &mut text, command);
        assert_eq!(text, "a🦀\n中bc", "{command}");
        assert_eq!(state.cursor, 1, "{command}");
        keys(&mut state, &mut text, ".");
        assert_eq!(text, repeated, "{command}");
        keys(&mut state, &mut text, "u");
        assert_eq!(text, "a🦀\n中bc", "{command}");
    }
}

#[test]
fn visual_uppercase_p_preserves_register_for_further_pastes_and_dot() {
    for (command, register) in [("p", "abc"), ("P", "XY")] {
        let mut state = VimState::new();
        let mut text = "abcdefghi".to_owned();
        state.set_register(Yank {
            text: "XY".into(),
            linewise: false,
            block_width: None,
        });
        keys(&mut state, &mut text, "v2l");
        let action = state.handle_key(&mut text, VimKey::Char(command.chars().next().unwrap()));
        assert_eq!(text, "XYdefghi");
        assert_eq!(state.register().unwrap().text, register);
        assert_eq!(action.yanked.is_none(), command == "P");
        keys(&mut state, &mut text, "u");
        assert_eq!(text, "abcdefghi");
    }
    let mut state = VimState::new();
    let mut text = "abcdefghi".to_owned();
    state.set_register(Yank {
        text: "XY".into(),
        linewise: false,
        block_width: None,
    });
    keys(&mut state, &mut text, "v2lPl.");
    assert_eq!(text, "XYXYghi");
    assert_eq!(state.register().unwrap().text, "XY");
    keys(&mut state, &mut text, "u");
    assert_eq!(text, "XYdefghi");
}

#[test]
fn oversized_counted_paste_is_rejected_before_allocation() {
    let mut state = VimState::new();
    let mut text = "a".to_owned();
    state.set_register(Yank {
        text: "b".repeat(2_000),
        linewise: false,
        block_width: None,
    });
    keys(&mut state, &mut text, "9999p");
    assert_eq!(text, "a");
    assert!(state.undo.is_empty());
}

#[test]
fn visual_character_paste_of_a_line_register_adds_line_boundaries() {
    let mut state = VimState::new();
    let mut text = "foo bar baz".to_owned();
    state.set_register(Yank {
        text: "qux\n".to_owned(),
        linewise: true,
        block_width: None,
    });
    keys(&mut state, &mut text, "wviwp");
    assert_eq!(text, "foo \nqux\n baz");
    keys(&mut state, &mut text, "u");
    assert_eq!(text, "foo bar baz");
}

#[test]
fn case_operators_transform_unicode_motions_objects_and_lines_without_yanking() {
    for (initial, command, expected) in [
        ("hello WORLD", "gUw", "HELLO WORLD"),
        ("ÄÖ ßeta", "guw", "äö ßeta"),
        ("aBc DEF", "g~iw", "AbC DEF"),
        ("call(äöß)", "gUi(", "call(ÄÖSS)"),
        ("one\n  two\nthree", "2gUU", "ONE\n  TWO\nthree"),
        ("ONE\n  TWO\nTHREE", "gu2j", "one\n  two\nthree"),
        ("aBc\nDeF", "g~~", "AbC\nDeF"),
        ("one TWO three", "gU2w", "ONE TWO three"),
    ] {
        let mut state = VimState::new();
        let mut text = initial.to_owned();
        let register = Yank {
            text: "keep".into(),
            linewise: false,
            block_width: None,
        };
        state.set_register(register.clone());
        state.set_yank_register(Some(register.clone()));
        keys(&mut state, &mut text, command);
        assert_eq!(text, expected, "{command}");
        assert_eq!(state.register(), Some(&register), "{command}");
        assert_eq!(state.yank_register(), Some(&register), "{command}");
        assert_eq!(state.mode, VimMode::Normal);
        assert_eq!(state.undo.len(), 1, "{command}");
        keys(&mut state, &mut text, "u");
        assert_eq!(text, initial, "{command}");
        state.handle_key(&mut text, VimKey::Redo);
        assert_eq!(text, expected, "{command}");
    }
}

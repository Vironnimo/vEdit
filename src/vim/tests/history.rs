use super::*;

#[test]
fn insert_replay_delta_retains_scalar_boundaries_and_cursor_limited_prefix() {
    for (before, after, cursor, start, removed, inserted) in [
        ("α🦀\r\nβ", "α中\r\nβ", 1, 0, 1, "中"),
        ("α🦀", "α🦀🦀", 1, 0, 0, "🦀"),
        ("α\r\nβ", "αβ", 3, -2, 2, ""),
        ("", "中", 0, 0, 0, "中"),
        ("α🦀", "α🦀", 0, 0, 0, ""),
    ] {
        let delta = insert_delta(before, after, cursor);
        assert_eq!(delta.relative_start, start);
        assert_eq!(delta.removed, removed);
        assert_eq!(delta.inserted, inserted);
    }
}

#[test]
fn dot_replays_search_delete_pattern_and_replacement_count_at_new_cursor() {
    let mut state = VimState::new();
    let original = "one foo two foo three foo four foo end";
    let mut text = original.to_owned();
    keys(&mut state, &mut text, "d");
    state.apply_search_motion(&mut text, 4, search_motion("foo", false, 1));
    assert_eq!(text, "foo two foo three foo four foo end");
    keys(&mut state, &mut text, ".");
    assert_eq!(text, "foo three foo four foo end");
    keys(&mut state, &mut text, "u2.");
    assert_eq!(text, "foo four foo end");
    keys(&mut state, &mut text, "u");
    assert_eq!(text, "foo two foo three foo four foo end");
}

#[test]
fn search_change_dot_repeats_insert_and_each_operation_has_one_undo() {
    let mut state = VimState::new();
    let mut text = "one foo two foo end".to_owned();
    keys(&mut state, &mut text, "c");
    state.apply_search_motion(&mut text, 4, search_motion("foo", false, 1));
    assert_eq!(state.mode, VimMode::Insert);
    state.insert_text(&mut text, "X ");
    state.finish_insert(&mut text);
    assert_eq!(text, "X foo two foo end");
    keys(&mut state, &mut text, "w.");
    assert_eq!(text, "X X foo end");
    keys(&mut state, &mut text, "u");
    assert_eq!(text, "X foo two foo end");
    keys(&mut state, &mut text, "u");
    assert_eq!(text, "one foo two foo end");
}

#[test]
fn mouse_navigation_in_insert_keeps_one_undo_transaction() {
    let mut state = VimState::new();
    let mut text = "ab".to_owned();
    keys(&mut state, &mut text, "i");
    state.insert_text(&mut text, "ä");
    state.set_mouse_selection(&text, 3, 1, false);
    assert_eq!(state.mode, VimMode::Insert);
    assert_eq!(state.cursor, 3);
    state.insert_text(&mut text, "🙂");
    state.finish_insert(&mut text);
    assert_eq!(text, "äab🙂");
    assert_eq!(state.undo.len(), 1);
    state.handle_key(&mut text, VimKey::Char('u'));
    assert_eq!(text, "ab");
    state.handle_key(&mut text, VimKey::Redo);
    assert_eq!(text, "äab🙂");
}

#[test]
fn pane_navigation_restores_selection_without_forking_history_or_registers() {
    let mut state = VimState::new();
    let mut text = "ä🙂\n中xy".to_owned();
    state.set_selection_boundaries(&text, 3, 1);
    let first_view = state.navigation();
    let second_view = VimNavigation::default();
    state.restore_navigation(&text, &second_view);
    assert_eq!(state.selection(&text), None);
    keys(&mut state, &mut text, "x");
    assert_eq!(text, "🙂\n中xy");
    let register = state.register().unwrap().clone();
    state.restore_navigation(&text, &first_view);
    assert_eq!(state.selection(&text), Some(1..3));
    assert_eq!(state.cursor, 1);
    assert_eq!(state.register(), Some(&register));
    assert!(state.can_undo());
    // Native undo remains available in a restored visual pane; Vim's
    // visual u now correctly means lowercase the selected characters.
    state.history_edit(&mut text, false);
    assert_eq!(text, "ä🙂\n中xy");
    state.restore_navigation(&text, &second_view);
    assert!(state.can_redo());
    state.handle_key(&mut text, VimKey::Redo);
    assert_eq!(text, "🙂\n中xy");
    keys(&mut state, &mut text, ".");
    assert_eq!(text, "\n中xy");
}

#[test]
fn unicode_offsets_and_redo() {
    let mut state = VimState::new();
    let mut text = "ä🙂中\nß".to_owned();
    assert_eq!(state.mode, VimMode::Normal);
    keys(&mut state, &mut text, "lx");
    assert_eq!(text, "ä中\nß");
    assert_eq!(state.register().unwrap().text, "🙂");
    keys(&mut state, &mut text, "u");
    assert_eq!(text, "ä🙂中\nß");
    assert_eq!(state.cursor, 1);
    state.handle_key(&mut text, VimKey::Redo);
    assert_eq!(text, "ä中\nß");
}

#[test]
fn crlf_insert_and_character_replacement_use_complete_line_breaks() {
    let mut state = VimState::new();
    let mut text = "αβ\r\nγ".to_owned();
    keys(&mut state, &mut text, "i");
    state.handle_key(&mut text, VimKey::Enter);
    assert_eq!(text, "\r\nαβ\r\nγ");
    assert_eq!(state.cursor, 2);
    state.handle_key(&mut text, VimKey::Backspace);
    assert_eq!(text, "αβ\r\nγ");
    assert_eq!(state.cursor, 0);
    state.finish_insert(&mut text);
    keys(&mut state, &mut text, "r");
    state.handle_key(&mut text, VimKey::Enter);
    assert_eq!(text, "\r\nβ\r\nγ");
    assert_eq!(state.cursor, 2);
    keys(&mut state, &mut text, "u");
    assert_eq!(text, "αβ\r\nγ");
}

#[test]
fn change_word_and_insert_are_one_undo_step() {
    let mut state = VimState::new();
    let mut text = "hello world".to_owned();
    keys(&mut state, &mut text, "cw");
    assert_eq!(text, " world");
    state.insert_text(&mut text, "é🙂");
    assert_eq!(state.cursor, 2);
    state.finish_insert(&mut text);
    keys(&mut state, &mut text, "u");
    assert_eq!(text, "hello world");
    state.handle_key(&mut text, VimKey::Redo);
    assert_eq!(text, "é🙂 world");
    state = VimState::new();
    text = "a b c".to_owned();
    keys(&mut state, &mut text, "2cw");
    assert_eq!(text, " c");
}

#[test]
fn redo_survives_noop_commands_but_not_new_edits() {
    let mut state = VimState::new();
    let mut text = "abc".to_owned();
    keys(&mut state, &mut text, "xu0yyi");
    state.finish_insert(&mut text);
    assert!(state.can_redo());
    state.handle_key(&mut text, VimKey::Redo);
    assert_eq!(text, "bc");
    keys(&mut state, &mut text, "ux");
    assert!(!state.can_redo());
}

#[test]
fn open_line_indent_and_external_edits_group() {
    let mut state = VimState::new();
    let mut text = "  one\ntwo".to_owned();
    keys(&mut state, &mut text, "o");
    assert_eq!(text, "  one\n  \ntwo");
    state.insert_text(&mut text, "three");
    state.finish_insert(&mut text);
    keys(&mut state, &mut text, "u");
    assert_eq!(text, "  one\ntwo");
    keys(&mut state, &mut text, "i");
    text.insert_str(0, "external");
    state.notify_external_text_change();
    state.finish_insert(&mut text);
    keys(&mut state, &mut text, "u");
    assert_eq!(text, "  one\ntwo");
}

#[test]
fn pending_commands_cancel_and_history_is_bounded() {
    let mut state = VimState::new();
    let mut text = "one two three".to_owned();
    keys(&mut state, &mut text, "3d");
    state.handle_key(&mut text, VimKey::Escape);
    keys(&mut state, &mut text, "wdx");
    assert_eq!(state.cursor, 4);
    assert_eq!(text, "one two three");
    assert!(state.pending_command().is_empty());
    text = "x".repeat(200);
    state = VimState::new();
    keys(&mut state, &mut text, &"x".repeat(150));
    assert_eq!(state.undo.len(), MAX_HISTORY);
}

#[test]
fn open_line_indents_python_and_preserves_windows_endings() {
    let mut state = VimState::new();
    let mut text = "if ready:\r\n    run()\r\n".to_owned();
    keys(&mut state, &mut text, "o");
    assert_eq!(text, "if ready:\r\n    \r\n    run()\r\n");
    assert_eq!(state.cursor, 15);
    state.insert_text(&mut text, "next()");
    state.finish_insert(&mut text);
    keys(&mut state, &mut text, "u");
    assert_eq!(text, "if ready:\r\n    run()\r\n");
    state.cursor = 11;
    keys(&mut state, &mut text, "O");
    assert_eq!(text, "if ready:\r\n    \r\n    run()\r\n");
    assert_eq!(state.cursor, 15);
}

#[test]
fn block_indentation_preserves_unicode_raw_endings_counts_and_undo() {
    for (tabs, count, right, expected) in [
        (
            false,
            2,
            true,
            "head\r\n        α🙂\r\n\r\n           β\r\n        \tγ\r\ntail",
        ),
        (
            true,
            2,
            true,
            "head\r\n\t\tα🙂\r\n\r\n\t\t   β\r\n\t\t\tγ\r\ntail",
        ),
        (false, 1, false, "head\r\nα🙂\r\n\r\nβ\r\nγ\r\ntail"),
    ] {
        let mut state = VimState::new();
        state.set_indentation(4, tabs);
        let original = "head\r\nα🙂\r\n\r\n   β\r\n\tγ\r\ntail";
        let mut text = original.to_owned();
        assert!(state.shift_lines(&mut text, 1, 4, right, count));
        assert_eq!(text, expected);
        assert_eq!(
            state.cursor,
            6 + if right { if tabs { 2 } else { 8 } } else { 0 }
        );
        assert!(state.history_edit(&mut text, false).text_changed);
        assert_eq!(text, original);
        assert!(state.history_edit(&mut text, true).text_changed);
        assert_eq!(text, expected);
    }
}

#[test]
fn surround_search_dot_resolves_the_pattern_at_the_new_cursor() {
    let mut state = VimState::new();
    let mut text = "a x b x c x".to_owned();
    keys(&mut state, &mut text, "ys");
    state.apply_search_motion(&mut text, 2, search_motion("x", false, 1));
    keys(&mut state, &mut text, "]");
    assert_eq!(text, "[a] x b x c x");
    keys(&mut state, &mut text, "www.");
    assert_eq!(text, "[a] [x b] x c x");
    keys(&mut state, &mut text, "u");
    assert_eq!(text, "[a] x b x c x");
}

#[test]
fn dot_count_replaces_counts_after_surround_and_register_prefixes() {
    let mut state = VimState::new();
    let mut text = "one two three four five six seven".to_owned();
    keys(&mut state, &mut text, "ys2w)");
    assert_eq!(text, "(one two) three four five six seven");
    state.set_cursor(&text, 10);
    keys(&mut state, &mut text, "3.");
    assert_eq!(text, "(one two) (three four five) six seven");

    state = VimState::new();
    text = "abcdef".to_owned();
    keys(&mut state, &mut text, "yl\"02p3.");
    assert_eq!(text, "aaaaaabcdef");
}

#[test]
fn dot_replays_multiline_find_deletes_counts_and_insert_changes() {
    let original = "a\nx b\nx c\nx tail";
    let mut state = VimState::new();
    let mut text = original.to_owned();
    keys(&mut state, &mut text, "dfx");
    assert_eq!(text, " b\nx c\nx tail");
    keys(&mut state, &mut text, ".");
    assert_eq!(text, " c\nx tail");
    keys(&mut state, &mut text, "u");
    assert_eq!(text, " b\nx c\nx tail");
    keys(&mut state, &mut text, "2.");
    assert_eq!(text, " tail");
    keys(&mut state, &mut text, "uu");
    assert_eq!(text, original);
    keys(&mut state, &mut text, "2d1fx");
    assert_eq!(text, " c\nx tail");

    state = VimState::new();
    text = original.to_owned();
    keys(&mut state, &mut text, "cfxZ");
    state.finish_insert(&mut text);
    assert_eq!(text, "Z b\nx c\nx tail");
    keys(&mut state, &mut text, "l.");
    assert_eq!(text, "ZZ c\nx tail");
    keys(&mut state, &mut text, "u");
    assert_eq!(text, "Z b\nx c\nx tail");
    keys(&mut state, &mut text, "u");
    assert_eq!(text, original);
}

#[test]
fn dot_replays_operators_and_replaces_the_original_count() {
    let mut state = VimState::new();
    let mut text = "one two three".to_owned();
    keys(&mut state, &mut text, "dw.");
    assert_eq!(text, "three");
    assert_eq!(state.undo.len(), 2);
    keys(&mut state, &mut text, "u");
    assert_eq!(text, "two three");
    keys(&mut state, &mut text, ".");
    assert_eq!(text, "three");
    text = "abcdefghi".to_owned();
    state = VimState::new();
    keys(&mut state, &mut text, "2x3.");
    assert_eq!(text, "fghi");
    keys(&mut state, &mut text, ".");
    assert_eq!(text, "i");
    text = "one two three four five six seven eight nine ten".to_owned();
    state = VimState::new();
    keys(&mut state, &mut text, "d2w3.");
    assert_eq!(text, "six seven eight nine ten");
}

#[test]
fn windows_history_discards_visual_operator_and_count_prefixes_before_dot() {
    for prefix in ["vll", "V", "d", "3"] {
        for redo in [false, true] {
            let mut state = VimState::new();
            let mut text = "abcdefghi".to_owned();
            keys(&mut state, &mut text, "x");
            if redo {
                keys(&mut state, &mut text, "u");
            }
            keys(&mut state, &mut text, prefix);
            state.history_edit(&mut text, redo);
            assert!(
                state.command_keys.is_empty(),
                "prefix={prefix}, redo={redo}"
            );
            keys(&mut state, &mut text, "x.");
            assert_eq!(text, if redo { "defghi" } else { "cdefghi" });
        }
    }
}

#[test]
fn mouse_visual_changes_repeat_completed_geometry_and_one_undo_step() {
    for (operation, first, repeated) in [("rX", "XXX def", "XXX XXX"), ("cZ", "Z def", "Z Z")] {
        let mut state = VimState::new();
        let mut text = "abc def".to_owned();
        state.set_mouse_selection(&text, 3, 0, false);
        keys(&mut state, &mut text, operation);
        state.finish_insert(&mut text);
        assert_eq!(text, first);
        keys(&mut state, &mut text, "w.");
        assert_eq!(text, repeated, "operation={operation}");
        keys(&mut state, &mut text, "u");
        assert_eq!(text, first);
        state.handle_key(&mut text, VimKey::Redo);
        assert_eq!(text, repeated);
    }

    let mut state = VimState::new();
    let mut text = "a\nb\nc\nd".to_owned();
    state.set_mouse_selection(&text, 4, 0, true);
    keys(&mut state, &mut text, ">jj.");
    assert_eq!(text, "    a\n    b\n    c\n    d");
    keys(&mut state, &mut text, "u");
    assert_eq!(text, "    a\n    b\nc\nd");
}

#[test]
fn dot_replays_external_widget_change_as_one_transaction() {
    let mut state = VimState::new();
    let mut text = "one two three".to_owned();
    keys(&mut state, &mut text, "ciw");
    text.insert_str(0, "é🙂");
    state.set_cursor(&text, 2);
    state.notify_external_text_change();
    state.finish_insert(&mut text);
    keys(&mut state, &mut text, "w.");
    assert_eq!(text, "é🙂 é🙂 three");
    keys(&mut state, &mut text, "u");
    assert_eq!(text, "é🙂 two three");
    state.handle_key(&mut text, VimKey::Redo);
    assert_eq!(text, "é🙂 é🙂 three");
}

#[test]
fn dot_replays_external_multiline_insert_and_adjacent_identical_text() {
    let mut state = VimState::new();
    let mut text = "abc\nxyz".to_owned();
    keys(&mut state, &mut text, "A");
    text.insert_str(3, "!\n  next");
    state.set_cursor(&text, 11);
    state.finish_insert(&mut text);
    keys(&mut state, &mut text, "G.");
    assert_eq!(text, "abc!\n  next\nxyz!\n  next");
    text = "abc".to_owned();
    state = VimState::new();
    keys(&mut state, &mut text, "ia");
    state.finish_insert(&mut text);
    keys(&mut state, &mut text, "$.");
    assert_eq!(text, "aabac");
}

#[test]
fn counted_insert_and_dot_count_repeat_the_original_typed_text() {
    let mut state = VimState::new();
    let mut text = "x".to_owned();
    keys(&mut state, &mut text, "3iab");
    state.finish_insert(&mut text);
    assert_eq!(text, "abababx");
    keys(&mut state, &mut text, "0.");
    assert_eq!(text, "ababababababx");
    keys(&mut state, &mut text, "02.");
    assert_eq!(text, "ababababababababx");
}

#[test]
fn dot_repeats_visual_selection_and_indent() {
    let mut state = VimState::new();
    let mut text = "one\ntwo\nthree\nfour".to_owned();
    keys(&mut state, &mut text, "Vj>jj.");
    assert_eq!(text, "    one\n    two\n    three\n    four");
    keys(&mut state, &mut text, "u");
    assert_eq!(text, "    one\n    two\nthree\nfour");
    text = "abcdefghi".to_owned();
    state = VimState::new();
    keys(&mut state, &mut text, "v2ld.");
    assert_eq!(text, "ghi");
}

#[test]
fn external_normal_edit_is_undoable_and_clears_visual_and_pending_state() {
    let mut state = VimState::new();
    let mut text = "original".to_owned();
    keys(&mut state, &mut text, "v2l");
    assert!(state.selection(&text).is_some());
    state.begin_external_edit(&text);
    text = "replaced".to_owned();
    assert_eq!(state.mode, VimMode::Normal);
    assert!(state.selection(&text).is_none());
    assert!(state.pending_command().is_empty());
    assert!(state.can_undo());
    keys(&mut state, &mut text, "u");
    assert_eq!(text, "original");
    state.handle_key(&mut text, VimKey::Redo);
    assert_eq!(text, "replaced");
    keys(&mut state, &mut text, "u3d");
    state.begin_external_edit(&text);
    text = "another".to_owned();
    assert!(!state.can_redo());
    assert!(state.pending_command().is_empty());
    keys(&mut state, &mut text, "u");
    assert_eq!(text, "original");
}

#[test]
fn external_insert_edit_stays_in_one_insert_undo_transaction() {
    let mut state = VimState::new();
    let mut text = "original".to_owned();
    keys(&mut state, &mut text, "iX");
    state.begin_external_edit(&text);
    text.insert_str(1, "completion");
    state.set_cursor(&text, 11);
    state.notify_external_text_change();
    assert!(state.undo.is_empty());
    state.finish_insert(&mut text);
    assert_eq!(state.undo.len(), 1);
    keys(&mut state, &mut text, "u");
    assert_eq!(text, "original");
    state.handle_key(&mut text, VimKey::Redo);
    assert_eq!(text, "Xcompletionoriginal");
}

#[test]
fn case_operator_dot_replaces_motion_counts_and_noop_preserves_redo() {
    let mut state = VimState::new();
    let mut text = "one two three four five".to_owned();
    keys(&mut state, &mut text, "gU2w2w1.");
    assert_eq!(text, "ONE TWO THREE four five");
    assert_eq!(state.undo.len(), 2);
    keys(&mut state, &mut text, "u");
    assert_eq!(text, "ONE TWO three four five");
    keys(&mut state, &mut text, "0gUiw");
    assert!(state.can_redo());
    state.handle_key(&mut text, VimKey::Redo);
    assert_eq!(text, "ONE TWO THREE four five");
}

#[test]
fn case_search_operator_dot_replays_its_prefix_and_keeps_registers() {
    let mut state = VimState::new();
    let mut text = "one END two END three END".to_owned();
    let register = Yank {
        text: "keep".into(),
        linewise: false,
        block_width: None,
    };
    state.set_register(register.clone());
    keys(&mut state, &mut text, "gU");
    assert!(state.search_operator_pending());
    assert_eq!(state.pending_command(), "gU");
    let action = state.apply_search_motion(&mut text, 4, search_motion("END", false, 1));
    assert!(action.text_changed);
    assert!(action.yanked.is_none());
    assert_eq!(text, "ONE END two END three END");
    keys(&mut state, &mut text, "2w.");
    assert_eq!(text, "ONE END TWO END three END");
    assert_eq!(state.register(), Some(&register));
    assert_eq!(state.undo.len(), 2);
    keys(&mut state, &mut text, "u");
    assert_eq!(text, "ONE END two END three END");
}

#[test]
fn visual_case_and_dot_use_completed_selection_geometry() {
    for command in ["U", "gU"] {
        let mut state = VimState::new();
        let mut text = "äö hello world".to_owned();
        state.set_selection(&text, 0..2);
        keys(&mut state, &mut text, command);
        assert_eq!(text, "ÄÖ hello world", "{command}");
        assert_eq!(state.mode, VimMode::Normal);
        keys(&mut state, &mut text, "w.");
        assert_eq!(text, "ÄÖ HEllo world", "{command}");
        keys(&mut state, &mut text, "u");
        assert_eq!(text, "ÄÖ hello world", "{command}");
    }
    let mut state = VimState::new();
    let mut text = "ONE\nTWO\nTHREE".to_owned();
    keys(&mut state, &mut text, "Vju");
    assert_eq!(text, "one\ntwo\nTHREE");
    assert_eq!(state.undo.len(), 1);
    keys(&mut state, &mut text, "u");
    assert_eq!(text, "ONE\nTWO\nTHREE");
}

#[test]
fn raw_join_dot_and_visual_selection_join_all_selected_lines() {
    let mut state = VimState::new();
    let mut text = "a\n b\nc\n d\ne\n f".to_owned();
    keys(&mut state, &mut text, "gJj.");
    assert_eq!(text, "a b\nc d\ne\n f");
    assert_eq!(state.undo.len(), 2);
    keys(&mut state, &mut text, "u");
    assert_eq!(text, "a b\nc\n d\ne\n f");

    let mut state = VimState::new();
    let mut text = "a\r\n b\r\n c\r\nd\r\n e\r\n f\r\n".to_owned();
    keys(&mut state, &mut text, "V2jgJ");
    assert_eq!(text, "a b c\r\nd\r\n e\r\n f\r\n");
    keys(&mut state, &mut text, "j.");
    assert_eq!(text, "a b c\r\nd e f\r\n");
    assert_eq!(state.undo.len(), 2);
}

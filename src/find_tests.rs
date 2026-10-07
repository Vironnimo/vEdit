use super::*;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

const TEXT_DOCUMENT: &str = "intro 🦀\nfoo first\nbar FOO\nfoo second\nfood is a word\nfoo last\n";

#[test]
fn reusing_whole_word_find_in_substitute_keeps_punctuation_boundaries() {
    for query in ["foo.", "*", "🦀", "foo"] {
        let text = format!("{query} {query}x x{query} ({query})");
        let state = FindState {
            last: Some(Pattern::literal(
                query.into(),
                editor::SearchOptions {
                    case_sensitive: true,
                    whole_word: true,
                },
            )),
            ..Default::default()
        };
        let pattern = state.last_substitute_pattern().unwrap();
        assert_eq!(
            crate::search::search(&text, &pattern, true, false, true).unwrap(),
            crate::search::search(&text, query, true, true, false).unwrap(),
            "query {query:?}"
        );
    }
}

#[test]
fn agent_inserted_crlf_supports_vim_line_end_search_and_editing() {
    let original = "α🙂\r\nβ\r\n";
    let mut app = fixture("");
    let tab = app.tab().unwrap();
    let response = app.control_request(crate::cli::Request::Edit {
        tab_id: tab.id,
        expected_revision: tab.revision,
        edits: Vec::new(),
        set_text: Some(original.into()),
    });
    assert!(response.ok, "{response:?}");
    assert_eq!(app.tab().unwrap().text, original);
    let mut screen = harness(app);
    input(&mut screen, "gg$");
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 1);
    input(&mut screen, "/β$");
    key(&mut screen, Key::Enter);
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 4);
    input(&mut screen, "x");
    assert_eq!(screen.state().tab().unwrap().text, "α🙂\r\n\r\n");
    input(&mut screen, "u");
    assert_eq!(screen.state().tab().unwrap().text, original);
}

fn fixture(text: &str) -> MorrowApp {
    let mut app = MorrowApp::empty(egui::Context::default());
    app.settings.autosave = false;
    app.settings.smooth_scroll = false;
    app.sidebar_visible = false;
    let root = std::env::temp_dir().join("morrow-vim-search-render-only");
    let mut project = Project::new(root.clone());
    project.last_git_refresh = Instant::now();
    let mut tab = EditorTab::new(Some(root.join("search.txt")), "search.txt".into());
    tab.text = text.into();
    tab.saved_text = text.into();
    project.tabs.push(tab);
    app.projects.push(project);
    app.bind_current_pane();
    app
}

fn harness(app: MorrowApp) -> Harness<'static, MorrowApp> {
    let mut screen = Harness::builder()
        .with_size([1200.0, 760.0])
        .with_os(egui::os::OperatingSystem::Windows)
        .build_ui_state(
            |ui, app: &mut MorrowApp| {
                app.ctx = ui.ctx().clone();
                app.last_disk_check = Instant::now();
                app.last_session_save = Instant::now();
                for project in &mut app.projects {
                    project.last_git_refresh = Instant::now();
                    for tab in &mut project.tabs {
                        tab.last_edit = Instant::now();
                    }
                }
                app.draw_ui(ui);
            },
            app,
        );
    set_style(&screen.ctx);
    screen.ctx.all_styles_mut(|style| {
        style.visuals.text_cursor.blink = false;
        style.animation_time = 0.0;
        style.scroll_animation = egui::style::ScrollAnimation::none();
    });
    screen.run_steps(3);
    screen
}

fn input(screen: &mut Harness<'_, MorrowApp>, text: &str) {
    screen.event(egui::Event::Text(text.into()));
    screen.run_steps(2);
}
fn key_event(key: Key, pressed: bool) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}
fn key(screen: &mut Harness<'_, MorrowApp>, key: Key) {
    screen
        .input_mut()
        .events
        .extend([key_event(key, true), key_event(key, false)]);
    screen.run_steps(2);
}
fn matches(text: &str, query: &str) -> Vec<Range<usize>> {
    crate::search::search(text, query, true, false, false).unwrap()
}
fn assert_unedited(screen: &Harness<'_, MorrowApp>, text: &str) {
    let tab = screen.state().tab().unwrap();
    assert_eq!(tab.text, text);
    assert!(!tab.dirty);
    assert!(!tab.vim.can_undo());
}

#[test]
fn slash_incrementally_jumps_confirm_returns_to_editor_and_repeats() {
    let mut screen = harness(fixture(TEXT_DOCUMENT));
    let expected = matches(TEXT_DOCUMENT, "foo");
    input(&mut screen, "/foo");
    assert!(screen.state().vim_search_active());
    assert!(!screen.state().find_visible);
    assert_eq!(screen.state().tab().unwrap().vim.cursor, expected[0].start);
    assert_eq!(
        screen.state().tab().unwrap().selection,
        expected[0].start..expected[0].start
    );
    key(&mut screen, Key::Enter);
    assert!(!screen.state().vim_search_active());
    input(&mut screen, "n");
    assert_eq!(screen.state().tab().unwrap().vim.cursor, expected[1].start);
    input(&mut screen, "N");
    assert_eq!(screen.state().tab().unwrap().vim.cursor, expected[0].start);
    input(&mut screen, "3n");
    assert_eq!(screen.state().tab().unwrap().vim.cursor, expected[3].start);
    assert_unedited(&screen, TEXT_DOCUMENT);
}

#[test]
fn question_mark_establishes_direction_and_empty_search_reuses_pattern() {
    let mut screen = harness(fixture(TEXT_DOCUMENT));
    let expected = matches(TEXT_DOCUMENT, "foo");
    input(&mut screen, "?foo");
    key(&mut screen, Key::Enter);
    assert_eq!(screen.state().tab().unwrap().vim.cursor, expected[3].start);
    input(&mut screen, "n");
    assert_eq!(screen.state().tab().unwrap().vim.cursor, expected[2].start);
    input(&mut screen, "N");
    assert_eq!(screen.state().tab().unwrap().vim.cursor, expected[3].start);
    input(&mut screen, "/");
    key(&mut screen, Key::Enter);
    assert_eq!(screen.state().tab().unwrap().vim.cursor, expected[0].start);
    input(&mut screen, "n");
    assert_eq!(screen.state().tab().unwrap().vim.cursor, expected[1].start);
    assert_unedited(&screen, TEXT_DOCUMENT);
}

#[test]
fn literal_character_finds_and_repeats_leave_n_and_n_uppercase_search_intact() {
    let document = "start\nn N . , / ? * #\nfoo\nn N . , / ? * #\nfoo\n";
    let mut screen = harness(fixture(document));
    let words = matches(document, "foo");
    input(&mut screen, "/foo");
    key(&mut screen, Key::Enter);
    for character in ['n', 'N', '.', ',', '/', '?', '*', '#'] {
        screen
            .state_mut()
            .tab_mut()
            .unwrap()
            .set_cursor(words[0].start..words[0].start);
        screen.run_steps(2);
        let hits = matches(document, &character.to_string());
        input(&mut screen, &format!("f{character}"));
        assert!(!screen.state().vim_search_active(), "f{character}");
        assert_eq!(
            screen.state().tab().unwrap().vim.cursor,
            hits[1].start,
            "f{character}"
        );
        input(&mut screen, ",");
        assert_eq!(
            screen.state().tab().unwrap().vim.cursor,
            hits[0].start,
            "f{character},"
        );
        input(&mut screen, ";");
        assert_eq!(
            screen.state().tab().unwrap().vim.cursor,
            hits[1].start,
            "f{character},;"
        );
        input(&mut screen, "n");
        assert_eq!(screen.state().tab().unwrap().vim.cursor, words[1].start);
        input(&mut screen, "N");
        assert_eq!(screen.state().tab().unwrap().vim.cursor, words[0].start);
    }
    input(&mut screen, "N");
    assert_eq!(screen.state().tab().unwrap().vim.cursor, words[1].start);
    input(&mut screen, "n");
    assert_eq!(screen.state().tab().unwrap().vim.cursor, words[0].start);
    assert_unedited(&screen, document);
}

#[test]
fn counted_search_repeats_extend_visual_selection_in_both_directions() {
    let mut screen = harness(fixture(TEXT_DOCUMENT));
    let hits = matches(TEXT_DOCUMENT, "foo");
    input(&mut screen, "/foo");
    key(&mut screen, Key::Enter);
    for (command, target) in [("v2n", 2), ("N", 1), ("n", 2), ("2N", 0)] {
        input(&mut screen, command);
        let tab = screen.state().tab().unwrap();
        assert_eq!(tab.vim.cursor, hits[target].start, "{command}");
        assert_eq!(
            tab.selection,
            hits[0].start..hits[target].start + 1,
            "{command}"
        );
        assert_eq!(tab.vim.mode, VimMode::Visual);
    }
    assert_unedited(&screen, TEXT_DOCUMENT);
}

#[test]
fn search_enter_and_next_in_one_frame_preserve_event_order() {
    let mut screen = harness(fixture(TEXT_DOCUMENT));
    screen.input_mut().events.extend([
        egui::Event::Text("/foo".into()),
        key_event(Key::Enter, true),
        key_event(Key::Enter, false),
        egui::Event::Text("n".into()),
    ]);
    screen.run_steps(2);
    assert_eq!(
        screen.state().tab().unwrap().vim.cursor,
        matches(TEXT_DOCUMENT, "foo")[1].start
    );
    assert!(!screen.state().vim_search_active());
    assert_unedited(&screen, TEXT_DOCUMENT);
}

#[test]
fn visual_search_extends_anchor_and_escape_restores_original_range() {
    let mut screen = harness(fixture(TEXT_DOCUMENT));
    input(&mut screen, "Vj");
    let original_cursor = screen.state().tab().unwrap().vim.cursor;
    let original_selection = screen.state().tab().unwrap().selection.clone();
    input(&mut screen, "/foo");
    assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::VisualLine);
    assert!(screen.state().tab().unwrap().selection.end > original_selection.end);
    key(&mut screen, Key::Escape);
    assert_eq!(screen.state().tab().unwrap().vim.cursor, original_cursor);
    assert_eq!(screen.state().tab().unwrap().selection, original_selection);
    assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::VisualLine);
    input(&mut screen, "/foo");
    key(&mut screen, Key::Enter);
    assert_eq!(
        screen.state().tab().unwrap().selection.start,
        original_selection.start
    );
    assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::VisualLine);
    assert_unedited(&screen, TEXT_DOCUMENT);
}

#[test]
fn invalid_and_missing_patterns_keep_cursor_and_show_feedback() {
    let mut screen = harness(fixture(TEXT_DOCUMENT));
    input(&mut screen, "/[");
    assert!(screen.state().find.prompt.as_ref().unwrap().error.is_some());
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 0);
    key(&mut screen, Key::Escape);
    input(&mut screen, "/missing-pattern");
    assert_eq!(
        screen
            .state()
            .find
            .prompt
            .as_ref()
            .unwrap()
            .error
            .as_deref(),
        Some("Pattern not found")
    );
    key(&mut screen, Key::Enter);
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 0);
    assert!(screen.state().status.as_ref().unwrap().1);
    assert_unedited(&screen, TEXT_DOCUMENT);
}

#[test]
fn star_and_hash_search_whole_words_and_retain_repeat_direction() {
    let mut screen = harness(fixture(TEXT_DOCUMENT));
    let words = crate::search::search(TEXT_DOCUMENT, "foo", true, true, false).unwrap();
    screen
        .state_mut()
        .tab_mut()
        .unwrap()
        .set_cursor(words[0].start + 1..words[0].start + 1);
    screen.run_steps(2);
    input(&mut screen, "*");
    assert_eq!(screen.state().tab().unwrap().vim.cursor, words[1].start);
    input(&mut screen, "n");
    assert_eq!(screen.state().tab().unwrap().vim.cursor, words[2].start);
    input(&mut screen, "#n");
    assert_eq!(screen.state().tab().unwrap().vim.cursor, words[0].start);
    input(&mut screen, "N");
    assert_eq!(screen.state().tab().unwrap().vim.cursor, words[1].start);
    assert_unedited(&screen, TEXT_DOCUMENT);
}

#[test]
fn anchors_and_newlines_repeat_from_logical_matches_not_clamped_carets() {
    for query in ["$", r"\n"] {
        let mut screen = harness(fixture("one\ntwo\n"));
        input(&mut screen, &format!("/{query}"));
        key(&mut screen, Key::Enter);
        assert_eq!(screen.state().tab().unwrap().vim.cursor, 2);
        input(&mut screen, "n");
        assert_eq!(screen.state().tab().unwrap().vim.cursor, 6, "{query}");
        input(&mut screen, "N");
        assert_eq!(screen.state().tab().unwrap().vim.cursor, 2, "{query}");
        // l is a real motion even when the Normal caret cannot advance.
        input(&mut screen, "ln");
        assert_eq!(screen.state().tab().unwrap().vim.cursor, 2, "{query}");
        input(&mut screen, "n");
        assert_eq!(screen.state().tab().unwrap().vim.cursor, 6, "{query}");
        assert_unedited(&screen, "one\ntwo\n");
    }
}

#[test]
fn counted_anchor_repeat_keeps_match_identity_through_zero_count_digits() {
    let document = "one\ntwo\nthree\n";
    let mut screen = harness(fixture(document));
    input(&mut screen, r"/\n");
    key(&mut screen, Key::Enter);
    input(&mut screen, "20n");
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 12);
    assert_unedited(&screen, document);
}

#[test]
fn repeated_end_anchor_prompt_previews_the_same_logical_hit_it_commits() {
    let mut screen = harness(fixture("one\ntwo\n"));
    input(&mut screen, "/$");
    key(&mut screen, Key::Enter);
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 2);
    input(&mut screen, "/$");
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 6);
    key(&mut screen, Key::Enter);
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 6);
    input(&mut screen, "n");
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 8);
    input(&mut screen, "n");
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 2);
    assert_unedited(&screen, "one\ntwo\n");
}

#[test]
fn star_and_hash_include_combining_marks_and_connector_punctuation() {
    for word in ["cafe\u{301}", "one\u{203f}two"] {
        let document = format!("{word} {word}");
        let mut screen = harness(fixture(&document));
        input(&mut screen, "*");
        assert_eq!(
            screen.state().tab().unwrap().vim.cursor,
            word.chars().count() + 1
        );
        input(&mut screen, "#");
        assert_eq!(screen.state().tab().unwrap().vim.cursor, 0);
        assert_unedited(&screen, &document);
    }
}

#[test]
fn operator_search_preview_is_read_only_and_enter_commits_one_undo_change() {
    let document = "one foo two foo three";
    let mut screen = harness(fixture(document));
    input(&mut screen, "d/foo");
    assert_unedited(&screen, document);
    key(&mut screen, Key::Enter);
    assert_eq!(screen.state().tab().unwrap().text, "foo two foo three");
    assert_eq!(
        screen.state().tab().unwrap().vim.register().unwrap().text,
        "one "
    );
    input(&mut screen, "u");
    assert_eq!(screen.state().tab().unwrap().text, document);
    input(&mut screen, "d/foo");
    key(&mut screen, Key::Escape);
    assert_eq!(screen.state().tab().unwrap().vim.pending_command(), "");
    input(&mut screen, "w");
    assert_eq!(screen.state().tab().unwrap().text, document);
}

#[test]
fn repeat_search_and_backwards_change_use_pending_operators() {
    let document = "one foo two foo three";
    let mut screen = harness(fixture(document));
    input(&mut screen, "/foo");
    key(&mut screen, Key::Enter);
    input(&mut screen, "0dn");
    assert_eq!(screen.state().tab().unwrap().text, "foo two foo three");
    input(&mut screen, "u$");
    input(&mut screen, "c?foo");
    let before = screen.state().tab().unwrap().text.clone();
    key(&mut screen, Key::Enter);
    assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::Insert);
    assert_eq!(screen.state().tab().unwrap().text, "one foo two e");
    input(&mut screen, "replacement");
    key(&mut screen, Key::Escape);
    input(&mut screen, "u");
    assert_eq!(screen.state().tab().unwrap().text, before);
}

#[test]
fn search_operator_counts_multiply_and_failed_search_cancels_operator() {
    let document = "x foo x foo x foo x foo x foo x foo end";
    let mut screen = harness(fixture(document));
    input(&mut screen, "2d3/foo");
    assert_unedited(&screen, document);
    key(&mut screen, Key::Enter);
    assert_eq!(screen.state().tab().unwrap().text, "foo end");
    input(&mut screen, "u");
    input(&mut screen, "d/no-such-pattern");
    key(&mut screen, Key::Enter);
    assert_eq!(screen.state().tab().unwrap().vim.pending_command(), "");
    input(&mut screen, "w");
    assert_eq!(screen.state().tab().unwrap().text, document);
}

#[test]
fn first_f3_after_vim_search_advances_to_next_hit() {
    let mut screen = harness(fixture("intro foo one foo two"));
    input(&mut screen, "/foo");
    key(&mut screen, Key::Enter);
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 6);
    key(&mut screen, Key::F3);
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 14);
}

#[test]
fn search_operator_register_follows_native_event_order() {
    let mut screen = harness(fixture("one foo two"));
    input(&mut screen, "d/foo");
    screen.input_mut().events.extend([
        key_event(Key::Enter, true),
        key_event(Key::Enter, false),
        egui::Event::Text("x".into()),
    ]);
    screen.run_steps(2);
    assert_eq!(screen.state().tab().unwrap().text, "oo two");
    assert_eq!(screen.state().register.as_ref().unwrap().text, "f");
}

#[test]
fn native_copy_cancels_pending_search_operator_and_restores_visual_selection() {
    let document = "one foo two foo";
    let mut screen = harness(fixture(document));
    input(&mut screen, "d/foo");
    screen.event(egui::Event::Copy);
    screen.run_steps(2);
    assert!(!screen.state().vim_search_active());
    assert_eq!(screen.state().tab().unwrap().vim.pending_command(), "");
    input(&mut screen, "w");
    assert_unedited(&screen, document);
    input(&mut screen, "vll");
    let selection = screen.state().tab().unwrap().selection.clone();
    input(&mut screen, "/foo");
    screen.event(egui::Event::Copy);
    screen.run_steps(2);
    assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::Visual);
    assert_eq!(screen.state().tab().unwrap().selection, selection);
    assert_unedited(&screen, document);
}

#[test]
fn first_find_after_pane_switch_refreshes_logical_match_context() {
    let mut app = fixture("one\ntwo\n");
    app.find.last = Some(Pattern::vim("$".into()));
    app.find.current_pane = app.current_pane_id();
    app.split_active(crate::cli::SplitOrientation::Vertical)
        .unwrap();
    let mut screen = harness(app);
    // find_next is called by shortcuts before handle_vim validates the pane.
    screen.state_mut().find.current_pane = 0;
    screen.state_mut().find_next(false);
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 2);
    screen.state_mut().validate_vim_search();
    screen.state_mut().find_next(false);
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 6);
}

#[test]
fn long_query_viewport_tracks_end_home_and_bounded_bulk_text() {
    let mut screen = harness(fixture("unchanged"));
    input(&mut screen, &format!("/{}tail", "a".repeat(5000)));
    let prompt = screen.state().find.prompt.as_ref().unwrap();
    assert_eq!(prompt.query.chars().count(), MAX_QUERY_CHARS);
    assert!(prompt.horizontal_scroll > 1000.0);
    key(&mut screen, Key::Home);
    assert_eq!(
        screen
            .state()
            .find
            .prompt
            .as_ref()
            .unwrap()
            .horizontal_scroll,
        0.0
    );
    key(&mut screen, Key::End);
    assert!(
        screen
            .state()
            .find
            .prompt
            .as_ref()
            .unwrap()
            .horizontal_scroll
            > 1000.0
    );
    assert_unedited(&screen, "unchanged");
    let offset = prompt_scroll(0.0, 1000.0, 200.0, 1100.0);
    assert!(1000.0 - offset < 200.0);
    assert_eq!(prompt_scroll(offset, 0.0, 200.0, 1100.0), 0.0);
}

#[test]
fn nohlsearch_hides_all_marks_but_next_restores_the_saved_pattern() {
    let mut screen = harness(fixture(TEXT_DOCUMENT));
    input(&mut screen, "/foo");
    key(&mut screen, Key::Enter);
    assert_eq!(screen.state_mut().editor_search_matches().len(), 4);
    input(&mut screen, ":noh");
    key(&mut screen, Key::Enter);
    assert!(screen.state_mut().editor_search_matches().is_empty());
    input(&mut screen, "n");
    assert_eq!(screen.state_mut().editor_search_matches().len(), 4);
    assert_eq!(
        screen.state().tab().unwrap().vim.cursor,
        matches(TEXT_DOCUMENT, "foo")[1].start
    );
    assert_unedited(&screen, TEXT_DOCUMENT);
}

#[test]
fn regex_unicode_case_override_and_all_hit_highlights_use_character_ranges() {
    let document = "🦀 intro\nÄpfel first\näpfel second\nÄpfel third\n";
    let mut screen = harness(fixture(document));
    input(&mut screen, "/äpfel\\c");
    let expected = crate::search::search(document, "äpfel\\c", true, false, true).unwrap();
    assert_eq!(
        screen.state_mut().editor_search_matches().as_ref(),
        expected
    );
    assert_eq!(screen.state().tab().unwrap().vim.cursor, expected[0].start);
    let backgrounds: Vec<_> = screen
        .output()
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Rect(rect)
                if rect.fill == Color32::from_rgba_unmultiplied(237, 191, 126, 85)
                    || rect.fill == Color32::from_rgba_unmultiplied(147, 174, 255, 38) =>
            {
                Some(rect)
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        backgrounds.len(),
        3,
        "All visible matches have a background, with one current hit"
    );
    assert_unedited(&screen, document);
}

#[test]
fn search_history_recall_keeps_the_draft_caret_and_cancelled_queries_out() {
    let mut screen = harness(fixture(TEXT_DOCUMENT));
    input(&mut screen, "/foo");
    key(&mut screen, Key::Enter);
    input(&mut screen, "?bar");
    key(&mut screen, Key::Enter);
    input(&mut screen, "/unused");
    key(&mut screen, Key::Escape);
    input(&mut screen, "/🦀draft");
    key(&mut screen, Key::ArrowLeft);
    let draft_cursor = screen.state().find.prompt.as_ref().unwrap().query_cursor;
    key(&mut screen, Key::ArrowUp);
    assert_eq!(screen.state().find.prompt.as_ref().unwrap().query, "bar");
    key(&mut screen, Key::ArrowUp);
    assert_eq!(screen.state().find.prompt.as_ref().unwrap().query, "foo");
    key(&mut screen, Key::ArrowDown);
    key(&mut screen, Key::ArrowDown);
    let prompt = screen.state().find.prompt.as_ref().unwrap();
    assert_eq!(prompt.query, "🦀draft");
    assert_eq!(prompt.query_cursor, draft_cursor);
    key(&mut screen, Key::ArrowUp);
    key(&mut screen, Key::Enter);
    assert_eq!(screen.state().find.history, ["foo", "bar"]);
    assert_unedited(&screen, TEXT_DOCUMENT);
}

#[test]
fn recalled_search_can_be_edited_without_destroying_previous_history() {
    let mut state = FindState::default();
    let mut tab = EditorTab::new(None, "history".into());
    tab.text = "foo bar".into();
    for query in ["foo", "bar", "foo"] {
        state.start(&mut tab, 1, false);
        state.text(&mut tab, query);
        state.key(&mut tab, Key::Enter, egui::Modifiers::NONE);
    }
    assert_eq!(state.history, ["bar", "foo"]);
    state.start(&mut tab, 1, false);
    state.key(&mut tab, Key::P, egui::Modifiers::CTRL);
    state.text(&mut tab, "tail");
    state.key(&mut tab, Key::P, egui::Modifiers::CTRL);
    assert_eq!(state.prompt.as_ref().unwrap().query, "foo");
    state.key(&mut tab, Key::N, egui::Modifiers::CTRL);
    assert_eq!(state.prompt.as_ref().unwrap().query, "footail");
    assert_eq!(state.history, ["bar", "foo"]);
    assert!(!tab.vim.can_undo());
}

#[test]
fn search_history_is_bounded_and_empty_confirmation_keeps_it_unchanged() {
    let mut state = FindState::default();
    let mut tab = EditorTab::new(None, "history".into());
    for number in 0..MAX_SEARCH_HISTORY + 4 {
        state.start(&mut tab, 1, false);
        state.text(&mut tab, &number.to_string());
        state.key(&mut tab, Key::Enter, egui::Modifiers::NONE);
    }
    assert_eq!(state.history.len(), MAX_SEARCH_HISTORY);
    assert_eq!(state.history.first().unwrap(), "4");
    let history = state.history.clone();
    state.start(&mut tab, 1, true);
    state.key(&mut tab, Key::Enter, egui::Modifiers::NONE);
    assert_eq!(state.history, history);
}

#[test]
fn search_word_editing_and_ctrl_u_preserve_unicode_suffix_at_the_caret() {
    let mut state = FindState::default();
    let mut tab = EditorTab::new(None, "editing".into());
    state.start(&mut tab, 1, false);
    state.text(&mut tab, "Äpfel βeta.* 🦀tail");
    state.key(&mut tab, Key::ArrowLeft, egui::Modifiers::CTRL);
    assert_eq!(state.prompt.as_ref().unwrap().query_cursor, 14);
    state.key(&mut tab, Key::ArrowLeft, egui::Modifiers::CTRL);
    assert_eq!(state.prompt.as_ref().unwrap().query_cursor, 13);
    state.key(&mut tab, Key::U, egui::Modifiers::CTRL);
    assert_eq!(state.prompt.as_ref().unwrap().query, "🦀tail");
    assert_eq!(state.prompt.as_ref().unwrap().query_cursor, 0);
    state.key(&mut tab, Key::Delete, egui::Modifiers::CTRL);
    assert_eq!(state.prompt.as_ref().unwrap().query, "tail");
    state.key(&mut tab, Key::E, egui::Modifiers::CTRL);
    state.key(&mut tab, Key::W, egui::Modifiers::CTRL);
    assert_eq!(state.prompt.as_ref().unwrap().query, "");
    state.text(&mut tab, "Äpfel βeta.*");
    state.key(&mut tab, Key::B, egui::Modifiers::CTRL);
    assert_eq!(state.prompt.as_ref().unwrap().query_cursor, 0);
    state.key(&mut tab, Key::E, egui::Modifiers::CTRL);
    state.key(&mut tab, Key::Backspace, egui::Modifiers::CTRL);
    assert_eq!(state.prompt.as_ref().unwrap().query, "Äpfel βeta");
    state.key(&mut tab, Key::W, egui::Modifiers::CTRL);
    assert_eq!(state.prompt.as_ref().unwrap().query, "Äpfel ");
    assert!(!tab.vim.can_undo());
}

#[test]
fn toolbar_shift_enter_moves_backwards_and_preserves_query_focus() {
    let mut app = fixture(TEXT_DOCUMENT);
    app.find_visible = true;
    app.find_focus = true;
    app.find_text = "foo".into();
    app.find_case = true;
    app.find_next(false);
    app.find_next(false);
    let mut screen = harness(app);
    let focused = screen.ctx.memory(|memory| memory.focused());
    assert!(focused.is_some());
    screen
        .input_mut()
        .events
        .extend([true, false].map(|pressed| egui::Event::Key {
            key: Key::Enter,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::SHIFT,
        }));
    screen.run_steps(2);
    assert_eq!(
        screen.state().tab().unwrap().vim.cursor,
        matches(TEXT_DOCUMENT, "foo")[0].start
    );
    assert_eq!(screen.ctx.memory(|memory| memory.focused()), focused);
    assert_unedited(&screen, TEXT_DOCUMENT);
}

#[test]
fn replacing_adjacent_unicode_matches_steps_forward_and_undo_restores_one_change() {
    let document = "🦀äää end";
    let mut app = fixture(document);
    app.find_visible = true;
    app.replace_visible = true;
    app.find_focus = true;
    app.find_text = "ä".into();
    app.replace_text.clear();
    app.find_next(false);
    let mut screen = harness(app);
    screen.get_by_label("Replace").click();
    screen.run_steps(2);
    assert_eq!(screen.state().tab().unwrap().text, "🦀ää end");
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 1);
    assert_eq!(screen.state().find_match, Some(1));
    screen.get_by_label("Close find").click();
    screen.run_steps(2);
    input(&mut screen, "u");
    assert_eq!(screen.state().tab().unwrap().text, document);
}

#[test]
fn repeated_idle_frames_reuse_matches_then_edits_invalidate_cache() {
    let mut screen = harness(fixture(TEXT_DOCUMENT));
    input(&mut screen, "/foo");
    key(&mut screen, Key::Enter);
    let first = screen.state_mut().editor_search_matches();
    screen.run_steps(12);
    let second = screen.state_mut().editor_search_matches();
    assert!(Arc::ptr_eq(&first, &second));
    let tab = screen.state_mut().tab_mut().unwrap();
    tab.text.push_str("foo extra\n");
    tab.changed();
    let updated = screen.state_mut().editor_search_matches();
    assert!(!Arc::ptr_eq(&first, &updated));
    assert_eq!(updated.len(), first.len() + 1);
}

#[test]
fn escape_restores_scroll_after_incremental_jump_to_far_result() {
    let document = format!("{}needle\n", "ordinary line\n".repeat(300));
    let mut screen = harness(fixture(&document));
    let original = screen.state().find.view_scroll.values().next().unwrap().1;
    input(&mut screen, "/needle");
    assert!(screen.state().find.view_scroll.values().next().unwrap().1.y > 0.0);
    key(&mut screen, Key::Escape);
    assert_eq!(
        screen.state().find.view_scroll.values().next().unwrap().1,
        original
    );
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 0);
    assert_unedited(&screen, &document);
}

#[test]
fn search_target_in_wrapped_line_and_focused_split_stays_visible() {
    let document = format!("intro\n{}needle end\n", "wrapped words ".repeat(170));
    let mut app = fixture(&document);
    app.settings.word_wrap = true;
    let mut screen = harness(app);
    let right = screen
        .state_mut()
        .split_active(crate::cli::SplitOrientation::Vertical)
        .unwrap();
    screen.run_steps(2);
    input(&mut screen, "/needle");
    key(&mut screen, Key::Enter);
    assert_eq!(screen.state().current_pane_id(), right);
    assert_eq!(
        screen.state().tab().unwrap().vim.cursor,
        matches(&document, "needle")[0].start
    );
    let caret = screen.state().editor_cursor_rect.unwrap();
    let bounds = screen.state().editor_rect.unwrap();
    assert!(caret.center().y >= bounds.top() && caret.center().y <= bounds.bottom());
    assert_unedited(&screen, &document);
}

#[test]
fn changing_tabs_cancels_draft_without_moving_original_or_new_tab() {
    let mut app = fixture(TEXT_DOCUMENT);
    let mut second = EditorTab::new(None, "other.txt".into());
    second.text = "another foo".into();
    second.saved_text = second.text.clone();
    app.projects[0].tabs.push(second);
    let mut screen = harness(app);
    input(&mut screen, "/foo");
    screen.state_mut().activate_tab(0, 1);
    screen.run_steps(2);
    assert!(!screen.state().vim_search_active());
    assert_eq!(screen.state().projects[0].tabs[0].vim.cursor, 0);
    assert_eq!(screen.state().projects[0].tabs[1].vim.cursor, 0);
    assert_eq!(screen.state().projects[0].tabs[0].text, TEXT_DOCUMENT);
}

#[test]
fn vim_search_prompt_and_highlights_snapshot() {
    let mut app = MorrowApp::empty(egui::Context::default());
    super::super::visual_tests::workspace(&mut app);
    let mut screen = super::super::visual_tests::harness(app);
    input(&mut screen, "/Path\\w*");
    super::super::visual_tests::snapshot(&mut screen, "morrow_vim_search");
}

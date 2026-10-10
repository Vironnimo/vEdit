use super::*;

fn fixture() -> VEditApp {
    let mut app = VEditApp::empty(egui::Context::default());
    app.settings.vim = true;
    app.settings.autosave = false;
    let mut project = Project::new(std::env::temp_dir().join("vedit-find-async-unit"));
    project.last_git_refresh = Instant::now();
    let mut tab = EditorTab::new(None, "search.txt".into());
    tab.text = format!("start foo middle foo {}", "a".repeat(ASYNC_DOCUMENT_BYTES));
    tab.saved_text = tab.text.clone();
    project.tabs.push(tab);
    app.projects.push(project);
    app.bind_current_pane();
    app.find.worker = Some(worker::Worker::manual());
    app.find.context = Some(app.ctx.clone());
    app.find.current_pane = app.current_pane_id();
    app
}

fn begin(app: &mut VEditApp, operator: Option<char>) {
    let pane = app.current_pane_id();
    let mut state = std::mem::take(&mut app.find);
    let tab = app.tab_mut().unwrap();
    if let Some(operator) = operator {
        tab.vim.handle_key(&mut tab.text, VimKey::Char(operator));
    }
    state.start(tab, pane, false);
    state.text(tab, "foo");
    state.key(tab, Key::Enter, egui::Modifiers::NONE);
    assert!(state.waiting());
    assert_eq!(tab.vim.cursor, 0);
    app.find = state;
}

fn finish(app: &VEditApp) {
    let tab = app.tab().unwrap();
    let matches = crate::search::search(&tab.text, "foo", true, false, true).unwrap();
    app.find.worker.as_ref().unwrap().finish(tab.id, matches);
}

#[test]
fn enter_before_ready_preserves_operator_and_publishes_yank_before_next_input() {
    let mut app = fixture();
    begin(&mut app, Some('d'));
    assert!(app.tab().unwrap().text.starts_with("start foo"));
    finish(&app);
    app.validate_vim_search();
    assert!(!app.vim_search_active());
    assert!(app.tab().unwrap().text.starts_with("foo middle foo"));
    assert_eq!(app.register.as_ref().unwrap().text, "start ");
    assert!(app.tab().unwrap().vim.can_undo());
}

#[test]
fn native_copy_cancels_ready_operator_before_it_can_modify_text() {
    let mut app = fixture();
    let original = app.tab().unwrap().text.clone();
    begin(&mut app, Some('d'));
    finish(&app);
    app.ctx
        .input_mut(|input| input.events.push(egui::Event::Copy));
    app.validate_vim_search();
    assert_eq!(app.tab().unwrap().text, original);
    assert!(!app.vim_search_active());
    assert!(!app.tab().unwrap().vim.search_operator_pending());
    assert!(app.find.deferred_events.is_empty());
    assert_eq!(app.status.as_ref().unwrap().0, "Search cancelled");
}

#[test]
fn selected_native_match_steps_strictly_and_changed_query_includes_cursor() {
    let mut app = fixture();
    let tab = app.tab_mut().unwrap();
    tab.text = "one one two two".into();
    tab.saved_text = tab.text.clone();
    tab.set_cursor(4..7);
    app.find_visible = true;
    app.find_text = "one".into();
    app.find_match = Some(4);
    app.find_next(true);
    assert_eq!(app.find_match, Some(0));
    assert_eq!(app.tab().unwrap().selection, 0..3);
    app.find_next(false);
    assert_eq!(app.find_match, Some(4));
    app.tab_mut().unwrap().set_cursor(8..11);
    app.find_text = "two".into();
    app.find_next(false);
    assert_eq!(app.find_match, Some(8));
    assert_eq!(app.tab().unwrap().selection, 8..11);
}

#[test]
fn completion_waits_behind_modal_and_cancels_after_document_or_pane_change() {
    let mut app = fixture();
    begin(&mut app, Some('d'));
    finish(&app);
    app.settings_open = true;
    app.validate_vim_search();
    assert!(app.tab().unwrap().text.starts_with("start foo"));
    assert!(app.vim_search_active());
    app.settings_open = false;
    app.find.prompt.as_mut().unwrap().pane_id += 1;
    app.validate_vim_search();
    assert!(!app.vim_search_active());
    assert!(app.tab().unwrap().text.starts_with("start foo"));

    begin(&mut app, Some('d'));
    finish(&app);
    app.tab_mut().unwrap().revision += 1;
    app.validate_vim_search();
    assert!(!app.vim_search_active());
    assert!(app.tab().unwrap().text.starts_with("start foo"));
}

#[test]
fn enter_followed_by_repeat_replays_before_new_frame_input() {
    let mut app = fixture();
    begin(&mut app, None);
    let mut state = std::mem::take(&mut app.find);
    state.text(app.tab_mut().unwrap(), "n");
    assert_eq!(state.deferred_events, vec![egui::Event::Text("n".into())]);
    app.find = state;
    finish(&app);
    app.ctx
        .input_mut(|input| input.events.push(egui::Event::Text("j".into())));
    app.validate_vim_search();
    assert_eq!(app.tab().unwrap().vim.cursor, 6);
    app.ctx.input(|input| {
        assert_eq!(
            input.events,
            vec![egui::Event::Text("n".into()), egui::Event::Text("j".into())]
        )
    });
    let mut state = std::mem::take(&mut app.find);
    state.repeat(app.tab_mut().unwrap(), false);
    assert_eq!(app.tab().unwrap().vim.cursor, 17);
}

#[test]
fn repeated_navigation_is_cancelled_after_manual_motion() {
    let mut app = fixture();
    app.find.last = Some(Pattern::vim("foo".into()));
    let mut state = std::mem::take(&mut app.find);
    state.repeat(app.tab_mut().unwrap(), false);
    assert!(state.waiting());
    app.find = state;
    finish(&app);
    let tab = app.tab_mut().unwrap();
    tab.vim.handle_key(&mut tab.text, VimKey::Char('l'));
    tab.selection = tab.vim.cursor..tab.vim.cursor;
    app.validate_vim_search();
    assert_eq!(app.tab().unwrap().vim.cursor, 1);
    assert!(!app.vim_search_active());
}

#[test]
fn displayed_results_survive_history_eviction_without_starting_more_jobs() {
    let mut app = fixture();
    app.find_visible = true;
    app.find_text = "foo".into();
    assert!(app.editor_search_matches().is_empty());
    finish(&app);
    let ready = app.editor_search_matches();
    assert_eq!(&*ready, &[6..9, 17..20]);
    let id = app.tab().unwrap().id;
    app.find.cache.invalidate(id);
    let again = app.editor_search_matches();
    assert!(Arc::ptr_eq(&ready, &again));
    assert!(!app.find_search_running());
}

#[test]
fn clearing_toolbar_query_cancels_old_pending_navigation() {
    let mut app = fixture();
    app.find_visible = true;
    app.find_text = "foo".into();
    app.find_next(false);
    assert!(app.find.pending_navigation.is_some());
    assert!(!app.find.waiting());
    finish(&app);
    app.find_text.clear();
    app.validate_vim_search();
    assert!(!app.find.waiting());
    assert!(!app.find_search_running());
    assert_eq!(app.tab().unwrap().vim.cursor, 0);
}

#[test]
fn toolbar_pending_search_keeps_globals_available_and_preserves_query_focus() {
    let mut app = fixture();
    app.find_visible = true;
    app.find_text = "foo".into();
    let tab = app.tab_mut().unwrap();
    tab.vim.handle_key(&mut tab.text, VimKey::Char('d'));
    app.find_next(false);
    assert!(!app.vim_search_active());
    assert!(!app.tab().unwrap().vim.search_operator_pending());
    app.ctx.input_mut(|input| {
        input.events.push(egui::Event::Key {
            key: Key::Comma,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        })
    });
    app.shortcuts();
    assert!(app.settings_open);
    finish(&app);
    app.settings_open = false;
    app.validate_vim_search();
    assert_eq!(app.tab().unwrap().selection, 6..9);
    assert!(!app.tab().unwrap().focus);
    assert!(app.tab().unwrap().text.starts_with("start foo"));
}

#[test]
fn native_toolbar_cancel_keeps_copy_and_escape_for_focused_controls() {
    for event in [
        egui::Event::Copy,
        egui::Event::Key {
            key: Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        },
    ] {
        let mut app = fixture();
        app.find_visible = true;
        app.find_text = "foo".into();
        app.find_next(false);
        finish(&app);
        app.ctx.input_mut(|input| input.events.push(event.clone()));
        app.validate_vim_search();
        assert!(app.find.pending_navigation.is_none());
        app.ctx.input(|input| assert_eq!(input.events, vec![event]));
        assert_eq!(app.tab().unwrap().vim.cursor, 0);
    }
}

#[test]
fn queued_global_shortcut_reaches_full_draw_dispatch_after_completion() {
    let mut app = fixture();
    app.settings.smooth_scroll = false;
    app.sidebar_visible = false;
    let mut screen = egui_kittest::Harness::builder()
        .with_size([900.0, 500.0])
        .with_os(egui::os::OperatingSystem::Windows)
        .build_ui_state(
            |ui, app: &mut VEditApp| {
                app.ctx = ui.ctx().clone();
                app.last_disk_check = Instant::now();
                app.last_session_save = Instant::now();
                app.draw_ui(ui);
            },
            app,
        );
    set_style(&screen.ctx);
    screen.run_steps(2);
    begin(screen.state_mut(), None);
    let app = screen.state_mut();
    let mut state = std::mem::take(&mut app.find);
    state.key(app.tab_mut().unwrap(), Key::Comma, egui::Modifiers::COMMAND);
    app.find = state;
    finish(screen.state());
    screen.run_steps(1);
    assert!(!screen.state().vim_search_active());
    assert!(screen.state().settings_open);
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 6);
}

#[test]
fn toolbar_completion_keeps_query_widget_focused_for_following_text() {
    let mut app = fixture();
    app.settings.smooth_scroll = false;
    app.sidebar_visible = false;
    app.find_visible = true;
    app.find_focus = true;
    app.find_text = "foo".into();
    app.find_next(false);
    let original = app.tab().unwrap().text.clone();
    let mut screen = egui_kittest::Harness::builder()
        .with_size([900.0, 500.0])
        .with_os(egui::os::OperatingSystem::Windows)
        .build_ui_state(
            |ui, app: &mut VEditApp| {
                app.ctx = ui.ctx().clone();
                app.last_disk_check = Instant::now();
                app.last_session_save = Instant::now();
                app.draw_ui(ui);
            },
            app,
        );
    set_style(&screen.ctx);
    screen.run_steps(2);
    let focus = screen.ctx.memory(|memory| memory.focused());
    assert!(focus.is_some());
    finish(screen.state());
    screen.run_steps(1);
    assert_eq!(screen.ctx.memory(|memory| memory.focused()), focus);
    screen.event(egui::Event::Text("x".into()));
    screen.run_steps(1);
    assert!(screen.state().find_text.contains('x'));
    assert_eq!(screen.state().tab().unwrap().text, original);
}

#[test]
fn repeated_toolbar_steps_before_completion_keep_their_count_and_match_identity() {
    let mut app = fixture();
    app.find_visible = true;
    app.find_text = "foo".into();
    app.find_next(false);
    app.find_next(false);
    assert_eq!(app.find.pending_navigation.as_ref().unwrap().count, 2);
    finish(&app);
    app.validate_vim_search();
    assert_eq!(app.tab().unwrap().selection, 17..20);
    assert_eq!(app.find_match, Some(17));
    assert!(!app.tab().unwrap().focus);
    app.find_next(false);
    assert_eq!(app.tab().unwrap().selection, 6..9);
}

#[test]
fn closing_toolbar_cancels_pending_navigation_before_a_ready_result_jumps() {
    let mut app = fixture();
    app.find_visible = true;
    app.find_text = "foo".into();
    app.find_next(false);
    finish(&app);
    app.find_visible = false;
    app.validate_vim_search();
    assert!(app.find.pending_navigation.is_none());
    assert_eq!(app.tab().unwrap().vim.cursor, 0);
    assert_eq!(app.tab().unwrap().selection, 0..0);
    assert!(!app.find_search_running());
}

#[test]
fn changing_toolbar_query_does_not_accumulate_old_pending_steps() {
    let mut app = fixture();
    app.find_visible = true;
    app.find_text = "foo".into();
    app.find_next(false);
    app.find_next(false);
    app.find_text = "middle".into();
    app.find_next(false);
    assert_eq!(app.find.pending_navigation.as_ref().unwrap().count, 1);
    let tab = app.tab().unwrap();
    let matches = crate::search::search(&tab.text, "middle", true, false, false).unwrap();
    app.find.worker.as_ref().unwrap().finish(tab.id, matches);
    app.validate_vim_search();
    assert_eq!(app.tab().unwrap().selection, 10..16);
    assert_eq!(app.find_match, Some(10));
}

#[test]
fn next_press_as_worker_becomes_ready_preserves_steps_and_clears_old_origin() {
    let mut app = fixture();
    app.find_visible = true;
    app.find_text = "foo".into();
    app.find_next(false);
    finish(&app);
    // Shortcuts can run before the next deferred-results poll.
    app.find_next(false);
    assert_eq!(app.tab().unwrap().selection, 17..20);
    assert!(app.find.pending_navigation.is_none());
    app.validate_vim_search();
    assert_eq!(app.find_match, Some(17));
    assert!(app.find.current_match.is_some());
    app.find_next(false);
    assert_eq!(app.tab().unwrap().selection, 6..9);
}

use super::*;
use crate::cli::AgentMarksAction;

fn marks(app: &mut VEditApp, action: AgentMarksAction) -> Value {
    let response = app.control_request(Request::AgentMarks { action });
    assert!(response.ok, "{response:?}");
    response.result
}

fn click_at(
    screen: &mut Harness<'_, VEditApp>,
    point: egui::Pos2,
    modifiers: egui::Modifiers,
    button: egui::PointerButton,
) {
    screen.hover_at(point);
    screen.run_steps(1);
    screen
        .input_mut()
        .events
        .push(egui::Event::ModifiersChanged(modifiers));
    for pressed in [true, false] {
        screen.input_mut().events.push(egui::Event::PointerButton {
            pos: point,
            button,
            pressed,
            modifiers,
        });
        screen.run_steps(1);
    }
    screen
        .input_mut()
        .events
        .push(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    screen.run_steps(1);
}

fn diff_rect(screen: &Harness<'_, VEditApp>) -> egui::Rect {
    screen.get_by_label("Change review diff").rect()
}

#[test]
fn diff_rows_become_a_comment_the_agent_takes_with_file_and_lines() {
    let repository = ReviewRepository::new();
    let mut screen = harness(repository.app());
    screen
        .state_mut()
        .show_change_review(0, ReviewSource::Branch { base: None });
    settle(&mut screen);
    // src/app.rs: Shift+Down chooses rows from the line on screen on.
    type_key(&mut screen, Key::N, "n");
    for _ in 0..6 {
        screen.key_press_modifiers(egui::Modifiers::SHIFT, Key::ArrowDown);
    }
    screen.run_steps(1);
    let selection = view(screen.state()).selection.clone().unwrap();
    assert_eq!(selection.file, 1);
    type_key(&mut screen, Key::C, "c");
    assert!(screen.state().agent_marks_modal_open());
    for label in ["Comment for agent", "Add for agent", "Cancel"] {
        assert!(screen.query_by_label(label).is_some(), "missing {label}");
    }
    screen.event(egui::Event::Text("Use the configured admin account".into()));
    screen.run_steps(1);
    screen.key_press_modifiers(egui::Modifiers::CTRL, Key::Enter);
    screen.run_steps(2);
    assert!(!screen.state().agent_marks_modal_open());
    assert!(view(screen.state()).selection.is_none());
    let (status, error, _) = screen.state().status.clone().unwrap();
    assert!(!error && status.starts_with("Comment added for agent · src/app.rs · Lines 12–"));
    assert!(body_focused(&screen), "the keyboard returns to the diff");

    // The comment arrives through the agent-marks channel without the file
    // being open: path, new-side lines, the diff excerpt and the note.
    let app = screen.state_mut();
    let listed = marks(
        app,
        AgentMarksAction::List {
            limit: 10,
            offset: 0,
        },
    );
    let item = &listed["items"][0];
    let path = repository.root.join("src").join("app.rs");
    assert_eq!(item["path"], crate::paths::display(&path));
    assert_eq!(item["note"], "Use the configured admin account");
    assert_eq!(
        item["review"],
        json!({"comparison": "feature/login vs main", "source": "branch"})
    );
    assert_eq!(item["source_open"], false);
    assert_eq!(item["stale"], false);
    let text = item["text"].as_str().unwrap();
    assert!(text.starts_with(" fn line_12() {}\n"), "{text}");
    assert!(
        text.contains("-fn line_15() {}\n+fn line_15() { crate::login::login(\"admin\"); }\n"),
        "{text}"
    );
    let new_side = text.lines().filter(|line| !line.starts_with('-')).count();
    assert_eq!(item["range"]["start"], json!({"line": 12, "column": 1}));
    assert_eq!(
        item["range"]["end"],
        json!({"line": 12 + new_side, "column": 1}),
        "an exclusive end after the last new-side line"
    );
    let taken = marks(
        app,
        AgentMarksAction::Take {
            request_id: Some("review-comment".into()),
            limit: 10,
        },
    );
    assert_eq!(taken["items"][0]["review"]["source"], "branch");
    let batch = taken["batch_id"].as_u64().unwrap();
    marks(app, AgentMarksAction::Ack { batch_id: batch });
    assert!(app.agent_marks.items.is_empty());
}

#[test]
fn pointer_selects_rows_and_the_context_menu_comments_on_them() {
    let repository = ReviewRepository::new();
    let mut screen = harness(repository.app());
    screen
        .state_mut()
        .show_change_review(0, ReviewSource::Branch { base: None });
    settle(&mut screen);
    // src/app.rs at the top, below its sticky header.
    type_key(&mut screen, Key::N, "n");
    let rect = diff_rect(&screen);
    let x = rect.center().x;
    let row = |screen: &Harness<'_, VEditApp>| {
        view(screen.state())
            .selection
            .clone()
            .map(|selection| (selection.file, selection.rows()))
    };
    click_at(
        &mut screen,
        egui::pos2(x, rect.top() + FILE_HEADER_HEIGHT + 60.0),
        egui::Modifiers::NONE,
        egui::PointerButton::Primary,
    );
    let (file, first) = row(&screen).unwrap();
    assert_eq!((file, first.start() == first.end()), (1, true));
    assert!(*first.start() >= 2);
    // A click on the sticky file header chooses nothing new.
    click_at(
        &mut screen,
        egui::pos2(rect.left() + 40.0, rect.top() + 10.0),
        egui::Modifiers::SHIFT,
        egui::PointerButton::Primary,
    );
    assert_eq!(row(&screen).unwrap().1, first);
    click_at(
        &mut screen,
        egui::pos2(x, rect.top() + FILE_HEADER_HEIGHT + 2.0),
        egui::Modifiers::SHIFT,
        egui::PointerButton::Primary,
    );
    let (_, extended) = row(&screen).unwrap();
    assert_eq!(*extended.end(), *first.end(), "Shift+click extends upward");
    assert!(extended.start() < first.start());
    // Right-click inside keeps the rows; the menu comments on them.
    click_at(
        &mut screen,
        egui::pos2(x, rect.top() + FILE_HEADER_HEIGHT + 2.0),
        egui::Modifiers::NONE,
        egui::PointerButton::Secondary,
    );
    assert_eq!(row(&screen).unwrap().1, extended);
    screen.get_by_label("Comment for agent…").click();
    screen.run_steps(2);
    assert!(screen.state().agent_marks_modal_open());
    // Escape cancels without a mark and keeps the rows.
    screen.key_press(Key::Escape);
    screen.run_steps(2);
    assert!(!screen.state().agent_marks_modal_open());
    assert!(screen.state().agent_marks.items.is_empty());
    assert!(screen.state().change_review_visible());
    assert_eq!(row(&screen).unwrap().1, extended);
    // Escape on the diff clears the rows before it closes the review.
    screen.key_press(Key::Escape);
    screen.run_steps(2);
    assert!(row(&screen).is_none());
    assert!(screen.state().change_review_visible());
}

#[test]
fn changes_after_loading_show_a_calm_refresh_hint_that_keeps_the_position() {
    let repository = ReviewRepository::new();
    let mut screen = harness(repository.app());
    let status = |screen: &mut Harness<'_, VEditApp>| {
        let (head, fingerprint) = git::working_tree_fingerprint(&repository.root).unwrap();
        let project = &mut screen.state_mut().projects[0];
        project.git_head = head;
        project.git_fingerprint = Some(fingerprint);
        screen.run_steps(2);
    };
    let hint = "Changes since this review was loaded";
    // A status read before the comparison is no sign of change.
    screen.state_mut().projects[0].git_fingerprint = Some(1);
    screen
        .state_mut()
        .show_change_review(0, ReviewSource::WorkingTree);
    settle(&mut screen);
    assert!(screen.query_by_label(hint).is_none());
    status(&mut screen);
    assert!(screen.query_by_label(hint).is_none());
    type_key(&mut screen, Key::J, "j");
    let scroll = view(screen.state()).scroll;
    repository.write("README.md", "# Demo\n\nA small project.\nAnother edit\n");
    status(&mut screen);
    assert!(screen.query_by_label(hint).is_some());
    assert_eq!(
        view(screen.state()).scroll,
        scroll,
        "nothing moves by itself"
    );
    screen.get_by_label("Refresh").click();
    settle(&mut screen);
    assert!(screen.query_by_label(hint).is_none());
    assert_eq!(view(screen.state()).scroll, scroll);
    // A commit's comparison never changes.
    screen
        .state_mut()
        .show_change_review(0, ReviewSource::Commit("HEAD".into()));
    settle(&mut screen);
    repository.write("README.md", "# Demo\n");
    status(&mut screen);
    assert!(screen.query_by_label(hint).is_none());
}

#[test]
fn unsaved_documents_in_the_repository_are_named_and_shown_on_request() {
    let repository = ReviewRepository::new();
    let mut app = repository.app();
    let path = repository.root.join("README.md");
    let mut tab = EditorTab::new(Some(path.clone()), "README.md".into());
    tab.text = "# Demo\n\nUnsaved edit\n".into();
    tab.saved_text = "# Demo\n\nA small project.\nWorking tree edit\n".into();
    tab.dirty = true;
    let id = tab.id;
    app.projects[0].tabs.push(tab);
    let mut screen = harness(app);
    let banner = "1 unsaved document is not included";
    screen
        .state_mut()
        .show_change_review(0, ReviewSource::Branch { base: None });
    settle(&mut screen);
    assert!(
        screen.query_by_label(banner).is_none(),
        "a branch comparison does not read the working tree"
    );
    screen
        .state_mut()
        .show_change_review(0, ReviewSource::WorkingTree);
    settle(&mut screen);
    assert!(screen.query_by_label(banner).is_some());
    screen.get_by_label("Show").click();
    settle(&mut screen);
    assert!(!screen.state().change_review_visible());
    assert_eq!(screen.state().tab().unwrap().id, id);
    assert!(
        screen.state().tab().unwrap().dirty,
        "the draft stays as it was"
    );
}

#[test]
fn the_diff_dialog_opens_the_file_at_the_double_clicked_line() {
    let repository = ReviewRepository::new();
    let mut screen = harness(repository.app());
    let root = repository.root.clone();
    screen
        .state_mut()
        .request_git_diff(root.clone(), "README.md".into(), false, false);
    let deadline = Instant::now() + Duration::from_secs(20);
    while screen.state().diff_loading {
        assert!(Instant::now() < deadline, "diff worker timed out");
        std::thread::sleep(Duration::from_millis(5));
        screen.run_steps(1);
    }
    screen.run_steps(3);
    // README.md: three unchanged lines, then the added line 4.
    let rect = screen.get_by_label("Diff contents").rect();
    let row_height = (screen
        .ctx
        .fonts_mut(|fonts| fonts.row_height(&FontId::monospace(12.0)))
        + 7.0)
        .max(20.0);
    double_click(
        &mut screen,
        egui::pos2(rect.left() + 200.0, rect.top() + row_height * 3.5),
    );
    settle(&mut screen);
    assert!(screen.state().diff.is_none());
    let tab = screen.state().tab().unwrap();
    assert_eq!(tab.path.as_deref(), Some(root.join("README.md").as_path()));
    assert_eq!(cursor_line(tab), 4);
    // Enter opens the first line on screen when no row was clicked.
    screen
        .state_mut()
        .request_git_diff(root.clone(), "README.md".into(), false, false);
    while screen.state().diff_loading {
        assert!(Instant::now() < deadline, "diff worker timed out");
        std::thread::sleep(Duration::from_millis(5));
        screen.run_steps(1);
    }
    screen.run_steps(3);
    screen.key_press(Key::Enter);
    settle(&mut screen);
    assert!(screen.state().diff.is_none());
    assert_eq!(cursor_line(screen.state().tab().unwrap()), 1);
}

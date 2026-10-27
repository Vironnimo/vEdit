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

/// Pointer interaction starts with a delivered comparison; Git loading has its
/// own entry-point, freshness, generation and screenshot integration tests.
fn loaded_branch_review_app() -> VEditApp {
    let mut app = VEditApp::empty(egui::Context::default());
    visual_tests::workspace(&mut app);
    let root = app.projects[0].root.clone();
    let source = ReviewSource::Branch { base: None };
    let generation = next_generation();
    let mut review = ChangeReview::new(source.clone(), root.clone());
    review.generation = generation;
    review.loading = true;
    review.focus_pending = true;
    app.projects[0].git_root = Some(root.clone());
    app.projects[0].git_branch = Some("feature/login".into());
    app.projects[0].change_review.view = Some(review);
    let patches = [
        (
            "docs/old.md",
            FileChange::Deleted,
            "@@ -1 +0,0 @@\n-Old notes\n".to_owned(),
            LineStats {
                added: 0,
                removed: 1,
                binary: false,
            },
        ),
        (
            "src/app.rs",
            FileChange::Modified,
            concat!(
                "@@ -12,7 +12,7 @@\n",
                " fn line_12() {}\n fn line_13() {}\n fn line_14() {}\n",
                "-fn line_15() {}\n+fn line_15() { crate::login::login(\"admin\"); }\n",
                " fn line_16() {}\n fn line_17() {}\n fn line_18() {}\n",
                "@@ -28,3 +28,4 @@\n",
                " fn line_28() {}\n fn line_29() {}\n fn line_30() {}\n+mod login;\n",
            )
            .to_owned(),
            LineStats {
                added: 2,
                removed: 1,
                binary: false,
            },
        ),
        (
            "src/login.rs",
            FileChange::Added,
            format!(
                "@@ -0,0 +1,5 @@\n{}",
                LOGIN
                    .lines()
                    .map(|line| format!("+{line}\n"))
                    .collect::<String>()
            ),
            LineStats {
                added: 5,
                removed: 0,
                binary: false,
            },
        ),
    ];
    let files = patches
        .into_iter()
        .enumerate()
        .map(|(index, (path, status, patch, stats))| {
            (
                ReviewFile {
                    path: path.into(),
                    original_path: None,
                    status,
                    stats: Some(stats),
                    content_key: index as u64,
                    patch: ReviewPatch::Deferred,
                },
                EntryPatch::from(ReviewPatch::Loaded(patch)),
            )
        })
        .collect();
    app.sender
        .send(WorkerResult::ChangeReview {
            root,
            generation,
            result: Ok(LoadedReview {
                comparison: ReviewComparison {
                    source,
                    key: "pointer-fixture".into(),
                    title: "feature/login vs main".into(),
                    detail: "main...feature/login".into(),
                    old: "fixture-main".into(),
                    new: Some("fixture-feature".into()),
                },
                files,
                omitted_untracked: 0,
                basis: None,
            }),
        })
        .unwrap();
    app.poll();
    assert!(!view(&app).loading);
    assert_eq!(view(&app).files.len(), 3);
    app
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
    screen
        .state_mut()
        .settings
        .keybindings
        .insert("review_comment".into(), Some("F9".into()));
    type_key(&mut screen, Key::C, "c");
    assert!(!screen.state().agent_marks_modal_open());
    screen.key_press(Key::F9);
    screen.run_steps(2);
    assert!(screen.state().agent_marks_modal_open());
    for label in ["Comment for agent", "Add for agent", "Cancel"] {
        assert!(screen.query_by_label(label).is_some(), "missing {label}");
    }
    screen.event(egui::Event::Text("Use the configured admin account".into()));
    screen.run_steps(1);
    screen
        .state_mut()
        .settings
        .keybindings
        .insert("review_submit_comment".into(), None);
    screen.key_press(Key::F9);
    screen.run_steps(2);
    assert!(screen.state().agent_marks_modal_open());
    screen
        .state_mut()
        .settings
        .keybindings
        .insert("review_submit_comment".into(), Some("F9".into()));
    screen.key_press(Key::F9);
    screen.run_steps(2);
    assert!(!screen.state().agent_marks_modal_open());
    assert!(view(screen.state()).selection.is_none());
    let (status, error, _) = screen.state().status.clone().unwrap();
    assert!(!error && status.starts_with("Comment added for agent · src/app.rs · Old 12–"));
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
    assert_eq!(item["review"]["comparison"], "feature/login vs main");
    assert_eq!(item["review"]["source"], "branch");
    let location = &item["review"]["location"];
    assert_eq!(location["old_lines"]["start"], 12);
    assert_eq!(location["new_lines"]["start"], 12);
    assert_eq!(location["old_path"], "src/app.rs");
    assert_eq!(location["new_path"], "src/app.rs");
    assert_eq!(
        location["old_revision"],
        repository.git(&["rev-parse", "main"])
    );
    assert_eq!(
        location["new_revision"],
        repository.git(&["rev-parse", "HEAD"])
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
fn deleted_diff_lines_keep_their_old_location_through_collection_and_restart() {
    let repository = ReviewRepository::new();
    let mut screen = visual_tests::harness(repository.app());
    screen
        .state_mut()
        .show_change_review(0, ReviewSource::Commit("HEAD".into()));
    settle(&mut screen);
    let review = screen.state_mut().projects[0]
        .change_review
        .view
        .as_mut()
        .unwrap();
    let file = review
        .files
        .iter()
        .position(|file| file.path == "docs/old.md")
        .unwrap();
    let EntryPatch::Parsed(patch) = &review.files[file].patch else {
        panic!("text diff")
    };
    let row = (0..patch.rows())
        .find(|row| patch.diff_line(*row).is_some())
        .unwrap();
    feedback::click_row(
        review,
        file,
        row,
        feedback::RowClick {
            position: egui::Pos2::ZERO,
            extend: false,
            secondary: false,
        },
    );
    review.comment = feedback::CommentDraft::new(review);
    screen.run_steps(2);
    screen.event(egui::Event::Text("Keep these notes · β🦀".into()));
    screen.run_steps(1);
    screen.get_by_label("Add for agent").click();
    screen.run_steps(2);
    let app = screen.state_mut();
    let item = marks(
        app,
        AgentMarksAction::List {
            limit: 10,
            offset: 0,
        },
    )["items"][0]
        .clone();
    assert_eq!(item["text"], "-Old notes\n");
    assert_eq!(item["note"], "Keep these notes · β🦀");
    assert_eq!(
        item["review"]["location"]["old_lines"],
        json!({"start":1,"end":1})
    );
    assert!(item["review"]["location"]["new_lines"].is_null());
    assert_eq!(item["review"]["location"]["old_path"], "docs/old.md");
    assert!(item["review"]["location"]["new_path"].is_null());
    assert_eq!(item["source_open"], false);
    let taken = marks(
        app,
        AgentMarksAction::Take {
            request_id: Some("deleted-lines".into()),
            limit: 10,
        },
    );
    let batch_id = taken["batch_id"].as_u64().unwrap();
    marks(app, AgentMarksAction::Ack { batch_id });
    app.agent_marks =
        serde_json::from_slice(&serde_json::to_vec(&app.agent_marks).unwrap()).unwrap();
    marks(
        app,
        AgentMarksAction::Restore {
            batch_id: Some(batch_id),
        },
    );
    assert_eq!(
        marks(
            app,
            AgentMarksAction::List {
                limit: 10,
                offset: 0
            }
        )["items"][0],
        item
    );
    // The collection itself identifies the absent new side, without opening a deleted file.
    screen.get_by_label("For agent").click();
    screen.run_steps(3);
    assert!(
        screen
            .query_by_label_contains("Marked block Old 1 · New —")
            .is_some()
    );
    visual_tests::snapshot(&mut screen, "vedit_agent_review_marks");
    screen.key_press(Key::Escape);
    screen.run_steps(3);
    assert!(
        body_focused(&screen),
        "closing the collection returns to the diff"
    );
}

#[test]
fn review_locations_keep_renames_additions_and_the_comparison_at_capture() {
    let repository = ReviewRepository::new();
    repository.git(&["mv", "src/app.rs", "src/neu β.rs"]);
    let text = app_source(true)
        .replace("fn line_2() {}", "fn line_2() { /* β🦀 */ }")
        .replace("fn line_29() {}", "fn line_29() { /* changed */ }");
    repository.write("src/neu β.rs", &text);
    let mut app = repository.app();
    app.show_change_review(0, ReviewSource::WorkingTree);
    wait(&mut app);
    let capture = |app: &mut VEditApp, path: &str| {
        let review = app.projects[0].change_review.view.as_mut().unwrap();
        let file = review
            .files
            .iter()
            .position(|file| file.path == path)
            .unwrap();
        let EntryPatch::Parsed(patch) = &review.files[file].patch else {
            panic!("text diff")
        };
        let rows: Vec<_> = (0..patch.rows())
            .filter(|row| patch.diff_line(*row).is_some())
            .collect();
        for (row, extend) in [(rows[0], false), (*rows.last().unwrap(), true)] {
            feedback::click_row(
                review,
                file,
                row,
                feedback::RowClick {
                    position: egui::Pos2::ZERO,
                    extend,
                    secondary: false,
                },
            );
        }
        feedback::CommentDraft::new(review).unwrap()
    };
    let rename = capture(&mut app, "src/neu β.rs");
    let addition = capture(&mut app, "todo.txt");
    // A later comparison cannot silently relabel an already captured comment.
    app.show_change_review(0, ReviewSource::Branch { base: None });
    wait(&mut app);
    app.add_review_comment(0, rename);
    app.add_review_comment(0, addition);
    let listed = marks(
        &mut app,
        AgentMarksAction::List {
            limit: 10,
            offset: 0,
        },
    );
    let renamed = &listed["items"][0];
    let location = &renamed["review"]["location"];
    assert_eq!(renamed["review"]["source"], "working");
    assert_eq!(
        location["old_revision"],
        repository.git(&["rev-parse", "HEAD"])
    );
    assert!(location["new_revision"].is_null());
    assert_eq!(location["old_path"], "src/app.rs");
    assert_eq!(location["new_path"], "src/neu β.rs");
    assert_eq!(location["old_lines"], json!({"start":1,"end":31}));
    assert_eq!(location["new_lines"], json!({"start":1,"end":31}));
    assert!(renamed["text"].as_str().unwrap().contains("β🦀"));
    let added = &listed["items"][1]["review"]["location"];
    assert!(added["old_path"].is_null());
    assert!(added["old_lines"].is_null());
    assert_eq!(added["new_path"], "todo.txt");
    assert_eq!(added["new_lines"], json!({"start":1,"end":1}));

    // Older saved comments keep their text and note without inventing old-side data.
    let mut stored = serde_json::to_value(&app.agent_marks).unwrap();
    stored["items"][0]["review"]
        .as_object_mut()
        .unwrap()
        .remove("location");
    app.agent_marks = serde_json::from_value(stored).unwrap();
    let restored = marks(
        &mut app,
        AgentMarksAction::List {
            limit: 10,
            offset: 0,
        },
    );
    assert!(restored["items"][0]["review"].get("location").is_none());
    assert_eq!(restored["items"][0]["text"], renamed["text"]);
    assert_eq!(restored["items"][1], listed["items"][1]);
}

#[test]
fn pointer_selects_rows_and_the_context_menu_comments_on_them() {
    let mut screen = harness(loaded_branch_review_app());
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
fn branch_review_warns_when_the_base_moves_without_head_moving() {
    for base in [None, Some("main~0".to_owned())] {
        let repository = ReviewRepository::new();
        let source = ReviewSource::Branch { base };
        let before = git::change_set(&repository.root, &source).unwrap();
        assert!(!before.files.is_empty());
        let mut screen = harness(repository.app());
        let refresh = |screen: &mut Harness<'_, VEditApp>| {
            screen.state_mut().refresh_git(0, true);
            let deadline = Instant::now() + Duration::from_secs(20);
            while screen.state().projects[0].git_loading {
                assert!(Instant::now() < deadline, "Git status worker timed out");
                std::thread::sleep(Duration::from_millis(5));
                screen.state_mut().poll();
            }
            screen.run_steps(2);
        };
        refresh(&mut screen);
        screen.state_mut().show_change_review(0, source.clone());
        settle(&mut screen);
        let hint = "Changes since this review was loaded";
        assert!(screen.query_by_label(hint).is_none());
        let generation = view(screen.state()).generation;
        let state = &mut screen.state_mut().projects[0].change_review;
        state.apply_branch_check(generation - 1, &repository.root, Err("old review".into()));
        state.apply_branch_check(generation, Path::new("wrong-root"), Err("old root".into()));
        screen.run_steps(2);
        assert!(screen.query_by_label(hint).is_none());

        let head = repository.git(&["rev-parse", "HEAD"]);
        repository.git(&["update-ref", "refs/heads/main", &head]);
        let after = git::change_set(&repository.root, &source).unwrap();
        assert_eq!(before.comparison.new, after.comparison.new);
        assert_ne!(before.comparison.old, after.comparison.old);
        assert!(after.files.is_empty());
        refresh(&mut screen);
        assert!(screen.query_by_label(hint).is_some());
        screen.get_by_label("Refresh").click();
        settle(&mut screen);
        assert!(screen.query_by_label(hint).is_none());
        repository.git(&["update-ref", "-d", "refs/heads/main"]);
        refresh(&mut screen);
        assert!(screen.query_by_label(hint).is_some());
    }
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

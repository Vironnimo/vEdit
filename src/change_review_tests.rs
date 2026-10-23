//! Change review against real temporary repositories: entry points, keys,
//! seen marks, closing, CLI answers and stale worker results.
use super::*;
use crate::cli::{Parsed, SplitOrientation, parse_args};
use egui::accesskit::{Role, Toggled};
use egui_kittest::{Harness, Node, kittest::NodeT, kittest::Queryable};
use std::sync::mpsc;

/// Fixed dates keep commit hashes, and with them the titles, deterministic.
const DATE: &str = "2026-01-02T03:04:05+00:00";
const LOGIN: &str =
    "pub fn login(user: &str) -> bool {\n    !user.is_empty()\n}\n\npub fn logout() {}\n";

fn app_source(fixed: bool) -> String {
    let mut text: String = (1..=30)
        .map(|line| {
            if fixed && line == 15 {
                "fn line_15() { crate::login::login(\"admin\"); }\n".to_owned()
            } else {
                format!("fn line_{line}() {{}}\n")
            }
        })
        .collect();
    if fixed {
        text.push_str("mod login;\n");
    }
    text
}

/// `main` has one commit. `feature/login` adds "Fix login": it edits
/// src/app.rs, adds src/login.rs and deletes docs/old.md. The working tree
/// edits README.md, stages notes.txt and leaves todo.txt untracked.
struct ReviewRepository {
    base: PathBuf,
    root: PathBuf,
}

impl ReviewRepository {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir().join(format!(
            "vedit-change-review-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        // The displayed project name and path suffix stay deterministic.
        let root = base.join("review").join("fixtures").join("demo-project");
        fs::create_dir_all(&root).unwrap();
        let repository = Self { base, root };
        repository.git(&["init", "--quiet", "--initial-branch=main"]);
        let hooks = repository.root.join(".git").join("vedit-no-hooks");
        fs::create_dir_all(&hooks).unwrap();
        // Preserve init's platform settings and configure only this fresh fixture.
        let config_path = repository.root.join(".git/config");
        let mut config = fs::read_to_string(&config_path).unwrap();
        config.push_str(
            "\n[user]\n\tname = Review Author\n\temail = review@example.invalid\n\
             [core]\n\tautocrlf = false\n\thooksPath = .git/vedit-no-hooks\n\
             [commit]\n\tgpgsign = false\n",
        );
        fs::write(config_path, config).unwrap();
        repository.write("src/app.rs", &app_source(false));
        repository.write("README.md", "# Demo\n\nA small project.\n");
        repository.write("docs/old.md", "Old notes\n");
        repository.commit("Initial");
        repository.git(&["switch", "--quiet", "-c", "feature/login"]);
        repository.write("src/app.rs", &app_source(true));
        repository.write("src/login.rs", LOGIN);
        fs::remove_file(repository.root.join("docs").join("old.md")).unwrap();
        repository.commit("Fix login");
        repository.write(
            "README.md",
            "# Demo\n\nA small project.\nWorking tree edit\n",
        );
        repository.write("notes.txt", "Staged note\n");
        repository.git(&["add", "notes.txt"]);
        repository.write("todo.txt", "Untracked todo\n");
        repository
    }

    fn git(&self, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(args)
            .env("GIT_AUTHOR_DATE", DATE)
            .env("GIT_COMMITTER_DATE", DATE)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    fn write(&self, path: &str, text: &str) {
        let path = path
            .split('/')
            .fold(self.root.clone(), |path, part| path.join(part));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn commit(&self, message: &str) {
        self.git(&["add", "--all"]);
        self.git(&["commit", "--quiet", "-m", message]);
    }

    fn app(&self) -> VEditApp {
        let mut app = VEditApp::empty(egui::Context::default());
        visual_tests::workspace(&mut app);
        let index = workspace::build_index(&self.root, workspace::IndexOptions::default()).unwrap();
        let project = &mut app.projects[0];
        project.root = self.root.clone();
        project.git_root = Some(self.root.clone());
        project.git_branch = Some("feature/login".into());
        project.index = Some(Arc::new(index));
        app
    }

    /// The same project with Source control on the left and loaded history.
    fn source_control_app(&self) -> VEditApp {
        let mut app = self.app();
        app.set_sidebar_view_visible(SidebarView::Explorer, false);
        app.move_sidebar(SidebarView::SourceControl, sidebar_layout::Side::Left);
        let snapshot = git::history_snapshot(&self.root).unwrap();
        let page = git::history_page(&self.root, &snapshot, 0, 10).unwrap();
        let project = &mut app.projects[0];
        project.git_history_snapshot = Some(snapshot);
        project.git_history_has_more = false;
        project.recent_commits = page.commits;
        project.git_view.graph.append(&project.recent_commits);
        project.git_main_comparison = Ok(Some(git::BranchComparison {
            base: "main".into(),
            ahead: 1,
            behind: 0,
        }));
        app
    }
}

impl Drop for ReviewRepository {
    fn drop(&mut self) {
        let temporary = fs::canonicalize(std::env::temp_dir()).unwrap();
        let base = fs::canonicalize(&self.base).unwrap();
        assert!(base.starts_with(&temporary));
        assert!(
            base.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("vedit-change-review-")
        );
        let _ = fs::remove_dir_all(&base);
    }
}

fn harness(app: VEditApp) -> Harness<'static, VEditApp> {
    let mut screen = Harness::builder()
        .with_size([1440.0, 920.0])
        .with_os(egui::os::OperatingSystem::Windows)
        .build_ui_state(
            |ui, app: &mut VEditApp| {
                app.ctx = ui.ctx().clone();
                // Only the review's own workers may run: no disk polling,
                // Git refresh, session writes or autosave.
                app.last_disk_check = Instant::now();
                app.last_session_save = Instant::now();
                app.session_dirty = false;
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
    screen.run_steps(4);
    screen
}

fn pending(app: &VEditApp) -> bool {
    app.projects.iter().any(|project| {
        project.tabs.iter().any(|tab| tab.loading)
            || project.change_review.view.as_ref().is_some_and(|view| {
                view.loading
                    || view.lazy_request.is_some()
                    || view
                        .files
                        .iter()
                        .any(|entry| matches!(entry.patch, EntryPatch::Loading))
            })
    })
}

/// Run frames until review and document workers have delivered.
fn settle(screen: &mut Harness<'_, VEditApp>) {
    let deadline = Instant::now() + Duration::from_secs(20);
    screen.run_steps(1);
    while pending(screen.state()) {
        assert!(Instant::now() < deadline, "change review workers timed out");
        std::thread::sleep(Duration::from_millis(5));
        screen.run_steps(1);
    }
    screen.run_steps(3);
}

fn wait(app: &mut VEditApp) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while pending(app) {
        assert!(Instant::now() < deadline, "change review workers timed out");
        std::thread::sleep(Duration::from_millis(5));
        app.poll();
    }
}

fn view(app: &VEditApp) -> &ChangeReview {
    app.projects[0]
        .change_review
        .view
        .as_ref()
        .expect("a visible change review")
}

fn files(app: &VEditApp) -> Vec<(&str, FileChange)> {
    view(app)
        .files
        .iter()
        .map(|entry| (entry.path.as_str(), entry.status))
        .collect()
}

fn seen(app: &VEditApp) -> Vec<bool> {
    let state = &app.projects[0].change_review;
    let view = view(app);
    (0..view.files.len())
        .map(|index| view.is_seen(&state.seen, index))
        .collect()
}

fn body_focused(screen: &Harness<'_, VEditApp>) -> bool {
    let id = review_body_id(&screen.state().projects[0].root);
    screen.ctx.memory(|memory| memory.has_focus(id))
}

/// A typed key arrives as its key press and its text, as on Windows.
fn type_key(screen: &mut Harness<'_, VEditApp>, key: Key, text: &str) {
    let event = |pressed| egui::Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    screen
        .input_mut()
        .events
        .extend([event(true), egui::Event::Text(text.into()), event(false)]);
    screen.run_steps(2);
}

/// The file list row of `path`: (clicked when asked, highlighted as current).
fn list_row(screen: &Harness<'_, VEditApp>, path: &str, click: bool) -> bool {
    // The Explorer on the left can list a file of the same name.
    let row = screen
        .get_all_by_role_and_label(Role::Button, path)
        .max_by(|left, right| left.rect().left().total_cmp(&right.rect().left()))
        .unwrap();
    if click {
        row.click();
    }
    row.accesskit_node().toggled() == Some(Toggled::True)
}

/// The seen box of `path` in the file list, or in its file header (the
/// rightmost one). Returns whether it was checked before any click.
fn seen_box(screen: &Harness<'_, VEditApp>, path: &str, header: bool, click: bool) -> bool {
    let label = format!("Seen {path}");
    let mut boxes: Vec<Node<'_>> = screen
        .get_all_by_role_and_label(Role::CheckBox, &label)
        .collect();
    boxes.sort_by(|left, right| left.rect().left().total_cmp(&right.rect().left()));
    assert_eq!(boxes.len(), 2, "list and header boxes for {path}");
    let node = if header { &boxes[1] } else { &boxes[0] };
    if click {
        node.click();
    }
    node.accesskit_node().toggled() == Some(Toggled::True)
}

fn cursor_line(tab: &EditorTab) -> usize {
    tab.text
        .chars()
        .take(tab.selection.start)
        .filter(|character| *character == '\n')
        .count()
        + 1
}

fn double_click(screen: &mut Harness<'_, VEditApp>, point: egui::Pos2) {
    screen.hover_at(point);
    screen.run_steps(1);
    let started = screen.ctx.input(|input| input.time);
    for click in 0..2 {
        // A real double-click interval, with press and release in one frame.
        screen.input_mut().time = Some(started + 0.1 * (click + 1) as f64);
        for pressed in [true, false] {
            screen.input_mut().events.push(egui::Event::PointerButton {
                pos: point,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::default(),
            });
        }
        screen.step();
    }
}

#[test]
fn palette_opens_the_working_tree_review_and_escape_restores_the_panes() {
    let repository = ReviewRepository::new();
    let mut screen = harness(repository.app());
    screen
        .state_mut()
        .split_active(SplitOrientation::Vertical)
        .unwrap();
    screen.run_steps(3);
    let layout = screen.state().layout_status();
    let document = screen.state().tab().unwrap().id;
    let text = screen.state().tab().unwrap().text.clone();
    screen.state_mut().open_palette(Palette::Commands);
    screen.run_steps(3);
    screen.event(egui::Event::Text("Review working tree".into()));
    screen.run_steps(2);
    screen.key_press(Key::Enter);
    settle(&mut screen);
    assert!(screen.state().palette.is_none());
    assert!(screen.state().change_review_visible());
    assert_eq!(view(screen.state()).title(), "Working tree");
    assert_eq!(
        files(screen.state()),
        [
            ("notes.txt", FileChange::Added),
            ("README.md", FileChange::Modified),
            ("todo.txt", FileChange::Untracked),
        ]
    );
    assert!(
        view(screen.state())
            .files
            .iter()
            .all(|entry| matches!(entry.patch, EntryPatch::Parsed(_)))
    );
    for label in ["Working tree", "3 files", "+3", "0 of 3 seen", "Refresh"] {
        assert!(screen.query_by_label(label).is_some(), "missing {label}");
    }
    assert!(body_focused(&screen), "the review takes the keyboard");
    // The panes are hidden, not rearranged, and their tabs stay untouched.
    assert_eq!(screen.state().layout_status(), layout);
    assert!(screen.query_by_label("Split right").is_none());
    screen.key_press(Key::Escape);
    screen.run_steps(3);
    assert!(!screen.state().change_review_visible());
    assert_eq!(screen.state().layout_status(), layout);
    let tab = screen.state().tab().unwrap();
    assert_eq!((tab.id, &tab.text, tab.dirty), (document, &text, false));
    assert_eq!(
        screen.ctx.memory(|memory| memory.focused()),
        Some(screen.state().document_widget_id(document)),
        "closing returns the keyboard to the document"
    );
}

#[test]
fn keys_scroll_jump_mark_seen_and_open_the_line_on_screen() {
    let repository = ReviewRepository::new();
    let mut screen = harness(repository.app());
    screen
        .state_mut()
        .show_change_review(0, ReviewSource::Branch { base: None });
    settle(&mut screen);
    assert_eq!(view(screen.state()).title(), "feature/login vs main");
    assert_eq!(
        files(screen.state()),
        [
            ("docs/old.md", FileChange::Deleted),
            ("src/app.rs", FileChange::Modified),
            ("src/login.rs", FileChange::Added),
        ]
    );
    assert!(body_focused(&screen));
    let tops = view(screen.state()).tops.clone();
    assert_eq!(view(screen.state()).scroll, 0.0);
    let position = |screen: &Harness<'_, VEditApp>| {
        let view = view(screen.state());
        (view.current, view.scroll)
    };
    type_key(&mut screen, Key::N, "n");
    assert_eq!(position(&screen), (1, tops[1]));
    assert!(
        list_row(&screen, "src/app.rs", false),
        "the list highlights the current file"
    );
    assert!(!list_row(&screen, "src/login.rs", false));
    type_key(&mut screen, Key::J, "j");
    let row = view(screen.state()).scroll - tops[1];
    assert!((10.0..40.0).contains(&row), "j scrolls one row, not {row}");
    type_key(&mut screen, Key::K, "k");
    assert_eq!(position(&screen), (1, tops[1]));
    type_key(&mut screen, Key::CloseBracket, "]");
    assert_eq!(position(&screen), (2, tops[2]));
    type_key(&mut screen, Key::P, "p");
    assert_eq!(position(&screen), (1, tops[1]));
    type_key(&mut screen, Key::OpenBracket, "[");
    assert_eq!(position(&screen), (0, 0.0));
    // Space marks the current file and moves to the next unseen one.
    type_key(&mut screen, Key::Space, " ");
    assert_eq!(position(&screen), (1, tops[1]));
    assert_eq!(seen(screen.state()), [true, false, false]);
    type_key(&mut screen, Key::Space, " ");
    assert_eq!(position(&screen), (2, tops[2]));
    assert_eq!(seen(screen.state()), [true, true, false]);
    assert!(screen.query_by_label("2 of 3 seen").is_some());
    // Enter opens the file at the first line below the sticky header: the
    // first hunk of src/app.rs starts at line 12.
    type_key(&mut screen, Key::P, "p");
    assert_eq!(position(&screen), (1, tops[1]));
    screen.key_press(Key::Enter);
    settle(&mut screen);
    assert!(!screen.state().change_review_visible());
    let tab = screen.state().tab().unwrap();
    assert_eq!(
        tab.path.as_deref(),
        Some(repository.root.join("src").join("app.rs").as_path())
    );
    assert_eq!(tab.text, app_source(true));
    assert_eq!(cursor_line(tab), 12);
    // Reopening the same review keeps the reading position and seen marks.
    screen
        .state_mut()
        .show_change_review(0, ReviewSource::Branch { base: None });
    settle(&mut screen);
    assert_eq!(position(&screen), (1, tops[1]));
    assert_eq!(seen(screen.state()), [true, true, false]);
    // o opens as well; a deleted file has nothing to open.
    type_key(&mut screen, Key::OpenBracket, "[");
    type_key(&mut screen, Key::O, "o");
    assert!(screen.state().change_review_visible());
    let (status, error, _) = screen.state().status.clone().unwrap();
    assert!(error && status.contains("docs/old.md is deleted in this comparison"));
}

#[test]
fn seen_marks_follow_the_compared_content_and_rows_jump_to_files() {
    let repository = ReviewRepository::new();
    let mut screen = harness(repository.app());
    screen
        .state_mut()
        .show_change_review(0, ReviewSource::WorkingTree);
    settle(&mut screen);
    assert!(!seen_box(&screen, "README.md", false, true));
    screen.run_steps(3);
    assert!(!seen_box(&screen, "notes.txt", true, true));
    screen.run_steps(3);
    assert_eq!(seen(screen.state()), [true, true, false]);
    assert!(screen.query_by_label("2 of 3 seen").is_some());
    assert!(seen_box(&screen, "notes.txt", false, false));
    assert!(seen_box(&screen, "README.md", true, false));
    assert!(body_focused(&screen), "keys keep working after a click");
    // Seen marks only the version that was read.
    repository.write(
        "README.md",
        "# Demo\n\nA small project.\nWorking tree edit, revised later\n",
    );
    screen.get_by_label("Refresh").click();
    settle(&mut screen);
    assert_eq!(seen(screen.state()), [true, false, false]);
    assert!(screen.query_by_label("1 of 3 seen").is_some());
    // Unmarking keeps the other marks; clicking a row jumps to its file.
    assert!(seen_box(&screen, "notes.txt", false, true));
    screen.run_steps(3);
    assert_eq!(seen(screen.state()), [false, false, false]);
    list_row(&screen, "todo.txt", true);
    screen.run_steps(3);
    let tops = view(screen.state()).tops.clone();
    assert_eq!(
        (view(screen.state()).current, view(screen.state()).scroll),
        (2, tops[2])
    );
    assert!(list_row(&screen, "todo.txt", false));
    assert!(body_focused(&screen), "keys keep working after a click");
    type_key(&mut screen, Key::Space, " ");
    assert_eq!(seen(screen.state()), [false, false, true]);
}

#[test]
fn source_control_entry_points_open_commit_branch_and_working_tree_reviews() {
    let repository = ReviewRepository::new();
    let head = repository.git(&["rev-parse", "HEAD"]);
    let commit_title = format!("{} Fix login", &head[..7]);
    let mut screen = harness(repository.source_control_app());
    // A single click keeps opening the commit details.
    screen.get_by_label("Fix login").click();
    screen.run_steps(4);
    assert!(screen.state().git_detail_open());
    assert!(!screen.state().change_review_visible());
    screen.key_press(Key::Escape);
    screen.run_steps(4);
    assert!(!screen.state().git_detail_open());
    // A double-click reviews the commit and leaves no details behind.
    let row = screen.get_by_label("Fix login").rect().center();
    double_click(&mut screen, row);
    settle(&mut screen);
    assert!(screen.state().change_review_visible());
    assert!(!screen.state().git_detail_open());
    assert_eq!(view(screen.state()).title(), commit_title);
    assert_eq!(files(screen.state()).len(), 3);
    screen.get_by_label("Close change review").click();
    screen.run_steps(3);
    assert!(!screen.state().change_review_visible());
    // The history row's context menu.
    screen.get_by_label("Fix login").click_secondary();
    screen.run_steps(3);
    screen.get_by_label("Review commit").click();
    settle(&mut screen);
    assert_eq!(view(screen.state()).title(), commit_title);
    // The main comparison in the overview reviews the branch.
    screen.get_by_label("main ↑1 ↓0").click();
    settle(&mut screen);
    assert_eq!(view(screen.state()).title(), "feature/login vs main");
    // The panel menu reviews the working tree.
    screen.get_by_label("Source control panel actions").click();
    screen.run_steps(3);
    screen.get_by_label("Review all changes").click();
    settle(&mut screen);
    assert_eq!(view(screen.state()).title(), "Working tree");
    // The replaced reviews stay parked one at a time; nothing became a tab.
    assert_eq!(screen.state().projects[0].tabs.len(), 2);
}

#[test]
fn cli_requests_answer_once_the_comparison_loaded() {
    let repository = ReviewRepository::new();
    let mut screen = harness(repository.app());
    let request = |source, rev: Option<&str>, workspace: Option<PathBuf>| Request::ChangeReview {
        source,
        rev: rev.map(str::to_owned),
        base: None,
        workspace,
        background: false,
    };
    let (reply, answers) = mpsc::channel();
    let commit = request(
        ChangeReviewSource::Commit,
        Some("HEAD"),
        Some(repository.root.clone()),
    );
    assert!(screen.state_mut().control_change_review(&commit, reply));
    assert!(screen.state().change_review_visible());
    assert!(answers.try_recv().is_err(), "the answer waits for the load");
    settle(&mut screen);
    let response = answers.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(response.ok, "{:?}", response.error);
    let head = repository.git(&["rev-parse", "HEAD"]);
    assert_eq!(response.result["source"], "commit");
    assert_eq!(
        response.result["title"],
        format!("{} Fix login", &head[..7])
    );
    assert_eq!(response.result["files"], 3);
    assert_eq!(response.result["added"], 7);
    assert_eq!(response.result["removed"], 2);
    assert_eq!(response.result["seen"], 0);
    assert_eq!(response.result["visible"], true);
    assert_eq!(response.result["workspace_source"], "request");

    let failure = |screen: &mut Harness<'_, VEditApp>, request: &Request| {
        let (reply, answers) = mpsc::channel();
        assert!(screen.state_mut().control_change_review(request, reply));
        settle(screen);
        let response = answers.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(!response.ok);
        response.error.unwrap().code
    };
    let missing = request(ChangeReviewSource::Commit, Some("no-such-revision"), None);
    assert_eq!(failure(&mut screen, &missing), "review_failed");
    assert!(
        screen
            .query_by_label("The comparison could not be loaded")
            .is_some()
    );
    let elsewhere = request(
        ChangeReviewSource::Working,
        None,
        Some(repository.base.clone()),
    );
    assert_eq!(failure(&mut screen, &elsewhere), "unknown_workspace");
    let invalid = request(ChangeReviewSource::Working, Some("HEAD"), None);
    assert_eq!(failure(&mut screen, &invalid), "invalid_request");
    screen.state_mut().settings_open = true;
    let working = request(ChangeReviewSource::Working, None, None);
    assert_eq!(failure(&mut screen, &working), "user_dialog_open");
    screen.state_mut().settings_open = false;
    screen.run_steps(2);

    // Closing before the comparison loaded answers the waiting request.
    let (reply, answers) = mpsc::channel();
    let branch = request(ChangeReviewSource::Branch, None, None);
    assert!(screen.state_mut().control_change_review(&branch, reply));
    screen.state_mut().close_change_review(0);
    let response = answers.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(response.error.unwrap().code, "review_closed");
    settle(&mut screen);
    assert!(!screen.state().change_review_visible());

    screen.state_mut().projects[0].git_root = None;
    assert_eq!(failure(&mut screen, &working), "no_repository");
}

#[test]
fn results_for_replaced_or_other_reviews_are_ignored() {
    let repository = ReviewRepository::new();
    let mut app = repository.app();
    app.open_change_review(0, ReviewSource::WorkingTree)
        .unwrap();
    let replaced = view(&app).generation;
    app.open_change_review(0, ReviewSource::Commit("HEAD".into()))
        .unwrap();
    let current = view(&app).generation;
    assert!(current > replaced);
    wait(&mut app);
    // The working-tree worker also finished; only the commit was applied.
    let head = repository.git(&["rev-parse", "HEAD"]);
    assert_eq!(view(&app).title(), format!("{} Fix login", &head[..7]));
    assert!(app.projects[0].change_review.parked.is_some());
    let comparison = view(&app).comparison.clone().unwrap();
    let empty = || LoadedReview {
        comparison: comparison.clone(),
        files: Vec::new(),
        omitted_untracked: 0,
        basis: None,
    };
    let root = repository.root.clone();
    for (root, generation) in [(root.clone(), replaced), (repository.base.clone(), current)] {
        app.sender
            .send(WorkerResult::ChangeReview {
                root,
                generation,
                result: Ok(empty()),
            })
            .unwrap();
    }
    app.poll();
    assert_eq!(files(&app).len(), 3, "stale or foreign lists are ignored");
    // Patches must match the generation, the path and the listed version.
    let key = view(&app).files[1].content_key;
    app.projects[0].change_review.view.as_mut().unwrap().files[1].patch = EntryPatch::Deferred;
    let patch = |generation, content_key| WorkerResult::ChangeReviewPatches {
        root: root.clone(),
        generation,
        request: 0,
        patches: vec![LoadedPatch {
            path: "src/app.rs".into(),
            content_key,
            patch: EntryPatch::Binary,
        }],
    };
    for result in [patch(replaced, key), patch(current, key ^ 1)] {
        app.sender.send(result).unwrap();
    }
    app.poll();
    assert!(matches!(view(&app).files[1].patch, EntryPatch::Deferred));
    app.sender.send(patch(current, key)).unwrap();
    app.poll();
    assert!(matches!(view(&app).files[1].patch, EntryPatch::Binary));
}

#[test]
fn a_reopened_project_ignores_results_requested_before_it_closed() {
    let repository = ReviewRepository::new();
    let mut app = repository.app();
    app.open_change_review(0, ReviewSource::WorkingTree)
        .unwrap();
    let before = view(&app).generation;
    // Closing and reopening the project at the same root starts a fresh state.
    app.projects[0].change_review = ChangeReviewState::default();
    app.open_change_review(0, ReviewSource::WorkingTree)
        .unwrap();
    assert_ne!(view(&app).generation, before);
    app.sender
        .send(WorkerResult::ChangeReview {
            root: repository.root.clone(),
            generation: before,
            result: Err("stale result".into()),
        })
        .unwrap();
    app.poll();
    assert_ne!(view(&app).error.as_deref(), Some("stale result"));
}

#[test]
fn a_background_review_of_the_visible_project_waits_out_of_sight() {
    let repository = ReviewRepository::new();
    let mut app = repository.app();
    // A user dialog does not block preparing a review in the background.
    app.settings_open = true;
    let (reply, answers) = mpsc::channel();
    let request = Request::ChangeReview {
        source: ChangeReviewSource::Working,
        rev: None,
        base: None,
        workspace: None,
        background: true,
    };
    assert!(app.control_change_review(&request, reply));
    assert!(!app.change_review_visible());
    let deadline = Instant::now() + Duration::from_secs(20);
    let response = loop {
        app.poll();
        if let Ok(response) = answers.try_recv() {
            break response;
        }
        assert!(Instant::now() < deadline, "the review did not load");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(response.ok, "{:?}", response.error);
    assert_eq!(response.result["visible"], false);
    assert_eq!(response.result["background"], true);
    assert_eq!(response.result["workspace_source"], "active_project");
    assert_eq!(response.result["files"], 3);
    assert!(!app.change_review_visible());
    app.settings_open = false;
    // The user's next review command shows the prepared comparison.
    app.open_change_review(0, ReviewSource::WorkingTree)
        .unwrap();
    assert!(app.change_review_visible());
    assert_eq!(view(&app).files.len(), 3);
}

#[test]
fn review_flags_parse_to_validated_change_review_requests() {
    let parse = |args: &[&str]| parse_args(args.iter().map(std::ffi::OsString::from));
    let Ok(Parsed::Command {
        request:
            Request::ChangeReview {
                source,
                rev,
                base,
                workspace,
                ..
            },
        start,
    }) = parse(&["review", "--commit", "HEAD~1", "C:/project"])
    else {
        panic!("commit review")
    };
    assert_eq!(
        (source, rev.as_deref(), base, start),
        (ChangeReviewSource::Commit, Some("HEAD~1"), None, false),
        "a change review never starts vEdit"
    );
    assert!(workspace.unwrap().is_absolute());
    let Ok(Parsed::Command {
        request: Request::ChangeReview { source, base, .. },
        ..
    }) = parse(&["review", "--branch", "--base", "develop"])
    else {
        panic!("branch review")
    };
    assert_eq!(
        (source, base.as_deref()),
        (ChangeReviewSource::Branch, Some("develop"))
    );
    assert!(matches!(
        parse(&["review", "--working"]),
        Ok(Parsed::Command {
            request: Request::ChangeReview {
                source: ChangeReviewSource::Working,
                workspace: None,
                ..
            },
            start: false,
        })
    ));
    for args in [
        &["review", "--working", "--branch"][..],
        &["review", "--base", "main"],
        &["review", "--commit"],
        &["review", "--commit", "-x"],
        &["review", "--working", "--base", "main"],
        &["review", "--working", "one", "two"],
        &["status", "--working"],
    ] {
        assert!(parse(args).is_err(), "{args:?} must be rejected");
    }
    let request = |value: Value| serde_json::from_value::<Request>(value).unwrap();
    assert!(
        crate::cli::validate(&request(json!({"op":"change_review","source":"commit"}))).is_err()
    );
    assert!(
        crate::cli::validate(&request(
            json!({"op":"change_review","source":"branch","rev":"HEAD"})
        ))
        .is_err()
    );
    assert!(
        crate::cli::validate(&request(
            json!({"op":"change_review","source":"branch","base":"main\n"})
        ))
        .is_err()
    );
    let batch =
        request(json!({"op":"batch","actions":[{"op":"change_review","source":"working"}]}));
    assert!(
        crate::cli::validate(&batch)
            .unwrap_err()
            .contains("change review")
    );
    assert!(crate::cli::schema_topic(Some("change-review")).unwrap()["change_review"].is_object());
}

#[test]
fn change_review_screenshots() {
    let repository = ReviewRepository::new();
    let mut screen = visual_tests::harness(repository.app());
    // Highlight the hidden documents first; the snapshot waits for them.
    visual_tests::settle_render(&mut screen);
    screen
        .state_mut()
        .show_change_review(0, ReviewSource::Branch { base: None });
    settle(&mut screen);
    type_key(&mut screen, Key::Space, " ");
    for _ in 0..3 {
        type_key(&mut screen, Key::J, "j");
    }
    assert_eq!(view(screen.state()).current, 1);
    visual_tests::snapshot(&mut screen, "vedit_change_review");

    // Large and binary files are listed with a note instead of a patch.
    repository.write(
        "data/large.txt",
        &(0..=git::REVIEW_LARGE_FILE_LINES)
            .map(|line| format!("line {line}\n"))
            .collect::<String>(),
    );
    fs::write(
        repository.root.join("logo.bin"),
        [0x89, b'P', b'N', b'G', 0, 1, 2, 3],
    )
    .unwrap();
    screen
        .state_mut()
        .show_change_review(0, ReviewSource::WorkingTree);
    settle(&mut screen);
    let notes: Vec<_> = view(screen.state())
        .files
        .iter()
        .map(|entry| match entry.patch {
            EntryPatch::Large => "large",
            EntryPatch::Binary => "binary",
            EntryPatch::Parsed(_) => "parsed",
            _ => "other",
        })
        .collect();
    assert_eq!(notes, ["binary", "parsed", "parsed", "parsed", "large"]);
    assert!(
        screen
            .query_by_label("Large diff · 3001 changed lines")
            .is_some()
    );
    assert!(
        screen
            .query_by_label("Binary file · no text diff")
            .is_some()
    );
    visual_tests::snapshot(&mut screen, "vedit_change_review_notes");
    // Load fetches the large patch on request.
    screen.get_by_label("Load").click();
    settle(&mut screen);
    assert!(matches!(
        view(screen.state()).files[4].patch,
        EntryPatch::Parsed(_)
    ));
}

/// Comments for the agent, freshness, unsaved drafts and the diff dialog.
#[path = "change_review_feedback_tests.rs"]
mod feedback_tests;

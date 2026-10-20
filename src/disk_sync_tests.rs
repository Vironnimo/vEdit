//! Disk changes to open documents: undoable reloads, conflicts with unsaved
//! edits, comparisons and the agent-draft state, in isolated fixtures.
use super::*;
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;

struct Fixture(PathBuf);

impl Fixture {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let root = std::env::temp_dir().join(format!(
            "vedit-disk-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).expect("create isolated disk fixture");
        Self(fs::canonicalize(root).expect("canonicalize fixture"))
    }

    fn write(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, text).expect("write fixture file");
        path
    }

    /// An app with one project and `text` open as `notes.txt`.
    fn app(&self, text: &str) -> (VEditApp, u64, PathBuf) {
        let path = self.write("notes.txt", text);
        let ctx = egui::Context::default();
        ctx.set_os(egui::os::OperatingSystem::Windows);
        let mut app = VEditApp::empty(ctx);
        app.recovery_session_path = self.0.join("recovery/session.json");
        app.settings.autosave = false;
        app.add_project(self.0.clone());
        wait_until(&mut app, |app| app.projects.iter().all(|p| !p.indexing));
        app.open_file(path.clone());
        let id = app.tab().expect("opened tab").id;
        // Pending Explorer results would race with tests that read the worker
        // channel directly.
        wait_until(&mut app, |app| {
            !app.tab().unwrap().loading
                && app.projects[0]
                    .explorer
                    .as_ref()
                    .is_none_or(|explorer| explorer.pending_count() == 0)
        });
        app.tab_mut().unwrap().recovery_path = self.0.join(format!("recovery/{id}.txt"));
        (app, id, path)
    }

    /// A buffer with unsaved edits whose file then changed on disk.
    fn conflict(&self) -> (VEditApp, u64, PathBuf) {
        let (mut app, id, path) = self.app("one\ntwo\n");
        change_tab(&mut app, "one\nmine\n");
        change_on_disk(&mut app, id, &path, "one\ndisk\n");
        assert!(app.tab().unwrap().disk_conflict.is_some());
        (app, id, path)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let temporary_root = fs::canonicalize(std::env::temp_dir()).expect("temporary root");
        assert!(self.0.starts_with(&temporary_root));
        assert!(
            self.0
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("vedit-disk-")
        );
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn wait_until(app: &mut VEditApp, condition: impl Fn(&VEditApp) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        app.poll();
        if condition(app) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "background app operation did not finish"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn change_tab(app: &mut VEditApp, text: &str) {
    let tab = app.tab_mut().expect("active tab");
    tab.text = text.into();
    tab.changed();
}

/// Write the file and deliver the periodic disk check for it.
fn change_on_disk(app: &mut VEditApp, id: u64, path: &Path, text: &str) {
    let baseline = app.tab_by_id_mut(id).unwrap().fingerprint.clone().unwrap();
    fs::write(path, text).unwrap();
    let document = workspace::read_document(path, MAX_DOCUMENT).unwrap();
    app.disk_check_running = true;
    app.apply_disk_checks(vec![disk_sync::DiskCheck {
        id,
        path: path.to_path_buf(),
        baseline,
        result: Ok(document),
    }]);
    assert!(!app.disk_check_running);
}

fn status(app: &VEditApp) -> (String, bool) {
    let status = app.status.as_ref().expect("a status message");
    (status.0.clone(), status.1)
}

fn harness(app: VEditApp) -> Harness<'static, VEditApp> {
    let mut harness = Harness::builder()
        .with_size([1200.0, 760.0])
        .with_os(egui::os::OperatingSystem::Windows)
        .build_ui_state(
            |ui, app: &mut VEditApp| {
                app.ctx = ui.ctx().clone();
                app.poll();
                app.shortcuts();
                app.handle_vim();
                app.editor_ui(ui);
                app.overlays();
            },
            app,
        );
    harness.run_steps(2);
    harness
}

fn wait_for_diff(harness: &mut Harness<'static, VEditApp>) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while harness.state().diff_loading {
        assert!(Instant::now() < deadline, "comparison did not finish");
        std::thread::sleep(Duration::from_millis(5));
        harness.step();
    }
    harness.run_steps(2);
}

fn patch_counts(patch: &str) -> (usize, usize) {
    let removed = patch
        .lines()
        .filter(|line| line.starts_with('-') && !line.starts_with("---"))
        .count();
    let added = patch
        .lines()
        .filter(|line| line.starts_with('+') && !line.starts_with("+++"))
        .count();
    (removed, added)
}

#[test]
fn clean_reload_is_one_vim_undo_step_and_reports_the_changed_lines() {
    let fixture = Fixture::new("vim-reload");
    let original = "alpha\nbeta\ngamma\n";
    let (app, id, path) = fixture.app(original);
    let mut screen = harness(app);
    assert!(screen.state().settings.vim);

    change_on_disk(screen.state_mut(), id, &path, "alpha\nBETA\ngamma\ndelta\n");
    let (message, error) = status(screen.state());
    assert!(!error);
    assert_eq!(
        message,
        "Reloaded notes.txt from disk: 2 lines changed · Ctrl+Z restores"
    );
    let tab = screen.state().tab().unwrap();
    assert_eq!(tab.text, "alpha\nBETA\ngamma\ndelta\n");
    assert_eq!(tab.saved_text, tab.text);
    assert!(!tab.dirty);
    assert!(
        tab.disk_reload.as_ref().is_some_and(
            |reload| reload.revision == tab.revision && reload.changed_lines == Some(2)
        )
    );

    screen.step();
    screen.event(egui::Event::Text("u".into()));
    screen.step();
    let tab = screen.state().tab().unwrap();
    assert_eq!(tab.text, original);
    assert!(tab.dirty, "the previous text differs from the file");
}

#[test]
fn clean_reload_is_one_native_undo_step_with_ctrl_z() {
    let fixture = Fixture::new("native-reload");
    let original = "alpha\nbeta\n";
    let (mut app, id, path) = fixture.app(original);
    app.settings.vim = false;
    let mut screen = harness(app);

    change_on_disk(screen.state_mut(), id, &path, "alpha\nchanged\n");
    screen.run_steps(2);
    assert_eq!(screen.state().tab().unwrap().text, "alpha\nchanged\n");

    screen.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
    screen.step();
    assert_eq!(screen.state().tab().unwrap().text, original);
    assert!(screen.state().tab().unwrap().dirty);

    screen.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Y);
    screen.step();
    assert_eq!(screen.state().tab().unwrap().text, "alpha\nchanged\n");
    assert!(!screen.state().tab().unwrap().dirty);
}

#[test]
fn reload_keeps_every_pane_on_the_same_text() {
    let fixture = Fixture::new("pane-reload");
    let original: String = (0..10).map(|line| format!("line {line}\n")).collect();
    let (mut app, id, path) = fixture.app(&original);
    app.bind_current_pane();
    let position = |text: &str, needle: &str| text[..text.find(needle).unwrap()].chars().count();
    let first_pane = app.current_pane_id();
    let target = position(&original, "line 7") + 3;
    app.tab_mut().unwrap().set_cursor(target..target);
    app.split_active(crate::cli::SplitOrientation::Vertical)
        .expect("split the document");
    assert_ne!(app.current_pane_id(), first_pane);
    let focused = position(&original, "line 2") + 5;
    app.tab_mut().unwrap().set_cursor(focused..focused);
    app.capture_pane_navigation();

    let changed = format!(
        "inserted a\ninserted b\n{}",
        original.replace("line 4\n", "")
    );
    change_on_disk(&mut app, id, &path, &changed);

    let tab = app.tab().unwrap();
    assert_eq!(tab.vim.cursor, position(&changed, "line 2") + 5);
    assert_eq!(tab.selection.start, tab.vim.cursor);
    let focus: crate::cli::Request = serde_json::from_value(
        json!({"op": "layout", "change": {"action": "focus", "pane_id": first_pane}}),
    )
    .unwrap();
    assert!(app.control_request(focus).ok);
    assert_eq!(app.current_pane_id(), first_pane);
    assert_eq!(
        app.tab().unwrap().vim.cursor,
        position(&changed, "line 7") + 3
    );
}

#[test]
fn dirty_buffer_change_becomes_a_conflict_that_blocks_only_saving() {
    let fixture = Fixture::new("conflict");
    let (mut app, id, path) = fixture.conflict();
    let tab = app.tab().unwrap();
    assert_eq!(tab.text, "one\nmine\n");
    assert_eq!(tab.saved_text, "one\ntwo\n");
    assert!(tab.dirty);
    assert!(
        tab.error.is_none(),
        "find, jumps and bookmarks stay available"
    );
    assert_eq!(tab.disk_conflict.as_ref().unwrap().disk.text, "one\ndisk\n");
    let (message, error) = status(&app);
    assert!(!error);
    assert!(message.contains("changed on disk"));

    app.save_current();
    assert!(!app.tab().unwrap().saving);
    let (message, error) = status(&app);
    assert!(error);
    assert!(message.contains("Keep my version"));

    app.settings.autosave = true;
    app.tab_mut().unwrap().last_edit = Instant::now() - Duration::from_secs(5);
    app.autosave();
    assert!(!app.tab().unwrap().saving);
    assert_eq!(fs::read_to_string(&path).unwrap(), "one\ndisk\n");

    // A newer disk version replaces the held one without touching the buffer.
    let baseline = app.tab_by_id_mut(id).unwrap().fingerprint.clone().unwrap();
    fs::write(&path, "one\nnewer\n").unwrap();
    let document = workspace::read_document(&path, MAX_DOCUMENT).unwrap();
    app.apply_disk_checks(vec![disk_sync::DiskCheck {
        id,
        path: path.clone(),
        baseline,
        result: Ok(document),
    }]);
    let tab = app.tab().unwrap();
    assert_eq!(tab.text, "one\nmine\n");
    assert_eq!(
        tab.disk_conflict.as_ref().unwrap().disk.text,
        "one\nnewer\n"
    );
}

#[test]
fn use_disk_version_is_one_undo_step_and_cleans_the_buffer() {
    let fixture = Fixture::new("use-disk");
    let (mut app, id, path) = fixture.conflict();
    app.use_disk_version(id);
    let tab = app.tab().unwrap();
    assert_eq!(tab.text, "one\ndisk\n");
    assert_eq!(tab.saved_text, "one\ndisk\n");
    assert!(!tab.dirty && tab.disk_conflict.is_none());
    assert_eq!(
        tab.fingerprint,
        Some(
            workspace::read_document(&path, MAX_DOCUMENT)
                .unwrap()
                .fingerprint
        )
    );
    assert!(status(&app).0.contains("restores your edits"));

    let tab = app.tab_mut().unwrap();
    let result = tab
        .vim
        .handle_key(&mut tab.text, crate::vim::VimKey::Char('u'));
    assert!(result.text_changed);
    tab.changed();
    assert_eq!(tab.text, "one\nmine\n");
    assert!(tab.dirty);
    assert!(tab.disk_conflict.is_none());
}

#[test]
fn keep_my_version_adopts_the_disk_baseline_and_saves_over_it() {
    let fixture = Fixture::new("keep-mine");
    let (mut app, id, path) = fixture.conflict();
    let recovery_generation = app.tab().unwrap().recovery_generation;
    app.keep_my_version(id);
    let disk = workspace::read_document(&path, MAX_DOCUMENT).unwrap();
    let tab = app.tab().unwrap();
    assert_eq!(tab.text, "one\nmine\n");
    assert_eq!(tab.saved_text, "one\ndisk\n");
    assert_eq!(tab.fingerprint, Some(disk.fingerprint));
    assert!(tab.dirty && tab.disk_conflict.is_none());
    assert_ne!(tab.recovery_generation, recovery_generation);
    assert!(status(&app).0.contains("Ctrl+S replaces the disk version"));

    app.save_current();
    wait_until(&mut app, |app| !app.tab().unwrap().saving);
    assert_eq!(fs::read_to_string(&path).unwrap(), "one\nmine\n");
    assert!(!app.tab().unwrap().dirty);
}

#[test]
fn keep_my_version_resumes_autosave_only_for_user_edits() {
    let fixture = Fixture::new("keep-autosave");
    let (mut app, id, path) = fixture.conflict();
    app.settings.autosave = true;
    app.tab_mut().unwrap().agent_unsaved = true;
    app.keep_my_version(id);
    assert!(status(&app).0.contains("Ctrl+S replaces"));
    app.tab_mut().unwrap().last_edit = Instant::now() - Duration::from_secs(5);
    app.autosave();
    assert!(!app.tab().unwrap().saving, "agent drafts wait for a save");

    let (mut app, id, path_2) = Fixture::conflict(&fixture);
    app.settings.autosave = true;
    app.keep_my_version(id);
    assert!(status(&app).0.contains("auto save replaces"));
    app.tab_mut().unwrap().last_edit = Instant::now() - Duration::from_secs(5);
    app.autosave();
    wait_until(&mut app, |app| !app.tab().unwrap().saving);
    assert_eq!(fs::read_to_string(&path_2).unwrap(), "one\nmine\n");
    assert_eq!(path, path_2);
}

#[test]
fn banner_buttons_resolve_the_conflict() {
    let fixture = Fixture::new("banner");
    let (app, id, _) = fixture.conflict();
    let mut screen = harness(app);
    assert!(
        screen
            .query_by_label("notes.txt changed on disk.")
            .is_some()
    );
    screen.get_by_label("Keep my version").click();
    screen.run_steps(2);
    assert!(screen.state().tab().unwrap().disk_conflict.is_none());
    assert!(screen.query_by_label("Keep my version").is_none());

    let (app, _, _) = fixture.conflict();
    let mut screen = harness(app);
    screen.get_by_label("Use disk version").click();
    screen.run_steps(2);
    assert_eq!(screen.state().tab().unwrap().text, "one\ndisk\n");

    let (app, _, _) = fixture.conflict();
    let mut screen = harness(app);
    screen.get_by_label("Save as…").click();
    screen.run_steps(2);
    assert!(
        screen.state().picker.is_some(),
        "Save as… opens the file dialog"
    );
    assert_eq!(
        screen.state().save_as,
        Some(screen.state().tab().unwrap().id)
    );
    let _ = id;
}

#[test]
fn compare_shows_disk_to_buffer_and_escape_closes_it() {
    let fixture = Fixture::new("compare");
    let (app, _, _) = fixture.conflict();
    let mut screen = harness(app);
    screen.get_by_label("Compare").click();
    screen.run_steps(2);
    wait_for_diff(&mut screen);
    let (title, patch) = screen.state().diff.clone().expect("comparison open");
    assert_eq!(title, "notes.txt · disk version → your edits");
    assert!(
        patch.contains("-disk") && patch.contains("+mine"),
        "{patch}"
    );
    assert!(screen.state().comparison.is_some());
    assert!(
        screen
            .query_by_label("− disk version   + your edits")
            .is_some()
    );

    screen.key_press(egui::Key::Escape);
    screen.run_steps(2);
    assert!(screen.state().diff.is_none());
    assert!(screen.state().comparison.is_none());
    assert!(screen.state().tab().unwrap().disk_conflict.is_some());
}

#[test]
fn compare_with_saved_ignores_raw_crlf_and_needs_a_saved_version() {
    let fixture = Fixture::new("compare-saved");
    let (mut app, id, _) = fixture.app("a\nb\n");
    // CLI edits may contain raw CRLF; only the added line is a change.
    change_tab(&mut app, "a\r\nb\r\nc\r\n");
    let mut screen = harness(app);
    screen.state_mut().command_action("Compare with saved");
    screen.run_steps(2);
    wait_for_diff(&mut screen);
    let (title, patch) = screen.state().diff.clone().expect("comparison open");
    assert_eq!(title, "notes.txt · saved → current buffer");
    assert_eq!(patch_counts(&patch), (0, 1), "{patch}");
    screen.key_press(egui::Key::Escape);
    screen.run_steps(2);
    assert!(screen.state().diff.is_none());

    let mut app = screen.into_state();
    app.compare_with_saved(id);
    assert!(app.diff.is_some());
    app.close_diff();
    let untitled = EditorTab::new(None, "Untitled".into());
    let untitled_id = untitled.id;
    app.projects[0].tabs.push(untitled);
    app.compare_with_saved(untitled_id);
    assert!(app.diff.is_none());
    assert!(status(&app).0.contains("no saved version"));
}

#[test]
fn agent_drafts_end_when_the_buffer_is_clean_and_are_not_kept_on_discard() {
    let fixture = Fixture::new("agent-flag");
    let (mut app, _, _) = fixture.app("saved\n");
    change_tab(&mut app, "agent text\n");
    app.tab_mut().unwrap().agent_unsaved = true;
    change_tab(&mut app, "saved\n");
    assert!(!app.tab().unwrap().dirty);
    assert!(
        !app.tab().unwrap().agent_unsaved,
        "Undo to saved ends the draft"
    );

    change_tab(&mut app, "agent text again\n");
    app.tab_mut().unwrap().agent_unsaved = true;
    app.perform_close(CloseAction::Exit);
    let session = app.session();
    assert!(
        session.projects[0]
            .tabs
            .iter()
            .all(|tab| !tab.agent_unsaved)
    );
}

#[test]
fn disk_results_for_a_changed_path_are_ignored() {
    let fixture = Fixture::new("path-bound");
    let (mut app, id, path) = fixture.app("before\n");
    let baseline = app.tab().unwrap().fingerprint.clone().unwrap();
    fs::write(&path, "external\n").unwrap();
    let document = workspace::read_document(&path, MAX_DOCUMENT).unwrap();
    // The tab was renamed after the check started.
    app.tab_mut().unwrap().path = Some(fixture.0.join("renamed.txt"));
    app.apply_disk_checks(vec![disk_sync::DiskCheck {
        id,
        path: path.clone(),
        baseline,
        result: Ok(document),
    }]);
    assert_eq!(app.tab().unwrap().text, "before\n");
    assert!(app.tab().unwrap().disk_reload.is_none());
}

#[test]
fn an_unchanged_conflicted_file_is_not_read_again() {
    let fixture = Fixture::new("conflict-cache");
    let (mut app, _, _) = fixture.conflict();
    app.last_disk_check = Instant::now() - Duration::from_secs(3);
    app.watch_disk();
    assert!(app.disk_check_running);
    let deadline = Instant::now() + Duration::from_secs(8);
    let result = loop {
        if let Ok(result) = app.receiver.try_recv() {
            break result;
        }
        assert!(Instant::now() < deadline, "disk check timed out");
        std::thread::sleep(Duration::from_millis(5));
    };
    assert!(matches!(&result, WorkerResult::DiskChanges(checks) if checks.is_empty()));
}

#[test]
fn a_conflict_undone_to_the_saved_text_follows_the_disk_again() {
    let fixture = Fixture::new("conflict-settle");
    let (mut app, _, _) = fixture.conflict();
    change_tab(&mut app, "one\ntwo\n");
    assert!(!app.tab().unwrap().dirty);
    app.last_disk_check = Instant::now() - Duration::from_secs(3);
    app.watch_disk();
    let tab = app.tab().unwrap();
    assert_eq!(tab.text, "one\ndisk\n");
    assert!(tab.disk_conflict.is_none() && !tab.dirty);
}

#[test]
fn save_as_confirmed_during_a_save_runs_after_it() {
    let fixture = Fixture::new("deferred-save-as");
    let (mut app, id, path) = fixture.app("start\n");
    change_tab(&mut app, "edited\n");
    app.request_save_as(id);
    let request = app.picker.unwrap();
    app.save_current();
    assert!(app.tab().unwrap().saving);
    let destination = fixture.0.join("copy.txt");
    app.finish_native_dialog(
        request,
        Ok(Some(DialogSelection {
            paths: vec![destination.clone()],
            overwrite: None,
        })),
    );
    assert!(status(&app).0.contains("after the current save finishes"));
    wait_until(&mut app, |app| {
        let tab = app.tab().unwrap();
        !tab.saving && tab.path.as_ref() == Some(&destination)
    });
    assert_eq!(fs::read_to_string(&path).unwrap(), "edited\n");
    assert_eq!(fs::read_to_string(&destination).unwrap(), "edited\n");
}

#[test]
fn disk_conflict_banner_screenshot() {
    let mut app = VEditApp::empty(egui::Context::default());
    visual_tests::workspace(&mut app);
    app.bind_current_pane();
    app.sidebar_visible = false;
    let tab = &mut app.projects[0].tabs[0];
    let disk = format!("{}\n// Rewritten by an agent\n", tab.saved_text);
    tab.text.push_str("\n// My unsaved edit\n");
    tab.changed();
    tab.disk_conflict = Some(disk_sync::DiskConflict::new(workspace::LoadedDocument {
        fingerprint: workspace::FileFingerprint {
            len: disk.len() as u64,
            modified: None,
            hash: 1,
        },
        text: disk,
        line_ending: workspace::LineEnding::Lf,
        bom: false,
        mixed_line_endings: false,
    }));
    let mut screen = visual_tests::harness(app);
    visual_tests::snapshot(&mut screen, "vedit_disk_conflict");
}

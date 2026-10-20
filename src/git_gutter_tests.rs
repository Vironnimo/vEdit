//! Gutter change markers: comparison, reverting one change, worker results
//! that no longer belong to the document, and a real repository.
use super::*;

fn hunk(old: std::ops::Range<usize>, new: std::ops::Range<usize>) -> Hunk {
    Hunk { old, new }
}

#[test]
fn crlf_committed_versions_compare_by_line_with_normalized_buffers() {
    let base = "one\r\ntwo\r\nthree\r\nfour\r\n";
    let text = "one\nTWO\nthree\nfour\nfive\n";
    assert_eq!(
        changes(base, text),
        [hunk(1..2, 1..2), hunk(4..4, 4..5)],
        "only the edited and the added line differ"
    );
    assert_eq!(
        changes(base, "one\ntwo\nthree\nfour\n"),
        [],
        "line endings alone are no change"
    );
    assert_eq!(changes(base, "one\nfour\n"), [hunk(1..3, 1..1)]);
    assert_eq!(changes(base, "one\nfour\n")[0].kind(), HunkKind::Removed);
    assert_eq!(changes("", "new\nfile\n"), [hunk(0..0, 0..2)]);
}

#[test]
fn reverting_restores_committed_lines_with_the_buffer_line_endings() {
    let base = "one\r\ntwo\r\nthree\r\nfour\r\n";
    // Modified line in a normalized buffer: the restored line ends in LF.
    let text = "one\nTWO\nthree\nfour\n";
    let (reverted, cursor) = revert_hunk(base, text, &changes(base, text)[0]);
    assert_eq!(reverted, "one\ntwo\nthree\nfour\n");
    assert_eq!(cursor, 4);
    // Raw CRLF from an agent edit keeps CRLF around the restored lines.
    let text = "one\r\nfour\r\n";
    let (reverted, cursor) = revert_hunk(base, text, &changes(base, text)[0]);
    assert_eq!(reverted, "one\r\ntwo\r\nthree\r\nfour\r\n");
    assert_eq!(cursor, 5, "after one\\r\\n");
    // Added lines are removed.
    let text = "zero\none\ntwo\nthree\nfour\n";
    assert_eq!(
        revert_hunk(base, text, &changes(base, text)[0]),
        ("one\ntwo\nthree\nfour\n".to_owned(), 0)
    );
    // Unicode positions are characters, not bytes.
    let base = "β\nä\nend\n";
    let text = "β\n😀\nend\n";
    let (reverted, cursor) = revert_hunk(base, text, &changes(base, text)[0]);
    assert_eq!(reverted, base);
    assert_eq!(cursor, 2);
}

#[test]
fn reverting_at_the_end_of_a_file_without_a_final_line_break() {
    let base = "a\nb\nc\n";
    // Removed last lines after an unterminated last line.
    let (reverted, cursor) = revert_hunk(base, "a", &changes(base, "a")[0]);
    assert_eq!(reverted, "a\nb\nc");
    assert_eq!(cursor, 2);
    // Modified unterminated last line.
    let text = "a\nX";
    let (reverted, _) = revert_hunk(base, text, &changes(base, text)[0]);
    assert_eq!(reverted, "a\nb\nc");
    // Added unterminated last line.
    let text = "a\nb\nc\nd";
    let (reverted, _) = revert_hunk(base, text, &changes(base, text)[0]);
    assert_eq!(reverted, "a\nb\nc\n");
    // Everything removed from an empty buffer.
    let (reverted, cursor) = revert_hunk(base, "", &changes(base, "")[0]);
    assert_eq!(reverted, base);
    assert_eq!(cursor, 0);
}

fn app_with_markers(base: &str, text: &str, vim: bool) -> (VEditApp, u64, Hunk) {
    let mut app = VEditApp::empty(egui::Context::default());
    app.settings.vim = vim;
    let mut project = Project::new(std::env::temp_dir());
    let mut tab = EditorTab::new(None, "fixture.rs".into());
    tab.text = text.into();
    tab.saved_text = text.into();
    let id = tab.id;
    let revision = tab.revision;
    project.tabs.push(tab);
    app.projects.push(project);
    let key = BaseKey {
        repository: std::env::temp_dir(),
        head: Some("0".repeat(40)),
        path: "fixture.rs".into(),
    };
    let base: Arc<str> = base.into();
    let hunks = changes(&base, text);
    let first = hunks[0].clone();
    app.git_gutter.markers.insert(
        id,
        DocumentMarkers {
            key,
            base,
            revision,
            hunks: hunks.into(),
        },
    );
    (app, id, first)
}

#[test]
fn revert_is_one_buffer_edit_that_native_undo_restores() {
    let (mut app, id, change) = app_with_markers("a\nb\nc\n", "a\nB\nB2\nc\n", false);
    app.revert_git_change(id, 0, &change).unwrap();
    let tab = app.tab().unwrap();
    assert_eq!(tab.text, "a\nb\nc\n");
    assert!(tab.dirty, "the buffer changed; disk did not");
    assert_eq!(tab.vim.cursor, 2);
    app.context_native_undo(id, false);
    assert_eq!(app.tab().unwrap().text, "a\nB\nB2\nc\n");
    app.context_native_undo(id, true);
    assert_eq!(app.tab().unwrap().text, "a\nb\nc\n");
}

#[test]
fn revert_is_one_buffer_edit_that_vim_undo_restores() {
    let (mut app, id, change) = app_with_markers("a\nb\nc\n", "a\nc\n", true);
    app.revert_git_change(id, 0, &change).unwrap();
    assert_eq!(app.tab().unwrap().text, "a\nb\nc\n");
    let tab = app.tab_mut().unwrap();
    tab.vim.history_edit(&mut tab.text, false);
    assert_eq!(tab.text, "a\nc\n");
    assert!(!tab.vim.can_undo());
    tab.vim.history_edit(&mut tab.text, true);
    assert_eq!(tab.text, "a\nb\nc\n");
}

#[test]
fn revert_refuses_changes_of_an_older_revision() {
    let (mut app, id, change) = app_with_markers("a\nb\n", "a\nX\n", true);
    let tab = app.tab_mut().unwrap();
    tab.text = "a\nX\nY\n".into();
    tab.changed();
    assert!(app.revert_git_change(id, 0, &change).is_err());
    assert_eq!(app.tab().unwrap().text, "a\nX\nY\n");
}

#[test]
fn worker_comparisons_apply_only_to_their_revision_and_committed_version() {
    let (mut app, id, _) = app_with_markers("a\nb\n", "a\nX\n", true);
    let key = app.git_gutter.markers[&id].key.clone();
    let base = app.git_gutter.markers[&id].base.clone();
    app.git_gutter.bases.insert(
        key.clone(),
        BaseEntry::Ready {
            text: Some(base.clone()),
            committed: true,
        },
    );
    let stale = vec![hunk(0..0, 0..9)];
    // An unknown request is ignored.
    app.apply_git_gutter_diff(id, 7, 0, key.clone(), base.clone(), stale.clone());
    assert_eq!(&*app.git_gutter.markers[&id].hunks, [hunk(1..2, 1..2)]);
    // A comparison of a revision the document has left is discarded.
    app.git_gutter.diffs.insert(id, (8, 0));
    app.tab_mut().unwrap().changed();
    app.apply_git_gutter_diff(id, 8, 0, key.clone(), base.clone(), stale.clone());
    assert_eq!(&*app.git_gutter.markers[&id].hunks, [hunk(1..2, 1..2)]);
    assert!(app.git_gutter.diffs.is_empty());
    // A replaced committed version (a new HEAD read) is discarded too.
    app.git_gutter.diffs.insert(id, (9, 1));
    let other: Arc<str> = "a\nb\n".into();
    app.apply_git_gutter_diff(id, 9, 1, key, other, stale);
    assert_eq!(app.git_gutter.markers[&id].revision, 0);
    // A late committed version for a dropped request is ignored.
    let unknown = BaseKey {
        repository: std::env::temp_dir(),
        head: None,
        path: "other.rs".into(),
    };
    app.apply_git_gutter_base(unknown.clone(), 3, Ok(git::BaseVersion::Added));
    assert!(!app.git_gutter.bases.contains_key(&unknown));
}

/// A temporary repository whose `crlf.txt` is committed with CRLF.
struct Repository {
    root: PathBuf,
}

impl Repository {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "vedit-gutter-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let repository = Self { root };
        repository.run(&["init", "--quiet"]);
        let hooks = repository.root.join(".git").join("vedit-no-hooks");
        fs::create_dir_all(&hooks).unwrap();
        // Repository-local configuration only.
        for (key, value) in [
            ("user.name", "Gutter Author"),
            ("user.email", "gutter@example.invalid"),
            ("core.autocrlf", "false"),
            ("commit.gpgsign", "false"),
            ("core.hooksPath", hooks.to_str().unwrap()),
        ] {
            repository.run(&["config", key, value]);
        }
        fs::write(repository.root.join("crlf.txt"), "one\r\ntwo\r\nthree\r\n").unwrap();
        fs::write(repository.root.join("same.txt"), "same\n").unwrap();
        repository.run(&["add", "--all"]);
        repository.run(&["commit", "--quiet", "-m", "Initial"]);
        fs::write(repository.root.join("untracked.txt"), "new\n").unwrap();
        repository
    }

    fn run(&self, args: &[&str]) {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{args:?}");
    }

    fn app(&self, files: &[(&str, &str)]) -> (VEditApp, Vec<u64>) {
        let mut app = VEditApp::empty(egui::Context::default());
        let mut project = Project::new(self.root.clone());
        let (head, fingerprint) = git::working_tree_fingerprint(&self.root).unwrap();
        project.git_root = Some(self.root.clone());
        project.git_head = head;
        project.git_fingerprint = Some(fingerprint);
        let mut ids = Vec::new();
        for (name, text) in files {
            let mut tab = EditorTab::new(Some(self.root.join(name)), (*name).into());
            tab.text = (*text).into();
            tab.saved_text = (*text).into();
            ids.push(tab.id);
            project.tabs.push(tab);
        }
        app.projects.push(project);
        (app, ids)
    }
}

impl Drop for Repository {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Ask for markers until the committed version and comparison arrived.
fn settled_markers(app: &mut VEditApp, document: u64) -> Option<GutterMarkers> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let markers = app.git_gutter_markers(document);
        let pending = app
            .git_gutter
            .bases
            .values()
            .any(|entry| matches!(entry, BaseEntry::Pending(_)))
            || !app.git_gutter.diffs.is_empty();
        if !pending {
            return app.git_gutter_markers(document);
        }
        assert!(Instant::now() < deadline, "gutter workers timed out");
        drop(markers);
        std::thread::sleep(Duration::from_millis(5));
        app.poll();
    }
}

#[test]
fn repository_files_get_markers_from_head_and_untracked_files_none() {
    let repository = Repository::new();
    let (mut app, ids) = repository.app(&[
        ("crlf.txt", "one\nTWO\nthree\n"),
        ("same.txt", "same\n"),
        ("untracked.txt", "new\n"),
    ]);
    let markers = settled_markers(&mut app, ids[0]).unwrap();
    assert!(markers.current);
    assert_eq!(&*markers.hunks, [hunk(1..2, 1..2)]);
    assert_eq!(&*markers.base, "one\r\ntwo\r\nthree\r\n");
    assert!(settled_markers(&mut app, ids[1]).unwrap().hunks.is_empty());
    assert!(settled_markers(&mut app, ids[2]).is_none());
    // Unsaved edits are compared too, and the setting turns markers off.
    let tab = app.tab_by_id_mut(ids[0]).unwrap();
    tab.text.push_str("four\n");
    tab.changed();
    let markers = settled_markers(&mut app, ids[0]).unwrap();
    assert_eq!(&*markers.hunks, [hunk(1..2, 1..2), hunk(3..3, 3..4)]);
    app.settings.git_gutter = false;
    assert!(app.git_gutter_markers(ids[0]).is_none());
}

#[test]
fn a_new_status_drops_versions_that_depend_on_the_index() {
    let repository = Repository::new();
    let (mut app, ids) = repository.app(&[("untracked.txt", "new\n")]);
    assert!(settled_markers(&mut app, ids[0]).is_none());
    // Staging the file makes it an added file: every line is new.
    repository.run(&["add", "untracked.txt"]);
    let (_, fingerprint) = git::working_tree_fingerprint(&repository.root).unwrap();
    app.projects[0].git_fingerprint = Some(fingerprint);
    let markers = settled_markers(&mut app, ids[0]).unwrap();
    assert_eq!(&*markers.hunks, [hunk(0..0, 0..1)]);
}

/// Manual check of [`INLINE_DIFF_BYTES`]: run with
/// `cargo test --release --bin vedit inline_comparison_budget -- --ignored --nocapture`.
#[test]
#[ignore = "manual release benchmark"]
fn inline_comparison_budget() {
    for total in [
        INLINE_DIFF_BYTES / 2,
        INLINE_DIFF_BYTES,
        INLINE_DIFF_BYTES * 4,
    ] {
        let mut base = String::new();
        for line in 0.. {
            let line = format!("    let value_{line} = compute({line}) + offset;\n");
            if base.len() + line.len() > total / 2 {
                break;
            }
            base.push_str(&line);
        }
        let scattered: String = base
            .lines()
            .enumerate()
            .map(|(index, line)| {
                if index % 97 == 0 {
                    format!("{line} // edited\n")
                } else {
                    format!("{line}\n")
                }
            })
            .collect();
        let rewritten: String = base.lines().map(|line| format!("{line}!\n")).collect();
        for (label, text) in [("scattered", &scattered), ("rewritten", &rewritten)] {
            let fastest = (0..5)
                .map(|_| {
                    let started = Instant::now();
                    std::hint::black_box(changes(&base, text));
                    started.elapsed()
                })
                .min()
                .unwrap();
            println!(
                "{} KiB {label}: {} lines, fastest of 5 {fastest:?}",
                total / 1024,
                base.lines().count()
            );
        }
    }
}

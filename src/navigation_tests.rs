//! Exercise navigation through the app's real input dispatch and source layout.

use super::*;
use egui_kittest::{Harness, kittest::Queryable};

const DOCUMENT: &str = "intro 🦀\nalpha gi first\nbeta GI second\nthird gi marker\nlast line\n";

fn fixture(document: &str) -> VEditApp {
    let mut app = VEditApp::empty(egui::Context::default());
    app.settings.autosave = false;
    app.settings.smooth_scroll = false;
    app.sidebar_visible = false;
    let root = std::env::temp_dir().join("vedit-navigation-render-only");
    let mut project = Project::new(root.clone());
    project.last_git_refresh = Instant::now();
    let mut tab = EditorTab::new(Some(root.join("notes.txt")), "notes.txt".into());
    tab.text = document.into();
    tab.saved_text = document.into();
    project.tabs.push(tab);
    app.projects.push(project);
    app.bind_current_pane();
    app
}

fn harness(app: VEditApp) -> Harness<'static, VEditApp> {
    let mut screen = Harness::builder()
        .with_size([1360.0, 900.0])
        .with_os(egui::os::OperatingSystem::Windows)
        .build_ui_state(
            |ui, app: &mut VEditApp| {
                app.ctx = ui.ctx().clone();
                // Render-only fixtures never run disk/session/recovery writes.
                // Loaded-file workers remain available to the isolated temp test.
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

fn text(screen: &mut Harness<'_, VEditApp>, characters: &str) {
    screen.event(egui::Event::Text(characters.into()));
    screen.run_steps(2);
}

fn character(screen: &mut Harness<'_, VEditApp>, key: Key, character: &str) {
    let event = |pressed| egui::Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    screen.input_mut().events.extend([
        event(true),
        egui::Event::Text(character.into()),
        event(false),
    ]);
    screen.run_steps(2);
}

fn cursor(screen: &mut Harness<'_, VEditApp>, position: usize) {
    screen
        .state_mut()
        .tab_mut()
        .unwrap()
        .set_cursor(position..position);
    screen.run_steps(2);
}

fn assert_unchanged(screen: &Harness<'_, VEditApp>, document: &str) {
    let tab = screen.state().tab().unwrap();
    assert_eq!(tab.text, document);
    assert!(!tab.dirty);
    assert!(
        !tab.vim.can_undo(),
        "navigation must not create an edit transaction"
    );
}

fn gi_positions(document: &str) -> Vec<usize> {
    document
        .chars()
        .collect::<Vec<_>>()
        .windows(2)
        .enumerate()
        .filter_map(|(index, characters)| {
            (characters[0].eq_ignore_ascii_case(&'g') && characters[1].eq_ignore_ascii_case(&'i'))
                .then_some(index)
        })
        .collect()
}

fn begin_jump(screen: &mut Harness<'_, VEditApp>) {
    text(screen, "  f");
    let jump = screen
        .state()
        .screen_jump
        .as_ref()
        .expect("Space Space f opens the screen jump");
    assert!(
        !jump.glyphs.is_empty(),
        "source painting must cache actual visible glyphs"
    );
    assert_eq!(jump.tab_id, screen.state().tab().unwrap().id);
    assert_eq!(jump.pane_id, screen.state().current_pane_id());
}

#[test]
fn space_m_creates_persistent_unicode_bookmark_without_an_edit() {
    let mut screen = harness(fixture(DOCUMENT));
    let position = "intro 🦀\nalpha ".chars().count();
    cursor(&mut screen, position);
    character(&mut screen, Key::Space, " ");
    character(&mut screen, Key::M, "m");
    assert_eq!(screen.state().bookmarks.items.len(), 1);
    let bookmark = &screen.state().bookmarks.items[0];
    assert_eq!((bookmark.line, bookmark.column), (2, 7));
    assert_eq!(bookmark.context_line, "alpha gi first");
    assert!(screen.state().session_dirty);
    assert_unchanged(&screen, DOCUMENT);
    let bytes = serde_json::to_vec(&screen.state().session()).unwrap();
    let persisted: Session = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(persisted.bookmarks, screen.state().bookmarks.items);
    // Persistent bookmarks belong to the app, including closed files/projects.
    screen.state_mut().projects.clear();
    let closed: Session =
        serde_json::from_slice(&serde_json::to_vec(&screen.state().session()).unwrap()).unwrap();
    assert_eq!(closed.bookmarks, persisted.bookmarks);
    assert!(
        serde_json::from_str::<Session>(r#"{"projects":[]}"#)
            .unwrap()
            .bookmarks
            .is_empty()
    );
}

#[test]
fn space_b_then_enter_restores_the_exact_character_position() {
    let document = "heading\na🦀é target\nlast\n";
    let mut screen = harness(fixture(document));
    let position = "heading\na🦀".chars().count();
    cursor(&mut screen, position);
    text(&mut screen, " m");
    let id = screen.state().bookmarks.items[0].id;
    cursor(&mut screen, 0);
    text(&mut screen, " b");
    assert!(screen.state().navigation_modal_open());
    screen.key_press(Key::Enter);
    screen.run_steps(3);
    assert!(!screen.state().navigation_modal_open());
    assert_eq!(screen.state().tab().unwrap().vim.cursor, position);
    assert_eq!(screen.state().bookmarks.get(id).unwrap().column, 3);
    assert_unchanged(&screen, document);
}

#[test]
fn clicking_a_bookmark_row_restores_source_view_and_cursor() {
    let mut screen = harness(fixture(DOCUMENT));
    let position = gi_positions(DOCUMENT)[2];
    cursor(&mut screen, position);
    text(&mut screen, " m");
    let id = screen.state().bookmarks.items[0].id;
    screen.state_mut().bookmarks.set_label(id, "Review target");
    cursor(&mut screen, 0);
    text(&mut screen, " b");
    screen.get_by_label_contains("Review target:").click();
    screen.run_steps(3);
    assert!(!screen.state().navigation_modal_open());
    assert_eq!(screen.state().tab().unwrap().vim.cursor, position);
    assert_eq!(
        screen.state().tab().unwrap().agent_view,
        crate::cli::DocumentView::Source
    );
    assert_unchanged(&screen, DOCUMENT);
}

#[test]
fn literal_find_space_and_insert_spaces_survive_leader_dispatch() {
    let mut screen = harness(fixture(DOCUMENT));
    text(&mut screen, "f ");
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 5);
    assert!(!screen.state().navigation_pending());
    assert_unchanged(&screen, DOCUMENT);
    cursor(&mut screen, 0);
    text(&mut screen, "i  ");
    assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::Insert);
    assert_eq!(screen.state().tab().unwrap().text, format!("  {DOCUMENT}"));
    assert!(!screen.state().navigation_pending());
    screen.key_press(Key::Escape);
    screen.run_steps(2);
    assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::Normal);
}

#[test]
fn unknown_leader_suffix_remains_a_vim_motion_and_escape_restores_focus() {
    let mut screen = harness(fixture(DOCUMENT));
    text(&mut screen, " j");
    assert_eq!(
        screen.state().tab().unwrap().vim.cursor,
        "intro 🦀\n".chars().count()
    );
    assert!(!screen.state().navigation_pending());
    text(&mut screen, " ");
    assert!(screen.state().navigation_pending());
    screen.key_press(Key::Escape);
    screen.run_steps(2);
    text(&mut screen, "l");
    assert_eq!(
        screen.state().tab().unwrap().vim.cursor,
        "intro 🦀\na".chars().count()
    );
    assert_unchanged(&screen, DOCUMENT);
}

#[test]
fn enter_jumps_to_a_single_visible_match() {
    let document = "intro\nonly target\n";
    let mut screen = harness(fixture(document));
    begin_jump(&mut screen);
    text(&mut screen, "tar");
    screen.key_press(Key::Enter);
    screen.run_steps(2);
    assert!(screen.state().screen_jump.is_none());
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 11);
    assert_unchanged(&screen, document);
}

#[test]
fn visible_matches_have_home_row_labels_and_jump_without_changing_the_buffer() {
    let mut screen = harness(fixture(DOCUMENT));
    begin_jump(&mut screen);
    text(&mut screen, "gi");
    let jump = screen.state().screen_jump.as_ref().unwrap();
    let targets: Vec<_> = jump
        .state
        .targets
        .iter()
        .map(|target| target.char_index)
        .collect();
    assert_eq!(targets, gi_positions(DOCUMENT));
    assert_eq!(
        jump.state
            .targets
            .iter()
            .map(|target| target.label.as_str())
            .collect::<Vec<_>>(),
        vec!["f", "j", "d"]
    );
    character(&mut screen, Key::J, "j");
    assert!(screen.state().screen_jump.is_none());
    assert_eq!(screen.state().tab().unwrap().vim.cursor, targets[1]);
    assert_unchanged(&screen, DOCUMENT);
}

#[test]
fn query_and_label_in_one_input_batch_use_the_current_visible_glyph_cache() {
    let mut screen = harness(fixture(DOCUMENT));
    begin_jump(&mut screen);
    text(&mut screen, "gidi accidental text");
    assert!(screen.state().screen_jump.is_none());
    assert_eq!(
        screen.state().tab().unwrap().vim.cursor,
        gi_positions(DOCUMENT)[2]
    );
    assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::Normal);
    assert_unchanged(&screen, DOCUMENT);
}

#[test]
fn real_key_events_refine_the_query_and_lock_after_three_characters() {
    let document = "intro\ngif gif\n";
    let mut screen = harness(fixture(document));
    begin_jump(&mut screen);
    for (key, value) in [(Key::G, "g"), (Key::I, "i"), (Key::F, "f")] {
        character(&mut screen, key, value);
    }
    let jump = &screen.state().screen_jump.as_ref().unwrap().state;
    assert_eq!(jump.query, "gif");
    assert_eq!(jump.phase, crate::screen_jump::JumpPhase::Select);
    assert!(jump.hint().contains("type a label"));
    character(&mut screen, Key::J, "j");
    assert!(screen.state().screen_jump.is_none());
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 10);
    assert_unchanged(&screen, document);
}

#[test]
fn translated_punctuation_and_ime_are_search_text_not_duplicate_keys() {
    let document = "intro\n!ö target\n";
    let mut screen = harness(fixture(document));
    begin_jump(&mut screen);
    screen.input_mut().events.extend([
        egui::Event::Key {
            key: Key::Num1,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::SHIFT,
        },
        egui::Event::Text("!".into()),
        egui::Event::Key {
            key: Key::O,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        },
        egui::Event::Ime(egui::ImeEvent::Preedit {
            text: "ö".into(),
            active_range_chars: None,
        }),
        egui::Event::Ime(egui::ImeEvent::Commit("ö".into())),
    ]);
    screen.run_steps(2);
    assert_eq!(
        screen.state().screen_jump.as_ref().unwrap().state.query,
        "!ö"
    );
    screen.key_press(Key::Enter);
    screen.run_steps(2);
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 6);
    assert!(screen.state().screen_jump.is_none());
    assert_unchanged(&screen, document);
}

#[test]
fn enter_unlocks_labels_when_all_letters_are_search_continuations() {
    let document: String = ('a'..='z').map(|key| format!("a{key} ")).collect();
    let mut screen = harness(fixture(&document));
    begin_jump(&mut screen);
    text(&mut screen, "a");
    assert!(
        screen
            .state()
            .screen_jump
            .as_ref()
            .unwrap()
            .state
            .hint()
            .contains("Enter for labels")
    );
    screen.key_press(Key::Enter);
    screen.run_steps(2);
    let jump = &screen.state().screen_jump.as_ref().unwrap().state;
    assert_eq!(jump.phase, crate::screen_jump::JumpPhase::Select);
    let label = jump.targets[1].label.clone();
    let position = jump.targets[1].char_index;
    text(&mut screen, &label);
    assert!(screen.state().screen_jump.is_none());
    assert_eq!(screen.state().tab().unwrap().vim.cursor, position);
    assert_unchanged(&screen, &document);
}

#[test]
fn backspace_corrects_pairs_and_queries_without_losing_editor_focus() {
    let document = "gi \n".repeat(30);
    let mut screen = harness(fixture(&document));
    begin_jump(&mut screen);
    text(&mut screen, "gi");
    let label = screen.state().screen_jump.as_ref().unwrap().state.targets[27]
        .label
        .clone();
    assert_eq!(label.len(), 2);
    text(&mut screen, &label[..1]);
    assert!(
        screen
            .state()
            .screen_jump
            .as_ref()
            .unwrap()
            .state
            .hint()
            .contains("type the next letter")
    );
    screen.key_press(Key::Backspace);
    screen.run_steps(2);
    let jump = &screen.state().screen_jump.as_ref().unwrap().state;
    assert!(jump.label_input.is_empty());
    assert_eq!(jump.query, "gi");
    screen.key_press(Key::Backspace);
    screen.run_steps(2);
    assert_eq!(
        screen.state().screen_jump.as_ref().unwrap().state.query,
        "g"
    );
    text(&mut screen, "i");
    let jump = &screen.state().screen_jump.as_ref().unwrap().state;
    let label = jump.targets[27].label.clone();
    let position = jump.targets[27].char_index;
    text(&mut screen, &label);
    assert!(screen.state().screen_jump.is_none());
    assert_eq!(screen.state().tab().unwrap().vim.cursor, position);
    text(&mut screen, "l");
    assert_eq!(screen.state().tab().unwrap().vim.cursor, position + 1);
    assert_unchanged(&screen, &document);
}

#[test]
fn pasted_query_and_label_preserve_raw_crlf_and_visual_selection() {
    let document = "intro\r\ngi first\r\ngi second\r\n";
    let mut screen = harness(fixture(document));
    text(&mut screen, "v");
    begin_jump(&mut screen);
    screen.input_mut().events.extend([
        egui::Event::Paste("gij".into()),
        egui::Event::Cut,
        egui::Event::Paste("accidental text".into()),
    ]);
    screen.run_steps(2);
    assert!(screen.state().screen_jump.is_none());
    let tab = screen.state().tab().unwrap();
    assert_eq!(tab.vim.mode, VimMode::Visual);
    assert_eq!(tab.vim.cursor, 17);
    assert_eq!(tab.selection, 0..18);
    assert_unchanged(&screen, document);
}

#[test]
fn screen_jump_extends_visual_line_selection_from_its_original_anchor() {
    let mut screen = harness(fixture(DOCUMENT));
    text(&mut screen, "V");
    assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::VisualLine);
    begin_jump(&mut screen);
    text(&mut screen, "gid");
    let tab = screen.state().tab().unwrap();
    assert_eq!(tab.vim.mode, VimMode::VisualLine);
    assert_eq!(
        tab.selection,
        0.."intro 🦀\nalpha gi first\nbeta GI second\nthird gi marker\n"
            .chars()
            .count()
    );
    assert_unchanged(&screen, DOCUMENT);
}

#[test]
fn wrapped_split_jump_reads_only_the_focused_panes_source() {
    let mut app = fixture(DOCUMENT);
    let side_document = format!("gi {}gi wrapped end\nGI side end\n", "word ".repeat(70));
    let root = app.projects[0].root.clone();
    let mut side = EditorTab::new(Some(root.join("side.txt")), "side.txt".into());
    side.text = side_document.clone();
    side.saved_text = side_document.clone();
    app.projects[0].tabs.push(side);
    app.settings.word_wrap = true;
    app.split_command("right");
    app.activate_tab(0, 1);
    let focused = app.current_pane_id();
    let mut screen = harness(app);
    begin_jump(&mut screen);
    text(&mut screen, "gi");
    let jump = screen.state().screen_jump.as_ref().unwrap();
    assert_eq!(jump.pane_id, focused);
    assert_eq!(
        jump.state
            .targets
            .iter()
            .map(|target| target.char_index)
            .collect::<Vec<_>>(),
        gi_positions(&side_document)
    );
    let pane_rect = screen
        .state()
        .editor_rect
        .expect("focused source rectangle");
    assert!(
        jump.state
            .targets
            .iter()
            .all(|target| pane_rect.contains(target.rect.center()))
    );
    let label = jump.state.targets[2].label.clone();
    text(&mut screen, &label);
    assert_eq!(
        screen.state().tab().unwrap().vim.cursor,
        gi_positions(&side_document)[2]
    );
    assert_eq!(screen.state().projects[0].tabs[0].text, DOCUMENT);
    assert_unchanged(&screen, &side_document);
}

#[test]
fn horizontally_and_vertically_hidden_matches_are_excluded() {
    let document = format!(
        "{}gi horizontal hidden\ngi visible\n{}",
        "x".repeat(600),
        "ordinary filler\n".repeat(100)
    ) + "gi vertical hidden\n";
    let mut screen = harness(fixture(&document));
    begin_jump(&mut screen);
    text(&mut screen, "gi");
    let jump = screen.state().screen_jump.as_ref().unwrap();
    let positions = gi_positions(&document);
    assert_eq!(
        jump.state
            .targets
            .iter()
            .map(|target| target.char_index)
            .collect::<Vec<_>>(),
        vec![positions[1]]
    );
    let label = jump.state.targets[0].label.clone();
    text(&mut screen, &label);
    assert_eq!(screen.state().tab().unwrap().vim.cursor, positions[1]);
    assert_unchanged(&screen, &document);
}

#[test]
fn escape_cancels_a_query_and_resumes_normal_vim_input() {
    let mut screen = harness(fixture(DOCUMENT));
    begin_jump(&mut screen);
    text(&mut screen, "gi");
    screen.key_press(Key::Escape);
    screen.run_steps(2);
    assert!(screen.state().screen_jump.is_none());
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 0);
    text(&mut screen, "l");
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 1);
    assert_unchanged(&screen, DOCUMENT);
}

struct TemporaryDocument(PathBuf);

impl TemporaryDocument {
    fn new(document: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let folder = std::env::temp_dir().join(format!(
            "vedit-navigation-load-test-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&folder).unwrap();
        fs::write(folder.join("closed ü.txt"), document).unwrap();
        Self(folder)
    }

    fn file(&self) -> PathBuf {
        fs::canonicalize(self.0.join("closed ü.txt")).unwrap()
    }
}

impl Drop for TemporaryDocument {
    fn drop(&mut self) {
        assert_eq!(self.0.parent(), Some(std::env::temp_dir().as_path()));
        assert!(
            self.0
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("vedit-navigation-load-test-")
        );
        for _ in 0..3 {
            if fs::remove_dir_all(&self.0).is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

#[test]
fn bookmark_of_a_closed_file_waits_for_loading_then_resolves_its_context() {
    let document = "inserted\nheading\na🦀é target\nlast\n";
    let temporary = TemporaryDocument::new(document);
    let path = temporary.file();
    let mut app = fixture("different open file\n");
    let bookmark = crate::bookmarks::Bookmark {
        id: 90,
        path: path.clone(),
        line: 2,
        column: 3,
        label: "Closed file".into(),
        context_line: "a🦀é target".into(),
    };
    app.bookmarks = crate::bookmarks::BookmarkStore::from_items(vec![bookmark]);
    let mut screen = harness(app);
    screen.state_mut().open_bookmarks();
    screen.run_steps(2);
    screen.get_by_label("All projects").click();
    screen.run_steps(2);
    screen.get_by_label_contains("Closed file:").click();
    // Harness clicks enqueue pointer events. Dispatch the click before checking
    // the load state, otherwise the still-active original tab satisfies the
    // completion condition and the async read only starts after the wait ends.
    screen.run_steps(1);
    assert!(!screen.state().navigation_modal_open());
    assert_eq!(screen.state().tab().unwrap().path.as_ref(), Some(&path));
    let deadline = Instant::now() + Duration::from_secs(5);
    while screen.state().pending_bookmark_jump.is_some()
        || screen.state().tab().is_some_and(|tab| tab.loading)
    {
        assert!(
            Instant::now() < deadline,
            "closed bookmarked file did not finish loading"
        );
        std::thread::sleep(Duration::from_millis(2));
        screen.run_steps(1);
    }
    screen.run_steps(2);
    let tab = screen.state().tab().unwrap();
    assert_eq!(tab.path.as_ref(), Some(&path));
    assert_eq!(tab.text, document);
    assert_eq!(tab.vim.cursor, "inserted\nheading\na🦀".chars().count());
    assert!(screen.state().pending_bookmark_jump.is_none());
    assert!(!screen.state().navigation_modal_open());
    assert_eq!(fs::read_to_string(&path).unwrap(), document);
    assert_unchanged(&screen, document);
}

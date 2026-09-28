//! Desktop undo keys must use Vim history and respect input order and ownership.

use super::*;
use context_actions::EditorAction;
use egui_kittest::Harness;

const PRESERVED_REGISTER: &str = "KEEP🦀";

fn fixture(document: &str) -> MorrowApp {
    let mut app = MorrowApp::empty(egui::Context::default());
    app.settings.autosave = false;
    app.settings.vim = true;
    app.settings.smooth_scroll = false;
    app.sidebar_visible = false;
    let root = std::env::temp_dir().join("morrow-windows-undo-render-only");
    let mut project = Project::new(root.clone());
    project.last_git_refresh = Instant::now();
    project.index = Some(Arc::new(workspace::WorkspaceIndex::empty(root)));
    let mut tab = EditorTab::new(None, "notes.txt".into());
    tab.text = document.into();
    tab.saved_text = document.into();
    let register = Yank {
        text: PRESERVED_REGISTER.into(),
        linewise: false,
    };
    tab.vim.set_register(register.clone());
    app.register = Some(register);
    project.tabs.push(tab);
    app.projects.push(project);
    app.bind_current_pane();
    app
}

fn harness(app: MorrowApp) -> Harness<'static, MorrowApp> {
    let mut screen = Harness::builder()
        .with_size([1200.0, 760.0])
        .with_step_dt(1.0 / 60.0)
        .with_os(egui::os::OperatingSystem::Windows)
        .build_ui_state(
            |ui, app: &mut MorrowApp| {
                app.ctx = ui.ctx().clone();
                app.draw_ui(ui);
            },
            app,
        );
    set_style(&screen.ctx);
    screen.ctx.all_styles_mut(|style| {
        style.animation_time = 0.0;
        style.scroll_animation = egui::style::ScrollAnimation::none();
        style.visuals.text_cursor.blink = false;
    });
    screen.run_steps(3);
    screen
}

fn events(screen: &mut Harness<'_, MorrowApp>, events: impl IntoIterator<Item = egui::Event>) {
    screen.input_mut().events.extend(events);
    screen.run_steps(1);
    super::editor_input_tests::settle(screen);
}

fn text(screen: &mut Harness<'_, MorrowApp>, value: &str) {
    events(screen, [egui::Event::Text(value.into())]);
}

fn windows_modifiers(modifiers: egui::Modifiers) -> egui::Modifiers {
    egui::Modifiers {
        command: modifiers.ctrl,
        ..modifiers
    }
}

fn key_event(key: Key, pressed: bool, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers,
    }
}

fn shortcut(screen: &mut Harness<'_, MorrowApp>, key: Key, modifiers: egui::Modifiers) {
    let modifiers = windows_modifiers(modifiers);
    events(
        screen,
        [
            egui::Event::ModifiersChanged(modifiers),
            key_event(key, true, modifiers),
        ],
    );
    events(
        screen,
        [
            key_event(key, false, modifiers),
            egui::Event::ModifiersChanged(egui::Modifiers::NONE),
        ],
    );
}

fn escape(screen: &mut Harness<'_, MorrowApp>) {
    shortcut(screen, Key::Escape, egui::Modifiers::NONE);
}

fn insert_transaction(screen: &mut Harness<'_, MorrowApp>, value: &str) {
    text(screen, &format!("i{value}"));
    escape(screen);
}

fn assert_registers_preserved(app: &MorrowApp) {
    let register = app.register.as_ref().expect("global unnamed register");
    assert_eq!(register.text, PRESERVED_REGISTER);
    assert!(!register.linewise);
    for tab in app.projects.iter().flat_map(|project| &project.tabs) {
        let register = tab.vim.register().expect("document unnamed register");
        assert_eq!(register.text, PRESERVED_REGISTER, "tab {}", tab.id);
        assert!(!register.linewise, "tab {}", tab.id);
    }
}

fn assert_document(screen: &Harness<'_, MorrowApp>, expected: &str) {
    assert_eq!(screen.state().tab().unwrap().text, expected);
    assert_registers_preserved(screen.state());
}

fn document_by_id(app: &MorrowApp, id: u64) -> &EditorTab {
    app.projects
        .iter()
        .flat_map(|project| &project.tabs)
        .find(|tab| tab.id == id)
        .expect("document remains open")
}

#[test]
fn windows_undo_and_both_redo_shortcuts_work_in_every_vim_editor_mode() {
    let original = "αβ🦀tail\n";
    let changed = "δ😀αβ🦀tail\n";
    for mode in [
        VimMode::Normal,
        VimMode::Visual,
        VimMode::VisualLine,
        VimMode::Insert,
    ] {
        for (redo_key, redo_modifiers) in [
            (Key::Y, egui::Modifiers::CTRL),
            (Key::Z, egui::Modifiers::CTRL.plus(egui::Modifiers::SHIFT)),
        ] {
            let mut screen = harness(fixture(original));
            text(&mut screen, "iδ😀");
            if mode != VimMode::Insert {
                escape(&mut screen);
            }
            match mode {
                VimMode::Visual => text(&mut screen, "vll"),
                VimMode::VisualLine => text(&mut screen, "V"),
                _ => {}
            }
            assert_eq!(screen.state().tab().unwrap().vim.mode, mode);
            assert_document(&screen, changed);

            shortcut(&mut screen, Key::Z, egui::Modifiers::CTRL);
            assert_document(&screen, original);
            assert!(!screen.state().tab().unwrap().dirty);
            if mode == VimMode::Insert {
                assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::Insert);
            }

            // Undo restores Normal; enter Visual again to exercise redo there too.
            match mode {
                VimMode::Visual => text(&mut screen, "vll"),
                VimMode::VisualLine => text(&mut screen, "V"),
                _ => {}
            }
            if matches!(mode, VimMode::Visual | VimMode::VisualLine) {
                assert_eq!(screen.state().tab().unwrap().vim.mode, mode);
            }

            shortcut(&mut screen, redo_key, redo_modifiers);
            assert_document(&screen, changed);
            assert!(screen.state().tab().unwrap().dirty);
            if mode == VimMode::Insert {
                assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::Insert);
            }
        }
    }
}

#[test]
fn windows_and_vim_undo_redo_traverse_one_document_history() {
    let original = "tail 🦀\n";
    let mut screen = harness(fixture(original));
    insert_transaction(&mut screen, "A");
    text(&mut screen, "0");
    insert_transaction(&mut screen, "B");
    assert_document(&screen, "BAtail 🦀\n");

    shortcut(&mut screen, Key::Z, egui::Modifiers::CTRL);
    assert_document(&screen, "Atail 🦀\n");
    shortcut(&mut screen, Key::R, egui::Modifiers::CTRL);
    assert_document(&screen, "BAtail 🦀\n");
    text(&mut screen, "u");
    assert_document(&screen, "Atail 🦀\n");
    shortcut(&mut screen, Key::Y, egui::Modifiers::CTRL);
    assert_document(&screen, "BAtail 🦀\n");

    shortcut(&mut screen, Key::Z, egui::Modifiers::CTRL);
    assert_document(&screen, "Atail 🦀\n");
    text(&mut screen, "u");
    assert_document(&screen, original);
    assert!(!screen.state().tab().unwrap().dirty);
    shortcut(
        &mut screen,
        Key::Z,
        egui::Modifiers::CTRL.plus(egui::Modifiers::SHIFT),
    );
    assert_document(&screen, "Atail 🦀\n");
    shortcut(&mut screen, Key::R, egui::Modifiers::CTRL);
    assert_document(&screen, "BAtail 🦀\n");
}

#[test]
fn same_frame_insert_prefix_undo_suffix_keeps_order_and_starts_a_new_transaction() {
    let original = "tail\n";
    let mut screen = harness(fixture(original));
    text(&mut screen, "i");
    let ctrl = windows_modifiers(egui::Modifiers::CTRL);
    events(
        &mut screen,
        [
            egui::Event::Text("PRE/dd/🦀".into()),
            egui::Event::ModifiersChanged(ctrl),
            key_event(Key::Z, true, ctrl),
            key_event(Key::Z, false, ctrl),
            egui::Event::ModifiersChanged(egui::Modifiers::NONE),
            egui::Event::Text("SUF😀".into()),
        ],
    );
    assert_document(&screen, "SUF😀tail\n");
    let tab = screen.state().tab().unwrap();
    let suffix_length = "SUF😀".chars().count();
    assert_eq!(tab.vim.mode, VimMode::Insert);
    assert_eq!(tab.vim.cursor, suffix_length);
    assert_eq!(tab.selection, suffix_length..suffix_length);

    shortcut(&mut screen, Key::Z, egui::Modifiers::CTRL);
    assert_document(&screen, original);
    assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::Insert);
    shortcut(&mut screen, Key::Z, egui::Modifiers::CTRL);
    assert_document(&screen, original);
    shortcut(&mut screen, Key::Y, egui::Modifiers::CTRL);
    assert_document(&screen, "SUF😀tail\n");
    escape(&mut screen);
    text(&mut screen, "u");
    assert_document(&screen, original);
}

#[test]
fn redo_keeps_unicode_insert_caret_at_line_end_and_eof() {
    for (tail, line_ending, paste_crlf) in [
        ("", workspace::LineEnding::Lf, false),
        ("", workspace::LineEnding::CrLf, false),
        ("\nnext 🦀\n", workspace::LineEnding::Lf, false),
        ("\nnext 🦀\n", workspace::LineEnding::CrLf, false),
        ("\nnext 🦀\n", workspace::LineEnding::CrLf, true),
    ] {
        let original = format!("α🦀{tail}");
        let changed = format!("α🦀δ😀{tail}");
        let mut app = fixture(if paste_crlf { "" } else { &original });
        app.tab_mut().unwrap().line_ending = line_ending;
        let mut screen = harness(app);
        if paste_crlf {
            // Real Windows clipboard input must keep the editing buffer normalized.
            events(
                &mut screen,
                [egui::Event::Paste("α🦀\r\nnext 🦀\r\n".into())],
            );
            let tab = screen.state_mut().tab_mut().unwrap();
            let id = tab.id;
            tab.set_cursor(0..0);
            screen.state_mut().sync_external_document_navigation(id);
            screen.run_steps(2);
        }
        text(&mut screen, "Aδ😀");
        assert_document(&screen, &changed);
        shortcut(&mut screen, Key::Z, egui::Modifiers::CTRL);
        assert_document(&screen, &original);
        assert_eq!(screen.state().tab().unwrap().line_ending, line_ending);
        shortcut(&mut screen, Key::Y, egui::Modifiers::CTRL);
        assert_document(&screen, &changed);
        let caret = "α🦀δ😀".chars().count();
        let tab = screen.state().tab().unwrap();
        assert_eq!(tab.vim.mode, VimMode::Insert, "tail={tail:?}");
        assert_eq!(tab.vim.cursor, caret, "tail={tail:?}");
        assert_eq!(tab.selection, caret..caret, "tail={tail:?}");
        assert_eq!(tab.line_ending, line_ending);

        text(&mut screen, "!");
        assert_document(&screen, &format!("α🦀δ😀!{tail}"));
        assert_eq!(screen.state().tab().unwrap().vim.cursor, caret + 1);
        shortcut(&mut screen, Key::Z, egui::Modifiers::CTRL);
        assert_document(&screen, &changed);
        assert_eq!(screen.state().tab().unwrap().line_ending, line_ending);
    }
}

#[test]
fn context_history_actions_finalize_counts_keep_insert_active_and_preserve_redo() {
    let original = "α🦀";
    for (entry, changed, finalize_with_redo) in [
        ("Aδ😀", "α🦀δ😀", false),
        ("3Aδ😀", "α🦀δ😀δ😀δ😀", false),
        ("3Aδ😀", "α🦀δ😀δ😀δ😀", true),
    ] {
        let mut screen = harness(fixture(original));
        text(&mut screen, entry);
        // Counted Insert repeats when its transaction is finalized, not while typing.
        assert_document(&screen, "α🦀δ😀");
        if finalize_with_redo {
            // Model a saved typed prefix, so expansion must mark the document dirty.
            let tab = screen.state_mut().tab_mut().unwrap();
            tab.saved_text = tab.text.clone();
            tab.dirty = false;
            let revision = tab.revision;
            assert!(!tab.vim.can_redo());
            shortcut(&mut screen, Key::Y, egui::Modifiers::CTRL);
            assert_document(&screen, changed);
            let tab = screen.state().tab().unwrap();
            assert!(tab.dirty);
            assert!(tab.revision > revision);
            assert_eq!(tab.vim.mode, VimMode::Insert);
        }
        let id = screen.state().tab().unwrap().id;
        screen
            .state_mut()
            .apply_editor_context_action(id, EditorAction::Undo);
        screen.run_steps(2);
        assert_document(&screen, original);
        assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::Insert);
        assert!(screen.state().tab().unwrap().vim.can_redo());

        let revision = screen.state().tab().unwrap().revision;
        shortcut(&mut screen, Key::Z, egui::Modifiers::CTRL);
        assert_document(&screen, original);
        assert_eq!(screen.state().tab().unwrap().revision, revision);
        assert!(screen.state().tab().unwrap().vim.can_redo());
        screen
            .state_mut()
            .apply_editor_context_action(id, EditorAction::Redo);
        screen.run_steps(2);
        assert_document(&screen, changed);
        let caret = changed.chars().count();
        let tab = screen.state().tab().unwrap();
        assert_eq!(tab.vim.mode, VimMode::Insert);
        assert_eq!(tab.vim.cursor, caret);
        assert_eq!(tab.selection, caret..caret);

        escape(&mut screen);
        text(&mut screen, "0.");
        let inserted = changed.strip_prefix(original).unwrap();
        assert_document(&screen, &format!("{changed}{inserted}"));
        shortcut(&mut screen, Key::Z, egui::Modifiers::CTRL);
        assert_document(&screen, changed);
    }
}

#[derive(Clone, Copy, Debug)]
enum InputOwner {
    Settings,
    Palette,
    Find,
    VimSearch,
    VimCommand,
}

fn focus_input_owner(screen: &mut Harness<'_, MorrowApp>, owner: InputOwner) {
    match owner {
        InputOwner::Settings => shortcut(screen, Key::Comma, egui::Modifiers::CTRL),
        InputOwner::Palette => shortcut(
            screen,
            Key::P,
            egui::Modifiers::CTRL.plus(egui::Modifiers::SHIFT),
        ),
        InputOwner::Find => shortcut(screen, Key::H, egui::Modifiers::CTRL),
        InputOwner::VimSearch => text(screen, "/needle"),
        InputOwner::VimCommand => text(screen, ":w"),
    }
    screen.run_steps(2);
    match owner {
        InputOwner::Settings => assert!(screen.state().settings_open),
        InputOwner::Palette => assert!(screen.state().palette.is_some()),
        InputOwner::Find => assert!(screen.state().find_visible),
        InputOwner::VimSearch => assert!(screen.state().vim_search_active()),
        InputOwner::VimCommand => assert!(screen.state().vim_command.is_some()),
    }
    if matches!(owner, InputOwner::Palette | InputOwner::Find) {
        text(screen, "query 🦀");
        let tab = screen.state().tab().unwrap();
        assert!(
            !screen
                .ctx
                .memory(|memory| { memory.has_focus(screen.state().document_widget_id(tab.id)) })
        );
    }
}

#[test]
fn dialog_and_query_undo_redo_never_consume_source_history() {
    let original = "protected 🦀 source\n";
    for owner in [
        InputOwner::Settings,
        InputOwner::Palette,
        InputOwner::Find,
        InputOwner::VimSearch,
        InputOwner::VimCommand,
    ] {
        for (key, modifiers, prepare_redo) in [
            (Key::Z, egui::Modifiers::CTRL, false),
            (Key::Y, egui::Modifiers::CTRL, true),
            (
                Key::Z,
                egui::Modifiers::CTRL.plus(egui::Modifiers::SHIFT),
                true,
            ),
        ] {
            let mut screen = harness(fixture(original));
            insert_transaction(&mut screen, "δ😀");
            if prepare_redo {
                text(&mut screen, "u");
            } else if matches!(
                owner,
                InputOwner::Settings | InputOwner::Palette | InputOwner::Find
            ) {
                // An editable Insert widget must also yield to dialog/query input.
                text(&mut screen, "i");
            }
            let expected = screen.state().tab().unwrap().text.clone();
            focus_input_owner(&mut screen, owner);
            let tab = screen.state().tab().unwrap();
            let revision = tab.revision;
            let dirty = tab.dirty;
            let can_undo = tab.vim.can_undo();
            let can_redo = tab.vim.can_redo();
            assert_eq!(tab.text, expected, "owner={owner:?}");

            shortcut(&mut screen, key, modifiers);
            let tab = screen.state().tab().unwrap();
            assert_eq!(tab.text, expected, "owner={owner:?}, key={key:?}");
            assert_eq!(tab.revision, revision, "owner={owner:?}, key={key:?}");
            assert_eq!(tab.dirty, dirty, "owner={owner:?}, key={key:?}");
            assert_eq!(tab.vim.can_undo(), can_undo, "owner={owner:?}");
            assert_eq!(tab.vim.can_redo(), can_redo, "owner={owner:?}");
            assert_registers_preserved(screen.state());
        }
    }
}

fn focus_pane(screen: &mut Harness<'_, MorrowApp>, pane_id: u64) {
    let response = screen
        .state_mut()
        .control_request(crate::cli::Request::Layout {
            change: crate::cli::LayoutAction::Focus { pane_id },
        });
    assert!(response.ok, "{response:?}");
    screen.run_steps(2);
}

#[test]
fn split_documents_keep_separate_windows_and_vim_undo_histories() {
    let mut app = fixture("left 🦀\n");
    let left_pane = app.current_pane_id();
    let left_id = app.tab().unwrap().id;
    let mut right = EditorTab::new(None, "right.txt".into());
    right.text = "right 😀\n".into();
    right.saved_text = right.text.clone();
    right.vim.set_register(app.register.clone().unwrap());
    let right_id = right.id;
    app.projects[0].tabs.push(right);
    let right_pane = app
        .split_active(crate::cli::SplitOrientation::Vertical)
        .unwrap();
    let response = app.control_request(crate::cli::Request::Layout {
        change: crate::cli::LayoutAction::Assign {
            pane_id: right_pane,
            tab_id: right_id,
            view: Some(crate::cli::DocumentView::Source),
        },
    });
    assert!(response.ok, "{response:?}");
    let mut screen = harness(app);
    focus_pane(&mut screen, left_pane);
    assert_eq!(screen.state().tab().unwrap().id, left_id);
    insert_transaction(&mut screen, "L");
    focus_pane(&mut screen, right_pane);
    assert_eq!(screen.state().tab().unwrap().id, right_id);
    insert_transaction(&mut screen, "R");

    shortcut(&mut screen, Key::Z, egui::Modifiers::CTRL);
    assert_document(&screen, "right 😀\n");
    assert_eq!(document_by_id(screen.state(), left_id).text, "Lleft 🦀\n");
    focus_pane(&mut screen, left_pane);
    shortcut(&mut screen, Key::Z, egui::Modifiers::CTRL);
    assert_document(&screen, "left 🦀\n");
    assert_eq!(document_by_id(screen.state(), right_id).text, "right 😀\n");

    focus_pane(&mut screen, right_pane);
    shortcut(
        &mut screen,
        Key::Z,
        egui::Modifiers::CTRL.plus(egui::Modifiers::SHIFT),
    );
    assert_document(&screen, "Rright 😀\n");
    assert_eq!(document_by_id(screen.state(), left_id).text, "left 🦀\n");
    focus_pane(&mut screen, left_pane);
    shortcut(&mut screen, Key::Y, egui::Modifiers::CTRL);
    assert_document(&screen, "Lleft 🦀\n");
    assert_eq!(document_by_id(screen.state(), right_id).text, "Rright 😀\n");

    text(&mut screen, "u");
    assert_document(&screen, "left 🦀\n");
    assert!(document_by_id(screen.state(), right_id).dirty);
    focus_pane(&mut screen, right_pane);
    text(&mut screen, "u");
    assert_document(&screen, "right 😀\n");
    assert!(screen.state().projects[0].tabs.iter().all(|tab| !tab.dirty));
}

//! Exercise project navigation and Explorer structure through real UI input.

use super::*;
use egui_kittest::{Harness, kittest::Queryable};

const DOCUMENT: &str = "alpha 🦀 document\nsecond line\nunsaved review text\n";

fn fixture(count: usize) -> MorrowApp {
    let mut app = MorrowApp::empty(egui::Context::default());
    app.settings.autosave = false;
    app.settings.smooth_scroll = false;
    for index in 0..count {
        let root = std::env::temp_dir().join(format!("morrow-workspace-ui-{index:02}"));
        let mut project = Project::new(root.clone());
        project.index = Some(Arc::new(workspace::WorkspaceIndex::empty(root.clone())));
        project.last_git_refresh = Instant::now();
        let mut tab = EditorTab::new(Some(root.join("notes.txt")), "notes.txt".into());
        tab.text = DOCUMENT.into();
        tab.saved_text = DOCUMENT.into();
        project.tabs.push(tab);
        app.projects.push(project);
    }
    app.bind_current_pane();
    app
}

fn harness(app: MorrowApp) -> Harness<'static, MorrowApp> {
    let mut screen = Harness::builder()
        .with_size([1360.0, 900.0])
        .with_os(egui::os::OperatingSystem::Windows)
        .build_ui_state(
            |ui, app: &mut MorrowApp| {
                app.ctx = ui.ctx().clone();
                // These render-only fixtures never persist sessions or buffers,
                // refresh Git, inspect files, or open native operating-system UI.
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

fn shortcut(screen: &mut Harness<'_, MorrowApp>, key: Key, modifiers: egui::Modifiers) {
    // Windows input reports Ctrl both as the physical control modifier and
    // egui's platform-independent command modifier.
    let modifiers = egui::Modifiers {
        command: modifiers.ctrl,
        ..modifiers
    };
    screen.event(egui::Event::ModifiersChanged(modifiers));
    screen.event(egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    });
    screen.run_steps(1);
    screen.event(egui::Event::Key {
        key,
        physical_key: None,
        pressed: false,
        repeat: false,
        modifiers,
    });
    screen.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    screen.run_steps(2);
}

fn project_shortcut(screen: &mut Harness<'_, MorrowApp>, key: Key) {
    shortcut(
        screen,
        key,
        egui::Modifiers::CTRL.plus(egui::Modifiers::ALT),
    );
}

fn visual_draft(app: &mut MorrowApp) {
    let tab = app.tab_mut().unwrap();
    tab.saved_text = "saved original\n".into();
    tab.dirty = true;
    tab.revision = 1;
    tab.recovery_revision = 1;
    tab.vim.set_selection(&tab.text, 6..12);
    tab.selection = 6..12;
    tab.pending_selection = Some(6..12);
}

fn assert_draft(app: &MorrowApp, project: usize) {
    let tab = &app.projects[project].tabs[0];
    assert_eq!(tab.text, DOCUMENT);
    assert_eq!(tab.saved_text, "saved original\n");
    assert!(tab.dirty);
    assert_eq!(tab.revision, 1);
    assert!(!tab.loading);
    assert!(!tab.saving);
}

fn tab_groups(app: &MorrowApp) -> serde_json::Value {
    let status = app.layout_status();
    serde_json::json!({
        "tree": status["tree"],
        "groups": status["panes"].as_array().unwrap().iter().map(|pane| {
            serde_json::json!({"id":pane["id"], "tabs":pane["tab_ids"]})
        }).collect::<Vec<_>>()
    })
}

fn split_fixture() -> (MorrowApp, Vec<serde_json::Value>) {
    let mut app = fixture(3);
    app.split_active(crate::cli::SplitOrientation::Vertical)
        .unwrap();
    // Each project owns its complete arrangement: A has two split groups,
    // while B and C each begin with one group containing only their document.
    let mut groups = Vec::new();
    for project in 0..3 {
        app.switch_project(project);
        groups.push(tab_groups(&app));
    }
    app.switch_project(0);
    visual_draft(&mut app);
    (app, groups)
}

fn assert_groups_belong_to_project(app: &MorrowApp, project: usize) {
    let own_ids = app.projects[project]
        .tabs
        .iter()
        .map(|tab| tab.id)
        .collect::<Vec<_>>();
    for pane in app.layout_status()["panes"].as_array().unwrap() {
        assert!(
            pane["tab_ids"]
                .as_array()
                .unwrap()
                .iter()
                .all(|id| { own_ids.contains(&id.as_u64().unwrap()) }),
            "project groups must contain only their own documents"
        );
        assert!(own_ids.contains(&pane["tab_id"].as_u64().unwrap()));
    }
}

#[test]
fn project_switcher_is_on_the_left_before_the_file_menu() {
    let screen = harness(fixture(3));
    let project = screen.get_by_label("Switch project").rect();
    let file = screen.get_by_label("File").rect();
    assert!(project.right() < file.left());
    assert!(project.left() < 300.0);
    assert!((project.center().y - file.center().y).abs() < 5.0);
}

#[test]
fn pointer_switch_preserves_visual_draft_and_ordered_split_groups() {
    let (app, expected_groups) = split_fixture();
    let mut screen = harness(app);
    assert_eq!(tab_groups(screen.state()), expected_groups[0]);
    screen.get_by_label("Switch project").click();
    screen.run_steps(3);
    assert!(screen.state().workspace_picker_open());
    assert!(
        screen
            .query_by_label_contains("morrow-workspace-ui-00 · 1 open · 1 unsaved")
            .is_some()
    );
    screen
        .get_by_label_contains("morrow-workspace-ui-01 ·")
        .click();
    screen.run_steps(3);
    assert_eq!(screen.state().active_project, 1);
    assert!(!screen.state().workspace_picker_open());
    assert_eq!(tab_groups(screen.state()), expected_groups[1]);
    assert_eq!(screen.state().tab_drag_pane_ids().len(), 1);
    assert_groups_belong_to_project(screen.state(), 1);
    assert_draft(screen.state(), 0);
    screen.get_by_label("Switch project").click();
    screen.run_steps(3);
    screen
        .get_by_label_contains("morrow-workspace-ui-00 ·")
        .click();
    screen.run_steps(3);
    let tab = screen.state().tab().unwrap();
    assert_eq!(screen.state().active_project, 0);
    assert_eq!(tab.vim.mode, VimMode::Visual);
    assert_eq!(tab.vim.selection(&tab.text), Some(6..12));
    assert_eq!(tab.selection, 6..12);
    assert_eq!(tab_groups(screen.state()), expected_groups[0]);
    assert_eq!(screen.state().tab_drag_pane_ids().len(), 2);
    assert_groups_belong_to_project(screen.state(), 0);
    assert_draft(screen.state(), 0);
    assert!(screen.state().projects.iter().all(|project| {
        project.index_generation == 0 && project.tabs.iter().all(|tab| !tab.loading)
    }));
}

#[test]
fn project_cycle_shortcuts_wrap_without_reloading_or_losing_selection() {
    let (app, expected_groups) = split_fixture();
    let mut screen = harness(app);
    assert_eq!(tab_groups(screen.state()), expected_groups[0]);
    for expected in [1, 2, 0] {
        project_shortcut(&mut screen, Key::PageDown);
        assert_eq!(screen.state().active_project, expected);
        assert_eq!(tab_groups(screen.state()), expected_groups[expected]);
        assert_groups_belong_to_project(screen.state(), expected);
        assert_draft(screen.state(), 0);
    }
    let tab = screen.state().tab().unwrap();
    assert_eq!(tab.vim.mode, VimMode::Visual);
    assert_eq!(tab.vim.selection(&tab.text), Some(6..12));
    assert_eq!(tab.selection, 6..12);
    project_shortcut(&mut screen, Key::PageUp);
    assert_eq!(screen.state().active_project, 2);
    assert_eq!(tab_groups(screen.state()), expected_groups[2]);
    assert_groups_belong_to_project(screen.state(), 2);
    assert!(screen.state().projects.iter().all(|project| {
        project.index_generation == 0 && project.tabs.iter().all(|tab| !tab.loading)
    }));
}

#[test]
fn remapped_project_navigation_keeps_prefix_in_source_and_replaces_old_chords() {
    let mut app = fixture(2);
    app.settings.vim = false;
    app.settings.keybindings.insert(
        keybindings::Action::NextProject.id().into(),
        Some("F8".into()),
    );
    app.settings.keybindings.insert(
        keybindings::Action::SwitchProject.id().into(),
        Some("F9".into()),
    );
    let mut screen = harness(app);
    project_shortcut(&mut screen, Key::PageDown);
    assert_eq!(screen.state().active_project, 0);
    screen.input_mut().events.extend([
        egui::Event::Text("prefix".into()),
        egui::Event::Key {
            key: Key::F8,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        },
        egui::Event::Text("suffix".into()),
    ]);
    screen.run_steps(3);
    assert_eq!(screen.state().active_project, 1);
    assert_eq!(
        screen.state().projects[0].tabs[0].text,
        format!("prefix{DOCUMENT}")
    );
    assert_eq!(screen.state().projects[1].tabs[0].text, DOCUMENT);
    project_shortcut(&mut screen, Key::P);
    assert!(!screen.state().workspace_picker_open());
    shortcut(&mut screen, Key::F9, egui::Modifiers::NONE);
    assert!(screen.state().workspace_picker_open());
    shortcut(&mut screen, Key::F9, egui::Modifiers::NONE);
    assert!(!screen.state().workspace_picker_open());
}

#[test]
fn project_cycle_waits_for_an_earlier_application_command() {
    let mut app = fixture(2);
    app.settings.vim = false;
    let wrapped = app.settings.word_wrap;
    let mut screen = harness(app);
    screen.input_mut().events.extend([
        egui::Event::Text("prefix".into()),
        egui::Event::Key {
            key: Key::Z,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::ALT,
        },
        egui::Event::Text("z".into()),
        egui::Event::Key {
            key: Key::PageDown,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers {
                ctrl: true,
                command: true,
                alt: true,
                ..egui::Modifiers::NONE
            },
        },
    ]);
    screen.run_steps(8);
    assert_eq!(screen.state().settings.word_wrap, !wrapped);
    assert_eq!(screen.state().active_project, 1);
    assert_eq!(
        screen.state().projects[0].tabs[0].text,
        format!("prefix{DOCUMENT}")
    );
    assert_eq!(screen.state().projects[1].tabs[0].text, DOCUMENT);
}

fn insert_then_project_chord(screen: &mut Harness<'_, MorrowApp>, key: Key) {
    let modifiers = egui::Modifiers {
        ctrl: true,
        command: true,
        alt: true,
        ..egui::Modifiers::NONE
    };
    screen.input_mut().events.extend([
        egui::Event::Text("abc".into()),
        egui::Event::ModifiersChanged(modifiers),
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        },
        egui::Event::Text("later".into()),
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers,
        },
        egui::Event::ModifiersChanged(egui::Modifiers::NONE),
    ]);
    screen.run_steps(3);
}

#[test]
fn insert_text_before_project_cycle_is_saved_in_the_old_buffer_and_can_be_undone() {
    for count in [1, 2] {
        for vim in [true, false] {
            let mut app = fixture(count);
            app.settings.vim = vim;
            let mut screen = harness(app);
            if vim {
                screen.event(egui::Event::Text("i".into()));
                screen.run_steps(3);
                assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::Insert);
            } else {
                let widget = screen
                    .state()
                    .document_widget_id(screen.state().tab().unwrap().id);
                assert_eq!(screen.ctx.memory(|memory| memory.focused()), Some(widget));
            }
            insert_then_project_chord(&mut screen, Key::PageDown);
            assert_eq!(screen.state().active_project, usize::from(count > 1));
            assert_eq!(
                screen.state().projects[0].tabs[0].text,
                format!("abc{DOCUMENT}"),
                "vim={vim}, projects={count}"
            );
            assert!(screen.state().projects[0].tabs[0].dirty);
            for project in screen.state().projects.iter().skip(1) {
                assert_eq!(project.tabs[0].text, DOCUMENT);
                assert!(!project.tabs[0].dirty);
            }
            assert!(
                screen
                    .state()
                    .projects
                    .iter()
                    .all(|project| { project.tabs.iter().all(|tab| !tab.text.contains("later")) })
            );
            if count > 1 {
                project_shortcut(&mut screen, Key::PageUp);
            }
            if vim {
                screen.key_press(Key::Escape);
                screen.run_steps(3);
                screen.event(egui::Event::Text("u".into()));
                screen.run_steps(3);
            } else {
                shortcut(&mut screen, Key::Z, egui::Modifiers::CTRL);
            }
            assert_eq!(
                screen.state().tab().unwrap().text,
                DOCUMENT,
                "vim={vim}, projects={count} must preserve the original edit history"
            );
            assert!(!screen.state().tab().unwrap().dirty);
        }
    }
}

#[test]
fn insert_text_before_opening_project_picker_stays_in_the_document_and_out_of_query() {
    let mut screen = harness(fixture(2));
    screen.event(egui::Event::Text("i".into()));
    screen.run_steps(3);
    insert_then_project_chord(&mut screen, Key::P);
    assert!(screen.state().workspace_picker_open());
    assert_eq!(screen.state().active_project, 0);
    assert_eq!(
        screen.state().projects[0].tabs[0].text,
        format!("abc{DOCUMENT}")
    );
    assert!(screen.state().projects[0].tabs[0].dirty);
    assert_eq!(screen.state().projects[1].tabs[0].text, DOCUMENT);
    assert!(!screen.state().workspace_ui.query.contains("abc"));
    assert!(
        screen
            .state()
            .projects
            .iter()
            .all(|project| { project.tabs.iter().all(|tab| !tab.text.contains("later")) })
    );
    screen.key_press(Key::Escape);
    screen.run_steps(3);
    // The first Escape closes the picker; the second finishes the original
    // Insert transaction so Vim's undo must restore the unchanged document.
    screen.key_press(Key::Escape);
    screen.run_steps(3);
    screen.event(egui::Event::Text("u".into()));
    screen.run_steps(3);
    assert_eq!(screen.state().tab().unwrap().text, DOCUMENT);
    assert!(!screen.state().tab().unwrap().dirty);
}

#[test]
fn typed_project_filter_does_not_reach_the_editor() {
    let mut app = fixture(3);
    visual_draft(&mut app);
    let mut screen = harness(app);
    project_shortcut(&mut screen, Key::P);
    assert!(screen.state().workspace_picker_open());
    assert!(screen.state().palette.is_none());
    assert_eq!(
        screen.ctx.memory(|memory| memory.focused()),
        Some(egui::Id::new("workspace_picker_query")),
        "project search must retain keyboard focus after the first popup frames"
    );
    screen.event(egui::Event::Text("workspace-ui-02".into()));
    screen.run_steps(3);
    assert_eq!(screen.state().workspace_ui.query, "workspace-ui-02");
    assert!(
        screen
            .query_by_label_contains("morrow-workspace-ui-02 ·")
            .is_some()
    );
    assert!(
        screen
            .query_by_label_contains("morrow-workspace-ui-01 ·")
            .is_none()
    );
    assert_draft(screen.state(), 0);
    screen.key_press(Key::Enter);
    screen.run_steps(3);
    assert_eq!(screen.state().active_project, 2);
    assert!(!screen.state().workspace_picker_open());
    assert_draft(screen.state(), 0);
}

#[test]
fn many_projects_keep_a_bounded_picker_and_keyboard_reveal_the_last_row() {
    let mut screen = harness(fixture(40));
    project_shortcut(&mut screen, Key::P);
    let query = screen
        .ctx
        .read_response(egui::Id::new("workspace_picker_query"))
        .expect("the project search field is present")
        .rect;
    screen.key_press(Key::End);
    screen.run_steps(4);
    let row = screen
        .get_by_label_contains("morrow-workspace-ui-39 ·")
        .rect();
    let add = screen.get_by_label("Add project…").rect();
    assert!(row.top() >= query.bottom());
    assert!(row.bottom() <= add.top());
    assert!(add.bottom() - query.bottom() < 460.0);
    assert!(row.bottom() < screen.ctx.content_rect().bottom());
    screen.key_press(Key::Enter);
    screen.run_steps(3);
    assert_eq!(screen.state().active_project, 39);
    assert_eq!(screen.state().projects.len(), 40);
    assert!(!screen.state().workspace_picker_open());
}

#[test]
fn add_project_uses_the_native_folder_picker_without_closing_open_projects() {
    let mut screen = harness(fixture(2));
    screen.get_by_label("Switch project").click();
    screen.run_steps(3);
    screen.get_by_label("Add project…").click();
    screen.run_steps(3);
    assert_eq!(
        screen.state().picker.unwrap().purpose,
        DialogPurpose::Projects
    );
    assert_eq!(screen.state().projects.len(), 2);
    assert!(screen.state().native_dialog_builder.is_none());
    assert!(!screen.state().workspace_picker_open());
}

#[test]
fn empty_header_add_project_uses_the_native_folder_picker() {
    let mut screen = harness(fixture(0));
    screen.get_by_label("Add project").click();
    screen.run_steps(3);
    assert_eq!(
        screen.state().picker.unwrap().purpose,
        DialogPurpose::Projects
    );
    assert!(screen.state().projects.is_empty());
    assert!(screen.state().native_dialog_builder.is_none());
}

#[test]
fn accepting_a_project_from_insert_mode_does_not_insert_the_enter_text_event() {
    let mut screen = harness(fixture(2));
    screen.event(egui::Event::Text("i".into()));
    screen.run_steps(3);
    assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::Insert);
    project_shortcut(&mut screen, Key::P);
    assert_eq!(
        screen.ctx.memory(|memory| memory.focused()),
        Some(egui::Id::new("workspace_picker_query"))
    );
    screen.event(egui::Event::Text("workspace-ui-01".into()));
    screen.run_steps(3);
    screen.event(egui::Event::Key {
        key: Key::Enter,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    });
    screen.event(egui::Event::Text("\n".into()));
    screen.run_steps(3);
    assert_eq!(screen.state().active_project, 1);
    assert!(!screen.state().workspace_picker_open());
    for project in &screen.state().projects {
        assert_eq!(project.tabs[0].text, DOCUMENT);
        assert!(!project.tabs[0].dirty);
        assert!(!project.tabs[0].vim.can_undo());
    }
}

#[test]
fn closing_the_selected_dirty_project_requires_confirmation() {
    let mut app = fixture(3);
    app.switch_project(2);
    visual_draft(&mut app);
    app.switch_project(0);
    let root = app.projects[2].root.clone();
    let mut screen = harness(app);
    project_shortcut(&mut screen, Key::P);
    screen.key_press(Key::End);
    screen.run_steps(3);
    assert_eq!(
        screen.state().workspace_ui.selected_root.as_ref(),
        Some(&root)
    );
    screen.get_by_label("Close project").click();
    screen.run_steps(3);
    assert!(matches!(
        screen.state().close_action,
        Some(CloseAction::Project(2))
    ));
    assert_eq!(screen.state().projects.len(), 3);
    assert_draft(screen.state(), 2);
    assert!(screen.query_by_label("Save & close").is_some());
    screen.get_by_label("Cancel").click();
    screen.run_steps(3);
    assert!(screen.state().close_action.is_none());
    assert_eq!(screen.state().projects.len(), 3);
    assert_eq!(screen.state().active_project, 0);
    assert_draft(screen.state(), 2);
}

#[test]
fn control_p_remains_file_search_and_escape_dismisses_project_picker() {
    let mut screen = harness(fixture(2));
    shortcut(&mut screen, Key::P, egui::Modifiers::CTRL);
    assert!(screen.state().palette == Some(Palette::Files));
    assert!(!screen.state().workspace_picker_open());
    screen.key_press(Key::Escape);
    screen.run_steps(3);
    assert!(screen.state().palette.is_none());
    project_shortcut(&mut screen, Key::P);
    assert!(screen.state().workspace_picker_open());
    project_shortcut(&mut screen, Key::P);
    assert!(!screen.state().workspace_picker_open());
    project_shortcut(&mut screen, Key::P);
    screen.key_press(Key::Escape);
    screen.run_steps(3);
    assert!(!screen.state().workspace_picker_open());
    assert_eq!(screen.state().active_project, 0);
    assert_eq!(screen.state().tab().unwrap().text, DOCUMENT);
}

#[test]
fn explorer_root_heading_stays_open_and_children_have_one_level_of_indent() {
    let root = fs::canonicalize(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tiny-project"),
    )
    .unwrap();
    let mut app = fixture(1);
    app.projects[0].root = root.clone();
    app.projects[0].index = Some(Arc::new(
        workspace::build_index(&root, Default::default()).unwrap(),
    ));
    app.projects[0].tabs[0].path = Some(root.join("src/main.rs"));
    let mut screen = harness(app);
    let root_title = file_title(&root);
    let heading = screen.get_by_label(&root_title).rect();
    let readme = screen.get_by_label("README.md").rect();
    let folder = screen.get_by_label("src").rect();
    let nested = screen.get_by_label("main.rs").rect();
    assert!((heading.left() - readme.left()).abs() < 1.0);
    assert!((folder.left() - readme.left()).abs() < 1.0);
    assert!((nested.left() - folder.left() - 14.0).abs() < 2.0);
    for _ in 0..2 {
        screen.get_by_label(&root_title).click();
        screen.run_steps(3);
        assert!(screen.query_by_label("README.md").is_some());
        assert!(screen.query_by_label("main.rs").is_some());
    }
    // Child folders retain their ordinary independent collapse behavior.
    screen.get_by_label("src").click();
    screen.run_steps(3);
    assert!(screen.query_by_label("main.rs").is_none());
    assert!(screen.query_by_label("README.md").is_some());
}

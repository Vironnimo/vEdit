//! Exercise project navigation and Explorer structure through real UI input.

use super::*;
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};

const DOCUMENT: &str = "alpha 🦀 document\nsecond line\nunsaved review text\n";

fn fixture(count: usize) -> VEditApp {
    let mut app = VEditApp::empty(egui::Context::default());
    app.settings.autosave = false;
    app.settings.smooth_scroll = false;
    for index in 0..count {
        let root = std::env::temp_dir().join(format!("vedit-workspace-ui-{index:02}"));
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

fn harness(app: VEditApp) -> Harness<'static, VEditApp> {
    harness_with_size(app, Vec2::new(1360.0, 900.0))
}

fn harness_with_size(app: VEditApp, size: Vec2) -> Harness<'static, VEditApp> {
    let mut screen = Harness::builder()
        .with_size(size)
        .with_os(egui::os::OperatingSystem::Windows)
        .build_ui_state(
            |ui, app: &mut VEditApp| {
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

fn shortcut(screen: &mut Harness<'_, VEditApp>, key: Key, modifiers: egui::Modifiers) {
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

fn project_shortcut(screen: &mut Harness<'_, VEditApp>, key: Key) {
    shortcut(
        screen,
        key,
        egui::Modifiers::CTRL.plus(egui::Modifiers::ALT),
    );
}

fn visual_draft(app: &mut VEditApp) {
    let tab = app.tab_mut().unwrap();
    tab.saved_text = "saved original\n".into();
    tab.dirty = true;
    tab.revision = 1;
    tab.recovery_revision = 1;
    tab.vim.set_selection(&tab.text, 6..12);
    tab.selection = 6..12;
    tab.pending_selection = Some(6..12);
}

fn assert_draft(app: &VEditApp, project: usize) {
    let tab = &app.projects[project].tabs[0];
    assert_eq!(tab.text, DOCUMENT);
    assert_eq!(tab.saved_text, "saved original\n");
    assert!(tab.dirty);
    assert_eq!(tab.revision, 1);
    assert!(!tab.loading);
    assert!(!tab.saving);
}

fn tab_groups(app: &VEditApp) -> serde_json::Value {
    let status = app.layout_status();
    serde_json::json!({
        "tree": status["tree"],
        "groups": status["panes"].as_array().unwrap().iter().map(|pane| {
            serde_json::json!({"id":pane["id"], "tabs":pane["tab_ids"]})
        }).collect::<Vec<_>>()
    })
}

fn split_fixture() -> (VEditApp, Vec<serde_json::Value>) {
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

fn assert_groups_belong_to_project(app: &VEditApp, project: usize) {
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
fn project_tabs_and_the_compact_project_list_stay_in_the_titlebar() {
    let screen = harness(fixture(3));
    let project = screen.get_by_label("Project: vedit-workspace-ui-00").rect();
    let second = screen.get_by_label("Project: vedit-workspace-ui-01").rect();
    let third = screen.get_by_label("Project: vedit-workspace-ui-02").rect();
    let list = screen.get_by_label("Switch project").rect();
    let menu = screen.get_by_label("Main menu").rect();
    assert!(project.top() < menu.top());
    assert!(project.left() < 300.0);
    assert!(project.right() < second.left());
    assert!(second.right() < third.left());
    assert!(third.right() < list.left());
    assert!(list.right() < screen.get_by_label("Minimize window").rect().left());
    assert!(list.width() <= 30.0);
    assert_eq!(
        screen
            .get_by_label("Project: vedit-workspace-ui-00")
            .accesskit_node()
            .toggled(),
        Some(egui::accesskit::Toggled::True),
    );
    for title in ["File", "View", "Run"] {
        assert!(screen.query_by_label(title).is_none());
    }
}

#[test]
fn opening_project_picker_reveals_the_current_project_beyond_the_first_page() {
    let mut app = fixture(25);
    app.switch_project(22);
    let mut screen = harness(app);
    screen.get_by_label("Switch project").click();
    screen.run_steps(3);
    assert!(screen.state().workspace_picker_open());
    assert!(
        screen
            .query_by_label_contains("vedit-workspace-ui-22 ·")
            .is_some(),
        "the selected project must be rendered in the visible virtualized rows"
    );
    let next = screen
        .get_by_label_contains("vedit-workspace-ui-23 ·")
        .rect();
    assert!(
        next.bottom() <= screen.get_by_label("Add project…").rect().top(),
        "the next project must be completely visible before pointer switching: {next:?}"
    );
    screen.hover_at(next.center());
    screen.run_steps(1);
    let after_hover = screen
        .get_by_label_contains("vedit-workspace-ui-23 ·")
        .rect();
    assert_eq!(
        next, after_hover,
        "opening near the end must not move project hit targets after hover"
    );
    screen
        .get_by_label_contains("vedit-workspace-ui-23 ·")
        .click();
    screen.run_steps(3);
    assert_eq!(
        screen.state().active_project,
        23,
        "pointer switch must choose the clicked project; popup open: {}",
        screen.state().workspace_picker_open()
    );
}

#[test]
fn pointer_switch_preserves_visual_draft_and_ordered_split_groups() {
    let (app, expected_groups) = split_fixture();
    let mut screen = harness(app);
    assert_eq!(tab_groups(screen.state()), expected_groups[0]);
    screen
        .get_by_label("Project: vedit-workspace-ui-01")
        .click();
    screen.run_steps(3);
    assert_eq!(screen.state().active_project, 1);
    assert!(!screen.state().workspace_picker_open());
    assert_eq!(tab_groups(screen.state()), expected_groups[1]);
    assert_eq!(screen.state().tab_drag_pane_ids().len(), 1);
    assert_groups_belong_to_project(screen.state(), 1);
    assert_draft(screen.state(), 0);
    screen
        .get_by_label("Project: vedit-workspace-ui-00")
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
fn project_tabs_support_keyboard_and_accesskit_activation() {
    let mut screen = harness(fixture(3));
    for (index, key) in [(1, Key::Enter), (2, Key::Space)] {
        screen
            .get_by_label(&format!("Project: vedit-workspace-ui-{index:02}"))
            .focus();
        screen.run_steps(1);
        screen.key_press(key);
        screen.run_steps(3);
        assert_eq!(screen.state().active_project, index);
        assert!(!screen.state().workspace_picker_open());
    }
    screen
        .get_by_label("Project: vedit-workspace-ui-00")
        .click_accesskit();
    screen.run_steps(3);
    assert_eq!(screen.state().active_project, 0);
    assert!(
        screen
            .state()
            .projects
            .iter()
            .all(|project| project.tabs[0].text == DOCUMENT)
    );
    screen.get_by_label("Switch project").focus();
    screen.run_steps(1);
    screen.key_press(Key::Space);
    screen.run_steps(3);
    assert!(screen.state().workspace_picker_open());
    assert_eq!(
        screen.ctx.memory(|memory| memory.focused()),
        Some(query_id())
    );
}

#[test]
fn direct_project_click_keeps_same_frame_insert_prefix_in_the_old_buffer_and_undo() {
    for vim in [true, false] {
        let mut app = fixture(2);
        app.settings.vim = vim;
        let mut screen = harness(app);
        if vim {
            screen.event(egui::Event::Text("i".into()));
            screen.run_steps(3);
            assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::Insert);
        }
        let old_id = screen.state().tab().unwrap().id;
        assert!(
            screen
                .ctx
                .memory(|memory| memory.has_focus(screen.state().document_widget_id(old_id)))
        );
        let focus_policy = screen
            .ctx
            .options(|options| options.input_options.surrender_focus_on);
        let point = screen
            .get_by_label("Project: vedit-workspace-ui-01")
            .rect()
            .center();
        // All events share one RawInput frame; Harness::event would instead
        // render each separately and hide the old/new document ownership bug.
        screen.input_mut().events.extend([
            egui::Event::Text("abc".into()),
            egui::Event::PointerMoved(point),
            egui::Event::PointerButton {
                pos: point,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: point,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::Text("unowned suffix".into()),
        ]);
        screen.run_steps(3);
        assert_eq!(screen.state().active_project, 1);
        assert_eq!(
            screen.state().projects[0].tabs[0].text,
            format!("abc{DOCUMENT}"),
            "vim={vim}"
        );
        assert_eq!(
            screen.state().projects[1].tabs[0].text,
            DOCUMENT,
            "vim={vim}"
        );
        assert!(screen.state().projects[0].tabs[0].dirty);
        assert!(!screen.state().projects[1].tabs[0].dirty);
        assert_eq!(
            screen
                .ctx
                .options(|options| options.input_options.surrender_focus_on),
            focus_policy,
            "the prefix guard must restore ordinary pointer focus behavior"
        );
        screen
            .get_by_label("Project: vedit-workspace-ui-00")
            .click();
        screen.run_steps(3);
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
            "vim={vim}: native and Vim undo must own the prefix"
        );
    }
}

#[test]
fn cached_git_changes_have_a_dot_while_clean_unknown_and_unsaved_only_do_not() {
    let mut app = fixture(3);
    visual_draft(&mut app);
    app.projects[0].git_indicator_count = Some(0);
    app.projects[1].git_indicator_count = Some(3);
    app.projects[2].git_indicator_count = None;
    app.projects[2].tabs[0].dirty = true;
    let mut screen = harness(app);
    let changed = screen.get_by_label("Project: vedit-workspace-ui-01").rect();
    let marker = screen
        .get_by_label("3 uncommitted changes in vedit-workspace-ui-01")
        .rect();
    assert!(changed.contains_rect(marker));
    assert!(
        screen
            .query_by_label_contains("uncommitted changes in vedit-workspace-ui-00")
            .is_none()
    );
    assert!(
        screen
            .query_by_label_contains("uncommitted changes in vedit-workspace-ui-02")
            .is_none()
    );
    assert!(screen.output().shapes.iter().any(|shape| {
        matches!(&shape.shape, egui::Shape::Circle(circle)
            if marker.contains(circle.center) && circle.radius < 4.0 && circle.fill == ORANGE.gamma_multiply(0.85))
    }));
    screen.state_mut().projects[1].git_indicator_count = Some(0);
    screen.state_mut().projects[0].git_indicator_count = Some(1);
    screen.run_steps(3);
    assert!(
        screen
            .query_by_label("3 uncommitted changes in vedit-workspace-ui-01")
            .is_none()
    );
    assert!(
        screen
            .query_by_label("1 uncommitted change in vedit-workspace-ui-00")
            .is_some()
    );
    assert_eq!(
        changed,
        screen.get_by_label("Project: vedit-workspace-ui-01").rect(),
        "status refresh must not move a neighboring tab's hit target"
    );
}

#[test]
fn cycling_many_projects_and_resizing_reveal_the_active_header_tab() {
    let mut screen = harness(fixture(24));
    for (key, selected) in [(Key::PageUp, 23), (Key::PageDown, 0), (Key::PageUp, 23)] {
        project_shortcut(&mut screen, key);
        assert_eq!(screen.state().active_project, selected);
        let tab = screen
            .get_by_label(&format!("Project: vedit-workspace-ui-{selected:02}"))
            .rect();
        let list = screen.get_by_label("Switch project").rect();
        assert!(
            tab.left() >= 122.0 - 1.0,
            "selected tab must be revealed after cycling: {tab:?}"
        );
        assert!(tab.right() <= list.left() - PROJECT_TAB_GAP + 1.0);
        assert_eq!(tab.height(), PROJECT_TAB_HEIGHT);
        assert!(!screen.state().workspace_picker_open());
    }
    screen.set_size(Vec2::new(500.0, 360.0));
    screen.run_steps(3);
    let active = screen.get_by_label("Project: vedit-workspace-ui-23").rect();
    let list = screen.get_by_label("Switch project").rect();
    assert!(active.left() >= 122.0 - 1.0);
    assert!(active.right() <= list.left() - PROJECT_TAB_GAP + 1.0);
    assert!(list.right() < screen.get_by_label("Minimize window").rect().left());
}

#[test]
fn long_colliding_project_names_keep_the_distinguishing_parent_visible() {
    let mut app = fixture(2);
    let name = "very_long_shared_project_name_".repeat(5);
    for (project, parent) in app.projects.iter_mut().zip(["client", "server"]) {
        project.root = PathBuf::from("C:/work").join(parent).join(&name);
    }
    let screen = harness(app);
    for parent in ["client", "server"] {
        let tab = screen
            .get_by_label(&format!("Project: {name} — {parent}"))
            .rect();
        assert!(tab.width() <= 220.0);
        assert!(screen.output().shapes.iter().any(|shape| {
            matches!(&shape.shape, egui::Shape::Text(text)
                if text.galley.text() == format!(" — {parent}") && tab.contains_rect(egui::Rect::from_min_size(text.pos, text.galley.size())))
        }), "the parent suffix remains painted inside its project tab: {parent}");
    }
}

#[test]
fn narrow_zoom_keeps_project_list_keyboard_search_and_footer_inside_the_viewport() {
    let mut app = fixture(3);
    app.settings.ui_scale = 2.0;
    // Begin with the native minimum at 100% native DPI. egui applies the
    // initial interface zoom and transforms this to 250×180 logical points.
    // Setting zoom and size together on an existing context would let egui's
    // zoom transition overwrite the requested size with its old viewport.
    let mut screen = harness_with_size(app, Vec2::new(500.0, 360.0));
    let viewport = screen.ctx.content_rect();
    assert_eq!(viewport.size(), Vec2::new(250.0, 180.0));
    assert_eq!(screen.ctx.pixels_per_point(), 2.0);
    let list = screen.get_by_label("Switch project").rect();
    assert!(viewport.contains_rect(list));
    for label in [
        "Minimize window",
        "Maximize window",
        "Close window",
        "Main menu",
    ] {
        assert!(
            viewport.contains_rect(screen.get_by_label(label).rect()),
            "{label}"
        );
    }
    assert!(
        screen
            .query_by_label("Project: vedit-workspace-ui-00")
            .is_none()
    );
    project_shortcut(&mut screen, Key::P);
    assert!(screen.state().workspace_picker_open());
    let popup = screen
        .ctx
        .memory(|memory| memory.area_rect(picker_id()))
        .unwrap();
    assert!(
        viewport.contains_rect(popup),
        "compact popup must fit {viewport:?}: {popup:?}"
    );
    assert!(viewport.contains_rect(screen.get_by_label("Find project").rect()));
    assert!(viewport.contains_rect(screen.get_by_label("Add project…").rect()));
    assert!(viewport.contains_rect(screen.get_by_label("Close project").rect()));
    screen.event(egui::Event::Text("no matching project".into()));
    screen.run_steps(3);
    assert!(
        viewport.contains_rect(
            screen
                .ctx
                .memory(|memory| memory.area_rect(picker_id()))
                .unwrap()
        ),
        "the empty search state must also fit the narrow viewport"
    );
    for label in ["Find project", "Add project…", "Close project"] {
        assert!(
            viewport.contains_rect(screen.get_by_label(label).rect()),
            "{label}"
        );
    }
    screen.key_press(Key::Enter);
    screen.run_steps(3);
    assert_eq!(screen.state().active_project, 0);
    assert!(screen.state().workspace_picker_open());
    shortcut(&mut screen, Key::A, egui::Modifiers::CTRL);
    screen.event(egui::Event::Text("workspace-ui-02".into()));
    screen.run_steps(3);
    screen.key_press(Key::Enter);
    screen.run_steps(3);
    assert_eq!(screen.state().active_project, 2);
    assert!(!screen.state().workspace_picker_open());
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
        Some("F10".into()),
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
            key: Key::F10,
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

fn insert_then_project_chord(screen: &mut Harness<'_, VEditApp>, key: Key) {
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
            .query_by_label_contains("vedit-workspace-ui-02 ·")
            .is_some()
    );
    assert!(
        screen
            .query_by_label_contains("vedit-workspace-ui-01 ·")
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
fn project_filter_accepts_pasted_paths_with_forward_or_backward_slashes() {
    for separator in ['/', '\\'] {
        let mut screen = harness(fixture(3));
        let path = crate::paths::display(&screen.state().projects[2].root)
            .replace(['/', '\\'], &separator.to_string());
        project_shortcut(&mut screen, Key::P);
        screen.event(egui::Event::Text(path));
        screen.run_steps(3);
        assert!(
            screen
                .query_by_label_contains("vedit-workspace-ui-02 ·")
                .is_some(),
            "separator {separator:?}"
        );
        assert!(
            screen
                .query_by_label_contains("vedit-workspace-ui-01 ·")
                .is_none()
        );
        screen.key_press(Key::Enter);
        screen.run_steps(3);
        assert_eq!(screen.state().active_project, 2);
        assert!(
            screen
                .state()
                .projects
                .iter()
                .all(|project| { project.tabs[0].text == DOCUMENT && !project.tabs[0].dirty })
        );
    }
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
        .get_by_label_contains("vedit-workspace-ui-39 ·")
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
fn project_context_menu_supports_keyboard_and_copies_the_clicked_project_path() {
    let mut screen = harness(fixture(2));
    let root = screen.state().projects[1].root.clone();
    screen
        .get_by_label("Project: vedit-workspace-ui-01")
        .focus();
    screen.run_steps(1);
    shortcut(&mut screen, Key::F10, egui::Modifiers::SHIFT);
    assert!(screen.query_by_label("Reveal in Explorer").is_some());
    let before = screen.get_by_label("Copy path").rect();
    screen.run_steps(2);
    assert_eq!(screen.get_by_label("Copy path").rect(), before);
    screen.get_by_label("Copy path").click();
    screen.run_steps(1);
    assert!(screen.output().platform_output.commands.iter().any(|command| {
        matches!(command, egui::OutputCommand::CopyText(text) if *text == crate::paths::display(&root))
    }));
    assert_eq!(screen.state().active_project, 0);
}

#[test]
fn project_context_menu_closes_its_target_and_preserves_dirty_projects() {
    let mut app = fixture(3);
    app.switch_project(1);
    visual_draft(&mut app);
    app.switch_project(0);
    let mut screen = harness(app);
    screen
        .get_by_label("Project: vedit-workspace-ui-01")
        .click_secondary();
    screen.run_steps(2);
    assert_eq!(screen.state().active_project, 0);
    screen.get_by_label("Close project").click();
    screen.run_steps(3);
    assert!(matches!(
        screen.state().close_action,
        Some(CloseAction::Project(1))
    ));
    assert_draft(screen.state(), 1);
    screen.get_by_label("Cancel").click();
    screen.run_steps(2);
    screen
        .get_by_label("Project: vedit-workspace-ui-02")
        .click_secondary();
    screen.run_steps(2);
    screen.get_by_label("Close project").click();
    screen.run_steps(3);
    assert_eq!(screen.state().projects.len(), 2);
    assert_eq!(screen.state().active_project, 0);
    assert_draft(screen.state(), 1);
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
        // This render fixture needs its checked-in files even when a parent
        // directory ignores the checkout (for example a managed worktree).
        workspace::build_index(
            &root,
            workspace::IndexOptions {
                show_ignored_files: true,
                ..Default::default()
            },
        )
        .unwrap(),
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
    assert!((nested.left() - folder.left()).abs() < 1.0);
    assert!((nested.right() - folder.right()).abs() < 1.0);
    // Indentation belongs to the painted contents. Full-width interaction and
    // selection bounds remain aligned across every depth.
    let painted_left = |label: &str, row: egui::Rect| {
        screen
            .output()
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text)
                    if text.galley.text() == label && row.contains(text.pos) =>
                {
                    Some(text.pos.x)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("painted Explorer label {label}"))
    };
    assert!((painted_left("README.md", readme) - painted_left("src", folder)).abs() < 1.0);
    assert!((painted_left("main.rs", nested) - painted_left("src", folder) - 16.0).abs() < 1.0);
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

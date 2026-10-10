//! Real pointer and keyboard flows for Git's nested modal stack.
use super::*;
use egui_kittest::{Harness, kittest::Queryable};

const PATCH: &str = "diff --git a/notes.txt b/notes.txt\n--- a/notes.txt\n+++ b/notes.txt\n@@ -1 +1 @@\n-old\n+new\n";

fn fixture() -> VEditApp {
    let mut app = VEditApp::empty(egui::Context::default());
    app.settings.autosave = false;
    app.sidebar_visible = false;
    let root = std::env::temp_dir().join("vedit-git-modals-render-only");
    let mut project = Project::new(root.clone());
    for (name, text) in [
        ("notes.txt", "first line\nsecond line\n"),
        ("other.txt", "other document\n"),
    ] {
        let mut tab = EditorTab::new(Some(root.join(name)), name.into());
        tab.text = text.into();
        tab.saved_text = text.into();
        project.tabs.push(tab);
    }
    let hash = "a".repeat(40);
    project.git_view.selected_commit = Some(hash.clone());
    project.git_view.detail = Some(git::CommitDetails {
        hash,
        parents: vec!["b".repeat(40)],
        author: "Alex Taylor".into(),
        author_email: "alex@example.test".into(),
        date: "2026-09-26T14:32:18+02:00".into(),
        message: "Preserve modal navigation\n\nKeep the editor's selection and reading position while reviewing a historical change.\n".into(),
        files: vec![git::CommitFile {
            path: "notes.txt".into(), original_path: None,
            status: git::FileChange::Modified,
            stats: git::LineStats { added: 1, removed: 1, binary: false },
        }],
    });
    app.projects.push(project);
    app.bind_current_pane();
    app
}

fn harness(app: VEditApp) -> Harness<'static, VEditApp> {
    let mut screen = Harness::builder()
        .with_size([1440.0, 920.0])
        .with_os(egui::os::OperatingSystem::Windows)
        .build_ui_state(
            |ui, app: &mut VEditApp| {
                app.ctx = ui.ctx().clone();
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

fn details(screen: &mut Harness<'_, VEditApp>) {
    screen.state_mut().projects[0].git_view.detail_open = true;
    screen.run_steps(4);
    assert!(screen.state().git_detail_open());
}

fn child_diff(screen: &mut Harness<'_, VEditApp>) {
    screen.state_mut().diff = Some(("notes.txt · aaaaaaaa".into(), PATCH.into()));
    screen.run_steps(4);
    assert!(screen.state().diff.is_some());
}

fn click(screen: &mut Harness<'_, VEditApp>, point: egui::Pos2) -> Vec<String> {
    let mut copied = Vec::new();
    for event in [
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
    ] {
        screen.event(event);
        screen.run_steps(1);
        copied.extend(
            screen
                .output()
                .platform_output
                .commands
                .iter()
                .filter_map(|command| {
                    if let egui::OutputCommand::CopyText(text) = command {
                        Some(text.clone())
                    } else {
                        None
                    }
                }),
        );
    }
    screen.run_steps(3);
    copied
}

fn outside_tab(screen: &Harness<'_, VEditApp>) -> egui::Pos2 {
    let point = screen.get_by_label("other.txt").rect().center();
    let modal = screen
        .ctx
        .data(|data| {
            data.get_temp::<egui::Rect>(
                egui::Id::new(("git-commit-details", 0usize)).with("content_rect"),
            )
        })
        .unwrap();
    assert!(!modal.contains(point));
    point
}

fn assert_source_unchanged(screen: &Harness<'_, VEditApp>, id: u64, selection: Range<usize>) {
    let tab = screen.state().tab().unwrap();
    assert_eq!(
        tab.id, id,
        "the backdrop click must not switch a tab behind the dialog"
    );
    assert_eq!(tab.text, tab.saved_text);
    assert!(!tab.dirty);
    assert_eq!(tab.selection, selection);
}

#[test]
fn outside_commit_click_dismisses_without_switching_tabs_or_losing_visual_selection() {
    let mut app = fixture();
    let tab = app.tab_mut().unwrap();
    tab.vim.set_selection(&tab.text, 0..5);
    tab.selection = 0..5;
    tab.pending_selection = Some(0..5);
    let mut screen = harness(app);
    let id = screen.state().tab().unwrap().id;
    details(&mut screen);
    let point = outside_tab(&screen);
    click(&mut screen, point);
    assert!(!screen.state().git_detail_open());
    assert_source_unchanged(&screen, id, 0..5);
    assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::Visual);
    assert!(
        screen
            .ctx
            .memory(|memory| memory.has_focus(screen.state().document_widget_id(id)))
    );
}

#[test]
fn outside_nested_diff_click_closes_only_the_child_then_the_parent() {
    let mut screen = harness(fixture());
    let id = screen.state().tab().unwrap().id;
    details(&mut screen);
    child_diff(&mut screen);
    let point = outside_tab(&screen);
    click(&mut screen, point);
    assert!(screen.state().diff.is_none());
    assert!(
        screen.state().git_detail_open(),
        "the parent must survive the child's backdrop click"
    );
    assert_source_unchanged(&screen, id, 0..0);
    let close = screen
        .ctx
        .data(|data| {
            data.get_temp::<egui::Id>(
                egui::Id::new(("git-commit-details", 0usize)).with("close_focus"),
            )
        })
        .unwrap();
    assert!(screen.ctx.memory(|memory| memory.has_focus(close)));
    click(&mut screen, point);
    assert!(!screen.state().git_detail_open());
    assert_source_unchanged(&screen, id, 0..0);
}

#[test]
fn escape_closes_the_top_git_modal_and_restores_insert_focus_only_after_the_parent() {
    let mut app = fixture();
    let tab = app.tab_mut().unwrap();
    tab.vim
        .handle_key(&mut tab.text, crate::vim::VimKey::Char('i'));
    let mut screen = harness(app);
    let id = screen.state().tab().unwrap().id;
    details(&mut screen);
    child_diff(&mut screen);
    screen.event(egui::Event::Text("do not type this".into()));
    screen.run_steps(2);
    assert_source_unchanged(&screen, id, 0..0);
    screen.key_press(Key::Escape);
    screen.run_steps(3);
    assert!(screen.state().diff.is_none() && screen.state().git_detail_open());
    assert!(
        !screen
            .ctx
            .memory(|memory| memory.has_focus(screen.state().document_widget_id(id)))
    );
    screen.key_press(Key::Escape);
    screen.run_steps(3);
    assert!(!screen.state().git_detail_open());
    assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::Insert);
    assert_source_unchanged(&screen, id, 0..0);
    assert!(
        screen
            .ctx
            .memory(|memory| memory.has_focus(screen.state().document_widget_id(id)))
    );
    screen.event(egui::Event::Text("Z".into()));
    screen.run_steps(2);
    assert!(
        screen
            .state()
            .tab()
            .unwrap()
            .text
            .starts_with("Zfirst line")
    );
}

#[test]
fn modal_content_and_copy_patch_do_not_dismiss_either_dialog() {
    let mut screen = harness(fixture());
    details(&mut screen);
    let bounds = screen
        .ctx
        .data(|data| {
            data.get_temp::<egui::Rect>(
                egui::Id::new(("git-commit-details", 0usize)).with("content_rect"),
            )
        })
        .unwrap();
    click(
        &mut screen,
        egui::pos2(bounds.center().x, bounds.bottom() - 45.0),
    );
    assert!(screen.state().git_detail_open());
    child_diff(&mut screen);
    let point = screen.get_by_label("Copy patch").rect().center();
    assert_eq!(click(&mut screen, point), [PATCH]);
    assert!(screen.state().diff.is_some() && screen.state().git_detail_open());
    assert_eq!(
        screen.state().tab().unwrap().text,
        screen.state().tab().unwrap().saved_text
    );
}

#[test]
fn outside_click_does_not_focus_another_split_behind_git_modals() {
    let mut app = fixture();
    app.split_active(split::SplitOrientation::Vertical).unwrap();
    app.activate_tab(0, 1);
    let mut screen = harness(app);
    let pane = screen.state().current_pane_id();
    let id = screen.state().tab().unwrap().id;
    let inactive = screen
        .state()
        .tab_drag_pane_rects()
        .into_iter()
        .find(|(id, _)| *id != pane)
        .unwrap()
        .1;
    let point = inactive.min + Vec2::new(24.0, 160.0);
    details(&mut screen);
    child_diff(&mut screen);
    click(&mut screen, point);
    assert_eq!(screen.state().current_pane_id(), pane);
    assert!(screen.state().diff.is_none() && screen.state().git_detail_open());
    click(&mut screen, point);
    assert_eq!(screen.state().current_pane_id(), pane);
    assert!(!screen.state().git_detail_open());
    assert_source_unchanged(&screen, id, 0..0);
}

#[test]
fn first_git_modal_frame_owns_a_held_inactive_tab_release() {
    for child in [false, true] {
        let mut app = fixture();
        let left = app.current_pane_id();
        let right = app.split_active(split::SplitOrientation::Vertical).unwrap();
        app.activate_tab(0, 1);
        assert!(
            app.control_request(crate::cli::Request::Layout {
                change: crate::cli::LayoutAction::Focus { pane_id: left },
            })
            .ok
        );
        let mut screen = harness(app);
        let pane = screen
            .state()
            .tab_drag_pane_rects()
            .into_iter()
            .find(|(id, _)| *id == right)
            .unwrap()
            .1;
        let target = screen
            .query_all_by_label("notes.txt")
            .find(|node| pane.contains(node.rect().center()))
            .unwrap()
            .rect()
            .center();
        // Focus the right group before starting the real held tab gesture.
        screen.event(egui::Event::PointerMoved(target));
        screen.event(egui::Event::PointerButton {
            pos: target,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        });
        screen.run_steps(1);
        let before_id = screen.state().tab().unwrap().id;
        let before_pane = screen.state().current_pane_id();
        let before_text = screen.state().tab().unwrap().text.clone();
        screen.state_mut().projects[0].git_view.detail_open = true;
        if child {
            screen.state_mut().diff = Some(("notes.txt · aaaaaaaa".into(), PATCH.into()));
        }
        // This is the first frame of the modal, without a previous modal layer.
        screen.event(egui::Event::PointerButton {
            pos: target,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        screen.run_steps(1);
        assert_eq!(screen.state().current_pane_id(), before_pane);
        assert_eq!(
            screen.state().tab().unwrap().id,
            before_id,
            "releasing an earlier tab press must not activate that tab behind the new modal"
        );
        assert_eq!(screen.state().tab().unwrap().text, before_text);
        assert!(screen.state().git_detail_open());
        assert_eq!(screen.state().diff.is_some(), child);
        screen.run_steps(3);
        assert_eq!(screen.state().tab().unwrap().id, before_id);
    }
}

#[test]
fn outside_click_also_dismisses_loading_and_failed_diffs() {
    for loading in [true, false] {
        let mut app = fixture();
        app.diff = Some(("notes.txt".into(), String::new()));
        app.diff_loading = loading;
        app.diff_error = (!loading).then(|| "Unable to load this comparison".into());
        let mut screen = harness(app);
        let id = screen.state().tab().unwrap().id;
        let point = screen.get_by_label("other.txt").rect().center();
        click(&mut screen, point);
        assert!(screen.state().diff.is_none());
        assert_source_unchanged(&screen, id, 0..0);
        assert!(
            screen
                .ctx
                .memory(|memory| memory.has_focus(screen.state().document_widget_id(id)))
        );
    }
}

#[test]
fn closing_the_last_git_modal_restores_markdown_reading_without_source_focus() {
    let mut app = fixture();
    let tab = app.tab_mut().unwrap();
    tab.path = Some(std::env::temp_dir().join("vedit-git-modals-render-only/plan.md"));
    tab.title = "plan.md".into();
    tab.text = "# Reading plan\n\nA longer paragraph to read.\n\n".repeat(40);
    tab.saved_text = tab.text.clone();
    app.set_document_view(crate::cli::DocumentView::Preview);
    let mut screen = harness(app);
    let id = screen.state().tab().unwrap().id;
    details(&mut screen);
    let point = outside_tab(&screen);
    click(&mut screen, point);
    assert!(!screen.state().git_detail_open());
    assert_eq!(
        screen.state().tab().unwrap().agent_view,
        crate::cli::DocumentView::Preview
    );
    assert!(!screen.state().tab().unwrap().focus);
    assert!(screen.ctx.memory(|memory| memory.focused().is_none()));
    assert_source_unchanged(&screen, id, 0..0);
}

#[test]
fn dense_commit_metadata_remains_scrollable_at_the_minimum_window_size() {
    let mut app = fixture();
    let detail = app.projects[0].git_view.detail.as_mut().unwrap();
    detail.message = format!(
        "{}\n\nThe body remains below the metadata.",
        "An unusually descriptive commit subject ".repeat(45)
    );
    detail.author = "A long author name ".repeat(10);
    detail.author_email = format!("{}@example.test", "author-alias-".repeat(20));
    detail.parents = (0..64).map(|index| format!("{index:040x}")).collect();
    let mut screen = harness(app);
    screen.set_size(Vec2::new(960.0, 640.0));
    screen.run_steps(3);
    details(&mut screen);
    let id = egui::Id::new(("git-commit-details", 0usize));
    let bounds = screen
        .ctx
        .data(|data| data.get_temp::<egui::Rect>(id.with("content_rect")))
        .unwrap();
    let close = screen.get_by_label("Close commit details").rect();
    assert!(
        bounds.top() >= 0.0 && bounds.bottom() <= 640.0,
        "dense content escaped the viewport: {bounds:?}"
    );
    assert!(close.top() >= bounds.top() && close.bottom() <= bounds.bottom());
    assert!(close.left() >= 0.0 && close.right() <= 960.0);
    fn subject_y(screen: &Harness<'_, VEditApp>) -> f32 {
        fn find(shape: &egui::Shape) -> Option<f32> {
            match shape {
                egui::Shape::Text(text)
                    if text
                        .galley
                        .job
                        .text
                        .starts_with("An unusually descriptive commit subject") =>
                {
                    Some(text.pos.y)
                }
                egui::Shape::Vec(shapes) => shapes.iter().find_map(find),
                _ => None,
            }
        }
        screen
            .output()
            .shapes
            .iter()
            .find_map(|shape| find(&shape.shape))
            .expect("commit subject is painted")
    }
    let subject_top = subject_y(&screen);
    screen.hover_at(bounds.center());
    screen.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: Vec2::new(0.0, -180.0),
        modifiers: egui::Modifiers::NONE,
        phase: egui::TouchPhase::Move,
    });
    screen.run_steps(6);
    assert!(screen.state().git_detail_open());
    assert!(
        subject_y(&screen) < subject_top - 1.0,
        "the dense metadata must actually scroll"
    );
    assert_eq!(
        screen.get_by_label("Close commit details").rect(),
        close,
        "scrolling metadata must leave the close header fixed"
    );
    click(&mut screen, close.center());
    assert!(!screen.state().git_detail_open());
}

#[test]
fn commit_modal_screenshot() {
    let mut app = fixture();
    app.projects[0].git_view.detail_open = true;
    let mut screen = visual_tests::harness(app);
    visual_tests::snapshot(&mut screen, "vedit_commit_modal");
}

#[test]
fn nested_diff_modal_screenshot() {
    let mut app = fixture();
    app.projects[0].git_view.detail_open = true;
    app.diff = Some(("notes.txt · aaaaaaaa".into(), PATCH.into()));
    let mut screen = visual_tests::harness(app);
    visual_tests::snapshot(&mut screen, "vedit_nested_diff_modal");
}

#[test]
fn wrapped_diff_modal_screenshot() {
    let mut app = fixture();
    let previous = "Read this longer explanation while reviewing the change; every part remains visible inside the diff dialog, including the last words. ".repeat(3);
    let replacement = "Die neue Beschreibung bleibt auch bei langen Zeilen lesbar: Wörter werden umgebrochen, und die Zeilennummer steht nur am Anfang. ".repeat(3);
    let identifier = "über界_long_unbroken_identifier_".repeat(12);
    let patch = format!(
        "diff --git a/notes.txt b/notes.txt\nindex aaaaaaa..bbbbbbb 100644\n--- a/notes.txt\n+++ b/notes.txt\n@@ -41,3 +41,4 @@ Review notes\n-{previous}\n+{replacement}\n+{identifier}\n Keep the surrounding context readable.\n Final context line.\n"
    );
    app.diff = Some(("notes.txt · Working tree".into(), patch));
    let mut screen = visual_tests::harness(app);
    visual_tests::snapshot(&mut screen, "vedit_wrapped_diff_modal");
}

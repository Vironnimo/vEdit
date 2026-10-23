use super::*;
use egui_kittest::{Harness, LazyRenderer, kittest::Queryable};

// Representative editor content is kept in memory: visual review must never
// write over a repository fixture or depend on rust-analyzer startup timing.
const RUST_DOCUMENT: &str = r#"use std::path::{Path, PathBuf};

/// A workspace keeps related files together.
#[derive(Debug, Clone)]
pub struct Workspace {
    root: PathBuf,
    open_files: Vec<PathBuf>,
}

impl Workspace {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            open_files: Vec::new(),
        }
    }

    pub fn open(&mut self, relative: &Path) -> Option<&Path> {
        let path = self.root.join(relative);
        if !path.is_file() {
            return None;
        }

        self.open_files.push(path);
        self.open_files.last().map(PathBuf::as_path)
    }
}

fn main() {
    let mut workspace = Workspace::new("./notes");
    match workspace.open(Path::new("hello.rs")) {
        Some(path) => println!("Opened {}", path.display()),
        None => eprintln!("Choose a file to get started."),
    }
}
"#;

const MARKDOWN_DOCUMENT: &str = r#"# Workspace notes

A quiet place to **write code**, review changes, and keep project notes.

> Vim motions stay on by default. Press `Shift+V` to select whole lines.

## Get started

1. Open a project folder.
2. Find a file with `Ctrl+P`.
3. Make a small change and save with `:w`.

## Run a check

```rust
fn main() {
    println!("Hello, workspace!");
}
```

## Editing checklist

- [x] Open the workspace
- [x] Keep files in view
- [ ] Review the next change

## Useful shortcuts

| Action | Shortcut |
| --- | --- |
| Command palette | Ctrl+Shift+P |
| Go to definition | gd / F12 |
| Visual lines | Shift+V |

Read more in the [Rust book](https://doc.rust-lang.org/book/).
"#;

const TABLE_DOCUMENT: &str = r#"# Workspace decisions

Keep the next steps close to the code. Each row has enough detail to read without opening another document.

| Area | Current behavior | Next step |
| --- | --- | --- |
| File navigation | Switching between open files keeps each document's cursor, selection, and scroll position. Long names remain available in the tab strip. | Use `Ctrl+P` to find a file, then keep related documents beside each other while reviewing a change. |
| Markdown notes | The preview wraps long explanations within each column. Inline **emphasis** and `code examples` stay readable together with the surrounding words. | Check the row below after resizing the window; all text should remain inside its own row with enough breathing room. |
| Review changes | Review notes point to the relevant source lines without changing the saved document. A second pane can show the same document independently. | Compare the source with its preview, follow a review note, and return to the previous cursor position when finished. |

## After the table

This paragraph starts after the last complete row. It should never overlap the table, even in a narrow pane.
"#;

pub(super) fn harness(app: VEditApp) -> Harness<'static, VEditApp> {
    let mut setup = egui_wgpu::WgpuSetup::without_display_handle();
    if let egui_wgpu::WgpuSetup::CreateNew(options) = &mut setup {
        options.instance_descriptor.backends = egui_wgpu::wgpu::Backends::GL;
    }
    let harness = Harness::builder()
        .with_size([1440.0, 920.0])
        .with_pixels_per_point(1.0)
        .with_theme(egui::Theme::Dark)
        .with_os(egui::os::OperatingSystem::Windows)
        .renderer(LazyRenderer::new(move || {
            egui_kittest::wgpu::WgpuTestRenderer::from_setup(setup)
        }))
        .build_ui_state(
            |ui, app: &mut VEditApp| {
                app.ctx = ui.ctx().clone();
                // Rendering fixtures must never schedule disk, Git, or
                // recovery work while waiting for background highlighting.
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
    set_style(&harness.ctx);
    // The app theme replaces Visuals after Harness disabled blinking. Restore
    // deterministic test settings after that replacement; product behavior is
    // unchanged and comparison tolerances stay strict.
    harness.ctx.all_styles_mut(|style| {
        style.visuals.text_cursor.blink = false;
        style.scroll_animation = egui::style::ScrollAnimation::none();
        style.animation_time = 0.0;
    });
    harness
}

fn document(root: &Path, relative: &str, text: &str) -> EditorTab {
    let mut tab = EditorTab::new(Some(root.join(relative)), file_title(Path::new(relative)));
    tab.text = text.into();
    tab.saved_text = text.into();
    tab.recovery_path = recovery_directory().join(format!("visual-{}.txt", tab.id));
    tab
}

pub(super) fn workspace(app: &mut VEditApp) {
    // A user can open the example checkout in the live editor. Render fixtures
    // must not pick up folders or edits made there during a test run.
    static ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    let root = ROOT
        .get_or_init(|| {
            let unique = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir()
                .join(format!("vedit-visual-{}-{unique}", std::process::id()))
                // Keep the displayed three-component suffix deterministic.
                .join("tests/fixtures/tiny-project");
            fs::create_dir_all(root.join("src")).unwrap();
            fs::write(root.join("src/main.rs"), RUST_DOCUMENT).unwrap();
            fs::write(root.join("README.md"), MARKDOWN_DOCUMENT).unwrap();
            fs::canonicalize(root).expect("isolated visual review fixture")
        })
        .clone();
    let index = workspace::build_index(&root, workspace::IndexOptions::default())
        .expect("read-only fixture index");
    let mut project = Project::new(root.clone());
    project.index = Some(Arc::new(index));
    project.git_branch = Some("main".into());
    project.last_git_refresh = Instant::now();
    project.tabs = vec![
        document(&root, "src/main.rs", RUST_DOCUMENT),
        document(&root, "README.md", MARKDOWN_DOCUMENT),
    ];
    app.projects = vec![project];
    app.active_project = 0;
    app.sidebar = SidebarView::Explorer;
    app.settings.autosave = false;
    app.session_dirty = false;
    assert!(
        app.settings.vim,
        "Vim is the default in every review fixture"
    );
}

pub(super) fn settle_render(screen: &mut Harness<'_, VEditApp>) {
    // Keep periodic disk/Git/session work out of render-only review fixtures.
    let app = screen.state_mut();
    app.last_disk_check = Instant::now();
    app.last_session_save = Instant::now();
    app.session_dirty = false;
    for project in &mut app.projects {
        project.last_git_refresh = Instant::now();
    }
    screen.hover_at(egui::pos2(-20.0, -20.0));
    screen.run_steps(3);
    // Highlighting deliberately arrives from a worker after the first plain
    // render. Wait for that result so baseline colors are deterministic.
    let deadline = Instant::now() + Duration::from_secs(8);
    while source_highlighting_pending(screen.state()) || screen.state().text_statistics.pending() {
        assert!(Instant::now() < deadline, "visual syntax worker timed out");
        screen.state_mut().last_disk_check = Instant::now();
        std::thread::sleep(Duration::from_millis(2));
        screen.run_steps(1);
    }
    screen.run_steps(2);
}

pub(super) fn snapshot(screen: &mut Harness<'_, VEditApp>, name: &str) {
    settle_render(screen);
    if let Some(directory) = std::env::var_os("VEDIT_SNAPSHOT_DIR") {
        screen.snapshot_options(
            name,
            &egui_kittest::SnapshotOptions::new().output_path(PathBuf::from(directory)),
        );
    } else {
        screen.snapshot(name);
    }
}

fn source_highlighting_pending(app: &VEditApp) -> bool {
    // The focused pane can be a preview while another pane renders source.
    // Settle every visible source, rather than only the globally active tab.
    app.layout_status()["panes"]
        .as_array()
        .expect("editor panes")
        .iter()
        .filter(|pane| pane["view"] == "source")
        .filter_map(|pane| pane["tab_id"].as_u64())
        .any(|id| {
            app.projects
                .iter()
                .flat_map(|project| &project.tabs)
                .find(|tab| tab.id == id)
                .is_some_and(|tab| {
                    !app.syntax.is_ready(
                        tab.id,
                        &tab.text,
                        syntax_language(tab.path.as_deref()),
                        app.settings.font_size,
                    )
                })
        })
}

#[test]
fn project_switcher_screenshot() {
    let mut app = VEditApp::empty(egui::Context::default());
    workspace(&mut app);
    for (folder, dirty) in [
        ("agent-plans", true),
        ("api-service", false),
        ("game-client", false),
    ] {
        let root = PathBuf::from("C:/Development/projects").join(folder);
        let mut project = Project::new(root.clone());
        project.last_git_refresh = Instant::now();
        if folder != "game-client" {
            let mut tab = document(&root, "README.md", MARKDOWN_DOCUMENT);
            if dirty {
                tab.text.push_str("\nPending notes\n");
                tab.changed();
            }
            project.tabs.push(tab);
        }
        app.projects.push(project);
    }
    let mut screen = harness(app);
    screen.get_by_label("Switch project").click();
    screen.run_steps(3);
    assert!(screen.state().workspace_picker_open());
    snapshot(&mut screen, "vedit_project_picker");
}

#[test]
fn project_tabs_and_git_indicators_screenshots() {
    let mut app = VEditApp::empty(egui::Context::default());
    workspace(&mut app);
    app.projects[0].git_indicator_count = Some(2);
    for (name, count) in [("vBot", 7), ("notes", 0), ("game-client", 0)] {
        let mut project = Project::new(std::env::temp_dir().join(name));
        project.last_git_refresh = Instant::now();
        project.git_indicator_count = Some(count);
        app.projects.push(project);
    }
    let mut screen = harness(app);
    snapshot(&mut screen, "vedit_project_tabs");
    screen.get_by_label("Project: vBot").click_secondary();
    screen.run_steps(3);
    snapshot(&mut screen, "vedit_project_context_menu");
    screen.key_press(Key::Escape);
    screen.run_steps(2);
    screen.set_size(Vec2::new(500.0, 360.0));
    snapshot(&mut screen, "vedit_project_tabs_compact");
}

#[test]
fn polished_screenshots() {
    let mut app = VEditApp::empty(egui::Context::default());
    app.recovery_session_path = recovery_directory().join("visual-session.json");
    let mut screen = harness(app);
    snapshot(&mut screen, "vedit_welcome");

    workspace(screen.state_mut());
    snapshot(&mut screen, "vedit_workspace");
    screen
        .query_all_by_label("main.rs")
        .min_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
        .expect("source document tab")
        .click_secondary();
    screen.run_steps(2);
    assert!(
        screen
            .query_by_label_contains("Copy relative path")
            .is_some()
    );
    snapshot(&mut screen, "vedit_tab_context_menu");
    screen.key_press(Key::Escape);
    screen.run_steps(2);
    screen.set_size(Vec2::new(1040.0, 720.0));
    snapshot(&mut screen, "vedit_compact_workspace");
    screen.set_size(Vec2::new(1440.0, 920.0));

    let tab = screen.state_mut().tab_mut().unwrap();
    tab.vim.cursor = editor::go_to_line(&tab.text, 10);
    tab.vim.handle_key(&mut tab.text, VimKey::Char('V'));
    tab.vim.handle_key(&mut tab.text, VimKey::Char('j'));
    tab.vim.handle_key(&mut tab.text, VimKey::Char('j'));
    tab.focus = true;
    tab.pending_selection = tab.vim.selection(&tab.text);
    assert_eq!(tab.vim.mode, VimMode::VisualLine);
    snapshot(&mut screen, "vedit_visual");

    let tab = screen.state_mut().tab_mut().unwrap();
    tab.vim.handle_key(&mut tab.text, VimKey::Escape);
    tab.set_cursor(0..0);
    let app = screen.state_mut();
    app.output_visible = true;
    let active = app.active_project;
    app.projects[active].run.output =
        "> cargo check\n\n    Checking example v0.1.0\n    Finished `dev` profile in 0.42s\n"
            .into();
    app.command = "cargo check".into();
    app.projects[active].run.status = "Completed".into();
    snapshot(&mut screen, "vedit_output");

    let app = screen.state_mut();
    app.output_visible = false;
    app.open_palette(Palette::Commands);
    app.palette_query = "show".into();
    snapshot(&mut screen, "vedit_command_palette");

    let app = screen.state_mut();
    app.palette = None;
    app.settings_open = true;
    snapshot(&mut screen, "vedit_settings");

    let app = screen.state_mut();
    app.settings_open = false;
    app.activate_tab(0, 1);
    app.set_document_view(crate::cli::DocumentView::Preview);
    snapshot(&mut screen, "vedit_markdown_preview");

    let app = screen.state_mut();
    review(app);
    snapshot(&mut screen, "vedit_review");

    let app = screen.state_mut();
    app.clear_review_sidebar();
    app.show_sidebar(SidebarView::Explorer);
    app.review = None;
    app.activate_tab(0, 1);
    app.set_document_view(crate::cli::DocumentView::Source);
    let markdown_id = app.tab().unwrap().id;
    app.control_layout(crate::cli::LayoutAction::Split {
        orientation: crate::cli::SplitOrientation::Vertical,
        pane_id: None,
        tab_id: Some(markdown_id),
        view: Some(crate::cli::DocumentView::Preview),
    })
    .expect("in-memory source and preview split");
    snapshot(&mut screen, "vedit_source_preview_split");

    let app = screen.state_mut();
    let focused = app.layout_status()["focused_pane"].as_u64().unwrap();
    app.control_layout(crate::cli::LayoutAction::Close { pane_id: focused })
        .expect("return to one pane");
    let root = app.projects[0].root.clone();
    app.projects[0].tabs[1] = document(&root, "README.md", TABLE_DOCUMENT);
    app.activate_tab(0, 1);
    app.set_document_view(crate::cli::DocumentView::Preview);
    snapshot(&mut screen, "vedit_markdown_table");
    screen.set_size(Vec2::new(840.0, 1120.0));
    snapshot(&mut screen, "vedit_narrow_markdown_table");

    screen.set_size(Vec2::new(1040.0, 920.0));
    let app = screen.state_mut();
    let markdown_id = app.tab().unwrap().id;
    app.control_layout(crate::cli::LayoutAction::Split {
        orientation: crate::cli::SplitOrientation::Vertical,
        pane_id: None,
        tab_id: Some(markdown_id),
        view: Some(crate::cli::DocumentView::Source),
    })
    .expect("narrow source and table preview split");
    snapshot(&mut screen, "vedit_narrow_source_preview_split");
}

fn review(app: &mut VEditApp) {
    use crate::cli::{Annotation, AnnotationSeverity, Location, ReviewSummary};
    app.activate_tab(0, 0);
    let tab = &mut app.projects[0].tabs[0];
    tab.set_cursor(0..0);
    tab.annotations = vec![
        Annotation {
            id: "construct".into(),
            start: Location {
                line: 11,
                column: 1,
            },
            end: Some(Location {
                line: 15,
                column: 9,
            }),
            note: "Construct a workspace without disk access".into(),
            severity: AnnotationSeverity::Info,
        },
        Annotation {
            id: "missing-file".into(),
            start: Location {
                line: 19,
                column: 1,
            },
            end: Some(Location {
                line: 26,
                column: 5,
            }),
            note: "Check the behavior when a file is missing".into(),
            severity: AnnotationSeverity::Warning,
        },
    ];
    tab.annotations_revision = Some(tab.revision);
    app.review = Some(ReviewSummary {
        title: "Workspace review".into(),
        note: "Two areas to check before applying this change.".into(),
        files: app.projects[0].tabs.iter().map(|tab| tab.id).collect(),
    });
    app.set_sidebar_view_visible(SidebarView::Explorer, false);
    app.show_sidebar(SidebarView::Review);
}

#[test]
fn menu_hover_explains_actions_and_keyboard_navigation_preserves_the_document() {
    let mut app = VEditApp::empty(egui::Context::default());
    workspace(&mut app);
    let original_text = app.tab().unwrap().text.clone();
    let original_cursor = app.tab().unwrap().vim.cursor;
    let mut screen = harness(app);
    screen.run_steps(3);

    screen.get_by_label("Main menu").hover();
    screen.run_steps(35);
    assert!(
        screen
            .query_by_label("File, Selection, View and Run actions")
            .is_some()
    );
    screen.get_by_label("Main menu").click();
    screen.run_steps(2);
    screen.get_by_label("File").click();
    screen.run_steps(2);
    assert!(screen.query_by_label_contains("Open project").is_some());
    assert!(screen.query_by_label_contains("Save all").is_some());
    screen
        .input_mut()
        .events
        .push(egui::Event::Text("i".into()));
    screen.step();
    assert_eq!(screen.state().tab().unwrap().text, original_text);
    assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::Normal);

    screen.key_press(Key::ArrowDown);
    screen.run_steps(1);
    screen.key_press(Key::Escape);
    screen.run_steps(2);
    if egui::Popup::is_any_open(&screen.ctx) {
        screen.key_press(Key::Escape);
        screen.run_steps(2);
    }
    assert_eq!(screen.state().tab().unwrap().text, original_text);
    assert_eq!(screen.state().tab().unwrap().vim.cursor, original_cursor);
    assert!(screen.query_by_label_contains("Open project").is_none());
    assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::Normal);
    let id = screen.state().tab().unwrap().id;
    assert!(
        screen
            .ctx
            .memory(|memory| memory.has_focus(egui::Id::new(("document", id)))),
        "Escape from a menu must restore editor focus"
    );
    screen
        .input_mut()
        .events
        .push(egui::Event::Text("l".into()));
    screen.step();
    assert_eq!(
        screen.state().tab().unwrap().vim.cursor,
        original_cursor + 1
    );
    assert_eq!(screen.state().tab().unwrap().text, original_text);
}

#[test]
fn markdown_preview_toggle_preserves_source_and_shift_v_keeps_selecting_lines() {
    let mut app = VEditApp::empty(egui::Context::default());
    workspace(&mut app);
    app.projects[0].active_tab = 1;
    let mut screen = harness(app);
    screen.run_steps(3);
    let source_id = screen.state().tab().unwrap().id;
    let preview_shortcut = egui::Modifiers::COMMAND.plus(egui::Modifiers::ALT);

    screen.key_press_modifiers(preview_shortcut, Key::M);
    screen.run_steps(3);
    assert_eq!(
        screen.state().tab().unwrap().agent_view,
        crate::cli::DocumentView::Preview
    );
    assert!(screen.query_by_label("Workspace notes").is_some());
    assert!(
        !screen
            .ctx
            .memory(|memory| memory.has_focus(egui::Id::new(("document", source_id))))
    );
    screen.key_press(Key::J);
    screen.run_steps(1);
    assert_eq!(screen.state().tab().unwrap().text, MARKDOWN_DOCUMENT);
    assert!(!screen.state().tab().unwrap().dirty);

    screen.key_press_modifiers(preview_shortcut, Key::M);
    screen.run_steps(3);
    assert_eq!(
        screen.state().tab().unwrap().agent_view,
        crate::cli::DocumentView::Source
    );
    assert!(
        screen
            .ctx
            .memory(|memory| memory.has_focus(egui::Id::new(("document", source_id))))
    );
    let tab = screen.state_mut().tab_mut().unwrap();
    tab.set_cursor(0..0);
    screen.run_steps(1);
    let key = |pressed| egui::Event::Key {
        key: Key::V,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::SHIFT,
    };
    screen.input_mut().events.extend([
        egui::Event::ModifiersChanged(egui::Modifiers::SHIFT),
        key(true),
        egui::Event::Text("V".into()),
        key(false),
        egui::Event::ModifiersChanged(egui::Modifiers::NONE),
    ]);
    screen.step();
    assert_eq!(screen.state().tab().unwrap().vim.mode, VimMode::VisualLine);
    assert_eq!(
        screen.state().tab().unwrap().selection,
        0.."# Workspace notes\n".chars().count()
    );
    assert_eq!(
        screen.state().tab().unwrap().agent_view,
        crate::cli::DocumentView::Source
    );
    assert_eq!(screen.state().tab().unwrap().text, MARKDOWN_DOCUMENT);
}

#[test]
fn tab_toolbar_pointer_actions_and_context_split_preserve_the_source() {
    let mut app = VEditApp::empty(egui::Context::default());
    workspace(&mut app);
    app.activate_tab(0, 1);
    let document_id = app.tab().unwrap().id;
    let mut screen = harness(app);
    screen.run_steps(3);

    // Exercise the actual pointer targets in the tab strip, rather than the
    // keyboard command route or AccessKit's direct click action.
    screen.get_by_label("Toggle Markdown preview").click();
    screen.run_steps(3);
    assert_eq!(
        screen.state().tab().unwrap().agent_view,
        crate::cli::DocumentView::Preview
    );
    assert!(screen.query_by_label("Workspace notes").is_some());
    assert!(screen.ctx.memory(|memory| memory.focused().is_none()));
    screen.key_press(Key::J);
    screen.run_steps(1);
    assert_eq!(screen.state().tab().unwrap().text, MARKDOWN_DOCUMENT);
    assert!(!screen.state().tab().unwrap().dirty);

    screen.get_by_label("Toggle Markdown preview").click();
    screen.run_steps(3);
    assert_eq!(
        screen.state().tab().unwrap().agent_view,
        crate::cli::DocumentView::Source
    );
    assert!(
        screen
            .ctx
            .memory(|memory| memory.has_focus(egui::Id::new(("document", document_id))))
    );
    let before = screen.state().tab().unwrap().vim.cursor;
    screen
        .input_mut()
        .events
        .push(egui::Event::Text("l".into()));
    screen.step();
    assert_eq!(screen.state().tab().unwrap().vim.cursor, before + 1);

    screen
        .query_all_by_label("README.md")
        .min_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
        .expect("README document tab, above its explorer row")
        .click_secondary();
    screen.run_steps(2);
    assert!(egui::Popup::is_any_open(&screen.ctx));
    assert!(
        screen
            .query_by_label_contains("Copy relative path")
            .is_some()
    );
    screen.get_by_label_contains("Split right").click();
    screen.run_steps(3);
    let layout = screen.state().layout_status();
    let panes = layout["panes"].as_array().unwrap();
    assert_eq!(panes.len(), 2);
    assert!(
        panes
            .iter()
            .all(|pane| pane["tab_id"] == document_id && pane["view"] == "source")
    );
    assert!(!egui::Popup::is_any_open(&screen.ctx));
    assert!(
        !screen.output().shapes.iter().any(|clipped| matches!(
            &clipped.shape,
            egui::Shape::Text(text) if text.galley.job.text.contains("use of widget ID")
        )),
        "Repeated tabs in separate panes must have distinct interaction IDs"
    );

    // The right pane's controls must act on that pane independently, while
    // both panes continue to share the same clean document.
    screen
        .query_all_by_label("Toggle Markdown preview")
        .max_by(|a, b| a.rect().left().total_cmp(&b.rect().left()))
        .expect("right-pane preview control")
        .click();
    screen.run_steps(3);
    let layout = screen.state().layout_status();
    let panes = layout["panes"].as_array().unwrap();
    assert_eq!(panes[0]["view"], "source");
    assert_eq!(panes[1]["view"], "preview");
    assert_eq!(layout["focused_pane"], panes[1]["id"]);
    assert_eq!(screen.state().projects[0].tabs.len(), 2);
    assert_eq!(screen.state().tab().unwrap().text, MARKDOWN_DOCUMENT);
    assert!(!screen.state().tab().unwrap().dirty);

    let original_tabs = screen.state().projects[0]
        .tabs
        .iter()
        .map(|tab| tab.id)
        .collect::<Vec<_>>();
    assert_eq!(panes[0]["tab_ids"], serde_json::json!(original_tabs));
    assert_eq!(panes[1]["tab_ids"], serde_json::json!([document_id]));

    // Click the actual right-pane tab X. Closing its last local tab removes
    // that split, while the shared document stays open in the original group.
    let right_tab = screen
        .query_all_by_label("README.md")
        .filter(|node| node.rect().top() < 90.0)
        .max_by(|a, b| a.rect().left().total_cmp(&b.rect().left()))
        .expect("right-pane README tab")
        .rect();
    let close_position = egui::pos2(right_tab.right() - 16.0, right_tab.center().y);
    screen.event(egui::Event::PointerMoved(close_position));
    screen.event(egui::Event::PointerButton {
        pos: close_position,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    screen.run_steps(1);
    screen.event(egui::Event::PointerButton {
        pos: close_position,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    screen.run_steps(3);
    let collapsed = screen.state().layout_status();
    let remaining = collapsed["panes"].as_array().unwrap();
    assert_eq!(remaining.len(), 1, "last local tab closes its split");
    assert_eq!(remaining[0]["id"], panes[0]["id"]);
    assert_eq!(remaining[0]["tab_ids"], serde_json::json!(original_tabs));
    assert_eq!(remaining[0]["tab_id"], document_id);
    assert_eq!(remaining[0]["view"], "source");
    assert_eq!(screen.state().projects[0].tabs.len(), 2);
    assert_eq!(screen.state().tab().unwrap().text, MARKDOWN_DOCUMENT);
    assert!(!screen.state().tab().unwrap().dirty);
    assert!(screen.state().close_action.is_none());
}

#[test]
fn review_note_reveals_source_and_clear_keeps_documents_open() {
    let mut app = VEditApp::empty(egui::Context::default());
    workspace(&mut app);
    review(&mut app);
    let mut screen = harness(app);
    screen.run_steps(3);
    screen
        .get_by_label_contains("Check the behavior when a file is missing")
        .click();
    screen.run_steps(3);
    let tab = screen.state().tab().unwrap();
    assert_eq!(tab.agent_view, crate::cli::DocumentView::Source);
    assert_eq!(tab.vim.cursor, editor::go_to_line(&tab.text, 19));
    assert!(!tab.dirty);

    screen.get_by_label("Clear").click();
    screen.run_steps(3);
    assert!(screen.state().review.is_none());
    assert!(!screen.state().sidebar_visible);
    assert_eq!(
        screen.state().sidebars_status()["left"]["panels"],
        serde_json::json!([])
    );
    assert_eq!(screen.state().projects[0].tabs.len(), 2);
    assert!(
        screen.state().projects[0]
            .tabs
            .iter()
            .all(|tab| tab.annotations.is_empty())
    );
    assert_eq!(screen.state().projects[0].tabs[0].text, RUST_DOCUMENT);
    assert_eq!(screen.state().projects[0].tabs[1].text, MARKDOWN_DOCUMENT);
}

#[test]
fn tab_strip_reveals_new_active_tabs_and_preserves_manual_scrolling() {
    let mut app = VEditApp::empty(egui::Context::default());
    workspace(&mut app);
    let root = app.projects[0].root.clone();
    let titles: Vec<_> = (0..10)
        .map(|index| format!("document_{index:02}_with_a_long_descriptive_file_name.rs"))
        .collect();
    app.projects[0].tabs = titles
        .iter()
        .map(|title| document(&root, title, "fn main() {}\n"))
        .collect();
    app.bind_current_pane();
    assert_eq!(
        app.current_pane_tab_ids(),
        app.projects[0]
            .tabs
            .iter()
            .map(|tab| tab.id)
            .collect::<Vec<_>>()
    );
    let mut strip = Harness::builder()
        .with_size([1040.0, 240.0])
        .with_theme(egui::Theme::Dark)
        .with_os(egui::os::OperatingSystem::Windows)
        .build_ui_state(
            |ui, app: &mut VEditApp| {
                app.ctx = ui.ctx().clone();
                app.tabs_ui(ui);
            },
            app,
        );
    set_style(&strip.ctx);
    strip.ctx.all_styles_mut(|style| {
        style.animation_time = 0.0;
        style.scroll_animation = egui::style::ScrollAnimation::none();
    });
    strip.run_steps(3);

    let visible = |strip: &Harness<'_, VEditApp>, title: &str| {
        let rect = strip.get_by_label(title).rect();
        // The scroll bar exposes the actual tab viewport, excluding the fixed
        // controls on the right. This also survives nested UI id changes.
        let viewport = strip.get_by_role(egui::accesskit::Role::ScrollBar).rect();
        rect.left() >= viewport.left() - 0.5 && rect.right() <= viewport.right() + 0.5
    };
    let initial_left = strip.get_by_label(&titles[0]).rect().left();
    let offset =
        |strip: &Harness<'_, VEditApp>| initial_left - strip.get_by_label(&titles[0]).rect().left();
    assert!(visible(&strip, &titles[0]));
    assert!(!visible(&strip, &titles[9]));

    // The common activation path is used by CLI, palette, keyboard and clicks.
    strip.state_mut().activate_tab(0, 9);
    strip.run_steps(3);
    assert!(
        visible(&strip, &titles[9]),
        "last active tab must be visible"
    );
    assert!(offset(&strip) > 1000.0);

    // Move the strip deliberately away from its selected last document.
    strip.hover_at(egui::pos2(500.0, 20.0));
    strip.input_mut().events.push(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: Vec2::new(600.0, 0.0),
        phase: egui::TouchPhase::Move,
        modifiers: egui::Modifiers::NONE,
    });
    strip.run_steps(5);
    let manually_scrolled = offset(&strip);
    assert!(!visible(&strip, &titles[9]));
    strip.run_steps(3);
    assert!(
        (offset(&strip) - manually_scrolled).abs() < 0.5,
        "idle frames must preserve the user's strip scroll position"
    );
    assert_eq!(strip.state().projects[0].active_tab, 9);

    strip.state_mut().activate_tab(0, 0);
    strip.run_steps(3);
    assert!(
        visible(&strip, &titles[0]),
        "first active tab must be visible"
    );
    assert!(offset(&strip) < 0.5);
}

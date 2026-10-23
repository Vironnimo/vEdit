//! The quick picker's backdrop owns dismissal clicks and preserves editor state.

use super::*;
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};

const DOCUMENT: &str = "# Plan\n\nfirst review line\nsecond review line\nthird review line\n";

struct Fixture {
    root: PathBuf,
    files: Vec<PathBuf>,
}

impl Fixture {
    fn new(count: usize) -> (Self, VEditApp) {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "vedit-palette-ui-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let files: Vec<_> = (0..count)
            .map(|index| root.join(format!("document-{index:02}.md")))
            .collect();
        for file in &files {
            fs::write(file, DOCUMENT).unwrap();
        }
        let mut app = VEditApp::empty(egui::Context::default());
        app.settings.autosave = false;
        app.settings.smooth_scroll = false;
        let mut project = Project::new(root.clone());
        project.last_git_refresh = Instant::now();
        project.index = Some(Arc::new(
            workspace::build_index(&root, Default::default()).unwrap(),
        ));
        for file in &files {
            let mut tab = EditorTab::new(Some(file.clone()), file_title(file));
            tab.text = DOCUMENT.into();
            tab.saved_text = DOCUMENT.into();
            project.tabs.push(tab);
        }
        app.projects.push(project);
        app.bind_current_pane();
        (Self { root, files }, app)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Remove only the exact fixture files; never recurse into an arbitrary path.
        for file in &self.files {
            let _ = fs::remove_file(file);
        }
        let _ = fs::remove_dir(&self.root);
    }
}

fn harness(app: VEditApp) -> Harness<'static, VEditApp> {
    let mut screen = Harness::builder()
        .with_size([1360.0, 900.0])
        .with_os(egui::os::OperatingSystem::Windows)
        .build_ui_state(
            |ui, app: &mut VEditApp| {
                app.ctx = ui.ctx().clone();
                // All buffers are already loaded. Rendering these isolated fixtures
                // must not write sessions, poll disk, refresh Git, or autosave.
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
        style.animation_time = 0.0;
        style.visuals.text_cursor.blink = false;
        style.scroll_animation = egui::style::ScrollAnimation::none();
    });
    screen.run_steps(3);
    screen
}

fn open(screen: &mut Harness<'_, VEditApp>, palette: Palette) {
    screen.state_mut().open_palette(palette);
    screen.run_steps(3);
    assert!(screen.state().palette == Some(palette));
    assert_query_focus(screen);
}

fn assert_query_focus(screen: &Harness<'_, VEditApp>) {
    let query = egui::Id::new("palette_input");
    assert_eq!(screen.ctx.memory(|memory| memory.focused()), Some(query));
    assert!(screen.ctx.read_response(query).is_some());
}

fn assert_source_focus(screen: &Harness<'_, VEditApp>) {
    let id = screen
        .state()
        .document_widget_id(screen.state().tab().unwrap().id);
    assert_eq!(screen.ctx.memory(|memory| memory.focused()), Some(id));
    assert!(screen.state().editor_focused);
}

fn click(screen: &mut Harness<'_, VEditApp>, point: egui::Pos2, button: egui::PointerButton) {
    screen.hover_at(point);
    screen.event(egui::Event::PointerButton {
        pos: point,
        button,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    screen.run_steps(1);
    screen.event(egui::Event::PointerButton {
        pos: point,
        button,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    screen.run_steps(3);
}

fn text(screen: &mut Harness<'_, VEditApp>, value: &str) {
    screen.event(egui::Event::Text(value.into()));
    screen.run_steps(2);
}

fn prepare_source(screen: &mut Harness<'_, VEditApp>, mode: VimMode) {
    let tab = screen.state_mut().tab_mut().unwrap();
    let at = editor::go_to_line(&tab.text, 3) + 2;
    tab.set_cursor(at..at);
    match mode {
        VimMode::Insert => {
            tab.vim.handle_key(&mut tab.text, VimKey::Char('i'));
        }
        VimMode::VisualLine => {
            tab.vim.handle_key(&mut tab.text, VimKey::Char('V'));
            tab.vim.handle_key(&mut tab.text, VimKey::Char('j'));
            tab.pending_selection = tab.vim.selection(&tab.text);
        }
        _ => {}
    }
    screen.run_steps(2);
    assert_eq!(screen.state().tab().unwrap().vim.mode, mode);
}

fn source_click_point(screen: &Harness<'_, VEditApp>) -> egui::Pos2 {
    let rect = screen.state().editor_rect.unwrap();
    let point = rect.left_top() + Vec2::new(6.0, 5.0);
    assert!(rect.contains(point));
    assert!(point.y < 125.0, "the source target is above the picker");
    point
}

#[derive(Debug, PartialEq)]
struct EditorState {
    id: u64,
    text: String,
    saved_text: String,
    cursor: usize,
    selection: Range<usize>,
    visual: Option<Range<usize>>,
    mode: VimMode,
    dirty: bool,
    revision: u64,
}

fn editor_state(app: &VEditApp) -> EditorState {
    let tab = app.tab().unwrap();
    EditorState {
        id: tab.id,
        text: tab.text.clone(),
        saved_text: tab.saved_text.clone(),
        cursor: tab.vim.cursor,
        selection: tab.selection.clone(),
        visual: tab.vim.selection(&tab.text),
        mode: tab.vim.mode,
        dirty: tab.dirty,
        revision: tab.revision,
    }
}

#[test]
fn outside_source_click_preserves_insert_and_the_next_typed_character() {
    for palette in [Palette::Files, Palette::Commands, Palette::Line] {
        let (_fixture, app) = Fixture::new(2);
        let mut screen = harness(app);
        prepare_source(&mut screen, VimMode::Insert);
        let before = editor_state(screen.state());
        let point = source_click_point(&screen);
        open(&mut screen, palette);
        click(&mut screen, point, egui::PointerButton::Primary);
        assert!(screen.state().palette.is_none());
        assert_eq!(editor_state(screen.state()), before);
        assert_source_focus(&screen);

        text(&mut screen, "x");
        let expected: String = before
            .text
            .chars()
            .take(before.cursor)
            .chain("x".chars())
            .chain(before.text.chars().skip(before.cursor))
            .collect();
        let tab = screen.state().tab().unwrap();
        assert_eq!(tab.text, expected);
        assert_eq!(tab.vim.mode, VimMode::Insert);
        assert!(tab.dirty);
    }
}

#[test]
fn outside_source_click_preserves_visual_lines_and_the_next_vim_motion() {
    for palette in [Palette::Files, Palette::Commands, Palette::Line] {
        let (_fixture, app) = Fixture::new(2);
        let mut screen = harness(app);
        prepare_source(&mut screen, VimMode::VisualLine);
        let before = editor_state(screen.state());
        let point = source_click_point(&screen);
        open(&mut screen, palette);
        click(&mut screen, point, egui::PointerButton::Primary);
        assert!(screen.state().palette.is_none());
        assert_eq!(editor_state(screen.state()), before);
        assert_source_focus(&screen);

        text(&mut screen, "j");
        let tab = screen.state().tab().unwrap();
        assert_eq!(tab.text, before.text);
        assert_eq!(tab.vim.mode, VimMode::VisualLine);
        assert!(tab.vim.cursor > before.cursor);
        assert_ne!(tab.vim.selection(&tab.text), before.visual);
        assert!(!tab.dirty);
    }
}

fn row_point(screen: &Harness<'_, VEditApp>, title: &str, explorer: bool) -> egui::Pos2 {
    let rect = screen
        .query_all_by_label(title)
        .map(|node| node.rect())
        .find(|rect| (rect.top() > 110.0) == explorer)
        .unwrap_or_else(|| {
            panic!(
                "missing {} row: {title}",
                if explorer { "Explorer" } else { "tab" }
            )
        });
    rect.center()
}

fn active_activity_point(screen: &Harness<'_, VEditApp>) -> egui::Pos2 {
    fn find(shape: &egui::Shape) -> Option<egui::Pos2> {
        match shape {
            egui::Shape::Rect(shape)
                if shape.rect.left() >= 0.0
                    && shape.rect.right() < 49.0
                    && (shape.rect.width() - 35.0).abs() < 0.1
                    && (shape.rect.height() - 35.0).abs() < 0.1 =>
            {
                Some(shape.rect.center())
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
        .expect("the active Explorer activity button is painted")
}

#[test]
fn dismissal_over_main_menu_does_not_open_the_menu() {
    let (_fixture, app) = Fixture::new(2);
    let mut screen = harness(app);
    let point = screen.get_by_label("Main menu").rect().center();
    let before = editor_state(screen.state());
    open(&mut screen, Palette::Files);
    click(&mut screen, point, egui::PointerButton::Primary);
    assert!(screen.state().palette.is_none());
    assert!(!egui::Popup::is_any_open(&screen.ctx));
    assert_eq!(editor_state(screen.state()), before);
    assert_source_focus(&screen);
}

#[test]
fn dismissal_over_project_header_does_not_open_the_project_picker() {
    let (_fixture, app) = Fixture::new(2);
    let mut screen = harness(app);
    let point = screen.get_by_label("Switch project").rect().center();
    open(&mut screen, Palette::Files);
    click(&mut screen, point, egui::PointerButton::Primary);
    assert!(screen.state().palette.is_none());
    assert!(!screen.state().workspace_picker_open());
    assert_eq!(screen.state().active_project, 0);
    assert_source_focus(&screen);
}

#[test]
fn dismissal_over_activity_does_not_hide_the_explorer() {
    let (_fixture, app) = Fixture::new(2);
    let mut screen = harness(app);
    let point = active_activity_point(&screen);
    open(&mut screen, Palette::Files);
    click(&mut screen, point, egui::PointerButton::Primary);
    assert!(screen.state().palette.is_none());
    assert!(screen.state().sidebar_visible);
    assert!(screen.state().sidebar == SidebarView::Explorer);
    assert_source_focus(&screen);
}

#[test]
fn dismissal_over_an_editor_tab_does_not_activate_it() {
    let (_fixture, app) = Fixture::new(2);
    let mut screen = harness(app);
    let point = row_point(&screen, "document-01.md", false);
    let before = editor_state(screen.state());
    open(&mut screen, Palette::Files);
    click(&mut screen, point, egui::PointerButton::Primary);
    assert!(screen.state().palette.is_none());
    assert_eq!(screen.state().projects[0].active_tab, 0);
    assert_eq!(editor_state(screen.state()), before);
    assert_source_focus(&screen);
}

#[test]
fn dismissal_over_an_explorer_file_does_not_open_it() {
    let (_fixture, app) = Fixture::new(2);
    let mut screen = harness(app);
    let point = row_point(&screen, "document-01.md", true);
    let before = editor_state(screen.state());
    open(&mut screen, Palette::Files);
    click(&mut screen, point, egui::PointerButton::Primary);
    assert!(screen.state().palette.is_none());
    assert_eq!(screen.state().projects[0].active_tab, 0);
    assert_eq!(screen.state().projects[0].tabs.len(), 2);
    assert_eq!(editor_state(screen.state()), before);
    assert_source_focus(&screen);
}

#[test]
fn secondary_and_middle_dismissal_do_not_open_context_menus_or_close_tabs() {
    for button in [egui::PointerButton::Secondary, egui::PointerButton::Middle] {
        let (_fixture, app) = Fixture::new(2);
        let mut screen = harness(app);
        let point = row_point(&screen, "document-01.md", false);
        let before = editor_state(screen.state());
        open(&mut screen, Palette::Files);
        click(&mut screen, point, button);
        assert!(screen.state().palette.is_none());
        assert!(!egui::Popup::is_any_open(&screen.ctx));
        assert_eq!(screen.state().projects[0].tabs.len(), 2);
        assert_eq!(editor_state(screen.state()), before);
        assert_source_focus(&screen);
    }
}

#[test]
fn preview_dismissal_keeps_reading_view_without_source_focus() {
    for palette in [Palette::Files, Palette::Commands, Palette::Line] {
        let (_fixture, app) = Fixture::new(2);
        let mut screen = harness(app);
        screen
            .state_mut()
            .set_document_view(crate::cli::DocumentView::Preview);
        screen.run_steps(3);
        let before = editor_state(screen.state());
        let point = screen.get_by_label("Main menu").rect().center();
        open(&mut screen, palette);
        click(&mut screen, point, egui::PointerButton::Primary);
        assert!(screen.state().palette.is_none());
        assert_eq!(editor_state(screen.state()), before);
        assert!(screen.state().tab().unwrap().agent_view == crate::cli::DocumentView::Preview);
        assert!(!screen.state().editor_focused);
        assert_eq!(screen.ctx.memory(|memory| memory.focused()), None);
        text(&mut screen, "j");
        assert_eq!(screen.state().tab().unwrap().text, DOCUMENT);
        assert!(!screen.state().tab().unwrap().dirty);
    }
}

fn palette_scrollbar(screen: &Harness<'_, VEditApp>) -> egui::Rect {
    let query = screen
        .ctx
        .read_response(egui::Id::new("palette_input"))
        .unwrap()
        .rect;
    screen
        .query_all_by_role(egui::accesskit::Role::ScrollBar)
        .map(|node| node.rect())
        .find(|rect| {
            rect.width() <= 20.0
                && rect.height() > 100.0
                && rect.left() >= query.left()
                && rect.right() <= query.right() + 2.0
                && rect.top() > query.bottom()
        })
        .expect("the picker exposes its vertical result scrollbar")
}

#[derive(Clone, Copy, Debug)]
struct PaintedRect {
    rect: egui::Rect,
    clip: egui::Rect,
    fill: Color32,
}

fn rect_paints(screen: &Harness<'_, VEditApp>) -> Vec<PaintedRect> {
    fn visit(shape: &egui::Shape, clip: egui::Rect, output: &mut Vec<PaintedRect>) {
        match shape {
            egui::Shape::Rect(shape) => output.push(PaintedRect {
                rect: shape.rect,
                clip,
                fill: shape.fill,
            }),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    visit(shape, clip, output);
                }
            }
            _ => {}
        }
    }
    let mut paints = Vec::new();
    for shape in &screen.output().shapes {
        visit(&shape.shape, shape.clip_rect, &mut paints);
    }
    paints
}

fn painted_scrollbar(screen: &Harness<'_, VEditApp>) -> (egui::Rect, egui::Rect) {
    let bar = palette_scrollbar(screen);
    let paints = rect_paints(screen);
    let bars: Vec<_> = paints
        .windows(2)
        .filter_map(|pair| {
            let (track, thumb) = (pair[0], pair[1]);
            let same_x = (track.rect.left() - thumb.rect.left()).abs() < 0.6
                && (track.rect.right() - thumb.rect.right()).abs() < 0.6;
            (track.fill.a() > 0
                && thumb.fill.a() > 0
                && track.rect.width() > 0.5
                && track.rect.width() <= 16.0
                && (track.rect.top() - bar.top()).abs() < 1.0
                && (track.rect.bottom() - bar.bottom()).abs() < 1.0
                && bar.expand(0.6).contains_rect(track.rect)
                && same_x
                && track.rect.expand(0.6).contains_rect(thumb.rect)
                && track.clip.expand(0.6).contains_rect(track.rect)
                && thumb.clip.expand(0.6).contains_rect(thumb.rect))
            .then_some((track.rect, thumb.rect))
        })
        .collect();
    assert_eq!(
        bars.len(),
        1,
        "expected one painted picker track and thumb: {bars:?}"
    );
    bars[0]
}

fn assert_row_visible(screen: &Harness<'_, VEditApp>, label: &str) {
    let node = screen.get_by_label(label);
    // egui's selectable buttons expose selection as AccessKit's toggled state.
    assert_eq!(
        node.accesskit_node().toggled(),
        Some(egui::accesskit::Toggled::True)
    );
    let rect = node.rect();
    let bar = palette_scrollbar(screen);
    assert!(
        rect.top() >= bar.top() - 0.5 && rect.bottom() <= bar.bottom() + 0.5,
        "selected row is fully inside the viewport: {label}, row {rect:?}, bar {bar:?}"
    );
    let background = rect_paints(screen)
        .into_iter()
        .find(|paint| {
            paint.fill.a() > 0
                && (paint.rect.min - rect.min).length() < 0.8
                && (paint.rect.max - rect.max).length() < 0.8
        })
        .expect("the selected result must actually paint its full row background");
    assert!(background.clip.expand(0.6).contains_rect(rect));
    assert_query_focus(screen);
}

#[test]
fn files_and_commands_keep_the_scrollbar_visible_without_hover() {
    for palette in [Palette::Files, Palette::Commands] {
        let (_fixture, app) = Fixture::new(45);
        let mut screen = harness(app);
        open(&mut screen, palette);
        screen.hover_at(egui::pos2(2.0, 2.0));
        screen.run_steps(20);
        let (track, thumb) = painted_scrollbar(&screen);
        assert!(track.height() > 250.0);
        assert!(thumb.height() > 5.0 && thumb.height() < track.height() * 0.5);
        assert_query_focus(&screen);
        assert!(screen.state().palette == Some(palette));
    }
}

#[test]
fn go_to_line_accepts_unicode_columns_and_clamps_within_the_line() {
    let (_fixture, mut app) = Fixture::new(1);
    app.settings.vim = false;
    let tab = app.tab_mut().unwrap();
    tab.text = "α\ncafé🦀\r\n尾".into();
    tab.saved_text.clone_from(&tab.text);
    let mut screen = harness(app);
    for (query, expected) in [("2:4", 5), ("2:999", 7), ("3", 9)] {
        open(&mut screen, Palette::Line);
        text(&mut screen, query);
        screen.key_press(Key::Enter);
        screen.run_steps(3);
        assert!(screen.state().palette.is_none(), "{query}");
        assert_eq!(
            screen.state().tab().unwrap().vim.cursor,
            expected,
            "{query}"
        );
    }
}

#[test]
fn go_to_line_keeps_invalid_locations_open_without_moving() {
    let (_fixture, app) = Fixture::new(1);
    let mut screen = harness(app);
    for query in ["0", "2:0", "2:-1", "2:", "abc", "2:3:4"] {
        open(&mut screen, Palette::Line);
        let before = screen.state().tab().unwrap().vim.cursor;
        text(&mut screen, query);
        screen.key_press(Key::Enter);
        screen.run_steps(3);
        assert!(screen.state().palette == Some(Palette::Line), "{query}");
        assert_eq!(screen.state().tab().unwrap().vim.cursor, before, "{query}");
        assert!(
            screen
                .query_by_label("Enter a line or line:column using positive numbers.")
                .is_some()
        );
        screen.key_press(Key::Escape);
        screen.run_steps(3);
    }
}

#[test]
fn quick_open_finds_loose_files_and_untitled_drafts_without_an_index() {
    let (_fixture, mut app) = Fixture::new(2);
    app.projects[0].kind = WorkspaceKind::OtherFiles;
    app.projects[0].index = None;
    app.projects[0].languages.clear();
    app.new_file();
    let draft = app.tab_mut().unwrap();
    draft.apply(editor::replace_selection(
        &draft.text,
        0..0,
        "draft café 🦀",
    ));
    let draft_id = draft.id;
    app.activate_tab(0, 0);
    let loose_id = app.projects[0].tabs[1].id;
    let mut screen = harness(app);
    open(&mut screen, Palette::Files);
    text(&mut screen, "document-01");
    screen.key_press(Key::Enter);
    screen.run_steps(3);
    assert!(screen.state().palette.is_none());
    assert_eq!(screen.state().tab().unwrap().id, loose_id);
    open(&mut screen, Palette::Files);
    text(&mut screen, "Untitled");
    screen.key_press(Key::Enter);
    screen.run_steps(3);
    assert!(screen.state().palette.is_none());
    assert_eq!(screen.state().tab().unwrap().id, draft_id);
    assert_eq!(screen.state().tab().unwrap().text, "draft café 🦀");
    assert!(screen.state().tab().unwrap().dirty);
    assert_eq!(screen.state().projects[0].tabs.len(), 3);
    assert_source_focus(&screen);
}

#[test]
fn quick_open_includes_open_files_missing_from_the_index_and_deduplicates() {
    let (fixture, mut app) = Fixture::new(2);
    let path = fixture.root.join("ignored-notes.txt");
    let mut tab = EditorTab::new(Some(path.clone()), file_title(&path));
    tab.text = "unsaved notes".into();
    tab.dirty = true;
    let ignored_id = tab.id;
    app.projects[0].tabs.push(tab);
    let mut screen = harness(app);
    open(&mut screen, Palette::Files);
    text(&mut screen, "ignored-notes");
    screen.key_press(Key::Enter);
    screen.run_steps(3);
    assert_eq!(screen.state().tab().unwrap().id, ignored_id);
    assert_eq!(screen.state().tab().unwrap().text, "unsaved notes");
    assert!(screen.state().tab().unwrap().dirty);
    open(&mut screen, Palette::Files);
    text(&mut screen, "document-00");
    screen.key_press(Key::End);
    screen.run_steps(3);
    assert_eq!(
        screen.state().palette_selected,
        0,
        "an open indexed file appears once"
    );
}

#[test]
fn quick_open_down_home_end_and_page_keys_reveal_the_selected_file() {
    let (_fixture, app) = Fixture::new(45);
    let mut screen = harness(app);
    open(&mut screen, Palette::Files);
    let labels: Vec<_> = screen
        .state()
        .project()
        .unwrap()
        .index
        .as_ref()
        .unwrap()
        .quick_open("", 40)
        .iter()
        .map(|path| {
            let relative = path
                .strip_prefix(&screen.state().project().unwrap().root)
                .unwrap();
            format!("{}: {}", file_title(path), crate::paths::display(relative))
        })
        .collect();
    assert_eq!(labels.len(), 40);
    for key in [
        Key::ArrowDown,
        Key::End,
        Key::PageUp,
        Key::Home,
        Key::PageDown,
        Key::ArrowUp,
    ] {
        screen.key_press(key);
        screen.run_steps(3);
        let selected = screen.state().palette_selected;
        match key {
            Key::End => assert_eq!(selected, 39),
            Key::Home => assert_eq!(selected, 0),
            Key::PageUp => assert!(selected < 39),
            Key::PageDown => assert!(selected > 0),
            _ => {}
        }
        assert_row_visible(&screen, &labels[selected]);
        assert_eq!(screen.state().projects[0].active_tab, 0);
        assert_eq!(screen.state().tab().unwrap().text, DOCUMENT);
    }
}

#[test]
fn quick_open_enter_activates_only_the_selected_existing_document() {
    let (_fixture, app) = Fixture::new(45);
    let mut screen = harness(app);
    let expected = screen.state().projects[0].tabs[39].id;
    let texts: Vec<_> = screen.state().projects[0]
        .tabs
        .iter()
        .map(|tab| tab.text.clone())
        .collect();
    open(&mut screen, Palette::Files);
    screen.key_press(Key::End);
    screen.run_steps(3);
    assert_eq!(screen.state().palette_selected, 39);
    assert_eq!(screen.state().projects[0].active_tab, 0);
    screen.key_press(Key::Enter);
    screen.run_steps(3);
    assert!(screen.state().palette.is_none());
    assert_eq!(screen.state().tab().unwrap().id, expected);
    assert_eq!(screen.state().projects[0].tabs.len(), 45);
    for (tab, original) in screen.state().projects[0].tabs.iter().zip(texts) {
        assert_eq!(tab.text, original);
        assert!(!tab.dirty);
        assert!(!tab.loading);
    }
    assert_source_focus(&screen);
}

#[test]
fn commands_navigation_keeps_query_focus_and_enter_runs_only_the_selection() {
    let (_fixture, app) = Fixture::new(2);
    let mut screen = harness(app);
    open(&mut screen, Palette::Commands);
    screen.key_press(Key::End);
    screen.run_steps(3);
    let commands: Vec<_> = keybindings::Action::ALL
        .iter()
        .copied()
        .filter(|action| action.palette_visible())
        .collect();
    assert!(!commands.contains(&keybindings::Action::ContextMenu));
    assert!(!commands.contains(&keybindings::Action::ReviewSubmitComment));
    assert_eq!(screen.state().palette_selected, commands.len() - 1);
    let name = commands.last().unwrap().label();
    let hint = screen
        .state()
        .shortcut_label(keybindings::Action::from_label(name).unwrap());
    assert_row_visible(&screen, &format!("{name}: {hint}"));
    assert!(!screen.state().settings_open);
    screen.key_press(Key::Home);
    screen.run_steps(3);
    assert_eq!(screen.state().palette_selected, 0);
    assert_query_focus(&screen);
    text(&mut screen, "word wrap");
    assert_eq!(screen.state().palette_query, "word wrap");
    assert_eq!(screen.state().palette_selected, 0);
    assert_query_focus(&screen);
    let original = screen.state().settings.word_wrap;
    screen.key_press(Key::Enter);
    screen.run_steps(3);
    assert!(screen.state().palette.is_none());
    assert_eq!(screen.state().settings.word_wrap, !original);
    assert_eq!(screen.state().tab().unwrap().text, DOCUMENT);
    assert_source_focus(&screen);
}

#[test]
fn recent_palette_filters_reuses_dirty_buffers_and_clears_only_history() {
    let (fixture, mut app) = Fixture::new(2);
    app.remember_recent(fixture.files[1].clone(), recent::Kind::File);
    app.remember_recent(fixture.root.clone(), recent::Kind::Folder);
    let tab = &mut app.projects[0].tabs[1];
    tab.text = "unsaved café".into();
    tab.dirty = true;
    let id = tab.id;
    let mut screen = harness(app);
    screen
        .state_mut()
        .run_shortcut_action(keybindings::Action::OpenRecent);
    screen.run_steps(3);
    text(&mut screen, "document-01");
    assert!(
        screen
            .query_by_label_contains("File · document-01.md")
            .is_some()
    );
    screen.key_press(Key::Enter);
    screen.run_steps(3);
    assert!(screen.state().palette.is_none());
    assert_eq!(screen.state().tab().unwrap().id, id);
    assert_eq!(screen.state().tab().unwrap().text, "unsaved café");
    open(&mut screen, Palette::Recent);
    screen.get_by_label("Clear recent history").click();
    screen.run_steps(3);
    assert!(screen.state().recent.is_empty());
    assert_eq!(screen.state().project().unwrap().tabs.len(), 2);
    assert_eq!(screen.state().tab().unwrap().text, "unsaved café");
    assert!(fixture.files.iter().all(|path| path.is_file()));
    assert!(
        screen
            .query_by_label("No recent files or folders")
            .is_some()
    );
}

#[test]
fn clicking_a_result_selects_only_that_file() {
    let (_fixture, app) = Fixture::new(3);
    let mut screen = harness(app);
    let expected = screen.state().projects[0].tabs[1].id;
    open(&mut screen, Palette::Files);
    screen
        .get_by_label("document-01.md: document-01.md")
        .click();
    screen.run_steps(3);
    assert!(screen.state().palette.is_none());
    assert_eq!(screen.state().tab().unwrap().id, expected);
    assert_eq!(screen.state().projects[0].tabs.len(), 3);
    assert_source_focus(&screen);
}

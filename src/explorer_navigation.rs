//! Explorer selection is separate from the active editor document.
//! All rows come from the immutable workspace index; navigation never reads disk.

use super::*;
use std::collections::HashSet;

const ROW_HEIGHT: f32 = 28.0;

#[derive(Default)]
pub(super) struct ExplorerNavigationState {
    projects: HashMap<PathBuf, ProjectNavigation>,
    keyboard_menu: Option<(PathBuf, egui::Pos2)>,
    focus_requested: Option<PathBuf>,
}

#[derive(Default)]
struct ProjectNavigation {
    selected: Option<PathBuf>,
    expanded: HashSet<PathBuf>,
    initialized: bool,
    scroll_to_selection: bool,
    page_rows: usize,
    index_identity: usize,
    awaiting_renamed_index: bool,
}

pub(super) struct VisibleRow<'a> {
    pub(super) entry: &'a workspace::WorkspaceEntry,
    pub(super) depth: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Move {
    Up,
    Down,
    First,
    Last,
    PageUp,
    PageDown,
    Parent,
    Child,
    Open,
    Rename,
    Menu,
    Editor,
}

#[derive(Debug, PartialEq, Eq)]
enum Effect {
    None,
    Open(PathBuf),
    Rename(PathBuf, bool),
    Menu(PathBuf),
    Editor,
}

fn focus_id(root: &Path) -> egui::Id {
    egui::Id::new(("explorer_keyboard_focus", root))
}

fn lock_explorer_focus(ctx: &egui::Context, id: egui::Id) {
    ctx.memory_mut(|memory| {
        memory.set_focus_lock_filter(
            id,
            egui::EventFilter {
                horizontal_arrows: true,
                vertical_arrows: true,
                escape: true,
                tab: false,
            },
        );
    });
}

fn visible_rows<'a>(
    index: &'a workspace::WorkspaceIndex,
    state: &ProjectNavigation,
) -> Vec<VisibleRow<'a>> {
    fn collect<'a>(
        index: &'a workspace::WorkspaceIndex,
        state: &ProjectNavigation,
        parent: &Path,
        depth: usize,
        rows: &mut Vec<VisibleRow<'a>>,
    ) {
        if depth > 32 {
            return;
        }
        for entry in index.children(parent) {
            if entry.depth > 32 {
                continue;
            }
            rows.push(VisibleRow { entry, depth });
            if entry.is_dir && state.expanded.contains(&entry.path) {
                collect(index, state, &entry.path, depth + 1, rows);
            }
        }
    }
    let mut rows = Vec::new();
    collect(index, state, &index.root, 0, &mut rows);
    rows
}

impl ProjectNavigation {
    fn initialize(&mut self, index: &workspace::WorkspaceIndex, active: Option<&Path>) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        if let Some(active) = active.filter(|path| path.starts_with(&index.root)) {
            for ancestor in active.ancestors().skip(1) {
                if ancestor == index.root {
                    break;
                }
                self.expanded.insert(ancestor.into());
            }
            self.selected = Some(active.into());
        }
        self.reconcile(index);
    }

    /// A refresh or collapse can remove the selected row. Keep its nearest
    /// visible parent before falling back to the first row.
    fn reconcile(&mut self, index: &workspace::WorkspaceIndex) {
        let identity = index as *const workspace::WorkspaceIndex as usize;
        if self.awaiting_renamed_index && self.index_identity == identity {
            // The old immutable index is deliberately kept visible while its
            // replacement is built. It cannot invalidate a successful rename.
            return;
        }
        self.awaiting_renamed_index = false;
        self.index_identity = identity;
        let rows = visible_rows(index, self);
        if self
            .selected
            .as_ref()
            .is_some_and(|selected| rows.iter().any(|row| row.entry.path == *selected))
        {
            return;
        }
        self.selected = self
            .selected
            .as_deref()
            .and_then(|selected| {
                selected
                    .ancestors()
                    .skip(1)
                    .find(|parent| rows.iter().any(|row| row.entry.path == **parent))
            })
            .map(Path::to_path_buf)
            .or_else(|| rows.first().map(|row| row.entry.path.clone()));
    }

    fn apply(&mut self, index: &workspace::WorkspaceIndex, movement: Move) -> Effect {
        self.reconcile(index);
        if movement == Move::Editor {
            return Effect::Editor;
        }
        let rows = visible_rows(index, self);
        let Some(position) = self
            .selected
            .as_ref()
            .and_then(|selected| rows.iter().position(|row| row.entry.path == *selected))
        else {
            return Effect::None;
        };
        let row = &rows[position];
        let path = row.entry.path.clone();
        let page = self.page_rows.max(1);
        let last = rows.len() - 1;
        let target = match movement {
            Move::Up => Some(position.saturating_sub(1)),
            Move::Down => Some((position + 1).min(last)),
            Move::First => Some(0),
            Move::Last => Some(last),
            Move::PageUp => Some(position.saturating_sub(page)),
            Move::PageDown => Some(position.saturating_add(page).min(last)),
            Move::Parent => {
                if row.entry.is_dir && self.expanded.remove(&path) {
                    self.scroll_to_selection = true;
                    return Effect::None;
                }
                path.parent()
                    .and_then(|parent| rows.iter().position(|row| row.entry.path == parent))
            }
            Move::Child => {
                if row.entry.is_dir {
                    if self.expanded.insert(path.clone()) {
                        self.scroll_to_selection = true;
                        return Effect::None;
                    }
                    rows.get(position + 1)
                        .filter(|child| child.entry.path.parent() == Some(path.as_path()))
                        .map(|_| position + 1)
                } else {
                    None
                }
            }
            Move::Open => {
                if row.entry.is_dir {
                    if !self.expanded.remove(&path) {
                        self.expanded.insert(path);
                    }
                    self.scroll_to_selection = true;
                    return Effect::None;
                }
                return Effect::Open(path);
            }
            Move::Rename => return Effect::Rename(path, row.entry.is_dir),
            Move::Menu => return Effect::Menu(path),
            Move::Editor => unreachable!(),
        };
        if let Some(target) = target {
            self.selected = Some(rows[target].entry.path.clone());
            self.scroll_to_selection = true;
        }
        Effect::None
    }
}

fn movement(key: Key, swap_jk: bool) -> Option<Move> {
    Some(match key {
        Key::ArrowUp => Move::Up,
        Key::ArrowDown => Move::Down,
        Key::K if swap_jk => Move::Down,
        Key::J if swap_jk => Move::Up,
        Key::K => Move::Up,
        Key::J => Move::Down,
        Key::Home => Move::First,
        Key::End => Move::Last,
        Key::PageUp => Move::PageUp,
        Key::PageDown => Move::PageDown,
        Key::ArrowLeft | Key::H => Move::Parent,
        Key::ArrowRight | Key::L => Move::Child,
        Key::Enter => Move::Open,
        Key::F2 => Move::Rename,
        Key::Escape => Move::Editor,
        _ => return None,
    })
}

/// The platform commonly delivers both Key::J and Text("j"). Process a
/// logical key once, while consuming all text/IME/clipboard input belonging to
/// Explorer focus, including trailing text after Enter moves focus to Source.
fn collect_moves(events: &mut Vec<egui::Event>, swap_jk: bool) -> Vec<Move> {
    let keyed_letters: HashSet<_> = events
        .iter()
        .filter_map(|event| match event {
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } if *modifiers == egui::Modifiers::NONE => Some(*key),
            _ => None,
        })
        .collect();
    let mut result = Vec::new();
    events.retain(|event| match event {
        egui::Event::Key {
            key: Key::F10,
            pressed,
            modifiers,
            ..
        } if *modifiers == egui::Modifiers::SHIFT => {
            if *pressed {
                result.push(Move::Menu);
            }
            false
        }
        egui::Event::Key {
            key,
            pressed,
            modifiers,
            ..
        } if *modifiers == egui::Modifiers::NONE && movement(*key, swap_jk).is_some() => {
            if *pressed {
                result.push(movement(*key, swap_jk).unwrap());
            }
            false
        }
        egui::Event::Text(text) => {
            for character in text.chars() {
                let key = match character {
                    'j' => Key::J,
                    'k' => Key::K,
                    'h' => Key::H,
                    'l' => Key::L,
                    _ => continue,
                };
                if !keyed_letters.contains(&key) {
                    result.push(movement(key, swap_jk).unwrap());
                }
            }
            false
        }
        egui::Event::Paste(_) | egui::Event::Ime(_) => false,
        _ => true,
    });
    result
}

impl MorrowApp {
    pub(super) fn explorer_keyboard_focused(&self) -> bool {
        self.sidebar_visible
            && self.sidebar == SidebarView::Explorer
            && self.project().is_some_and(|project| {
                self.ctx
                    .memory(|memory| memory.has_focus(focus_id(&project.root)))
                    || self.explorer_navigation.focus_requested.as_deref() == Some(&project.root)
            })
    }

    pub(super) fn focus_explorer(&mut self) {
        self.find.break_escape_sequence();
        let Some(root) = self.project().map(|project| project.root.clone()) else {
            return;
        };
        self.sidebar = SidebarView::Explorer;
        self.sidebar_visible = true;
        self.completions = None;
        self.cancel_vim_search(false);
        self.cancel_navigation();
        self.screen_jump = None;
        self.find_focus = false;
        if let Some(tab) = self.tab_mut() {
            tab.focus = false;
        }
        let active = self.tab().and_then(|tab| tab.path.clone());
        if let Some(index) = self.project().and_then(|project| project.index.clone()) {
            let state = self
                .explorer_navigation
                .projects
                .entry(root.clone())
                .or_default();
            state.initialize(&index, active.as_deref());
            state.scroll_to_selection = true;
        }
        self.ctx
            .memory_mut(|memory| memory.request_focus(focus_id(&root)));
        self.explorer_navigation.focus_requested = Some(root.clone());
        lock_explorer_focus(&self.ctx, focus_id(&root));
        self.ctx.request_repaint();
    }

    pub(super) fn focus_editor(&mut self) {
        self.explorer_navigation.focus_requested = None;
        self.find.break_escape_sequence();
        self.cancel_vim_search(false);
        self.cancel_navigation();
        self.screen_jump = None;
        self.completions = None;
        if let Some(focused) = self.ctx.memory(|memory| memory.focused()) {
            self.ctx
                .memory_mut(|memory| memory.surrender_focus(focused));
        }
        if let Some(tab) = self.tab_mut() {
            tab.focus = tab.agent_view == crate::cli::DocumentView::Source;
        }
        if let Some(tab) = self.tab()
            && tab.agent_view == crate::cli::DocumentView::Source
            && !tab.loading
            && tab.error.is_none()
        {
            let id = self.document_widget_id(tab.id);
            self.ctx.memory_mut(|memory| memory.request_focus(id));
        }
        self.ctx.request_repaint();
    }

    /// Run before editor/Vim input dispatch. Returns true while Explorer owns
    /// this frame's keyboard input, including a frame that opens an editor.
    pub(super) fn explorer_navigation_keys(&mut self) -> bool {
        if !self.explorer_keyboard_focused()
            || self.palette.is_some()
            || self.picker.is_some()
            || self.settings_open
            || self.agent_marks_modal_open()
            || self.save_as.is_some()
            || self.close_action.is_some()
            || self.diff.is_some()
            || self.git_detail_open()
            || self.navigation_modal_open()
            || self.workspace_picker_open()
            || self.context_file_dialog.is_some()
            || egui::Popup::is_any_open(&self.ctx)
        {
            return false;
        }
        if let Some(project) = self.project() {
            lock_explorer_focus(&self.ctx, focus_id(&project.root));
        }
        let moves = self
            .ctx
            .input_mut(|input| collect_moves(&mut input.events, self.settings.swap_jk));
        if !moves.is_empty() {
            // egui chooses spatial widget focus before application dispatch.
            // Cancel that choice only for keys our Explorer actually handles;
            // Tab remains available to the normal accessibility focus chain.
            self.ctx
                .memory_mut(|memory| memory.move_focus(egui::FocusDirection::None));
        }
        let Some((root, index)) = self.project().and_then(|project| {
            project
                .index
                .clone()
                .map(|index| (project.root.clone(), index))
        }) else {
            if moves.contains(&Move::Editor) {
                self.focus_editor();
            }
            return true;
        };
        let active = self.tab().and_then(|tab| tab.path.clone());
        self.explorer_navigation
            .projects
            .entry(root.clone())
            .or_default()
            .initialize(&index, active.as_deref());
        let changed = !moves.is_empty();
        for movement in moves {
            let effect = self
                .explorer_navigation
                .projects
                .get_mut(&root)
                .unwrap()
                .apply(&index, movement);
            match effect {
                Effect::None => {}
                Effect::Open(path) => {
                    self.open_file(path);
                    self.focus_editor();
                    break;
                }
                Effect::Rename(path, is_dir) => {
                    self.context_begin_rename(&path, is_dir);
                    break;
                }
                Effect::Menu(path) => {
                    self.explorer_navigation.keyboard_menu = Some((path, egui::Pos2::ZERO));
                    break;
                }
                Effect::Editor => {
                    self.focus_editor();
                    break;
                }
            }
        }
        if changed {
            self.ctx.request_repaint();
        }
        true
    }

    pub(super) fn explorer_initialize(&mut self, index: &workspace::WorkspaceIndex) {
        let active = self.tab().and_then(|tab| tab.path.as_deref());
        let active = active.map(Path::to_path_buf);
        let state = self
            .explorer_navigation
            .projects
            .entry(index.root.clone())
            .or_default();
        state.initialize(index, active.as_deref());
        state.reconcile(index);
    }

    pub(super) fn explorer_rows<'a>(
        &self,
        index: &'a workspace::WorkspaceIndex,
    ) -> Vec<VisibleRow<'a>> {
        self.explorer_navigation
            .projects
            .get(&index.root)
            .map(|state| visible_rows(index, state))
            .unwrap_or_default()
    }

    pub(super) fn explorer_take_scroll_row(
        &mut self,
        root: &Path,
        rows: &[VisibleRow<'_>],
    ) -> Option<usize> {
        let state = self.explorer_navigation.projects.get_mut(root)?;
        if !state.scroll_to_selection {
            return None;
        }
        let row = rows
            .iter()
            .position(|row| state.selected.as_deref() == Some(row.entry.path.as_path()))?;
        state.scroll_to_selection = false;
        Some(row)
    }

    pub(super) fn explorer_renamed(&mut self, old: &Path, new: &Path) {
        fn remap(path: &Path, old: &Path, new: &Path) -> PathBuf {
            match path.strip_prefix(old) {
                Ok(relative) if relative.as_os_str().is_empty() => new.into(),
                Ok(relative) => new.join(relative),
                Err(_) => path.into(),
            }
        }
        for state in self.explorer_navigation.projects.values_mut() {
            if let Some(selected) = state.selected.as_mut()
                && selected.starts_with(old)
            {
                *selected = remap(selected, old, new);
                state.scroll_to_selection = true;
                state.awaiting_renamed_index = true;
            }
            state.expanded = state
                .expanded
                .drain()
                .map(|path| remap(&path, old, new))
                .collect();
        }
        if let Some((path, _)) = &mut self.explorer_navigation.keyboard_menu {
            *path = remap(path, old, new);
        }
    }

    pub(super) fn explorer_register_focus(&mut self, ui: &mut egui::Ui, root: &Path) {
        let id = focus_id(root);
        // This non-interactive focus target never intercepts rows, blank-space
        // context menus, or the scroll bar.
        ui.interact(
            egui::Rect::from_min_size(ui.next_widget_position(), Vec2::ZERO),
            id,
            egui::Sense::focusable_noninteractive(),
        );
        if self.explorer_navigation.focus_requested.as_deref() == Some(root)
            && self.context_file_dialog.is_none()
            && ui.is_enabled()
            && ui.memory(|memory| memory.allows_interaction(ui.layer_id()))
        {
            ui.memory_mut(|memory| memory.request_focus(id));
            self.explorer_navigation.focus_requested = None;
        }
        if self.explorer_keyboard_focused() {
            lock_explorer_focus(ui.ctx(), id);
        }
        let page_rows = (ui.available_height() / ROW_HEIGHT).floor().max(1.0) as usize;
        self.explorer_navigation
            .projects
            .entry(root.into())
            .or_default()
            .page_rows = page_rows;
    }

    pub(super) fn explorer_selected(&self, path: &Path) -> bool {
        self.project()
            .and_then(|project| self.explorer_navigation.projects.get(&project.root))
            .is_some_and(|state| state.selected.as_deref() == Some(path))
    }

    pub(super) fn explorer_expanded(&self, path: &Path) -> bool {
        self.project()
            .and_then(|project| self.explorer_navigation.projects.get(&project.root))
            .is_some_and(|state| state.expanded.contains(path))
    }

    pub(super) fn explorer_row_interaction(
        &mut self,
        response: &egui::Response,
        path: &Path,
        is_dir: bool,
    ) {
        let Some(root) = self.project().map(|project| project.root.clone()) else {
            return;
        };
        if response.clicked() || response.secondary_clicked() {
            let state = self
                .explorer_navigation
                .projects
                .entry(root.clone())
                .or_default();
            state.selected = Some(path.into());
            if response.clicked() && is_dir && !state.expanded.remove(path) {
                state.expanded.insert(path.into());
            }
            if let Some(tab) = self.tab_mut() {
                tab.focus = false;
            }
            self.ctx
                .memory_mut(|memory| memory.request_focus(focus_id(&root)));
            lock_explorer_focus(&self.ctx, focus_id(&root));
            self.ctx.request_repaint();
        }
        let selected = self.explorer_selected(path);
        if selected
            && self.explorer_keyboard_focused()
            && response.rect.intersects(response.ctx.content_rect())
        {
            response
                .ctx
                .layer_painter(response.layer_id)
                .with_clip_rect(response.interact_rect)
                .rect_stroke(
                    response.rect.shrink(1.0),
                    4.0,
                    Stroke::new(1.0, ACCENT.gamma_multiply(0.7)),
                    egui::StrokeKind::Inside,
                );
        }
        if let Some(state) = self.explorer_navigation.projects.get_mut(&root)
            && selected
            && state.scroll_to_selection
        {
            response.scroll_to_me(None);
            state.scroll_to_selection = false;
        }
        if let Some((selected, position)) = self.explorer_navigation.keyboard_menu.as_mut()
            && *selected == path
            && *position == egui::Pos2::ZERO
        {
            *position = response.rect.left_bottom();
            egui::Popup::open_id(&self.ctx, self.explorer_menu_id(path));
        }
    }

    /// The same context menu contents serve pointer and keyboard invocation.
    pub(super) fn explorer_menu_position(
        &mut self,
        response: &egui::Response,
        path: &Path,
    ) -> Option<egui::Pos2> {
        let id = self.explorer_menu_id(path);
        if response.secondary_clicked() {
            self.explorer_navigation.keyboard_menu = None;
        }
        if let Some((selected, position)) = &self.explorer_navigation.keyboard_menu {
            if selected == path && egui::Popup::is_id_open(&self.ctx, id) {
                return Some(*position);
            }
            if selected == path {
                self.explorer_navigation.keyboard_menu = None;
                if self.context_file_dialog.is_none() {
                    self.focus_explorer();
                }
            }
        }
        None
    }

    pub(super) fn explorer_menu_id(&self, path: &Path) -> egui::Id {
        egui::Id::new(("explorer_item_context_menu", path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};

    struct Fixture(PathBuf);

    impl Fixture {
        fn new(extra_files: usize) -> (Self, MorrowApp) {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "morrow-explorer-navigation-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            fs::create_dir(root.join("src")).unwrap();
            fs::create_dir(root.join("src/nested")).unwrap();
            fs::write(root.join("src/lib.rs"), "saved rust\n").unwrap();
            fs::write(root.join("src/nested/deep.rs"), "nested rust\n").unwrap();
            fs::write(root.join(".gitignore"), "ignored\n").unwrap();
            fs::write(root.join("plan ü.md"), "# Plan\n").unwrap();
            for number in 0..extra_files {
                fs::write(root.join(format!("row{number:03}.txt")), "row\n").unwrap();
            }
            let root = fs::canonicalize(root).unwrap();
            let mut app = MorrowApp::empty(egui::Context::default());
            app.settings.autosave = false;
            let mut project = Project::new(root.clone());
            project.index = Some(Arc::new(
                workspace::build_index(&root, Default::default()).unwrap(),
            ));
            let mut tab = EditorTab::new(Some(root.join("plan ü.md")), "plan ü.md".into());
            tab.text = "# Plan\n".into();
            tab.saved_text = tab.text.clone();
            project.tabs.push(tab);
            app.projects.push(project);
            app.bind_current_pane();
            (Self(root), app)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            for name in [
                "src/lib.rs",
                "src/nested/deep.rs",
                ".gitignore",
                "plan ü.md",
                "renamed.md",
                "zzzz-plan.md",
            ] {
                let _ = fs::remove_file(self.0.join(name));
            }
            // Only our own generated regular files are removed.
            if let Ok(entries) = fs::read_dir(&self.0) {
                for entry in entries.flatten() {
                    if entry.file_name().to_string_lossy().starts_with("row") {
                        let _ = fs::remove_file(entry.path());
                    }
                }
            }
            for name in ["src/nested", "src"] {
                let _ = fs::remove_dir(self.0.join(name));
            }
            let _ = fs::remove_dir(&self.0);
        }
    }

    fn harness(app: MorrowApp) -> Harness<'static, MorrowApp> {
        let mut screen = Harness::builder()
            .with_size([440.0, 320.0])
            .with_os(egui::os::OperatingSystem::Windows)
            .build_ui_state(
                |ui, app: &mut MorrowApp| {
                    app.ctx = ui.ctx().clone();
                    app.poll();
                    app.explorer_navigation_keys();
                    app.explorer(ui);
                    let ctx = ui.ctx().clone();
                    app.context_file_dialog_ui(&ctx);
                },
                app,
            );
        screen.ctx.all_styles_mut(|style| {
            style.animation_time = 0.0;
            style.scroll_animation = egui::style::ScrollAnimation::none();
        });
        screen.run_steps(3);
        screen
    }

    fn key(screen: &mut Harness<'_, MorrowApp>, key: Key, modifiers: egui::Modifiers) {
        screen.input_mut().events.extend([
            egui::Event::ModifiersChanged(modifiers),
            egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            },
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

    fn selected(app: &MorrowApp) -> Option<PathBuf> {
        let root = &app.project()?.root;
        app.explorer_navigation.projects.get(root)?.selected.clone()
    }

    #[test]
    fn arrows_expand_collapse_and_parent_follow_visible_rows() {
        let (fixture, app) = Fixture::new(0);
        let mut screen = harness(app);
        screen.state_mut().focus_explorer();
        key(&mut screen, Key::Home, egui::Modifiers::NONE);
        assert_eq!(selected(screen.state()), Some(fixture.0.join("src")));
        key(&mut screen, Key::ArrowRight, egui::Modifiers::NONE);
        assert!(screen.query_by_label("lib.rs").is_some());
        assert_eq!(selected(screen.state()), Some(fixture.0.join("src")));
        key(&mut screen, Key::ArrowRight, egui::Modifiers::NONE);
        assert_eq!(selected(screen.state()), Some(fixture.0.join("src/nested")));
        key(&mut screen, Key::ArrowRight, egui::Modifiers::NONE);
        key(&mut screen, Key::ArrowDown, egui::Modifiers::NONE);
        assert_eq!(
            selected(screen.state()),
            Some(fixture.0.join("src/nested/deep.rs"))
        );
        key(&mut screen, Key::ArrowLeft, egui::Modifiers::NONE);
        assert_eq!(selected(screen.state()), Some(fixture.0.join("src/nested")));
        key(&mut screen, Key::ArrowLeft, egui::Modifiers::NONE);
        assert!(screen.query_by_label("deep.rs").is_none());
        key(&mut screen, Key::ArrowLeft, egui::Modifiers::NONE);
        assert_eq!(selected(screen.state()), Some(fixture.0.join("src")));
        key(&mut screen, Key::Enter, egui::Modifiers::NONE);
        assert!(screen.query_by_label("nested").is_none());
        assert!(screen.state().explorer_keyboard_focused());
    }

    #[test]
    fn vim_motions_respect_swap_and_do_not_leak_text_to_document() {
        let (_fixture, app) = Fixture::new(0);
        let mut screen = harness(app);
        screen.state_mut().focus_explorer();
        key(&mut screen, Key::Home, egui::Modifiers::NONE);
        let first = selected(screen.state());
        screen.input_mut().events.extend([
            egui::Event::Key {
                key: Key::J,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::Text("j".into()),
        ]);
        screen.run_steps(2);
        let second = selected(screen.state());
        assert_ne!(second, first);
        assert_eq!(screen.state().tab().unwrap().text, "# Plan\n");
        screen.state_mut().settings.swap_jk = true;
        key(&mut screen, Key::J, egui::Modifiers::NONE);
        assert_eq!(selected(screen.state()), first);
        key(&mut screen, Key::K, egui::Modifiers::NONE);
        assert_eq!(selected(screen.state()), second);
        screen.event(egui::Event::Text("arbitrary input".into()));
        screen.event(egui::Event::Paste("clipboard text".into()));
        screen.run_steps(2);
        assert_eq!(screen.state().tab().unwrap().text, "# Plan\n");
    }

    #[test]
    fn end_home_and_pages_scroll_virtualized_selection_into_view() {
        let (fixture, app) = Fixture::new(80);
        let mut screen = harness(app);
        screen.state_mut().focus_explorer();
        key(&mut screen, Key::End, egui::Modifiers::NONE);
        assert_eq!(selected(screen.state()), Some(fixture.0.join("row079.txt")));
        assert!(
            screen
                .get_by_label("row079.txt")
                .rect()
                .intersects(screen.ctx.content_rect())
        );
        assert!(screen.query_by_label("src").is_none());
        key(&mut screen, Key::PageUp, egui::Modifiers::NONE);
        assert_ne!(selected(screen.state()), Some(fixture.0.join("row079.txt")));
        key(&mut screen, Key::PageDown, egui::Modifiers::NONE);
        assert_eq!(selected(screen.state()), Some(fixture.0.join("row079.txt")));
        key(&mut screen, Key::Home, egui::Modifiers::NONE);
        assert_eq!(selected(screen.state()), Some(fixture.0.join("src")));
        assert!(screen.query_by_label("src").is_some());
    }

    #[test]
    fn mouse_selection_and_keyboard_selection_share_one_state() {
        let (fixture, app) = Fixture::new(0);
        let mut screen = harness(app);
        screen.get_by_label("src").click();
        screen.run_steps(2);
        assert!(screen.state().explorer_keyboard_focused());
        assert_eq!(selected(screen.state()), Some(fixture.0.join("src")));
        key(&mut screen, Key::L, egui::Modifiers::NONE);
        assert_eq!(selected(screen.state()), Some(fixture.0.join("src/nested")));
        screen.get_by_label("plan ü.md").click();
        screen.run_steps(2);
        assert!(screen.state().explorer_keyboard_focused());
        assert_eq!(selected(screen.state()), Some(fixture.0.join("plan ü.md")));
        key(&mut screen, Key::K, egui::Modifiers::NONE);
        assert_eq!(selected(screen.state()), Some(fixture.0.join(".gitignore")));
        assert_eq!(
            screen.state().tab().unwrap().path,
            Some(fixture.0.join("plan ü.md"))
        );
    }

    #[test]
    fn f2_selects_unicode_stem_and_escape_restores_explorer_focus() {
        let (_fixture, app) = Fixture::new(0);
        let mut screen = harness(app);
        screen.state_mut().focus_explorer();
        screen.run_steps(1);
        key(&mut screen, Key::F2, egui::Modifiers::NONE);
        let state =
            egui::TextEdit::load_state(&screen.ctx, egui::Id::new("file_action_name")).unwrap();
        let selection = state.cursor.char_range().unwrap().as_sorted_char_range();
        assert_eq!(selection.start.0..selection.end.0, 0..6);
        screen.event(egui::Event::Text("renamed".into()));
        screen.run_steps(1);
        // Observe the widget's buffer through its accessibility value; the
        // displayed field must keep the original .md extension.
        assert_eq!(
            screen.get_by_role(egui::accesskit::Role::TextInput).value(),
            Some("renamed.md".into())
        );
        key(&mut screen, Key::Escape, egui::Modifiers::NONE);
        assert!(screen.state().context_file_dialog.is_none());
        assert!(screen.state().explorer_keyboard_focused());
        assert_eq!(screen.state().tab().unwrap().text, "# Plan\n");
    }

    #[test]
    fn f2_folder_selects_full_name_and_dotfile_selects_full_name() {
        let (_fixture, app) = Fixture::new(0);
        let mut screen = harness(app);
        screen.state_mut().focus_explorer();
        key(&mut screen, Key::Home, egui::Modifiers::NONE);
        key(&mut screen, Key::F2, egui::Modifiers::NONE);
        let state =
            egui::TextEdit::load_state(&screen.ctx, egui::Id::new("file_action_name")).unwrap();
        let selection = state.cursor.char_range().unwrap().as_sorted_char_range();
        assert_eq!(selection.start.0..selection.end.0, 0..3);
        key(&mut screen, Key::Escape, egui::Modifiers::NONE);
        key(&mut screen, Key::ArrowDown, egui::Modifiers::NONE);
        key(&mut screen, Key::F2, egui::Modifiers::NONE);
        let state =
            egui::TextEdit::load_state(&screen.ctx, egui::Id::new("file_action_name")).unwrap();
        let selection = state.cursor.char_range().unwrap().as_sorted_char_range();
        assert_eq!(selection.start.0..selection.end.0, 0..10);
    }

    #[test]
    fn shift_f10_opens_selected_row_menu_without_mouse_pointer() {
        let (_fixture, app) = Fixture::new(0);
        let mut screen = harness(app);
        screen.state_mut().focus_explorer();
        key(&mut screen, Key::F10, egui::Modifiers::SHIFT);
        assert!(screen.query_by_label("Rename…").is_some());
        assert!(screen.query_by_label("Open").is_some());
        assert!(screen.query_by_label("Copy relative path").is_some());
        screen.get_by_label("Rename…").click();
        screen.run_steps(2);
        assert!(screen.state().context_file_dialog.is_some());
        key(&mut screen, Key::Escape, egui::Modifiers::NONE);
        assert!(screen.state().explorer_keyboard_focused());
    }

    #[test]
    fn per_project_selection_is_retained_and_text_fields_are_not_intercepted() {
        let (_first, mut app) = Fixture::new(0);
        let (_second, other) = Fixture::new(0);
        app.projects
            .push(other.projects.into_iter().next().unwrap());
        let mut screen = harness(app);
        screen.state_mut().focus_explorer();
        key(&mut screen, Key::Home, egui::Modifiers::NONE);
        let first = selected(screen.state());
        screen.state_mut().active_project = 1;
        screen.state_mut().focus_explorer();
        key(&mut screen, Key::End, egui::Modifiers::NONE);
        screen.state_mut().active_project = 0;
        screen.state_mut().focus_explorer();
        screen.run_steps(2);
        assert_eq!(selected(screen.state()), first);
        screen.state_mut().settings_open = true;
        screen.event(egui::Event::Text("j".into()));
        screen.run_steps(2);
        assert_eq!(selected(screen.state()), first);
    }

    #[test]
    fn reopening_selected_preview_moves_focus_out_of_explorer_without_editing() {
        let (_fixture, mut app) = Fixture::new(0);
        app.tab_mut().unwrap().agent_view = crate::cli::DocumentView::Preview;
        let mut screen = harness(app);
        screen.state_mut().focus_explorer();
        key(&mut screen, Key::Enter, egui::Modifiers::NONE);
        assert!(!screen.state().explorer_keyboard_focused());
        assert!(!screen.state().tab().unwrap().focus);
        assert_eq!(
            screen.state().tab().unwrap().agent_view,
            crate::cli::DocumentView::Preview
        );
        assert_eq!(screen.state().tab().unwrap().text, "# Plan\n");
    }

    #[test]
    fn collapse_and_refresh_reconcile_selection_to_visible_parent() {
        let (_fixture, app) = Fixture::new(0);
        let index = app.project().unwrap().index.as_ref().unwrap();
        let root = &index.root;
        let mut state = ProjectNavigation::default();
        state.initialize(index, Some(&root.join("src/nested/deep.rs")));
        state.expanded.remove(&root.join("src/nested"));
        state.reconcile(index);
        assert_eq!(state.selected, Some(root.join("src/nested")));
        state.expanded.remove(&root.join("src"));
        state.reconcile(index);
        assert_eq!(state.selected, Some(root.join("src")));
    }

    #[test]
    fn successful_rename_preserves_selected_descendant_and_expanded_folders() {
        let (_fixture, mut app) = Fixture::new(0);
        let root = app.project().unwrap().root.clone();
        let index = app.project().unwrap().index.clone().unwrap();
        let old = root.join("src");
        let new = root.join("source");
        app.explorer_navigation
            .projects
            .entry(root.clone())
            .or_default()
            .initialize(&index, Some(&old.join("nested/deep.rs")));
        app.explorer_renamed(&old, &new);
        app.explorer_initialize(&index);
        let state = app.explorer_navigation.projects.get(&root).unwrap();
        assert_eq!(state.selected, Some(new.join("nested/deep.rs")));
        assert!(state.expanded.contains(&new));
        assert!(state.expanded.contains(&new.join("nested")));
        assert!(!state.expanded.contains(&old));
        assert!(state.scroll_to_selection);
        app.explorer_renamed(&new.join("nested/deep.rs"), &new.join("nested/renamed.rs"));
        assert_eq!(selected(&app), Some(new.join("nested/renamed.rs")));
    }

    #[test]
    fn f2_rename_completion_and_refreshed_index_keep_new_row_selected() {
        let (fixture, app) = Fixture::new(0);
        let mut screen = harness(app);
        screen.state_mut().focus_explorer();
        key(&mut screen, Key::F2, egui::Modifiers::NONE);
        screen.event(egui::Event::Text("renamed".into()));
        screen.run_steps(1);
        key(&mut screen, Key::Enter, egui::Modifiers::NONE);
        let new = fixture.0.join("renamed.md");
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            screen.run_steps(1);
            let ready = screen
                .state()
                .project()
                .unwrap()
                .index
                .as_ref()
                .unwrap()
                .entries
                .iter()
                .any(|entry| entry.path == new);
            if ready && screen.state().context_file_dialog.is_none() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "background rename did not complete"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(selected(screen.state()), Some(new.clone()));
        assert_eq!(screen.state().tab().unwrap().path, Some(new));
        assert!(screen.state().explorer_keyboard_focused());
        screen.run_steps(3);
        assert!(screen.query_by_label("renamed.md").is_some());
        assert_eq!(
            fs::read_to_string(fixture.0.join("renamed.md")).unwrap(),
            "# Plan\n"
        );
    }

    #[test]
    fn rename_scroll_request_survives_old_index_until_new_sorted_row_exists() {
        let (fixture, mut app) = Fixture::new(80);
        let root = fixture.0.clone();
        let old_index = app.project().unwrap().index.clone().unwrap();
        app.explorer_initialize(&old_index);
        let old = root.join("plan ü.md");
        let new = root.join("zzzz-plan.md");
        fs::rename(&old, &new).unwrap();
        app.explorer_renamed(&old, &new);
        app.explorer_initialize(&old_index);
        let old_rows = app.explorer_rows(&old_index);
        assert_eq!(app.explorer_take_scroll_row(&root, &old_rows), None);
        assert!(
            app.explorer_navigation
                .projects
                .get(&root)
                .unwrap()
                .scroll_to_selection
        );
        let fresh_index = workspace::build_index(&root, Default::default()).unwrap();
        app.explorer_initialize(&fresh_index);
        let fresh_rows = app.explorer_rows(&fresh_index);
        let row = app.explorer_take_scroll_row(&root, &fresh_rows).unwrap();
        assert_eq!(row, fresh_rows.len() - 1);
        assert!(
            row > 50,
            "renamed row should be outside the previous viewport"
        );
        assert_eq!(fresh_rows[row].entry.path, new);
        assert!(
            !app.explorer_navigation
                .projects
                .get(&root)
                .unwrap()
                .scroll_to_selection
        );
        // Restore the fixture's known original name for its ordinary cleanup.
        fs::rename(&new, &old).unwrap();
    }

    #[test]
    fn focused_explorer_settles_without_an_idle_repaint_loop() {
        let (_fixture, app) = Fixture::new(0);
        let mut screen = harness(app);
        screen.state_mut().focus_explorer();
        screen.run_steps(4);
        assert!(screen.state().explorer_keyboard_focused());
        let settled = screen
            .try_run()
            .expect("idle Explorer continuously requested repaint");
        assert!(
            settled <= 4,
            "Explorer should settle promptly after focus is restored"
        );
        assert!(screen.state().explorer_keyboard_focused());
    }
}

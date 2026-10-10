//! Git navigation is driven entirely by cached worker results.
//!
//! Working files and historical files deliberately have distinct actions: a
//! commit row always opens the patch belonging to that commit, never today's
//! file contents. Tree state stays local to the project and survives refreshes.

use super::*;
use std::collections::{BTreeMap, VecDeque};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum GitSection {
    #[default]
    Changes,
    History,
}

#[derive(Default)]
pub(super) struct GitViewState {
    pub graph: crate::git_graph::Graph,
    // Active keyboard region; both regions are always rendered.
    pub(super) section: GitSection,
    pub(super) split_ratio: Option<f32>,
    changes_rect: Option<egui::Rect>,
    history_rect: Option<egui::Rect>,
    pub(super) selected_commit: Option<String>,
    pub(super) detail: Option<git::CommitDetails>,
    pub(super) detail_loading: bool,
    pub(super) detail_error: Option<String>,
    pub(super) detail_open: bool,
    pub(super) detail_cache: HashMap<String, git::CommitDetails>,
    pub(super) hover_stats: HashMap<(PathBuf, String), Result<git::CommitStats, String>>,
    pub(super) hover_stats_pending: HashMap<(PathBuf, String), u64>,
    pub(super) hover_stats_order: VecDeque<(PathBuf, String)>,
    pub(super) expanded: HashMap<String, bool>,
    pub(super) selected_file: Option<String>,
    working_cache: Option<WorkingTreeCache>,
    detail_tree: Option<(String, FileTree)>,
    keyboard_rows: Vec<egui::Id>,
    keyboard_last_focus: Option<egui::Id>,
    keyboard_header: Option<egui::Id>,
    keyboard_focus_pending: bool,
    keyboard_page_rows: usize,
    keyboard_controls: Vec<egui::Id>,
    detail_keyboard_focus: Option<egui::Id>,
    detail_keyboard_focus_pending: bool,
    detail_page_rows: usize,
}

fn git_row_id(root: &Path, key: &str) -> egui::Id {
    egui::Id::new(("git-content", root, key))
}

fn tree_keyboard_rows(
    tree: &FileTree,
    prefix: &str,
    namespace: &str,
    root: &Path,
    expanded: &HashMap<String, bool>,
    rows: &mut Vec<egui::Id>,
) {
    for (name, folder) in &tree.folders {
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        let key = format!("{namespace}:folder:{path}");
        rows.push(git_row_id(root, &key));
        if expanded.get(&key).copied().unwrap_or(true) {
            tree_keyboard_rows(folder, &path, namespace, root, expanded, rows);
        }
    }
    rows.extend(
        tree.files
            .iter()
            .map(|file| git_row_id(root, &format!("{namespace}:file:{}", file.path))),
    );
}

fn git_keyboard_target(rows: &[egui::Id], focused: egui::Id, key: Key) -> Option<egui::Id> {
    let index = rows
        .iter()
        .position(|id| *id == focused || id.with("action") == focused)?;
    let index = match key {
        Key::ArrowUp => index.saturating_sub(1),
        Key::ArrowDown => (index + 1).min(rows.len().saturating_sub(1)),
        Key::Home => 0,
        Key::End => rows.len().saturating_sub(1),
        _ => return None,
    };
    rows.get(index).copied()
}

#[derive(Clone, Copy)]
enum GitMovement {
    Up(usize),
    Down(usize),
    First,
    Last,
}

fn git_movement(
    key: Key,
    modifiers: egui::Modifiers,
    page: usize,
    swap_jk: bool,
) -> Option<GitMovement> {
    let page = page.max(1);
    if modifiers == egui::Modifiers::CTRL {
        return match key {
            Key::U => Some(GitMovement::Up((page / 2).max(1))),
            Key::D => Some(GitMovement::Down((page / 2).max(1))),
            _ => None,
        };
    }
    if modifiers != egui::Modifiers::NONE {
        return None;
    }
    match key {
        Key::ArrowUp => Some(GitMovement::Up(1)),
        Key::ArrowDown => Some(GitMovement::Down(1)),
        Key::J if swap_jk => Some(GitMovement::Up(1)),
        Key::K if swap_jk => Some(GitMovement::Down(1)),
        Key::K => Some(GitMovement::Up(1)),
        Key::J => Some(GitMovement::Down(1)),
        Key::PageUp => Some(GitMovement::Up(page)),
        Key::PageDown => Some(GitMovement::Down(page)),
        Key::Home => Some(GitMovement::First),
        Key::End => Some(GitMovement::Last),
        _ => None,
    }
}

/// Consume navigation in event order, including repeated keys within one frame.
/// Only rows own these motions; composer and popup text input stays untouched.
fn take_git_navigation(
    ctx: &egui::Context,
    rows: &[egui::Id],
    focused: egui::Id,
    page: usize,
    swap_jk: bool,
) -> Option<egui::Id> {
    let mut index = rows
        .iter()
        .position(|id| *id == focused || id.with("action") == focused)?;
    let mut moved = false;
    ctx.input_mut(|input| {
        let keyed_letters: Vec<_> = input
            .events
            .iter()
            .filter_map(|event| match event {
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } if matches!(key, Key::J | Key::K) && *modifiers == egui::Modifiers::NONE => {
                    Some(*key)
                }
                _ => None,
            })
            .collect();
        let mut apply = |movement| {
            index = match movement {
                GitMovement::Up(count) => index.saturating_sub(count),
                GitMovement::Down(count) => index.saturating_add(count).min(rows.len() - 1),
                GitMovement::First => 0,
                GitMovement::Last => rows.len() - 1,
            };
            moved = true;
        };
        input.events.retain(|event| match event {
            egui::Event::Key {
                key,
                pressed,
                modifiers,
                ..
            } => {
                if let Some(movement) = git_movement(*key, *modifiers, page, swap_jk) {
                    if *pressed {
                        apply(movement);
                    }
                    false
                } else {
                    true
                }
            }
            egui::Event::Text(text)
                if text.chars().all(|character| matches!(character, 'j' | 'k')) =>
            {
                for character in text.chars() {
                    let key = if character == 'j' { Key::J } else { Key::K };
                    if !keyed_letters.contains(&key) {
                        apply(git_movement(key, egui::Modifiers::NONE, page, swap_jk).unwrap());
                    }
                }
                false
            }
            _ => true,
        });
    });
    if moved {
        ctx.memory_mut(|memory| memory.move_focus(egui::FocusDirection::None));
        Some(rows[index])
    } else {
        None
    }
}

fn request_git_row_focus(ctx: &egui::Context, id: egui::Id) {
    ctx.memory_mut(|memory| memory.request_focus(id));
    ctx.data_mut(|data| data.insert_temp(id.with("reveal"), true));
}

fn git_row_keyboard(ui: &egui::Ui, response: &egui::Response) {
    if response.has_focus() {
        ui.memory_mut(|memory| {
            memory.set_focus_lock_filter(
                response.id,
                egui::EventFilter {
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    ..Default::default()
                },
            )
        });
        if response.gained_focus()
            || ui
                .data_mut(|data| data.remove_temp::<bool>(response.id.with("reveal")))
                .unwrap_or(false)
        {
            response.scroll_to_me(Some(Align::Center));
        }
        ui.painter().rect_stroke(
            response.rect.shrink(1.0),
            3.0,
            Stroke::new(1.0, ACCENT),
            egui::StrokeKind::Inside,
        );
    }
}

fn git_folder_key(ui: &egui::Ui, row: &egui::Response, open: &mut bool) {
    if row.has_focus() && !egui::Popup::is_any_open(ui.ctx()) {
        ui.input_mut(|input| {
            input.events.retain(|event| match event {
                egui::Event::Key {
                    key,
                    pressed,
                    modifiers,
                    ..
                } if *modifiers == egui::Modifiers::NONE
                    && matches!(key, Key::ArrowLeft | Key::ArrowRight | Key::H | Key::L) =>
                {
                    if *pressed {
                        *open = matches!(key, Key::ArrowRight | Key::L);
                    }
                    false
                }
                egui::Event::Text(text)
                    if text.chars().all(|character| matches!(character, 'h' | 'l')) =>
                {
                    for character in text.chars() {
                        *open = character == 'l';
                    }
                    false
                }
                _ => true,
            });
        });
    }
}

fn git_popup_focus(response: &egui::Response, popup_id: egui::Id) {
    let ctx = &response.ctx;
    let remembered = popup_id.with("git-was-open");
    if egui::Popup::is_id_open(ctx, popup_id) {
        ctx.data_mut(|data| data.insert_temp(remembered, true));
    } else if ctx
        .data_mut(|data| data.remove_temp::<bool>(remembered))
        .unwrap_or(false)
    {
        let escape = ctx.input(|input| input.key_pressed(Key::Escape));
        if escape {
            ctx.data_mut(|data| data.insert_temp(egui::Id::new("git-popup-return"), response.id));
        }
        if escape
            || ctx.input(|input| input.key_pressed(Key::Enter) || input.key_pressed(Key::Space))
        {
            request_git_row_focus(ctx, response.id);
        }
    }
}

fn take_git_popup_return(ctx: &egui::Context) -> Option<egui::Id> {
    ctx.data_mut(|data| {
        let key = egui::Id::new("git-popup-return");
        let id = data.get_temp::<egui::Id>(key);
        data.remove::<egui::Id>(key);
        id
    })
}

fn git_context_menu(response: &egui::Response, contents: impl FnOnce(&mut egui::Ui, bool)) {
    let keyboard = response.has_focus()
        && response
            .ctx
            .input_mut(|input| input.consume_key(egui::Modifiers::SHIFT, Key::F10));
    let id = response.id.with("git-context-menu");
    if keyboard {
        egui::Popup::open_id(&response.ctx, id);
        response.ctx.data_mut(|data| {
            data.insert_temp(id.with("keyboard-position"), response.rect.left_bottom())
        });
        response
            .ctx
            .data_mut(|data| data.insert_temp(id.with("focus-first"), true));
    } else if response.secondary_clicked() {
        response
            .ctx
            .data_mut(|data| data.remove::<egui::Pos2>(id.with("keyboard-position")));
    }
    let mut popup = egui::Popup::context_menu(response).id(id);
    if let Some(position) = response
        .ctx
        .data(|data| data.get_temp::<egui::Pos2>(id.with("keyboard-position")))
    {
        popup = popup.at_position(position);
    }
    popup.show(|ui| {
        let items_id = id.with("git-menu-items");
        let items = ui
            .data(|data| data.get_temp::<Vec<egui::Id>>(items_id))
            .unwrap_or_default();
        if let Some(focused) = ui.memory(|memory| memory.focused()) {
            for key in [Key::ArrowUp, Key::ArrowDown, Key::Home, Key::End] {
                if let Some(target) = git_keyboard_target(&items, focused, key)
                    && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, key))
                {
                    ui.memory_mut(|memory| memory.request_focus(target));
                    break;
                }
            }
        }
        ui.data_mut(|data| {
            data.insert_temp(egui::Id::new("git-active-menu"), items_id);
            data.insert_temp(items_id, Vec::<egui::Id>::new());
        });
        let focus = !ui.is_sizing_pass()
            && ui
                .data(|data| data.get_temp::<bool>(id.with("focus-first")))
                .unwrap_or(false);
        contents(ui, focus);
        ui.data_mut(|data| data.remove::<egui::Id>(egui::Id::new("git-active-menu")));
        if focus {
            ui.data_mut(|data| data.remove::<bool>(id.with("focus-first")));
        }
    });
    git_popup_focus(response, id);
}

fn git_menu_item(
    ui: &mut egui::Ui,
    label: &str,
    shortcut: &str,
    tooltip: &str,
    focus: bool,
) -> egui::Response {
    let response = design::menu_item(ui, label, shortcut, tooltip);
    if response.enabled() {
        ui.data_mut(|data| {
            if let Some(id) = data.get_temp::<egui::Id>(egui::Id::new("git-active-menu")) {
                data.get_temp_mut_or_default::<Vec<egui::Id>>(id)
                    .push(response.id);
            }
        });
        if response.has_focus() {
            ui.memory_mut(|memory| {
                memory.set_focus_lock_filter(
                    response.id,
                    egui::EventFilter {
                        vertical_arrows: true,
                        ..Default::default()
                    },
                )
            });
        }
    }
    if focus && response.enabled() {
        response.request_focus();
    }
    response
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChangeGroup {
    Staged,
    Changes,
    Untracked,
}

impl ChangeGroup {
    fn key(self) -> &'static str {
        match self {
            Self::Staged => "staged",
            Self::Changes => "changes",
            Self::Untracked => "untracked",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Staged => "Staged",
            Self::Changes => "Changes",
            Self::Untracked => "Untracked",
        }
    }

    fn staged(self) -> bool {
        self == Self::Staged
    }
}

#[derive(Clone)]
struct TreeFile {
    path: String,
    original_path: Option<String>,
    status: git::FileChange,
    stats: Option<git::LineStats>,
    conflicted: bool,
}

impl TreeFile {
    fn action_paths(&self) -> Vec<PathBuf> {
        let mut paths = vec![PathBuf::from(&self.path)];
        // The source belongs to a rename on this side of the index only.
        // A copied source or an already staged rename's unstaged edit is a
        // separate path and must not be staged/unstaged as a side effect.
        if self.status == git::FileChange::Renamed
            && let Some(original) = &self.original_path
        {
            paths.push(PathBuf::from(original));
        }
        paths
    }
}

#[derive(Default)]
struct FileTree {
    folders: BTreeMap<String, FileTree>,
    files: Vec<TreeFile>,
    summary: StatsSummary,
}

#[derive(Clone, Copy, Default)]
struct StatsSummary {
    added: u64,
    removed: u64,
    binary: usize,
    known: usize,
    total: usize,
}

impl StatsSummary {
    fn add(&mut self, stats: Option<git::LineStats>) {
        self.total += 1;
        if let Some(stats) = stats {
            self.known += 1;
            self.added = self.added.saturating_add(stats.added);
            self.removed = self.removed.saturating_add(stats.removed);
            self.binary += usize::from(stats.binary);
        }
    }
}

impl FileTree {
    fn from_files(files: impl IntoIterator<Item = TreeFile>) -> Self {
        let mut tree = Self::default();
        for file in files {
            let parts: Vec<_> = file.path.split('/').collect();
            tree.insert(&parts, file.clone());
        }
        tree.sort();
        tree
    }

    fn insert(&mut self, parts: &[&str], file: TreeFile) {
        self.summary.add(file.stats);
        if parts.len() > 1 {
            self.folders
                .entry(parts[0].to_owned())
                .or_default()
                .insert(&parts[1..], file);
        } else {
            self.files.push(file);
        }
    }

    fn sort(&mut self) {
        self.files.sort_by(|a, b| a.path.cmp(&b.path));
        for folder in self.folders.values_mut() {
            folder.sort();
        }
    }

    fn paths(&self, paths: &mut Vec<PathBuf>) {
        for file in &self.files {
            paths.extend(file.action_paths());
        }
        for folder in self.folders.values() {
            folder.paths(paths);
        }
    }
}

struct WorkingTreeCache {
    files: Vec<git::FileStatus>,
    stats: git::WorkingTreeStats,
    groups: [(ChangeGroup, FileTree); 3],
}

impl WorkingTreeCache {
    fn new(files: &[git::FileStatus], stats: &git::WorkingTreeStats) -> Self {
        let make = |group: ChangeGroup| {
            FileTree::from_files(files.iter().filter_map(|file| {
                let belongs = match group {
                    ChangeGroup::Staged => file.is_staged(),
                    ChangeGroup::Changes => file.is_unstaged(),
                    ChangeGroup::Untracked => file.is_untracked(),
                };
                belongs.then(|| TreeFile {
                    path: file.path.clone(),
                    original_path: file.original_path.clone(),
                    status: if group.staged() {
                        file.index
                    } else {
                        file.worktree
                    },
                    stats: if group.staged() {
                        stats.staged.get(&file.path)
                    } else {
                        stats.unstaged.get(&file.path)
                    }
                    .copied(),
                    conflicted: file.is_conflicted(),
                })
            }))
        };
        Self {
            files: files.to_vec(),
            stats: stats.clone(),
            groups: [
                (ChangeGroup::Staged, make(ChangeGroup::Staged)),
                (ChangeGroup::Changes, make(ChangeGroup::Changes)),
                (ChangeGroup::Untracked, make(ChangeGroup::Untracked)),
            ],
        }
    }
}

enum GitUiAction {
    SaveAll,
    Open(PathBuf),
    Diff {
        path: PathBuf,
        staged: bool,
        untracked: bool,
    },
    HistoricalDiff {
        hash: String,
        path: PathBuf,
    },
    Stage(Vec<PathBuf>),
    Unstage(Vec<PathBuf>),
    StageAll,
    UnstageAll,
    Commit(String),
    SelectCommit(String),
    LoadCommitStats(String),
    LoadMore,
    RetryHistory,
    RefreshHistory,
    Fetch,
    Pull,
    Push,
    Initialize,
    SwitchBranch(String),
}

impl VEditApp {
    pub(super) fn git_content_has_focus(&self) -> bool {
        let Some(focused) = self.ctx.memory(|memory| memory.focused()) else {
            return false;
        };
        self.project().is_some_and(|project| {
            let state = &project.git_view;
            state
                .keyboard_rows
                .iter()
                .any(|id| *id == focused || id.with("action") == focused)
                || state.keyboard_controls.contains(&focused)
        })
    }

    pub(super) fn restore_git_detail_content_focus(&mut self) -> bool {
        let Some(project) = self
            .projects
            .get_mut(self.active_project)
            .filter(|project| {
                project.git_view.detail_open && project.git_view.detail_keyboard_focus.is_some()
            })
        else {
            return false;
        };
        project.git_view.detail_keyboard_focus_pending = true;
        self.ctx.request_repaint();
        true
    }

    pub(super) fn restore_git_popup_focus(&mut self) -> bool {
        let Some(id) = take_git_popup_return(&self.ctx) else {
            return false;
        };
        if let Some(tab) = self.tab_mut() {
            tab.focus = false;
        }
        request_git_row_focus(&self.ctx, id);
        self.ctx.request_repaint();
        true
    }

    /// Enter Git without activating a row or stealing arrows from the composer.
    pub(super) fn focus_git_content(&mut self) {
        if let Some(tab) = self.tab() {
            let id = self.document_widget_id(tab.id);
            self.ctx.memory_mut(|memory| {
                if memory.focused() == Some(id) {
                    memory.surrender_focus(id);
                }
            });
        }
        if let Some(tab) = self.tab_mut() {
            tab.focus = false;
        }
        if let Some(project) = self.projects.get_mut(self.active_project) {
            project.git_view.keyboard_focus_pending = true;
        }
        self.ctx.request_repaint();
    }

    fn git_unsaved_document_count(&self) -> usize {
        let Some(active) = self.project() else {
            return 0;
        };
        let root = active.git_root.as_deref().unwrap_or(&active.root);
        self.projects
            .iter()
            .enumerate()
            .flat_map(|(index, project)| {
                project.tabs.iter().filter(move |tab| {
                    (tab.dirty || tab.saving)
                        && tab
                            .path
                            .as_ref()
                            .map_or(index == self.active_project, |path| {
                                context_actions::path_contains(root, path)
                            })
                })
            })
            .count()
    }

    pub(super) fn git_ui(&mut self, ui: &mut egui::Ui) {
        let height = ui.available_height();
        egui::ScrollArea::vertical()
            .id_salt(("git-panel-content", self.active_project))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_max_height(height.max(480.0));
                self.git_ui_contents(ui);
            });
    }

    fn git_ui_contents(&mut self, ui: &mut egui::Ui) {
        let project_index = self.active_project;
        let unsaved_count = self.git_unsaved_document_count();
        let busy = self.git_busy.is_some();
        let Some(project) = self.projects.get_mut(project_index) else {
            return;
        };
        let root = project
            .git_root
            .clone()
            .unwrap_or_else(|| project.root.clone());
        let repo = project.git_root.is_some();
        let loading = project.git_loading;
        let mut state = std::mem::take(&mut project.git_view);
        let previous_split_ratio = state.split_ratio;
        let mut actions = Vec::new();
        if let Some(pos) = ui.input(|input| {
            input
                .pointer
                .press_origin()
                .filter(|_| input.pointer.any_pressed())
        }) {
            if state.history_rect.is_some_and(|rect| rect.contains(pos)) {
                state.section = GitSection::History;
            } else if state.changes_rect.is_some_and(|rect| rect.contains(pos)) {
                state.section = GitSection::Changes;
            }
        }
        let focus_requested = state.keyboard_focus_pending;
        let commit_message_id = git_row_id(&root, "commit-message");
        state.keyboard_controls.clear();
        if repo {
            state.keyboard_controls.push(commit_message_id);
        }
        let focused = ui.memory(|memory| memory.focused());
        let previous_row_index = focused.and_then(|focused| {
            state
                .keyboard_rows
                .iter()
                .position(|id| *id == focused || id.with("action") == focused)
        });
        if state.working_cache.as_ref().is_none_or(|cache| {
            cache.files != project.git_changes || cache.stats != project.git_stats
        }) {
            state.working_cache = Some(WorkingTreeCache::new(
                &project.git_changes,
                &project.git_stats,
            ));
        }
        state.keyboard_rows.clear();
        if repo {
            match state.section {
                GitSection::Changes => {
                    if let Some(cache) = &state.working_cache {
                        for (group, tree) in &cache.groups {
                            if tree.summary.total == 0 {
                                continue;
                            }
                            let key = format!("working:{}", group.key());
                            state.keyboard_rows.push(git_row_id(&root, &key));
                            if state.expanded.get(&key).copied().unwrap_or(true) {
                                tree_keyboard_rows(
                                    tree,
                                    "",
                                    &key,
                                    &root,
                                    &state.expanded,
                                    &mut state.keyboard_rows,
                                );
                            }
                        }
                    }
                }
                GitSection::History => state.keyboard_rows.extend(
                    project
                        .recent_commits
                        .iter()
                        .map(|commit| git_row_id(&root, &commit.hash)),
                ),
            }
        } else {
            state.keyboard_header = None;
        }
        let mut keyboard_target = None;
        if state.keyboard_focus_pending {
            state.keyboard_focus_pending = false;
            keyboard_target = state
                .keyboard_last_focus
                .filter(|id| {
                    (state.section == GitSection::Changes && *id == commit_message_id)
                        || state
                            .keyboard_rows
                            .iter()
                            .any(|row| row == id || row.with("action") == *id)
                })
                .or_else(|| state.keyboard_rows.first().copied());
        } else if let Some(index) = previous_row_index
            && focused.is_some_and(|focused| {
                !state
                    .keyboard_rows
                    .iter()
                    .any(|id| *id == focused || id.with("action") == focused)
            })
        {
            keyboard_target = state
                .keyboard_rows
                .get(index.min(state.keyboard_rows.len().saturating_sub(1)))
                .copied();
        } else if let Some(focused) = focused
            && ui.is_enabled()
            && !egui::Popup::is_any_open(ui.ctx())
        {
            keyboard_target = take_git_navigation(
                ui.ctx(),
                &state.keyboard_rows,
                focused,
                state.keyboard_page_rows,
                self.settings.swap_jk,
            );
        }
        if let Some(id) = keyboard_target {
            request_git_row_focus(ui.ctx(), id);
        }

        ui.spacing_mut().item_spacing.y = 4.0;
        ui.horizontal(|ui| {
            let toolbar_width = if repo { 45.0 } else { 0.0 };
            let width = (ui.available_width() - toolbar_width).max(35.0);
            ui.allocate_ui_with_layout(
                Vec2::new(width, 28.0),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                    let label = project.git_branch.as_deref().unwrap_or(if repo {
                        "Detached HEAD"
                    } else if loading {
                        "Finding repository…"
                    } else {
                        "No repository"
                    });
                    let response = ui
                        .menu_button(
                            RichText::new(format!("{label} ▾")).strong().color(ACCENT),
                            |ui| {
                                ui.set_max_width(360.0);
                                ui.label(RichText::new("Switch branch").strong());
                                if unsaved_count > 0 {
                                    ui.label(
                                        RichText::new(
                                            "Save or close unsaved documents before switching.",
                                        )
                                        .color(ORANGE),
                                    );
                                }
                                egui::ScrollArea::vertical()
                                    .max_height(320.0)
                                    .show(ui, |ui| {
                                        for (kind, heading, prefix) in [
                                            (git::RefKind::Local, "Local branches", "refs/heads/"),
                                            (
                                                git::RefKind::Remote,
                                                "Remote branches",
                                                "refs/remotes/",
                                            ),
                                        ] {
                                            ui.label(RichText::new(heading).small().color(MUTED));
                                            for reference in project
                                                .git_branches
                                                .iter()
                                                .filter(|r| r.kind == kind)
                                            {
                                                let current = kind == git::RefKind::Local
                                                    && project.git_branch.as_deref()
                                                        == Some(&reference.name);
                                                let response = ui.add_enabled(
                                                    !busy && unsaved_count == 0 && !current,
                                                    egui::Button::new(format!(
                                                        "{}{}",
                                                        if current { "✓ " } else { "" },
                                                        reference.name
                                                    ))
                                                    .wrap(),
                                                );
                                                if response.clicked() {
                                                    actions.push(GitUiAction::SwitchBranch(
                                                        format!("{prefix}{}", reference.name),
                                                    ));
                                                    ui.close();
                                                }
                                            }
                                        }
                                    });
                                ui.label(
                                    RichText::new(
                                        "Remote branches create a local tracking branch.",
                                    )
                                    .small()
                                    .color(MUTED),
                                );
                            },
                        )
                        .response;
                    state.keyboard_controls.push(response.id);
                    git_popup_focus(&response, egui::Popup::default_response_id(&response));
                    response
                },
            )
            .inner
            .on_hover_text(crate::paths::display(&root));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if repo {
                    let menu = design::menu(ui, "···", "Repository actions", |ui| {
                        ui.add_enabled_ui(!busy, |ui| {
                            if design::menu_item(
                                ui,
                                "Stage all changes",
                                "",
                                "Stage all tracked and untracked changes in the repository",
                            )
                            .clicked()
                            {
                                actions.push(GitUiAction::StageAll);
                                ui.close();
                            }
                            if design::menu_item(
                                ui,
                                "Unstage all changes",
                                "",
                                "Move every staged change back to the working tree",
                            )
                            .clicked()
                            {
                                actions.push(GitUiAction::UnstageAll);
                                ui.close();
                            }
                            ui.separator();
                            for (label, tip, action) in [
                                ("Fetch", "Fetch updates from the remote", GitUiAction::Fetch),
                                (
                                    "Pull",
                                    "Pull updates into the current branch",
                                    GitUiAction::Pull,
                                ),
                                (
                                    "Push",
                                    "Push committed changes to the remote",
                                    GitUiAction::Push,
                                ),
                            ] {
                                if design::menu_item(ui, label, "", tip).clicked() {
                                    actions.push(action);
                                    ui.close();
                                }
                            }
                        });
                        ui.separator();
                        if design::menu_item(
                            ui,
                            "Copy repository path",
                            "",
                            "Copy the repository's full path",
                        )
                        .clicked()
                        {
                            ui.ctx().copy_text(crate::paths::display(&root));
                            ui.close();
                        }
                    });
                    state.keyboard_controls.push(menu.id);
                    git_popup_focus(&menu, egui::Popup::default_response_id(&menu));
                }
            });
        });
        if let Some(tracking) = &project.git_tracking
            && let Some(upstream) = &tracking.upstream
        {
            let suffix = if tracking.ahead != 0 || tracking.behind != 0 {
                format!("  ↑ {}  ↓ {}", tracking.ahead, tracking.behind)
            } else {
                String::new()
            };
            ui.add(
                egui::Label::new(
                    RichText::new(format!("{upstream}{suffix}"))
                        .size(10.5)
                        .color(MUTED),
                )
                .truncate(),
            )
            .on_hover_text("Remote tracking status, based on the last fetch");
        }
        if let Some(error) = &project.git_error {
            error_note(ui, error);
        }
        if !repo {
            ui.add_space(12.0);
            ui.label(
                RichText::new(if loading {
                    "Reading repository…"
                } else {
                    "Track this project's changes with Git."
                })
                .size(12.0)
                .color(MUTED),
            );
            if !loading && !busy {
                let initialize = design::ghost_button(
                    ui,
                    "Initialize repository",
                    "Create a local Git repository in this project",
                );
                state.keyboard_header = Some(initialize.id);
                state.keyboard_controls.push(initialize.id);
                if initialize.clicked() {
                    actions.push(GitUiAction::Initialize);
                }
            }
        } else {
            ui.add_space(7.0);
            let changed_count = project.git_changes.len();
            let total_height = ui.available_height().max(360.0);
            let ratio = state.split_ratio.unwrap_or(0.5).clamp(0.25, 0.75);
            let changes_height =
                (total_height * ratio).clamp(185.0, (total_height - 140.0).max(185.0));
            let (changes_rect, _) = ui.allocate_exact_size(
                Vec2::new(ui.available_width(), changes_height),
                egui::Sense::hover(),
            );
            state.changes_rect = Some(changes_rect);
            ui.scope_builder(egui::UiBuilder::new().max_rect(changes_rect), |ui| {
                ui.set_clip_rect(changes_rect.intersect(ui.clip_rect()));
                let header = region_header(ui, &format!("Changes  {changed_count}"));
                state.keyboard_header = Some(header.id);
                state.keyboard_controls.push(header.id);
                if header.clicked() {
                    state.section = GitSection::Changes;
                    state.keyboard_focus_pending = true;
                }
                let height = ui.available_height();
                egui::ScrollArea::vertical().id_salt(("git-changes-region", project_index)).auto_shrink([false, false]).show(ui, |ui| {
                    ui.set_max_height(height);
                    if unsaved_count > 0 {
                        ui.label(
                            RichText::new(format!(
                                "{unsaved_count} unsaved {}. Git uses files on disk.",
                                if unsaved_count == 1 {
                                    "document"
                                } else {
                                    "documents"
                                }
                            ))
                            .size(11.0)
                            .color(ORANGE),
                        );
                        if design::ghost_button(ui, "Save all files", "Save changed documents in all open projects, including agent drafts. Wait for saving to finish, then stage the saved changes.").clicked() {
                            actions.push(GitUiAction::SaveAll);
                        }
                    }
                    let staged_count = project
                        .git_changes
                        .iter()
                        .filter(|file| file.is_staged())
                        .count();
                    let conflicted = project
                        .git_changes
                        .iter()
                        .any(git::FileStatus::is_conflicted);
                    let composer_height = if conflicted { 145.0 } else { 118.0 };
                    if let Some(error) = &project.git_stats_error {
                        error_note(ui, &format!("Line counts unavailable: {error}"));
                    }
                    let tree_scroll = egui::ScrollArea::vertical()
                        .id_salt(("git-working-tree", project_index))
                        .auto_shrink([false, false])
                        .max_height((ui.available_height() - composer_height).max(24.0))
                        .show(ui, |ui| {
                            if project.git_changes.is_empty() {
                                ui.add_space(20.0);
                                ui.label(
                                    RichText::new(if unsaved_count > 0 {
                                        "No changes on disk"
                                    } else {
                                        "Working tree is clean"
                                    })
                                    .size(12.5)
                                    .color(TEXT),
                                );
                                ui.label(
                                    RichText::new(
                                        "Saved changes appear here. Stage them before committing.",
                                    )
                                    .size(11.0)
                                    .color(MUTED),
                                );
                            }
                            if let Some(cache) = &state.working_cache {
                                for (group, tree) in &cache.groups {
                                    if tree.summary.total == 0 {
                                        continue;
                                    }
                                    working_group(
                                        ui,
                                        *group,
                                        tree,
                                        &root,
                                        &mut state.expanded,
                                        &mut state.selected_file,
                                        busy,
                                        &mut actions,
                                    );
                                    ui.add_space(7.0);
                                }
                            }
                        });
                    if state.section == GitSection::Changes {
                    state.keyboard_page_rows =
                        (tree_scroll.inner_rect.intersect(ui.clip_rect()).height() / 31.0)
                            .floor()
                            .max(1.0) as usize;
                    }
                    ui.add_space(5.0);
                    ui.separator();
                    ui.add_space(5.0);
                    let composer = ui.add(
                        egui::TextEdit::multiline(&mut project.commit_message)
                            .id(commit_message_id)
                            .hint_text("Commit message")
                            .desired_width(f32::INFINITY)
                            .desired_rows(2)
                            .font(FontId::proportional(12.0)),
                    )
                    .on_hover_text(
                        "Describe the staged changes. Unstaged changes stay in the working tree.",
                    );
                    if composer.gained_focus()
                        || (composer.has_focus()
                            && !ui.clip_rect().contains(composer.rect.center()))
                    {
                        composer.scroll_to_me(Some(Align::Center));
                    }
                    if conflicted {
                        ui.label(
                            RichText::new("Resolve conflicts before committing.")
                                .size(10.5)
                                .color(ORANGE),
                        );
                    }
                    let label = if staged_count == 0 {
                        "Commit staged changes".to_owned()
                    } else {
                        format!(
                            "Commit {staged_count} staged {}",
                            if staged_count == 1 { "file" } else { "files" }
                        )
                    };
                    let commit = ui
                        .add_enabled_ui(
                            !busy
                                && staged_count > 0
                                && !conflicted
                                && !project.commit_message.trim().is_empty(),
                            |ui| {
                                design::primary_button(
                                    ui,
                                    &label,
                                    Vec2::new(ui.available_width(), 31.0),
                                )
                            },
                        )
                        .inner
                        .on_hover_text("Create a local commit from the staged files");
                    state.keyboard_controls.push(commit.id);
                    if commit.gained_focus() {
                        commit.scroll_to_me(None);
                    }
                    if commit.clicked() {
                        actions.push(GitUiAction::Commit(project.commit_message.clone()));
                    }
                });
            });
            let (divider_rect, divider) = ui.allocate_exact_size(
                Vec2::new(ui.available_width(), 9.0),
                egui::Sense::click_and_drag(),
            );
            divider.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::Slider,
                    ui.is_enabled(),
                    "Resize Changes and History",
                )
            });
            let divider = divider.on_hover_cursor(egui::CursorIcon::ResizeVertical)
                .on_hover_text("Drag to resize Changes and History. Double-click to reset. Arrow keys resize when focused.");
            if divider.dragged()
                && let Some(pointer) = divider.interact_pointer_pos()
            {
                state.split_ratio =
                    Some(((pointer.y - changes_rect.top()) / total_height).clamp(0.25, 0.75));
            }
            if divider.double_clicked() {
                state.split_ratio = None;
            }
            if divider.has_focus() {
                let step = ui.input_mut(|input| {
                    if input.consume_key(egui::Modifiers::NONE, Key::ArrowUp) {
                        -0.05
                    } else if input.consume_key(egui::Modifiers::NONE, Key::ArrowDown) {
                        0.05
                    } else {
                        0.0
                    }
                });
                if step != 0.0 {
                    state.split_ratio = Some((ratio + step).clamp(0.25, 0.75));
                }
            }
            state.keyboard_controls.push(divider.id);
            ui.painter().line_segment(
                [divider_rect.left_center(), divider_rect.right_center()],
                Stroke::new(
                    1.0,
                    if divider.hovered() || divider.dragged() || divider.has_focus() {
                        ACCENT
                    } else {
                        crate::palette::BORDER_STRONG
                    },
                ),
            );
            let history_rect = ui.available_rect_before_wrap();
            state.history_rect = Some(history_rect);
            ui.scope_builder(egui::UiBuilder::new().max_rect(history_rect), |ui| {
                ui.set_clip_rect(history_rect.intersect(ui.clip_rect()));
                let header = region_header(ui, "History");
                state.keyboard_controls.push(header.id);
                if header.clicked() {
                    state.section = GitSection::History;
                    state.keyboard_focus_pending = true;
                }
                ui.horizontal(|ui| {
                    ui.label(RichText::new("● Local only").small().color(ACCENT));
                    ui.label(RichText::new("● On remote").small().color(BLUE))
                        .on_hover_text("Reachable from a remote branch, based on the last fetch");
                });

                // The graph grows once per received page, never per painted
                // row. This also initializes synthetic screenshot fixtures.
                if state.graph.rows.len() > project.recent_commits.len() {
                    state.graph.clear();
                }
                state
                    .graph
                    .append(&project.recent_commits[state.graph.rows.len()..]);
                if project.git_history_update_available
                    && design::ghost_button(
                        ui,
                        "History changed · Refresh",
                        "Show the latest branches and commits",
                    )
                    .clicked()
                {
                    actions.push(GitUiAction::RefreshHistory);
                }
                if let Some(error) = &project.git_history_error {
                    error_note(ui, error);
                    if design::ghost_button(ui, "Retry history", "Retry the failed history request")
                        .clicked()
                    {
                        actions.push(GitUiAction::RetryHistory);
                    }
                }
                if project.recent_commits.is_empty()
                    && !loading
                    && project.git_history_error.is_none()
                {
                    ui.add_space(20.0);
                    ui.label(RichText::new("No commits yet").size(12.5).color(TEXT));
                    ui.label(
                        RichText::new("Stage changes and create your first commit.")
                            .size(11.0)
                            .color(MUTED),
                    );
                }
                let mut near_end = false;
                ui.scope(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.spacing_mut().scroll.floating = false;
                    let mut scroll = egui::ScrollArea::vertical()
                        .id_salt(("git-history", project_index, project.git_history_generation))
                        .auto_shrink([false, false])
                        .max_height((ui.available_height() - 31.0).max(HISTORY_ROW_HEIGHT));
                    if state.section == GitSection::History
                        && let Some(target) = keyboard_target
                        && let Some(index) = state.keyboard_rows.iter().position(|id| *id == target)
                    {
                        scroll = scroll.vertical_scroll_offset(index as f32 * HISTORY_ROW_HEIGHT);
                    }
                    let history_scroll = scroll.show_rows(
                        ui,
                        HISTORY_ROW_HEIGHT,
                        project.recent_commits.len(),
                        |ui, range| {
                            near_end = history_near_end(range.end, project.recent_commits.len());
                            for index in range {
                                let commit = &project.recent_commits[index];
                                ui.push_id(&commit.hash, |ui| {
                                    let response = history_row(
                                        ui,
                                        git_row_id(&root, &commit.hash),
                                        commit,
                                        state.selected_commit.as_deref() == Some(&commit.hash),
                                        &state.graph.rows[index],
                                        state.graph.lane_count,
                                    );
                                    history_hover(
                                        &response,
                                        commit,
                                        state.hover_stats.get(&(root.clone(), commit.hash.clone())),
                                        &mut actions,
                                    );
                                    if response.clicked() {
                                        actions
                                            .push(GitUiAction::SelectCommit(commit.hash.clone()));
                                    }
                                    git_context_menu(&response, |ui, focus| {
                                        if git_menu_item(
                                            ui,
                                            "Open commit",
                                            "",
                                            "Show metadata, message, and changed files",
                                            focus,
                                        )
                                        .clicked()
                                        {
                                            actions.push(GitUiAction::SelectCommit(
                                                commit.hash.clone(),
                                            ));
                                            ui.close();
                                        }
                                        if git_menu_item(
                                            ui,
                                            "Copy commit hash",
                                            "",
                                            "Copy the full commit identifier",
                                            false,
                                        )
                                        .clicked()
                                        {
                                            ui.ctx().copy_text(commit.hash.clone());
                                            ui.close();
                                        }
                                    });
                                });
                            }
                        },
                    );
                    if state.section == GitSection::History {
                        state.keyboard_page_rows =
                            (history_scroll.inner_rect.intersect(ui.clip_rect()).height()
                                / HISTORY_ROW_HEIGHT)
                                .floor()
                                .max(1.0) as usize;
                    }
                });
                ui.add_space(7.0);
                if project.git_history_loading || (project.recent_commits.is_empty() && loading) {
                    ui.horizontal(|ui| {
                        if project.git_history_loading {
                            ui.add(egui::Spinner::new().size(14.0).color(MUTED));
                        } else {
                            refresh_indicator(ui, true);
                        }
                        ui.label(RichText::new("Loading commits…").size(11.0).color(MUTED));
                    });
                } else if project.git_history_error.is_none() && !project.recent_commits.is_empty()
                {
                    ui.label(
                        RichText::new(if project.git_history_has_more {
                            "Scroll for earlier commits"
                        } else {
                            "End of available history"
                        })
                        .size(10.5)
                        .color(MUTED),
                    );
                }
                if near_end
                    && project.git_history_has_more
                    && !project.git_history_loading
                    && project.git_history_error.is_none()
                    && !busy
                {
                    actions.push(GitUiAction::LoadMore);
                }
            });
            if busy {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(14.0).color(MUTED));
                    ui.label(
                        RichText::new("Updating repository…")
                            .size(11.0)
                            .color(MUTED),
                    );
                });
            }
        }
        if (focus_requested || (previous_row_index.is_some() && state.keyboard_rows.is_empty()))
            && keyboard_target.is_none()
            && let Some(id) = state.keyboard_header
        {
            request_git_row_focus(ui.ctx(), id);
        }
        if let Some(focused) = ui.memory(|memory| memory.focused())
            && (focused == commit_message_id
                || state
                    .keyboard_rows
                    .iter()
                    .any(|id| *id == focused || id.with("action") == focused))
        {
            state.keyboard_last_focus = Some(focused);
        }
        self.session_dirty |= previous_split_ratio != state.split_ratio;
        project.git_view = state;
        self.perform_git_ui_actions(project_index, &root, actions);
    }

    pub(super) fn git_detail_open(&self) -> bool {
        self.project()
            .is_some_and(|project| project.git_view.detail_open)
    }

    pub(super) fn git_detail_ui(&mut self) {
        let index = self.active_project;
        let Some(project) = self.projects.get_mut(index) else {
            return;
        };
        if !project.git_view.detail_open {
            return;
        }
        let root = project
            .git_root
            .clone()
            .unwrap_or_else(|| project.root.clone());
        let mut state = std::mem::take(&mut project.git_view);
        let mut open = state.detail_open;
        let hash = state.selected_commit.clone().unwrap_or_default();
        let mut actions = Vec::new();
        let child_open = self.diff.is_some();
        if let Some(detail) = &state.detail
            && detail.hash == hash
            && state
                .detail_tree
                .as_ref()
                .is_none_or(|(cached_hash, _)| *cached_hash != hash)
        {
            state.detail_keyboard_focus = None;
            state.detail_tree = Some((
                hash.clone(),
                FileTree::from_files(detail.files.iter().map(|file| TreeFile {
                    path: file.path.clone(),
                    original_path: file.original_path.clone(),
                    status: file.status,
                    stats: Some(file.stats),
                    conflicted: false,
                })),
            ));
        }
        if !child_open
            && let Some((_, tree)) = &state.detail_tree
            && let Some(focused) = self.ctx.memory(|memory| memory.focused())
        {
            let mut rows = Vec::new();
            tree_keyboard_rows(
                tree,
                "",
                &TreeContext::Commit(&hash).key(),
                &root,
                &state.expanded,
                &mut rows,
            );
            if !egui::Popup::is_any_open(&self.ctx)
                && let Some(target) = take_git_navigation(
                    &self.ctx,
                    &rows,
                    focused,
                    state.detail_page_rows,
                    self.settings.swap_jk,
                )
            {
                request_git_row_focus(&self.ctx, target);
            }
        }
        let modal = crate::git_diff_view::dialog(
            &self.ctx,
            egui::Id::new(("git-commit-details", index)),
            &format!("Commit {}", short_hash(&hash)),
            "Close commit details",
            Vec2::new(760.0, 560.0),
            !child_open,
            |ui| {
                if state.detail_loading {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Loading commit…");
                    });
                }
                if let Some(error) = &state.detail_error {
                    error_note(ui, error);
                    if design::ghost_button(ui, "Try again", "Retry loading this commit").clicked()
                    {
                        actions.push(GitUiAction::SelectCommit(hash.clone()));
                    }
                }
                if let Some(detail) = &state.detail {
                    // Do not retain the previous commit's presentation while a
                    // different asynchronous detail request is in flight.
                    if detail.hash != hash {
                        return;
                    }
                    let (subject, body) = detail
                        .message
                        .split_once('\n')
                        .unwrap_or((&detail.message, ""));
                    // The close header stays fixed; even unusually long commit
                    // metadata and octopus-merge parents must fit below it.
                    let detail_scroll = egui::ScrollArea::vertical()
                        .id_salt(("git-commit-content", &hash))
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                    ui.label(RichText::new(subject).size(19.0).strong().color(TEXT));
                    ui.add_space(5.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(&detail.author).strong().size(12.0));
                        ui.label(RichText::new(&detail.author_email).size(11.5).color(MUTED));
                        ui.label(RichText::new(format_date(&detail.date)).size(11.5).color(MUTED));
                    });
                    ui.horizontal(|ui| {
                        ui.add(egui::Label::new(RichText::new(&detail.hash).monospace().size(11.0).color(MUTED)).truncate()).on_hover_text(&detail.hash);
                        if small_button(ui, "Copy hash", "Copy the full commit identifier").clicked() {
                            ui.ctx().copy_text(detail.hash.clone());
                        }
                    });
                    if !detail.parents.is_empty() {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new(if detail.parents.len() == 1 { "Parent" } else { "Parents" }).size(10.5).color(MUTED));
                            for parent in &detail.parents {
                                if design::ghost_button(ui, short_hash(parent), &format!("Open parent commit {parent}")).clicked() {
                                    actions.push(GitUiAction::SelectCommit(parent.clone()));
                                }
                            }
                        });
                    }
                    ui.add_space(5.0);
                    ui.separator();
                            if !body.trim().is_empty() {
                                ui.add_space(8.0);
                                ui.label(RichText::new(body.trim()).size(12.5).color(TEXT));
                                ui.add_space(10.0);
                                ui.separator();
                            }
                            let Some((_, tree)) = &state.detail_tree else { return; };
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(format!("{} changed {}", detail.files.len(), if detail.files.len() == 1 { "file" } else { "files" })).size(12.0).strong());
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    stats_labels(ui, tree.summary);
                                });
                            });
                            ui.label(RichText::new(if detail.parents.len() > 1 { "Changes against the first parent · select a file to view its patch" } else { "Select a file to view its patch in this commit" }).size(10.5).color(MUTED));
                            ui.add_space(7.0);
                            draw_file_tree(ui, tree, "", 0, TreeContext::Commit(&hash), &root, &mut state.expanded, &mut state.selected_file, false, &mut actions);
                        });
                    state.detail_page_rows =
                        (detail_scroll.inner_rect.height() / 31.0).floor().max(1.0) as usize;
                }
            },
        );
        if !child_open && (modal.inner || modal.should_close()) {
            open = false;
        }
        if !child_open {
            if state.detail_keyboard_focus_pending
                && let Some(id) = state.detail_keyboard_focus
                && self
                    .ctx
                    .read_response(id)
                    .is_some_and(|response| response.enabled())
            {
                request_git_row_focus(&self.ctx, id);
                state.detail_keyboard_focus_pending = false;
            }
            for action in &actions {
                if let GitUiAction::HistoricalDiff { hash, path } = action {
                    state.detail_keyboard_focus = Some(git_row_id(
                        &root,
                        &format!(
                            "commit:{hash}:file:{}",
                            path.to_string_lossy().replace('\\', "/")
                        ),
                    ));
                }
            }
            if self.ctx.input(|input| input.key_pressed(Key::Escape))
                && !egui::Popup::is_any_open(&self.ctx)
                && let Some(id) = take_git_popup_return(&self.ctx)
            {
                request_git_row_focus(&self.ctx, id);
            }
        }
        state.detail_open = open;
        project.git_view = state;
        if open {
            self.perform_git_ui_actions(index, &root, actions);
        } else {
            self.restore_git_overlay_focus();
        }
    }

    fn perform_git_ui_actions(
        &mut self,
        project_index: usize,
        root: &Path,
        actions: Vec<GitUiAction>,
    ) {
        for action in actions {
            match action {
                GitUiAction::SaveAll => self.save_all(),
                GitUiAction::Open(path) => self.open_file(path),
                GitUiAction::Diff {
                    path,
                    staged,
                    untracked,
                } => self.request_git_diff(root.to_path_buf(), path, staged, untracked),
                GitUiAction::HistoricalDiff { hash, path } => {
                    self.request_commit_diff(root.to_path_buf(), hash, path)
                }
                GitUiAction::Stage(paths) => {
                    self.git_action(root.to_path_buf(), GitAction::Stage(paths))
                }
                GitUiAction::Unstage(paths) => {
                    self.git_action(root.to_path_buf(), GitAction::Unstage(paths))
                }
                GitUiAction::StageAll => self.git_action(root.to_path_buf(), GitAction::StageAll),
                GitUiAction::UnstageAll => {
                    self.git_action(root.to_path_buf(), GitAction::UnstageAll)
                }
                GitUiAction::Commit(message) => {
                    self.git_action(root.to_path_buf(), GitAction::Commit(message))
                }
                GitUiAction::SelectCommit(hash) => self.request_git_commit(project_index, hash),
                GitUiAction::LoadCommitStats(hash) => {
                    self.request_git_hover_stats(project_index, hash)
                }
                GitUiAction::LoadMore => self.load_more_git_history(),
                GitUiAction::RetryHistory => {
                    let project = &mut self.projects[project_index];
                    project.git_history_error = None;
                    if project.git_history_snapshot.is_some() && project.git_history_has_more {
                        self.load_more_git_history();
                    } else {
                        self.refresh_git_history(project_index);
                    }
                }
                GitUiAction::RefreshHistory => self.refresh_git_history(project_index),
                GitUiAction::Fetch => self.git_action(root.to_path_buf(), GitAction::Fetch),
                GitUiAction::Pull => self.git_action(root.to_path_buf(), GitAction::Pull),
                GitUiAction::Push => self.git_action(root.to_path_buf(), GitAction::Push),
                GitUiAction::SwitchBranch(target) => {
                    self.git_action(root.to_path_buf(), GitAction::SwitchBranch(target))
                }
                GitUiAction::Initialize => {
                    self.git_action(root.to_path_buf(), GitAction::Initialize)
                }
            }
        }
    }
}

fn error_note(ui: &mut egui::Ui, error: &str) {
    ui.label(RichText::new(error).size(11.0).color(ORANGE));
}

pub(super) fn refresh_indicator(ui: &mut egui::Ui, running: bool) -> egui::Response {
    // Keep this slot even while idle: periodic refresh must not move the title
    // or repository controls, nor increase the header's height.
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(14.0), egui::Sense::hover());
    if running {
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::ProgressIndicator,
                ui.is_enabled(),
                "Refreshing repository",
            )
        });
        // Automatic refreshes run every few seconds. An animated spinner here
        // repaints the entire editor at display rate for every Git subprocess.
        // The worker wakes the UI when the snapshot is ready; a static activity
        // mark keeps that wait visible without creating an idle render loop.
        for offset in [-4.0, 0.0, 4.0] {
            ui.painter()
                .circle_filled(rect.center() + Vec2::new(offset, 0.0), 1.25, MUTED);
        }
        response.on_hover_text("Refreshing repository")
    } else {
        response
    }
}

fn region_header(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add_sized(
        [ui.available_width(), 25.0],
        egui::Button::new(RichText::new(label).strong().size(12.0).color(TEXT))
            .fill(SURFACE)
            .stroke(Stroke::NONE)
            .corner_radius(2.0),
    )
}

#[derive(Clone, Copy)]
enum TreeContext<'a> {
    Working(ChangeGroup),
    Commit(&'a str),
}

impl TreeContext<'_> {
    fn key(self) -> String {
        match self {
            Self::Working(group) => format!("working:{}", group.key()),
            Self::Commit(hash) => format!("commit:{hash}"),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn working_group(
    ui: &mut egui::Ui,
    group: ChangeGroup,
    tree: &FileTree,
    root: &Path,
    expanded: &mut HashMap<String, bool>,
    selected: &mut Option<String>,
    busy: bool,
    actions: &mut Vec<GitUiAction>,
) {
    let key = format!("working:{}", group.key());
    let mut open = expanded.get(&key).copied().unwrap_or(true);
    ui.push_id(&key, |ui| {
        let label = format!("{}  {}", group.label(), tree.summary.total);
        let response = tree_row(
            ui,
            git_row_id(root, &key),
            &label,
            0,
            Some(open),
            None,
            tree.summary,
            false,
            true,
            Some(if group.staged() { "−" } else { "+" }),
            !busy,
        );
        response.row.clone().on_hover_text(format!(
            "{} files · click to {} this group",
            group.label(),
            if open { "collapse" } else { "expand" }
        ));
        if response.row.clicked() {
            open = !open;
        }
        git_folder_key(ui, &response.row, &mut open);
        let tooltip = if group.staged() {
            "Unstage every file in this group"
        } else {
            "Stage every file in this group"
        };
        if let Some(button) = response.action {
            let button = button.on_hover_text(tooltip);
            if button.clicked() {
                response.row.request_focus();
                queue_tree_stage(tree, group, actions);
            }
        }
        git_context_menu(&response.row, |ui, focus| {
            ui.add_enabled_ui(!busy, |ui| {
                if git_menu_item(
                    ui,
                    if group.staged() {
                        "Unstage group"
                    } else {
                        "Stage group"
                    },
                    "",
                    tooltip,
                    focus,
                )
                .clicked()
                {
                    queue_tree_stage(tree, group, actions);
                    ui.close();
                }
            });
        });
        if open {
            draw_file_tree(
                ui,
                tree,
                "",
                0,
                TreeContext::Working(group),
                root,
                expanded,
                selected,
                busy,
                actions,
            );
        }
    });
    expanded.insert(key, open);
}

fn queue_tree_stage(tree: &FileTree, group: ChangeGroup, actions: &mut Vec<GitUiAction>) {
    let mut paths = Vec::new();
    tree.paths(&mut paths);
    paths.sort();
    paths.dedup();
    if !paths.is_empty() {
        actions.push(if group.staged() {
            GitUiAction::Unstage(paths)
        } else {
            GitUiAction::Stage(paths)
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_file_tree(
    ui: &mut egui::Ui,
    tree: &FileTree,
    prefix: &str,
    depth: usize,
    context: TreeContext<'_>,
    root: &Path,
    expanded: &mut HashMap<String, bool>,
    selected: &mut Option<String>,
    busy: bool,
    actions: &mut Vec<GitUiAction>,
) {
    let namespace = context.key();
    for (name, folder) in &tree.folders {
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        let key = format!("{namespace}:folder:{path}");
        let mut open = expanded.get(&key).copied().unwrap_or(true);
        ui.push_id(&key, |ui| {
            let response = tree_row(
                ui,
                git_row_id(root, &key),
                name,
                depth + 1,
                Some(open),
                None,
                folder.summary,
                false,
                false,
                match context {
                    TreeContext::Working(group) => Some(if group.staged() { "−" } else { "+" }),
                    TreeContext::Commit(_) => None,
                },
                !busy,
            );
            response.row.clone().on_hover_text(&path);
            if response.row.clicked() {
                open = !open;
            }
            git_folder_key(ui, &response.row, &mut open);
            if let (Some(button), TreeContext::Working(group)) = (response.action, context)
                && button
                    .on_hover_text(if group.staged() {
                        "Unstage files in this folder"
                    } else {
                        "Stage files in this folder"
                    })
                    .clicked()
            {
                response.row.request_focus();
                queue_tree_stage(folder, group, actions);
            }
            git_context_menu(&response.row, |ui, focus| {
                if let TreeContext::Working(group) = context {
                    ui.add_enabled_ui(!busy, |ui| {
                        if git_menu_item(
                            ui,
                            if group.staged() {
                                "Unstage folder"
                            } else {
                                "Stage folder"
                            },
                            "",
                            "Apply to the changed files in this folder, including nested folders",
                            focus,
                        )
                        .clicked()
                        {
                            queue_tree_stage(folder, group, actions);
                            ui.close();
                        }
                    });
                }
                if git_menu_item(
                    ui,
                    "Copy folder path",
                    "",
                    "Copy the full folder path",
                    focus && (busy || matches!(context, TreeContext::Commit(_))),
                )
                .clicked()
                {
                    ui.ctx().copy_text(crate::paths::display(&root.join(&path)));
                    ui.close();
                }
            });
            if open {
                draw_file_tree(
                    ui,
                    folder,
                    &path,
                    depth + 1,
                    context,
                    root,
                    expanded,
                    selected,
                    busy,
                    actions,
                );
            }
        });
        expanded.insert(key, open);
    }
    for file in &tree.files {
        let key = format!("{namespace}:file:{}", file.path);
        let name = file.path.rsplit('/').next().unwrap_or(&file.path);
        let mut summary = StatsSummary::default();
        summary.add(file.stats);
        let (mark, color, status_label) = change_marker(file.status, file.conflicted);
        ui.push_id(&key, |ui| {
            let response = tree_row(
                ui,
                git_row_id(root, &key),
                name,
                depth + 1,
                None,
                Some((mark, color)),
                summary,
                selected.as_deref() == Some(&key),
                false,
                match context {
                    TreeContext::Working(group) => Some(if group.staged() { "−" } else { "+" }),
                    TreeContext::Commit(_) => None,
                },
                !busy,
            );
            let rename = file
                .original_path
                .as_ref()
                .map(|old| {
                    format!(
                        "\n{} from {old}",
                        if file.status == git::FileChange::Copied {
                            "Copied"
                        } else {
                            "Renamed"
                        }
                    )
                })
                .unwrap_or_default();
            let counts = match file.stats {
                Some(stats) if stats.binary => "Binary file · line counts do not apply".to_owned(),
                Some(stats) => format!("{} added, {} removed", stats.added, stats.removed),
                None => "Line counts unavailable".to_owned(),
            };
            let tooltip = format!(
                "{}\n{status_label}{rename}\n{counts}\n{}",
                file.path,
                match context {
                    TreeContext::Working(ChangeGroup::Staged) => "Click to view staged changes",
                    TreeContext::Working(_) => "Click to view changes",
                    TreeContext::Commit(_) => "Click to view the patch in this commit",
                }
            );
            let row = response.row.on_hover_text(tooltip);
            if row.clicked() {
                *selected = Some(key.clone());
                queue_file_diff(file, context, actions);
            }
            if let (Some(button), TreeContext::Working(group)) = (response.action, context)
                && button
                    .on_hover_text(if group.staged() {
                        "Unstage this file"
                    } else {
                        "Stage this file"
                    })
                    .clicked()
            {
                row.request_focus();
                let paths = file.action_paths();
                actions.push(if group.staged() {
                    GitUiAction::Unstage(paths)
                } else {
                    GitUiAction::Stage(paths)
                });
            }
            git_context_menu(&row, |ui, focus| {
                if git_menu_item(
                    ui,
                    "View diff",
                    "",
                    "Show the patch for this version of the file",
                    focus,
                )
                .clicked()
                {
                    *selected = Some(key.clone());
                    queue_file_diff(file, context, actions);
                    ui.close();
                }
                if let TreeContext::Working(group) = context {
                    if ui
                        .add_enabled_ui(file.status != git::FileChange::Deleted, |ui| {
                            git_menu_item(
                                ui,
                                "Open file",
                                "",
                                "Open the current working file in the editor",
                                false,
                            )
                        })
                        .inner
                        .clicked()
                    {
                        actions.push(GitUiAction::Open(root.join(&file.path)));
                        ui.close();
                    }
                    ui.add_enabled_ui(!busy, |ui| {
                        if git_menu_item(
                            ui,
                            if group.staged() {
                                "Unstage file"
                            } else {
                                "Stage file"
                            },
                            "",
                            "Apply only to this file",
                            false,
                        )
                        .clicked()
                        {
                            let paths = file.action_paths();
                            actions.push(if group.staged() {
                                GitUiAction::Unstage(paths)
                            } else {
                                GitUiAction::Stage(paths)
                            });
                            ui.close();
                        }
                    });
                }
                ui.separator();
                if git_menu_item(
                    ui,
                    "Copy relative path",
                    "",
                    "Copy the path within the repository",
                    false,
                )
                .clicked()
                {
                    ui.ctx().copy_text(file.path.clone());
                    ui.close();
                }
                if git_menu_item(ui, "Copy full path", "", "Copy the full path", false).clicked() {
                    ui.ctx()
                        .copy_text(crate::paths::display(&root.join(&file.path)));
                    ui.close();
                }
            });
        });
    }
}

fn queue_file_diff(file: &TreeFile, context: TreeContext<'_>, actions: &mut Vec<GitUiAction>) {
    actions.push(match context {
        TreeContext::Working(group) => GitUiAction::Diff {
            path: PathBuf::from(&file.path),
            staged: group.staged(),
            untracked: group == ChangeGroup::Untracked,
        },
        TreeContext::Commit(hash) => GitUiAction::HistoricalDiff {
            hash: hash.to_owned(),
            path: PathBuf::from(&file.path),
        },
    });
}

struct TreeResponse {
    row: egui::Response,
    action: Option<egui::Response>,
}

#[allow(clippy::too_many_arguments)]
fn tree_row(
    ui: &mut egui::Ui,
    row_id: egui::Id,
    name: &str,
    depth: usize,
    folder: Option<bool>,
    status: Option<(&str, Color32)>,
    stats: StatsSummary,
    selected: bool,
    heading: bool,
    action: Option<&str>,
    action_enabled: bool,
) -> TreeResponse {
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width().max(1.0), 27.0),
        egui::Sense::hover(),
    );
    let action_width = if action.is_some() { 24.0 } else { 0.0 };
    let action_rect = egui::Rect::from_min_max(
        egui::pos2(rect.right() - action_width, rect.top()),
        rect.right_bottom(),
    );
    let content_rect = egui::Rect::from_min_max(
        rect.left_top(),
        egui::pos2(action_rect.left(), rect.bottom()),
    );
    let row = ui.interact(content_rect, row_id, egui::Sense::click());
    row.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            ui.is_enabled(),
            selected,
            name,
        )
    });
    let button = action.map(|label| {
        let mut builder = egui::UiBuilder::new().max_rect(action_rect);
        if !action_enabled {
            builder = builder.disabled();
        }
        let child = ui.new_child(builder);
        let response = child.interact(action_rect, row_id.with("action"), egui::Sense::click());
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Button,
                child.is_enabled(),
                format!("{} {name}", if label == "−" { "Unstage" } else { "Stage" }),
            )
        });
        response
    });
    if ui.is_rect_visible(rect) {
        let hovering = row.hovered() || button.as_ref().is_some_and(egui::Response::hovered);
        if selected || hovering || row.has_focus() {
            ui.painter().rect_filled(
                rect,
                4.0,
                if selected {
                    ui.visuals().selection.bg_fill
                } else {
                    HOVER
                },
            );
        }
        if selected {
            ui.painter().rect_filled(
                egui::Rect::from_min_max(
                    rect.left_top() + Vec2::new(0.0, 5.0),
                    rect.left_bottom() + Vec2::new(2.0, -5.0),
                ),
                1.0,
                ACCENT,
            );
        }
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        let mut x = rect.left() + 5.0 + (depth.min(7) as f32 * 11.0);
        if let Some(open) = folder {
            let center = egui::pos2(x + 5.0, rect.center().y);
            let points = if open {
                vec![
                    center + Vec2::new(-3.0, -1.5),
                    center + Vec2::new(0.0, 1.5),
                    center + Vec2::new(3.0, -1.5),
                ]
            } else {
                vec![
                    center + Vec2::new(-1.5, -3.0),
                    center + Vec2::new(1.5, 0.0),
                    center + Vec2::new(-1.5, 3.0),
                ]
            };
            painter.add(egui::Shape::line(points, Stroke::new(1.3, MUTED)));
            x += 14.0;
            if !heading {
                let folder_rect = egui::Rect::from_min_size(
                    egui::pos2(x, rect.center().y - 4.0),
                    Vec2::new(11.0, 8.0),
                );
                painter.rect_stroke(
                    folder_rect,
                    1.0,
                    Stroke::new(1.1, MUTED),
                    egui::StrokeKind::Inside,
                );
                painter.line_segment(
                    [
                        folder_rect.left_top() + Vec2::new(1.0, -2.0),
                        folder_rect.left_top() + Vec2::new(5.0, -2.0),
                    ],
                    Stroke::new(1.1, MUTED),
                );
                x += 17.0;
            }
        } else if let Some((mark, color)) = status {
            painter.text(
                egui::pos2(x + 6.0, rect.center().y),
                egui::Align2::CENTER_CENTER,
                mark,
                FontId::monospace(10.0),
                color,
            );
            x += 20.0;
        }
        let stats_width = paint_stats(
            ui,
            &painter,
            egui::pos2(content_rect.right() - 5.0, rect.center().y),
            stats,
        );
        let color = if heading || row.hovered() || selected {
            TEXT
        } else {
            TEXT.lerp_to_gamma(MUTED, 0.35)
        };
        let available = (content_rect.right() - stats_width - 12.0 - x).max(1.0);
        let galley = truncated_text(
            ui,
            name,
            FontId::proportional(if heading { 11.5 } else { 12.0 }),
            color,
            available,
        );
        painter.galley(
            egui::pos2(x, rect.center().y - galley.size().y * 0.5),
            galley,
            color,
        );
        if let (Some(label), Some(response)) = (action, &button) {
            if (response.hovered() || response.has_focus()) && action_enabled {
                painter.rect_filled(action_rect.shrink(2.0), 4.0, SURFACE);
            }
            painter.text(
                action_rect.center(),
                egui::Align2::CENTER_CENTER,
                label,
                FontId::proportional(15.0),
                if action_enabled {
                    if response.hovered() { ACCENT } else { MUTED }
                } else {
                    MUTED.gamma_multiply(0.35)
                },
            );
        }
    }
    git_row_keyboard(ui, &row);
    if let Some(button) = &button {
        git_row_keyboard(ui, button);
    }
    let row = row.on_hover_cursor(egui::CursorIcon::PointingHand);
    let button = button.map(|response| response.on_hover_cursor(egui::CursorIcon::PointingHand));
    TreeResponse {
        row,
        action: button,
    }
}

fn truncated_text(
    ui: &egui::Ui,
    text: &str,
    font: FontId,
    color: Color32,
    width: f32,
) -> Arc<egui::Galley> {
    ui.fonts_mut(|fonts| {
        let mut job = egui::text::LayoutJob::simple(text.to_owned(), font, color, width.max(1.0));
        job.wrap.max_rows = 1;
        job.wrap.break_anywhere = true;
        fonts.layout_job(job)
    })
}

fn paint_stats(
    ui: &egui::Ui,
    painter: &egui::Painter,
    right: egui::Pos2,
    stats: StatsSummary,
) -> f32 {
    let font = FontId::monospace(10.0);
    if stats.known == 0 {
        let galley = truncated_text(ui, "—", font, MUTED, 16.0);
        let width = galley.size().x;
        painter.galley(
            egui::pos2(right.x - width, right.y - galley.size().y * 0.5),
            galley,
            MUTED,
        );
        return width;
    }
    if stats.binary == stats.known && stats.added == 0 && stats.removed == 0 {
        let label = if stats.known == stats.total {
            "binary"
        } else {
            "binary…"
        };
        let galley = truncated_text(ui, label, font, MUTED, 48.0);
        let width = galley.size().x;
        painter.galley(
            egui::pos2(right.x - width, right.y - galley.size().y * 0.5),
            galley,
            MUTED,
        );
        return width;
    }
    let partial = if stats.known != stats.total {
        "…"
    } else {
        ""
    };
    let removed = truncated_text(
        ui,
        &format!("−{}{partial}", compact_number(stats.removed)),
        font.clone(),
        RED,
        55.0,
    );
    let added = truncated_text(
        ui,
        &format!("+{}", compact_number(stats.added)),
        font,
        GREEN,
        55.0,
    );
    let removed_width = removed.size().x;
    let width = removed_width + added.size().x + 7.0;
    painter.galley(
        egui::pos2(right.x - removed_width, right.y - removed.size().y * 0.5),
        removed,
        RED,
    );
    painter.galley(
        egui::pos2(right.x - width, right.y - added.size().y * 0.5),
        added,
        GREEN,
    );
    width
}

fn stats_labels(ui: &mut egui::Ui, stats: StatsSummary) {
    ui.label(
        RichText::new(format!("−{}", stats.removed))
            .monospace()
            .size(11.0)
            .color(RED),
    );
    ui.label(
        RichText::new(format!("+{}", stats.added))
            .monospace()
            .size(11.0)
            .color(GREEN),
    );
    if stats.binary > 0 {
        ui.label(
            RichText::new(format!("{} binary", stats.binary))
                .size(10.5)
                .color(MUTED),
        );
    }
}

fn compact_number(value: u64) -> String {
    if value >= 10_000 {
        format!("{:.0}k", value as f64 / 1_000.0)
    } else if value >= 1_000 {
        format!("{:.1}k", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}

const HISTORY_ROW_HEIGHT: f32 = 44.0;

fn history_near_end(visible_end: usize, loaded: usize) -> bool {
    loaded > 0 && visible_end.saturating_add(4) >= loaded
}

fn graph_color(index: usize) -> Color32 {
    [
        ACCENT,
        BLUE,
        ORANGE,
        RED,
        Color32::from_rgb(191, 155, 238),
        Color32::from_rgb(109, 199, 222),
    ][index % 6]
}

fn history_row(
    ui: &mut egui::Ui,
    row_id: egui::Id,
    commit: &git::CommitSummary,
    selected: bool,
    graph: &crate::git_graph::GraphRow,
    lanes: usize,
) -> egui::Response {
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width().max(1.0), HISTORY_ROW_HEIGHT),
        egui::Sense::hover(),
    );
    let response = ui.interact(rect, row_id, egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            ui.is_enabled(),
            selected,
            &commit.subject,
        )
    });
    if ui.is_rect_visible(rect) {
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        if selected || response.hovered() || response.has_focus() {
            painter.rect_filled(
                rect,
                4.0,
                if selected {
                    ui.visuals().selection.bg_fill
                } else {
                    HOVER
                },
            );
        }
        let lanes = lanes.max(graph.lane_count).max(1);
        let graph_width = (20.0 + lanes.saturating_sub(1) as f32 * 12.0)
            .min((rect.width() * 0.34).clamp(20.0, 98.0));
        let pitch = if lanes > 1 {
            (graph_width - 16.0) / (lanes - 1) as f32
        } else {
            0.0
        };
        let lane_x = |lane: usize| rect.left() + 9.0 + lane as f32 * pitch;
        let middle = rect.center().y;
        for (edges, top, bottom) in [
            (&graph.incoming, rect.top(), middle),
            (&graph.outgoing, middle, rect.bottom()),
        ] {
            for edge in edges {
                let from = egui::pos2(lane_x(edge.from_lane), top);
                let to = egui::pos2(lane_x(edge.to_lane), bottom);
                let bend = (top + bottom) * 0.5;
                painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
                    [from, egui::pos2(from.x, bend), egui::pos2(to.x, bend), to],
                    false,
                    Color32::TRANSPARENT,
                    Stroke::new(1.6, graph_color(edge.color)),
                ));
            }
        }
        let node = egui::pos2(lane_x(graph.node_lane), middle);
        let color = if commit.on_remote { BLUE } else { ACCENT };
        painter.circle_filled(
            node,
            5.0,
            if selected {
                ui.visuals().selection.bg_fill
            } else {
                PANEL
            },
        );
        if commit.parents.len() > 1 {
            painter.circle_stroke(node, 4.0, Stroke::new(1.7, color));
            painter.circle_filled(node, 1.5, color);
        } else {
            painter.circle_filled(node, 3.5, color);
        }
        let left = rect.left() + graph_width + 5.0;
        let mut line = egui::text::LayoutJob::default();
        line.wrap.max_rows = 1;
        line.append(
            &commit.subject,
            0.0,
            egui::TextFormat {
                font_id: FontId::proportional(12.0),
                color: TEXT,
                ..Default::default()
            },
        );
        line.append(
            &format!("  {}", commit.author),
            0.0,
            egui::TextFormat {
                font_id: FontId::proportional(11.0),
                color: MUTED,
                ..Default::default()
            },
        );
        // Lay out the whole line without an author reservation. Long subjects
        // keep their space and naturally push the author beyond the row clip.
        let line = ui.fonts_mut(|fonts| fonts.layout_job(line));
        painter.galley(egui::pos2(left, rect.top() + 4.0), line, TEXT);
        let mut detail = egui::text::LayoutJob::default();
        detail.wrap.max_rows = 1;
        detail.append(
            if commit.on_remote {
                "Remote  "
            } else {
                "Local  "
            },
            0.0,
            egui::TextFormat {
                font_id: FontId::proportional(10.5),
                color,
                ..Default::default()
            },
        );
        for reference in &commit.refs {
            detail.append(
                &format!(
                    "{}{}  ",
                    if reference.current { "HEAD → " } else { "" },
                    reference.name
                ),
                0.0,
                egui::TextFormat {
                    font_id: FontId::proportional(10.5),
                    color: if reference.kind == git::RefKind::Remote {
                        BLUE
                    } else {
                        ACCENT
                    },
                    ..Default::default()
                },
            );
        }
        let detail = ui.fonts_mut(|fonts| fonts.layout_job(detail));
        painter.galley(egui::pos2(left, rect.top() + 23.0), detail, MUTED);
    }
    git_row_keyboard(ui, &response);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn history_hover(
    response: &egui::Response,
    commit: &git::CommitSummary,
    stats: Option<&Result<git::CommitStats, String>>,
    actions: &mut Vec<GitUiAction>,
) {
    response.clone().on_hover_ui(|ui| {
        ui.ctx().request_repaint_after(Duration::from_secs(30));
        ui.set_max_width(460.0);
        ui.label(RichText::new(&commit.subject).strong());
        ui.label(RichText::new(&commit.author).color(MUTED));
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        // Snapshot tests inject a fixed clock; production uses wall-clock time.
        #[cfg(test)]
        let now = ui
            .data(|data| data.get_temp::<i64>(egui::Id::new("git-history-test-clock")))
            .unwrap_or(now);
        ui.label(
            RichText::new(format!(
                "{} · {}",
                commit.date.replace('T', " "),
                relative_time(commit.timestamp, now)
            ))
            .color(MUTED),
        );
        ui.label(
            RichText::new(if commit.on_remote {
                "On remote · based on the last fetch"
            } else {
                "Local only · not on a fetched remote branch"
            })
            .color(if commit.on_remote { BLUE } else { ACCENT }),
        );
        ui.label(
            RichText::new(&commit.hash)
                .monospace()
                .size(10.5)
                .color(MUTED),
        );
        if !commit.refs.is_empty() {
            ui.label(
                commit
                    .refs
                    .iter()
                    .map(|reference| {
                        if reference.current {
                            format!("HEAD → {}", reference.name)
                        } else {
                            reference.name.clone()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" · "),
            );
        }
        ui.separator();
        match stats {
            Some(Ok(stats)) => {
                ui.horizontal(|ui| {
                    ui.label(format!(
                        "{} {} changed",
                        stats.files_changed,
                        if stats.files_changed == 1 {
                            "file"
                        } else {
                            "files"
                        }
                    ));
                    ui.label(RichText::new(format!("+{}", stats.lines.added)).color(GREEN));
                    ui.label(RichText::new(format!("−{}", stats.lines.removed)).color(RED));
                });
                if stats.lines.binary {
                    ui.label(
                        RichText::new("Includes binary changes")
                            .size(10.5)
                            .color(MUTED),
                    );
                }
            }
            Some(Err(error)) => {
                ui.label(RichText::new("Change statistics unavailable").color(MUTED));
                ui.label(RichText::new(error).size(10.5).color(MUTED));
            }
            None => {
                actions.push(GitUiAction::LoadCommitStats(commit.hash.clone()));
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(12.0).color(MUTED));
                    ui.label(RichText::new("Loading changes…").color(MUTED));
                });
            }
        }
        ui.label(RichText::new("Open commit details").size(10.5).color(MUTED));
    });
}

fn short_hash(hash: &str) -> &str {
    hash.get(..7).unwrap_or(hash)
}

fn format_date(date: &str) -> String {
    date.replace('T', " ")
}

fn change_marker(
    change: git::FileChange,
    conflicted: bool,
) -> (&'static str, Color32, &'static str) {
    if conflicted {
        return ("!", RED, "Merge conflict");
    }
    match change {
        git::FileChange::Added => ("A", GREEN, "Added"),
        git::FileChange::Deleted => ("D", RED, "Deleted"),
        git::FileChange::Renamed => ("R", BLUE, "Renamed"),
        git::FileChange::Copied => ("C", BLUE, "Copied"),
        git::FileChange::Untracked => ("U", GREEN, "Untracked"),
        git::FileChange::Unmerged => ("!", RED, "Merge conflict"),
        git::FileChange::Modified => ("M", ORANGE, "Modified"),
        git::FileChange::None | git::FileChange::Other(_) => ("·", MUTED, "Changed"),
    }
}

#[cfg(test)]
#[path = "git_ux_tests.rs"]
mod ux_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, stats: Option<git::LineStats>) -> TreeFile {
        TreeFile {
            path: path.to_owned(),
            original_path: None,
            status: git::FileChange::Modified,
            stats,
            conflicted: false,
        }
    }

    fn counts(added: u64, removed: u64) -> git::LineStats {
        git::LineStats {
            added,
            removed,
            binary: false,
        }
    }

    #[test]
    fn tree_keeps_nested_folders_and_stable_order_without_filesystem_access() {
        let tree = FileTree::from_files([
            file("src/renderer/z.rs", Some(counts(5, 2))),
            file("README.md", Some(counts(2, 0))),
            file("docs/Pläne/Überblick.md", None),
            file("src/renderer/a.rs", Some(counts(1, 3))),
            file("src/main.rs", Some(counts(7, 4))),
        ]);
        assert_eq!(
            tree.folders.keys().cloned().collect::<Vec<_>>(),
            ["docs", "src"]
        );
        assert_eq!(tree.files[0].path, "README.md");
        let renderer = &tree.folders["src"].folders["renderer"];
        assert_eq!(
            renderer
                .files
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>(),
            ["src/renderer/a.rs", "src/renderer/z.rs"]
        );
        assert_eq!(renderer.summary.added, 6);
        assert_eq!(renderer.summary.removed, 5);
        assert_eq!(tree.summary.total, 5);
        assert_eq!(tree.summary.known, 4);
        assert_eq!(tree.summary.added, 15);
        assert_eq!(tree.summary.removed, 9);
        assert_eq!(
            tree.folders["docs"].folders["Pläne"].files[0].path,
            "docs/Pläne/Überblick.md"
        );
    }

    #[test]
    fn partially_staged_file_has_two_distinct_rows_and_stats() {
        let files = vec![git::FileStatus {
            path: "src/main.rs".into(),
            original_path: None,
            index: git::FileChange::Modified,
            worktree: git::FileChange::Modified,
        }];
        let stats = git::WorkingTreeStats {
            staged: HashMap::from([("src/main.rs".to_owned(), counts(12, 3))]),
            unstaged: HashMap::from([("src/main.rs".to_owned(), counts(2, 7))]),
        };
        let cache = WorkingTreeCache::new(&files, &stats);
        let staged = &cache.groups[0].1.folders["src"].files[0];
        let unstaged = &cache.groups[1].1.folders["src"].files[0];
        assert_eq!(staged.stats, Some(counts(12, 3)));
        assert_eq!(unstaged.stats, Some(counts(2, 7)));
        assert_eq!(cache.groups[0].1.summary.total, 1);
        assert_eq!(cache.groups[1].1.summary.total, 1);
        assert_eq!(cache.groups[2].1.summary.total, 0);
        assert_ne!(
            TreeContext::Working(ChangeGroup::Staged).key(),
            TreeContext::Working(ChangeGroup::Changes).key()
        );
    }

    #[test]
    fn group_filters_deleted_conflicted_and_untracked_files_correctly() {
        let files = [
            git::FileStatus {
                path: "old.rs".into(),
                original_path: None,
                index: git::FileChange::Deleted,
                worktree: git::FileChange::None,
            },
            git::FileStatus {
                path: "new.rs".into(),
                original_path: None,
                index: git::FileChange::None,
                worktree: git::FileChange::Untracked,
            },
            git::FileStatus {
                path: "conflict.rs".into(),
                original_path: None,
                index: git::FileChange::Unmerged,
                worktree: git::FileChange::Unmerged,
            },
        ];
        let cache = WorkingTreeCache::new(&files, &git::WorkingTreeStats::default());
        assert_eq!(
            cache.groups[0]
                .1
                .files
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>(),
            ["conflict.rs", "old.rs"]
        );
        assert_eq!(cache.groups[1].1.files[0].path, "conflict.rs");
        assert!(cache.groups[1].1.files[0].conflicted);
        assert_eq!(cache.groups[2].1.files[0].path, "new.rs");
        assert_eq!(cache.groups[2].1.summary.known, 0);
    }

    #[test]
    fn bulk_folder_action_includes_rename_source_and_deduplicates_paths() {
        let mut renamed = file("src/new.rs", Some(counts(1, 1)));
        renamed.status = git::FileChange::Renamed;
        renamed.original_path = Some("src/old.rs".into());
        let tree = FileTree::from_files([
            renamed,
            file("src/old.rs", None),
            file("src/nested/other.rs", None),
            file("docs/plan.md", None),
        ]);
        let mut actions = Vec::new();
        queue_tree_stage(&tree.folders["src"], ChangeGroup::Changes, &mut actions);
        assert_eq!(actions.len(), 1);
        let GitUiAction::Stage(paths) = &actions[0] else {
            panic!("folder should stage, not unstage")
        };
        assert_eq!(
            paths,
            &[
                PathBuf::from("src/nested/other.rs"),
                PathBuf::from("src/new.rs"),
                PathBuf::from("src/old.rs")
            ]
        );
        actions.clear();
        queue_tree_stage(&tree.folders["src"], ChangeGroup::Staged, &mut actions);
        assert!(matches!(actions.as_slice(), [GitUiAction::Unstage(_)]));
    }

    #[test]
    fn staged_rename_unstaged_edit_and_copies_do_not_mutate_original_sources() {
        let files = [git::FileStatus {
            path: "src/new.rs".into(),
            original_path: Some("src/old.rs".into()),
            index: git::FileChange::Renamed,
            worktree: git::FileChange::Modified,
        }];
        let cache = WorkingTreeCache::new(&files, &git::WorkingTreeStats::default());
        let mut actions = Vec::new();
        queue_tree_stage(&cache.groups[1].1, ChangeGroup::Changes, &mut actions);
        assert!(
            matches!(actions.as_slice(), [GitUiAction::Stage(paths)] if paths == &[PathBuf::from("src/new.rs")])
        );
        actions.clear();
        queue_tree_stage(&cache.groups[0].1, ChangeGroup::Staged, &mut actions);
        assert!(
            matches!(actions.as_slice(), [GitUiAction::Unstage(paths)] if paths == &[PathBuf::from("src/new.rs"), PathBuf::from("src/old.rs")])
        );
        let mut copied = file("copy.rs", None);
        copied.status = git::FileChange::Copied;
        copied.original_path = Some("source.rs".into());
        assert_eq!(copied.action_paths(), [PathBuf::from("copy.rs")]);
    }

    #[test]
    fn commit_file_action_always_requests_that_commits_patch() {
        let file = file("src/deleted.rs", Some(counts(0, 9)));
        let hash = "0123456789abcdef0123456789abcdef01234567";
        let mut actions = Vec::new();
        queue_file_diff(&file, TreeContext::Commit(hash), &mut actions);
        assert!(
            matches!(actions.as_slice(), [GitUiAction::HistoricalDiff { hash: requested, path }] if requested == hash && path == Path::new("src/deleted.rs"))
        );
        actions.clear();
        queue_file_diff(
            &file,
            TreeContext::Working(ChangeGroup::Staged),
            &mut actions,
        );
        assert!(matches!(
            actions.as_slice(),
            [GitUiAction::Diff {
                staged: true,
                untracked: false,
                ..
            }]
        ));
        actions.clear();
        queue_file_diff(
            &file,
            TreeContext::Working(ChangeGroup::Untracked),
            &mut actions,
        );
        assert!(matches!(
            actions.as_slice(),
            [GitUiAction::Diff {
                staged: false,
                untracked: true,
                ..
            }]
        ));
    }

    #[test]
    fn unknown_and_binary_line_stats_remain_distinguishable_from_zero_changes() {
        let tree = FileTree::from_files([
            file("large.rs", None),
            file(
                "image.png",
                Some(git::LineStats {
                    added: 0,
                    removed: 0,
                    binary: true,
                }),
            ),
            file("empty.md", Some(counts(0, 0))),
        ]);
        assert_eq!(tree.summary.total, 3);
        assert_eq!(tree.summary.known, 2);
        assert_eq!(tree.summary.binary, 1);
        assert!(
            tree.files
                .iter()
                .find(|file| file.path == "large.rs")
                .unwrap()
                .stats
                .is_none()
        );
        assert_eq!(
            tree.files
                .iter()
                .find(|file| file.path == "empty.md")
                .unwrap()
                .stats,
            Some(counts(0, 0))
        );
    }
}

fn relative_time(timestamp: i64, now: i64) -> String {
    if timestamp <= 0 {
        return "time unavailable".into();
    }
    let seconds = now.saturating_sub(timestamp).unsigned_abs();
    if seconds < 60 {
        return if timestamp > now {
            "in less than a minute"
        } else {
            "just now"
        }
        .into();
    }
    let (count, unit) = if seconds < 3600 {
        (seconds / 60, "minute")
    } else if seconds < 86400 {
        (seconds / 3600, "hour")
    } else if seconds < 2_592_000 {
        (seconds / 86400, "day")
    } else if seconds < 31_536_000 {
        (seconds / 2_592_000, "month")
    } else {
        (seconds / 31_536_000, "year")
    };
    let quantity = format!("{count} {unit}{}", if count == 1 { "" } else { "s" });
    if timestamp > now {
        format!("in {quantity}")
    } else {
        format!("{quantity} ago")
    }
}

#[cfg(test)]
mod time_tests {
    use super::*;
    #[test]
    fn git_relative_time_handles_singular_plural_and_future_dates() {
        let now = 2_000_000_000;
        assert_eq!(relative_time(now - 720, now), "12 minutes ago");
        assert_eq!(relative_time(now - 3600, now), "1 hour ago");
        assert_eq!(relative_time(now - 172800, now), "2 days ago");
        assert_eq!(relative_time(now + 120, now), "in 2 minutes");
        assert_eq!(relative_time(now, now), "just now");
    }
}

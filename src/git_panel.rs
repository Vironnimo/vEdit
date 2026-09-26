//! Git navigation is driven entirely by cached worker results.
//!
//! Working files and historical files deliberately have distinct actions: a
//! commit row always opens the patch belonging to that commit, never today's
//! file contents. Tree state stays local to the project and survives refreshes.

use super::*;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum GitSection {
    #[default]
    Changes,
    History,
}

#[derive(Default)]
pub(super) struct GitViewState {
    pub(super) section: GitSection,
    pub(super) selected_commit: Option<String>,
    pub(super) detail: Option<git::CommitDetails>,
    pub(super) detail_loading: bool,
    pub(super) detail_error: Option<String>,
    pub(super) detail_open: bool,
    pub(super) detail_cache: HashMap<String, git::CommitDetails>,
    pub(super) expanded: HashMap<String, bool>,
    pub(super) selected_file: Option<String>,
    working_cache: Option<WorkingTreeCache>,
    detail_tree: Option<(String, FileTree)>,
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
    LoadMore,
    Fetch,
    Pull,
    Push,
    Initialize,
}

impl MorrowApp {
    pub(super) fn git_ui(&mut self, ui: &mut egui::Ui) {
        let project_index = self.active_project;
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
        let mut actions = Vec::new();

        ui.spacing_mut().item_spacing.y = 4.0;
        ui.horizontal(|ui| {
            let toolbar_width = if repo { 45.0 } else { 0.0 } + if loading { 25.0 } else { 0.0 };
            let width = (ui.available_width() - toolbar_width).max(35.0);
            ui.allocate_ui_with_layout(
                Vec2::new(width, 28.0),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.add(
                        egui::Label::new(
                            RichText::new(project.git_branch.as_deref().unwrap_or(if repo {
                                "Detached HEAD"
                            } else if loading {
                                "Finding repository…"
                            } else {
                                "No repository"
                            }))
                            .size(13.0)
                            .strong()
                            .color(ACCENT),
                        )
                        .truncate(),
                    )
                },
            )
            .inner
            .on_hover_text(crate::paths::display(&root));
            if loading {
                ui.spinner();
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if repo {
                    design::menu(ui, "···", "Repository actions", |ui| {
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
                RichText::new("Track this project's changes with Git.")
                    .size(12.0)
                    .color(MUTED),
            );
            if !loading
                && !busy
                && design::ghost_button(
                    ui,
                    "Initialize repository",
                    "Create a local Git repository in this project",
                )
                .clicked()
            {
                actions.push(GitUiAction::Initialize);
            }
        } else {
            ui.add_space(7.0);
            let changed_count = project.git_changes.len();
            ui.horizontal(|ui| {
                let width = ((ui.available_width() - ui.spacing().item_spacing.x) * 0.5).max(1.0);
                if section_button(
                    ui,
                    &format!("Changes  {changed_count}"),
                    state.section == GitSection::Changes,
                    width,
                    "Browse staged, unstaged, and new files",
                )
                .clicked()
                {
                    state.section = GitSection::Changes;
                }
                if section_button(
                    ui,
                    "History",
                    state.section == GitSection::History,
                    width,
                    "Browse commits and their historical patches",
                )
                .clicked()
                {
                    state.section = GitSection::History;
                }
            });
            ui.add_space(6.0);
            ui.separator();
            match state.section {
                GitSection::Changes => {
                    if state.working_cache.as_ref().is_none_or(|cache| {
                        cache.files != project.git_changes || cache.stats != project.git_stats
                    }) {
                        state.working_cache = Some(WorkingTreeCache::new(
                            &project.git_changes,
                            &project.git_stats,
                        ));
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
                    egui::ScrollArea::vertical()
                        .id_salt(("git-working-tree", project_index))
                        .auto_shrink([false, false])
                        .max_height((ui.available_height() - composer_height).max(65.0))
                        .show(ui, |ui| {
                            if project.git_changes.is_empty() {
                                ui.add_space(20.0);
                                ui.label(
                                    RichText::new("Working tree is clean")
                                        .size(12.5)
                                        .color(TEXT),
                                );
                                ui.label(
                                    RichText::new("Your changes will appear here.")
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
                    ui.add_space(5.0);
                    ui.separator();
                    ui.add_space(5.0);
                    ui.add(
                        egui::TextEdit::multiline(&mut project.commit_message)
                            .hint_text("Commit message")
                            .desired_width(f32::INFINITY)
                            .desired_rows(2)
                            .font(FontId::proportional(12.0)),
                    )
                    .on_hover_text(
                        "Describe the staged changes. Unstaged changes stay in the working tree.",
                    );
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
                    if ui
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
                        .on_hover_text("Create a local commit from the staged files")
                        .clicked()
                    {
                        actions.push(GitUiAction::Commit(project.commit_message.clone()));
                    }
                }
                GitSection::History => {
                    if let Some(error) = &project.git_history_error {
                        error_note(ui, error);
                    }
                    if project.recent_commits.is_empty() && !loading {
                        ui.add_space(20.0);
                        ui.label(RichText::new("No commits yet").size(12.5).color(TEXT));
                        ui.label(
                            RichText::new("Stage changes and create your first commit.")
                                .size(11.0)
                                .color(MUTED),
                        );
                    }
                    let more_available = project.recent_commits.len() >= project.git_history_limit;
                    egui::ScrollArea::vertical()
                        .id_salt(("git-history", project_index))
                        .auto_shrink([false, false])
                        .max_height(
                            (ui.available_height() - if more_available { 44.0 } else { 0.0 })
                                .max(65.0),
                        )
                        .show_rows(ui, 53.0, project.recent_commits.len(), |ui, range| {
                            for index in range {
                                let commit = &project.recent_commits[index];
                                ui.push_id(&commit.hash, |ui| {
                                    let response = history_row(
                                        ui,
                                        commit,
                                        state.selected_commit.as_deref() == Some(&commit.hash),
                                        index + 1 < project.recent_commits.len(),
                                    );
                                    if response.clicked() {
                                        actions
                                            .push(GitUiAction::SelectCommit(commit.hash.clone()));
                                    }
                                    response.context_menu(|ui| {
                                        if design::menu_item(
                                            ui,
                                            "Open commit",
                                            "",
                                            "Show metadata, message, and changed files",
                                        )
                                        .clicked()
                                        {
                                            actions.push(GitUiAction::SelectCommit(
                                                commit.hash.clone(),
                                            ));
                                            ui.close();
                                        }
                                        if design::menu_item(
                                            ui,
                                            "Copy commit hash",
                                            "",
                                            "Copy the full commit identifier",
                                        )
                                        .clicked()
                                        {
                                            ui.ctx().copy_text(commit.hash.clone());
                                            ui.close();
                                        }
                                    });
                                });
                            }
                        });
                    if more_available {
                        ui.add_space(9.0);
                        if ui
                            .add_enabled_ui(!loading && !busy, |ui| {
                                design::ghost_button(
                                    ui,
                                    "Load more commits",
                                    "Load the next page of commit history",
                                )
                            })
                            .inner
                            .clicked()
                        {
                            actions.push(GitUiAction::LoadMore);
                        }
                    }
                }
            }
            if busy {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(
                        RichText::new("Updating repository…")
                            .size(11.0)
                            .color(MUTED),
                    );
                });
            }
        }
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
        if self.diff.is_none()
            && !egui::Popup::is_any_open(&self.ctx)
            && self
                .ctx
                .input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Escape))
        {
            open = false;
        }
        if let Some(detail) = &state.detail
            && detail.hash == hash
            && state
                .detail_tree
                .as_ref()
                .is_none_or(|(cached_hash, _)| *cached_hash != hash)
        {
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
        egui::Window::new(format!("Commit {}", short_hash(&hash)))
            .id(egui::Id::new(("git-commit-details", index)))
            .open(&mut open)
            .default_size(Vec2::new(760.0, 560.0))
            .default_pos(self.ctx.input(|input| input.content_rect()).center() - Vec2::new(380.0, 280.0))
            .min_size(Vec2::new(440.0, 300.0))
            .resizable(true)
            .collapsible(false)
            .show(&self.ctx, |ui| {
                if state.detail_loading {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Loading commit…");
                    });
                }
                if let Some(error) = &state.detail_error {
                    error_note(ui, error);
                    if design::ghost_button(ui, "Try again", "Retry loading this commit").clicked() {
                        actions.push(GitUiAction::SelectCommit(hash.clone()));
                    }
                }
                if let Some(detail) = &state.detail {
                    // Do not retain the previous commit's presentation while a
                    // different asynchronous detail request is in flight.
                    if detail.hash != hash {
                        return;
                    }
                    let (subject, body) = detail.message.split_once('\n').unwrap_or((&detail.message, ""));
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
                    egui::ScrollArea::vertical()
                        .id_salt(("git-commit-content", &hash))
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
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
                }
            });
        state.detail_open = open;
        project.git_view = state;
        self.perform_git_ui_actions(index, &root, actions);
    }

    fn perform_git_ui_actions(
        &mut self,
        project_index: usize,
        root: &Path,
        actions: Vec<GitUiAction>,
    ) {
        for action in actions {
            match action {
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
                GitUiAction::LoadMore => self.load_more_git_history(),
                GitUiAction::Fetch => self.git_action(root.to_path_buf(), GitAction::Fetch),
                GitUiAction::Pull => self.git_action(root.to_path_buf(), GitAction::Pull),
                GitUiAction::Push => self.git_action(root.to_path_buf(), GitAction::Push),
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

fn section_button(
    ui: &mut egui::Ui,
    label: &str,
    selected: bool,
    width: f32,
    tooltip: &str,
) -> egui::Response {
    ui.add_sized(
        [width, 29.0],
        egui::Button::new(RichText::new(label).size(12.0))
            .selected(selected)
            .frame_when_inactive(false),
    )
    .on_hover_text(tooltip)
    .on_hover_cursor(egui::CursorIcon::PointingHand)
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
        let tooltip = if group.staged() {
            "Unstage every file in this group"
        } else {
            "Stage every file in this group"
        };
        if let Some(button) = response.action {
            let button = button.on_hover_text(tooltip);
            if button.clicked() {
                queue_tree_stage(tree, group, actions);
            }
        }
        response.row.context_menu(|ui| {
            ui.add_enabled_ui(!busy, |ui| {
                if design::menu_item(
                    ui,
                    if group.staged() {
                        "Unstage group"
                    } else {
                        "Stage group"
                    },
                    "",
                    tooltip,
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
            if let (Some(button), TreeContext::Working(group)) = (response.action, context)
                && button
                    .on_hover_text(if group.staged() {
                        "Unstage files in this folder"
                    } else {
                        "Stage files in this folder"
                    })
                    .clicked()
            {
                queue_tree_stage(folder, group, actions);
            }
            response.row.context_menu(|ui| {
                if let TreeContext::Working(group) = context {
                    ui.add_enabled_ui(!busy, |ui| {
                        if design::menu_item(
                            ui,
                            if group.staged() {
                                "Unstage folder"
                            } else {
                                "Stage folder"
                            },
                            "",
                            "Apply to the changed files in this folder, including nested folders",
                        )
                        .clicked()
                        {
                            queue_tree_stage(folder, group, actions);
                            ui.close();
                        }
                    });
                }
                if design::menu_item(ui, "Copy folder path", "", "Copy the full folder path")
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
                let paths = file.action_paths();
                actions.push(if group.staged() {
                    GitUiAction::Unstage(paths)
                } else {
                    GitUiAction::Stage(paths)
                });
            }
            row.context_menu(|ui| {
                if design::menu_item(
                    ui,
                    "View diff",
                    "",
                    "Show the patch for this version of the file",
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
                            design::menu_item(
                                ui,
                                "Open file",
                                "",
                                "Open the current working file in the editor",
                            )
                        })
                        .inner
                        .clicked()
                    {
                        actions.push(GitUiAction::Open(root.join(&file.path)));
                        ui.close();
                    }
                    ui.add_enabled_ui(!busy, |ui| {
                        if design::menu_item(
                            ui,
                            if group.staged() {
                                "Unstage file"
                            } else {
                                "Stage file"
                            },
                            "",
                            "Apply only to this file",
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
                if design::menu_item(
                    ui,
                    "Copy relative path",
                    "",
                    "Copy the path within the repository",
                )
                .clicked()
                {
                    ui.ctx().copy_text(file.path.clone());
                    ui.close();
                }
                if design::menu_item(ui, "Copy full path", "", "Copy the full path").clicked() {
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
    let row = ui.interact(content_rect, ui.id().with("row"), egui::Sense::click());
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
        let response = child.interact(action_rect, ui.id().with("action"), egui::Sense::click());
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
            Color32::from_rgb(196, 205, 221)
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
            if response.hovered() && action_enabled {
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
        ACCENT,
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
        ACCENT,
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
            .color(ACCENT),
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

fn history_row(
    ui: &mut egui::Ui,
    commit: &git::CommitSummary,
    selected: bool,
    has_next: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width().max(1.0), 53.0),
        egui::Sense::click(),
    );
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
        let node = egui::pos2(rect.left() + 11.0, rect.top() + 17.0);
        painter.line_segment(
            [egui::pos2(node.x, rect.top()), node - Vec2::new(0.0, 4.0)],
            Stroke::new(1.0, BORDER),
        );
        if has_next {
            painter.line_segment(
                [
                    node + Vec2::new(0.0, 4.0),
                    egui::pos2(node.x, rect.bottom() + 4.0),
                ],
                Stroke::new(1.0, BORDER),
            );
        }
        painter.circle_filled(node, 3.5, if selected { ACCENT } else { MUTED });
        let left = rect.left() + 25.0;
        let hash = truncated_text(
            ui,
            short_hash(&commit.hash),
            FontId::monospace(10.0),
            MUTED,
            55.0,
        );
        let hash_width = hash.size().x;
        painter.galley(
            egui::pos2(rect.right() - hash_width - 5.0, rect.top() + 11.0),
            hash,
            MUTED,
        );
        let subject = truncated_text(
            ui,
            &commit.subject,
            FontId::proportional(12.0),
            TEXT,
            rect.right() - left - hash_width - 15.0,
        );
        painter.galley(egui::pos2(left, rect.top() + 9.0), subject, TEXT);
        let detail = truncated_text(
            ui,
            &format!("{} · {}", commit.author, commit.date),
            FontId::proportional(10.5),
            MUTED,
            rect.right() - left - 8.0,
        );
        painter.galley(egui::pos2(left, rect.top() + 31.0), detail, MUTED);
    }
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(format!(
            "{}\n{} · {}\n{}\nOpen commit details",
            commit.subject, commit.author, commit.date, commit.hash
        ))
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
        git::FileChange::Added => ("A", ACCENT, "Added"),
        git::FileChange::Deleted => ("D", RED, "Deleted"),
        git::FileChange::Renamed => ("R", BLUE, "Renamed"),
        git::FileChange::Copied => ("C", BLUE, "Copied"),
        git::FileChange::Untracked => ("U", ACCENT, "Untracked"),
        git::FileChange::Unmerged => ("!", RED, "Merge conflict"),
        git::FileChange::Modified => ("M", ORANGE, "Modified"),
        git::FileChange::None | git::FileChange::Other(_) => ("·", MUTED, "Changed"),
    }
}

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

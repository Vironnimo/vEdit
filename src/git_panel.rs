//! Git navigation is driven entirely by cached worker results.
//!
//! Working files and historical files deliberately have distinct actions: a
//! commit row always opens the patch belonging to that commit, never today's
//! file contents. Tree state stays local to the project and survives refreshes.

use super::*;
use crate::design::HoverDetails;
use std::collections::VecDeque;

#[path = "git_panel/changes.rs"]
mod changes;
#[path = "git_panel/details.rs"]
mod details;
#[path = "git_panel/file_tree.rs"]
mod file_tree;
#[path = "git_panel/history.rs"]
mod history;
#[path = "git_panel/navigation.rs"]
mod navigation;
#[path = "git_panel/repository_ui.rs"]
mod repository_ui;

use file_tree::{FileTree, WorkingTreeCache, truncated_text};
use history::{HISTORY_ROW_HEIGHT, short_hash};
use navigation::{
    benchmark_previous_git_frame, git_popup_focus, git_row_id, request_git_row_focus,
    take_git_navigation,
};

#[path = "git_repositories.rs"]
pub(super) mod repositories;

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
    /// Divider position as a share of the whole panel below its title.
    /// `None` sizes Changes to its content, up to half of the space.
    pub(super) split_ratio: Option<f32>,
    /// Height Changes needs to show all of its content, from the previous frame.
    changes_needed: f32,
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
    keyboard_rows_key: Option<(PathBuf, bool, GitSection, u64, usize)>,
    keyboard_expanded: HashMap<String, bool>,
    keyboard_last_focus: Option<egui::Id>,
    keyboard_header: Option<egui::Id>,
    keyboard_focus_pending: bool,
    keyboard_page_rows: usize,
    keyboard_controls: Vec<egui::Id>,
    detail_keyboard_focus: Option<egui::Id>,
    detail_keyboard_focus_pending: bool,
    detail_page_rows: usize,
    /// The last clicked history row, so a double-click can open its review.
    history_click: Option<(String, egui::Rect)>,
    history_scroll_offset: f32,
    history_scroll_pending: Option<f32>,
}

impl GitViewState {
    pub(super) fn preserve_history_scroll(
        &mut self,
        previous: &[git::CommitSummary],
        next: &[git::CommitSummary],
    ) {
        // At the top, follow incoming commits. While reading older commits,
        // keep the first visible commit and its partial-row offset in place.
        let offset = self
            .history_scroll_pending
            .unwrap_or(self.history_scroll_offset);
        let row = (offset / HISTORY_ROW_HEIGHT).floor() as usize;
        let anchored = (offset > 0.0)
            .then(|| previous.get(row))
            .flatten()
            .and_then(|commit| next.iter().position(|next| next.hash == commit.hash))
            .map(|index| index as f32 * HISTORY_ROW_HEIGHT + offset % HISTORY_ROW_HEIGHT);
        self.history_scroll_pending = Some(anchored.unwrap_or(offset));
    }

    pub(super) fn update_working_cache(
        &mut self,
        files: &[git::FileStatus],
        stats: &git::WorkingTreeStats,
    ) {
        if self
            .working_cache
            .as_ref()
            .is_none_or(|cache| cache.files != files || cache.stats != *stats)
        {
            self.working_cache = Some(WorkingTreeCache::new(files, stats));
            self.keyboard_rows_key = None;
        }
    }
}

/// Read-only inputs shared by this frame's panel regions; Project owns the state.
struct PanelContext<'a> {
    project_index: usize,
    root: &'a Path,
    unsaved_count: usize,
    busy: bool,
    push_count: Option<usize>,
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
    Fetch,
    Pull,
    Push,
    Initialize,
    UseParentRepository,
    StopUsingParentRepository,
    SwitchBranch(String),
    ReviewCommit(String),
    ReviewBranch,
    ReviewWorkingTree,
    CancelAction,
}

impl VEditApp {
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
        if self.project().is_some_and(Project::is_other_files) {
            ui.label("Open a project folder to use source control.");
            return;
        }
        let height = ui.available_height();
        egui::ScrollArea::vertical()
            .id_salt(("git-panel-content", self.active_project))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_max_height(height.max(480.0));
                self.git_ui_contents(ui);
                // Inner regions can be taller than a short sidebar. Their focus
                // requests must also reveal the control in this outer scroller.
                if self.git_content_has_focus()
                    && let Some(focused) = ui.memory(|memory| memory.focused())
                    && let Some(response) = ui.ctx().read_response(focused)
                    && !ui.clip_rect().contains(response.rect.center())
                {
                    ui.scroll_to_rect(response.rect, Some(Align::Center));
                }
            });
    }

    fn git_ui_contents(&mut self, ui: &mut egui::Ui) {
        let worktree_selector = self.worktree_selector_ui(ui);
        let panel_top = ui.cursor().top();
        let panel_height = ui.available_height().max(1.0);
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
        let push_count = project
            .git_tracking
            .as_ref()
            .filter(|tracking| {
                project.git_changes.is_empty()
                    && unsaved_count == 0
                    && project.git_branch.is_some()
                    && tracking.upstream.is_some()
                    && tracking.divergence_known
                    && tracking.ahead > 0
            })
            .map(|tracking| tracking.ahead);
        let panel = PanelContext {
            project_index,
            root: &root,
            unsaved_count,
            busy,
            push_count,
        };
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
        state.keyboard_controls.extend(worktree_selector);
        if repo && push_count.is_none() {
            state.keyboard_controls.push(commit_message_id);
        }
        let focused = ui.memory(|memory| memory.focused());
        let previous_row_index = focused.and_then(|focused| {
            state
                .keyboard_rows
                .iter()
                .position(|id| *id == focused || id.with("action") == focused)
        });
        if state.working_cache.is_none() {
            state.update_working_cache(&project.git_changes, &project.git_stats);
        }
        if benchmark_previous_git_frame(ui.ctx()) {
            // Reproduce the old per-frame preparation for the paired benchmark.
            state.update_working_cache(&project.git_changes, &project.git_stats);
            state.keyboard_rows_key = None;
        }
        state.update_keyboard_rows(
            &root,
            repo,
            project.git_history_generation,
            &project.recent_commits,
        );
        let mut keyboard_target = None;
        if state.keyboard_focus_pending {
            state.keyboard_focus_pending = false;
            keyboard_target = state
                .keyboard_last_focus
                .filter(|id| {
                    (state.section == GitSection::Changes
                        && push_count.is_none()
                        && *id == commit_message_id)
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

        repository_ui::show_header(ui, project, &mut state, &panel, &mut actions);
        if let Some(error) = &project.git_error {
            error_note(ui, error);
        }
        if let Some(action) = self
            .git_action
            .as_ref()
            .filter(|action| action.root == root)
            && ui.horizontal(|ui| action.progress_ui(ui, 11.5)).inner
        {
            actions.push(GitUiAction::CancelAction);
        }
        if repo && project.git_untracked_collapsed {
            ui.label(
                RichText::new(
                    "Too many untracked files to list individually; untracked folders are shown as a whole. Add build outputs to .gitignore to see single files again.",
                )
                .size(11.0)
                .color(MUTED),
            );
        }
        if !repo {
            repository_ui::show_setup(ui, project, &mut state, &panel, &mut actions);
        } else {
            ui.add_space(7.0);
            let split_top = ui.cursor().top();
            let remaining = panel_top + panel_height - split_top;
            let max_changes =
                (remaining - DIVIDER_HEIGHT - HISTORY_MIN_HEIGHT).max(CHANGES_MIN_HEIGHT);
            let changes_height = match state.split_ratio {
                Some(ratio) => panel_top + ratio * panel_height - split_top,
                None => state.changes_needed.min(remaining * 0.5),
            }
            .clamp(CHANGES_MIN_HEIGHT, max_changes);
            let ratio = (split_top + changes_height - panel_top) / panel_height;
            let (changes_rect, _) = ui.allocate_exact_size(
                Vec2::new(ui.available_width(), changes_height),
                egui::Sense::hover(),
            );
            state.changes_rect = Some(changes_rect);
            changes::show_changes(ui, project, &mut state, &panel, changes_rect, &mut actions);
            let (divider_rect, divider) = ui.allocate_exact_size(
                Vec2::new(ui.available_width(), DIVIDER_HEIGHT),
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
                // Layout clamps the position; the stored share follows the pointer.
                state.split_ratio = Some(((pointer.y - panel_top) / panel_height).clamp(0.0, 1.0));
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
                    state.split_ratio = Some((ratio + step).clamp(0.0, 1.0));
                }
            }
            state.keyboard_controls.push(divider.id);
            ui.painter().line_segment(
                [divider_rect.left_center(), divider_rect.right_center()],
                Stroke::new(
                    1.0,
                    if divider.hovered()
                        || divider.dragged()
                        || crate::design::keyboard_focus(&divider)
                    {
                        ACCENT
                    } else {
                        crate::palette::BORDER_STRONG
                    },
                ),
            );
            history::show_history(
                ui,
                project,
                &mut state,
                &panel,
                keyboard_target,
                &mut actions,
            );
            // Explicit actions already report progress and errors in the status bar.
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
                GitUiAction::Fetch => self.git_action(root.to_path_buf(), GitAction::Fetch),
                GitUiAction::Pull => self.git_action(root.to_path_buf(), GitAction::Pull),
                GitUiAction::Push => self.git_action(root.to_path_buf(), GitAction::Push),
                GitUiAction::SwitchBranch(target) => {
                    self.git_action(root.to_path_buf(), GitAction::SwitchBranch(target))
                }
                GitUiAction::Initialize => {
                    self.git_action(root.to_path_buf(), GitAction::Initialize)
                }
                GitUiAction::UseParentRepository => {
                    self.set_git_parent_repository(project_index, true);
                }
                GitUiAction::StopUsingParentRepository => {
                    self.set_git_parent_repository(project_index, false);
                }
                GitUiAction::ReviewCommit(hash) => {
                    self.show_change_review(project_index, git::ReviewSource::Commit(hash))
                }
                GitUiAction::ReviewBranch => {
                    self.show_change_review(project_index, git::ReviewSource::Branch { base: None })
                }
                GitUiAction::ReviewWorkingTree => {
                    self.show_change_review(project_index, git::ReviewSource::WorkingTree)
                }
                GitUiAction::CancelAction => self.cancel_git_action(),
            }
        }
    }
}

fn error_note(ui: &mut egui::Ui, error: &str) {
    ui.label(RichText::new(error).size(11.0).color(ORANGE));
}

/// Horizontal text inset shared by the branch, summary and section titles.
const SECTION_TEXT_INSET: f32 = 4.0;
/// Keep room for the first control when History is pulled up.
const CHANGES_MIN_HEIGHT: f32 = 34.0;
const HISTORY_MIN_HEIGHT: f32 = 120.0;
const DIVIDER_HEIGHT: f32 = 9.0;

/// A quiet, left-aligned section title. It stays clickable to move keyboard
/// focus into its region, but has no resting fill competing with the content.
fn region_header<'a>(
    ui: &mut egui::Ui,
    label: &str,
    right: impl egui::IntoAtoms<'a>,
) -> egui::Response {
    let button = egui::Button::new(RichText::new(label).strong().size(11.5).color(MUTED))
        .frame_when_inactive(false)
        .stroke(Stroke::NONE)
        .corner_radius(4.0)
        .right_text(right);
    let response = ui
        .scope(|ui| {
            ui.spacing_mut().button_padding.x = SECTION_TEXT_INSET;
            ui.add_sized([ui.available_width(), 22.0], button)
        })
        .inner;
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    response
}

pub(super) fn compact_number(value: u64) -> String {
    if value >= 10_000 {
        format!("{:.0}k", value as f64 / 1_000.0)
    } else if value >= 1_000 {
        format!("{:.1}k", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}

pub(super) fn change_marker(
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
#[path = "git_panel/tests.rs"]
mod tests;

pub(super) fn relative_time(timestamp: i64, now: i64) -> String {
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

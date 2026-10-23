//! Cached file trees, change groups, line statistics and their row actions.

use super::navigation::{git_context_menu, git_folder_key, git_menu_item, git_row_keyboard};
use super::*;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ChangeGroup {
    Staged,
    Changes,
    Untracked,
}

impl ChangeGroup {
    pub(super) fn key(self) -> &'static str {
        match self {
            Self::Staged => "staged",
            Self::Changes => "changes",
            Self::Untracked => "untracked",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Staged => "Staged",
            Self::Changes => "Unstaged",
            Self::Untracked => "Untracked",
        }
    }

    fn staged(self) -> bool {
        self == Self::Staged
    }
}

#[derive(Clone)]
pub(super) struct TreeFile {
    pub(super) path: String,
    pub(super) original_path: Option<String>,
    pub(super) status: git::FileChange,
    pub(super) stats: Option<git::LineStats>,
    pub(super) conflicted: bool,
}

impl TreeFile {
    pub(super) fn action_paths(&self) -> Vec<PathBuf> {
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
pub(super) struct FileTree {
    pub(super) folders: BTreeMap<String, FileTree>,
    pub(super) files: Vec<TreeFile>,
    pub(super) summary: StatsSummary,
}

#[derive(Clone, Copy, Default)]
pub(super) struct StatsSummary {
    pub(super) added: u64,
    pub(super) removed: u64,
    pub(super) binary: usize,
    pub(super) known: usize,
    pub(super) total: usize,
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
    pub(super) fn from_files(files: impl IntoIterator<Item = TreeFile>) -> Self {
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

pub(super) struct WorkingTreeCache {
    pub(super) files: Vec<git::FileStatus>,
    pub(super) stats: git::WorkingTreeStats,
    pub(super) groups: [(ChangeGroup, FileTree); 3],
    pub(super) conflicted: bool,
}

impl WorkingTreeCache {
    pub(super) fn new(files: &[git::FileStatus], stats: &git::WorkingTreeStats) -> Self {
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
            conflicted: files.iter().any(git::FileStatus::is_conflicted),
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum TreeContext<'a> {
    Working(ChangeGroup),
    Commit(&'a str),
}

impl TreeContext<'_> {
    pub(super) fn key(self) -> String {
        match self {
            Self::Working(group) => format!("working:{}", group.key()),
            Self::Commit(hash) => format!("commit:{hash}"),
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn working_group(
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

pub(super) fn queue_tree_stage(
    tree: &FileTree,
    group: ChangeGroup,
    actions: &mut Vec<GitUiAction>,
) {
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
pub(super) fn draw_file_tree(
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
            response.row.clone().on_hover_details(&path);
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
            let row = if benchmark_previous_git_frame(ui.ctx()) {
                response
                    .row
                    .on_hover_text(file_tooltip(file, context, status_label))
            } else {
                response.row.on_hover_details_ui(|ui| {
                    let text = file_tooltip(file, context, status_label);
                    ui.add(egui::Label::new(&text).wrap());
                    text
                })
            };
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

fn file_tooltip(file: &TreeFile, context: TreeContext<'_>, status_label: &str) -> String {
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
    format!(
        "{}\n{status_label}{rename}\n{counts}\n{}",
        file.path,
        match context {
            TreeContext::Working(ChangeGroup::Staged) => "Click to view staged changes",
            TreeContext::Working(_) => "Click to view changes",
            TreeContext::Commit(_) => "Click to view the patch in this commit",
        }
    )
}

pub(super) fn queue_file_diff(
    file: &TreeFile,
    context: TreeContext<'_>,
    actions: &mut Vec<GitUiAction>,
) {
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

/// Change rows match the one-line history rows.
const TREE_ROW_HEIGHT: f32 = 24.0;

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
        Vec2::new(ui.available_width().max(1.0), TREE_ROW_HEIGHT),
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
        if selected || hovering || crate::design::keyboard_focus(&row) {
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
        // Stage and unstage appear where the pointer or keyboard is, like
        // other editors' inline actions, so a long list stays readable.
        if let (Some(label), Some(response)) = (action, &button)
            && (hovering
                || selected
                || crate::design::keyboard_focus(&row)
                || crate::design::keyboard_focus(response))
        {
            if (response.hovered() || crate::design::keyboard_focus(response)) && action_enabled {
                painter.rect_filled(action_rect.shrink(2.0), 4.0, SURFACE);
            }
            painter.text(
                action_rect.center(),
                egui::Align2::CENTER_CENTER,
                label,
                FontId::proportional(15.0),
                if action_enabled {
                    if response.hovered() { TEXT } else { MUTED }
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

pub(super) fn truncated_text(
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

pub(super) fn stats_labels(ui: &mut egui::Ui, stats: StatsSummary) {
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

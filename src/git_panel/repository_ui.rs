//! Repository summary, branch selection, adoption and repository actions.

use super::navigation::git_popup_focus;
use super::*;

pub(super) fn show_header(
    ui: &mut egui::Ui,
    project: &Project,
    state: &mut GitViewState,
    panel: &PanelContext<'_>,
    actions: &mut Vec<GitUiAction>,
) {
    let &PanelContext {
        root,
        unsaved_count,
        busy,
        ..
    } = panel;
    let repo = project.git_root.is_some();
    let loading = project.git_loading;
    ui.spacing_mut().item_spacing.y = 4.0;
    ui.allocate_ui_with_layout(
        Vec2::new(ui.available_width(), 26.0),
        Layout::right_to_left(Align::Center),
        |ui| {
            // Measure the sync status first so a long branch name truncates
            // instead of pushing the remote counts out of the panel.
            if repo {
                sync_status(ui, project);
            }
            ui.allocate_ui_with_layout(
                Vec2::new(ui.available_width().max(35.0), 26.0),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                    // Narrow padding keeps the branch name on the summary's text edge.
                    ui.spacing_mut().button_padding.x = SECTION_TEXT_INSET;
                    let label = project.git_branch.as_deref().unwrap_or(if repo {
                        "Detached HEAD"
                    } else if loading {
                        "Finding repository…"
                    } else if project.git_parent_root.is_some() {
                        "Parent repository available"
                    } else {
                        "No repository"
                    });
                    if !repo {
                        return ui.label(RichText::new(label).strong().color(MUTED));
                    }
                    let button = egui::Button::new(
                        RichText::new(format!("{label} ▾")).size(12.5).color(TEXT),
                    )
                    .frame_when_inactive(false)
                    .min_size(Vec2::new(0.0, 24.0));
                    let (response, _) =
                        egui::containers::menu::MenuButton::from_button(button).ui(ui, |ui| {
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
                                        (git::RefKind::Remote, "Remote branches", "refs/remotes/"),
                                    ] {
                                        ui.label(RichText::new(heading).small().color(MUTED));
                                        for reference in
                                            project.git_branches.iter().filter(|r| r.kind == kind)
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
                                                actions.push(GitUiAction::SwitchBranch(format!(
                                                    "{prefix}{}",
                                                    reference.name
                                                )));
                                                ui.close();
                                            }
                                        }
                                    }
                                });
                            ui.label(
                                RichText::new("Remote branches create a local tracking branch.")
                                    .small()
                                    .color(MUTED),
                            );
                        });
                    state.keyboard_controls.push(response.id);
                    git_popup_focus(&response, egui::Popup::default_response_id(&response));
                    response
                },
            )
            .inner
            .on_hover_text(format!(
                "{}\nFetch, pull, push and other repository actions are in the panel menu (⋯).",
                crate::paths::display(root)
            ));
        },
    );
    if repo && let Some(parent) = &project.git_parent_root {
        ui.label(
            RichText::new("Using parent repository")
                .size(11.0)
                .color(MUTED),
        );
        ui.add(egui::Label::new(crate::paths::display(parent)).wrap());
    }
    if repo {
        let (header, review) = repository_overview(ui, project, state);
        state.keyboard_header = Some(header.id);
        state.keyboard_controls.push(header.id);
        if header.clicked() {
            state.section = GitSection::Changes;
            state.keyboard_focus_pending = true;
        }
        if review {
            actions.push(GitUiAction::ReviewBranch);
        }
    }
}

pub(super) fn show_setup(
    ui: &mut egui::Ui,
    project: &Project,
    state: &mut GitViewState,
    panel: &PanelContext<'_>,
    actions: &mut Vec<GitUiAction>,
) {
    let busy = panel.busy;
    let loading = project.git_loading;
    ui.add_space(12.0);
    if let Some(parent) = &project.git_parent_root {
        let name = parent
            .file_name()
            .unwrap_or(parent.as_os_str())
            .to_string_lossy();
        ui.label(
            RichText::new(format!("This folder belongs to the repository {name}."))
                .size(12.0)
                .color(TEXT),
        );
        ui.add(egui::Label::new(crate::paths::display(parent)).wrap());
        ui.add_space(8.0);
        ui.label(RichText::new("Using it shows changes and history for the entire repository. Git actions also apply to that repository.").size(11.0).color(MUTED));
        let select = ui
            .add_enabled_ui(!loading && !busy, |ui| {
                design::primary_button(ui, "Use parent repository", Vec2::new(0.0, 30.0))
                    .on_hover_text("Remember this repository for this project")
            })
            .inner;
        state.keyboard_header = Some(select.id);
        state.keyboard_controls.push(select.id);
        if select.clicked() {
            actions.push(GitUiAction::UseParentRepository);
        }
    } else {
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
    }
}

impl VEditApp {
    /// Repository actions for the Source control panel menu. Remote actions
    /// are rarely needed by hand, so they share the panel's overflow menu
    /// instead of occupying permanent space at the top of the panel.
    pub(in crate::app) fn git_repository_menu(&mut self, ui: &mut egui::Ui) -> bool {
        let project_index = self.active_project;
        let busy = self.git_busy.is_some();
        let Some(project) = self.projects.get(project_index) else {
            return false;
        };
        let Some(root) = project.git_root.clone() else {
            return false;
        };
        let parent = project.git_parent_root.is_some();
        let (ahead, behind) = project
            .git_tracking
            .as_ref()
            .filter(|tracking| tracking.divergence_known)
            .map_or((None, None), |tracking| {
                (Some(tracking.ahead), Some(tracking.behind))
            });
        let count = |value: Option<usize>, arrow: &str| {
            value
                .filter(|value| *value > 0)
                .map_or_else(String::new, |value| format!("{arrow}{value}"))
        };
        let mut actions = Vec::new();
        if design::menu_item(
            ui,
            "Review all changes",
            "",
            "Read staged, unstaged and untracked changes in one view",
        )
        .clicked()
        {
            actions.push(GitUiAction::ReviewWorkingTree);
            ui.close();
        }
        ui.separator();
        ui.add_enabled_ui(!busy, |ui| {
            for (label, hint, tip, action) in [
                (
                    "Fetch",
                    String::new(),
                    "Update remote status without changing local files",
                    GitUiAction::Fetch,
                ),
                (
                    "Pull",
                    count(behind, "↓"),
                    "Pull the tracking branch with fast-forward only",
                    GitUiAction::Pull,
                ),
                (
                    "Push",
                    count(ahead, "↑"),
                    "Push committed changes using Git's configured push destination",
                    GitUiAction::Push,
                ),
            ] {
                if design::menu_item(ui, label, &hint, tip).clicked() {
                    actions.push(action);
                    ui.close();
                }
            }
            ui.separator();
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
            if parent
                && design::menu_item(
                    ui,
                    "Stop using parent repository",
                    "",
                    "Disable Git for this project until you select the parent repository again",
                )
                .clicked()
            {
                actions.push(GitUiAction::StopUsingParentRepository);
                ui.close();
            }
        });
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
        self.perform_git_ui_actions(project_index, &root, actions);
        true
    }
}

/// One quiet line with what matters for an overview: changed files, their
/// line delta, and how far the branch has moved from the local main branch.
/// This is also the keyboard entry point for the working-tree region.
/// Returns the working-tree status control and whether to review the branch.
fn repository_overview(
    ui: &mut egui::Ui,
    project: &Project,
    state: &GitViewState,
) -> (egui::Response, bool) {
    let mut review = false;
    let mut status = None;
    let inset = egui::Margin {
        left: SECTION_TEXT_INSET as i8,
        ..Default::default()
    };
    let frame = egui::Frame::new().inner_margin(inset).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 5.0;
            if let Some(cache) = &state.working_cache {
                let count = cache.files.len();
                let files = format!("{count} {}", if count == 1 { "file" } else { "files" });
                let breakdown = format!(
                    "{} staged · {} unstaged · {} untracked",
                    cache.groups[0].1.summary.total,
                    cache.groups[1].1.summary.total,
                    cache.groups[2].1.summary.total
                );
                let (label, color) = if cache.conflicted {
                    (format!("Conflicts · {files}"), RED)
                } else if count == 0 {
                    ("No changes".into(), MUTED)
                } else {
                    (files, TEXT)
                };
                status = Some(ui.add(egui::Label::new(RichText::new(label).size(11.5).color(color))
                    .sense(egui::Sense::click()))
                    .on_hover_text(format!("{breakdown}\nA partially staged file counts as staged and unstaged.")));
                let (added, removed) =
                    cache.groups.iter().fold((0u64, 0u64), |(a, r), (_, tree)| {
                        (
                            a.saturating_add(tree.summary.added),
                            r.saturating_add(tree.summary.removed),
                        )
                    });
                if count > 0 && (added > 0 || removed > 0) {
                    let tip = "Lines added and removed in staged, unstaged and untracked files";
                    ui.label(
                        RichText::new(format!("+{}", compact_number(added)))
                            .size(11.5)
                            .color(GREEN),
                    )
                    .on_hover_text(tip);
                    ui.label(
                        RichText::new(format!("−{}", compact_number(removed)))
                            .size(11.5)
                            .color(RED),
                    )
                    .on_hover_text(tip);
                }
            }
            match &project.git_main_comparison {
                Ok(Some(comparison)) if comparison.ahead > 0 || comparison.behind > 0 => {
                    ui.label(RichText::new("·").size(11.5).color(MUTED));
                    review |= ui.add(
                    egui::Label::new(RichText::new(format!(
                        "{} ↑{} ↓{}",
                        comparison.base, comparison.ahead, comparison.behind
                    ))
                    .size(11.5)
                    .color(MUTED))
                    .sense(egui::Sense::click()),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text(format!(
                    "Compared with local {base}: {} commits only on this branch, {} only on {base}\nClick to review this branch's changes",
                    comparison.ahead,
                    comparison.behind,
                    base = comparison.base
                ))
                .clicked();
                }
                Ok(_) => {}
                Err(error) => {
                    ui.label(RichText::new("·").size(11.5).color(MUTED));
                    ui.label(
                        RichText::new("main comparison unavailable")
                            .size(11.5)
                            .color(ORANGE),
                    )
                    .on_hover_details(error);
                }
            }
        })
    });
    (status.unwrap_or(frame.response), review)
}

/// Commits to push and pull, right-aligned beside the branch name.
fn sync_status(ui: &mut egui::Ui, project: &Project) {
    let (text, color, tip) = match &project.git_tracking {
        Some(tracking) if tracking.upstream.is_some() => {
            let upstream = tracking.upstream.as_deref().unwrap_or_default();
            if !tracking.divergence_known {
                (
                    "↑? ↓?".to_owned(),
                    ORANGE,
                    format!("Tracking {upstream}\nStatus unavailable · Fetch to refresh"),
                )
            } else if tracking.ahead == 0 && tracking.behind == 0 {
                (
                    "Up to date".to_owned(),
                    MUTED,
                    format!("Tracking {upstream}\nRemote status as of last fetch"),
                )
            } else {
                (
                    format!("↑{} ↓{}", tracking.ahead, tracking.behind),
                    MUTED,
                    format!(
                        "Tracking {upstream}\n{} commits to push · {} to pull\nRemote status as of last fetch",
                        tracking.ahead, tracking.behind
                    ),
                )
            }
        }
        _ => (
            "No upstream".to_owned(),
            MUTED,
            "No tracking branch configured. Push uses Git's configured push destination."
                .to_owned(),
        ),
    };
    ui.add(
        egui::Label::new(RichText::new(text).size(11.5).color(color))
            .wrap_mode(egui::TextWrapMode::Extend)
            .selectable(false),
    )
    .on_hover_text(tip);
}

//! The working changes region and commit/push composer.

use super::file_tree::working_group;
use super::*;
use egui::AtomExt;

pub(super) fn show_changes(
    ui: &mut egui::Ui,
    project: &mut Project,
    state: &mut GitViewState,
    panel: &PanelContext<'_>,
    changes_rect: egui::Rect,
    actions: &mut Vec<GitUiAction>,
) {
    let &PanelContext {
        project_index,
        root,
        unsaved_count,
        busy,
        push_count,
        bindings,
    } = panel;
    let commit_message_id = git_row_id(root, "commit-message");
    // A child UI keeps overflowing content from pushing the divider down.
    ui.new_child(egui::UiBuilder::new().max_rect(changes_rect)).scope(|ui| {
        ui.set_clip_rect(changes_rect.intersect(ui.clip_rect()));
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
            let (staged_count, conflicted) = if benchmark_previous_git_frame(ui.ctx()) {
                (project.git_changes.iter().filter(|file| file.is_staged()).count(),
                    project.git_changes.iter().any(git::FileStatus::is_conflicted))
            } else {
                state.working_cache.as_ref()
                    .map(|cache| (cache.groups[0].1.summary.total, cache.conflicted))
                    .unwrap_or_default()
            };
            let composer_height = if push_count.is_some() { 30.0 } else if conflicted { 82.0 } else { 62.0 };
            if let Some(error) = &project.git_stats_error {
                error_note(ui, &format!("Line counts unavailable: {error}"));
            }
            let tree_top = ui.cursor().top();
            let tree_scroll = egui::ScrollArea::vertical()
                .id_salt(("git-working-tree", project_index))
                .auto_shrink([false, true])
                .min_scrolled_height(0.0)
                .max_height((ui.available_height() - composer_height).max(24.0))
                .show(ui, |ui| {
                    if let Some(cache) = &state.working_cache {
                        for (group, tree) in &cache.groups {
                            if tree.summary.total == 0 {
                                continue;
                            }
                            working_group(
                                ui,
                                *group,
                                tree,
                                root,
                                &mut state.expanded,
                                &mut state.selected_file,
                                busy,
                                bindings,
                                actions,
                            );
                            ui.add_space(7.0);
                        }
                    }
                });
            let needed = tree_top - changes_rect.top()
                + tree_scroll.content_size.y
                + composer_height
                + 6.0;
            if (needed - state.changes_needed).abs() > 0.5 {
                state.changes_needed = needed;
                if state.split_ratio.is_none() {
                    ui.ctx().request_repaint();
                }
            }
            if state.section == GitSection::Changes {
            state.keyboard_page_rows =
                (tree_scroll.inner_rect.intersect(ui.clip_rect()).height() / 31.0)
                    .floor()
                    .max(1.0) as usize;
            }
            ui.add_space(4.0);
            if let Some(count) = push_count {
                let label = format!("Push {count} {}", if count == 1 { "commit" } else { "commits" });
                let push = ui.add_enabled_ui(!busy, |ui| {
                    design::primary_button(ui, &label, Vec2::new(ui.available_width(), 26.0))
                }).inner.on_hover_text("Push local commits to the configured remote");
                state.keyboard_controls.push(push.id);
                if ui.memory(|memory| memory.has_focus(commit_message_id)) {
                    push.request_focus();
                }
                if push.gained_focus() {
                    push.scroll_to_me(None);
                }
                if push.clicked() {
                    actions.push(GitUiAction::Push);
                }
                return;
            }
            let composer = ui.add(
                egui::TextEdit::multiline(&mut project.commit_message)
                    .id(commit_message_id)
                    .hint_text("Commit message")
                    .desired_width(f32::INFINITY)
                    .desired_rows(1)
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
            let ready = !busy
                && staged_count > 0
                && !conflicted
                && !project.commit_message.trim().is_empty();
            let size = Vec2::new(ui.available_width(), 26.0);
            let commit = if ready {
                design::primary_button(ui, &label, size)
            } else {
                ui.add_enabled(
                    false,
                    egui::Button::new(RichText::new(&label).size(12.5).atom_grow(true)).min_size(size),
                )
            }
            .on_hover_text("Create a local commit from the staged files")
            .on_disabled_hover_text(if staged_count == 0 {
                "Stage files with + to commit them"
            } else if conflicted {
                "Resolve conflicts before committing"
            } else {
                "Write a commit message"
            });
            state.keyboard_controls.push(commit.id);
            if commit.gained_focus() {
                commit.scroll_to_me(None);
            }
            if commit.clicked() {
                actions.push(GitUiAction::Commit(project.commit_message.clone()));
            }
        });
    });
}

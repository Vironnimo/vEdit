//! Commit metadata and historical-file modal, including its focus lifecycle.

use super::file_tree::{TreeContext, TreeFile, draw_file_tree, stats_labels};
use super::history::{format_date, short_hash};
use super::navigation::{take_git_popup_return, tree_keyboard_rows};
use super::*;

impl VEditApp {
    pub(in crate::app) fn git_detail_open(&self) -> bool {
        self.project()
            .is_some_and(|project| project.git_view.detail_open)
    }

    pub(in crate::app) fn git_detail_ui(&mut self) {
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
                        ui.add(egui::Label::new(RichText::new(&detail.hash).monospace().size(11.0).color(MUTED)).truncate()).on_hover_details(&detail.hash);
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
        // The second click of a double-click on a history row lands on the
        // backdrop of the details its first click opened: review the commit.
        let review = (!child_open)
            .then(|| {
                self.ctx.input(|input| {
                    input
                        .pointer
                        .button_double_clicked(egui::PointerButton::Primary)
                        .then(|| input.pointer.interact_pos())
                        .flatten()
                })
            })
            .flatten()
            .filter(|position| !modal.response.rect.contains(*position))
            .and_then(|position| {
                state
                    .history_click
                    .as_ref()
                    .filter(|(clicked, rect)| *clicked == hash && rect.contains(position))
                    .map(|(clicked, _)| clicked.clone())
            });
        if review.is_some() {
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
        if let Some(hash) = review {
            self.show_change_review(index, git::ReviewSource::Commit(hash));
        }
    }
}

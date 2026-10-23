//! Feedback from a change review: diff rows chosen for a comment to the
//! agent, the dialog that writes it, whether the comparison is still fresh,
//! and the unsaved documents it does not include.
//!
//! A comment becomes an agent mark with frozen paths, revisions and line bounds
//! on both sides. Its text is the excerpt of the chosen rows in unified form: every
//! line starts with `+` (added), `-` (removed) or a space (unchanged), and
//! hunk headers are left out.

use super::*;
use crate::agent_marks::{MarkPosition, MarkTarget, ReviewLines, ReviewLocation, ReviewOrigin};
use std::ops::RangeInclusive;

pub(super) const BANNER_HEIGHT: f32 = 30.0;
const PREVIEW_LINES: usize = 8;
const PREVIEW_CHARS: usize = 160;

/// Rows of one file's diff, from the first chosen to the moving end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RowSelection {
    pub(super) file: usize,
    /// The file and version the rows belong to; a refresh that changes them
    /// clears the selection.
    path: String,
    content_key: u64,
    anchor: usize,
    head: usize,
}

impl RowSelection {
    fn new(view: &ChangeReview, file: usize, row: usize) -> Option<Self> {
        let entry = view.files.get(file)?;
        let EntryPatch::Parsed(patch) = &entry.patch else {
            return None;
        };
        (row < patch.rows()).then(|| Self {
            file,
            path: entry.path.clone(),
            content_key: entry.content_key,
            anchor: row,
            head: row,
        })
    }

    pub(super) fn contains(&self, file: usize, row: usize) -> bool {
        self.file == file && self.rows().contains(&row)
    }

    pub(super) fn rows(&self) -> RangeInclusive<usize> {
        self.anchor.min(self.head)..=self.anchor.max(self.head)
    }
}

/// A primary or secondary click on the diff.
pub(super) struct RowClick {
    pub(super) position: egui::Pos2,
    pub(super) extend: bool,
    pub(super) secondary: bool,
}

/// What the comparison was read from, and what the project knew when it
/// arrived: a status read earlier than the comparison is no sign of change.
pub(super) struct Freshness {
    pub(super) head: Option<git::StatusHead>,
    pub(super) fingerprint: u64,
    pub(super) arrived_head: Option<git::StatusHead>,
    pub(super) arrived_fingerprint: Option<u64>,
}

impl Freshness {
    /// Whether the project's latest status shows a repository state this
    /// comparison was not read from. A commit never changes.
    fn outdated(
        &self,
        source: &ReviewSource,
        head: Option<&git::StatusHead>,
        fingerprint: Option<u64>,
    ) -> bool {
        match source {
            ReviewSource::Commit(_) => false,
            ReviewSource::WorkingTree => fingerprint.is_some_and(|fingerprint| {
                fingerprint != self.fingerprint && Some(fingerprint) != self.arrived_fingerprint
            }),
            ReviewSource::Branch { .. } => head.is_some_and(|head| {
                Some(head) != self.head.as_ref() && Some(head) != self.arrived_head.as_ref()
            }),
        }
    }
}

/// What the review shows beside the comparison itself.
#[derive(Default)]
pub(super) struct ReviewContext {
    pub(super) stale: bool,
    pub(super) marked_blocks: usize,
    /// Unsaved documents inside the repository: display path and path.
    pub(super) drafts: Vec<(String, PathBuf)>,
    /// The user swapped `j` and `k`.
    pub(super) swap_jk: bool,
    pub(super) bindings: keybindings::Bindings,
}

/// The open "Comment for agent" dialog.
#[derive(Clone, Debug)]
pub(super) struct CommentDraft {
    /// Repository-relative path.
    path: String,
    start: MarkPosition,
    end: MarkPosition,
    excerpt: String,
    location: String,
    origin: ReviewOrigin,
    note: String,
    focus: bool,
}

impl CommentDraft {
    /// A comment on the selected rows; `None` when they hold no diff line.
    pub(super) fn new(view: &ChangeReview) -> Option<Self> {
        let selection = view.selection.as_ref()?;
        let entry = view.files.get(selection.file)?;
        let comparison = view.comparison.as_ref()?;
        let EntryPatch::Parsed(patch) = &entry.patch else {
            return None;
        };
        let mut excerpt = String::new();
        let mut old_lines = None;
        let mut new_lines = None;
        for row in selection.rows() {
            let Some(line) = patch.diff_line(row) else {
                continue;
            };
            excerpt.push(line.marker);
            excerpt.push_str(line.text);
            excerpt.push('\n');
            ReviewLines::include(&mut old_lines, line.old_line);
            ReviewLines::include(&mut new_lines, line.new_line);
        }
        if excerpt.is_empty() {
            return None;
        }
        // Removed lines alone sit before the next new-side line.
        let (first, last) = match new_lines {
            Some(ReviewLines { start, end }) => (start, Some(end)),
            None => (
                patch.new_line_at(*selection.rows().start()).unwrap_or(1),
                None,
            ),
        };
        let start = MarkPosition {
            line: first,
            column: 1,
        };
        // An exclusive end at the start of the next line covers whole lines.
        let end = MarkPosition {
            line: last.map_or(first, |last| last + 1),
            column: 1,
        };
        let location = ReviewLocation {
            repository: view.repository.clone(),
            old_revision: comparison.old.clone(),
            new_revision: comparison.new.clone(),
            old_path: (!matches!(entry.status, FileChange::Added | FileChange::Untracked))
                .then(|| entry.original_path.as_ref().unwrap_or(&entry.path).clone()),
            new_path: (entry.status != FileChange::Deleted).then(|| entry.path.clone()),
            old_lines,
            new_lines,
        };
        Some(Self {
            location: format!("{} · {}", entry.path, location.lines_label()),
            origin: ReviewOrigin {
                comparison: view.title(),
                source: view.source.kind().to_owned(),
                location: Some(Box::new(location)),
            },
            path: entry.path.clone(),
            start,
            end,
            excerpt,
            note: String::new(),
            focus: true,
        })
    }
}

/// Drop a selection whose file or version a refresh replaced.
pub(super) fn validate_selection(view: &mut ChangeReview) {
    let valid = view.selection.as_ref().is_some_and(|selection| {
        view.files.get(selection.file).is_some_and(|entry| {
            entry.path == selection.path
                && entry.content_key == selection.content_key
                && matches!(&entry.patch, EntryPatch::Parsed(patch)
                    if selection.anchor.max(selection.head) < patch.rows())
        })
    });
    if !valid {
        view.selection = None;
    }
}

/// The row below the sticky header of `file` at scroll `offset`.
pub(super) fn row_on_screen(view: &ChangeReview, file: usize, offset: f32) -> Option<RowSelection> {
    let entry = view.files.get(file)?;
    let EntryPatch::Parsed(patch) = &entry.patch else {
        return None;
    };
    if patch.rows() == 0 {
        return None;
    }
    let local = (offset - view.tops.get(file).copied().unwrap_or(0.0)).max(0.0);
    RowSelection::new(view, file, entry.wrapped.visible(local..local + 1.0).start)
}

/// The new-side line where the selection's moving end is.
pub(super) fn selection_line(view: &ChangeReview, selection: &RowSelection) -> Option<usize> {
    match &view.files.get(selection.file)?.patch {
        EntryPatch::Parsed(patch) => patch.new_line_at(selection.head),
        _ => None,
    }
}

/// Move the selection's end by `step` rows, or start one on screen. Returns
/// the content offset of the moving end's row.
pub(super) fn extend_selection(
    view: &mut ChangeReview,
    current: usize,
    offset: f32,
    step: isize,
) -> Option<f32> {
    match &mut view.selection {
        Some(selection) => {
            let rows = match &view.files[selection.file].patch {
                EntryPatch::Parsed(patch) => patch.rows(),
                _ => return None,
            };
            selection.head = selection
                .head
                .saturating_add_signed(step)
                .min(rows.saturating_sub(1));
        }
        None => view.selection = row_on_screen(view, current, offset),
    }
    let selection = view.selection.as_ref()?;
    let entry = &view.files[selection.file];
    Some(view.tops.get(selection.file)? + FILE_HEADER_HEIGHT + entry.wrapped.offset(selection.head))
}

pub(super) fn click_row(view: &mut ChangeReview, file: usize, row: usize, click: RowClick) {
    match &mut view.selection {
        Some(selection) if click.extend && selection.file == file => selection.head = row,
        Some(selection) if click.secondary && selection.contains(file, row) => {}
        _ => view.selection = RowSelection::new(view, file, row),
    }
}

/// A tint and an accent edge on chosen rows.
pub(super) fn paint_selected_row(ui: &egui::Ui, rect: egui::Rect) {
    ui.painter()
        .rect_filled(rect, 0.0, ACCENT.linear_multiply(0.12));
    ui.painter().rect_filled(
        egui::Rect::from_min_size(rect.min, Vec2::new(2.0, rect.height())),
        0.0,
        ACCENT,
    );
}

/// The diff's context menu. Returns `Some` when "Comment for agent…" was
/// chosen, with the draft when the selection holds diff lines.
pub(super) fn diff_menu(
    body: &egui::Response,
    view: &mut ChangeReview,
    bindings: &keybindings::Bindings,
) -> Option<Option<CommentDraft>> {
    let mut chosen = None;
    let mut clear = false;
    let selected = view.selection.is_some();
    // egui joins the shortcut into a button's accessible name; keep the
    // command name alone, as the other context menus do.
    let item = |ui: &mut egui::Ui, label: &str, shortcut: &str, tooltip: &str| {
        ui.add_enabled_ui(selected, |ui| {
            let response = design::menu_item(ui, label, shortcut, tooltip);
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
            });
            response
        })
        .inner
    };
    body.context_menu(|ui| {
        let comment = item(
            ui,
            "Comment for agent…",
            &keybindings::label(bindings, keybindings::Action::ReviewComment),
            "Write a note on the selected diff lines; the agent receives it with them",
        );
        if comment.clicked() {
            chosen = Some(CommentDraft::new(view));
            ui.close();
        }
        let clear_item = item(ui, "Clear selection", "Esc", "");
        if clear_item.clicked() {
            clear = true;
            ui.close();
        }
    });
    if clear {
        view.selection = None;
    }
    chosen
}

/// "Changes since this review was loaded" beside Refresh, in a
/// right-to-left header.
pub(super) fn stale_indicator(ui: &mut egui::Ui) {
    let tooltip = "The repository changed after this comparison was loaded. Refresh compares again; seen marks and the reading position stay.";
    ui.label(
        RichText::new("Changes since this review was loaded")
            .size(11.5)
            .color(MUTED),
    )
    .on_hover_text(tooltip);
    ui.label(RichText::new("●").size(8.0).color(ORANGE))
        .on_hover_text(tooltip);
}

/// "2 unsaved documents are not included", with a way to show them.
pub(super) fn drafts_banner(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    drafts: &[(String, PathBuf)],
    actions: &mut Vec<ReviewAction>,
) {
    ui.painter().rect_filled(rect, 0.0, SURFACE);
    ui.painter().line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1.0, BORDER),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(Vec2::new(12.0, 0.0)))
            .layout(Layout::left_to_right(Align::Center))
            .id_salt("change-review-drafts"),
    );
    let ui = &mut child;
    ui.spacing_mut().item_spacing.x = 8.0;
    let count = drafts.len();
    let tooltip = "The review compares the files on disk. Save these documents to include their edits, then Refresh.";
    ui.label(RichText::new("●").size(8.0).color(ORANGE))
        .on_hover_text(tooltip);
    ui.label(
        RichText::new(if count == 1 {
            "1 unsaved document is not included".to_owned()
        } else {
            format!("{count} unsaved documents are not included")
        })
        .size(12.0)
        .color(TEXT),
    )
    .on_hover_text(tooltip);
    if let [(name, path)] = drafts {
        if design::ghost_button(ui, "Show", &format!("Show {name} in the editor")).clicked() {
            actions.push(ReviewAction::ShowDocument(path.clone()));
        }
        return;
    }
    let menu = ui.menu_button(RichText::new("Show").size(12.0), |ui| {
        for (name, path) in drafts {
            if design::menu_item(ui, name, "", &crate::paths::display(path)).clicked() {
                actions.push(ReviewAction::ShowDocument(path.clone()));
                ui.close();
            }
        }
    });
    menu.response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Choose an unsaved document to show");
}

/// The "Comment for agent" dialog over the review.
pub(super) fn comment_dialog(
    ctx: &egui::Context,
    body_id: egui::Id,
    view: &mut ChangeReview,
    bindings: &keybindings::Bindings,
    actions: &mut Vec<ReviewAction>,
) {
    let Some(draft) = view.comment.as_mut() else {
        return;
    };
    let mut submit = false;
    let mut cancel = false;
    let modal = egui::Modal::new(body_id.with("comment-for-agent")).show(ctx, |ui| {
        ui.set_width(520.0);
        design::overlay_title(ui, "Comment for agent");
        ui.label(
            RichText::new(format!("{} · {}", draft.location, draft.origin.comparison))
                .size(12.0)
                .color(MUTED),
        );
        ui.add_space(8.0);
        egui::Frame::new()
            .fill(SURFACE)
            .corner_radius(4.0)
            .inner_margin(egui::Margin::symmetric(8, 6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 1.0;
                let lines: Vec<&str> = draft.excerpt.lines().collect();
                for line in lines.iter().take(PREVIEW_LINES) {
                    let color = match line.chars().next() {
                        Some('+') => GREEN,
                        Some('-') => RED,
                        _ => MUTED,
                    };
                    let mut chars = line.chars();
                    let mut shown: String = chars.by_ref().take(PREVIEW_CHARS).collect();
                    if chars.next().is_some() {
                        shown.push('…');
                    }
                    ui.add(
                        egui::Label::new(RichText::new(shown).monospace().size(12.0).color(color))
                            .truncate(),
                    );
                }
                if lines.len() > PREVIEW_LINES {
                    ui.label(
                        RichText::new(format!("… {} more lines", lines.len() - PREVIEW_LINES))
                            .size(11.0)
                            .color(MUTED),
                    );
                }
            });
        ui.add_space(8.0);
        // Consume the command before the note sees its key or generated text.
        submit |= keybindings::consume(bindings, keybindings::Action::ReviewSubmitComment, ctx);
        let note = ui.add(
            egui::TextEdit::multiline(&mut draft.note)
                .hint_text("What should the agent change or check here?")
                .desired_rows(4)
                .desired_width(f32::INFINITY),
        );
        if std::mem::take(&mut draft.focus) {
            note.request_focus();
        }
        ui.add_space(10.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let add = design::primary_button(ui, "Add for agent", Vec2::new(110.0, 28.0))
                .on_hover_text(review_hint(
                    bindings,
                    keybindings::Action::ReviewSubmitComment,
                    "The agent takes it with the other marks",
                ));
            submit |= add.clicked();
            cancel |= design::ghost_button(ui, "Cancel", "Esc").clicked();
        });
    });
    if modal.should_close() {
        // A stray click beside the dialog keeps a written note.
        cancel |= draft.note.trim().is_empty() || ctx.input(|input| input.key_pressed(Key::Escape));
    }
    if submit {
        actions.push(ReviewAction::Comment(draft.clone()));
    }
    if submit || cancel {
        view.comment = None;
        // The note field leaves; the diff takes the keyboard back when drawn.
        ctx.memory_mut(|memory| {
            if let Some(id) = memory.focused() {
                memory.surrender_focus(id);
            }
        });
        view.focus_pending = true;
    }
}

impl VEditApp {
    pub(super) fn review_context(&self, index: usize) -> ReviewContext {
        let project = &self.projects[index];
        let Some(view) = project.change_review.view.as_ref() else {
            return ReviewContext::default();
        };
        let stale = !view.loading
            && project.git_root.as_ref() == Some(&view.repository)
            && view.freshness.as_ref().is_some_and(|freshness| {
                freshness.outdated(
                    &view.source,
                    project.git_head.as_ref(),
                    project.git_fingerprint,
                )
            });
        let drafts = if view.source == ReviewSource::WorkingTree {
            project
                .tabs
                .iter()
                .filter(|tab| tab.dirty || tab.agent_unsaved)
                .filter_map(|tab| {
                    let path = tab.path.as_ref()?;
                    let relative = path.strip_prefix(&view.repository).ok()?;
                    Some((crate::paths::display(relative), path.clone()))
                })
                .collect()
        } else {
            Vec::new()
        };
        ReviewContext {
            stale,
            marked_blocks: self.agent_marks.items.len(),
            drafts,
            swap_jk: self.settings.swap_jk,
            bindings: self.settings.keybindings.clone(),
        }
    }

    pub(in crate::app) fn submit_review_comment(&mut self) {
        let Some(draft) = self
            .projects
            .get_mut(self.active_project)
            .and_then(|project| project.change_review.view.as_mut())
            .and_then(|view| view.comment.take())
        else {
            return;
        };
        self.add_review_comment(self.active_project, draft);
    }

    /// Whether the active project's review is asking for a comment.
    pub(in crate::app) fn review_comment_open(&self) -> bool {
        self.project()
            .and_then(|project| project.change_review.view.as_ref())
            .is_some_and(|view| view.comment.is_some())
    }

    /// Hand a review comment to the agent as a mark on the file.
    pub(super) fn add_review_comment(&mut self, index: usize, draft: CommentDraft) {
        let Some(project) = self.projects.get(index) else {
            return;
        };
        let repository = &draft
            .origin
            .location
            .as_ref()
            .expect("captured review location")
            .repository;
        let path = draft
            .path
            .split('/')
            .fold(repository.clone(), |path, part| path.join(part));
        let target = MarkTarget {
            workspace: project.root.clone(),
            title: draft
                .path
                .rsplit('/')
                .next()
                .unwrap_or(&draft.path)
                .to_owned(),
            path: Some(path),
        };
        match self.agent_marks.add_review(
            target,
            draft.start,
            draft.end,
            &draft.excerpt,
            &draft.note,
            draft.origin.clone(),
        ) {
            Ok(_) => {
                self.session_dirty = true;
                if let Some(view) = self.projects[index].change_review.view.as_mut() {
                    view.selection = None;
                }
                self.status(
                    format!("Comment added for agent · {}", draft.location),
                    false,
                );
            }
            Err(error) => {
                self.status(error.to_string(), true);
                if let Some(view) = self.projects[index].change_review.view.as_mut() {
                    // Keep what was written.
                    view.comment = Some(CommentDraft {
                        focus: true,
                        ..draft
                    });
                }
            }
        }
    }
}

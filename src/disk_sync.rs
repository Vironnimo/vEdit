//! Disk changes to open documents: undoable reloads, conflicts with unsaved
//! edits, and comparisons of a buffer with its disk or saved version.
//!
//! A clean buffer follows its file as one undoable step, so an agent rewriting
//! a file never erases the user's history. A buffer with unsaved edits keeps
//! them; only saving to the original path waits until the user chooses which
//! version continues. Every other feature keeps working during a conflict.
use super::*;
use crate::line_diff;

/// Above this combined size a reload keeps each view's line and column instead
/// of diffing on the UI thread. Undo still restores the previous text.
const MAPPED_RELOAD_BYTES: usize = 4 * 1024 * 1024;

/// A newer disk version observed while the buffer has unsaved edits.
pub(super) struct DiskConflict {
    pub(super) disk: workspace::LoadedDocument,
}

impl DiskConflict {
    pub(super) fn new(disk: workspace::LoadedDocument) -> Self {
        Self { disk }
    }
}

/// The last clean reload from disk. The save state reports it until the next edit.
pub(super) struct DiskReload {
    pub(super) revision: u64,
    pub(super) changed_lines: Option<usize>,
    pub(super) at: Instant,
}

/// One periodic disk check. `path` and `baseline` are the document's file and
/// comparison fingerprint when the check started; a result for a renamed or
/// newly saved document no longer applies.
pub(super) struct DiskCheck {
    pub(super) id: u64,
    pub(super) path: PathBuf,
    pub(super) baseline: workspace::FileFingerprint,
    pub(super) result: Result<workspace::LoadedDocument, String>,
}

/// A document comparison shown in the diff dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Comparison {
    pub(super) tab_id: u64,
    pub(super) kind: ComparisonKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ComparisonKind {
    /// Disk version → buffer, while saving waits for a decision.
    DiskConflict,
    /// Saved text → buffer.
    Saved,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ConflictAction {
    Compare,
    UseDisk,
    KeepMine,
    SaveAs,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ConflictChoice {
    pub(super) tab_id: u64,
    pub(super) action: ConflictAction,
}

/// Maps positions of a replaced text to the same logical text in its successor.
struct TextMap {
    /// `None` when the texts were too large to diff on the UI thread.
    hunks: Option<Vec<line_diff::Hunk>>,
    /// Start and content end (before `\r\n` or `\n`) of each line.
    old_lines: Vec<(usize, usize)>,
    new_lines: Vec<(usize, usize)>,
}

impl TextMap {
    fn new(old: &str, new: &str) -> Self {
        let hunks =
            (old.len() + new.len() <= MAPPED_RELOAD_BYTES).then(|| line_diff::hunks(old, new));
        Self {
            hunks,
            old_lines: line_bounds(old),
            new_lines: line_bounds(new),
        }
    }

    /// The new line that shows the same text as `old_line`. A line inside a
    /// changed block keeps its offset within the block where possible.
    fn line(&self, old_line: usize) -> usize {
        let last = self.new_lines.len() - 1;
        let Some(hunks) = &self.hunks else {
            return old_line.min(last);
        };
        let mut delta = 0isize;
        for hunk in hunks {
            if old_line < hunk.old.start {
                break;
            }
            if old_line < hunk.old.end {
                let line = match hunk.kind() {
                    // A removed line continues on the line after the removal.
                    line_diff::HunkKind::Removed => hunk.new.start,
                    // Only removed and modified blocks contain old lines.
                    line_diff::HunkKind::Added | line_diff::HunkKind::Modified => {
                        let offset = old_line - hunk.old.start;
                        hunk.new.start + offset.min(hunk.new.len() - 1)
                    }
                };
                return line.min(last);
            }
            delta = hunk.new.end as isize - hunk.old.end as isize;
        }
        old_line.saturating_add_signed(delta).min(last)
    }

    /// Map a Unicode scalar offset. The column is kept; a position at the end
    /// of its line stays at the line end, any other one stays on a character.
    fn position(&self, old: usize) -> usize {
        let line = self
            .old_lines
            .partition_point(|(start, _)| *start <= old)
            .saturating_sub(1);
        let (old_start, old_end) = self.old_lines[line];
        let column = old - old_start;
        let (start, end) = self.new_lines[self.line(line)];
        if column >= old_end - old_start {
            end
        } else {
            start + column.min((end - start).saturating_sub(1))
        }
    }

    fn range(&self, range: Range<usize>) -> Range<usize> {
        let start = self.position(range.start);
        start..self.position(range.end).max(start)
    }

    /// Keep the same first visible line where rows have a fixed height.
    fn scroll(&self, offset: f32, row_height: f32) -> f32 {
        if row_height <= 0.0 || !offset.is_finite() || offset <= 0.0 {
            return offset;
        }
        let top = (offset / row_height).floor() as usize;
        offset + (self.line(top) as f32 - top as f32) * row_height
    }

    fn changed_lines(&self) -> Option<usize> {
        self.hunks.as_ref().map(|hunks| {
            hunks
                .iter()
                .map(|hunk| hunk.old.len().max(hunk.new.len()))
                .sum()
        })
    }
}

/// Unicode scalar start and content end of each `\n`-separated line. A raw
/// `\r` before the line break does not belong to the content.
fn line_bounds(text: &str) -> Vec<(usize, usize)> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut previous = None;
    let mut count = 0;
    for (index, character) in text.chars().enumerate() {
        if character == '\n' {
            let end = if previous == Some('\r') {
                index - 1
            } else {
                index
            };
            lines.push((start, end));
            start = index + 1;
        }
        previous = Some(character);
        count = index + 1;
    }
    lines.push((start, count.max(start)));
    lines
}

/// Finishing a counted Vim insert would add text that is not in the buffer yet.
fn pending_insert_changes_text(tab: &EditorTab) -> bool {
    if tab.vim.mode != VimMode::Insert {
        return false;
    }
    let mut vim = tab.vim.clone();
    let mut text = tab.text.clone();
    vim.finish_insert(&mut text);
    text != tab.text
}

fn has_unsaved_edits(tab: &EditorTab) -> bool {
    tab.dirty || pending_insert_changes_text(tab)
}

fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// The legend and, for a disk conflict, its decisions above a comparison.
pub(super) fn comparison_header(
    ui: &mut egui::Ui,
    comparison: &Comparison,
    projects: &[Project],
) -> Option<ConflictChoice> {
    let mut choice = None;
    let conflict = comparison.kind == ComparisonKind::DiskConflict
        && projects
            .iter()
            .flat_map(|project| &project.tabs)
            .any(|tab| tab.id == comparison.tab_id && tab.disk_conflict.is_some());
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        let (old, new) = match comparison.kind {
            ComparisonKind::DiskConflict => ("disk version", "your edits"),
            ComparisonKind::Saved => ("saved version", "current buffer"),
        };
        ui.label(
            RichText::new(format!("− {old}   + {new}"))
                .size(12.0)
                .color(MUTED),
        );
        if conflict {
            ui.add_space(8.0);
            if ui
                .button("Use disk version")
                .on_hover_text("Replace the buffer with the disk version; Undo restores your edits")
                .clicked()
            {
                choice = Some(ConflictAction::UseDisk);
            }
            if ui
                .button("Keep my version")
                .on_hover_text("Keep your edits; the next save replaces the disk version")
                .clicked()
            {
                choice = Some(ConflictAction::KeepMine);
            }
        }
    });
    ui.add_space(4.0);
    choice.map(|action| ConflictChoice {
        tab_id: comparison.tab_id,
        action,
    })
}

impl VEditApp {
    pub(super) fn watch_disk(&mut self) {
        if self.disk_check_running || self.last_disk_check.elapsed() < Duration::from_secs(2) {
            return;
        }
        self.last_disk_check = Instant::now();
        self.settle_undone_conflicts();
        let documents: Vec<_> = self
            .projects
            .iter()
            .flat_map(|p| &p.tabs)
            .filter(|t| !t.loading && !t.saving)
            .filter_map(|t| {
                let baseline = t.fingerprint.clone()?;
                // A conflicted file is compared with the disk version already
                // observed, so an unchanged file is not read again every poll.
                let known = t
                    .disk_conflict
                    .as_ref()
                    .map_or(&baseline, |conflict| &conflict.disk.fingerprint)
                    .clone();
                Some((t.id, t.path.clone()?, baseline, known))
            })
            .collect();
        if documents.is_empty() {
            return;
        }
        self.disk_check_running = true;
        // A panicking check must still clear `disk_check_running`.
        self.worker_or(
            move || {
                let mut checks = Vec::new();
                for (id, path, baseline, known) in documents {
                    match fs::metadata(&path) {
                        Ok(meta)
                            if meta.len() == known.len
                                && meta.modified().ok() == known.modified => {}
                        _ => {
                            let result = workspace::read_document(&path, MAX_DOCUMENT);
                            checks.push(DiskCheck {
                                id,
                                path,
                                baseline,
                                result,
                            });
                        }
                    }
                }
                WorkerResult::DiskChanges(checks)
            },
            |_| WorkerResult::DiskChanges(Vec::new()),
        );
    }

    pub(super) fn apply_disk_checks(&mut self, checks: Vec<DiskCheck>) {
        self.disk_check_running = false;
        for check in checks {
            let Some(tab) = self.tab_by_id_mut(check.id) else {
                continue;
            };
            // A rename, reload or save after the check started makes it stale.
            if tab.path.as_ref() != Some(&check.path)
                || tab.fingerprint.as_ref() != Some(&check.baseline)
                || tab.saving
                || tab.loading
            {
                continue;
            }
            match check.result {
                Ok(disk) if disk.fingerprint.same_content(&check.baseline) => {
                    tab.fingerprint = Some(disk.fingerprint);
                    tab.error = None;
                    tab.disk_conflict = None;
                }
                Ok(disk) => self.disk_version_changed(check.id, disk),
                Err(error) => {
                    tab.error = Some(error);
                    tab.disk_conflict = None;
                }
            }
        }
    }

    fn disk_version_changed(&mut self, id: u64, disk: workspace::LoadedDocument) {
        let Some(tab) = self.tab_by_id_mut(id) else {
            return;
        };
        if has_unsaved_edits(tab) || tab.text == disk.text {
            self.observe_disk_version(id, disk);
            return;
        }
        self.reload_from_disk(id, disk);
    }

    /// Record a newer disk version for a buffer whose edits must be kept. A
    /// buffer that already has the disk text simply adopts it as saved.
    pub(super) fn observe_disk_version(&mut self, id: u64, disk: workspace::LoadedDocument) {
        let Some(tab) = self.tab_by_id_mut(id) else {
            return;
        };
        tab.error = None;
        if tab.text == disk.text {
            tab.saved_text = disk.text;
            tab.fingerprint = Some(disk.fingerprint);
            tab.line_ending = disk.line_ending;
            tab.bom = disk.bom;
            tab.dirty = false;
            tab.clear_agent_draft_when_saved();
            tab.disk_conflict = None;
            // As for a completed save, older snapshots no longer describe the
            // recovery state. Cleanup runs off the UI thread and retries if a
            // reader temporarily holds the file without delete sharing.
            tab.recovery_generation = tab.recovery_generation.wrapping_add(1);
            tab.recovery_stale = true;
            self.session_dirty = true;
            self.start_recovery_cleanup(id);
            return;
        }
        let first = tab.disk_conflict.is_none();
        tab.disk_conflict = Some(DiskConflict::new(disk));
        if first {
            let title = tab.title.clone();
            self.status(
                format!("{title} changed on disk. Your edits are kept; choose a version above the editor."),
                false,
            );
        }
        self.session_dirty = true;
    }

    /// Follow the file of a buffer without unsaved edits, as one undoable step.
    fn reload_from_disk(&mut self, id: u64, disk: workspace::LoadedDocument) {
        let Some(changed) = self.replace_with_disk_version(id, disk) else {
            return;
        };
        let title = self
            .tab_location(id)
            .map(|(p, t)| self.projects[p].tabs[t].title.clone())
            .unwrap_or_default();
        let undo = self.undo_hint();
        self.status(
            match changed {
                Some(lines) if lines > 0 => format!(
                    "Reloaded {title} from disk: {} changed · {undo} restores",
                    plural(lines, "line")
                ),
                _ => format!("Reloaded {title} from disk · {undo} restores"),
            },
            false,
        );
    }

    fn undo_hint(&self) -> String {
        let label = self.shortcut_label(keybindings::Action::Undo);
        if label.is_empty() {
            "Undo".into()
        } else {
            label
        }
    }

    /// A conflicted buffer that was undone back to its saved text has nothing
    /// left to keep; it follows the disk like any clean buffer.
    fn settle_undone_conflicts(&mut self) {
        let settled: Vec<_> = self
            .projects
            .iter()
            .flat_map(|project| &project.tabs)
            .filter(|tab| {
                tab.disk_conflict.is_some()
                    && !tab.saving
                    && !tab.loading
                    && !has_unsaved_edits(tab)
            })
            .map(|tab| tab.id)
            .collect();
        for id in settled {
            if let Some(conflict) = self
                .tab_by_id_mut(id)
                .and_then(|tab| tab.disk_conflict.take())
            {
                self.reload_from_disk(id, conflict.disk);
            }
        }
    }

    /// Replace the buffer with a disk version as one step in both the native
    /// and the Vim history. Every view keeps its place in the logical text.
    /// Returns the number of changed lines when it was measured.
    fn replace_with_disk_version(
        &mut self,
        id: u64,
        disk: workspace::LoadedDocument,
    ) -> Option<Option<usize>> {
        let (project, index) = self.tab_location(id)?;
        let tab = &mut self.projects[project].tabs[index];
        let navigation = tab.vim.navigation();
        if tab.vim.finish_insert(&mut tab.text).text_changed {
            tab.changed();
        }
        self.context_native_undo_checkpoint(id);
        let row_height = self.settings.font_size * self.settings.line_height;
        let tab = &mut self.projects[project].tabs[index];
        let map = TextMap::new(&tab.text, &disk.text);
        tab.vim.begin_external_edit(&tab.text);
        let selection = map.range(tab.selection.clone());
        let pending = tab.pending_selection.clone().map(|range| map.range(range));
        tab.text = disk.text;
        tab.saved_text.clone_from(&tab.text);
        tab.fingerprint = Some(disk.fingerprint);
        tab.line_ending = disk.line_ending;
        tab.bom = disk.bom;
        tab.revision = tab.revision.saturating_add(1);
        tab.dirty = false;
        tab.clear_agent_draft_when_saved();
        tab.error = None;
        tab.disk_conflict = None;
        tab.lines.invalidate();
        tab.indentation_cache.invalidate();
        // Insert mode resumes as a new insert session on the new text.
        tab.vim.restore_navigation(
            &tab.text,
            &navigation.mapped(|position| map.position(position)),
        );
        tab.selection = selection.clone();
        tab.pending_selection = Some(pending.unwrap_or(selection));
        tab.scroll_to_cursor = false;
        let changed = map.changed_lines();
        tab.disk_reload = Some(DiskReload {
            revision: tab.revision,
            changed_lines: changed,
            at: Instant::now(),
        });
        self.find.invalidate(id);
        self.remap_document_views(id, &|position| map.position(position), &|offset| {
            map.scroll(offset, row_height)
        });
        self.session_dirty = true;
        self.ctx.request_repaint();
        Some(changed)
    }

    pub(super) fn resolve_disk_conflict(&mut self, choice: ConflictChoice) {
        match choice.action {
            ConflictAction::Compare => self.compare_disk_conflict(choice.tab_id),
            ConflictAction::UseDisk => self.use_disk_version(choice.tab_id),
            ConflictAction::KeepMine => self.keep_my_version(choice.tab_id),
            ConflictAction::SaveAs => self.request_save_as(choice.tab_id),
        }
    }

    /// Replace a conflicted buffer with the disk version. Undo restores the edits.
    pub(super) fn use_disk_version(&mut self, id: u64) {
        let Some(tab) = self.tab_by_id_mut(id) else {
            return;
        };
        if tab.saving || tab.loading || tab.disk_conflict.is_none() {
            return;
        }
        let title = tab.title.clone();
        let Some(conflict) = tab.disk_conflict.take() else {
            return;
        };
        if self.replace_with_disk_version(id, conflict.disk).is_some() {
            let undo = self.undo_hint();
            self.status(
                format!("Using the disk version of {title} · {undo} restores your edits"),
                false,
            );
        }
    }

    /// Adopt the observed disk version as the comparison baseline, so the next
    /// save may replace it. The edits stay unsaved.
    pub(super) fn keep_my_version(&mut self, id: u64) {
        let autosave = self.settings.autosave;
        let save = self.shortcut_label(keybindings::Action::Save);
        let Some(tab) = self.tab_by_id_mut(id) else {
            return;
        };
        if tab.saving || tab.loading {
            return;
        }
        let Some(conflict) = tab.disk_conflict.take() else {
            return;
        };
        tab.fingerprint = Some(conflict.disk.fingerprint);
        tab.saved_text = conflict.disk.text;
        tab.dirty = tab.text != tab.saved_text;
        tab.clear_agent_draft_when_saved();
        tab.error = None;
        // Recovery records the adopted baseline from now on; an older snapshot
        // must not count as securing the edits against it.
        tab.recovery_generation = tab.recovery_generation.wrapping_add(1);
        tab.recovery_revision = 0;
        let title = tab.title.clone();
        let message = if !tab.dirty {
            format!("{title} matches the disk version")
        } else if autosave && !tab.agent_unsaved && tab.path.is_some() {
            format!("Keeping your version of {title}; auto save replaces the disk version")
        } else if save.is_empty() {
            format!("Keeping your version of {title}; saving replaces the disk version")
        } else {
            format!("Keeping your version of {title}; {save} replaces the disk version")
        };
        self.status(message, false);
        self.session_dirty = true;
    }

    pub(super) fn compare_disk_conflict(&mut self, id: u64) {
        let Some((project, index)) = self.tab_location(id) else {
            return;
        };
        let tab = &self.projects[project].tabs[index];
        let Some(conflict) = &tab.disk_conflict else {
            return;
        };
        let comparison = Comparison {
            tab_id: id,
            kind: ComparisonKind::DiskConflict,
        };
        let title = format!("{} · disk version → your edits", tab.title);
        let label = self.comparison_label(project, index);
        let (old, new) = (conflict.disk.text.clone(), tab.text.clone());
        self.show_comparison(comparison, title, label, old, new);
    }

    pub(super) fn compare_with_saved(&mut self, id: u64) {
        let Some((project, index)) = self.tab_location(id) else {
            return;
        };
        let tab = &self.projects[project].tabs[index];
        if tab.loading {
            self.status("Wait until the file has loaded", false);
            return;
        }
        if tab.path.is_none() {
            self.status(
                "This new file has no saved version yet. Save it once to compare later changes.",
                false,
            );
            return;
        }
        let comparison = Comparison {
            tab_id: id,
            kind: ComparisonKind::Saved,
        };
        let title = format!("{} · saved → current buffer", tab.title);
        let label = self.comparison_label(project, index);
        let (old, new) = (tab.saved_text.clone(), tab.text.clone());
        self.show_comparison(comparison, title, label, old, new);
    }

    pub(super) fn compare_current_with_saved(&mut self) {
        match self.tab().map(|tab| tab.id) {
            Some(id) => self.compare_with_saved(id),
            None => self.status("Open a file to compare it with its saved version", false),
        }
    }

    fn comparison_label(&self, project: usize, index: usize) -> String {
        let project = &self.projects[project];
        let tab = &project.tabs[index];
        tab.path
            .as_deref()
            .map(|path| crate::paths::display(path.strip_prefix(&project.root).unwrap_or(path)))
            .unwrap_or_else(|| tab.title.clone())
    }

    /// Diff on a worker and show the result in the same dialog as Git diffs.
    fn show_comparison(
        &mut self,
        comparison: Comparison,
        title: String,
        label: String,
        old: String,
        new: String,
    ) {
        self.palette = None;
        self.begin_git_diff(title.clone());
        self.comparison = Some(comparison);
        let generation = self.diff_generation;
        self.worker(move || WorkerResult::Diff {
            generation,
            title,
            result: Ok(line_diff::unified_patch(&old, &new, &label, 3)),
        });
    }

    /// Run a Save As whose destination was confirmed during another save.
    pub(super) fn resume_deferred_save_as(&mut self) {
        if self.deferred_save_as.is_empty() {
            return;
        }
        let mut ready = Vec::new();
        let mut waiting = Vec::new();
        for (id, path, overwrite) in std::mem::take(&mut self.deferred_save_as) {
            match self
                .projects
                .iter()
                .flat_map(|project| &project.tabs)
                .find(|tab| tab.id == id)
            {
                Some(tab) if tab.saving || tab.loading => waiting.push((id, path, overwrite)),
                Some(_) => ready.push((id, path, overwrite)),
                None => {}
            }
        }
        self.deferred_save_as = waiting;
        for (id, path, overwrite) in ready {
            self.save_tab_expected(id, Some(path), overwrite);
        }
    }

    /// Shown above the editor while the active document has a disk conflict.
    pub(super) fn disk_conflict_banner(&mut self, ui: &mut egui::Ui) {
        let Some((id, title, agent_draft)) = self
            .tab()
            .filter(|tab| tab.disk_conflict.is_some())
            .map(|tab| (tab.id, tab.title.clone(), tab.agent_unsaved))
        else {
            return;
        };
        let undo = self.undo_hint();
        let autosave = self.settings.autosave && !agent_draft;
        let mut action = None;
        egui::Frame::new()
            .fill(PANEL.lerp_to_gamma(ORANGE, 0.10))
            .inner_margin(Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    ui.label(
                        RichText::new(format!("{title} changed on disk."))
                            .size(12.0)
                            .strong()
                            .color(ORANGE),
                    );
                    ui.label(
                        RichText::new(if agent_draft {
                            "The agent draft is kept; saving waits for your choice."
                        } else {
                            "Your edits are kept; saving waits for your choice."
                        })
                        .size(12.0)
                        .color(TEXT),
                    );
                    ui.add_space(4.0);
                    for (label, tooltip, choice) in [
                        (
                            "Compare",
                            "Show the disk version and your edits as a diff".to_owned(),
                            ConflictAction::Compare,
                        ),
                        (
                            "Use disk version",
                            format!("Replace the buffer with the disk version. {undo} restores your edits."),
                            ConflictAction::UseDisk,
                        ),
                        (
                            "Keep my version",
                            if autosave {
                                "Keep your edits; auto save then replaces the disk version".to_owned()
                            } else {
                                "Keep your edits; the next save replaces the disk version".to_owned()
                            },
                            ConflictAction::KeepMine,
                        ),
                        (
                            "Save as…",
                            "Save your edits as another file; the disk version stays unchanged"
                                .to_owned(),
                            ConflictAction::SaveAs,
                        ),
                    ] {
                        if ui.button(label).on_hover_text(tooltip).clicked() {
                            action = Some(choice);
                        }
                    }
                });
            });
        if let Some(action) = action {
            self.resolve_disk_conflict(ConflictChoice { tab_id: id, action });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_follow_their_lines_through_insertions_and_deletions() {
        let old = "one\ntwo\nthree\nfour\n";
        let new = "zero\none\nthree\nfour!\n";
        let map = TextMap::new(old, new);
        // "two" at line 1 was removed: its position moves to the next kept line.
        assert_eq!(map.line(0), 1);
        assert_eq!(map.line(1), 2);
        assert_eq!(map.line(2), 2);
        // Column 2 of "three" stays on "three".
        assert_eq!(map.position(10), 11);
        // A modified line keeps the column, clamped to its new length.
        assert_eq!(
            map.position(old.find("four").unwrap() + 3),
            new.find("four!").unwrap() + 3
        );
        // The empty position after the final line break stays at the end.
        assert_eq!(map.position(old.chars().count()), new.chars().count());
        assert_eq!(map.changed_lines(), Some(3));
    }

    #[test]
    fn positions_use_unicode_scalars_and_ignore_carriage_returns() {
        let old = "ä🙂\r\nzwei\r\n";
        let new = "neu\nä🙂\nzwei\n";
        let map = TextMap::new(old, new);
        assert_eq!(map.changed_lines(), Some(1));
        // "w" in "zwei" (scalar 5 in old) stays on "zwei".
        assert_eq!(map.position(5), 8);
        assert_eq!(map.range(0..2), 4..6);
    }

    #[test]
    fn scroll_offsets_keep_the_first_visible_line() {
        let old: String = (0..40).map(|line| format!("line {line}\n")).collect();
        let new = format!("{}{old}", "inserted\n".repeat(5));
        let map = TextMap::new(&old, &new);
        assert_eq!(map.scroll(200.0, 20.0), 300.0);
        assert_eq!(map.scroll(0.0, 20.0), 0.0);
    }
}

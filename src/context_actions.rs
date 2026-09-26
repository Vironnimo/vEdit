//! Context menus and reversible filesystem actions.
use super::*;
use std::fs::OpenOptions;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EditorAction {
    Cut,
    Copy,
    Paste,
    SelectAll,
    Undo,
    Redo,
    Find,
    CommandPalette,
}

/// Collect the user's choice; apply it only after the editor releases its borrow.
pub(super) fn editor_menu(
    _ui: &mut egui::Ui,
    response: &egui::Response,
    can_edit: bool,
    can_copy: bool,
) -> Option<EditorAction> {
    let mut action = None;
    response.context_menu(|ui| {
        for (label, shortcut, tooltip, enabled, command) in [
            (
                "Cut",
                "Ctrl+X",
                "Cut the selected text",
                can_edit && can_copy,
                EditorAction::Cut,
            ),
            (
                "Copy",
                "Ctrl+C",
                "Copy the selected text",
                can_copy,
                EditorAction::Copy,
            ),
            (
                "Paste",
                "Ctrl+V",
                "Insert text from the Windows clipboard",
                can_edit,
                EditorAction::Paste,
            ),
        ] {
            if ui
                .add_enabled_ui(enabled, |ui| {
                    design::menu_item(ui, label, shortcut, tooltip)
                })
                .inner
                .clicked()
            {
                action = Some(command);
                ui.close();
            }
        }
        ui.separator();
        for (label, shortcut, tooltip, command) in [
            (
                "Select all",
                "Ctrl+A",
                "Select the entire document",
                EditorAction::SelectAll,
            ),
            (
                "Undo",
                "u / Ctrl+Z",
                "Undo the most recent edit",
                EditorAction::Undo,
            ),
            (
                "Redo",
                "Ctrl+R / Ctrl+Y",
                "Restore an undone edit",
                EditorAction::Redo,
            ),
        ] {
            if ui
                .add_enabled_ui(can_edit || command == EditorAction::SelectAll, |ui| {
                    design::menu_item(ui, label, shortcut, tooltip)
                })
                .inner
                .clicked()
            {
                action = Some(command);
                ui.close();
            }
        }
        ui.separator();
        if design::menu_item(
            ui,
            "Find in file",
            "Ctrl+F in Insert",
            "Find literal text. Vim Normal: / searches and Ctrl+F scrolls a page. Insert: Ctrl+F opens Find.",
        ).clicked() {
            action = Some(EditorAction::Find);
            ui.close();
        }
        if design::menu_item(
            ui,
            "Command palette",
            "Ctrl+Shift+P",
            "Show editing, navigation and workspace commands",
        )
        .clicked()
        {
            action = Some(EditorAction::CommandPalette);
            ui.close();
        }
    });
    action
}

pub(super) struct PendingPaste {
    id: u64,
    revision: u64,
    selection: Range<usize>,
    requested: Instant,
}

fn native_history_id(document: u64) -> egui::Id {
    egui::Id::new(("document_native_history", document))
}

/// A disk reload starts a new content history, including every split's shared undoer.
pub(super) fn reset_native_history(ctx: &egui::Context, document: u64) {
    if let Some(mut state) = egui::TextEdit::load_state(ctx, native_history_id(document)) {
        state.clear_undoer();
        state.store(ctx, native_history_id(document));
    }
}

fn native_history_state(
    ctx: &egui::Context,
    document: u64,
    widget: egui::Id,
) -> egui::text_edit::TextEditState {
    egui::TextEdit::load_state(ctx, native_history_id(document))
        .or_else(|| egui::TextEdit::load_state(ctx, widget))
        .unwrap_or_default()
}

/// TextEditState clones share the undoer's Arc. Bind only that document history
/// to the active widget while retaining its pane's independent text cursor.
pub(super) fn bind_native_history(
    ctx: &egui::Context,
    document: u64,
    widget: egui::Id,
    text: &str,
) {
    let local = egui::TextEdit::load_state(ctx, widget).unwrap_or_default();
    let mut shared = native_history_state(ctx, document, widget);
    let owner_id = native_history_id(document).with("owner");
    let owner = ctx.data(|data| data.get_temp::<egui::Id>(owner_id));
    if owner.is_some_and(|owner| owner != widget)
        || local.cursor.char_range() != shared.cursor.char_range()
    {
        // Switching panes ends a native typing group before another pane edits.
        // Match the incoming pane's cursor without clearing pending Redo: egui
        // otherwise interprets a different local range as a document change.
        let range = local
            .cursor
            .char_range()
            .or_else(|| shared.cursor.char_range())
            .unwrap_or_default();
        let mut undoer = shared.undoer();
        undoer.add_undo(&(range, text.to_owned()));
        shared.set_undoer(undoer);
        shared.cursor.set_char_range(Some(range));
    }
    shared.clone().store(ctx, native_history_id(document));
    shared.cursor = local.cursor;
    shared.store(ctx, widget);
    ctx.data_mut(|data| data.insert_temp(owner_id, widget));
}

pub(super) fn publish_native_history(
    ctx: &egui::Context,
    document: u64,
    state: &egui::text_edit::TextEditState,
    text_changed: bool,
) {
    let mut shared = state.clone();
    if !text_changed
        && let Some(previous) = egui::TextEdit::load_state(ctx, native_history_id(document))
    {
        // Cursor movement in another pane is not a document edit. Keep the
        // last edited range as the document history's current snapshot.
        shared.cursor = previous.cursor;
    }
    shared.store(ctx, native_history_id(document));
}

pub(super) fn native_history_checkpoint(
    ctx: &egui::Context,
    document: u64,
    widget: egui::Id,
    text: &str,
    selection: Range<usize>,
) {
    let range = egui::text::CCursorRange::two(
        egui::text::CCursor::new(selection.start),
        egui::text::CCursor::new(selection.end),
    );
    let mut state = native_history_state(ctx, document, widget);
    let mut undoer = state.undoer();
    undoer.add_undo(&(range, text.to_owned()));
    state.set_undoer(undoer);
    state.cursor.set_char_range(Some(range));
    state.store(ctx, native_history_id(document));
}

type NativeSnapshot = (egui::text::CCursorRange, String);
type NativeUndoer = egui::util::undoer::Undoer<NativeSnapshot>;

fn native_history_step(
    undoer: &mut NativeUndoer,
    current: &NativeSnapshot,
    redo: bool,
) -> Option<NativeSnapshot> {
    let mut observed = current.clone();
    loop {
        let restored = if redo {
            undoer.redo(&observed)
        } else {
            undoer.undo(&observed)
        }?
        .clone();
        if restored.1 != current.1 {
            return Some(restored);
        }
        // egui includes cursor ranges in snapshots. Cursor-only snapshots
        // must not consume an Undo/Redo action when changing editor panes.
        observed = restored;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FileOperation {
    Rename,
    NewFile,
    NewFolder,
}

fn rename_name_selection(name: &str, is_dir: bool) -> Range<usize> {
    let selected = if is_dir {
        name
    } else {
        Path::new(name)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or(name)
    };
    0..selected.chars().count()
}

pub(super) struct FileActionDialog {
    operation: FileOperation,
    path: PathBuf,
    name: String,
    error: Option<String>,
    focus: bool,
    busy: bool,
    name_selection: Option<Range<usize>>,
    return_to_explorer: bool,
}

pub(super) struct FileActionResult {
    job_id: Option<u64>,
    operation: FileOperation,
    old_path: PathBuf,
    new_path: PathBuf,
    result: Result<(), String>,
}

#[derive(Default)]
pub(super) struct FileJobRegistry {
    entries: std::collections::VecDeque<FileJob>,
}

struct FileJob {
    id: u64,
    operation: FileOperation,
    source: PathBuf,
    destination: PathBuf,
    phase: FileJobPhase,
    error: Option<String>,
    revisions: Vec<(u64, u64)>,
    updated_tabs: Vec<u64>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FileJobPhase {
    Preparing,
    Applying,
    Applied,
    Failed,
}

impl FileJob {
    fn pending(&self) -> bool {
        matches!(self.phase, FileJobPhase::Preparing | FileJobPhase::Applying)
    }
    fn value(&self) -> serde_json::Value {
        serde_json::json!({
            "job_id": self.id,
            "action": if self.operation == FileOperation::Rename { "rename" } else { "create_folder" },
            "status": match self.phase { FileJobPhase::Preparing | FileJobPhase::Applying => "pending", FileJobPhase::Applied => "applied", FileJobPhase::Failed => "failed" },
            "phase": match self.phase { FileJobPhase::Preparing => "preparing", FileJobPhase::Applying => "applying", _ => "complete" },
            "complete": !self.pending(), "applied": self.phase == FileJobPhase::Applied,
            "path": crate::paths::display(&self.source), "destination": crate::paths::display(&self.destination),
            "error": self.error, "updated_tabs": self.updated_tabs,
        })
    }
}

pub(super) struct FilePreparationResult {
    id: u64,
    result: Result<(PathBuf, PathBuf), String>,
}

#[derive(Clone, Copy)]
pub(super) enum TabAction {
    Close,
    CloseOthers,
    CloseSaved,
    Save,
    SaveAs,
    SplitVertical,
    SplitHorizontal,
    CopyPath,
    CopyRelativePath,
    Reveal,
}

pub(super) fn tab_menu(
    response: &egui::Response,
    has_path: bool,
    can_save: bool,
) -> Option<TabAction> {
    let mut action = None;
    response.context_menu(|ui| {
        for (label, shortcut, tooltip, enabled, command) in [
            (
                "Close",
                "Ctrl+W",
                "Close this tab; unsaved changes ask for a decision",
                true,
                TabAction::Close,
            ),
            (
                "Close other saved tabs",
                "",
                "Keep this tab and every document with unsaved changes",
                true,
                TabAction::CloseOthers,
            ),
            (
                "Close saved tabs",
                "",
                "Close documents that have no unsaved changes",
                true,
                TabAction::CloseSaved,
            ),
            (
                "Save",
                "Ctrl+S",
                "Save this document",
                can_save,
                TabAction::Save,
            ),
            (
                "Save as…",
                "",
                "Choose a new location with the Windows file dialog",
                can_save,
                TabAction::SaveAs,
            ),
        ] {
            if ui
                .add_enabled_ui(enabled, |ui| {
                    design::menu_item(ui, label, shortcut, tooltip)
                })
                .inner
                .clicked()
            {
                action = Some(command);
                ui.close();
            }
        }
        ui.separator();
        for (label, tooltip, command) in [
            (
                "Split right",
                "Show this document in a pane beside the current one",
                TabAction::SplitVertical,
            ),
            (
                "Split below",
                "Show this document in a pane below the current one",
                TabAction::SplitHorizontal,
            ),
        ] {
            if design::menu_item(ui, label, "", tooltip).clicked() {
                action = Some(command);
                ui.close();
            }
        }
        ui.separator();
        for (label, tooltip, command) in [
            (
                "Copy path",
                "Copy the full Windows path",
                TabAction::CopyPath,
            ),
            (
                "Copy relative path",
                "Copy the path relative to the project folder",
                TabAction::CopyRelativePath,
            ),
            (
                "Reveal in Explorer",
                "Select this file in Windows Explorer",
                TabAction::Reveal,
            ),
        ] {
            if ui
                .add_enabled_ui(has_path, |ui| design::menu_item(ui, label, "", tooltip))
                .inner
                .clicked()
            {
                action = Some(command);
                ui.close();
            }
        }
    });
    action
}

impl MorrowApp {
    pub(super) fn control_file_action(
        &mut self,
        action: crate::cli::FileAction,
    ) -> crate::cli::Response {
        use crate::cli::{FileAction, Response};
        let (operation, source, name) = match action {
            FileAction::Rename { path, name } => (FileOperation::Rename, path, name),
            FileAction::CreateFolder { parent, name } => (FileOperation::NewFolder, parent, name),
        };
        if !source.is_absolute() {
            return Response::failure(
                "invalid_path",
                "Filesystem actions require an absolute workspace path",
            );
        }
        if source
            .as_os_str()
            .to_string_lossy()
            .encode_utf16()
            .take(32768)
            .count()
            > 32767
        {
            return Response::failure(
                "invalid_path",
                "Windows paths cannot exceed 32767 UTF-16 code units",
            );
        }
        if let Err(error) = validate_name(&name) {
            return Response::failure("invalid_name", error);
        }
        if self
            .context_file_jobs
            .entries
            .iter()
            .filter(|job| job.pending())
            .count()
            >= 4
        {
            return Response::failure(
                "file_operations_busy",
                "Four filesystem operations are pending. Inspect their job status before submitting more.",
            );
        }
        let destination = if operation == FileOperation::Rename {
            source.parent().unwrap_or(&source).join(&name)
        } else {
            source.join(&name)
        };
        if self
            .context_file_jobs
            .entries
            .iter()
            .filter(|job| job.pending())
            .any(|job| file_jobs_overlap(job, operation, &source, &destination))
        {
            return Response::failure(
                "file_operation_conflict",
                "A pending filesystem operation affects this path. Wait for its job result first.",
            );
        }
        if operation == FileOperation::Rename
            && self
                .projects
                .iter()
                .flat_map(|project| &project.tabs)
                .any(|tab| {
                    tab.path
                        .as_ref()
                        .is_some_and(|path| path_contains(&source, path))
                        && (tab.loading || tab.saving)
                })
        {
            return Response::failure(
                "document_busy",
                "An affected document is loading or saving. Wait before renaming it.",
            );
        }
        while self.context_file_jobs.entries.len() >= 32 {
            let Some(index) = self
                .context_file_jobs
                .entries
                .iter()
                .position(|job| !job.pending())
            else {
                break;
            };
            self.context_file_jobs.entries.remove(index);
        }
        static NEXT_JOB: AtomicU64 = AtomicU64::new(1);
        let id = (u64::from(std::process::id()) << 32) | NEXT_JOB.fetch_add(1, Ordering::Relaxed);
        let revisions = if operation == FileOperation::Rename {
            self.projects
                .iter()
                .flat_map(|project| &project.tabs)
                .filter(|tab| {
                    tab.path
                        .as_ref()
                        .is_some_and(|path| path_contains(&source, path))
                })
                .map(|tab| (tab.id, tab.revision))
                .collect()
        } else {
            Vec::new()
        };
        self.context_file_jobs.entries.push_back(FileJob {
            id,
            operation,
            source: source.clone(),
            destination,
            phase: FileJobPhase::Preparing,
            error: None,
            revisions,
            updated_tabs: Vec::new(),
        });
        self.worker(move || {
            let result = prepare_file_operation(operation, source, name);
            WorkerResult::ContextFilePrepared(FilePreparationResult { id, result })
        });
        Response::success(
            serde_json::json!({"accepted":true,"pending":true,"applied":false,"job_id":id,"status":"pending","phase":"preparing"}),
        )
    }

    pub(super) fn control_file_job(&self, job_id: u64) -> crate::cli::Response {
        self.context_file_jobs.entries.iter().find(|job| job.id == job_id).map_or_else(
            || crate::cli::Response::failure("unknown_file_job", "Filesystem job is unknown or expired. Only the latest 32 jobs in this running app are retained."),
            |job| crate::cli::Response::success(job.value()),
        )
    }

    pub(super) fn context_file_jobs_status(&self) -> serde_json::Value {
        serde_json::Value::Array(
            self.context_file_jobs
                .entries
                .iter()
                .map(|job| {
                    let mut value = job.value();
                    for field in ["path", "destination", "error"] {
                        if let Some(text) = value[field].as_str()
                            && text.chars().count() > 384
                        {
                            value[field] =
                                serde_json::Value::String(text.chars().take(384).collect());
                            value["details_truncated"] = serde_json::Value::Bool(true);
                        }
                    }
                    if job.updated_tabs.len() > 32 {
                        value["updated_tabs"] = serde_json::json!(&job.updated_tabs[..32]);
                        value["updated_tab_count"] = serde_json::json!(job.updated_tabs.len());
                        value["details_truncated"] = serde_json::Value::Bool(true);
                    }
                    value
                })
                .collect(),
        )
    }

    pub(super) fn context_file_prepared(&mut self, prepared: FilePreparationResult) {
        let Some(index) = self
            .context_file_jobs
            .entries
            .iter()
            .position(|job| job.id == prepared.id)
        else {
            return;
        };
        let (source, destination) = match prepared.result {
            Ok(paths) => paths,
            Err(error) => {
                self.context_fail_file_job(prepared.id, error);
                return;
            }
        };
        let operation = self.context_file_jobs.entries[index].operation;
        let failure = if !self.projects.iter().any(|project| {
            source.starts_with(&project.root) && destination.starts_with(&project.root)
        }) {
            Some(
                "Filesystem actions must stay inside a registered workspace's real folder"
                    .to_owned(),
            )
        } else if operation == FileOperation::Rename
            && self
                .projects
                .iter()
                .any(|project| project.root.starts_with(&source))
        {
            Some("A workspace folder or a folder containing a registered workspace cannot be renamed while it is open".to_owned())
        } else if self
            .context_file_jobs
            .entries
            .iter()
            .filter(|job| job.id != prepared.id && job.pending())
            .any(|job| file_jobs_overlap(job, operation, &source, &destination))
        {
            Some("Another pending operation affects the resolved path".to_owned())
        } else if operation == FileOperation::Rename
            && self
                .projects
                .iter()
                .flat_map(|project| &project.tabs)
                .any(|tab| {
                    tab.path
                        .as_ref()
                        .is_some_and(|path| path.starts_with(&source))
                        && (tab.loading || tab.saving)
                })
        {
            Some(
                "An affected document started loading or saving; no filesystem change was applied"
                    .to_owned(),
            )
        } else if self.context_file_jobs.entries[index]
            .revisions
            .iter()
            .any(|(id, revision)| {
                self.projects
                    .iter()
                    .flat_map(|project| &project.tabs)
                    .any(|tab| tab.id == *id && tab.revision != *revision)
            })
        {
            Some("An affected buffer changed while the operation was being prepared; no filesystem change was applied".to_owned())
        } else if self.context_file_dialog.as_ref().is_some_and(|dialog| {
            dialog.busy
                && (path_contains(&dialog.path, &source) || path_contains(&source, &dialog.path))
        }) {
            Some("A user filesystem operation affects this path".to_owned())
        } else {
            None
        };
        if let Some(error) = failure {
            self.context_fail_file_job(prepared.id, error);
            return;
        }
        let job = &mut self.context_file_jobs.entries[index];
        job.source = source.clone();
        job.destination = destination.clone();
        job.phase = FileJobPhase::Applying;
        self.worker(move || {
            let result = if operation == FileOperation::Rename {
                rename_without_overwrite(&source, &destination)
            } else {
                fs::create_dir(&destination).map_err(|error| error.to_string())
            };
            WorkerResult::ContextFile(FileActionResult {
                job_id: Some(prepared.id),
                operation,
                old_path: source,
                new_path: destination,
                result,
            })
        });
    }

    fn context_fail_file_job(&mut self, id: u64, error: String) {
        if let Some(job) = self
            .context_file_jobs
            .entries
            .iter_mut()
            .find(|job| job.id == id)
        {
            job.phase = FileJobPhase::Failed;
            job.error = Some(error.clone());
        }
        self.status(error, true);
    }

    pub(super) fn apply_editor_context_action(&mut self, id: u64, action: EditorAction) {
        match action {
            EditorAction::Find => {
                self.find_visible = true;
                self.find_focus = true;
            }
            EditorAction::CommandPalette => self.open_palette(Palette::Commands),
            EditorAction::Paste => {
                let widget = self.document_widget_id(id);
                let vim = self.settings.vim;
                let Some(tab) = self.tab_by_id_mut(id) else {
                    return;
                };
                if tab.loading {
                    return;
                }
                tab.focus = true;
                let pending = PendingPaste {
                    id,
                    revision: tab.revision,
                    selection: if vim {
                        tab.vim
                            .selection(&tab.text)
                            .unwrap_or_else(|| tab.selection.clone())
                    } else {
                        tab.selection.clone()
                    },
                    requested: Instant::now(),
                };
                self.context_paste = Some(pending);
                self.ctx.memory_mut(|memory| memory.request_focus(widget));
                self.ctx
                    .send_viewport_cmd(egui::ViewportCommand::RequestPaste);
            }
            EditorAction::Copy | EditorAction::Cut => {
                let vim = self.settings.vim;
                if action == EditorAction::Cut && vim {
                    let Some(tab) = self.tab_by_id_mut(id) else {
                        return;
                    };
                    if tab.loading {
                        return;
                    }
                    if matches!(tab.vim.mode, VimMode::Visual | VimMode::VisualLine) {
                        let (result, copied) = tab.vim.cut_visual_to_clipboard(&mut tab.text);
                        if result.text_changed {
                            tab.changed();
                        }
                        tab.selection = tab
                            .vim
                            .selection(&tab.text)
                            .unwrap_or(tab.vim.cursor..tab.vim.cursor);
                        tab.pending_selection = Some(tab.selection.clone());
                        tab.focus = true;
                        tab.scroll_to_cursor = true;
                        if let Some(copied) = copied {
                            self.ctx.copy_text(copied);
                        }
                        return;
                    }
                }
                if action == EditorAction::Cut {
                    self.context_native_undo_checkpoint(id);
                }
                let Some(tab) = self.tab_by_id_mut(id) else {
                    return;
                };
                if action == EditorAction::Cut && tab.loading {
                    return;
                }
                let selection = if vim {
                    tab.vim
                        .selection(&tab.text)
                        .unwrap_or_else(|| tab.selection.clone())
                } else {
                    tab.selection.clone()
                };
                let cursor = if vim {
                    tab.vim.cursor
                } else {
                    tab.selection.start
                };
                let linewise = selection.is_empty() || vim && tab.vim.mode == VimMode::VisualLine;
                let selection = crate::vim::clipboard_range(&tab.text, selection, cursor);
                if selection.is_empty() {
                    return;
                }
                let copied: String = tab
                    .text
                    .chars()
                    .skip(selection.start)
                    .take(selection.len())
                    .collect();
                if action == EditorAction::Cut {
                    let edit = editor::replace_selection(&tab.text, selection, "");
                    let cursor = edit.selection.start;
                    tab.apply(edit);
                    tab.set_cursor(cursor..cursor);
                    tab.vim.notify_external_text_change();
                } else {
                    if linewise {
                        tab.flash_line_yank(vec![selection]);
                    } else {
                        tab.flash_yank(vec![selection]);
                    }
                    tab.vim.cancel_pending_command();
                }
                self.ctx.copy_text(copied);
            }
            EditorAction::SelectAll => {
                let vim = self.settings.vim;
                let Some(tab) = self.tab_by_id_mut(id) else {
                    return;
                };
                let length = tab.text.chars().count();
                if vim {
                    let insert = tab.vim.finish_insert(&mut tab.text);
                    if insert.text_changed {
                        tab.changed();
                    }
                    tab.vim.set_selection_boundaries(&tab.text, 0, length);
                    tab.selection = tab.vim.selection(&tab.text).unwrap_or(0..length);
                } else {
                    tab.selection = 0..length;
                }
                tab.pending_selection = Some(tab.selection.clone());
                tab.focus = true;
            }
            EditorAction::Undo | EditorAction::Redo => {
                if !self.settings.vim {
                    self.context_native_undo(id, action == EditorAction::Redo);
                    return;
                }
                let Some(tab) = self.tab_by_id_mut(id) else {
                    return;
                };
                if tab.loading {
                    return;
                }
                let insert = tab.vim.finish_insert(&mut tab.text);
                if insert.text_changed {
                    tab.changed();
                }
                let result = tab.vim.handle_key(
                    &mut tab.text,
                    if action == EditorAction::Undo {
                        VimKey::Char('u')
                    } else {
                        VimKey::Redo
                    },
                );
                if result.text_changed {
                    tab.changed();
                }
                tab.selection = tab.vim.cursor..tab.vim.cursor;
                tab.pending_selection = Some(tab.selection.clone());
                tab.focus = true;
                tab.scroll_to_cursor = true;
            }
        }
    }

    pub(super) fn context_native_undo_checkpoint(&mut self, id: u64) {
        if self.settings.vim {
            return;
        }
        let widget = self.document_widget_id(id);
        let Some(tab) = self
            .projects
            .iter()
            .flat_map(|project| &project.tabs)
            .find(|tab| tab.id == id)
        else {
            return;
        };
        native_history_checkpoint(&self.ctx, id, widget, &tab.text, tab.selection.clone());
    }

    pub(super) fn context_native_history(&self, id: u64) -> (bool, bool) {
        let widget = self.document_widget_id(id);
        let Some(tab) = self
            .projects
            .iter()
            .flat_map(|project| &project.tabs)
            .find(|tab| tab.id == id)
        else {
            return (false, false);
        };
        let state = native_history_state(&self.ctx, id, widget);
        let current = (
            state.cursor.char_range().unwrap_or_default(),
            tab.text.clone(),
        );
        let undoer = state.undoer();
        (
            native_history_step(&mut undoer.clone(), &current, false).is_some(),
            native_history_step(&mut undoer.clone(), &current, true).is_some(),
        )
    }

    pub(super) fn context_native_undo(&mut self, id: u64, redo: bool) {
        let widget = self.document_widget_id(id);
        let ctx = self.ctx.clone();
        let Some(tab) = self.tab_by_id_mut(id) else {
            return;
        };
        if tab.loading {
            return;
        }
        let mut state = native_history_state(&ctx, id, widget);
        let current = (
            state.cursor.char_range().unwrap_or_default(),
            tab.text.clone(),
        );
        let mut undoer = state.undoer();
        let restored = native_history_step(&mut undoer, &current, redo);
        state.set_undoer(undoer);
        if let Some((range, text)) = restored {
            tab.text = text;
            tab.changed();
            tab.selection =
                range.as_sorted_char_range().start.0..range.as_sorted_char_range().end.0;
            tab.pending_selection = Some(tab.selection.clone());
            tab.vim.set_cursor(&tab.text, range.primary.index.0);
            tab.vim.cursor = range.primary.index.0;
            tab.focus = true;
            tab.scroll_to_cursor = true;
            state.cursor.set_char_range(Some(range));
        }
        state.store(&ctx, native_history_id(id));
    }

    pub(super) fn context_paste_poll(&mut self) {
        let Some(pending) = self.context_paste.as_ref() else {
            return;
        };
        if pending.requested.elapsed() > Duration::from_secs(3) {
            self.context_paste = None;
            return;
        }
        let text = self.ctx.input_mut(|input| {
            let index = input
                .events
                .iter()
                .position(|event| matches!(event, egui::Event::Paste(_)))?;
            match input.events.remove(index) {
                egui::Event::Paste(text) => Some(text),
                _ => None,
            }
        });
        let Some(text) = text else { return };
        let pending = self.context_paste.take().unwrap();
        self.context_native_undo_checkpoint(pending.id);
        let Some(tab) = self.tab_by_id_mut(pending.id) else {
            return;
        };
        if tab.revision != pending.revision || tab.loading {
            self.status("Paste cancelled because the document changed", true);
            return;
        }
        let edit = editor::replace_selection(&tab.text, pending.selection, &text);
        let cursor = edit.selection.start;
        tab.apply(edit);
        if matches!(tab.vim.mode, VimMode::Visual | VimMode::VisualLine) {
            tab.vim.set_selection(&tab.text, cursor..cursor);
        }
        tab.set_cursor(cursor..cursor);
        tab.vim.notify_external_text_change();
    }

    pub(super) fn apply_tab_context_action(&mut self, id: u64, action: TabAction) {
        self.bind_current_pane();
        let pane_id = self.current_pane_id();
        let Some((project, index)) = self.projects.iter().enumerate().find_map(|(p, project)| {
            project
                .tabs
                .iter()
                .position(|tab| tab.id == id)
                .map(|index| (p, index))
        }) else {
            return;
        };
        let path = self.projects[project].tabs[index].path.clone();
        let root = self.projects[project].root.clone();
        match action {
            TabAction::Close => self.request_close(CloseAction::PaneTab { pane_id, id }),
            TabAction::CloseOthers | TabAction::CloseSaved => {
                let ids: Vec<_> = self
                    .current_pane_tab_ids()
                    .into_iter()
                    .filter(|candidate_id| {
                        self.projects
                            .iter()
                            .flat_map(|project| &project.tabs)
                            .find(|tab| tab.id == *candidate_id)
                            .is_some_and(|tab| {
                                !tab.dirty
                                    && !tab.loading
                                    && !tab.saving
                                    && (matches!(action, TabAction::CloseSaved) || tab.id != id)
                            })
                    })
                    .collect();
                for id in ids {
                    self.perform_close(CloseAction::PaneTab { pane_id, id });
                }
                self.status("Saved tabs closed; unsaved documents kept open", false);
            }
            TabAction::Save => self.save_tab(id, None),
            TabAction::SaveAs => self.request_save_as(id),
            TabAction::SplitVertical | TabAction::SplitHorizontal => {
                self.activate_tab(project, index);
                let orientation = if matches!(action, TabAction::SplitVertical) {
                    split::SplitOrientation::Vertical
                } else {
                    split::SplitOrientation::Horizontal
                };
                if let Err(error) = self.split_active(orientation) {
                    self.status(error, true);
                }
            }
            TabAction::CopyPath => {
                if let Some(path) = path {
                    self.ctx.copy_text(crate::paths::display(&path));
                }
            }
            TabAction::CopyRelativePath => {
                if let Some(path) = path {
                    self.ctx.copy_text(crate::paths::display(
                        path.strip_prefix(&root).unwrap_or(&path),
                    ));
                }
            }
            TabAction::Reveal => {
                if let Some(path) = path {
                    self.context_reveal(path);
                }
            }
        }
    }

    pub(super) fn file_context_menu(
        &mut self,
        response: &egui::Response,
        path: &Path,
        is_dir: bool,
    ) {
        let can_rename = !self.context_path_busy(path)
            && !self
                .projects
                .iter()
                .any(|project| project.root.starts_with(path))
            && !self
                .projects
                .iter()
                .flat_map(|project| &project.tabs)
                .any(|tab| {
                    tab.path.as_ref().is_some_and(|open| open.starts_with(path))
                        && (tab.loading || tab.saving)
                });
        let mut action = None;
        let mut popup = egui::Popup::context_menu(response).id(self.explorer_menu_id(path));
        if let Some(position) = self.explorer_menu_position(response, path) {
            popup = popup.at_position(position);
        }
        popup.show(|ui| {
            if !is_dir {
                if design::menu_item(ui, "Open", "", "Open this file in the current pane").clicked()
                {
                    action = Some(0);
                    ui.close();
                }
                if design::menu_item(
                    ui,
                    "Open to the side",
                    "",
                    "Open this file in a separate pane",
                )
                .clicked()
                {
                    action = Some(1);
                    ui.close();
                }
                ui.separator();
            }
            for (label, tooltip, command) in [
                ("New file…", "Create a file in this folder", 2),
                ("New folder…", "Create a folder here", 3),
            ] {
                if design::menu_item(ui, label, "", tooltip).clicked() {
                    action = Some(command);
                    ui.close();
                }
            }
            if ui
                .add_enabled_ui(can_rename, |ui| {
                    let response = design::menu_item(
                        ui,
                        "Rename…",
                        "F2",
                        "Rename without replacing another file; unsaved buffers stay open",
                    );
                    // egui concatenates the shortcut into a Button's default
                    // accessible name. Keep the command name stable while its
                    // visible F2 hint remains available alongside it.
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::Button,
                            ui.is_enabled(),
                            "Rename…",
                        )
                    });
                    response
                })
                .inner
                .clicked()
            {
                action = Some(4);
                ui.close();
            }
            ui.separator();
            for (label, tooltip, command) in [
                ("Copy path", "Copy the full Windows path", 5),
                (
                    "Copy relative path",
                    "Copy a path relative to the project folder",
                    6,
                ),
                (
                    "Reveal in Explorer",
                    "Show this item in Windows Explorer",
                    7,
                ),
            ] {
                if design::menu_item(ui, label, "", tooltip).clicked() {
                    action = Some(command);
                    ui.close();
                }
            }
        });
        match action {
            Some(0) => self.open_file(path.into()),
            Some(1) => self.open_to_side(path.into()),
            Some(2 | 3) => {
                self.context_begin_file_action(
                    if action == Some(2) {
                        FileOperation::NewFile
                    } else {
                        FileOperation::NewFolder
                    },
                    if is_dir {
                        path
                    } else {
                        path.parent().unwrap_or(path)
                    },
                );
            }
            Some(4) => {
                self.context_begin_rename(path, is_dir);
            }
            Some(5) => self.ctx.copy_text(crate::paths::display(path)),
            Some(6) => self.ctx.copy_text(crate::paths::display(
                self.project()
                    .and_then(|project| path.strip_prefix(&project.root).ok())
                    .unwrap_or(path),
            )),
            Some(7) => self.context_reveal(path.into()),
            _ => {}
        }
    }

    /// Attach the workspace menu only to a genuinely unoccupied Explorer region.
    /// The caller supplies disjoint background rectangles so file rows and the
    /// scroll bar retain their own pointer interactions.
    pub(super) fn explorer_background_menu(
        &mut self,
        ui: &mut egui::Ui,
        root: &Path,
        rect: egui::Rect,
    ) {
        let rect = rect.intersect(ui.clip_rect());
        if !rect.is_positive() {
            return;
        }
        let response = ui.interact(
            rect,
            ui.make_persistent_id((
                "explorer_background",
                root,
                rect.min.x.to_bits(),
                rect.min.y.to_bits(),
            )),
            egui::Sense::click(),
        );
        let mut action = None;
        response.context_menu(|ui| {
            for (label, tooltip, command) in [
                ("New file…", "Create a file in the project folder", 0),
                ("New folder…", "Create a folder in the project folder", 1),
            ] {
                if design::menu_item(ui, label, "", tooltip).clicked() {
                    action = Some(command);
                    ui.close();
                }
            }
            ui.separator();
            for (label, tooltip, command) in [
                (
                    "Copy project path",
                    "Copy the full Windows path of this project",
                    2,
                ),
                (
                    "Reveal in Explorer",
                    "Open the project folder in Windows Explorer",
                    3,
                ),
                ("Refresh", "Refresh the files and source control status", 4),
            ] {
                if design::menu_item(ui, label, "", tooltip).clicked() {
                    action = Some(command);
                    ui.close();
                }
            }
        });
        match action {
            Some(0) => self.context_begin_file_action(FileOperation::NewFile, root),
            Some(1) => self.context_begin_file_action(FileOperation::NewFolder, root),
            Some(2) => self.ctx.copy_text(crate::paths::display(root)),
            Some(3) => self.context_reveal(root.into()),
            Some(4) => {
                if let Some(index) = self
                    .projects
                    .iter()
                    .position(|project| project.root == root)
                {
                    self.index_project(index);
                    self.refresh_git(index, true);
                }
            }
            _ => {}
        }
    }

    fn context_begin_file_action(&mut self, operation: FileOperation, path: &Path) {
        let return_to_explorer = self.explorer_keyboard_focused();
        self.context_file_dialog = Some(FileActionDialog {
            operation,
            path: path.into(),
            name: if operation == FileOperation::Rename {
                path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            } else {
                String::new()
            },
            error: None,
            focus: true,
            busy: false,
            name_selection: None,
            return_to_explorer,
        });
    }

    pub(super) fn context_begin_rename(&mut self, path: &Path, is_dir: bool) {
        if self.context_path_busy(path)
            || self
                .projects
                .iter()
                .any(|project| project.root.starts_with(path))
            || self
                .projects
                .iter()
                .flat_map(|project| &project.tabs)
                .any(|tab| {
                    tab.path.as_ref().is_some_and(|open| open.starts_with(path))
                        && (tab.loading || tab.saving)
                })
        {
            self.status(
                "Wait for pending file operations before renaming this item",
                true,
            );
            return;
        }
        self.context_begin_file_action(FileOperation::Rename, path);
        if let Some(dialog) = self.context_file_dialog.as_mut() {
            dialog.name_selection = Some(rename_name_selection(&dialog.name, is_dir));
            // Rename is invoked from Explorer, even if a menu item temporarily
            // acquired keyboard focus before this action was dispatched.
            dialog.return_to_explorer = true;
        }
        // The name field owns the next frame's keyboard input. Explorer has
        // already consumed this frame's F2 and trailing text before opening it.
        if let Some(tab) = self.tab_mut() {
            tab.focus = false;
        }
    }

    pub(super) fn context_path_busy(&self, path: &Path) -> bool {
        self.context_file_dialog.as_ref().is_some_and(|dialog| {
            dialog.busy
                && dialog.operation == FileOperation::Rename
                && path.starts_with(&dialog.path)
        }) || self.context_file_jobs.entries.iter().any(|job| {
            job.operation == FileOperation::Rename
                && job.pending()
                && (path_contains(&job.source, path) || path_contains(&job.destination, path))
        })
    }

    pub(super) fn context_file_path_pending(&self, path: &Path) -> bool {
        self.context_path_busy(path)
    }

    pub(super) fn context_file_dialog_ui(&mut self, ctx: &egui::Context) {
        let Some(dialog) = self.context_file_dialog.as_mut() else {
            return;
        };
        let title = match dialog.operation {
            FileOperation::Rename => "Rename",
            FileOperation::NewFile => "New file",
            FileOperation::NewFolder => "New folder",
        };
        let mut submit = false;
        let mut cancel = false;
        let modal = egui::Modal::new(egui::Id::new("file_operation")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.label(RichText::new(title).size(18.0).strong());
            ui.add_space(8.0);
            ui.label(
                RichText::new(crate::paths::display(&dialog.path))
                    .size(11.0)
                    .color(MUTED),
            );
            ui.add_space(8.0);
            ui.add_enabled_ui(!dialog.busy, |ui| {
                let name_id = egui::Id::new("file_action_name");
                if dialog.focus && !ui.is_sizing_pass() {
                    let mut state =
                        egui::TextEdit::load_state(ui.ctx(), name_id).unwrap_or_default();
                    if let Some(selection) = dialog.name_selection.take() {
                        state
                            .cursor
                            .set_char_range(Some(egui::text::CCursorRange::two(
                                egui::text::CCursor::new(selection.start),
                                egui::text::CCursor::new(selection.end),
                            )));
                        state.store(ui.ctx(), name_id);
                    }
                    ui.memory_mut(|memory| memory.request_focus(name_id));
                }
                let response = ui.add(
                    egui::TextEdit::singleline(&mut dialog.name)
                        .id(name_id)
                        .hint_text("Name")
                        .desired_width(f32::INFINITY),
                );
                if dialog.focus && !ui.is_sizing_pass() {
                    response.request_focus();
                    dialog.focus = false;
                }
                if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Enter)) {
                    submit = true;
                }
                if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Escape)) {
                    cancel = true;
                }
                if let Some(error) = &dialog.error {
                    ui.add_space(6.0);
                    ui.colored_label(RED, error);
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if design::primary_button(
                        ui,
                        if dialog.operation == FileOperation::Rename {
                            "Rename"
                        } else {
                            "Create"
                        },
                        Vec2::new(95.0, 30.0),
                    )
                    .clicked()
                    {
                        submit = true;
                    }
                    if design::ghost_button(ui, "Cancel", "Leave all files unchanged").clicked() {
                        cancel = true;
                    }
                });
            });
            if dialog.busy {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Working…");
                });
            }
        });
        if (modal.should_close() || cancel) && !dialog.busy {
            let return_to_explorer = dialog.return_to_explorer;
            self.context_file_dialog = None;
            self.context_restore_file_action_focus(return_to_explorer);
            return;
        }
        if !submit || dialog.busy {
            return;
        }
        let operation = dialog.operation;
        let old_path = dialog.path.clone();
        if operation == FileOperation::Rename
            && old_path
                .file_name()
                .is_some_and(|name| name.to_string_lossy() == dialog.name)
        {
            let return_to_explorer = dialog.return_to_explorer;
            self.context_file_dialog = None;
            self.context_restore_file_action_focus(return_to_explorer);
            return;
        }
        let parent = if operation == FileOperation::Rename {
            old_path.parent().unwrap_or(&old_path)
        } else {
            &old_path
        };
        let new_path = match validated_child(parent, &dialog.name) {
            Ok(path) => path,
            Err(error) => {
                dialog.error = Some(error);
                return;
            }
        };
        if self
            .context_file_jobs
            .entries
            .iter()
            .any(|job| job.pending() && file_jobs_overlap(job, operation, &old_path, &new_path))
        {
            dialog.error = Some("A pending agent filesystem operation affects this path".into());
            return;
        }
        if operation == FileOperation::Rename
            && (self
                .projects
                .iter()
                .any(|project| project.root.starts_with(&old_path))
                || self
                    .projects
                    .iter()
                    .flat_map(|project| &project.tabs)
                    .any(|tab| {
                        tab.path
                            .as_ref()
                            .is_some_and(|path| path.starts_with(&old_path))
                            && (tab.saving || tab.loading)
                    }))
        {
            dialog.error = Some("Wait for affected files to finish loading or saving".into());
            return;
        }
        dialog.busy = true;
        dialog.error = None;
        self.worker(move || {
            let result = match operation {
                FileOperation::NewFile => OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&new_path)
                    .map(|_| ())
                    .map_err(|error| error.to_string()),
                FileOperation::NewFolder => {
                    fs::create_dir(&new_path).map_err(|error| error.to_string())
                }
                FileOperation::Rename => rename_without_overwrite(&old_path, &new_path),
            };
            WorkerResult::ContextFile(FileActionResult {
                job_id: None,
                operation,
                old_path,
                new_path,
                result,
            })
        });
    }

    pub(super) fn context_file_result(&mut self, result: FileActionResult) {
        if let Err(error) = result.result {
            if let Some(id) = result.job_id {
                self.context_fail_file_job(id, error);
                return;
            }
            if let Some(dialog) = self.context_file_dialog.as_mut() {
                dialog.busy = false;
                dialog.error = Some(error.clone());
            }
            self.status(error, true);
            return;
        }
        if result.job_id.is_none() {
            let return_to_explorer = self
                .context_file_dialog
                .as_ref()
                .is_some_and(|dialog| dialog.return_to_explorer);
            self.context_file_dialog = None;
            self.context_restore_file_action_focus(return_to_explorer);
        }
        let mut updated_tabs = Vec::new();
        if result.operation == FileOperation::Rename {
            self.explorer_renamed(&result.old_path, &result.new_path);
            self.bookmarks
                .rename_prefix(&result.old_path, &result.new_path);
            self.bookmark_lines.clear();
            for project in &mut self.projects {
                let old_open: Vec<_> = project
                    .language_opened
                    .keys()
                    .filter(|path| path.starts_with(&result.old_path))
                    .cloned()
                    .collect();
                for old_path in old_open {
                    if let Some(language) = &project.language {
                        language.close(&old_path);
                    }
                    project.language_opened.remove(&old_path);
                    project.diagnostics.remove(&old_path);
                }
                for tab in &mut project.tabs {
                    if let Some(path) = &tab.path
                        && let Ok(relative) = path.strip_prefix(&result.old_path)
                    {
                        let new_path = if relative.as_os_str().is_empty() {
                            result.new_path.clone()
                        } else {
                            result.new_path.join(relative)
                        };
                        tab.title = file_title(&new_path);
                        tab.path = Some(new_path);
                        updated_tabs.push(tab.id);
                    }
                }
            }
            self.session_dirty = true;
            self.status(
                "Renamed; open documents and unsaved changes preserved",
                false,
            );
        } else {
            self.status(
                if result.operation == FileOperation::NewFile {
                    "File created"
                } else {
                    "Folder created"
                },
                false,
            );
        }
        let affected: Vec<_> = self
            .projects
            .iter()
            .enumerate()
            .filter(|(_, project)| {
                result.new_path.starts_with(&project.root)
                    || result.old_path.starts_with(&project.root)
            })
            .map(|(index, _)| index)
            .collect();
        for index in affected {
            self.index_project(index);
            self.refresh_git(index, true);
        }
        if result.operation == FileOperation::NewFile {
            self.open_argument(result.new_path);
        }
        if let Some(id) = result.job_id
            && let Some(job) = self
                .context_file_jobs
                .entries
                .iter_mut()
                .find(|job| job.id == id)
        {
            job.phase = FileJobPhase::Applied;
            job.updated_tabs = updated_tabs;
        }
    }

    fn context_restore_editor_focus(&mut self) {
        if let Some(focused) = self.ctx.memory(|memory| memory.focused()) {
            self.ctx
                .memory_mut(|memory| memory.surrender_focus(focused));
        }
        if let Some(tab) = self.tab_mut() {
            tab.focus = tab.agent_view == crate::cli::DocumentView::Source;
        }
    }

    fn context_restore_file_action_focus(&mut self, explorer: bool) {
        if explorer {
            self.focus_explorer();
        } else {
            self.context_restore_editor_focus();
        }
    }

    fn context_reveal(&mut self, path: PathBuf) {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let display = crate::paths::display(&path);
            let mut command = std::process::Command::new("explorer.exe");
            command
                .creation_flags(0x0800_0000)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            if path.is_dir() {
                command.arg(display);
            } else {
                command.raw_arg(format!("/select,\"{display}\""));
            }
            if let Err(error) = command.spawn() {
                self.status(format!("Could not open Windows Explorer: {error}"), true);
            }
        }
        #[cfg(not(windows))]
        self.status(
            format!(
                "Reveal in Explorer is available on Windows: {}",
                crate::paths::display(&path)
            ),
            true,
        );
    }
}

fn validated_child(parent: &Path, name: &str) -> Result<PathBuf, String> {
    validate_name(name)?;
    let path = parent.join(name);
    if path.exists() {
        return Err("An item with this name already exists".into());
    }
    Ok(path)
}

fn validate_name(name: &str) -> Result<(), String> {
    if name.encode_utf16().take(256).count() > 255 {
        return Err("Windows file and folder names cannot exceed 255 UTF-16 code units".into());
    }
    if name.is_empty()
        || name.trim() != name
        || name.ends_with('.')
        || name
            .chars()
            .any(|character| character < ' ' || "<>:\"/\\|?*".contains(character))
        || matches!(name, "." | "..")
    {
        return Err("Enter one valid Windows file or folder name".into());
    }
    let base = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let port_number = base
        .strip_prefix("COM")
        .or_else(|| base.strip_prefix("LPT"));
    if matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$")
        || port_number.is_some_and(|number| {
            matches!(
                number,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
    {
        return Err("This name is reserved by Windows".into());
    }
    Ok(())
}

fn prepare_file_operation(
    operation: FileOperation,
    source: PathBuf,
    name: String,
) -> Result<(PathBuf, PathBuf), String> {
    if operation == FileOperation::Rename {
        let metadata = fs::symlink_metadata(&source).map_err(|error| error.to_string())?;
        #[cfg(windows)]
        let reparse = {
            use std::os::windows::fs::MetadataExt;
            metadata.file_attributes() & 0x400 != 0
        };
        #[cfg(not(windows))]
        let reparse = false;
        if metadata.file_type().is_symlink() || reparse {
            return Err("Use the real workspace path; renaming symlinks or reparse points through the CLI is unsupported".into());
        }
        let source = fs::canonicalize(&source).map_err(|error| error.to_string())?;
        let parent = source.parent().ok_or("A volume root cannot be renamed")?;
        let destination = validated_child(parent, &name)?;
        Ok((source, destination))
    } else {
        let parent = fs::canonicalize(&source).map_err(|error| error.to_string())?;
        if !parent.is_dir() {
            return Err("The parent must be an existing workspace folder".into());
        }
        let destination = validated_child(&parent, &name)?;
        Ok((parent, destination))
    }
}

fn path_contains(base: &Path, candidate: &Path) -> bool {
    fn key(path: &Path) -> String {
        let display = crate::paths::display(path);
        let mut normalized = PathBuf::new();
        for component in Path::new(&display).components() {
            if component == std::path::Component::ParentDir {
                normalized.pop();
            } else {
                normalized.push(component);
            }
        }
        let text = normalized.to_string_lossy().replace('/', "\\");
        #[cfg(windows)]
        let text = text.to_lowercase();
        text.trim_end_matches('\\').to_owned()
    }
    let base = key(base);
    let candidate = key(candidate);
    candidate == base
        || candidate
            .strip_prefix(&base)
            .is_some_and(|tail| tail.starts_with('\\'))
}

fn file_jobs_overlap(
    job: &FileJob,
    operation: FileOperation,
    source: &Path,
    destination: &Path,
) -> bool {
    let own = if job.operation == FileOperation::Rename {
        vec![job.source.as_path(), job.destination.as_path()]
    } else {
        vec![job.destination.as_path()]
    };
    let proposed = if operation == FileOperation::Rename {
        vec![source, destination]
    } else {
        vec![destination]
    };
    own.iter().any(|left| {
        proposed
            .iter()
            .any(|right| path_contains(left, right) || path_contains(right, left))
    })
}

fn rename_without_overwrite(old_path: &Path, new_path: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn MoveFileW(existing: *const u16, new_name: *const u16) -> i32;
        }
        let existing: Vec<u16> = old_path.as_os_str().encode_wide().chain(Some(0)).collect();
        let new_name: Vec<u16> = new_path.as_os_str().encode_wide().chain(Some(0)).collect();
        // MoveFileW fails if the destination already exists, even if another process
        // created it after the dialog's validation. No replacement flag is used.
        if unsafe { MoveFileW(existing.as_ptr(), new_name.as_ptr()) } != 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error().to_string())
        }
    }
    #[cfg(not(windows))]
    {
        if new_path.exists() {
            return Err("An item with this name already exists".into());
        }
        fs::rename(old_path, new_path).map_err(|error| error.to_string())
    }
}

#[cfg(test)]
#[path = "explorer_menu_tests.rs"]
mod explorer_menu_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn filesystem_fixture() -> (MorrowApp, PathBuf) {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir().join(format!(
            "morrow-file-jobs-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&base).unwrap();
        let root = base.join("workspace");
        fs::create_dir(&root).unwrap();
        fs::create_dir(base.join("outside")).unwrap();
        let mut app = MorrowApp::empty(egui::Context::default());
        app.settings.autosave = false;
        let mut project = Project::new(fs::canonicalize(root).unwrap());
        project.last_git_refresh = Instant::now();
        fs::write(project.root.join("source.txt"), "saved on disk").unwrap();
        let mut tab = EditorTab::new(Some(project.root.join("source.txt")), "source.txt".into());
        tab.text = "unsaved human buffer".into();
        tab.saved_text = "saved on disk".into();
        tab.dirty = true;
        tab.selection = 2..6;
        project.tabs.push(tab);
        app.projects.push(project);
        (app, base)
    }

    fn wait_file_job(app: &mut MorrowApp, id: u64) -> serde_json::Value {
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            app.poll();
            let result = app.control_file_job(id).result;
            if result["complete"] == true {
                return result;
            }
            assert!(
                Instant::now() < deadline,
                "file job did not finish: {result}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn cleanup_filesystem_fixture(base: &Path) {
        for file in ["source.txt", "renamed.txt"] {
            let path = base.join("workspace").join(file);
            if path.exists() {
                fs::remove_file(path).unwrap();
            }
        }
        let folder = base.join("workspace/new folder");
        if folder.exists() {
            fs::remove_dir(folder).unwrap();
        }
        fs::remove_dir(base.join("workspace")).unwrap();
        fs::remove_dir(base.join("outside")).unwrap();
        fs::remove_dir(base).unwrap();
    }

    #[test]
    fn cli_rename_reports_pending_then_applied_and_preserves_dirty_buffer() {
        let (mut app, base) = filesystem_fixture();
        let old = app.projects[0].root.join("source.txt");
        let new = app.projects[0].root.join("renamed.txt");
        let tab_id = app.tab().unwrap().id;
        let response = app.control_file_action(crate::cli::FileAction::Rename {
            path: PathBuf::from(crate::paths::display(&old)),
            name: "renamed.txt".into(),
        });
        assert!(response.ok);
        assert_eq!(response.result["applied"], false);
        let id = response.result["job_id"].as_u64().unwrap();
        assert!(app.context_path_busy(&old));
        let done = wait_file_job(&mut app, id);
        assert_eq!(done["status"], "applied");
        assert_eq!(done["updated_tabs"], serde_json::json!([tab_id]));
        assert_eq!(fs::read_to_string(&new).unwrap(), "saved on disk");
        assert!(!old.exists());
        let tab = app.tab().unwrap();
        assert_eq!(tab.path, Some(new));
        assert_eq!(tab.text, "unsaved human buffer");
        assert_eq!(tab.selection, 2..6);
        assert!(tab.dirty);
        assert!(!app.context_path_busy(&old));
        cleanup_filesystem_fixture(&base);
    }

    #[test]
    fn cli_mkdir_reports_completion_and_rejects_outside_workspace() {
        let (mut app, base) = filesystem_fixture();
        let response = app.control_file_action(crate::cli::FileAction::CreateFolder {
            parent: app.projects[0].root.clone(),
            name: "new folder".into(),
        });
        let done = wait_file_job(&mut app, response.result["job_id"].as_u64().unwrap());
        assert_eq!(done["status"], "applied");
        assert!(app.projects[0].root.join("new folder").is_dir());
        let response = app.control_file_action(crate::cli::FileAction::CreateFolder {
            parent: base.join("outside"),
            name: "must not exist".into(),
        });
        let done = wait_file_job(&mut app, response.result["job_id"].as_u64().unwrap());
        assert_eq!(done["status"], "failed");
        assert_eq!(done["applied"], false);
        assert!(
            done["error"]
                .as_str()
                .unwrap()
                .contains("registered workspace")
        );
        assert!(!base.join("outside/must not exist").exists());
        cleanup_filesystem_fixture(&base);
    }

    #[test]
    fn cli_rename_rechecks_revision_before_changing_the_filesystem() {
        let (mut app, base) = filesystem_fixture();
        let old = app.projects[0].root.join("source.txt");
        let response = app.control_file_action(crate::cli::FileAction::Rename {
            path: old.clone(),
            name: "renamed.txt".into(),
        });
        app.tab_mut().unwrap().text.push_str(" while preparing");
        app.tab_mut().unwrap().changed();
        let done = wait_file_job(&mut app, response.result["job_id"].as_u64().unwrap());
        assert_eq!(done["status"], "failed");
        assert!(done["error"].as_str().unwrap().contains("buffer changed"));
        assert!(old.exists());
        assert!(!app.projects[0].root.join("renamed.txt").exists());
        cleanup_filesystem_fixture(&base);
    }

    #[test]
    fn file_jobs_have_bounded_retention_and_admission() {
        let mut app = fixture();
        let root = app.projects[0].root.clone();
        for id in 1..=32 {
            app.context_file_jobs.entries.push_back(FileJob {
                id,
                operation: FileOperation::NewFolder,
                source: root.clone(),
                destination: root.join(format!("old{id}")),
                phase: FileJobPhase::Applied,
                error: None,
                revisions: Vec::new(),
                updated_tabs: Vec::new(),
            });
        }
        let response = app.control_file_action(crate::cli::FileAction::CreateFolder {
            parent: root.clone(),
            name: "fresh".into(),
        });
        assert!(response.ok);
        assert_eq!(app.context_file_jobs.entries.len(), 32);
        assert!(!app.control_file_job(1).ok);
        for id in 100..103 {
            app.context_file_jobs.entries.push_back(FileJob {
                id,
                operation: FileOperation::NewFolder,
                source: root.clone(),
                destination: root.join(format!("pending{id}")),
                phase: FileJobPhase::Preparing,
                error: None,
                revisions: Vec::new(),
                updated_tabs: Vec::new(),
            });
        }
        let rejected = app.control_file_action(crate::cli::FileAction::CreateFolder {
            parent: root,
            name: "fifth".into(),
        });
        assert!(!rejected.ok);
        assert_eq!(rejected.error.unwrap().code, "file_operations_busy");
    }

    #[test]
    fn busy_path_matching_handles_windows_prefix_case_and_components() {
        #[cfg(windows)]
        {
            assert!(path_contains(
                Path::new(r"\\?\C:\Work\Folder"),
                Path::new(r"c:\work\folder\part\..\file.rs")
            ));
            assert!(!path_contains(
                Path::new(r"C:\Work\Folder"),
                Path::new(r"C:\Work\FolderOther\file.rs")
            ));
        }
    }

    fn fixture() -> MorrowApp {
        let mut app = MorrowApp::empty(egui::Context::default());
        app.settings.autosave = false;
        let mut project = Project::new(PathBuf::from("C:/context-fixture"));
        for (name, text) in [
            ("first.txt", "αβγ\nsecond\n"),
            ("second.txt", "unchanged\n"),
            ("dirty.txt", "unsaved\n"),
        ] {
            let mut tab = EditorTab::new(Some(project.root.join(name)), name.into());
            tab.text = text.into();
            tab.saved_text = text.into();
            tab.dirty = name == "dirty.txt";
            project.tabs.push(tab);
        }
        app.projects.push(project);
        app
    }

    #[test]
    fn cut_preserves_unicode_and_can_be_undone_in_vim() {
        let mut app = fixture();
        let preserved = Yank {
            text: "Vim register".into(),
            linewise: false,
        };
        app.register = Some(preserved.clone());
        app.tab_mut().unwrap().vim.set_register(preserved.clone());
        let id = app.tab().unwrap().id;
        app.tab_mut().unwrap().selection = 1..3;
        app.apply_editor_context_action(id, EditorAction::Cut);
        assert_eq!(app.tab().unwrap().text, "α\nsecond\n");
        assert_eq!(app.register.as_ref(), Some(&preserved));
        assert_eq!(app.tab().unwrap().vim.register(), Some(&preserved));
        app.apply_editor_context_action(id, EditorAction::Undo);
        assert_eq!(app.tab().unwrap().text, "αβγ\nsecond\n");
        assert!(!app.tab().unwrap().dirty);
    }

    #[test]
    fn close_others_preserves_dirty_and_in_flight_documents() {
        let mut app = fixture();
        let keep = app.projects[0].tabs[0].id;
        let dirty = app.projects[0].tabs[2].id;
        app.apply_tab_context_action(keep, TabAction::CloseOthers);
        assert_eq!(
            app.projects[0]
                .tabs
                .iter()
                .map(|tab| tab.id)
                .collect::<Vec<_>>(),
            vec![keep, dirty]
        );
        app.projects[0].tabs[0].saving = true;
        app.apply_tab_context_action(dirty, TabAction::CloseSaved);
        assert_eq!(app.projects[0].tabs.len(), 2);
    }

    #[test]
    fn native_context_paste_replaces_selection_once_and_remains_undoable() {
        let mut app = fixture();
        let id = app.tab().unwrap().id;
        app.tab_mut().unwrap().selection = 1..3;
        app.apply_editor_context_action(id, EditorAction::Paste);
        let ctx = app.ctx.clone();
        let mut output = ctx.run_ui(
            egui::RawInput {
                events: vec![egui::Event::Paste("δ".into())],
                ..Default::default()
            },
            |_| {
                app.context_paste_poll();
                app.context_paste_poll();
            },
        );
        output.textures_delta.clear();
        assert_eq!(app.tab().unwrap().text, "αδ\nsecond\n");
        assert_eq!(app.tab().unwrap().vim.mode, VimMode::Normal);
        assert!(app.context_paste.is_none());
        app.apply_editor_context_action(id, EditorAction::Undo);
        assert_eq!(app.tab().unwrap().text, "αβγ\nsecond\n");
    }

    #[test]
    fn delayed_clipboard_response_cannot_overwrite_a_changed_document() {
        let mut app = fixture();
        let id = app.tab().unwrap().id;
        app.apply_editor_context_action(id, EditorAction::Paste);
        app.tab_mut().unwrap().text.push_str("edited");
        app.tab_mut().unwrap().changed();
        let ctx = app.ctx.clone();
        let mut output = ctx.run_ui(
            egui::RawInput {
                events: vec![egui::Event::Paste("must not paste".into())],
                ..Default::default()
            },
            |_| app.context_paste_poll(),
        );
        output.textures_delta.clear();
        assert_eq!(app.tab().unwrap().text, "αβγ\nsecond\nedited");
        assert!(app.status.as_ref().unwrap().1);
    }

    #[test]
    fn select_all_is_a_vim_visual_selection_and_copy_does_not_edit() {
        let mut app = fixture();
        let id = app.tab().unwrap().id;
        let original = app.tab().unwrap().text.clone();
        app.apply_editor_context_action(id, EditorAction::SelectAll);
        let tab = app.tab().unwrap();
        assert_eq!(tab.vim.mode, VimMode::Visual);
        assert_eq!(
            tab.vim.selection(&tab.text),
            Some(0..tab.text.chars().count())
        );
        app.apply_editor_context_action(id, EditorAction::Copy);
        assert!(app.register.is_none());
        assert!(app.tab().unwrap().vim.register().is_none());
        assert_eq!(app.tab().unwrap().vim.mode, VimMode::Visual);
        assert_eq!(app.tab().unwrap().selection, 0..original.chars().count());
        assert_eq!(app.tab().unwrap().text, original);
        assert!(!app.tab().unwrap().dirty);
    }

    #[test]
    fn context_cut_uses_native_history_when_vim_is_disabled() {
        let mut app = fixture();
        app.settings.vim = false;
        let id = app.tab().unwrap().id;
        app.tab_mut().unwrap().selection = 1..3;
        app.apply_editor_context_action(id, EditorAction::Cut);
        assert_eq!(app.tab().unwrap().text, "α\nsecond\n");
        app.apply_editor_context_action(id, EditorAction::Undo);
        assert_eq!(app.tab().unwrap().text, "αβγ\nsecond\n");
        app.apply_editor_context_action(id, EditorAction::Redo);
        assert_eq!(app.tab().unwrap().text, "α\nsecond\n");
    }

    #[test]
    fn rename_updates_open_paths_without_losing_dirty_text_or_selection() {
        let mut app = fixture();
        let root = app.projects[0].root.clone();
        app.projects[0].tabs[0].path = Some(root.join("src/first.txt"));
        app.projects[0].tabs[2].path = Some(root.join("src/dirty.txt"));
        app.projects[0].tabs[2].selection = 1..4;
        let dirty_id = app.projects[0].tabs[2].id;
        app.context_file_result(FileActionResult {
            job_id: None,
            operation: FileOperation::Rename,
            old_path: root.join("src"),
            new_path: root.join("source"),
            result: Ok(()),
        });
        assert_eq!(
            app.projects[0].tabs[0].path,
            Some(root.join("source/first.txt"))
        );
        assert_eq!(app.projects[0].tabs[1].path, Some(root.join("second.txt")));
        let dirty = &app.projects[0].tabs[2];
        assert_eq!(dirty.id, dirty_id);
        assert_eq!(dirty.text, "unsaved\n");
        assert!(dirty.dirty);
        assert_eq!(dirty.selection, 1..4);
        app.context_file_result(FileActionResult {
            job_id: None,
            operation: FileOperation::Rename,
            old_path: root.join("source/dirty.txt"),
            new_path: root.join("source/renamed.txt"),
            result: Ok(()),
        });
        let dirty = &app.projects[0].tabs[2];
        assert_eq!(dirty.title, "renamed.txt");
        assert_eq!(dirty.path, Some(root.join("source/renamed.txt")));
        assert_eq!(dirty.text, "unsaved\n");
        assert!(dirty.dirty);
    }

    #[test]
    fn filename_validation_rejects_escape_and_windows_reserved_names() {
        let root = std::env::temp_dir().join("morrow-context-name-validation");
        for invalid in [
            "",
            "..",
            ".",
            "../outside",
            "C:\\outside",
            "name.",
            "trailing ",
            " leading",
            "CON.txt",
            "aux",
            "LPT1",
            "COM9",
            "a:b",
            "a\n",
        ] {
            assert!(
                validated_child(&root, invalid).is_err(),
                "accepted {invalid:?}"
            );
        }
        assert_eq!(
            validated_child(&root, "Plan ü.md").unwrap(),
            root.join("Plan ü.md")
        );
    }

    #[test]
    fn rename_cannot_replace_existing_file() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "morrow-context-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let from = root.join("from.txt");
        let to = root.join("to.txt");
        fs::write(&from, "source").unwrap();
        fs::write(&to, "destination").unwrap();
        assert!(rename_without_overwrite(&from, &to).is_err());
        assert_eq!(fs::read_to_string(&from).unwrap(), "source");
        assert_eq!(fs::read_to_string(&to).unwrap(), "destination");
        fs::remove_file(from).unwrap();
        fs::remove_file(to).unwrap();
        fs::remove_dir(root).unwrap();
    }
}

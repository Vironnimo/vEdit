//! Document mutations execute on the GUI thread against an exact buffer revision.
use super::*;
use crate::cli::{EditorCommand, SettingsPatch, TaskAction, TextEdit, TextRange};

impl VEditApp {
    fn control_bookmarks(&mut self, change: cli::BookmarkAction) -> Response {
        match change {
            cli::BookmarkAction::List { limit, offset } => {
                let total = self.bookmarks.items.len();
                let mut items = Vec::new();
                let mut bytes = 0;
                for bookmark in self.bookmarks.items.iter().skip(offset).take(limit) {
                    let item = json!({"id":bookmark.id,"path":crate::paths::display(&bookmark.path),"line":bookmark.line,"column":bookmark.column,"label":bookmark.label});
                    let size = serde_json::to_vec(&item).map_or(262_144, |value| value.len());
                    if bytes + size > 262_144 {
                        break;
                    }
                    bytes += size;
                    items.push(item);
                }
                let next = offset.saturating_add(items.len());
                Response::success(
                    json!({"items":items,"total":total,"offset":offset,"has_more":next<total,"next_offset":if next<total{Some(next)}else{None},"scope":"session"}),
                )
            }
            cli::BookmarkAction::Toggle { tab_id, label } => {
                let tab_id = tab_id.or_else(|| self.tab().map(|tab| tab.id));
                match self.try_toggle_bookmark(tab_id) {
                    Ok(result) => {
                        let (id, added) = match result {
                            crate::bookmarks::ToggleResult::Added(id) => (id, true),
                            crate::bookmarks::ToggleResult::Removed(id) => (id, false),
                        };
                        if added && let Some(label) = label {
                            self.bookmarks.set_label(id, &label);
                        }
                        self.persist_session();
                        Response::success(
                            json!({"applied":true,"id":id,"added":added,"tab_id":tab_id,"persistence":"queued"}),
                        )
                    }
                    Err(error) => Response::failure("bookmark_unavailable", error),
                }
            }
            cli::BookmarkAction::Remove { id } => {
                if !self.bookmarks.remove(id) {
                    return Response::failure(
                        "unknown_bookmark",
                        "The bookmark no longer exists; list bookmarks before retrying",
                    );
                }
                self.bookmark_lines.clear();
                self.session_dirty = true;
                self.persist_session();
                Response::success(
                    json!({"applied":true,"id":id,"removed":true,"persistence":"queued"}),
                )
            }
            cli::BookmarkAction::Rename { id, label } => {
                if !self.bookmarks.set_label(id, &label) {
                    return Response::failure(
                        "unknown_bookmark",
                        "The bookmark no longer exists; list bookmarks before retrying",
                    );
                }
                self.bookmark_lines.clear();
                self.session_dirty = true;
                self.persist_session();
                Response::success(
                    json!({"applied":true,"id":id,"label":label,"persistence":"queued"}),
                )
            }
            cli::BookmarkAction::Jump { id } => {
                if self.bookmarks.get(id).is_none() {
                    return Response::failure(
                        "unknown_bookmark",
                        "The bookmark no longer exists; list bookmarks before retrying",
                    );
                }
                if self
                    .tab()
                    .is_some_and(|tab| tab.vim.mode == VimMode::Insert)
                {
                    return Response::failure(
                        "editor_busy",
                        "Finish the current Insert transaction before navigating to a bookmark; nothing changed",
                    );
                }
                match self.try_jump_to_bookmark(id) {
                    Ok(tab_id) => Response::success(
                        json!({"applied":true,"id":id,"tab_id":tab_id,"pending":self.pending_bookmark_jump.is_some()}),
                    ),
                    Err(error) => Response::failure("bookmark_unavailable", error),
                }
            }
        }
    }
    pub(super) fn control_document_request(&mut self, request: Request) -> Response {
        match request {
            Request::Edit {
                tab_id,
                expected_revision,
                edits,
                set_text,
            } => {
                let Some((project, index)) = self.control_find_tab(tab_id) else {
                    return unknown_tab(tab_id);
                };
                let tab = &self.projects[project].tabs[index];
                if let Some(error) = ready_for_edit(tab, expected_revision) {
                    return error;
                }
                let replacement = match set_text {
                    Some(text) => PreparedEdit {
                        cursor: text.chars().count(),
                        text,
                        count: 1,
                    },
                    None => match prepare_edits(&tab.text, &edits) {
                        Ok(edit) => edit,
                        Err(error) => return Response::failure("invalid_edit", error),
                    },
                };
                if replacement.text.len() > MAX_DOCUMENT as usize {
                    return Response::failure(
                        "document_too_large",
                        "Result exceeds the editor's 8 MiB document limit; nothing was changed",
                    );
                }
                let count = replacement.count;
                let changed =
                    self.apply_control_edit(project, index, replacement.text, replacement.cursor);
                Response::success(
                    json!({"applied":true,"changed":changed,"edits_applied":count,"tab":self.control_tab_value(project,index),"selection":self.control_selection_value(project,index)}),
                )
            }
            Request::Selection {
                tab_id,
                range,
                expected_revision,
            } => {
                let Some((project, index)) = self.control_find_tab(tab_id) else {
                    return unknown_tab(tab_id);
                };
                let tab = &self.projects[project].tabs[index];
                if let Some(error) = ready_for_read(tab) {
                    return error;
                }
                if let Some(range) = range {
                    if let Some(error) = ready_for_edit(
                        tab,
                        expected_revision.expect("validated selection revision"),
                    ) {
                        return error;
                    }
                    let start = match character_offset(&tab.text, &range.start) {
                        Ok(offset) => offset,
                        Err(error) => return Response::failure("invalid_location", error),
                    };
                    let end = match character_offset(&tab.text, &range.end) {
                        Ok(offset) => offset,
                        Err(error) => return Response::failure("invalid_location", error),
                    };
                    finish_control_insert(&mut self.projects[project].tabs[index]);
                    self.activate_tab(project, index);
                    self.set_document_view(DocumentView::Source);
                    let tab = &mut self.projects[project].tabs[index];
                    tab.set_cursor(start..end);
                    tab.vim.set_selection(&tab.text, start..end);
                    self.session_dirty = true;
                }
                Response::success(self.control_selection_value(project, index))
            }
            Request::Settings { patch } => {
                if let Some(patch) = patch {
                    if let Some(bindings) = &patch.keybindings
                        && let Err(error) = keybindings::validate(bindings)
                    {
                        return Response::failure("invalid_keybindings", error);
                    }
                    if patch.vim.is_some_and(|value| value != self.settings.vim)
                        && self
                            .projects
                            .iter()
                            .flat_map(|p| &p.tabs)
                            .any(|tab| tab.vim.mode == VimMode::Insert)
                    {
                        return Response::failure(
                            "editor_busy",
                            "A document is in Vim Insert mode. Finish the user's editing transaction before switching Vim settings; no preference changed",
                        );
                    }
                    let previous_excludes = patch
                        .search_excludes
                        .as_ref()
                        .map(|_| self.settings.search_excludes.clone());
                    apply_settings_patch(&mut self.settings, &patch);
                    if previous_excludes
                        .is_some_and(|previous| previous != self.settings.search_excludes)
                    {
                        self.invalidate_project_search();
                    }
                    for tab in self.projects.iter_mut().flat_map(|p| &mut p.tabs) {
                        let indent = tab.indentation(&self.settings);
                        tab.vim.set_indentation(indent.width, indent.tabs);
                    }
                    self.session_dirty = true;
                    self.persist_session();
                    Response::success(
                        json!({"applied":true,"settings":self.settings,"keybinding_actions":self.keybindings_catalog(),"persistence":"queued","autosave_paused_tabs":self.projects.iter().flat_map(|p|&p.tabs).filter(|t|t.agent_unsaved).map(|t|t.id).collect::<Vec<_>>()}),
                    )
                } else {
                    Response::success(
                        json!({"settings":self.settings,"keybinding_actions":self.keybindings_catalog()}),
                    )
                }
            }
            Request::Command {
                tab_id,
                command,
                expected_revision,
            } => self.control_editor_command(tab_id, command, expected_revision),
            Request::Find {
                tab_id,
                query,
                case_sensitive,
                whole_word,
                limit,
                offset,
            } => {
                let Some((project, index)) = self.control_find_tab(tab_id) else {
                    return unknown_tab(tab_id);
                };
                let tab = &self.projects[project].tabs[index];
                if let Some(error) = ready_for_read(tab) {
                    return error;
                }
                let found = match literal_matches(
                    &tab.text,
                    &query,
                    case_sensitive,
                    whole_word,
                    offset + limit + 1,
                ) {
                    Ok(found) => found,
                    Err(error) => return Response::failure("invalid_query", error),
                };
                let has_more = found.len() > offset + limit;
                let items=found.into_iter().skip(offset).take(limit).map(|range|checked_coordinate_range(tab,range.clone()).map(|coordinates|json!({"range":coordinates,"text":char_slice(&tab.text,range)}))).collect::<Result<Vec<_>,String>>();
                let items = match items {
                    Ok(items) => items,
                    Err(error) => return Response::failure("unaddressable_match", error),
                };
                Response::success(
                    json!({"tab_id":tab_id,"revision":tab.revision,"query":query,"items":items,"offset":offset,"has_more":has_more,"next_offset":if has_more{Some(offset+limit)}else{None},"coordinates":"exclusive end"}),
                )
            }
            Request::Replace {
                tab_id,
                expected_revision,
                query,
                replacement,
                case_sensitive,
                whole_word,
                all,
            } => {
                let Some((project, index)) = self.control_find_tab(tab_id) else {
                    return unknown_tab(tab_id);
                };
                let tab = &self.projects[project].tabs[index];
                if let Some(error) = ready_for_edit(tab, expected_revision) {
                    return error;
                }
                let matches = match literal_matches(
                    &tab.text,
                    &query,
                    case_sensitive,
                    whole_word,
                    if all { 10_001 } else { 1 },
                ) {
                    Ok(found) => found,
                    Err(error) => return Response::failure("invalid_query", error),
                };
                if matches.len() > 10_000 {
                    return Response::failure(
                        "too_many_matches",
                        "More than 10000 matches; narrow the query or use precise edit ranges. Nothing was changed",
                    );
                }
                let edits = matches
                    .into_iter()
                    .map(|range| {
                        checked_coordinate_range(tab, range).map(|range| TextEdit {
                            range,
                            text: replacement.clone(),
                        })
                    })
                    .collect::<Result<Vec<_>, String>>();
                let edits = match edits {
                    Ok(edits) => edits,
                    Err(error) => return Response::failure("unaddressable_match", error),
                };
                if edits.is_empty() {
                    return Response::success(
                        json!({"applied":true,"changed":false,"replaced":0,"tab":self.control_tab_value(project,index)}),
                    );
                }
                let edit = match prepare_edits(&tab.text, &edits) {
                    Ok(edit) => edit,
                    Err(error) => return Response::failure("invalid_edit", error),
                };
                if edit.text.len() > MAX_DOCUMENT as usize {
                    return Response::failure(
                        "document_too_large",
                        "Result exceeds 8 MiB; nothing was changed",
                    );
                }
                let count = edit.count;
                let changed = self.apply_control_edit(project, index, edit.text, edit.cursor);
                Response::success(
                    json!({"applied":true,"changed":changed,"replaced":count,"tab":self.control_tab_value(project,index)}),
                )
            }
            Request::New { title, text, path } => {
                if path
                    .as_ref()
                    .is_some_and(|path| self.context_file_path_pending(path))
                {
                    return Response::failure(
                        "file_operation_pending",
                        "A rename affects the new document path; wait for file_job completion before creating it",
                    );
                }
                if let Some(path) = &path
                    && fs::symlink_metadata(path).is_ok()
                {
                    return Response::failure(
                        "destination_exists",
                        "The new document path already exists; open it instead. Nothing was created",
                    );
                }
                if let Some(path) = &path {
                    let index = self.file_workspace(&super::canonical_target(path));
                    self.finish_insert_before_switch();
                    self.switch_project(index);
                } else if self.projects.is_empty() {
                    let index = self.ensure_other_files();
                    self.switch_project(index);
                }
                let title = title
                    .or_else(|| {
                        path.as_ref()
                            .and_then(|p| p.file_name())
                            .map(|n| n.to_string_lossy().into_owned())
                    })
                    .unwrap_or_else(|| "Untitled".into());
                let mut tab = EditorTab::new(path, title);
                tab.text = text;
                if !tab.text.is_empty() || tab.path.is_some() {
                    tab.changed();
                }
                // A named new document must be saved even when its text is empty.
                tab.dirty = tab.path.is_some() || !tab.text.is_empty();
                tab.agent_unsaved = tab.dirty;
                let project = self.active_project;
                let index = self.projects[project].tabs.len();
                self.projects[project].tabs.push(tab);
                self.activate_tab(project, index);
                self.session_dirty = true;
                Response::success(
                    json!({"applied":true,"created_on_disk":false,"tab":self.control_tab_value(project,index)}),
                )
            }
            Request::SaveAs {
                tab_id,
                expected_revision,
                path,
            } => {
                let Some((project, index)) = self.control_find_tab(tab_id) else {
                    return unknown_tab(tab_id);
                };
                let tab = &self.projects[project].tabs[index];
                if let Some(error) = ready_for_edit(tab, expected_revision) {
                    return error;
                }
                if self.context_file_path_pending(&path) {
                    return Response::failure(
                        "file_operation_pending",
                        "A rename affects the save-as destination; wait for file_job completion before saving",
                    );
                }
                if let Some(error) = self.git_checkout_save_error(&path) {
                    return Response::failure("git_checkout_running", error);
                }
                // Existing destination refusal also covers symlinks and case aliases.
                if fs::symlink_metadata(&path).is_ok() {
                    return Response::failure(
                        "destination_exists",
                        "Save-as never overwrites an existing destination. Choose a new path, or use save for the current document",
                    );
                }
                if self
                    .projects
                    .iter()
                    .flat_map(|p| &p.tabs)
                    .any(|t| t.id != tab_id && t.path.as_ref() == Some(&path))
                {
                    return Response::failure(
                        "destination_open",
                        "Another tab already owns that path; nothing was saved",
                    );
                }
                self.save_tab(tab_id, Some(path));
                Response::success(
                    json!({"applied":true,"saving":self.projects[project].tabs[index].saving,"tab":self.control_tab_value(project,index),"completion":"Inspect status until saving is false and error is null"}),
                )
            }
            Request::Task { action, workspace } => {
                let (project, source) = match self.control_project_index(workspace.as_deref()) {
                    Ok(found) => found,
                    Err(response) => return response,
                };
                let root = crate::paths::display(&self.projects[project].root);
                let run = &self.projects[project].run;
                if matches!(action, TaskAction::Stop) {
                    let running = run.task.is_some();
                    if let Some(task) = &run.task {
                        task.cancel();
                    }
                    return Response::success(
                        json!({"applied":true,"workspace":root,"workspace_source":source,"cancellation_requested":running,"running":running,"status":run.status}),
                    );
                }
                if run.task.is_some() {
                    return Response::failure(
                        "task_running",
                        format!(
                            "A task is already running in {root}. Inspect output or stop it explicitly before starting another"
                        ),
                    );
                }
                let action = match action {
                    TaskAction::Check => project_tasks::ProjectTaskAction::Check,
                    TaskAction::Test => project_tasks::ProjectTaskAction::Test,
                    TaskAction::Build => project_tasks::ProjectTaskAction::Build,
                    TaskAction::RunFile => project_tasks::ProjectTaskAction::RunFile,
                    TaskAction::Stop => unreachable!(),
                };
                let task = match self.resolve_project_task_in(project, action) {
                    Ok(task) => task,
                    Err(error) => return Response::failure("task_unavailable", error),
                };
                let command = task.command.display();
                self.start_resolved_task(project, task);
                let run = &self.projects[project].run;
                Response::success(
                    json!({"applied":true,"workspace":root,"workspace_source":source,"running":run.task.is_some(),"status":run.status,"command":command,"source":"Tasks read saved files on disk; unsaved buffers are not written"}),
                )
            }
            Request::File { action } => self.control_file_action(action),
            Request::FileJob { job_id } => self.control_file_job(job_id),
            Request::Bookmarks { change } => self.control_bookmarks(change),
            Request::Layout { change } => match self.control_layout(change) {
                Ok(layout) => Response::success(layout),
                Err(error) => Response::failure("invalid_layout", error),
            },
            Request::Batch {
                actions,
                stop_on_error,
            } => {
                let count = actions.len();
                let mut items = Vec::new();
                let mut stopped_at = None;
                let mut stopped_reason = None;
                let mut response_bytes = 0;
                for (index, action) in actions.into_iter().enumerate() {
                    let response = self.control_request(action);
                    let successful =
                        response.ok && response.result.get("complete") != Some(&Value::Bool(false));
                    let response = compact_batch_response(response);
                    response_bytes += serde_json::to_vec(&response).map_or(0, |bytes| bytes.len());
                    items.push(json!({"index":index,"response":response}));
                    if !successful && stop_on_error {
                        stopped_at = Some(index);
                        stopped_reason = Some("action_failed");
                        break;
                    }
                    if response_bytes >= 6 * 1024 * 1024 && index + 1 < count {
                        stopped_at = Some(index + 1);
                        stopped_reason = Some("result_budget");
                        break;
                    }
                }
                let completed = items.len();
                let complete = completed == count
                    && items.iter().all(|item| {
                        item["response"]["ok"] == true
                            && item["response"]["result"]["complete"] != false
                    });
                Response::success(
                    json!({"complete":complete,"items":items,"stopped_at":stopped_at,"stopped_reason":stopped_reason,"remaining":count-completed,"rollback":false,"active_tab":self.tab().map(|t|t.id)}),
                )
            }
            _ => Response::failure("unsupported_request", "Request has no document adapter"),
        }
    }

    fn apply_control_edit(
        &mut self,
        project: usize,
        index: usize,
        text: String,
        cursor: usize,
    ) -> bool {
        let changed = self.projects[project].tabs[index].text != text;
        if changed {
            self.context_native_undo_checkpoint(self.projects[project].tabs[index].id);
        }
        let tab = &mut self.projects[project].tabs[index];
        if changed {
            finish_control_insert(tab);
            tab.vim.begin_external_edit(&tab.text);
            tab.text = text;
            tab.changed();
            if tab.path.is_some() && tab.fingerprint.is_none() {
                tab.dirty = true;
            }
            tab.agent_unsaved = tab.dirty;
        }
        tab.set_cursor(cursor..cursor);
        tab.vim.set_selection(&tab.text, cursor..cursor);
        tab.pending_agent_location = None;
        tab.agent_location_error = None;
        let id = tab.id;
        self.sync_external_document_navigation(id);
        self.session_dirty = true;
        changed
    }

    fn control_selection_value(&self, project: usize, index: usize) -> Value {
        let tab = &self.projects[project].tabs[index];
        let selection = if self.settings.vim {
            tab.vim
                .selection(&tab.text)
                .unwrap_or(tab.vim.cursor..tab.vim.cursor)
        } else {
            tab.selection.clone()
        };
        let selected_chars = selection.len();
        let mut text = char_slice(&tab.text, selection.clone());
        let truncated = text.len() > 65_536;
        truncate_utf8(&mut text, 65_536);
        let (can_undo, can_redo) = if self.settings.vim {
            (tab.vim.can_undo(), tab.vim.can_redo())
        } else {
            self.context_native_history(tab.id)
        };
        json!({"tab_id":tab.id,"revision":tab.revision,"range":coordinate_range(tab,selection),"selected_characters":selected_chars,"text":text,"text_truncated":truncated,"mode":if self.settings.vim{format!("{:?}",tab.vim.mode).to_lowercase()}else{"native".into()},"can_undo":can_undo,"can_redo":can_redo,"dirty":tab.dirty})
    }

    fn control_editor_command(
        &mut self,
        id: u64,
        command: EditorCommand,
        revision: u64,
    ) -> Response {
        let Some((project, index)) = self.control_find_tab(id) else {
            return unknown_tab(id);
        };
        if let Some(error) = ready_for_edit(&self.projects[project].tabs[index], revision) {
            return error;
        }
        if matches!(command, EditorCommand::Preview)
            && !self.projects[project].tabs[index]
                .path
                .as_ref()
                .is_some_and(|p| cli::is_markdown(p))
        {
            return Response::failure(
                "preview_requires_markdown",
                "This tab is not a Markdown document",
            );
        }
        if matches!(command, EditorCommand::Reload) && self.projects[project].tabs[index].dirty {
            return Response::failure(
                "unsaved_changes",
                "Reload would discard unsaved text; save or undo it first",
            );
        }
        if matches!(
            command,
            EditorCommand::FindNext | EditorCommand::FindPrevious
        ) && self.find_text.is_empty()
        {
            return Response::failure(
                "no_find_query",
                "Find-next/previous use the app's visible Find query, which is empty. Use find with an explicit query then set selection from a returned range",
            );
        }
        finish_control_insert(&mut self.projects[project].tabs[index]);
        self.activate_tab(project, index);
        if !self.settings.vim && matches!(command, EditorCommand::Undo | EditorCommand::Redo) {
            let before = self.projects[project].tabs[index].text.clone();
            self.context_native_undo(id, matches!(command, EditorCommand::Redo));
            let tab = &mut self.projects[project].tabs[index];
            let changed = tab.text != before;
            if changed {
                if tab.path.is_some() && tab.fingerprint.is_none() {
                    tab.dirty = true;
                }
                tab.agent_unsaved = tab.dirty;
            }
            return Response::success(
                json!({"applied":true,"changed":changed,"command":command,"tab":self.control_tab_value(project,index),"selection":self.control_selection_value(project,index)}),
            );
        }
        let indent = self.projects[project].tabs[index].indentation(&self.settings);
        let tab = &mut self.projects[project].tabs[index];
        let selection = if self.settings.vim {
            tab.vim
                .selection(&tab.text)
                .unwrap_or_else(|| tab.selection.clone())
        } else {
            tab.selection.clone()
        };
        let mut changed = false;
        match command {
            EditorCommand::Undo | EditorCommand::Redo => {
                let cursor = tab.vim.cursor;
                tab.vim.set_selection(&tab.text, cursor..cursor);
                let action = tab.vim.handle_key(
                    &mut tab.text,
                    if matches!(command, EditorCommand::Undo) {
                        VimKey::Char('u')
                    } else {
                        VimKey::Redo
                    },
                );
                changed = action.text_changed;
                if changed {
                    tab.changed();
                    if tab.path.is_some() && tab.fingerprint.is_none() {
                        tab.dirty = true;
                    }
                    tab.agent_unsaved = tab.dirty;
                }
                tab.set_cursor(tab.vim.cursor..tab.vim.cursor);
            }
            EditorCommand::SelectAll => {
                let range = 0..tab.text.chars().count();
                tab.set_cursor(range.clone());
                tab.vim.set_selection(&tab.text, range);
            }
            EditorCommand::ClearSelection => {
                let range = tab.vim.cursor..tab.vim.cursor;
                tab.set_cursor(range.clone());
                tab.vim.set_selection(&tab.text, range);
            }
            EditorCommand::Indent
            | EditorCommand::Outdent
            | EditorCommand::ToggleComment
            | EditorCommand::DuplicateLine
            | EditorCommand::DeleteLine => {
                let edit = match command {
                    EditorCommand::Indent | EditorCommand::Outdent => {
                        editor::indent_lines_with_width(
                            &tab.text,
                            selection,
                            &indent.unit(),
                            indent.width,
                            matches!(command, EditorCommand::Outdent),
                        )
                    }
                    EditorCommand::ToggleComment => editor::toggle_line_comment(
                        &tab.text,
                        selection,
                        crate::file_kind::comment_language(tab.path.as_deref()),
                    ),
                    EditorCommand::DuplicateLine => editor::duplicate_lines(&tab.text, selection),
                    EditorCommand::DeleteLine => {
                        let start_line = tab
                            .lines
                            .line_col(&tab.text, tab.revision, selection.start)
                            .0;
                        let end_line = tab
                            .lines
                            .line_col(
                                &tab.text,
                                tab.revision,
                                selection
                                    .end
                                    .saturating_sub(usize::from(!selection.is_empty())),
                            )
                            .0;
                        let start = editor::go_to_line(&tab.text, start_line);
                        let end = editor::go_to_line(&tab.text, end_line + 1);
                        let end = if end_line >= tab.text.split('\n').count() {
                            tab.text.chars().count()
                        } else {
                            end
                        };
                        editor::replace_selection(&tab.text, start..end, "")
                    }
                    _ => unreachable!(),
                };
                if edit.text.len() > MAX_DOCUMENT as usize {
                    return Response::failure(
                        "document_too_large",
                        "Command result exceeds 8 MiB; nothing changed",
                    );
                }
                changed = tab.text != edit.text;
                if changed {
                    self.context_native_undo_checkpoint(id);
                    let tab = &mut self.projects[project].tabs[index];
                    tab.vim.begin_external_edit(&tab.text);
                    tab.text = edit.text;
                    tab.changed();
                    if tab.path.is_some() && tab.fingerprint.is_none() {
                        tab.dirty = true;
                    }
                    tab.agent_unsaved = tab.dirty;
                }
                let tab = &mut self.projects[project].tabs[index];
                tab.set_cursor(edit.selection.clone());
                tab.vim.set_selection(&tab.text, edit.selection);
            }
            EditorCommand::Reload => self.reload_current(),
            EditorCommand::FindNext | EditorCommand::FindPrevious => {
                self.find_next(matches!(command, EditorCommand::FindPrevious))
            }
            EditorCommand::Source => self.set_document_view(DocumentView::Source),
            EditorCommand::Preview => self.set_document_view(DocumentView::Preview),
        }
        self.session_dirty = true;
        Response::success(
            json!({"applied":true,"changed":changed,"command":command,"tab":self.control_tab_value(project,index),"selection":self.control_selection_value(project,index)}),
        )
    }
}

fn ready_for_read(tab: &EditorTab) -> Option<Response> {
    if tab.loading {
        Some(Response::failure(
            "loading",
            "The document is still loading; inspect status before continuing",
        ))
    } else {
        tab.error
            .as_ref()
            .map(|error| Response::failure("file_error", error))
    }
}
fn ready_for_edit(tab: &EditorTab, revision: u64) -> Option<Response> {
    if let Some(error) = ready_for_read(tab) {
        return Some(error);
    }
    if tab.revision != revision {
        let mut response = Response::failure(
            "revision_conflict",
            "The buffer changed since observation; no edit was applied. Read the current buffer and construct the remaining edit against its returned revision",
        );
        response.result = json!({"tab_id":tab.id,"expected_revision":revision,"actual_revision":tab.revision,"dirty":tab.dirty});
        return Some(response);
    }
    if tab.saving {
        return Some(Response::failure(
            "saving",
            "A save is in progress; wait for status saving:false before mutating this document",
        ));
    }
    if tab.vim.mode == VimMode::Insert {
        let mut preview_vim = tab.vim.clone();
        let mut preview_text = tab.text.clone();
        preview_vim.finish_insert(&mut preview_text);
        if preview_text != tab.text {
            return Some(Response::failure(
                "counted_insert_active",
                "Finishing the user's counted Vim Insert would change the text beyond the observed revision. Finish it in the app, then read the buffer again; nothing was changed",
            ));
        }
    }
    None
}
fn apply_settings_patch(settings: &mut Settings, patch: &SettingsPatch) {
    if let Some(bindings) = &patch.keybindings {
        settings.keybindings = bindings.clone();
    }
    if let Some(value) = patch.vim {
        settings.vim = value;
    }
    if let Some(value) = patch.swap_jk {
        settings.swap_jk = value;
    }
    if let Some(value) = &patch.search_excludes {
        // The entire patch was validated before any preference could change.
        settings.search_excludes =
            cli::normalize_search_excludes(value).expect("validated search exclusion patch");
    }
    if let Some(value) = patch.autosave {
        settings.autosave = value;
    }
    if let Some(value) = patch.autosave_delay_ms {
        settings.autosave_delay_ms = value;
    }
    if let Some(value) = patch.ui_scale {
        settings.ui_scale = value;
    }
    if let Some(value) = patch.line_height {
        settings.line_height = value;
    }
    if let Some(value) = patch.line_numbers {
        settings.line_numbers = value;
    }
    if let Some(value) = patch.highlight_current_line {
        settings.highlight_current_line = value;
    }
    if let Some(value) = patch.git_gutter {
        settings.git_gutter = value;
    }
    // Retained for older CLI clients; Explorer now always lists ignored files.
    settings.show_ignored_files = true;
    if let Some(value) = patch.font_size {
        settings.font_size = value;
    }
    if let Some(value) = patch.editor_font {
        settings.editor_font = value;
    }
    if let Some(value) = patch.tab_width {
        settings.tab_width = value;
    }
    if let Some(value) = patch.detect_indentation {
        settings.detect_indentation = value;
    }
    if let Some(value) = patch.indent_with_tabs {
        settings.indent_with_tabs = value;
    }
    if let Some(value) = &patch.language_indent_widths {
        settings.language_indent_widths = value.clone();
    }
    if let Some(value) = &patch.language_indent_with_tabs {
        settings.language_indent_with_tabs = value.clone();
    }
    if let Some(value) = patch.word_wrap {
        settings.word_wrap = value;
    }
    if let Some(value) = patch.smooth_scroll {
        settings.smooth_scroll = value;
    }
    if let Some(value) = &patch.leader_key {
        // validate(request) has checked this before applying any preference.
        settings.leader_key = cli::normalize_leader_key(value).unwrap_or_else(|_| " ".into());
    }
}
fn finish_control_insert(tab: &mut EditorTab) {
    if tab.vim.mode == VimMode::Insert {
        let before = tab.text.clone();
        tab.vim.finish_insert(&mut tab.text);
        debug_assert_eq!(
            tab.text, before,
            "preflight must ensure finishing Insert cannot change the buffer"
        );
    }
}
fn coordinate_range(tab: &EditorTab, range: Range<usize>) -> TextRange {
    let (start_line, start_column) = tab.lines.line_col(&tab.text, tab.revision, range.start);
    let (end_line, end_column) = tab.lines.line_col(&tab.text, tab.revision, range.end);
    TextRange {
        start: Location {
            line: start_line,
            column: start_column,
        },
        end: Location {
            line: end_line,
            column: end_column,
        },
    }
}
fn checked_coordinate_range(tab: &EditorTab, range: Range<usize>) -> Result<TextRange, String> {
    let value = coordinate_range(tab, range.clone());
    if tab.lines.position(
        &tab.text,
        tab.revision,
        value.start.line,
        value.start.column,
    ) != Some(range.start)
        || tab
            .lines
            .position(&tab.text, tab.revision, value.end.line, value.end.column)
            != Some(range.end)
    {
        return Err("A literal match splits a CRLF line ending, which editor coordinates cannot represent. Search/replace the entire CRLF sequence or use a whole-line edit; nothing was changed".into());
    }
    Ok(value)
}
fn char_slice(text: &str, range: Range<usize>) -> String {
    text.chars().skip(range.start).take(range.len()).collect()
}
struct PreparedEdit {
    text: String,
    cursor: usize,
    count: usize,
}
fn prepare_edits(text: &str, edits: &[TextEdit]) -> Result<PreparedEdit, String> {
    let lines = crate::render::LineIndex::default();
    let offset = |location: &Location| {
        lines
            .position(text, 0, location.line, location.column)
            .ok_or_else(|| {
                format!(
                    "No character boundary at line {}, column {}; no edit applied",
                    location.line, location.column
                )
            })
    };
    let mut ranges = edits
        .iter()
        .map(|edit| {
            Ok((
                offset(&edit.range.start)?,
                offset(&edit.range.end)?,
                &edit.text,
            ))
        })
        .collect::<Result<Vec<_>, String>>()?;
    ranges.sort_by_key(|(start, end, _)| (*start, *end));
    for pair in ranges.windows(2) {
        if pair[1].0 < pair[0].1 || pair[1].0 == pair[0].0 {
            return Err("Edits overlap or insert at the same boundary; no edit was applied".into());
        }
    }
    let chars: Vec<char> = text.chars().collect();
    let mut result = String::new();
    let mut previous = 0;
    let mut cursor = 0;
    for (start, end, replacement) in ranges {
        result.extend(chars[previous..start].iter());
        result.push_str(replacement);
        cursor += start - previous + replacement.chars().count();
        previous = end;
        if result.len() > MAX_DOCUMENT as usize {
            return Err("Result exceeds 8 MiB; no edit was applied".into());
        }
    }
    result.extend(chars[previous..].iter());
    Ok(PreparedEdit {
        text: result,
        cursor,
        count: edits.len(),
    })
}
fn literal_matches(
    text: &str,
    query: &str,
    case_sensitive: bool,
    whole_word: bool,
    limit: usize,
) -> Result<Vec<Range<usize>>, String> {
    let pattern = regex::RegexBuilder::new(&regex::escape(query))
        .case_insensitive(!case_sensitive)
        .build()
        .map_err(|error| error.to_string())?;
    let mut previous_byte = 0;
    let mut previous_char = 0;
    let mut result = Vec::new();
    for found in pattern.find_iter(text) {
        if whole_word
            && (text[..found.start()]
                .chars()
                .next_back()
                .is_some_and(crate::search::word_char)
                || text[found.end()..]
                    .chars()
                    .next()
                    .is_some_and(crate::search::word_char))
        {
            continue;
        }
        let start = previous_char + text[previous_byte..found.start()].chars().count();
        let end = start + found.as_str().chars().count();
        result.push(start..end);
        previous_byte = found.end();
        previous_char = end;
        if result.len() >= limit {
            break;
        }
    }
    Ok(result)
}

fn compact_batch_response(mut response: Response) -> Response {
    let bytes = serde_json::to_vec(&response.result).map_or(0, |bytes| bytes.len());
    if bytes <= 128 * 1024 {
        return response;
    }
    let original = response.result;
    let mut summary = serde_json::Map::new();
    for key in [
        "applied",
        "changed",
        "complete",
        "edits_applied",
        "replaced",
        "active_tab",
        "tab_id",
        "revision",
        "dirty",
        "saving",
        "loading",
        "start_line",
        "end_line",
        "total_lines",
        "truncated",
        "next_line",
        "offset",
        "total",
        "has_more",
        "next_offset",
        "job_id",
        "pending",
        "accepted",
    ] {
        if let Some(value) = original.get(key) {
            summary.insert(key.into(), value.clone());
        }
    }
    if let Some(tab) = original.get("tab") {
        summary.insert("tab".into(), compact_tab(tab));
    }
    if let Some(items) = original.get("items").and_then(Value::as_array) {
        summary.insert(
            "items".into(),
            Value::Array(
                items
                    .iter()
                    .enumerate()
                    .map(|(index, item)| compact_item(index, item))
                    .collect(),
            ),
        );
    }
    if let Some(tabs) = original.get("tabs") {
        let mut compact = serde_json::Map::new();
        for key in ["total", "offset", "has_more"] {
            if let Some(value) = tabs.get(key) {
                compact.insert(key.into(), value.clone());
            }
        }
        if let Some(items) = tabs.get("items").and_then(Value::as_array) {
            compact.insert(
                "items".into(),
                Value::Array(items.iter().map(compact_tab).collect()),
            );
        }
        summary.insert("tabs".into(), Value::Object(compact));
    }
    for key in ["layout", "settings"] {
        if let Some(value) = original.get(key) {
            summary.insert(key.into(), value.clone());
        }
    }
    summary.insert("details_omitted".into(), Value::Bool(true));
    summary.insert("original_result_bytes".into(), json!(bytes));
    summary.insert("continuation".into(),json!("This batch returns compact effect evidence for large results. Request read/tabs/diff/output/diagnostics separately for the omitted detail; do not replay completed mutations."));
    response.result = Value::Object(summary);
    response
}
fn compact_tab(tab: &Value) -> Value {
    let mut result = serde_json::Map::new();
    for key in [
        "id",
        "revision",
        "dirty",
        "autosave_paused",
        "active",
        "view",
        "loading",
        "saving",
        "error",
        "mode",
        "location",
        "location_pending",
        "location_error",
        "annotation_count",
        "annotations_stale",
        "can_undo",
        "can_redo",
    ] {
        if let Some(value) = tab.get(key) {
            result.insert(key.into(), value.clone());
        }
    }
    Value::Object(result)
}
fn compact_item(index: usize, item: &Value) -> Value {
    let mut result = serde_json::Map::new();
    result.insert("item_index".into(), json!(index));
    for key in [
        "id",
        "tab_id",
        "ok",
        "closed",
        "saving",
        "dirty",
        "revision",
        "annotation_count",
        "range",
        "severity",
        "location_clamped",
    ] {
        if let Some(value) = item.get(key) {
            result.insert(key.into(), value.clone());
        }
    }
    if let Some(tab) = item.get("tab") {
        result.insert("tab".into(), compact_tab(tab));
    }
    if let Some(error) = item.get("error") {
        let mut message = error["message"].as_str().unwrap_or_default().to_owned();
        let truncated = message.len() > 1024;
        truncate_utf8(&mut message, 1024);
        result.insert(
            "error".into(),
            json!({"code":error["code"],"message":message,"message_truncated":truncated}),
        );
    }
    Value::Object(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn app_with_text(text: &str) -> (VEditApp, u64) {
        let mut app = VEditApp::empty(egui::Context::default());
        let mut project = Project::new(std::env::temp_dir());
        let mut tab = EditorTab::new(None, "fixture.rs".into());
        tab.text = text.into();
        tab.saved_text = text.into();
        let id = tab.id;
        project.tabs.push(tab);
        app.projects.push(project);
        (app, id)
    }
    fn location(line: usize, column: usize) -> Location {
        Location { line, column }
    }
    fn edit(line: usize, start: usize, end: usize, text: &str) -> TextEdit {
        TextEdit {
            range: TextRange {
                start: location(line, start),
                end: location(line, end),
            },
            text: text.into(),
        }
    }
    fn request_edit(id: u64, revision: u64, edits: Vec<TextEdit>) -> Request {
        Request::Edit {
            tab_id: id,
            expected_revision: revision,
            edits,
            set_text: None,
        }
    }
    fn request_command(id: u64, revision: u64, command: EditorCommand) -> Request {
        Request::Command {
            tab_id: id,
            expected_revision: revision,
            command,
        }
    }

    #[test]
    fn atomic_unicode_edits_use_original_coordinates_and_one_undo_step() {
        let (mut app, id) = app_with_text("β😀c\r\nx64\n");
        let response = app.control_request(request_edit(
            id,
            0,
            vec![edit(2, 2, 4, "96"), edit(1, 2, 3, "🦀")],
        ));
        assert!(response.ok, "{response:?}");
        assert_eq!(app.tab().unwrap().text, "β🦀c\r\nx96\n");
        assert_eq!(response.result["tab"]["revision"], 1);
        assert_eq!(response.result["tab"]["autosave_paused"], true);
        assert!(app.tab().unwrap().dirty);
        let response = app.control_request(request_command(id, 1, EditorCommand::Undo));
        assert!(response.ok);
        assert_eq!(app.tab().unwrap().text, "β😀c\r\nx64\n");
        assert!(!app.tab().unwrap().dirty);
        assert!(!app.tab().unwrap().vim.can_undo());
        assert!(app.tab().unwrap().vim.can_redo());
    }

    #[test]
    fn revision_conflict_and_invalid_final_range_leave_all_edits_unapplied() {
        let (mut app, id) = app_with_text("budget64");
        let response = app.control_request(request_edit(id, 7, vec![edit(1, 7, 9, "96")]));
        assert_eq!(response.error.unwrap().code, "revision_conflict");
        assert_eq!(response.result["actual_revision"], 0);
        let response = app.control_request(request_edit(
            id,
            0,
            vec![edit(1, 7, 9, "96"), edit(2, 1, 2, "x")],
        ));
        assert_eq!(response.error.unwrap().code, "invalid_edit");
        assert_eq!(app.tab().unwrap().text, "budget64");
        assert_eq!(app.tab().unwrap().revision, 0);
        assert!(!app.tab().unwrap().vim.can_undo());
        assert!(!app.tab().unwrap().agent_unsaved);
    }

    #[test]
    fn overlapping_edits_and_duplicate_insertions_do_not_mutate() {
        for edits in [
            vec![edit(1, 1, 4, "a"), edit(1, 3, 5, "b")],
            vec![edit(1, 2, 2, "a"), edit(1, 2, 2, "b")],
        ] {
            let (mut app, id) = app_with_text("abcdef");
            let response = app.control_request(request_edit(id, 0, edits));
            assert_eq!(response.error.unwrap().code, "invalid_edit");
            assert_eq!(app.tab().unwrap().text, "abcdef");
            assert!(!app.tab().unwrap().vim.can_undo());
        }
    }

    #[test]
    fn empty_set_text_clears_document_but_unknown_target_does_not_fallback() {
        let (mut app, id) = app_with_text("keep");
        let response = app.control_request(Request::Edit {
            tab_id: u64::MAX,
            expected_revision: 0,
            edits: Vec::new(),
            set_text: Some(String::new()),
        });
        assert_eq!(response.error.unwrap().code, "unknown_tab");
        assert_eq!(app.tab().unwrap().text, "keep");
        let response = app.control_request(Request::Edit {
            tab_id: id,
            expected_revision: 0,
            edits: Vec::new(),
            set_text: Some(String::new()),
        });
        assert!(response.ok);
        assert!(app.tab().unwrap().text.is_empty());
        assert!(app.tab().unwrap().dirty);
        assert_eq!(app.tab().unwrap().vim.cursor, 0);
    }

    #[test]
    fn ordinary_insert_and_agent_edit_are_separate_undo_transactions() {
        let (mut app, id) = app_with_text("ab");
        let tab = app.tab_mut().unwrap();
        tab.vim.handle_key(&mut tab.text, VimKey::Char('i'));
        tab.vim.insert_text(&mut tab.text, "X");
        tab.changed();
        let response = app.control_request(request_edit(id, 1, vec![edit(1, 2, 3, "Y")]));
        assert!(response.ok, "{response:?}");
        assert_eq!(app.tab().unwrap().text, "XYb");
        assert_eq!(app.tab().unwrap().vim.mode, VimMode::Normal);
        let response = app.control_request(request_command(id, 2, EditorCommand::Undo));
        assert!(response.ok);
        assert_eq!(app.tab().unwrap().text, "Xab");
        let response = app.control_request(request_command(id, 3, EditorCommand::Undo));
        assert!(response.ok);
        assert_eq!(app.tab().unwrap().text, "ab");
    }

    #[test]
    fn counted_insert_preflight_refuses_without_finishing_or_discarding_count() {
        let (mut app, id) = app_with_text("ab");
        let tab = app.tab_mut().unwrap();
        tab.vim.handle_key(&mut tab.text, VimKey::Char('3'));
        tab.vim.handle_key(&mut tab.text, VimKey::Char('i'));
        tab.vim.insert_text(&mut tab.text, "X");
        tab.changed();
        let response = app.control_request(request_edit(id, 1, vec![edit(1, 2, 3, "Y")]));
        assert_eq!(response.error.unwrap().code, "counted_insert_active");
        assert_eq!(app.tab().unwrap().text, "Xab");
        assert_eq!(app.tab().unwrap().vim.mode, VimMode::Insert);
        assert_eq!(app.tab().unwrap().revision, 1);
        let tab = app.tab_mut().unwrap();
        tab.vim.finish_insert(&mut tab.text);
        assert_eq!(tab.text, "XXXab");
    }

    #[test]
    fn exact_visual_selection_includes_unicode_and_final_newline() {
        let (mut app, id) = app_with_text("😀two\nlast\n");
        let response = app.control_request(Request::Selection {
            tab_id: id,
            range: Some(TextRange {
                start: location(1, 1),
                end: location(3, 1),
            }),
            expected_revision: Some(0),
        });
        assert!(response.ok, "{response:?}");
        assert_eq!(response.result["text"], "😀two\nlast\n");
        assert_eq!(response.result["mode"], "visual");
        assert_eq!(
            app.tab().unwrap().vim.selection(&app.tab().unwrap().text),
            Some(0..10)
        );
        let response = app.control_request(Request::Selection {
            tab_id: id,
            range: None,
            expected_revision: None,
        });
        assert_eq!(response.result["range"]["end"]["line"], 3);
        assert_eq!(response.result["range"]["end"]["column"], 1);
    }

    #[test]
    fn find_queries_current_buffer_without_focus_and_replace_is_revision_checked() {
        let (mut app, id) = app_with_text("β βeta Β\n");
        app.find_text = "unrelated".into();
        let response = app.control_request(Request::Find {
            tab_id: id,
            query: "β".into(),
            case_sensitive: false,
            whole_word: true,
            limit: 1,
            offset: 0,
        });
        assert!(response.ok);
        assert_eq!(response.result["items"][0]["range"]["start"]["column"], 1);
        assert_eq!(response.result["has_more"], true);
        assert_eq!(app.find_text, "unrelated");
        assert_eq!(app.tab().unwrap().vim.cursor, 0);
        let response = app.control_request(Request::Replace {
            tab_id: id,
            expected_revision: 0,
            query: "β".into(),
            replacement: "😀".into(),
            case_sensitive: false,
            whole_word: true,
            all: true,
        });
        assert!(response.ok);
        assert_eq!(response.result["replaced"], 2);
        assert_eq!(app.tab().unwrap().text, "😀 βeta 😀\n");
        assert!(app.tab().unwrap().agent_unsaved);
    }

    #[test]
    fn replacement_never_expands_partial_crlf_match_or_lone_final_cr() {
        let (mut app, id) = app_with_text("a\r\nb\r");
        let response = app.control_request(Request::Replace {
            tab_id: id,
            expected_revision: 0,
            query: "\n".into(),
            replacement: "|".into(),
            case_sensitive: true,
            whole_word: false,
            all: true,
        });
        assert_eq!(response.error.unwrap().code, "unaddressable_match");
        assert_eq!(app.tab().unwrap().text, "a\r\nb\r");
        let response = app.control_request(request_edit(id, 0, vec![edit(2, 2, 3, "!")]));
        assert!(response.ok, "{response:?}");
        assert_eq!(app.tab().unwrap().text, "a\r\nb!");
    }

    #[test]
    fn agent_whole_word_find_and_replace_preserve_unicode_identifiers() {
        let (mut app, id) = app_with_text("git\u{301} git\u{200d} git‿ git");
        let response = app.control_request(Request::Find {
            tab_id: id,
            query: "git".into(),
            case_sensitive: true,
            whole_word: true,
            limit: 100,
            offset: 0,
        });
        assert!(response.ok, "{response:?}");
        let items = response.result["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["range"]["start"]["column"], 16);
        let response = app.control_request(Request::Replace {
            tab_id: id,
            expected_revision: 0,
            query: "git".into(),
            replacement: "hg".into(),
            case_sensitive: true,
            whole_word: true,
            all: true,
        });
        assert!(response.ok, "{response:?}");
        assert_eq!(response.result["replaced"], 1);
        assert_eq!(app.tab().unwrap().text, "git\u{301} git\u{200d} git‿ hg");
    }

    #[test]
    fn ordered_batch_reports_applied_edit_and_stops_at_conflict_without_replay() {
        let (mut app, id) = app_with_text("64");
        let response = app.control_request(Request::Batch {
            actions: vec![
                request_edit(id, 0, vec![edit(1, 1, 3, "96")]),
                request_edit(id, 0, vec![edit(1, 1, 3, "128")]),
                request_command(id, 1, EditorCommand::Undo),
            ],
            stop_on_error: true,
        });
        assert!(response.ok);
        assert_eq!(response.result["complete"], false);
        assert_eq!(response.result["items"].as_array().unwrap().len(), 2);
        assert_eq!(
            response.result["items"][0]["response"]["result"]["tab"]["revision"],
            1
        );
        assert_eq!(
            response.result["items"][1]["response"]["error"]["code"],
            "revision_conflict"
        );
        assert_eq!(response.result["stopped_at"], 1);
        assert_eq!(response.result["remaining"], 1);
        assert_eq!(app.tab().unwrap().text, "96");
    }

    #[test]
    fn batch_rejects_async_search_before_earlier_edits_can_apply() {
        let (mut app, id) = app_with_text("64");
        let response = app.control_request(Request::Batch {
            actions: vec![
                request_edit(id, 0, vec![edit(1, 1, 3, "96")]),
                Request::Search {
                    query: "x".into(),
                    workspace: None,
                    excludes: None,
                    case_sensitive: false,
                    whole_word: false,
                    limit: 100,
                },
            ],
            stop_on_error: true,
        });
        assert_eq!(response.error.unwrap().code, "invalid_request");
        assert_eq!(app.tab().unwrap().text, "64");
        assert_eq!(app.tab().unwrap().revision, 0);
    }

    #[test]
    fn undo_cancels_armed_operator_and_performs_one_step() {
        let (mut app, id) = app_with_text("abc");
        app.control_request(request_edit(id, 0, vec![edit(1, 1, 2, "X")]));
        let tab = app.tab_mut().unwrap();
        tab.vim.handle_key(&mut tab.text, VimKey::Char('d'));
        let response = app.control_request(request_command(id, 1, EditorCommand::Undo));
        assert!(response.ok);
        assert_eq!(app.tab().unwrap().text, "abc");
        assert_eq!(app.tab().unwrap().vim.pending_command(), "");
    }

    #[test]
    fn agent_edits_survive_autosave_and_only_explicit_save_writes() {
        let root = temporary_directory("hold");
        let path = root.join("budget.rs");
        fs::write(&path, "64").unwrap();
        let path = fs::canonicalize(path).unwrap();
        let (mut app, id) = app_with_text("64");
        app.projects[0].root = root.clone();
        let tab = app.tab_mut().unwrap();
        tab.path = Some(path.clone());
        tab.fingerprint = Some(workspace::fingerprint(&path, MAX_DOCUMENT).unwrap());
        tab.recovery_path = root.join("recovery.txt");
        let response = app.control_request(request_edit(id, 0, vec![edit(1, 1, 3, "96")]));
        assert!(response.ok);
        app.tab_mut().unwrap().last_edit = Instant::now() - Duration::from_secs(2);
        app.autosave();
        assert!(app.settings.autosave);
        assert!(app.tab().unwrap().dirty);
        assert!(!app.tab().unwrap().saving);
        assert_eq!(fs::read_to_string(&path).unwrap(), "64");
        assert!(app.session().projects[0].tabs[0].agent_unsaved);
        let response = app.control_request(Request::Save { tab_ids: vec![id] });
        assert_eq!(response.result["items"][0]["saving"], true);
        let deadline = Instant::now() + Duration::from_secs(2);
        while app.tab().is_some_and(|t| t.saving || t.recovery_pending) && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(5));
            app.poll();
        }
        assert!(!app.tab().unwrap().saving);
        assert_eq!(app.tab().unwrap().error, None);
        assert_eq!(fs::read_to_string(&path).unwrap(), "96");
        assert!(!app.tab().unwrap().dirty);
        assert!(!app.tab().unwrap().agent_unsaved);
        drop(app);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn settings_patch_changes_supplied_fields_and_persists_snapshot() {
        let root = temporary_directory("settings");
        let (mut app, _) = app_with_text("64");
        app.recovery_session_path = root.join("session.json");
        let response = app.control_request(Request::Settings {
            patch: Some(SettingsPatch {
                word_wrap: Some(true),
                smooth_scroll: Some(true),
                swap_jk: Some(true),
                ui_scale: Some(1.25),
                font_size: Some(28.0),
                line_height: Some(1.7),
                line_numbers: Some(cli::LineNumbers::Relative),
                highlight_current_line: Some(false),
                git_gutter: Some(false),
                autosave_delay_ms: Some(1500),
                show_ignored_files: Some(true),
                search_excludes: Some(vec![" generated ".into(), "src\\cache".into()]),
                ..Default::default()
            }),
        });
        assert!(response.ok);
        assert!(app.settings.vim);
        assert!(app.settings.autosave);
        assert_eq!(app.settings.font_size, 28.0);
        assert!(app.settings.word_wrap);
        assert!(app.settings.smooth_scroll);
        assert!(app.settings.swap_jk);
        assert_eq!(app.settings.search_excludes, ["generated", "src/cache"]);
        let deadline = Instant::now() + Duration::from_secs(2);
        while !app.recovery_session_path.is_file() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        let snapshot: Session =
            serde_json::from_slice(&fs::read(&app.recovery_session_path).unwrap()).unwrap();
        let saved = snapshot.settings.unwrap();
        assert_eq!(saved.ui_scale, 1.25);
        assert_eq!(saved.line_height, 1.7);
        assert_eq!(saved.line_numbers, cli::LineNumbers::Relative);
        assert!(!saved.highlight_current_line);
        assert!(!saved.git_gutter);
        assert_eq!(saved.autosave_delay_ms, 1500);
        assert!(saved.show_ignored_files);
        assert!(saved.word_wrap && saved.swap_jk);
        assert_eq!(saved.search_excludes, ["generated", "src/cache"]);
        drop(app);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn editor_font_shortcuts_apply_and_persist_without_changing_other_preferences_or_text() {
        for (input, expected, serialized) in [
            (
                "jetbrains-mono",
                cli::EditorFont::JetBrainsMono,
                "jetbrains_mono",
            ),
            ("consolas", cli::EditorFont::Consolas, "consolas"),
            (
                "cascadia-mono",
                cli::EditorFont::CascadiaMono,
                "cascadia_mono",
            ),
        ] {
            let root = temporary_directory(input);
            let (mut app, _) = app_with_text("keep\nβ😀");
            app.recovery_session_path = root.join("session.json");
            app.settings.autosave = false;
            app.settings.swap_jk = true;
            app.settings.font_size = 17.0;
            app.settings.editor_font = cli::EditorFont::Consolas;
            let tab = app.tab_mut().unwrap();
            tab.vim.handle_key(&mut tab.text, VimKey::Char('i'));
            let mut expected_settings = serde_json::to_value(&app.settings).unwrap();
            expected_settings["editor_font"] = json!(serialized);
            let cli::Parsed::Command { request, start } = cli::parse_args(
                ["settings", "font", input]
                    .into_iter()
                    .map(std::ffi::OsString::from),
            )
            .unwrap() else {
                panic!("expected settings command")
            };
            assert!(!start);
            let response = app.control_request(request);
            assert!(response.ok, "{response:?}");
            assert_eq!(response.result["applied"], true);
            assert_eq!(response.result["settings"], expected_settings);
            assert_eq!(
                serde_json::to_value(&app.settings).unwrap(),
                expected_settings
            );
            assert_eq!(app.settings.editor_font, expected);
            assert_eq!(app.tab().unwrap().text, "keep\nβ😀");
            assert_eq!(app.tab().unwrap().revision, 0);
            assert_eq!(app.tab().unwrap().vim.mode, VimMode::Insert);
            assert!(!app.tab().unwrap().dirty);
            for patch in [json!({"editor_font":null}), json!({})] {
                let request =
                    serde_json::from_value(json!({"op":"settings","patch":patch})).unwrap();
                assert!(app.control_request(request).ok);
                assert_eq!(
                    serde_json::to_value(&app.settings).unwrap(),
                    expected_settings
                );
            }
            let deadline = Instant::now() + Duration::from_secs(2);
            while !app.recovery_session_path.is_file() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            let saved: Session =
                serde_json::from_slice(&fs::read(&app.recovery_session_path).unwrap()).unwrap();
            assert_eq!(
                serde_json::to_value(saved.settings.unwrap()).unwrap(),
                expected_settings
            );
            drop(app);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn invalid_font_size_patch_does_not_apply_a_valid_font_choice() {
        let (mut app, _) = app_with_text("unchanged");
        let before = serde_json::to_value(&app.settings).unwrap();
        let response = app.control_request(Request::Settings {
            patch: Some(SettingsPatch {
                editor_font: Some(cli::EditorFont::Consolas),
                font_size: Some(33.0),
                autosave: Some(false),
                ..Default::default()
            }),
        });
        assert!(!response.ok);
        assert_eq!(response.error.unwrap().code, "invalid_request");
        assert_eq!(serde_json::to_value(&app.settings).unwrap(), before);
        assert_eq!(app.tab().unwrap().text, "unchanged");
        assert_eq!(app.tab().unwrap().revision, 0);
    }

    #[test]
    fn invalid_comfort_settings_leave_every_preference_unchanged() {
        for mut patch in [
            SettingsPatch {
                ui_scale: Some(2.01),
                ..Default::default()
            },
            SettingsPatch {
                ui_scale: Some(f32::NAN),
                ..Default::default()
            },
            SettingsPatch {
                line_height: Some(0.9),
                ..Default::default()
            },
            SettingsPatch {
                line_height: Some(f32::INFINITY),
                ..Default::default()
            },
            SettingsPatch {
                autosave_delay_ms: Some(299),
                ..Default::default()
            },
            SettingsPatch {
                autosave_delay_ms: Some(5001),
                ..Default::default()
            },
        ] {
            let (mut app, _) = app_with_text("unchanged");
            let before = serde_json::to_value(&app.settings).unwrap();
            patch.word_wrap = Some(true);
            patch.show_ignored_files = Some(true);
            let response = app.control_request(Request::Settings { patch: Some(patch) });
            assert!(!response.ok);
            assert_eq!(response.error.unwrap().code, "invalid_request");
            assert_eq!(serde_json::to_value(&app.settings).unwrap(), before);
            assert_eq!(app.tab().unwrap().text, "unchanged");
        }
    }

    #[test]
    fn indentation_patch_is_atomic_and_commands_use_document_language() {
        let (mut app, id) = app_with_text("value\n");
        app.projects[0].tabs[0].path = Some(PathBuf::from("fixture.dart"));
        let response = app.control_request(Request::Command {
            tab_id: id,
            command: EditorCommand::Indent,
            expected_revision: 0,
        });
        assert!(response.ok);
        assert_eq!(app.tab().unwrap().text, "  value\n");
        let before = serde_json::to_value(&app.settings).unwrap();
        let response = app.control_request(Request::Settings {
            patch: Some(SettingsPatch {
                detect_indentation: Some(false),
                language_indent_widths: Some(HashMap::from([("python".into(), 9)])),
                ..Default::default()
            }),
        });
        assert!(!response.ok);
        assert_eq!(serde_json::to_value(&app.settings).unwrap(), before);
        let response = app.control_request(Request::Settings {
            patch: Some(SettingsPatch {
                detect_indentation: Some(false),
                language_indent_widths: Some(HashMap::from([("dart".into(), 4)])),
                language_indent_with_tabs: Some(HashMap::from([("dart".into(), true)])),
                ..Default::default()
            }),
        });
        assert!(response.ok);
        assert_eq!(app.tab().unwrap().text, "  value\n");
        let response = app.control_request(Request::Command {
            tab_id: id,
            command: EditorCommand::Outdent,
            expected_revision: 1,
        });
        assert!(response.ok);
        assert_eq!(app.tab().unwrap().text, "value\n");
        let response = app.control_request(Request::Command {
            tab_id: id,
            command: EditorCommand::Indent,
            expected_revision: 2,
        });
        assert!(response.ok);
        assert_eq!(app.tab().unwrap().text, "\tvalue\n");
    }

    #[test]
    fn invalid_exclusion_patch_changes_no_preferences_or_search_and_valid_change_cancels_old_results()
     {
        let root = temporary_directory("search-settings");
        let (mut app, _) = app_with_text("64");
        app.recovery_session_path = root.join("session.json");
        app.settings.swap_jk = false;
        app.settings.search_excludes = vec!["generated".into()];
        app.project_query = "needle".into();
        app.project_search = Some(workspace::SearchResults::default());
        app.search_running = true;
        let cancel = Arc::new(AtomicBool::new(false));
        app.search_cancel = cancel.clone();
        let generation = app.search_generation;
        let before = serde_json::to_value(&app.settings).unwrap();
        let response = app.control_request(Request::Settings {
            patch: Some(SettingsPatch {
                swap_jk: Some(true),
                autosave: Some(false),
                search_excludes: Some(vec!["[unclosed".into()]),
                ..Default::default()
            }),
        });
        assert!(!response.ok);
        assert_eq!(response.error.unwrap().code, "invalid_request");
        assert_eq!(serde_json::to_value(&app.settings).unwrap(), before);
        assert_eq!(app.search_generation, generation);
        assert!(app.project_search.is_some() && app.search_running);
        assert!(!cancel.load(Ordering::Relaxed));
        let response = app.control_request(Request::Settings {
            patch: Some(SettingsPatch {
                search_excludes: Some(Vec::new()),
                ..Default::default()
            }),
        });
        assert!(response.ok);
        assert!(app.settings.search_excludes.is_empty());
        assert!(!app.settings.swap_jk && app.settings.autosave);
        assert!(cancel.load(Ordering::Relaxed));
        assert_eq!(app.search_generation, generation.wrapping_add(1));
        assert!(app.project_search.is_none() && !app.search_running && app.search_dirty);
        let generation = app.search_generation;
        let response = app.control_request(Request::Settings {
            patch: Some(SettingsPatch {
                search_excludes: Some(Vec::new()),
                ..Default::default()
            }),
        });
        assert!(response.ok);
        assert_eq!(
            app.search_generation, generation,
            "unchanged filters should not restart search"
        );
        // Every settings change persists the session in the background. Wait
        // for all of those writes, not only the first, before removing their
        // folder; a later write would otherwise recreate a temporary file.
        assert!(app.recovery_writer.wait_idle(Duration::from_secs(10)));
        assert!(app.recovery_session_path.is_file());
        drop(app);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn new_named_draft_and_save_as_never_overwrite_existing_file() {
        let root = temporary_directory("new");
        let (mut app, id) = app_with_text("keep");
        let path = root.join("draft.md");
        let response = app.control_request(Request::New {
            title: None,
            text: "# Draft".into(),
            path: Some(path.clone()),
        });
        assert!(response.ok);
        assert_eq!(response.result["created_on_disk"], false);
        assert!(!path.exists());
        assert_eq!(app.tab().unwrap().text, "# Draft");
        assert!(app.tab().unwrap().agent_unsaved);
        fs::write(&path, "valuable").unwrap();
        let response = app.control_request(Request::SaveAs {
            tab_id: id,
            expected_revision: 0,
            path: path.clone(),
        });
        assert_eq!(response.error.unwrap().code, "destination_exists");
        assert_eq!(fs::read_to_string(path).unwrap(), "valuable");
        assert_eq!(app.projects[0].tabs[0].text, "keep");
        assert!(!app.projects[0].tabs[0].saving);
        drop(app);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn new_agent_drafts_write_recovery_without_a_followup_edit() {
        for (named, text) in [(false, "Unsaved 🦀 draft"), (true, "# Draft"), (true, "")] {
            let root = temporary_directory("new-recovery");
            let (mut app, _) = app_with_text("");
            app.projects[0].root = root.clone();
            app.recovery_session_path = root.join("session.json");
            let destination = root.join("draft.md");
            let response = app.control_request(Request::New {
                title: None,
                text: text.into(),
                path: named.then(|| destination.clone()),
            });
            assert!(response.ok, "{response:?}");
            let recovery_path = root.join("draft-recovery.txt");
            let tab = app.tab_mut().unwrap();
            tab.recovery_path = recovery_path.clone();
            tab.last_edit = Instant::now() - Duration::from_secs(2);
            app.autosave();
            assert!(app.tab().unwrap().recovery_pending);
            assert!(!app.tab().unwrap().saving);
            let deadline = Instant::now() + Duration::from_secs(2);
            while app.tab().unwrap().recovery_pending && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
                app.poll();
            }
            let tab = app.tab().unwrap();
            assert!(!tab.recovery_pending);
            assert_eq!(tab.recovery_revision, tab.revision);
            assert!(tab.dirty && tab.agent_unsaved);
            let recovered = crate::recovery::read_document(&recovery_path, MAX_DOCUMENT).unwrap();
            assert_eq!(recovered.text, text);
            assert!(!destination.exists());
            drop(app);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn native_mode_agent_edit_and_history_commands_use_actual_textedit_history() {
        let (mut app, id) = app_with_text("64");
        app.settings.vim = false;
        let response = app.control_request(request_edit(id, 0, vec![edit(1, 1, 3, "96")]));
        assert!(response.ok);
        assert_eq!(response.result["tab"]["can_undo"], true);
        assert_eq!(response.result["tab"]["mode"], "native");
        let response = app.control_request(request_command(id, 1, EditorCommand::Undo));
        assert!(response.ok);
        assert_eq!(response.result["changed"], true);
        assert_eq!(app.tab().unwrap().text, "64");
        assert_eq!(response.result["selection"]["can_redo"], true);
        let response = app.control_request(request_command(id, 2, EditorCommand::Redo));
        assert!(response.ok);
        assert_eq!(app.tab().unwrap().text, "96");
        assert!(app.tab().unwrap().agent_unsaved);
    }

    #[test]
    fn readonly_batch_remains_available_during_user_dialog_without_touching_focus() {
        let (mut app, id) = app_with_text("64");
        app.settings_open = true;
        let response = app.control_request(Request::Batch {
            actions: vec![
                Request::Read {
                    tab_id: id,
                    start_line: 1,
                    start_column: 1,
                    line_count: 1,
                },
                Request::Settings { patch: None },
            ],
            stop_on_error: true,
        });
        assert!(response.ok, "{response:?}");
        assert_eq!(response.result["complete"], true);
        assert_eq!(
            response.result["items"][0]["response"]["result"]["text"],
            "64"
        );
        assert!(app.settings_open);
        assert_eq!(app.tab().unwrap().text, "64");
        assert_eq!(app.tab().unwrap().vim.cursor, 0);
    }

    #[test]
    fn large_batch_reads_keep_revision_evidence_and_explicitly_omit_detail() {
        let (mut app, id) = app_with_text(&"a".repeat(200_000));
        let response = app.control_request(Request::Batch {
            actions: vec![Request::Read {
                tab_id: id,
                start_line: 1,
                start_column: 1,
                line_count: 1,
            }],
            stop_on_error: true,
        });
        assert!(response.ok);
        assert_eq!(response.result["complete"], true);
        let result = &response.result["items"][0]["response"]["result"];
        assert_eq!(result["details_omitted"], true);
        assert_eq!(result["tab_id"], id);
        assert_eq!(result["revision"], 0);
        assert_eq!(result["dirty"], false);
        assert!(result.get("text").is_none());
        assert!(serde_json::to_vec(&response).unwrap().len() < 1024);
        assert_eq!(app.tab().unwrap().text.len(), 200_000);
    }

    fn temporary_directory(label: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let path = std::env::temp_dir().join(format!(
            "vedit-document-control-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }
}

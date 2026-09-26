//! GUI-thread adapter for the external control protocol.
use super::*;
use crate::cli::{self, Annotation, DocumentView, Location, OpenFile, Panel, Request, Response};
use serde_json::{Value, json};

#[path = "control_documents.rs"]
mod documents;
#[path = "control_queries.rs"]
mod queries;

impl MorrowApp {
    pub(super) fn start_control(&mut self) {
        match cli::Server::start(self.ctx.clone()) {
            Ok(server) => self.control = Some(server),
            Err(error) => self.status(format!("Agent controls unavailable: {error}"), true),
        }
    }

    pub(super) fn poll_control(&mut self) {
        for _ in 0..8 {
            let Some(envelope) = self.control.as_ref().and_then(cli::Server::try_recv) else {
                break;
            };
            if let Err(error) = cli::validate(&envelope.request) {
                let _ = envelope
                    .reply
                    .send(Response::failure("invalid_request", error));
                continue;
            }
            if self.control_query_async(&envelope.request, envelope.reply.clone()) {
                continue;
            }
            let response = self.control_request(envelope.request);
            let _ = envelope.reply.send(response);
        }
    }

    pub(super) fn apply_pending_agent_locations(&mut self) {
        let mut located = Vec::new();
        for tab in self
            .projects
            .iter_mut()
            .flat_map(|project| &mut project.tabs)
        {
            if tab.loading {
                continue;
            }
            if let Some(location) = tab.pending_agent_location.take() {
                let action = tab.vim.finish_insert(&mut tab.text);
                if action.text_changed {
                    tab.changed();
                }
                match character_offset(&tab.text, &location) {
                    Ok(offset) if tab.error.is_none() => {
                        tab.set_cursor(offset..offset);
                        tab.focus = tab.agent_view == DocumentView::Source;
                        tab.agent_location_error = None;
                        located.push(tab.id);
                    }
                    Ok(_) => {
                        tab.agent_location_error = Some(
                            "The file did not load; the requested location was not applied".into(),
                        )
                    }
                    Err(error) => tab.agent_location_error = Some(error),
                }
            }
            if !tab.annotations.is_empty() && tab.annotations_revision.is_none() {
                tab.annotations_revision = Some(tab.revision);
                if let Some(error) = tab
                    .annotations
                    .iter()
                    .find_map(|annotation| validate_annotation(&tab.text, annotation).err())
                {
                    tab.agent_location_error = Some(error);
                }
            }
        }
        for id in located {
            self.sync_external_document_navigation(id);
        }
    }

    pub(super) fn control_request(&mut self, request: Request) -> Response {
        if let Err(error) = cli::validate(&request) {
            return Response::failure("invalid_request", error);
        }
        let observation = control_observation(&request);
        let dialog = self.control_dialog();
        let closing_settings = matches!(
            request,
            Request::Panel {
                panel: Panel::Settings,
                visible: false
            }
        ) && dialog == Some("settings");
        let own_marks_dialog =
            matches!(request, Request::AgentMarks { .. }) && dialog == Some("agent_marks");
        if !observation
            && !closing_settings
            && !own_marks_dialog
            && let Some(dialog) = dialog
        {
            return Response::failure(
                "user_dialog_open",
                format!(
                    "The user's {dialog} interaction is active. No control action was applied; inspect status after the user finishes it."
                ),
            );
        }
        if let Some(tab_id) = match &request {
            Request::Edit { tab_id, .. }
            | Request::Replace { tab_id, .. }
            | Request::Command { tab_id, .. }
            | Request::SaveAs { tab_id, .. }
            | Request::Selection {
                tab_id,
                range: Some(_),
                ..
            } => Some(*tab_id),
            _ => None,
        } && let Some((project, index)) = self.control_find_tab(tab_id)
            && self.projects[project].tabs[index]
                .path
                .as_ref()
                .is_some_and(|path| self.context_file_path_pending(path))
        {
            return Response::failure(
                "file_operation_pending",
                "This document is being renamed. Inspect file_jobs/status and wait for the operation before editing or saving; no control action applied",
            );
        }
        let result = match request {
            Request::AgentMarks { action } => return self.control_agent_marks(action),
            Request::Status {} => self.control_status(),
            Request::Tabs { limit, offset } => self.control_tabs(limit, offset),
            Request::Open { files } => self.control_open(files, None),
            Request::Review { title, note, files } => self.control_open(files, Some((title, note))),
            Request::Focus {
                tab_id,
                location,
                view,
            } => {
                let Some((project, index)) = self.control_find_tab(tab_id) else {
                    return unknown_tab(tab_id);
                };
                let tab = &self.projects[project].tabs[index];
                if view == Some(DocumentView::Preview)
                    && !tab.path.as_ref().is_some_and(|path| cli::is_markdown(path))
                {
                    return Response::failure(
                        "preview_requires_markdown",
                        "This tab is not a Markdown document. Use source, or open a Markdown file.",
                    );
                }
                if !tab.loading
                    && let Some(location) = &location
                    && let Err(error) = character_offset(&tab.text, location)
                {
                    return Response::failure("invalid_location", error);
                }
                self.set_active_project_context(project);
                self.focus_existing_project_tab(project, index);
                self.activate_tab(project, index);
                if let Some(view) = view {
                    self.set_document_view(view);
                }
                if let Some(location) = location {
                    let tab = &mut self.projects[project].tabs[index];
                    tab.pending_agent_location = Some(location);
                    tab.agent_location_error = None;
                }
                self.apply_pending_agent_locations();
                self.raise_control_window();
                json!({"applied":true,"tab":self.control_tab_value(project,index)})
            }
            Request::Close { tab_ids } => {
                let mut items = Vec::new();
                for id in tab_ids {
                    let Some((project, index)) = self.control_find_tab(id) else {
                        items.push(item_error(id, "unknown_tab", "Tab is no longer open"));
                        continue;
                    };
                    let tab = &self.projects[project].tabs[index];
                    if tab.dirty || tab.saving {
                        items.push(item_error(id,"unsaved_changes","Tab has unsaved changes or a save in progress; close it in the app or save it explicitly first"));
                    } else {
                        self.perform_close(CloseAction::Tab(id));
                        items.push(json!({"tab_id":id,"ok":true,"closed":true}));
                    }
                }
                self.control_prune_review();
                batch_result(items)
            }
            Request::ClearMarks { tab_ids } => {
                let all = tab_ids.is_empty();
                let ids: Vec<_> = if all {
                    self.projects
                        .iter()
                        .flat_map(|p| p.tabs.iter().map(|t| t.id))
                        .collect()
                } else {
                    tab_ids
                };
                let mut items = Vec::new();
                for id in ids {
                    if let Some(tab) = self.tab_by_id_mut(id) {
                        tab.annotations.clear();
                        tab.annotations_revision = None;
                        tab.agent_location_error = None;
                        items.push(json!({"tab_id":id,"ok":true,"annotation_count":0}));
                    } else {
                        items.push(item_error(id, "unknown_tab", "Tab is no longer open"));
                    }
                }
                if all {
                    self.review = None;
                }
                self.control_prune_review();
                batch_result(items)
            }
            Request::Workspace { path } => {
                let path = match fs::canonicalize(&path) {
                    Ok(path) if path.is_dir() => path,
                    _ => {
                        return Response::failure(
                            "invalid_workspace",
                            format!("Not an accessible directory: {}", path.display()),
                        );
                    }
                };
                if let Some(tab) = self.tab_mut() {
                    let action = tab.vim.finish_insert(&mut tab.text);
                    if action.text_changed {
                        tab.changed();
                    }
                }
                self.add_project(path);
                self.raise_control_window();
                json!({"applied":true,"workspace":self.project().map(|p|crate::paths::display(&p.root)),"indexing":self.project().is_some_and(|p|p.indexing)})
            }
            Request::Panel { panel, visible } => {
                match panel {
                    Panel::Explorer | Panel::Search | Panel::SourceControl | Panel::Review => {
                        self.sidebar = match panel {
                            Panel::Explorer => SidebarView::Explorer,
                            Panel::Search => SidebarView::Search,
                            Panel::SourceControl => SidebarView::SourceControl,
                            _ => SidebarView::Review,
                        };
                        self.sidebar_visible = visible;
                    }
                    Panel::Output => self.output_visible = visible,
                    Panel::Problems => {
                        self.problems_visible = visible;
                        if visible {
                            self.output_visible = true;
                        }
                    }
                    Panel::Settings => self.settings_open = visible,
                }
                self.session_dirty = true;
                json!({"applied":true,"panel":panel,"visible":visible})
            }
            Request::Read {
                tab_id,
                start_line,
                line_count,
            } => {
                let Some((project, index)) = self.control_find_tab(tab_id) else {
                    return unknown_tab(tab_id);
                };
                let tab = &self.projects[project].tabs[index];
                if tab.loading {
                    return Response::failure(
                        "loading",
                        "The tab is still loading. Inspect status, then read it once loading is false.",
                    );
                }
                if let Some(error) = &tab.error {
                    return Response::failure("file_error", error);
                }
                let lines: Vec<_> = tab.text.split('\n').collect();
                if start_line > lines.len() {
                    return Response::failure(
                        "invalid_location",
                        format!(
                            "Document has {} lines; requested line {start_line}",
                            lines.len()
                        ),
                    );
                }
                let end = (start_line - 1).saturating_add(line_count).min(lines.len());
                let mut text = lines[start_line - 1..end].join("\n");
                let truncated = text.len() > 262_144;
                if truncated {
                    truncate_utf8(&mut text, 262_144);
                }
                json!({"tab_id":tab_id,"revision":tab.revision,"dirty":tab.dirty,"start_line":start_line,"end_line":end,"total_lines":lines.len(),"text":text,"truncated":truncated,"next_line":if !truncated && end<lines.len(){Some(end+1)}else{None}})
            }
            Request::Save { tab_ids } => {
                let mut items = Vec::new();
                for id in tab_ids {
                    let Some((project, index)) = self.control_find_tab(id) else {
                        items.push(item_error(id, "unknown_tab", "Tab is no longer open"));
                        continue;
                    };
                    let tab = &self.projects[project].tabs[index];
                    if tab
                        .path
                        .as_ref()
                        .is_some_and(|path| self.context_file_path_pending(path))
                    {
                        items.push(item_error(
                            id,
                            "file_operation_pending",
                            "Document is being renamed; wait for file_job completion before saving",
                        ));
                        continue;
                    }
                    if tab.path.is_none() {
                        items.push(item_error(
                            id,
                            "untitled_tab",
                            "Use Save as in the app to choose a path for this untitled tab",
                        ));
                        continue;
                    }
                    if tab.loading {
                        items.push(item_error(
                            id,
                            "loading",
                            "File has not loaded; save was not started",
                        ));
                        continue;
                    }
                    if let Some(error) = &tab.error {
                        items.push(item_error(id, "file_error", error));
                        continue;
                    }
                    self.save_tab(id, None);
                    let tab = &self.projects[project].tabs[index];
                    items
                        .push(json!({"tab_id":id,"ok":true,"saving":tab.saving,"dirty":tab.dirty}));
                }
                batch_result(items)
            }
            Request::Raise {} => {
                self.raise_control_window();
                json!({"applied":true,"active_tab":self.tab().map(|t|t.id)})
            }
            other => {
                if let Some(response) = self.control_query(&other) {
                    return response;
                }
                let response = self.control_document_request(other);
                self.ctx.request_repaint();
                return response;
            }
        };
        self.ctx.request_repaint();
        Response::success(result)
    }

    fn control_open(&mut self, files: Vec<OpenFile>, review: Option<(String, String)>) -> Value {
        // Validate all ready-buffer constraints before changing the previous review.
        // File-system failures remain independent per-item failures.
        let mut prepared = Vec::new();
        let mut items = vec![Value::Null; files.len()];
        let mut seen = std::collections::HashSet::new();
        for (ordinal, file) in files.into_iter().enumerate() {
            if self.context_file_path_pending(&file.path) {
                items[ordinal] = json!({"path":crate::paths::display(&file.path),"ok":false,"error":{"code":"file_operation_pending","message":"A rename affects this path. Wait for file_job completion before opening or refreshing it"}});
                continue;
            }
            let path = match fs::canonicalize(&file.path) {
                Ok(path) if path.is_file() => path,
                _ => {
                    items[ordinal] = json!({"path":crate::paths::display(&file.path),"ok":false,"error":{"code":"file_not_found","message":"Not an accessible file; no tab was opened"}});
                    continue;
                }
            };
            if self.context_file_path_pending(&path) {
                items[ordinal] = json!({"path":crate::paths::display(&path),"ok":false,"error":{"code":"file_operation_pending","message":"A rename affects this path. Wait for file_job completion before opening or refreshing it"}});
                continue;
            }
            if !seen.insert(path.clone()) {
                items[ordinal] = json!({"path":crate::paths::display(&path),"ok":false,"error":{"code":"duplicate_path","message":"This canonical file already appears in the batch; its first entry is authoritative and this entry was not applied"}});
                continue;
            }
            if let Some((project, index)) =
                self.projects.iter().enumerate().find_map(|(p, project)| {
                    project
                        .tabs
                        .iter()
                        .position(|t| t.path.as_ref() == Some(&path))
                        .map(|t| (p, t))
                })
            {
                let tab = &self.projects[project].tabs[index];
                // An external presentation reloads a clean buffer first, so
                // its coordinates must be checked against the new disk text.
                if !tab.loading && (tab.dirty || tab.saving) {
                    let error = file
                        .location
                        .as_ref()
                        .and_then(|location| character_offset(&tab.text, location).err())
                        .or_else(|| {
                            file.annotations.iter().find_map(|annotation| {
                                validate_annotation(&tab.text, annotation).err()
                            })
                        });
                    if let Some(error) = error {
                        items[ordinal] = json!({"path":crate::paths::display(&path),"ok":false,"error":{"code":"invalid_location","message":error}});
                        continue;
                    }
                }
            }
            prepared.push((ordinal, file, path));
        }
        let is_review = review.is_some();
        if is_review && !prepared.is_empty() {
            for tab in self.projects.iter_mut().flat_map(|p| &mut p.tabs) {
                tab.annotations.clear();
                tab.annotations_revision = None;
                tab.agent_location_error = None;
            }
        }
        let mut review_ids = Vec::new();
        let mut first = None;
        for (ordinal, file, path) in prepared {
            let existing = self.projects.iter().enumerate().find_map(|(pi, p)| {
                p.tabs
                    .iter()
                    .position(|t| t.path.as_ref() == Some(&path))
                    .map(|ti| (pi, ti))
            });
            let target = existing.map(|(pi, _)| pi).or_else(|| {
                self.projects
                    .iter()
                    .enumerate()
                    .filter(|(_, project)| path.starts_with(&project.root))
                    .max_by_key(|(_, project)| project.root.components().count())
                    .map(|(index, _)| index)
            });
            if let Some(index) = target {
                self.switch_project(index);
            } else if let Some(parent) = path.parent() {
                if let Some(tab) = self.tab_mut() {
                    let action = tab.vim.finish_insert(&mut tab.text);
                    if action.text_changed {
                        tab.changed();
                    }
                }
                self.add_project(parent.to_owned());
            }
            if let Some((project, index)) = existing {
                self.focus_existing_project_tab(project, index);
                self.activate_tab(project, index);
            } else {
                self.open_file(path.clone());
            }
            let project = self.active_project;
            let index = self.projects[project].active_tab;
            self.set_document_view(file.view);
            let refresh = existing.is_some()
                && self
                    .tab()
                    .is_some_and(|tab| !tab.dirty && !tab.saving && !tab.loading);
            if refresh {
                let tab = &mut self.projects[project].tabs[index];
                tab.loading = true;
                let id = tab.id;
                self.load_tab(id, Some(path.clone()), None);
            }
            let tab = &mut self.projects[project].tabs[index];
            let location = file.location.or_else(|| {
                file.annotations
                    .first()
                    .map(|annotation| annotation.start.clone())
            });
            if let Some(location) = location {
                tab.pending_agent_location = Some(location);
                tab.agent_location_error = None;
            }
            if !file.annotations.is_empty() || is_review {
                tab.annotations = file.annotations;
                for (number, annotation) in tab.annotations.iter_mut().enumerate() {
                    if annotation.id.is_empty() {
                        annotation.id = format!("{}-{}", tab.id, number + 1);
                    }
                }
                tab.annotations_revision = None;
                tab.agent_location_error = None;
            }
            review_ids.push(tab.id);
            first.get_or_insert((project, index));
            items[ordinal] = json!({"path":crate::paths::display(&path),"ok":true,"tab":self.control_tab_value(project,index)});
        }
        self.apply_pending_agent_locations();
        if let Some((title, note)) = review
            && !review_ids.is_empty()
        {
            self.review = Some(cli::ReviewSummary {
                title,
                note,
                files: review_ids,
            });
            self.sidebar = SidebarView::Review;
            self.sidebar_visible = true;
            if let Some((project, index)) = first {
                self.set_active_project_context(project);
                self.focus_existing_project_tab(project, index);
                self.activate_tab(project, index);
            }
        }
        if first.is_some() {
            self.raise_control_window();
        }
        let mut result = batch_result(items);
        result["applied"] = json!(first.is_some());
        result["active_tab"] = json!(self.tab().map(|t| t.id));
        result["review"] = json!(self.review);
        result
    }

    fn control_status(&self) -> Value {
        json!({"protocol":1,"pid":std::process::id(),"session":std::process::id().to_string(),"version":env!("CARGO_PKG_VERSION"),"active_tab":self.tab().map(|t|t.id),"active_workspace":self.project().map(|p|crate::paths::display(&p.root)),"file_jobs":self.context_file_jobs_status(),"vim_default":self.settings.vim,"settings":self.settings,"layout":self.layout_status(),"workspaces":self.projects.iter().map(|p|json!({"path":crate::paths::display(&p.root),"indexing":p.indexing,"index_error":p.index_error,"rust_analysis_ready":p.language_ready,"rust_analysis_error":p.language_error})).take(100).collect::<Vec<_>>(),"tabs":self.control_tabs(100,0),"review":self.review,"agent_marks":{"total":self.agent_marks.items.len(),"last_batch_id":self.agent_marks.last_batch().map(|batch|batch.id),"delivery_pending":self.agent_marks.last_batch().is_some_and(|batch|!batch.acknowledged),"restore_count":self.agent_marks.restore_count()},"dialog":self.control_dialog(),"task":{"status":self.task_status,"running":self.task.is_some()},"panels":{"sidebar":self.sidebar,"sidebar_visible":self.sidebar_visible,"output":self.output_visible,"problems":self.problems_visible,"settings":self.settings_open}})
    }
    fn control_tabs(&self, limit: usize, offset: usize) -> Value {
        let total = self.projects.iter().map(|p| p.tabs.len()).sum::<usize>();
        let items = self
            .projects
            .iter()
            .enumerate()
            .flat_map(|(project, p)| (0..p.tabs.len()).map(move |index| (project, index)))
            .skip(offset)
            .take(limit)
            .map(|(project, index)| self.control_tab_value(project, index))
            .collect::<Vec<_>>();
        json!({"items":items,"total":total,"offset":offset,"has_more":offset.saturating_add(limit)<total})
    }
    fn control_tab_value(&self, project: usize, index: usize) -> Value {
        let tab = &self.projects[project].tabs[index];
        let annotations = tab
            .annotations
            .iter()
            .take(20)
            .map(|a| {
                let mut value = json!(a);
                let mut note = a.note.clone();
                truncate_utf8(&mut note, 1024);
                value["note"] = json!(note);
                value
            })
            .collect::<Vec<_>>();
        let (line, column) = tab.lines.line_col(&tab.text, tab.revision, tab.vim.cursor);
        let (can_undo, can_redo) = if self.settings.vim {
            (tab.vim.can_undo(), tab.vim.can_redo())
        } else {
            self.context_native_history(tab.id)
        };
        json!({"id":tab.id,"path":tab.path.as_ref().map(|p|crate::paths::display(p)),"title":tab.title,"workspace":crate::paths::display(&self.projects[project].root),"active":project==self.active_project && index==self.projects[project].active_tab,"view":tab.agent_view,"loading":tab.loading,"saving":tab.saving,"dirty":tab.dirty,"autosave_paused":tab.agent_unsaved,"file_operation_pending":tab.path.as_ref().is_some_and(|path|self.context_file_path_pending(path)),"mode":if self.settings.vim{format!("{:?}",tab.vim.mode).to_lowercase()}else{"native".into()},"revision":tab.revision,"error":tab.error,"location":{"line":line,"column":column},"location_pending":tab.pending_agent_location,"location_error":tab.agent_location_error,"annotation_count":tab.annotations.len(),"annotations":annotations,"annotations_truncated":tab.annotations.len()>20,"annotations_stale":tab.annotations_revision.is_some_and(|revision|revision!=tab.revision),"can_undo":can_undo,"can_redo":can_redo})
    }
    fn control_find_tab(&self, id: u64) -> Option<(usize, usize)> {
        self.projects.iter().enumerate().find_map(|(project, p)| {
            p.tabs
                .iter()
                .position(|t| t.id == id)
                .map(|index| (project, index))
        })
    }
    pub(super) fn control_prune_review(&mut self) {
        let open: std::collections::HashSet<_> = self
            .projects
            .iter()
            .flat_map(|p| p.tabs.iter().map(|t| t.id))
            .collect();
        if let Some(review) = &mut self.review {
            review.files.retain(|id| open.contains(id));
            if review.files.is_empty() {
                self.review = None;
            }
        }
        if self.review.is_none() && self.sidebar == SidebarView::Review {
            self.sidebar = SidebarView::Explorer;
            self.session_dirty = true;
        }
    }
    fn raise_control_window(&self) {
        self.ctx
            .send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        self.ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }
    fn control_dialog(&self) -> Option<&'static str> {
        if self.close_action.is_some() {
            Some("confirm_close")
        } else if self.save_as.is_some() {
            Some("save_as")
        } else if self.picker.is_some() {
            Some("file_picker")
        } else if self.settings_open {
            Some("settings")
        } else if self.agent_marks_modal_open() {
            Some("agent_marks")
        } else if self.palette.is_some() {
            Some("palette")
        } else if self.workspace_picker_open() {
            Some("project_picker")
        } else if self.diff.is_some() {
            Some("diff")
        } else if self.git_detail_open() {
            Some("git_commit")
        } else if self.navigation_modal_open() {
            Some("bookmarks")
        } else if self.navigation_pending() {
            Some("vim_leader")
        } else if self.screen_jump.is_some() {
            Some("screen_jump")
        } else if self.vim_command.is_some() {
            Some("vim_command")
        } else if self.vim_search_active() {
            Some("vim_search")
        } else if self.tab_drag_active() {
            Some("tab_drag")
        } else if self.context_file_dialog.is_some() {
            Some("file_operation")
        } else {
            None
        }
    }
}

fn control_observation(request: &Request) -> bool {
    match request {
        Request::Batch { actions, .. } => actions.iter().all(control_observation),
        Request::AgentMarks { action } => matches!(
            action,
            cli::AgentMarksAction::List { .. }
                | cli::AgentMarksAction::Batch { .. }
                | cli::AgentMarksAction::Text { .. }
        ),
        Request::Status {}
        | Request::Tabs { .. }
        | Request::Read { .. }
        | Request::Raise {}
        | Request::Selection { range: None, .. }
        | Request::Settings { patch: None }
        | Request::Find { .. }
        | Request::Diff { .. }
        | Request::Diagnostics { .. }
        | Request::Search { .. }
        | Request::Output { .. }
        | Request::GitStatus { .. }
        | Request::Bookmarks {
            change: cli::BookmarkAction::List { .. },
        }
        | Request::FileJob { .. }
        | Request::Layout {
            change: cli::LayoutAction::Status {},
        } => true,
        _ => false,
    }
}
fn item_error(id: u64, code: &str, message: &str) -> Value {
    json!({"tab_id":id,"ok":false,"error":{"code":code,"message":message}})
}
fn unknown_tab(id: u64) -> Response {
    Response::failure(
        "unknown_tab",
        format!("Tab {id} is not open in this process. Inspect tabs/status and use a returned ID."),
    )
}
fn batch_result(items: Vec<Value>) -> Value {
    let complete = items.iter().all(|item| item["ok"] == true);
    json!({"complete":complete,"items":items})
}
fn truncate_utf8(text: &mut String, maximum: usize) {
    if text.len() > maximum {
        let mut end = maximum;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
}

/// Converts exact one-based coordinates without clamping or splitting CRLF.
pub(super) fn character_offset(text: &str, location: &Location) -> Result<usize, String> {
    if location.line == 0 || location.column == 0 {
        return Err("Lines and columns must be positive".into());
    }
    let mut offset = 0;
    let line_count = text.split('\n').count();
    for (index, line) in text.split('\n').enumerate() {
        if index + 1 == location.line {
            let content = if index + 1 < line_count {
                line.strip_suffix('\r').unwrap_or(line)
            } else {
                line
            };
            let length = content.chars().count();
            if location.column > length + 1 {
                return Err(format!(
                    "Line {} has {} characters; column {} is outside it",
                    location.line, length, location.column
                ));
            }
            return Ok(offset + location.column - 1);
        }
        offset += line.chars().count() + 1;
    }
    Err(format!(
        "Document has {} lines; requested line {}",
        text.split('\n').count(),
        location.line
    ))
}
fn validate_annotation(text: &str, annotation: &Annotation) -> Result<(), String> {
    character_offset(text, &annotation.start)
        .map_err(|error| format!("Mark {}: {error}", annotation.id))?;
    if let Some(end) = &annotation.end {
        character_offset(text, end).map_err(|error| format!("Mark {}: {error}", annotation.id))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_unicode_and_crlf_coordinates() {
        let text = "a😀\r\nβeta\n";
        assert_eq!(
            character_offset(text, &Location { line: 1, column: 3 }).unwrap(),
            2
        );
        assert_eq!(
            character_offset(text, &Location { line: 2, column: 2 }).unwrap(),
            5
        );
        assert_eq!(
            character_offset(text, &Location { line: 3, column: 1 }).unwrap(),
            9
        );
        assert!(character_offset(text, &Location { line: 1, column: 4 }).is_err());
        assert!(character_offset(text, &Location { line: 4, column: 1 }).is_err());
    }
    #[test]
    fn unknown_tab_never_uses_active_tab() {
        let mut app = MorrowApp::empty(egui::Context::default());
        app.projects.push(Project::new(std::env::temp_dir()));
        app.projects[0]
            .tabs
            .push(EditorTab::new(None, "unsaved".into()));
        let active = app.projects[0].tabs[0].id;
        let response = app.control_request(Request::Close {
            tab_ids: vec![u64::MAX],
        });
        assert!(response.ok);
        assert_eq!(response.result["complete"], false);
        assert_eq!(app.projects[0].tabs[0].id, active);
    }
    #[test]
    fn close_batch_preserves_dirty_buffer_and_closes_clean() {
        let mut app = MorrowApp::empty(egui::Context::default());
        let mut project = Project::new(std::env::temp_dir());
        let mut dirty = EditorTab::new(None, "dirty".into());
        dirty.text = "valuable".into();
        dirty.dirty = true;
        let dirty_id = dirty.id;
        let clean = EditorTab::new(None, "clean".into());
        let clean_id = clean.id;
        project.tabs = vec![dirty, clean];
        app.projects.push(project);
        let response = app.control_request(Request::Close {
            tab_ids: vec![dirty_id, clean_id],
        });
        assert_eq!(response.result["complete"], false);
        assert_eq!(app.projects[0].tabs.len(), 1);
        assert_eq!(app.projects[0].tabs[0].text, "valuable");
    }
    #[test]
    fn focus_outside_document_leaves_cursor_and_tab_unchanged() {
        let mut app = MorrowApp::empty(egui::Context::default());
        let mut project = Project::new(std::env::temp_dir());
        let mut tab = EditorTab::new(None, "text".into());
        tab.text = "hello".into();
        tab.saved_text = "hello".into();
        let id = tab.id;
        project.tabs.push(tab);
        app.projects.push(project);
        let response = app.control_request(Request::Focus {
            tab_id: id,
            location: Some(Location { line: 2, column: 1 }),
            view: None,
        });
        assert!(!response.ok);
        assert_eq!(app.tab().unwrap().vim.cursor, 0);
    }
    #[test]
    fn pending_loaded_location_is_observed_and_validated() {
        let mut app = MorrowApp::empty(egui::Context::default());
        let mut project = Project::new(std::env::temp_dir());
        let mut tab = EditorTab::new(None, "text".into());
        tab.loading = true;
        tab.pending_agent_location = Some(Location { line: 2, column: 2 });
        project.tabs.push(tab);
        app.projects.push(project);
        app.apply_pending_agent_locations();
        assert!(app.tab().unwrap().pending_agent_location.is_some());
        let tab = app.tab_mut().unwrap();
        tab.text = "one\n😀two".into();
        tab.loading = false;
        app.apply_pending_agent_locations();
        assert_eq!(app.tab().unwrap().vim.cursor, 5);
        assert!(app.tab().unwrap().pending_agent_location.is_none());
        assert!(app.tab().unwrap().agent_location_error.is_none());
    }
    #[test]
    fn loaded_invalid_marks_are_reported_separately_from_editor_error() {
        let mut app = MorrowApp::empty(egui::Context::default());
        let mut project = Project::new(std::env::temp_dir());
        let mut tab = EditorTab::new(None, "text".into());
        tab.text = "one".into();
        tab.annotations.push(Annotation {
            id: "important".into(),
            start: Location { line: 3, column: 1 },
            end: None,
            note: "look".into(),
            severity: cli::AnnotationSeverity::Info,
        });
        project.tabs.push(tab);
        app.projects.push(project);
        app.apply_pending_agent_locations();
        assert!(app.tab().unwrap().agent_location_error.is_some());
        assert!(app.tab().unwrap().error.is_none());
    }
    #[test]
    fn showing_problems_also_reveals_output_container() {
        let mut app = MorrowApp::empty(egui::Context::default());
        app.control_request(Request::Panel {
            panel: Panel::Problems,
            visible: true,
        });
        assert!(app.output_visible);
        assert!(app.problems_visible);
    }
    #[test]
    fn unrelated_close_preserves_review_and_last_review_close_restores_explorer() {
        let mut app = MorrowApp::empty(egui::Context::default());
        let mut project = Project::new(std::env::temp_dir());
        let plan = EditorTab::new(None, "plan.md".into());
        let plan_id = plan.id;
        let extra = EditorTab::new(None, "extra".into());
        let extra_id = extra.id;
        project.tabs = vec![plan, extra];
        app.projects.push(project);
        app.review = Some(cli::ReviewSummary {
            title: "Review".into(),
            note: String::new(),
            files: vec![plan_id],
        });
        app.sidebar = SidebarView::Review;
        app.control_request(Request::Close {
            tab_ids: vec![extra_id],
        });
        assert_eq!(app.review.as_ref().unwrap().files, vec![plan_id]);
        assert_eq!(app.sidebar, SidebarView::Review);
        app.control_request(Request::Close {
            tab_ids: vec![plan_id],
        });
        assert!(app.review.is_none());
        assert_eq!(app.sidebar, SidebarView::Explorer);
    }
    #[test]
    fn settings_overlay_blocks_presentation_and_is_reported() {
        let mut app = MorrowApp::empty(egui::Context::default());
        app.settings_open = true;
        let response = app.control_request(Request::Workspace {
            path: std::env::temp_dir(),
        });
        assert!(!response.ok);
        assert!(app.projects.is_empty());
        assert_eq!(app.control_status()["dialog"], "settings");
        let response = app.control_request(Request::Panel {
            panel: Panel::Settings,
            visible: false,
        });
        assert!(response.ok);
        assert!(!app.settings_open);
    }
    #[test]
    fn vim_search_allows_observation_but_preserves_user_state_against_control_mutations() {
        let mut app = MorrowApp::empty(egui::Context::default());
        let mut project = Project::new(std::env::temp_dir());
        let mut tab = EditorTab::new(None, "search fixture".into());
        tab.text = "abc foo 😀 bar\n".into();
        tab.saved_text = tab.text.clone();
        let id = tab.id;
        tab.vim.set_selection(&tab.text, 1..3);
        tab.selection = tab.vim.selection(&tab.text).unwrap();
        let origin = (tab.vim.mode, tab.vim.cursor, tab.selection.clone());
        project.tabs.push(tab);
        app.projects.push(project);
        app.bind_current_pane();
        let pane = app.current_pane_id();
        let mut search = std::mem::take(&mut app.find);
        search.start(app.tab_mut().unwrap(), pane, false);
        search.character(app.tab_mut().unwrap(), 'f');
        app.find = search;
        assert!(app.vim_search_active());
        assert_eq!(app.tab().unwrap().vim.cursor, 4);
        let preview = (
            app.tab().unwrap().vim.mode,
            app.tab().unwrap().vim.cursor,
            app.tab().unwrap().selection.clone(),
        );
        let layout = app.layout_status();
        let revision = app.tab().unwrap().revision;

        let status = app.control_request(Request::Status {});
        assert!(status.ok);
        assert_eq!(status.result["dialog"], "vim_search");
        let read = app.control_request(Request::Read {
            tab_id: id,
            start_line: 1,
            line_count: 10,
        });
        assert!(read.ok);
        assert_eq!(read.result["text"], "abc foo 😀 bar\n");
        let edit = Request::Edit {
            tab_id: id,
            expected_revision: revision,
            edits: Vec::new(),
            set_text: Some("external change".into()),
        };
        for mutation in [
            edit.clone(),
            Request::Layout {
                change: cli::LayoutAction::MoveTab {
                    source_pane_id: pane,
                    pane_id: pane,
                    tab_id: id,
                    index: 0,
                },
            },
        ] {
            let result = app.control_request(mutation);
            assert!(!result.ok);
            assert_eq!(result.error.unwrap().code, "user_dialog_open");
        }
        assert_eq!(app.layout_status(), layout);
        let tab = app.tab().unwrap();
        assert_eq!(tab.text, "abc foo 😀 bar\n");
        assert_eq!(tab.revision, revision);
        assert!(!tab.dirty);
        assert_eq!(
            (tab.vim.mode, tab.vim.cursor, tab.selection.clone()),
            preview
        );
        assert!(app.vim_search_active());

        // Continuing the real search verifies that its draft pattern survived.
        let mut search = std::mem::take(&mut app.find);
        search.character(app.tab_mut().unwrap(), 'o');
        app.find = search;
        assert_eq!(app.tab().unwrap().vim.cursor, 4);
        app.cancel_vim_search(true);
        assert!(!app.vim_search_active());
        let tab = app.tab().unwrap();
        assert_eq!(
            (tab.vim.mode, tab.vim.cursor, tab.selection.clone()),
            origin
        );
        assert!(app.control_request(edit).ok);
        assert_eq!(app.tab().unwrap().text, "external change");
    }
    #[test]
    fn canonical_duplicates_preserve_first_marks_and_batch_order() {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let root = std::env::temp_dir().join(format!(
            "morrow-cli-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("a.rs");
        fs::write(&path, "fn main() {}\n").unwrap();
        let mut app = MorrowApp::empty(egui::Context::default());
        let mut first = OpenFile::source(path.clone());
        first.annotations.push(Annotation {
            id: "first".into(),
            start: Location { line: 1, column: 1 },
            end: None,
            note: "Keep first".into(),
            severity: cli::AnnotationSeverity::Info,
        });
        let missing = OpenFile::source(root.join("missing.rs"));
        let duplicate = OpenFile::source(root.join(".").join("a.rs"));
        let response = app.control_request(Request::Open {
            files: vec![first, missing, duplicate],
        });
        assert_eq!(response.result["complete"], false);
        assert_eq!(response.result["items"][0]["ok"], true);
        assert_eq!(
            response.result["items"][1]["error"]["code"],
            "file_not_found"
        );
        assert_eq!(
            response.result["items"][2]["error"]["code"],
            "duplicate_path"
        );
        assert_eq!(app.projects[0].tabs.len(), 1);
        assert_eq!(app.tab().unwrap().annotations[0].note, "Keep first");
        let deadline = Instant::now() + Duration::from_secs(2);
        while app.tab().is_some_and(|t| t.loading) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
            app.poll();
        }
        drop(app);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn current_tab_navigation_commits_insert_as_one_undo_transaction() {
        let mut app = MorrowApp::empty(egui::Context::default());
        let mut project = Project::new(std::env::temp_dir());
        let mut tab = EditorTab::new(None, "text".into());
        tab.text = "ab".into();
        tab.saved_text = "ab".into();
        tab.vim.handle_key(&mut tab.text, VimKey::Char('i'));
        tab.text.insert(0, 'X');
        tab.vim.cursor = 1;
        tab.changed();
        let id = tab.id;
        project.tabs.push(tab);
        app.projects.push(project);
        let response = app.control_request(Request::Focus {
            tab_id: id,
            location: Some(Location { line: 1, column: 2 }),
            view: None,
        });
        assert!(response.ok);
        assert_eq!(app.tab().unwrap().vim.mode, VimMode::Normal);
        assert_eq!(app.tab().unwrap().vim.cursor, 1);
        let tab = app.tab_mut().unwrap();
        tab.vim.handle_key(&mut tab.text, VimKey::Char('u'));
        assert_eq!(tab.text, "ab");
    }
    #[test]
    fn fresh_review_reloads_clean_file_before_validating_marks() {
        let root =
            std::env::temp_dir().join(format!("morrow-review-reload-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("plan.md");
        fs::write(&path, "old\n").unwrap();
        let mut app = MorrowApp::empty(egui::Context::default());
        app.control_request(Request::Open {
            files: vec![OpenFile::source(path.clone())],
        });
        await_file(&mut app);
        let initial_id = app.tab().unwrap().id;
        fs::write(&path, "# New plan\n\nImportant new point\n").unwrap();
        let mut file = OpenFile::source(path);
        file.view = DocumentView::Preview;
        file.annotations.push(Annotation {
            id: "new-section".into(),
            start: Location { line: 3, column: 1 },
            end: None,
            note: "Discuss this".into(),
            severity: cli::AnnotationSeverity::Info,
        });
        let response = app.control_request(Request::Review {
            title: "Latest plan".into(),
            note: String::new(),
            files: vec![file],
        });
        assert!(response.ok);
        assert_eq!(response.result["complete"], true);
        assert_eq!(response.result["items"][0]["tab"]["loading"], true);
        await_file(&mut app);
        app.apply_pending_agent_locations();
        let tab = app.tab().unwrap();
        assert_eq!(tab.id, initial_id);
        assert!(tab.text.contains("Important new point"));
        assert_eq!(
            tab.lines
                .line_col(&tab.text, tab.revision, tab.vim.cursor)
                .0,
            3
        );
        assert_eq!(tab.annotations_revision, Some(tab.revision));
        assert!(tab.agent_location_error.is_none());
        drop(app);
        fs::remove_dir_all(root).unwrap();
    }
    fn await_file(app: &mut MorrowApp) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while app.tab().is_some_and(|t| t.loading) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
            app.poll();
        }
        assert!(!app.tab().unwrap().loading, "file worker did not complete");
    }
}

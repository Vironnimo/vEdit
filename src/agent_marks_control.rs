//! The application adapter for user-selected excerpts and durable handoff receipts.
use super::*;
use crate::agent_marks::{MarkDocument, MarkError, UserMark};
use crate::cli::{AgentMarksAction, Response};
use serde_json::{Value, json};

const PAGE_BYTES: usize = 256 * 1024;
const PREVIEW_CHARS: usize = 4096;

impl VEditApp {
    pub(super) fn sync_agent_marks(&mut self) {
        if self.agent_marks.items.is_empty() {
            return;
        }
        let mut changed = false;
        for project in &self.projects {
            for tab in &project.tabs {
                if tab.loading || tab.error.is_some() {
                    continue;
                }
                changed |= self.agent_marks.synchronize_document(
                    MarkDocument {
                        id: tab.id,
                        workspace: &project.root,
                        path: tab.path.as_deref(),
                        title: &tab.title,
                    },
                    &tab.text,
                    tab.revision,
                );
            }
        }
        self.session_dirty |= changed;
    }

    fn mark_document_index(&self, id: u64) -> Option<(usize, usize)> {
        self.projects.iter().enumerate().find_map(|(p, project)| {
            project
                .tabs
                .iter()
                .position(|tab| tab.id == id)
                .map(|t| (p, t))
        })
    }

    fn agent_mark_live_index(&self, mark: &UserMark) -> Option<(usize, usize)> {
        self.projects.iter().enumerate().find_map(|(p, project)| {
            if project.root != mark.target.workspace {
                return None;
            }
            project
                .tabs
                .iter()
                .position(|tab| match &mark.target.path {
                    Some(path) => tab.path.as_ref() == Some(path),
                    None => {
                        mark.has_runtime_identity()
                            && tab.id == mark.document_id
                            && tab.path.is_none()
                    }
                })
                .map(|t| (p, t))
        })
    }

    fn try_capture_agent_mark(&mut self, id: u64, note: Option<&str>) -> Result<u64, Response> {
        let Some((project, index)) = self.mark_document_index(id) else {
            return Err(Response::failure(
                "unknown_tab",
                "The document is no longer open; inspect tabs before selecting a block",
            ));
        };
        let project = &self.projects[project];
        let tab = &project.tabs[index];
        if tab.loading || tab.error.is_some() {
            return Err(Response::failure(
                "document_not_ready",
                "The document must finish loading before marking text",
            ));
        }
        if tab.agent_view != crate::cli::DocumentView::Source {
            return Err(Response::failure(
                "source_selection_required",
                "Select text in Source before marking a block for an agent",
            ));
        }
        let range = if self.settings.vim {
            tab.vim
                .selection(&tab.text)
                .unwrap_or_else(|| tab.selection.clone())
        } else {
            tab.selection.clone()
        };
        if range.is_empty() {
            return Err(Response::failure(
                "selection_required",
                "Select a block first, using the mouse or Vim v / V, then mark it for an agent",
            ));
        }
        let id = self
            .agent_marks
            .add(
                MarkDocument {
                    id: tab.id,
                    workspace: &project.root,
                    path: tab.path.as_deref(),
                    title: &tab.title,
                },
                &tab.text,
                tab.revision,
                range,
                note.unwrap_or_default(),
            )
            .map_err(mark_error)?;
        self.session_dirty = true;
        self.ctx.request_repaint();
        Ok(id)
    }

    pub(super) fn capture_agent_mark(&mut self) -> bool {
        let Some(id) = self.tab().map(|tab| tab.id) else {
            return false;
        };
        self.capture_agent_mark_for_tab(id, None)
    }

    pub(super) fn capture_agent_mark_for_tab(&mut self, id: u64, note: Option<&str>) -> bool {
        match self.try_capture_agent_mark(id, note) {
            Ok(_) => {
                self.status(
                    "Block marked for agent · Leader A opens the collection",
                    false,
                );
                true
            }
            Err(response) => {
                if let Some(error) = response.error {
                    self.status(error.message, true);
                }
                false
            }
        }
    }

    pub(super) fn agent_marks_remove(&mut self, id: u64) -> bool {
        let changed = self.agent_marks.remove(id);
        self.session_dirty |= changed;
        if changed {
            self.ctx.request_repaint();
        }
        changed
    }

    pub(super) fn agent_marks_clear(&mut self) -> usize {
        let count = self.agent_marks.clear();
        self.session_dirty |= count > 0;
        self.ctx.request_repaint();
        count
    }

    pub(super) fn agent_marks_restore(&mut self) -> usize {
        match self.agent_marks.restore(None) {
            Ok(count) => {
                self.session_dirty |= count > 0;
                self.sync_agent_marks();
                self.ctx.request_repaint();
                count
            }
            Err(error) => {
                self.status(error.to_string(), true);
                0
            }
        }
    }

    pub(super) fn agent_marks_set_note(&mut self, id: u64, note: &str) -> bool {
        match self.agent_marks.set_note(id, note) {
            Ok(changed) => {
                self.session_dirty |= changed;
                self.ctx.request_repaint();
                true
            }
            Err(error) => {
                self.status(error.to_string(), true);
                false
            }
        }
    }

    pub(super) fn navigate_agent_mark(&mut self, id: u64) -> Result<(), String> {
        self.sync_agent_marks();
        self.record_jump();
        let mark = self
            .agent_marks
            .get(id, None)
            .ok_or("This block was removed from the collection")?
            .clone();
        let open = self.agent_mark_live_index(&mark);
        if let Some((project, index)) = open {
            self.set_active_project_context(project);
            self.focus_existing_project_tab(project, index);
            self.activate_tab(project, index);
        } else if let Some(path) = mark.target.path.clone() {
            if self.context_path_busy(&path) {
                return Err(
                    "This file has a pending file operation; retry when it finishes".into(),
                );
            }
            self.add_project(mark.target.workspace.clone());
            self.open_file(path);
        } else {
            return Err("This unsaved document is closed; its captured text is still available to the agent".into());
        }
        let target_is_active = self.agent_mark_live_index(&mark).is_some_and(|(p, t)| {
            p == self.active_project
                && self.projects[p].tabs[t].id == self.tab().map_or(0, |tab| tab.id)
        });
        if !target_is_active {
            return Err(
                "The marked document could not be opened; retry after inspecting its path".into(),
            );
        }
        self.set_document_view(crate::cli::DocumentView::Source);
        if let Some(tab) = self.tab_mut() {
            if tab.loading {
                tab.pending_agent_location = Some(crate::cli::Location {
                    line: mark.start.line,
                    column: mark.start.column,
                });
            } else {
                let at = if mark.is_attached() {
                    mark.range.start.min(tab.text.chars().count())
                } else {
                    tab.lines
                        .position(&tab.text, tab.revision, mark.start.line, mark.start.column)
                        .unwrap_or_else(|| editor::go_to_line(&tab.text, mark.start.line))
                };
                tab.set_cursor(at..at);
            }
            tab.center_on_reveal = true;
            tab.scroll_to_cursor = true;
            tab.focus = true;
        }
        if mark.stale {
            self.status("Showing the original location · the captured block changed; its saved excerpt is retained", false);
        }
        Ok(())
    }

    fn agent_mark_value(&self, mark: &UserMark, current_collection: bool) -> Value {
        let live = self.agent_mark_live_index(mark).and_then(|(p, t)| {
            let tab = &self.projects[p].tabs[t];
            (tab.path == mark.target.path && !tab.loading && tab.error.is_none()).then_some(tab)
        });
        let text: String = mark.snippet.chars().take(PREVIEW_CHARS).collect();
        let total_chars = mark.range.end.saturating_sub(mark.range.start);
        let shown_chars = text.chars().count();
        let mut value = json!({"id":mark.id,"workspace":crate::paths::display(&mark.target.workspace),
            "path":mark.target.path.as_deref().map(crate::paths::display),"title":mark.target.title,
            "range":{"start":mark.start,"end":mark.end},"text":text,"text_chars":total_chars,
            "text_next_offset":(shown_chars<total_chars).then_some(shown_chars),"note":mark.note,
            "stale":mark.stale,"source_open":live.is_some(),"range_current":current_collection && mark.is_attached() && live.is_some(),
            "tab_id":live.map(|tab|tab.id),"revision":live.map(|tab|tab.revision),"dirty":live.map(|tab|tab.dirty)});
        if let Some(review) = &mark.review {
            // `range` stays a new-side navigation hint; location captures both sides.
            value["review"] = json!(review);
        }
        value
    }

    fn agent_marks_page(
        &self,
        items: &[UserMark],
        limit: usize,
        offset: usize,
        current_collection: bool,
    ) -> Value {
        let mut page = Vec::new();
        let mut bytes = 0;
        for mark in items.iter().skip(offset).take(limit) {
            let value = self.agent_mark_value(mark, current_collection);
            let size = serde_json::to_vec(&value).map_or(PAGE_BYTES, |bytes| bytes.len());
            if !page.is_empty() && bytes + size > PAGE_BYTES {
                break;
            }
            bytes += size;
            page.push(value);
        }
        let next = offset.saturating_add(page.len());
        json!({"items":page,"total":items.len(),"offset":offset,
            "next_offset":(next<items.len()).then_some(next),"remaining_current":self.agent_marks.items.len(),
            "batch_id":null,"request_id":null,"acknowledged":null})
    }

    pub(super) fn control_agent_marks(&mut self, action: AgentMarksAction) -> Response {
        self.sync_agent_marks();
        let durable = matches!(
            action,
            AgentMarksAction::Take { .. } | AgentMarksAction::Ack { .. }
        );
        // Retain the original live caches until the atomic session write succeeds.
        let previous = durable.then(|| {
            let previous = std::mem::take(&mut self.agent_marks);
            self.agent_marks = previous.clone();
            previous
        });
        let mut response = self.apply_agent_marks_action(action, durable);
        if let Some(mut previous) = previous {
            if response.ok {
                let saved = serde_json::to_vec(&self.session())
                    .map_err(|error| error.to_string())
                    .and_then(|bytes| {
                        self.recovery_writer.write(
                            &self.recovery_session_path,
                            self.recovery_writer.stamp(),
                            &bytes,
                        )
                    });
                match saved {
                    Ok(()) => {
                        self.agent_marks.preserve_live_caches_from(&mut previous);
                        response.result["persistence"] = json!("saved");
                    }
                    Err(error) => {
                        self.agent_marks = previous;
                        return Response::failure(
                            "persistence_failed",
                            format!(
                                "The handoff could not be saved; marks and the previous receipt are unchanged. Retry after resolving the session storage error: {error}"
                            ),
                        );
                    }
                }
            } else {
                self.agent_marks = previous;
            }
        }
        response
    }

    fn apply_agent_marks_action(&mut self, action: AgentMarksAction, durable: bool) -> Response {
        let mut changed = false;
        let result = match action {
            AgentMarksAction::List { limit, offset } => {
                let mut page = self.agent_marks_page(&self.agent_marks.items, limit, offset, true);
                page["last_batch_id"] = json!(self.agent_marks.last_batch().map(|batch| batch.id));
                page["restore_count"] = json!(self.agent_marks.restore_count());
                page
            }
            AgentMarksAction::Take { request_id, limit } => {
                let request_id = request_id.unwrap_or_else(new_request_id);
                let outcome = match self.agent_marks.take(&request_id) {
                    Ok(outcome) => outcome,
                    Err(error) => return mark_error(error),
                };
                changed = !outcome.replayed;
                let mut page = match outcome.batch_id {
                    Some(id) => self
                        .agent_marks_batch_page(Some(id), limit, 0)
                        .expect("The receipt just returned by the store must exist"),
                    None => self.agent_marks_page(&[], limit, 0, false),
                };
                page["applied"] = json!(true);
                page["replayed"] = json!(outcome.replayed);
                page["request_id"] = json!(request_id);
                page["last_batch_id"] = json!(self.agent_marks.last_batch().map(|batch| batch.id));
                page["persistence"] = json!("queued");
                page
            }
            AgentMarksAction::Batch {
                batch_id,
                limit,
                offset,
            } => match self.agent_marks_batch_page(batch_id, limit, offset) {
                Some(page) => page,
                None => {
                    return Response::failure(
                        "batch_not_found",
                        "No retained batch matches this ID; agent-marks lists current marks and the last batch ID",
                    );
                }
            },
            AgentMarksAction::Text {
                id,
                batch_id,
                offset,
                count,
            } => match self.agent_marks.snippet_chunk(id, batch_id, offset, count) {
                Ok(chunk) => {
                    json!({"id":id,"batch_id":batch_id,"text":chunk.text,"offset":chunk.offset,
                        "total_chars":chunk.total_chars,"next_offset":chunk.next_offset})
                }
                Err(error) => return mark_error(error),
            },
            AgentMarksAction::Ack { batch_id } => match self.agent_marks.ack(batch_id) {
                Ok(applied) => {
                    changed = applied;
                    json!({"applied":true,"batch_id":batch_id,"acknowledged":true,"persistence":"queued"})
                }
                Err(error) => return mark_error(error),
            },
            AgentMarksAction::Restore { batch_id } => match self.agent_marks.restore(batch_id) {
                Ok(count) => {
                    changed = count > 0;
                    self.sync_agent_marks();
                    json!({"applied":true,"restored":count,"total":self.agent_marks.items.len(),"persistence":"queued"})
                }
                Err(error) => return mark_error(error),
            },
            AgentMarksAction::Clear {} => {
                let removed = self.agent_marks_clear();
                changed = removed > 0;
                json!({"applied":true,"removed":removed,"persistence":"queued"})
            }
            AgentMarksAction::Remove { id } => {
                if !self.agent_marks_remove(id) {
                    return mark_error(MarkError::UnknownMark(id));
                }
                changed = true;
                json!({"applied":true,"id":id,"removed":true,"persistence":"queued"})
            }
            AgentMarksAction::Note { id, note } => match self.agent_marks.set_note(id, &note) {
                Ok(applied) => {
                    changed = applied;
                    json!({"applied":true,"id":id,"note":note,"persistence":"queued"})
                }
                Err(error) => return mark_error(error),
            },
            AgentMarksAction::Add { tab_id, note } => {
                let Some(tab_id) = tab_id.or_else(|| self.tab().map(|tab| tab.id)) else {
                    return Response::failure(
                        "no_active_document",
                        "Open a source document and select a block first",
                    );
                };
                match self.try_capture_agent_mark(tab_id, note.as_deref()) {
                    Ok(id) => {
                        changed = true;
                        json!({"applied":true,"id":id,"item":self.agent_mark_value(self.agent_marks.get(id,None).unwrap(), true),"persistence":"queued"})
                    }
                    Err(error) => return error,
                }
            }
        };
        if changed {
            self.session_dirty = true;
            if !durable {
                self.persist_session();
            }
            self.ctx.request_repaint();
        }
        Response::success(result)
    }

    fn agent_marks_batch_page(
        &self,
        batch_id: Option<u64>,
        limit: usize,
        offset: usize,
    ) -> Option<Value> {
        let batch = self.agent_marks.batch(batch_id)?;
        // A receipt freezes capture-time coordinates. It is not a live range
        // verification, even while its original document remains open.
        let mut page = self.agent_marks_page(&batch.items, limit, offset, false);
        page["batch_id"] = json!(batch.id);
        page["request_id"] = json!(batch.request_id);
        page["acknowledged"] = json!(batch.acknowledged);
        Some(page)
    }
}

fn mark_error(error: MarkError) -> Response {
    let mut response = Response::failure(error.code(), error.to_string());
    if let Some(id) = error.batch_id() {
        response.result = json!({"batch_id":id});
    }
    response
}

fn new_request_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    format!(
        "vedit-{}-{time}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

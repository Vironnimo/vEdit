//! Window-wide uppercase Vim marks. Positions stay with the owning Document;
//! closed files retain only their path and last logical position for reopening.
use super::*;
use crate::vim::GlobalMarkRequest;

#[derive(Clone)]
struct FileMark {
    document: u64,
    path: Option<PathBuf>,
    offset: usize,
    line: usize,
    column: usize,
    available: bool,
}

struct PendingMark {
    request: u64,
    command: GlobalMarkRequest,
    events: Vec<egui::Event>,
    pane: u64,
    source: u64,
    revision: u64,
    cursor: usize,
    mode: VimMode,
    target: Option<u64>,
    destination_pane: Option<u64>,
    macro_input: bool,
}

#[derive(Default)]
pub(super) struct FileMarks {
    marks: [Option<FileMark>; 26],
    pending: Option<PendingMark>,
    request: u64,
}

impl VEditApp {
    pub(super) fn cancel_macro_file_mark(&mut self) {
        if self
            .vim_file_marks
            .pending
            .as_ref()
            .is_some_and(|pending| pending.macro_input)
        {
            let pending = self.vim_file_marks.pending.take().unwrap();
            if pending.target.is_some_and(|target| {
                self.pending_jump
                    .as_ref()
                    .is_some_and(|(id, _)| *id == target)
            }) {
                self.pending_jump = None;
            }
        }
    }
    pub(super) fn vim_file_mark_pending(&self) -> bool {
        self.vim_file_marks.pending.is_some()
    }

    pub(super) fn refresh_vim_file_marks(&mut self) {
        for (index, mark) in self.vim_file_marks.marks.iter_mut().enumerate() {
            let Some(mark) = mark else { continue };
            let Some(tab) = self
                .projects
                .iter()
                .flat_map(|project| &project.tabs)
                .find(|tab| tab.id == mark.document)
            else {
                continue;
            };
            if tab.loading {
                continue;
            }
            mark.path.clone_from(&tab.path);
            let name = (b'A' + index as u8) as char;
            mark.available = tab.vim.global_mark_position(name).is_some();
            if let Some(offset) = tab.vim.global_mark_position(name) {
                mark.offset = offset;
                (mark.line, mark.column) = tab.lines.line_col(&tab.text, tab.revision, offset);
            }
        }
    }

    fn defer_file_mark_events(&self, document: u64, events: Vec<egui::Event>) {
        editor_input::defer(&self.ctx, document, events);
    }

    pub(super) fn handle_vim_file_mark(
        &mut self,
        command: GlobalMarkRequest,
        events: Vec<egui::Event>,
    ) {
        let macro_input = self.vim_macros.active();
        self.refresh_vim_file_marks();
        let Some(tab) = self.tab().filter(|tab| !tab.loading) else {
            return;
        };
        let source = tab.id;
        if let GlobalMarkRequest::Set(name) = command {
            let index = name as usize - 'A' as usize;
            let offset = tab.vim.cursor;
            let (line, column) = tab.lines.line_col(&tab.text, tab.revision, offset);
            self.vim_file_marks.marks[index] = Some(FileMark {
                document: source,
                path: tab.path.clone(),
                offset,
                line,
                column,
                available: true,
            });
            for tab in self
                .projects
                .iter_mut()
                .flat_map(|project| &mut project.tabs)
                .filter(|tab| tab.id != source)
            {
                tab.vim.set_global_mark_position(name, None);
            }
            self.defer_file_mark_events(source, events);
            return;
        }
        let GlobalMarkRequest::Jump { name, operator, .. } = command else {
            unreachable!()
        };
        let mark = self.vim_file_marks.marks[name as usize - 'A' as usize].clone();
        let Some(mark) = mark.filter(|mark| mark.available) else {
            if let Some(tab) = self.tab_mut() {
                tab.vim.cancel_pending_command();
            }
            self.vim_input_status(format!("Mark not set: {name}"));
            if !macro_input {
                self.defer_file_mark_events(source, events);
            }
            return;
        };
        if mark.document == source {
            let mut yanked = None;
            if let Some(tab) = self.tab_mut() {
                let action = tab
                    .vim
                    .resolve_global_mark(&mut tab.text, name, mark.offset);
                if action.text_changed {
                    tab.changed();
                }
                if !action.text_changed && action.yanked.is_some() {
                    if action.yanked.as_ref().is_some_and(|yank| yank.linewise) {
                        tab.flash_line_yank(action.yanked_ranges);
                    } else {
                        tab.flash_yank(action.yanked_ranges);
                    }
                }
                yanked = action.yanked;
                if let Some(message) = action.message {
                    tab.vim_message = Some(message);
                }
                let selection = tab
                    .vim
                    .selection(&tab.text)
                    .unwrap_or(tab.vim.cursor..tab.vim.cursor);
                tab.selection = selection.clone();
                tab.pending_selection = Some(selection);
                tab.scroll_to_cursor = true;
            }
            if let Some(yank) = yanked {
                self.register = Some(yank);
            }
            if let Some((bank, yank)) = self.tab().map(|tab| {
                (
                    tab.vim.register_bank().clone(),
                    tab.vim.yank_register().cloned(),
                )
            }) {
                self.vim_registers = bank;
                self.yank_register = yank;
            }
            self.record_vim_jumps();
            self.defer_file_mark_events(source, events);
            return;
        }
        if let Some(tab) = self.tab_mut() {
            tab.vim.cancel_pending_command();
        }
        if operator {
            self.vim_input_status("Cannot use a mark from another document with an operator");
            if !macro_input {
                self.defer_file_mark_events(source, events);
            }
            return;
        }
        self.vim_file_marks.request = self.vim_file_marks.request.wrapping_add(1);
        let request = self.vim_file_marks.request;
        let tab = self.tab().unwrap();
        self.vim_file_marks.pending = Some(PendingMark {
            request,
            command,
            events,
            pane: self.current_pane_id(),
            source,
            revision: tab.revision,
            cursor: tab.vim.cursor,
            mode: tab.vim.mode,
            target: None,
            destination_pane: None,
            macro_input,
        });
        let open = self
            .projects
            .iter()
            .flat_map(|project| &project.tabs)
            .any(|tab| tab.id == mark.document);
        if open {
            self.open_vim_file_mark(mark);
        } else if let Some(path) = mark.path {
            self.worker(move || WorkerResult::VimFileMarkPath {
                request,
                path: fs::metadata(&path)
                    .ok()
                    .filter(|metadata| metadata.is_file())
                    .map(|_| path),
            });
        } else {
            self.fail_vim_file_mark("The marked untitled document is no longer open");
        }
    }

    fn file_mark_source_valid(&self, pending: &PendingMark) -> bool {
        self.current_pane_id() == pending.pane
            && self.tab().is_some_and(|tab| {
                tab.id == pending.source
                    && tab.revision == pending.revision
                    && tab.vim.cursor == pending.cursor
                    && tab.vim.mode == pending.mode
            })
            && !self.vim_reference_open()
            && self.status_details.is_none()
            && !self.change_review_visible()
            && self.palette.is_none()
            && self.picker.is_none()
            && !self.settings_open
            && self.save_as.is_none()
            && self.close_action.is_none()
    }

    pub(super) fn resolve_vim_file_mark_path(&mut self, request: u64, path: Option<PathBuf>) {
        let Some(pending) = self
            .vim_file_marks
            .pending
            .as_ref()
            .filter(|pending| pending.request == request && pending.target.is_none())
        else {
            return;
        };
        if !self.file_mark_source_valid(pending) {
            self.vim_file_marks.pending = None;
            return;
        }
        let Some(path) = path else {
            self.fail_vim_file_mark("The marked file is no longer available");
            return;
        };
        let GlobalMarkRequest::Jump { name, .. } = pending.command else {
            return;
        };
        let Some(mut mark) = self.vim_file_marks.marks[name as usize - 'A' as usize].clone() else {
            self.fail_vim_file_mark("The mark is no longer available");
            return;
        };
        mark.path = Some(path);
        self.open_vim_file_mark(mark);
    }

    fn fail_vim_file_mark(&mut self, message: &str) {
        let Some(pending) = self.vim_file_marks.pending.take() else {
            return;
        };
        self.vim_input_status(message);
        if !pending.macro_input && self.tab().is_some_and(|tab| tab.id == pending.source) {
            self.defer_file_mark_events(pending.source, pending.events);
        }
    }

    fn open_vim_file_mark(&mut self, mark: FileMark) {
        let Some(pending) = self.vim_file_marks.pending.as_ref() else {
            return;
        };
        if !self.file_mark_source_valid(pending) {
            self.vim_file_marks.pending = None;
            return;
        }
        // Cross-document marks are jumps even with g' / g` (only same-document
        // g-mark motions suppress history).
        self.record_jump();
        let existing = self
            .projects
            .iter()
            .enumerate()
            .find_map(|(project, state)| {
                state
                    .tabs
                    .iter()
                    .position(|tab| tab.id == mark.document)
                    .map(|tab| (project, tab))
            });
        if let Some((project, tab)) = existing {
            self.activate_tab(project, tab);
        } else if let Some(path) = mark.path.as_ref() {
            let project = self.file_workspace(path);
            self.switch_project(project);
        }
        let target = jump_list::Jump::vim_mark(
            mark.document,
            mark.path,
            mark.offset,
            mark.line,
            mark.column,
        );
        if !self.open_jump(&target) {
            self.fail_vim_file_mark("The marked file could not be opened");
            return;
        }
        let Some(id) = self.tab().map(|tab| tab.id) else {
            self.vim_file_marks.pending = None;
            return;
        };
        let pane = self.current_pane_id();
        if let Some(pending) = self.vim_file_marks.pending.as_mut() {
            pending.target = Some(id);
            pending.destination_pane = Some(pane);
        }
        self.complete_vim_file_mark_jump(id);
    }

    pub(super) fn complete_vim_file_mark_jump(&mut self, id: u64) {
        let Some(pending) = self
            .vim_file_marks
            .pending
            .as_ref()
            .filter(|pending| pending.target == Some(id))
        else {
            return;
        };
        if self.tab().is_none_or(|tab| tab.id != id)
            || pending.destination_pane != Some(self.current_pane_id())
        {
            self.vim_file_marks.pending = None;
            return;
        }
        if self.pending_jump.is_some() || self.tab().is_none_or(|tab| tab.loading) {
            return;
        }
        let GlobalMarkRequest::Jump { name, linewise, .. } = pending.command else {
            return;
        };
        let Some(tab) = self.tab_by_id_mut(id) else {
            return;
        };
        if tab.error.is_some() {
            self.fail_vim_file_mark("The marked file could not be loaded");
            return;
        }
        let position = tab.vim.cursor;
        if tab.vim.finish_insert(&mut tab.text).text_changed {
            tab.changed();
        }
        tab.vim.set_global_mark_position(name, Some(position));
        tab.vim.set_selection(&tab.text, position..position);
        if linewise {
            tab.vim
                .handle_key(&mut tab.text, crate::vim::VimKey::Char('^'));
        }
        let cursor = tab.vim.cursor;
        tab.set_cursor(cursor..cursor);
        let pending = self.vim_file_marks.pending.take().unwrap();
        if let Some(mark) = self.vim_file_marks.marks[name as usize - 'A' as usize].as_mut() {
            mark.document = id;
        }
        self.refresh_vim_file_marks();
        self.sync_external_document_navigation(id);
        if pending.macro_input {
            self.vim_macro_navigation_arrived();
        }
        self.defer_file_mark_events(id, pending.events);
    }
}

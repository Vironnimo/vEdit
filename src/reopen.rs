//! A bounded, per-project history of closed files, without retaining discarded edits.
use super::*;

const HISTORY_LIMIT: usize = 20;

#[derive(Clone)]
pub(super) struct ClosedTab {
    path: PathBuf,
    cursor: usize,
    view: crate::cli::DocumentView,
}

impl VEditApp {
    pub(super) fn closed_tab_for_action(&self, action: CloseAction) -> Option<(usize, ClosedTab)> {
        let (id, pane) = match action {
            CloseAction::Tab(id) => (id, None),
            CloseAction::PaneTab { pane_id, id } => (id, Some(pane_id)),
            CloseAction::Project(_) | CloseAction::Exit => return None,
        };
        self.projects
            .iter()
            .enumerate()
            .find_map(|(index, project)| {
                let tab = project.tabs.iter().find(|tab| tab.id == id)?;
                // Untitled drafts and files that have never been saved have no disk
                // version to reopen. A deliberate Discard must not resurrect edits.
                tab.fingerprint.as_ref()?;
                let path = tab.path.clone()?;
                let (cursor, view) = pane
                    .and_then(|pane| self.closed_pane_position(pane, id))
                    .unwrap_or((tab.vim.cursor, tab.agent_view));
                Some((index, ClosedTab { path, cursor, view }))
            })
    }

    pub(super) fn remember_closed_tab(&mut self, closed: Option<(usize, ClosedTab)>) {
        let Some((index, closed)) = closed else {
            return;
        };
        let Some(project) = self.projects.get_mut(index) else {
            return;
        };
        project
            .closed_tabs
            .retain(|entry| entry.path != closed.path);
        project.closed_tabs.push(closed);
        if project.closed_tabs.len() > HISTORY_LIMIT {
            project.closed_tabs.remove(0);
        }
    }

    pub(super) fn can_reopen_closed_tab(&self) -> bool {
        self.project()
            .is_some_and(|project| !project.closed_tabs.is_empty())
    }

    pub(super) fn reopen_closed_tab(&mut self) {
        let Some(closed) = self
            .project()
            .and_then(|project| project.closed_tabs.last())
            .cloned()
        else {
            self.status("No closed files to reopen in this project", false);
            return;
        };
        if self.context_path_busy(&closed.path) {
            self.status(
                "Wait for the file rename to finish before reopening it",
                true,
            );
            return;
        }
        self.open_file(closed.path.clone());
        let Some(tab) = self
            .tab_mut()
            .filter(|tab| tab.path.as_ref() == Some(&closed.path))
        else {
            return;
        };
        // Asynchronous loading clamps the pending position against the newly
        // read text. An already open, possibly dirty buffer is reused as-is.
        let cursor = if tab.loading {
            closed.cursor
        } else {
            closed.cursor.min(tab.text.chars().count())
        };
        tab.set_cursor(cursor..cursor);
        self.set_document_view(closed.view);
        self.projects[self.active_project].closed_tabs.pop();
    }
}

#[cfg(test)]
#[path = "reopen_tests.rs"]
mod tests;

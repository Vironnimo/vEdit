//! Terminal tabs are placements of Project-owned sessions, never Documents.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum PaneTab {
    Document(u64),
    Terminal(u64),
}

impl PaneTab {
    pub(crate) fn id(self) -> u64 {
        match self {
            Self::Document(id) | Self::Terminal(id) => id,
        }
    }
}

impl Pane {
    /// Document membership keeps its existing contract. The order records the
    /// terminal positions between those Documents; new Documents append.
    pub(super) fn ordered_tabs(&self) -> Vec<PaneTab> {
        let mut documents = self.documents.iter().copied();
        let mut seen = std::collections::HashSet::new();
        let mut order = Vec::new();
        for item in &self.order {
            match *item {
                PaneTab::Document(id) if self.documents.contains(&id) && seen.insert(*item) => {
                    if let Some(id) = documents.next() {
                        order.push(PaneTab::Document(id));
                    }
                }
                PaneTab::Terminal(_) if seen.insert(*item) => order.push(*item),
                _ => {}
            }
        }
        order.extend(documents.map(PaneTab::Document));
        order
    }

    pub(super) fn has_terminal(&self, id: u64) -> bool {
        self.order.contains(&PaneTab::Terminal(id))
    }

    pub(super) fn is_empty(&self) -> bool {
        self.documents.is_empty()
            && !self
                .order
                .iter()
                .any(|tab| matches!(tab, PaneTab::Terminal(_)))
    }

    pub(super) fn normalize_tabs(&mut self) {
        self.order = self.ordered_tabs();
        if self.terminal.is_some_and(|id| !self.has_terminal(id)) {
            self.terminal = None;
        }
        if self
            .document
            .is_some_and(|id| !self.documents.contains(&id))
        {
            self.document = None;
        }
        if self.document.is_none() && self.terminal.is_none() {
            match self.order.first().copied() {
                Some(PaneTab::Document(id)) => self.document = Some(id),
                Some(PaneTab::Terminal(id)) => self.terminal = Some(id),
                None => {}
            }
        }
    }

    fn remove_terminal(&mut self, id: u64) {
        self.order = self.ordered_tabs();
        let position = self
            .order
            .iter()
            .position(|tab| *tab == PaneTab::Terminal(id));
        self.order.retain(|tab| *tab != PaneTab::Terminal(id));
        if self.terminal == Some(id) {
            self.terminal = None;
            if let Some(next) = position.and_then(|index| {
                self.order
                    .get(index.min(self.order.len().saturating_sub(1)))
            }) {
                match *next {
                    PaneTab::Terminal(id) => self.terminal = Some(id),
                    PaneTab::Document(id) => self.document = Some(id),
                }
            }
        }
        self.tab_strip_active = None;
    }
}

impl VEditApp {
    pub(in crate::app) fn current_pane_tabs(&self) -> Vec<PaneTab> {
        self.editor_layout
            .panes
            .iter()
            .find(|pane| pane.id == self.pane_context())
            .map_or_else(Vec::new, Pane::ordered_tabs)
    }

    pub(in crate::app) fn current_pane_active_tab(&self) -> Option<PaneTab> {
        self.editor_layout
            .panes
            .iter()
            .find(|pane| pane.id == self.pane_context())
            .and_then(|pane| {
                pane.terminal
                    .map(PaneTab::Terminal)
                    .or_else(|| pane.document.map(PaneTab::Document))
            })
    }

    pub(in crate::app) fn current_pane_terminal(&self) -> Option<u64> {
        self.editor_layout
            .panes
            .iter()
            .find(|pane| pane.id == self.pane_context())
            .and_then(|pane| pane.terminal)
    }

    pub(in crate::app) fn active_pane_terminal(&self) -> Option<u64> {
        self.editor_layout
            .panes
            .iter()
            .find(|pane| pane.id == self.editor_layout.focused)
            .and_then(|pane| pane.terminal)
    }

    pub(in crate::app) fn pane_terminal(&self, terminal: u64) -> Option<u64> {
        self.editor_layout
            .panes
            .iter()
            .find(|pane| pane.has_terminal(terminal))
            .map(|pane| pane.id)
    }

    /// Called after selecting a Document and its Project, before binding its view.
    pub(in crate::app) fn activate_pane_document(&mut self) {
        let id = self.pane_context();
        if let Some(pane) = self
            .editor_layout
            .panes
            .iter_mut()
            .find(|pane| pane.id == id)
        {
            pane.terminal = None;
        }
    }

    pub(in crate::app) fn focus_pane_terminal(&mut self, terminal: u64) -> Result<(), String> {
        let pane_id = self
            .pane_terminal(terminal)
            .ok_or("Terminal is not in an editor pane")?;
        self.cancel_vim_search(false);
        if let Some(tab) = self.tab_mut()
            && tab.vim.finish_insert(&mut tab.text).text_changed
        {
            tab.changed();
        }
        self.capture_pane_navigation();
        self.editor_layout.focused = pane_id;
        self.render_pane = pane_id;
        let pane = self
            .editor_layout
            .panes
            .iter_mut()
            .find(|pane| pane.id == pane_id)
            .unwrap();
        pane.terminal = Some(terminal);
        self.find_visible = false;
        self.completions = None;
        self.completion_request = None;
        self.editor_focused = false;
        self.editor_rect = None;
        self.editor_cursor_rect = None;
        self.ctx
            .memory_mut(|memory| memory.request_focus(Self::terminal_widget_id(terminal)));
        self.session_dirty = true;
        Ok(())
    }

    pub(in crate::app) fn dock_terminal_in_pane(
        &mut self,
        terminal: u64,
        pane_id: u64,
        index: usize,
    ) -> Result<(), String> {
        if !self.terminal_belongs_to_project(terminal, self.active_project) {
            return Err("Terminal does not belong to this Project".into());
        }
        let destination = self
            .editor_layout
            .panes
            .iter()
            .find(|pane| pane.id == pane_id)
            .ok_or("Destination pane no longer exists")?;
        let length =
            destination.ordered_tabs().len() - usize::from(destination.has_terminal(terminal));
        let index = if index == usize::MAX { length } else { index };
        if index > length {
            return Err(format!(
                "Tab position must be between 0 and {length}; no pane changed"
            ));
        }
        self.capture_pane_navigation();
        for pane in &mut self.editor_layout.panes {
            pane.remove_terminal(terminal);
        }
        let pane = self
            .editor_layout
            .panes
            .iter_mut()
            .find(|pane| pane.id == pane_id)
            .unwrap();
        pane.order = pane.ordered_tabs();
        pane.order.insert(index, PaneTab::Terminal(terminal));
        pane.empty_workspace = None;
        self.focus_terminal(terminal);
        self.collapse_empty_panes();
        self.session_dirty = true;
        Ok(())
    }

    pub(in crate::app) fn dock_terminal_in_split(
        &mut self,
        terminal: u64,
        pane_id: u64,
        orientation: SplitOrientation,
        before: bool,
    ) -> Result<u64, String> {
        if !self.terminal_belongs_to_project(terminal, self.active_project) {
            return Err("Terminal does not belong to this Project".into());
        }
        if self.editor_layout.panes.len() >= MAX_PANES {
            return Err(format!("At most {MAX_PANES} panes can be open"));
        }
        if !self
            .editor_layout
            .panes
            .iter()
            .any(|pane| pane.id == pane_id)
        {
            return Err("Destination pane no longer exists".into());
        }
        let id = self.allocate_pane_id();
        self.editor_layout.root.split(pane_id, id, orientation);
        if before {
            self.editor_layout.root.swap_split_children(pane_id, id);
        }
        self.editor_layout.panes.push(Pane::empty(id));
        self.dock_terminal_in_pane(terminal, id, 0)?;
        Ok(id)
    }

    /// Release placement only. The Project continues owning the running shell.
    pub(in crate::app) fn undock_terminal(&mut self, terminal: u64) {
        for pane in &mut self.editor_layout.panes {
            pane.remove_terminal(terminal);
        }
        for project in &mut self.projects {
            if let Some(layout) = &mut project.layout {
                for pane in &mut layout.panes {
                    pane.remove_terminal(terminal);
                }
            }
        }
        self.collapse_empty_panes();
        self.restore_focused_pane(false);
        self.session_dirty = true;
    }

    pub(in crate::app) fn drop_document_tab_at_mixed_index(
        &mut self,
        source: u64,
        destination: u64,
        document: u64,
        index: usize,
    ) -> Result<(), String> {
        let mut order = self
            .editor_layout
            .panes
            .iter()
            .find(|pane| pane.id == destination)
            .ok_or("Destination pane no longer exists")?
            .ordered_tabs();
        order.retain(|tab| *tab != PaneTab::Document(document));
        if index > order.len() {
            return Err("Tab position is outside this pane".into());
        }
        let document_index = order[..index]
            .iter()
            .filter(|tab| matches!(tab, PaneTab::Document(_)))
            .count();
        self.drop_pane_tab(source, destination, document, document_index)?;
        order.insert(index, PaneTab::Document(document));
        if let Some(pane) = self
            .editor_layout
            .panes
            .iter_mut()
            .find(|pane| pane.id == destination)
        {
            pane.order = order;
            pane.terminal = None;
        }
        Ok(())
    }
}

impl PaneNode {
    fn swap_split_children(&mut self, first_id: u64, second_id: u64) -> bool {
        let Self::Split { first, second, .. } = self else {
            return false;
        };
        if first.first_id() == first_id && second.first_id() == second_id {
            std::mem::swap(first, second);
            true
        } else {
            first.swap_split_children(first_id, second_id)
                || second.swap_split_children(first_id, second_id)
        }
    }
}

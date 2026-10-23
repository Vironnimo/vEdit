//! Editor panes share documents and edit history, while keeping view/navigation local.
use super::*;
pub(super) use crate::cli::SplitOrientation;
use crate::cli::{DocumentView, LayoutAction};
use crate::vim::VimNavigation;
use serde_json::{Value, json};
pub(super) use spatial_navigation::output_command_id;

const MAX_PANES: usize = 8;

#[derive(Default)]
pub(super) struct RegionFocusState {
    pending: Option<RegionFocusAction>,
    trailing: Vec<egui::Event>,
    owner: Option<RegionInputOwner>,
    awaiting_mark_owner: bool,
    mark_destination_filter: bool,
    boundary_event: Option<egui::Event>,
    awaiting_region: Option<spatial_navigation::Region>,
    rendered_focus_widgets: std::collections::HashSet<egui::Id>,
    spatial_anchor: Option<egui::Pos2>,
    output_rect: Option<egui::Rect>,
    output_widget: Option<egui::Id>,
    output_widget_problems: Option<bool>,
    output_focus_pending: bool,
}

#[derive(PartialEq)]
struct RegionInputOwner {
    project: Option<PathBuf>,
    pane: u64,
    document: Option<(u64, u64, usize, Range<usize>, VimMode)>,
    widget: Option<egui::Id>,
    vim_enabled: bool,
    vim_swap_jk: bool,
    leader_key: String,
    view: Option<DocumentView>,
}

#[derive(Clone, Copy)]
enum RegionFocusAction {
    Explorer,
    Editor,
    Git,
    Output,
    Cycle(bool),
    Direction(&'static str),
    AgentMark(bool),
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum PaneNode {
    Pane {
        id: u64,
    },
    Split {
        orientation: SplitOrientation,
        ratio: f32,
        first: Box<PaneNode>,
        second: Box<PaneNode>,
    },
}

impl PaneNode {
    fn retain_panes(self, retained: &std::collections::HashSet<u64>) -> Option<Self> {
        match self {
            Self::Pane { id } => retained.contains(&id).then_some(Self::Pane { id }),
            Self::Split {
                orientation,
                ratio,
                first,
                second,
            } => match (first.retain_panes(retained), second.retain_panes(retained)) {
                (Some(first), Some(second)) => Some(Self::Split {
                    orientation,
                    ratio: if ratio.is_finite() {
                        ratio.clamp(0.15, 0.85)
                    } else {
                        0.5
                    },
                    first: Box::new(first),
                    second: Box::new(second),
                }),
                (Some(node), None) | (None, Some(node)) => Some(node),
                _ => None,
            },
        }
    }
    fn remap_ids(&mut self, mapping: &HashMap<u64, u64>) {
        match self {
            Self::Pane { id } => *id = mapping[id],
            Self::Split { first, second, .. } => {
                first.remap_ids(mapping);
                second.remap_ids(mapping);
            }
        }
    }
    fn ids(&self, ids: &mut Vec<u64>) {
        match self {
            Self::Pane { id } => ids.push(*id),
            Self::Split { first, second, .. } => {
                first.ids(ids);
                second.ids(ids);
            }
        }
    }
    fn first_id(&self) -> u64 {
        match self {
            Self::Pane { id } => *id,
            Self::Split { first, .. } => first.first_id(),
        }
    }
    fn neighbor(&self, target: u64) -> Option<u64> {
        let Self::Split { first, second, .. } = self else {
            return None;
        };
        if matches!(first.as_ref(), Self::Pane { id } if *id == target) {
            Some(second.first_id())
        } else if matches!(second.as_ref(), Self::Pane { id } if *id == target) {
            Some(first.first_id())
        } else {
            first.neighbor(target).or_else(|| second.neighbor(target))
        }
    }
    fn split(&mut self, target: u64, new_id: u64, orientation: SplitOrientation) -> bool {
        match self {
            Self::Pane { id } if *id == target => {
                *self = Self::Split {
                    orientation,
                    ratio: 0.5,
                    first: Box::new(Self::Pane { id: target }),
                    second: Box::new(Self::Pane { id: new_id }),
                };
                true
            }
            Self::Pane { .. } => false,
            Self::Split { first, second, .. } => {
                first.split(target, new_id, orientation)
                    || second.split(target, new_id, orientation)
            }
        }
    }
    fn remove(&mut self, target: u64) -> bool {
        let Self::Split { first, second, .. } = self else {
            return false;
        };
        if matches!(first.as_ref(), Self::Pane { id } if *id == target) {
            *self = *second.clone();
            return true;
        }
        if matches!(second.as_ref(), Self::Pane { id } if *id == target) {
            *self = *first.clone();
            return true;
        }
        first.remove(target) || second.remove(target)
    }
    fn resize(&mut self, target: u64, new_ratio: f32) -> bool {
        let Self::Split {
            first,
            second,
            ratio,
            ..
        } = self
        else {
            return false;
        };
        if first.resize(target, new_ratio) || second.resize(target, new_ratio) {
            return true;
        }
        if matches!(first.as_ref(), Self::Pane { id } if *id == target)
            || matches!(second.as_ref(), Self::Pane { id } if *id == target)
        {
            *ratio = new_ratio;
            return true;
        }
        false
    }
    fn resize_divider(&mut self, first_id: u64, second_id: u64, new_ratio: f32) -> bool {
        let Self::Split {
            first,
            second,
            ratio,
            ..
        } = self
        else {
            return false;
        };
        if first.first_id() == first_id && second.first_id() == second_id {
            *ratio = new_ratio;
            return true;
        }
        first.resize_divider(first_id, second_id, new_ratio)
            || second.resize_divider(first_id, second_id, new_ratio)
    }
}

#[derive(Clone)]
struct PaneDocumentState {
    navigation: VimNavigation,
    view: DocumentView,
    selection: Range<usize>,
    source_scroll: Option<[f32; 2]>,
    preview_scroll: Option<f32>,
    /// The document revision these positions refer to; see
    /// `shift_shared_document_positions`.
    revision: u64,
}
#[derive(Clone)]
struct Pane {
    id: u64,
    documents: Vec<u64>,
    document: Option<u64>,
    empty_workspace: Option<PathBuf>,
    view: DocumentView,
    states: HashMap<u64, PaneDocumentState>,
    tab_strip_active: Option<u64>,
    preview_ui_id: Option<egui::Id>,
    /// Back/forward navigation history of this pane.
    jumps: super::jump_list::JumpList,
}

/// Rendering and restoring focus only need the active tab's view. Keep the
/// complete tab list, parked view states and jump history out of frame copies.
struct PaneView {
    id: u64,
    document: Option<u64>,
    empty_workspace: Option<PathBuf>,
    view: DocumentView,
    state: Option<PaneDocumentState>,
    tab_strip_active: Option<u64>,
}

impl Pane {
    fn empty(id: u64) -> Self {
        Self {
            id,
            documents: Vec::new(),
            document: None,
            empty_workspace: None,
            view: DocumentView::Source,
            states: HashMap::new(),
            tab_strip_active: None,
            preview_ui_id: None,
            jumps: Default::default(),
        }
    }

    fn active_view(&self) -> PaneView {
        PaneView {
            id: self.id,
            document: self.document,
            empty_workspace: self.empty_workspace.clone(),
            view: self.view,
            state: self.document.and_then(|id| self.states.get(&id)).cloned(),
            tab_strip_active: self.tab_strip_active,
        }
    }
}

pub(super) struct EditorLayout {
    root: PaneNode,
    panes: Vec<Pane>,
    focused: u64,
    rects: HashMap<u64, egui::Rect>,
    groups_initialized: bool,
}
impl Default for EditorLayout {
    fn default() -> Self {
        Self {
            root: PaneNode::Pane { id: 1 },
            panes: vec![Pane::empty(1)],
            focused: 1,
            rects: HashMap::new(),
            groups_initialized: false,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct SessionLayout {
    root: PaneNode,
    panes: Vec<SessionPane>,
    focused: u64,
}
#[derive(Clone, Serialize, Deserialize)]
struct SessionPane {
    id: u64,
    project: Option<usize>,
    tab: Option<usize>,
    view: DocumentView,
    cursor: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    empty_workspace: Option<PathBuf>,
    #[serde(default)]
    documents: Option<Vec<(usize, usize)>>,
    #[serde(default)]
    states: Vec<SessionPaneDocument>,
}
#[derive(Clone, Serialize, Deserialize)]
struct SessionPaneDocument {
    project: usize,
    tab: usize,
    view: DocumentView,
    cursor: usize,
    selection: Range<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_scroll: Option<[f32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    preview_scroll: Option<f32>,
}

impl SessionLayout {
    pub(super) fn remap_projects(&mut self, mapping: &[Option<usize>]) {
        for pane in &mut self.panes {
            pane.project = pane
                .project
                .and_then(|index| mapping.get(index).copied().flatten());
            if pane.project.is_none() {
                pane.tab = None;
            }
            if let Some(documents) = &mut pane.documents {
                documents.retain_mut(|(project, _)| {
                    if let Some(restored) = mapping.get(*project).copied().flatten() {
                        *project = restored;
                        true
                    } else {
                        false
                    }
                });
            }
            pane.states.retain_mut(|state| {
                if let Some(restored) = mapping.get(state.project).copied().flatten() {
                    state.project = restored;
                    true
                } else {
                    false
                }
            });
        }
    }
}

/// How character positions move from one revision of a document to the next.
/// Whatever the edit source, the change is the span between the texts'
/// common prefix and common suffix.
#[derive(Clone, Copy, Debug)]
struct TextShift {
    from: u64,
    to: u64,
    start: usize,
    old_end: usize,
    new_end: usize,
    old_len: usize,
}

impl TextShift {
    fn between(old: &str, new: &str, from: u64, to: u64) -> Self {
        let mut prefix = old
            .bytes()
            .zip(new.bytes())
            .take_while(|(a, b)| a == b)
            .count();
        while !old.is_char_boundary(prefix) {
            prefix -= 1;
        }
        let limit = old.len().min(new.len()) - prefix;
        let mut suffix = old
            .bytes()
            .rev()
            .zip(new.bytes().rev())
            .take(limit)
            .take_while(|(a, b)| a == b)
            .count();
        while !old.is_char_boundary(old.len() - suffix) {
            suffix -= 1;
        }
        let start = old[..prefix].chars().count();
        Self {
            from,
            to,
            start,
            old_end: start + old[prefix..old.len() - suffix].chars().count(),
            new_end: start + new[prefix..new.len() - suffix].chars().count(),
            old_len: old.chars().count(),
        }
    }

    /// A position keeps its character. One inside replaced text moves to
    /// the replacement's start; `before_insert` keeps a range end in front
    /// of text inserted exactly there.
    fn position(self, position: usize, before_insert: bool) -> usize {
        if position > self.old_len || position < self.start {
            // Beyond the old text, a position was never in its coordinates.
            position
        } else if position > self.old_end
            || position == self.old_end && !(before_insert && self.start == self.old_end)
        {
            position - self.old_end + self.new_end
        } else {
            self.start
        }
    }

    fn selection(self, selection: &Range<usize>) -> Range<usize> {
        if selection.is_empty() {
            let position = self.position(selection.start, false);
            position..position
        } else {
            let start = self.position(selection.start, false);
            start..self.position(selection.end, true).max(start)
        }
    }
}

impl VEditApp {
    /// Documents are shared, navigation is pane-local: after any edit — Vim,
    /// native typing, Find/Replace, agents or a reload — move the cursor,
    /// Visual anchor and selection that other panes keep for the document,
    /// so they stay on the same text. The focused pane's live state is
    /// captured from the document itself after each edit and needs no shift.
    ///
    /// Each pane state records the revision its positions refer to. A tab
    /// keeps the text of its last synchronized revision only while another
    /// pane holds a position in it.
    pub(super) fn shift_shared_document_positions(&mut self) {
        let mut shifts = HashMap::new();
        let mut fresh = HashMap::new();
        for tab in self.projects.iter().flat_map(|project| &project.tabs) {
            if tab.loading {
                continue;
            }
            match &tab.pane_baseline {
                Some((revision, _)) if *revision == tab.revision => {}
                Some((revision, text)) => {
                    shifts.insert(
                        tab.id,
                        TextShift::between(text, &tab.text, *revision, tab.revision),
                    );
                }
                None => {
                    fresh.insert(tab.id, tab.revision);
                }
            }
        }
        let live = self.editor_layout.focused;
        let mut shared = std::collections::HashSet::new();
        let layouts = std::iter::once(&mut self.editor_layout).chain(
            self.projects
                .iter_mut()
                .filter_map(|project| project.layout.as_mut()),
        );
        for (index, layout) in layouts.enumerate() {
            for pane in &mut layout.panes {
                for (id, state) in &mut pane.states {
                    if !(index == 0 && pane.id == live && pane.document == Some(*id)) {
                        shared.insert(*id);
                    }
                    if let Some(shift) = shifts.get(id) {
                        if state.revision == shift.from {
                            state.navigation = state
                                .navigation
                                .remapped(|position| shift.position(position, false));
                            state.selection = shift.selection(&state.selection);
                            state.revision = shift.to;
                        }
                    } else if let Some(revision) = fresh.get(id) {
                        // Without a recorded text, assume the stored positions
                        // already match the document, as when it was restored.
                        state.revision = *revision;
                    }
                }
            }
        }
        for tab in self
            .projects
            .iter_mut()
            .flat_map(|project| &mut project.tabs)
        {
            if tab.loading {
                continue;
            }
            if !shared.contains(&tab.id) {
                tab.pane_baseline = None;
            } else if tab
                .pane_baseline
                .as_ref()
                .is_none_or(|(revision, _)| *revision != tab.revision)
            {
                tab.pane_baseline = Some((tab.revision, tab.text.clone()));
            }
        }
    }
    /// A view drawn in place of every pane, such as the change review,
    /// occupies the whole area as the focused pane's region. The pane tree
    /// itself stays unchanged so closing the view restores the layout.
    pub(super) fn cover_editor_panes(&mut self, rect: egui::Rect) {
        let pane = self.editor_layout.focused;
        self.editor_layout.rects.clear();
        self.editor_layout.rects.insert(pane, rect);
    }
    fn allocate_pane_id(&mut self) -> u64 {
        let id = self.next_pane_id;
        self.next_pane_id = self.next_pane_id.saturating_add(1);
        id
    }
    fn empty_project_layout(&mut self, root: PathBuf) -> EditorLayout {
        let id = self.allocate_pane_id();
        let mut pane = Pane::empty(id);
        pane.empty_workspace = Some(root);
        EditorLayout {
            root: PaneNode::Pane { id },
            panes: vec![pane],
            focused: id,
            rects: HashMap::new(),
            groups_initialized: false,
        }
    }
    /// Move only pane metadata: document text and edit history never leave their project.
    pub(super) fn activate_project_layout(&mut self, index: usize) {
        let root = self.projects.get(index).map(|project| project.root.clone());
        if self.layout_project == root && root.is_some() {
            self.active_project = index;
            return;
        }
        if self.layout_project.is_none()
            && self.active_project == index
            && root.is_some()
            && self.projects[index].layout.is_none()
        {
            self.layout_project = root;
            return;
        }
        if self.layout_project.as_deref() == self.project().map(|project| project.root.as_path()) {
            self.cancel_vim_search(false);
            if let Some(tab) = self.tab_mut()
                && tab.vim.finish_insert(&mut tab.text).text_changed
            {
                tab.changed();
            }
            self.capture_pane_navigation();
        }
        let previous = std::mem::take(&mut self.editor_layout);
        if let Some(owner) = self.layout_project.take()
            && let Some(project) = self
                .projects
                .iter_mut()
                .find(|project| project.root == owner)
        {
            project.layout = Some(previous);
        }
        self.active_project = index;
        self.editor_layout = if let Some(root) = &root {
            self.projects[index]
                .layout
                .take()
                .unwrap_or_else(|| self.empty_project_layout(root.clone()))
        } else {
            let id = self.allocate_pane_id();
            EditorLayout {
                root: PaneNode::Pane { id },
                panes: vec![Pane::empty(id)],
                focused: id,
                rects: HashMap::new(),
                groups_initialized: false,
            }
        };
        self.layout_project = root;
        self.editor_layout.rects.clear();
        self.render_pane = self.editor_layout.focused;
        self.tab_strip_active = None;
        self.tab_drag = tab_drag::TabDragState::default();
        self.completions = None;
        self.completion_request = None;
        self.editor_rect = None;
        self.editor_cursor_rect = None;
        self.editor_focused = false;
        self.ctx.memory_mut(|memory| {
            if let Some(id) = memory.focused() {
                memory.surrender_focus(id);
            }
        });
        self.initialize_pane_groups();
        self.adopt_orphan_documents();
        self.restore_focused_pane(false);
        self.restore_project_scroll();
        let has_saved_scroll = self
            .editor_layout
            .panes
            .iter()
            .find(|pane| pane.id == self.editor_layout.focused)
            .and_then(|pane| pane.document.and_then(|id| pane.states.get(&id)))
            .is_some_and(|state| state.source_scroll.is_some() || state.preview_scroll.is_some());
        if let Some(tab) = self.tab_mut() {
            tab.pending_selection = Some(tab.selection.clone());
            tab.focus = tab.agent_view == DocumentView::Source;
            if has_saved_scroll {
                tab.scroll_to_cursor = false;
            }
        }
        self.session_dirty = true;
        self.ctx.request_repaint();
    }
    fn restore_project_scroll(&mut self) {
        for pane in &self.editor_layout.panes {
            for (document, state) in &pane.states {
                if let Some([x, y]) = state.source_scroll {
                    self.find
                        .restore_scroll
                        .insert((pane.id, *document), Vec2::new(x, y));
                }
                if let (Some(id), Some(offset)) = (pane.preview_ui_id, state.preview_scroll) {
                    self.markdown.restore_view_scroll(*document, id, offset);
                }
            }
        }
    }
    pub(super) fn has_saved_document_scroll(&self, document: u64) -> bool {
        let Some((project, _)) = self.tab_location(document) else {
            return false;
        };
        let layout = if project == self.active_project {
            Some(&self.editor_layout)
        } else {
            self.projects[project].layout.as_ref()
        };
        let Some(layout) = layout else {
            return false;
        };
        let pane = layout
            .panes
            .iter()
            .find(|pane| pane.id == layout.focused && pane.documents.contains(&document))
            .or_else(|| {
                layout
                    .panes
                    .iter()
                    .find(|pane| pane.documents.contains(&document))
            });
        pane.and_then(|pane| pane.states.get(&document))
            .is_some_and(|state| match state.view {
                DocumentView::Source => state.source_scroll.is_some(),
                DocumentView::Preview => state.preview_scroll.is_some(),
            })
    }
    pub(super) fn pane_has_empty_workspace(&self) -> bool {
        self.editor_layout.panes.iter().any(|pane| {
            pane.id == self.pane_context()
                && pane.document.is_none()
                && pane.empty_workspace.is_some()
        })
    }
    pub(super) fn clear_empty_workspace(&mut self) {
        let pane_id = self.pane_context();
        if let Some(pane) = self
            .editor_layout
            .panes
            .iter_mut()
            .find(|pane| pane.id == pane_id)
        {
            pane.empty_workspace = None;
        }
    }
    fn pane_context(&self) -> u64 {
        if self.rendering_panes {
            self.render_pane
        } else {
            self.editor_layout.focused
        }
    }
    pub(super) fn document_widget_id(&self, tab_id: u64) -> egui::Id {
        let pane = self.pane_context();
        if pane == 1 {
            egui::Id::new(("document", tab_id))
        } else {
            egui::Id::new(("document", tab_id, pane))
        }
    }
    pub(super) fn focused_jump_list(&mut self) -> Option<&mut super::jump_list::JumpList> {
        let focused = self.editor_layout.focused;
        self.editor_layout
            .panes
            .iter_mut()
            .find(|pane| pane.id == focused)
            .map(|pane| &mut pane.jumps)
    }
    pub(super) fn render_pane_is_focused(&self) -> bool {
        self.pane_context() == self.editor_layout.focused
    }
    pub(super) fn capture_pane_navigation(&mut self) {
        let pane_id = self.pane_context();
        let Some(tab) = self.tab() else { return };
        let pane = self
            .editor_layout
            .panes
            .iter()
            .find(|pane| pane.id == pane_id);
        let previous = pane.and_then(|pane| pane.states.get(&tab.id));
        let source_scroll = self
            .find
            .restore_scroll
            .get(&(pane_id, tab.id))
            .copied()
            .or_else(|| {
                self.find
                    .view_scroll
                    .get(&(pane_id, tab.id))
                    .map(|(_, offset)| *offset)
            })
            .map(|offset| [offset.x, offset.y])
            .or_else(|| previous.and_then(|state| state.source_scroll));
        let preview_scroll = pane
            .and_then(|pane| pane.preview_ui_id)
            .and_then(|id| self.markdown.view_scroll(tab.id, id))
            .or_else(|| previous.and_then(|state| state.preview_scroll));
        let (id, state) = (
            tab.id,
            PaneDocumentState {
                navigation: tab.vim.navigation(),
                view: tab.agent_view,
                selection: tab.selection.clone(),
                source_scroll,
                preview_scroll,
                revision: tab.revision,
            },
        );
        if let Some(pane) = self
            .editor_layout
            .panes
            .iter_mut()
            .find(|pane| pane.id == pane_id)
            && pane.document == Some(id)
        {
            pane.view = state.view;
            pane.states.insert(id, state);
        }
    }
    pub(super) fn bind_current_pane(&mut self) {
        if self.layout_project.as_deref() != self.project().map(|project| project.root.as_path()) {
            self.activate_project_layout(self.active_project);
        }
        self.shift_shared_document_positions();
        self.initialize_pane_groups();
        let pane_id = self.pane_context();
        let Some(tab) = self.tab() else {
            let empty_workspace = self.project().map(|project| project.root.clone());
            if let Some(pane) = self
                .editor_layout
                .panes
                .iter_mut()
                .find(|pane| pane.id == pane_id)
            {
                pane.document = None;
                pane.empty_workspace = empty_workspace;
                pane.view = DocumentView::Source;
            }
            return;
        };
        let (id, default_view, navigation, selection, revision) = (
            tab.id,
            tab.agent_view,
            tab.vim.navigation(),
            tab.selection.clone(),
            tab.revision,
        );
        let Some(pane) = self
            .editor_layout
            .panes
            .iter_mut()
            .find(|pane| pane.id == pane_id)
        else {
            return;
        };
        if !pane.documents.contains(&id) {
            pane.documents.push(id);
        }
        pane.empty_workspace = None;
        if pane.document == Some(id) {
            return;
        }
        let state = pane
            .states
            .entry(id)
            .or_insert(PaneDocumentState {
                navigation,
                view: default_view,
                selection,
                source_scroll: None,
                preview_scroll: None,
                revision,
            })
            .clone();
        pane.document = Some(id);
        pane.view = state.view;
        let vim_enabled = self.settings.vim;
        if let Some(tab) = self.tab_mut() {
            if tab.loading {
                tab.vim.cursor = state.navigation.cursor;
            } else {
                tab.vim.restore_navigation(&tab.text, &state.navigation);
            }
            tab.agent_view = state.view;
            let range = if vim_enabled && tab.vim.mode != VimMode::Insert && !tab.vim.multi.active()
            {
                tab.vim
                    .selection(&tab.text)
                    .unwrap_or(tab.vim.cursor..tab.vim.cursor)
            } else {
                state.selection.clone()
            };
            tab.selection = range.clone();
            tab.pending_selection = Some(range);
            tab.focus = state.view == DocumentView::Source;
        }
        if let Some([x, y]) = state.source_scroll {
            self.find
                .restore_scroll
                .insert((pane_id, id), Vec2::new(x, y));
        }
    }
    fn initialize_pane_groups(&mut self) {
        if self.editor_layout.groups_initialized {
            return;
        }
        let active = self
            .project()
            .and_then(|project| project.tabs.get(project.active_tab))
            .map(|tab| (tab.id, tab.agent_view));
        self.editor_layout.groups_initialized = true;
        if let Some((id, view)) = active
            && let Some(pane) = self
                .editor_layout
                .panes
                .iter_mut()
                .find(|pane| pane.id == self.editor_layout.focused)
        {
            pane.document = Some(id);
            pane.view = view;
            pane.empty_workspace = None;
        }
        self.adopt_orphan_documents();
    }
    /// The focused pane owns newly opened documents; other groups keep their membership.
    pub(super) fn adopt_orphan_documents(&mut self) {
        let assigned: std::collections::HashSet<_> = self
            .editor_layout
            .panes
            .iter()
            .flat_map(|pane| pane.documents.iter().copied())
            .collect();
        let orphaned: Vec<_> = self
            .project()
            .into_iter()
            .flat_map(|project| project.tabs.iter())
            .filter(|tab| !assigned.contains(&tab.id))
            .map(|tab| {
                (
                    tab.id,
                    PaneDocumentState {
                        navigation: tab.vim.navigation(),
                        view: tab.agent_view,
                        selection: tab.selection.clone(),
                        source_scroll: None,
                        preview_scroll: None,
                        revision: tab.revision,
                    },
                )
            })
            .collect();
        if let Some(pane) = self
            .editor_layout
            .panes
            .iter_mut()
            .find(|pane| pane.id == self.editor_layout.focused)
        {
            for (id, state) in orphaned {
                pane.documents.push(id);
                pane.states.insert(id, state);
            }
            if pane.document.is_none() && pane.empty_workspace.is_none() {
                pane.document = pane.documents.first().copied();
                pane.view = pane
                    .document
                    .and_then(|id| pane.states.get(&id))
                    .map_or(DocumentView::Source, |state| state.view);
            }
        }
    }
    /// Whether a pane of the visible layout currently shows this document.
    pub(super) fn layout_shows_document(&self, id: u64) -> bool {
        self.editor_layout
            .panes
            .iter()
            .any(|pane| pane.document == Some(id))
    }
    pub(super) fn current_pane_tab_ids(&self) -> Vec<u64> {
        self.editor_layout
            .panes
            .iter()
            .find(|pane| pane.id == self.pane_context())
            .map_or_else(Vec::new, |pane| pane.documents.clone())
    }
    pub(super) fn current_pane_id(&self) -> u64 {
        self.pane_context()
    }
    pub(super) fn replace_pane_preview(&mut self, previous: u64, replacement: u64) {
        let replaceable = self
            .projects
            .iter()
            .flat_map(|project| &project.tabs)
            .any(|tab| {
                tab.id == previous
                    && tab.preview_tab
                    && !tab.dirty
                    && !tab.saving
                    && !tab.agent_unsaved
                    && tab.annotations.is_empty()
            });
        if !replaceable {
            self.keep_tab_open(previous);
            return;
        }
        let pane_id = self.current_pane_id();
        if let Some(pane) = self
            .editor_layout
            .panes
            .iter_mut()
            .find(|pane| pane.id == pane_id)
            && let Some(position) = pane.documents.iter().position(|id| *id == previous)
        {
            pane.documents.retain(|id| *id != replacement);
            pane.documents.insert(position, replacement);
        }
        // Browsing is not an explicit close: do not flood Reopen Closed Tab.
        if let Ok(unreferenced) = self.detach_pane_tab(pane_id, previous) {
            if unreferenced {
                self.remove_closed_document(previous);
            }
            self.control_prune_review();
            self.reconcile_layout_after_close();
        }
    }
    pub(super) fn focus_existing_project_tab(&mut self, project: usize, index: usize) {
        let Some(id) = self
            .projects
            .get(project)
            .and_then(|project| project.tabs.get(index))
            .map(|tab| tab.id)
        else {
            return;
        };
        let focused = self.editor_layout.focused;
        let pane = self
            .editor_layout
            .panes
            .iter()
            .find(|pane| pane.id == focused && pane.document == Some(id))
            .or_else(|| {
                self.editor_layout
                    .panes
                    .iter()
                    .find(|pane| pane.document == Some(id))
            })
            .or_else(|| {
                self.editor_layout
                    .panes
                    .iter()
                    .find(|pane| pane.id == focused && pane.documents.contains(&id))
            })
            .or_else(|| {
                self.editor_layout
                    .panes
                    .iter()
                    .find(|pane| pane.documents.contains(&id))
            })
            .map(|pane| pane.id);
        if let Some(pane) = pane {
            let _ = self.focus_editor_pane(pane);
        }
    }
    /// Move every pane's view of `id` through a replacement of its whole text,
    /// so cursors, selections and scroll positions stay on the same lines.
    pub(super) fn remap_document_views(
        &mut self,
        id: u64,
        position: &dyn Fn(usize) -> usize,
        scroll: &dyn Fn(f32) -> f32,
    ) {
        // The mapped positions refer to the new text, so the generic shift
        // after an edit must not move them a second time.
        let mut revision = None;
        if let Some(tab) = self.tab_by_id_mut(id) {
            revision = Some(tab.revision);
            if tab.pane_baseline.is_some() {
                tab.pane_baseline = Some((tab.revision, tab.text.clone()));
            }
        }
        let layouts = std::iter::once(&mut self.editor_layout).chain(
            self.projects
                .iter_mut()
                .filter_map(|project| project.layout.as_mut()),
        );
        for layout in layouts {
            for pane in &mut layout.panes {
                if let Some(state) = pane.states.get_mut(&id) {
                    state.navigation = state.navigation.mapped(position);
                    let start = position(state.selection.start);
                    state.selection = start..position(state.selection.end).max(start);
                    if let Some([x, y]) = state.source_scroll {
                        state.source_scroll = Some([x, scroll(y)]);
                    }
                    if let Some(revision) = revision {
                        state.revision = revision;
                    }
                }
            }
        }
        // Rendered panes restore these offsets on their next frame.
        let mut offsets: HashMap<(u64, u64), Vec2> = self
            .find
            .view_scroll
            .iter()
            .filter(|((_, document), _)| *document == id)
            .map(|(key, (_, offset))| (*key, *offset))
            .collect();
        for (key, offset) in &self.find.restore_scroll {
            if key.1 == id {
                offsets.insert(*key, *offset);
            }
        }
        for (key, offset) in offsets {
            self.find
                .restore_scroll
                .insert(key, Vec2::new(offset.x, scroll(offset.y)));
        }
    }
    /// Explicit agent navigation also updates parked views, without changing project focus.
    pub(super) fn sync_external_document_navigation(&mut self, id: u64) {
        let Some((pi, ti)) = self.tab_location(id) else {
            return;
        };
        // Other panes' positions still refer to the text before this change.
        self.shift_shared_document_positions();
        let tab = &self.projects[pi].tabs[ti];
        let navigation = tab.vim.navigation();
        let selection = tab.selection.clone();
        let view = tab.agent_view;
        let revision = tab.revision;
        let layout = if pi == self.active_project {
            Some(&mut self.editor_layout)
        } else {
            self.projects[pi].layout.as_mut()
        };
        if let Some(layout) = layout {
            // Prefer the project-focused group. Other split cursors remain local.
            let target = layout
                .panes
                .iter()
                .find(|pane| pane.id == layout.focused && pane.document == Some(id))
                .or_else(|| layout.panes.iter().find(|pane| pane.document == Some(id)))
                .or_else(|| {
                    layout
                        .panes
                        .iter()
                        .find(|pane| pane.id == layout.focused && pane.documents.contains(&id))
                })
                .or_else(|| {
                    layout
                        .panes
                        .iter()
                        .find(|pane| pane.documents.contains(&id))
                })
                .map(|pane| pane.id);
            if let Some(pane) = layout.panes.iter_mut().find(|pane| Some(pane.id) == target) {
                if let Some(state) = pane.states.get_mut(&id) {
                    state.navigation = navigation;
                    state.selection = selection;
                    state.revision = revision;
                    // An explicit location/edit reveals its result on the next focus.
                    state.source_scroll = None;
                    state.preview_scroll = None;
                } else {
                    pane.states.insert(
                        id,
                        PaneDocumentState {
                            navigation,
                            selection,
                            view,
                            source_scroll: None,
                            preview_scroll: None,
                            revision,
                        },
                    );
                }
                self.find.restore_scroll.remove(&(pane.id, id));
                self.find.view_scroll.remove(&(pane.id, id));
            }
        }
    }
    pub(super) fn pane_contains_tab(&self, pane_id: u64, id: u64) -> bool {
        self.editor_layout
            .panes
            .iter()
            .any(|pane| pane.id == pane_id && pane.documents.contains(&id))
    }
    pub(super) fn closed_pane_position(
        &self,
        pane_id: u64,
        id: u64,
    ) -> Option<(usize, DocumentView)> {
        self.editor_layout
            .panes
            .iter()
            .find(|pane| pane.id == pane_id)?
            .states
            .get(&id)
            .map(|state| (state.navigation.cursor, state.view))
    }
    pub(super) fn pane_tab_is_shared(&self, pane_id: u64, id: u64) -> bool {
        self.editor_layout
            .panes
            .iter()
            .any(|pane| pane.id != pane_id && pane.documents.contains(&id))
    }
    /// Move a group's reference, never the shared buffer or its edit history.
    /// `index` is the final position after an existing destination reference is removed.
    pub(super) fn move_pane_tab(
        &mut self,
        source_pane_id: u64,
        pane_id: u64,
        tab_id: u64,
        index: usize,
    ) -> Result<(), String> {
        self.move_pane_tab_inner(source_pane_id, pane_id, tab_id, index, true, false)
    }
    pub(super) fn drop_pane_tab(
        &mut self,
        source_pane_id: u64,
        pane_id: u64,
        tab_id: u64,
        index: usize,
    ) -> Result<(), String> {
        self.move_pane_tab_inner(source_pane_id, pane_id, tab_id, index, false, false)
    }
    fn move_pane_tab_inner(
        &mut self,
        source_pane_id: u64,
        pane_id: u64,
        tab_id: u64,
        index: usize,
        request_focus: bool,
        prefer_source_state: bool,
    ) -> Result<(), String> {
        let source = self
            .editor_layout
            .panes
            .iter()
            .position(|p| p.id == source_pane_id)
            .ok_or("Source pane no longer exists; inspect layout status")?;
        let destination = self
            .editor_layout
            .panes
            .iter()
            .position(|p| p.id == pane_id)
            .ok_or("Destination pane no longer exists; inspect layout status")?;
        if !self.editor_layout.panes[source].documents.contains(&tab_id)
            || self.tab_location(tab_id).is_none()
        {
            return Err("Tab no longer belongs to the source pane; inspect layout status".into());
        }
        let existing = self.editor_layout.panes[destination]
            .documents
            .contains(&tab_id);
        let length = self.editor_layout.panes[destination].documents.len() - usize::from(existing);
        if index > length {
            return Err(format!(
                "Tab position must be between 0 and {length}; no pane changed"
            ));
        }
        self.keep_tab_open(tab_id);
        self.cancel_vim_search(false);
        // Validation precedes capture or finishing Insert: rejected requests have no effect.
        let changes_document_focus =
            self.editor_layout.focused != pane_id || self.tab().is_none_or(|tab| tab.id != tab_id);
        if changes_document_focus
            && let Some(tab) = self.tab_mut()
            && tab.vim.finish_insert(&mut tab.text).text_changed
        {
            tab.changed();
        }
        self.capture_pane_navigation();
        if source_pane_id != pane_id {
            self.transfer_tab_preview(
                source_pane_id,
                pane_id,
                tab_id,
                existing && !prefer_source_state,
            );
            if (!existing || prefer_source_state)
                && let Some(offset) = self
                    .find
                    .restore_scroll
                    .get(&(source_pane_id, tab_id))
                    .copied()
                    .or_else(|| {
                        self.find
                            .view_scroll
                            .get(&(source_pane_id, tab_id))
                            .map(|(_, offset)| *offset)
                    })
            {
                self.find.restore_scroll.insert((pane_id, tab_id), offset);
            }
            self.find.restore_scroll.remove(&(source_pane_id, tab_id));
            self.find.view_scroll.remove(&(source_pane_id, tab_id));
        }
        let source_state = self.editor_layout.panes[source]
            .states
            .get(&tab_id)
            .cloned()
            .or_else(|| {
                self.tab_location(tab_id).map(|(pi, ti)| {
                    let tab = &self.projects[pi].tabs[ti];
                    PaneDocumentState {
                        navigation: tab.vim.navigation(),
                        view: tab.agent_view,
                        selection: tab.selection.clone(),
                        source_scroll: None,
                        preview_scroll: None,
                        revision: tab.revision,
                    }
                })
            })
            .expect("validated open tab");
        {
            let pane = &mut self.editor_layout.panes[source];
            let old = pane.documents.iter().position(|id| *id == tab_id).unwrap();
            pane.documents.remove(old);
            if source != destination {
                pane.states.remove(&tab_id);
                if let Some(ui_id) = pane.preview_ui_id {
                    self.markdown.forget_view_restore(tab_id, ui_id);
                }
                if pane.document == Some(tab_id) {
                    pane.document = pane
                        .documents
                        .get(old.min(pane.documents.len().saturating_sub(1)))
                        .copied();
                    pane.view = pane
                        .document
                        .and_then(|id| pane.states.get(&id))
                        .map_or(DocumentView::Source, |state| state.view);
                }
            }
            pane.tab_strip_active = None;
        }
        {
            let pane = &mut self.editor_layout.panes[destination];
            pane.documents.retain(|id| *id != tab_id);
            pane.documents.insert(index, tab_id);
            // A shared destination's cursor/view remains local; a newly moved view travels.
            // Explicit directional moves take the active source view with them.
            if prefer_source_state {
                pane.states.insert(tab_id, source_state.clone());
            }
            let state = pane.states.entry(tab_id).or_insert(source_state);
            pane.document = Some(tab_id);
            pane.empty_workspace = None;
            pane.view = state.view;
            pane.tab_strip_active = None;
        }
        self.editor_layout.focused = pane_id;
        self.render_pane = pane_id;
        self.completions = None;
        self.completion_request = None;
        self.collapse_empty_panes();
        self.restore_focused_pane(request_focus);
        if !request_focus && let Some(tab) = self.tab_mut() {
            // A drop runs after the frame's editors. Request focus from the next
            // rendered widget, once its AccessKit node exists in that pane.
            tab.pending_selection = Some(tab.selection.clone());
            tab.focus = tab.agent_view == DocumentView::Source;
        }
        if !request_focus {
            self.ctx.memory_mut(|memory| {
                if let Some(focused) = memory.focused() {
                    memory.surrender_focus(focused);
                }
            });
        }
        self.session_dirty = true;
        Ok(())
    }
    pub(super) fn tab_drag_pane_rects(&self) -> Vec<(u64, egui::Rect)> {
        self.editor_layout
            .rects
            .iter()
            .map(|(id, rect)| (*id, *rect))
            .collect()
    }
    pub(super) fn tab_drag_pane_ids(&self) -> Vec<u64> {
        self.editor_layout
            .panes
            .iter()
            .map(|pane| pane.id)
            .collect()
    }
    /// Remove only this group's reference. The caller releases the buffer on its last reference.
    pub(super) fn detach_pane_tab(&mut self, pane_id: u64, id: u64) -> Result<bool, String> {
        if !self.pane_contains_tab(pane_id, id) {
            return Err("Tab no longer belongs to this pane; inspect layout status".into());
        }
        self.capture_pane_navigation();
        if let Some(pane) = self
            .editor_layout
            .panes
            .iter_mut()
            .find(|pane| pane.id == pane_id)
            && let Some(index) = pane.documents.iter().position(|document| *document == id)
        {
            pane.documents.remove(index);
            pane.states.remove(&id);
            if let Some(ui_id) = pane.preview_ui_id {
                self.markdown.forget_view_restore(id, ui_id);
            }
            if pane.document == Some(id) {
                pane.document = pane
                    .documents
                    .get(index.min(pane.documents.len().saturating_sub(1)))
                    .copied();
                if let Some(state) = pane
                    .document
                    .and_then(|document| pane.states.get(&document))
                {
                    pane.view = state.view;
                } else {
                    pane.view = DocumentView::Source;
                }
            }
            pane.tab_strip_active = None;
        }
        let unreferenced = !self
            .editor_layout
            .panes
            .iter()
            .any(|pane| pane.documents.contains(&id));
        self.collapse_empty_panes();
        self.restore_focused_pane(true);
        self.session_dirty = true;
        Ok(unreferenced)
    }
    pub(super) fn remove_document_from_layout(&mut self, id: u64) {
        for project in &mut self.projects {
            if let Some(layout) = &mut project.layout {
                Self::prune_parked_document(layout, id);
            }
        }
        for pane in &mut self.editor_layout.panes {
            if let Some(index) = pane.documents.iter().position(|document| *document == id) {
                pane.documents.remove(index);
                if pane.document == Some(id) {
                    pane.document = pane
                        .documents
                        .get(index.min(pane.documents.len().saturating_sub(1)))
                        .copied();
                    pane.view = pane
                        .document
                        .and_then(|document| pane.states.get(&document))
                        .map_or(DocumentView::Source, |state| state.view);
                }
            }
            pane.states.remove(&id);
            if pane.tab_strip_active == Some(id) {
                pane.tab_strip_active = None;
            }
        }
        self.collapse_empty_panes();
        self.restore_focused_pane(true);
        self.session_dirty = true;
    }
    fn prune_parked_document(layout: &mut EditorLayout, id: u64) {
        for pane in &mut layout.panes {
            let old = pane.documents.iter().position(|document| *document == id);
            pane.documents.retain(|document| *document != id);
            pane.states.remove(&id);
            if pane.document == Some(id) {
                pane.document = old.and_then(|index| {
                    pane.documents
                        .get(index.min(pane.documents.len().saturating_sub(1)))
                        .copied()
                });
                pane.view = pane
                    .document
                    .and_then(|document| pane.states.get(&document))
                    .map_or(DocumentView::Source, |state| state.view);
            }
        }
        let empty: Vec<_> = layout
            .panes
            .iter()
            .filter(|pane| pane.documents.is_empty())
            .map(|pane| pane.id)
            .collect();
        for id in empty {
            if layout.panes.len() == 1 {
                break;
            }
            layout.root.remove(id);
            layout.panes.retain(|pane| pane.id != id);
            layout.rects.remove(&id);
            if layout.focused == id {
                layout.focused = layout.root.first_id();
            }
        }
    }
    /// Reconcile a global buffer removal without importing a project fallback into a group.
    pub(super) fn reconcile_layout_after_close(&mut self) {
        let open: std::collections::HashSet<_> = self
            .project()
            .into_iter()
            .flat_map(|project| project.tabs.iter().map(|tab| tab.id))
            .collect();
        let roots: std::collections::HashSet<_> = self
            .projects
            .iter()
            .map(|project| project.root.clone())
            .collect();
        for pane in &mut self.editor_layout.panes {
            pane.documents.retain(|id| open.contains(id));
            pane.states.retain(|id, _| open.contains(id));
            if pane
                .empty_workspace
                .as_ref()
                .is_some_and(|root| !roots.contains(root))
            {
                pane.empty_workspace = None;
            }
            if pane
                .document
                .is_some_and(|id| !pane.documents.contains(&id))
                || pane.document.is_none() && pane.empty_workspace.is_none()
            {
                pane.document = pane.documents.first().copied();
                pane.view = pane
                    .document
                    .and_then(|document| pane.states.get(&document))
                    .map_or(DocumentView::Source, |state| state.view);
            }
            if pane.document.is_some() {
                pane.empty_workspace = None;
            }
        }
        self.collapse_empty_panes();
        self.restore_focused_pane(true);
    }
    fn collapse_empty_panes(&mut self) {
        let empty: Vec<_> = self
            .editor_layout
            .panes
            .iter()
            .filter(|pane| pane.documents.is_empty())
            .map(|pane| pane.id)
            .collect();
        for id in empty {
            if self.editor_layout.panes.len() == 1 {
                break;
            }
            if let Some(ui_id) = self
                .editor_layout
                .panes
                .iter()
                .find(|pane| pane.id == id)
                .and_then(|pane| pane.preview_ui_id)
            {
                self.markdown.forget_pane_restores(ui_id);
            }
            let next = self.editor_layout.root.neighbor(id);
            self.editor_layout.root.remove(id);
            self.editor_layout.panes.retain(|pane| pane.id != id);
            self.editor_layout.rects.remove(&id);
            if self.editor_layout.focused == id {
                self.editor_layout.focused =
                    next.unwrap_or_else(|| self.editor_layout.root.first_id());
                self.render_pane = self.editor_layout.focused;
                self.completions = None;
                self.completion_request = None;
            }
        }
    }
    pub(super) fn tab_location(&self, id: u64) -> Option<(usize, usize)> {
        self.projects
            .iter()
            .enumerate()
            .find_map(|(pi, p)| p.tabs.iter().position(|t| t.id == id).map(|ti| (pi, ti)))
    }
    fn restore_focused_pane(&mut self, request_focus: bool) {
        self.shift_shared_document_positions();
        let Some(pane) = self
            .editor_layout
            .panes
            .iter()
            .find(|p| p.id == self.editor_layout.focused)
            .map(Pane::active_view)
        else {
            return;
        };
        let Some(id) = pane.document else {
            if let Some(index) = pane.empty_workspace.as_ref().and_then(|root| {
                self.projects
                    .iter()
                    .position(|project| &project.root == root)
            }) {
                self.set_active_project_context(index);
                self.render_pane = pane.id;
                if request_focus {
                    self.ctx.memory_mut(|memory| {
                        if let Some(focused) = memory.focused() {
                            memory.surrender_focus(focused);
                        }
                    });
                }
            }
            return;
        };
        let Some((pi, ti)) = self.tab_location(id) else {
            return;
        };
        self.set_active_project_context(pi);
        self.projects[pi].active_tab = ti;
        self.render_pane = pane.id;
        let tab = &mut self.projects[pi].tabs[ti];
        if let Some(state) = &pane.state {
            if tab.loading {
                tab.vim.cursor = state.navigation.cursor;
            } else {
                tab.vim.restore_navigation(&tab.text, &state.navigation);
            }
            tab.selection = if self.settings.vim
                && tab.vim.mode != VimMode::Insert
                && !tab.vim.multi.active()
            {
                tab.vim
                    .selection(&tab.text)
                    .unwrap_or(tab.vim.cursor..tab.vim.cursor)
            } else {
                state.selection.clone()
            };
        }
        tab.agent_view = pane.view;
        if request_focus {
            tab.pending_selection = Some(tab.selection.clone());
            tab.focus = pane.view == DocumentView::Source;
        }
        if request_focus {
            let widget = self.document_widget_id(id);
            self.ctx.memory_mut(|memory| {
                if pane.view == DocumentView::Source {
                    memory.request_focus(widget);
                } else if let Some(focused) = memory.focused() {
                    memory.surrender_focus(focused);
                }
            });
        }
    }
    pub(super) fn focus_editor_pane(&mut self, pane_id: u64) -> Result<(), String> {
        if !self
            .editor_layout
            .panes
            .iter()
            .any(|pane| pane.id == pane_id)
        {
            return Err("Pane no longer exists; inspect layout status".into());
        }
        self.find.break_escape_sequence();
        if pane_id == self.editor_layout.focused {
            return Ok(());
        }
        self.cancel_vim_search(false);
        if let Some(tab) = self.tab_mut()
            && tab.vim.finish_insert(&mut tab.text).text_changed
        {
            tab.changed();
        }
        self.capture_pane_navigation();
        self.editor_layout.focused = pane_id;
        self.render_pane = pane_id;
        self.completions = None;
        self.completion_request = None;
        self.restore_focused_pane(true);
        self.session_dirty = true;
        Ok(())
    }
    pub(super) fn focus_pane_from_pointer(&mut self) {
        if self.picker.is_some()
            || self.workspace_picker_open()
            || self.save_as.is_some()
            || self.settings_open
            || self.agent_marks_modal_open()
            || self.close_action.is_some()
            || self.palette.is_some()
            || self.context_file_dialog.is_some()
            || self.diff.is_some()
            || self.git_detail_open()
            || egui::Popup::is_any_open(&self.ctx)
        {
            return;
        }
        let position = self.ctx.input(|i| {
            i.pointer
                .any_pressed()
                .then(|| i.pointer.interact_pos())
                .flatten()
        });
        if let Some(position) = position {
            let pane = self
                .editor_layout
                .rects
                .iter()
                .find_map(|(id, rect)| rect.contains(position).then_some(*id));
            if let Some(id) = pane {
                if self.focused_sidebar().is_some() || self.output_keyboard_focused() {
                    self.focus_editor();
                }
                let _ = self.focus_editor_pane(id);
            }
        }
    }
    fn synchronize_layout(&mut self) {
        self.initialize_pane_groups();
        let open: std::collections::HashSet<_> = self
            .project()
            .into_iter()
            .flat_map(|p| p.tabs.iter().map(|t| t.id))
            .collect();
        let markdown_documents: std::collections::HashSet<_> = self
            .projects
            .iter()
            .flat_map(|project| &project.tabs)
            .filter(|tab| is_markdown(tab.path.as_deref()))
            .map(|tab| tab.id)
            .collect();
        let roots: std::collections::HashSet<_> = self
            .projects
            .iter()
            .map(|project| project.root.clone())
            .collect();
        for pane in &mut self.editor_layout.panes {
            pane.documents.retain(|id| open.contains(id));
            pane.states.retain(|id, _| open.contains(id));
            if pane
                .empty_workspace
                .as_ref()
                .is_some_and(|root| !roots.contains(root))
            {
                pane.empty_workspace = None;
            }
            if pane
                .document
                .is_some_and(|id| !pane.documents.contains(&id))
                || pane.document.is_none() && pane.empty_workspace.is_none()
            {
                pane.document = pane.documents.first().copied();
                pane.view = pane
                    .document
                    .and_then(|id| pane.states.get(&id))
                    .map_or(DocumentView::Source, |state| state.view);
            }
            if pane.document.is_some() {
                pane.empty_workspace = None;
            }
            if pane
                .document
                .is_some_and(|id| !markdown_documents.contains(&id))
            {
                pane.view = DocumentView::Source;
                for (id, state) in &mut pane.states {
                    if !markdown_documents.contains(id) {
                        state.view = DocumentView::Source;
                    }
                }
            }
        }
        if let Some(tab) = self.tab_mut()
            && !is_markdown(tab.path.as_deref())
        {
            tab.agent_view = DocumentView::Source;
        }
        self.collapse_empty_panes();
        self.capture_pane_navigation();
    }
    pub(super) fn split_active(&mut self, orientation: SplitOrientation) -> Result<u64, String> {
        self.synchronize_layout();
        if self.editor_layout.panes.len() >= MAX_PANES {
            return Err(format!("At most {MAX_PANES} panes can be open"));
        }
        if self.tab().is_none() {
            return Err("Open a document before splitting".into());
        }
        if let Some(tab) = self.tab_mut() {
            tab.preview_tab = false;
        }
        if let Some(tab) = self.tab_mut()
            && tab.vim.finish_insert(&mut tab.text).text_changed
        {
            tab.changed();
        }
        self.capture_pane_navigation();
        let target = self.editor_layout.focused;
        let id = self.allocate_pane_id();
        let mut pane = self
            .editor_layout
            .panes
            .iter()
            .find(|pane| pane.id == target)
            .cloned()
            .ok_or("Focused pane is unavailable")?;
        pane.id = id;
        pane.documents = pane.document.into_iter().collect();
        pane.states
            .retain(|document, _| Some(*document) == pane.document);
        pane.tab_strip_active = None;
        if !self.editor_layout.root.split(target, id, orientation) {
            return Err("Focused pane is unavailable".into());
        }
        self.editor_layout.panes.push(pane);
        self.focus_editor_pane(id)?;
        Ok(id)
    }
    pub(super) fn open_to_side(&mut self, path: PathBuf) {
        match self.split_active(SplitOrientation::Vertical) {
            Ok(id) => {
                if let Some(pane) = self
                    .editor_layout
                    .panes
                    .iter_mut()
                    .find(|pane| pane.id == id)
                {
                    pane.documents.clear();
                    pane.document = None;
                    pane.states.clear();
                }
                self.open_argument(path);
            }
            Err(error) => self.status(error, true),
        }
    }
    fn close_editor_pane(&mut self, id: u64) -> Result<(), String> {
        if self.editor_layout.panes.len() == 1 {
            return Err("The last pane stays open; close its document instead".into());
        }
        if !self.editor_layout.panes.iter().any(|pane| pane.id == id) {
            return Err("Pane no longer exists".into());
        }
        self.capture_pane_navigation();
        let documents = self
            .editor_layout
            .panes
            .iter()
            .find(|pane| pane.id == id)
            .map(|pane| pane.documents.clone())
            .unwrap_or_default();
        if self.editor_layout.focused == id {
            let next = self
                .editor_layout
                .root
                .neighbor(id)
                .ok_or("Pane has no remaining neighbor")?;
            self.focus_editor_pane(next)?;
        }
        let states = self
            .editor_layout
            .panes
            .iter()
            .find(|pane| pane.id == id)
            .map(|pane| pane.states.clone())
            .unwrap_or_default();
        if let Some(ui_id) = self
            .editor_layout
            .panes
            .iter()
            .find(|pane| pane.id == id)
            .and_then(|pane| pane.preview_ui_id)
        {
            self.markdown.forget_pane_restores(ui_id);
        }
        self.editor_layout.root.remove(id);
        self.editor_layout.panes.retain(|pane| pane.id != id);
        self.editor_layout.rects.remove(&id);
        let assigned: std::collections::HashSet<_> = self
            .editor_layout
            .panes
            .iter()
            .flat_map(|pane| pane.documents.iter().copied())
            .collect();
        if let Some(pane) = self
            .editor_layout
            .panes
            .iter_mut()
            .find(|pane| pane.id == self.editor_layout.focused)
        {
            for id in documents.into_iter().filter(|id| !assigned.contains(id)) {
                pane.documents.push(id);
                if let Some(state) = states.get(&id) {
                    pane.states.insert(id, state.clone());
                }
            }
        }
        self.adopt_orphan_documents();
        self.session_dirty = true;
        Ok(())
    }
    pub(super) fn layout_status(&self) -> Value {
        let panes: Vec<_> = self.editor_layout.panes.iter().map(|pane| {
            let current = self.tab().filter(|t| pane.id == self.editor_layout.focused && Some(t.id) == pane.document);
            let navigation = current.map(|t|t.vim.navigation()).or_else(||pane.document.and_then(|id|pane.states.get(&id)).map(|state|state.navigation.clone()));
            let location = pane.document.and_then(|id|self.tab_location(id)).map(|(pi,ti)| {
                let tab = &self.projects[pi].tabs[ti];
                let cursor = navigation.as_ref().map_or(0,|nav|nav.cursor);
                let (line,column) = tab.lines.line_col(&tab.text,tab.revision,cursor);
                json!({"line":line,"column":column})
            });
            json!({"id":pane.id,"tab_id":pane.document,"tab_ids":pane.documents,"view":current.map_or(pane.view,|tab|tab.agent_view),"focused":pane.id==self.editor_layout.focused,"location":location})
        }).collect();
        json!({"workspace":self.active_folder().map(crate::paths::display),"workspace_kind":self.project().map(|p|p.kind),"scope":"active_workspace","focused_pane":self.editor_layout.focused,"max_panes":MAX_PANES,"tree":self.editor_layout.root,"panes":panes})
    }
    pub(super) fn control_layout(&mut self, change: LayoutAction) -> Result<Value, String> {
        self.synchronize_layout();
        match change {
            LayoutAction::Status {} => {}
            LayoutAction::Split {
                orientation,
                pane_id,
                tab_id,
                view,
            } => {
                if self.editor_layout.panes.len() >= MAX_PANES {
                    return Err(format!(
                        "At most {MAX_PANES} panes can be open; no pane changed"
                    ));
                }
                if let Some(id) = pane_id
                    && !self.editor_layout.panes.iter().any(|pane| pane.id == id)
                {
                    return Err("Pane no longer exists; no pane changed".into());
                }
                if let Some(id) = tab_id {
                    let (project, _) = self.tab_location(id).ok_or("Tab no longer exists")?;
                    if project != self.active_project {
                        return Err("Tab belongs to another workspace; focus its tab or workspace first; no pane changed".into());
                    }
                }
                let target = tab_id
                    .or_else(|| {
                        pane_id.and_then(|id| {
                            self.editor_layout
                                .panes
                                .iter()
                                .find(|pane| pane.id == id)
                                .and_then(|pane| pane.document)
                        })
                    })
                    .or_else(|| self.tab().map(|tab| tab.id));
                if target.is_none() {
                    return Err("Open a document before splitting; no pane changed".into());
                }
                if view == Some(DocumentView::Preview)
                    && target
                        .and_then(|id| self.tab_location(id))
                        .is_none_or(|(pi, ti)| {
                            !is_markdown(self.projects[pi].tabs[ti].path.as_deref())
                        })
                {
                    return Err("Preview requires a Markdown document; no pane changed".into());
                }
                if let Some(id) = pane_id {
                    self.focus_editor_pane(id)?;
                }
                let new_id = self.split_active(orientation)?;
                if let Some(id) = tab_id {
                    if let Some(pane) = self
                        .editor_layout
                        .panes
                        .iter_mut()
                        .find(|pane| pane.id == new_id)
                    {
                        pane.documents.clear();
                        pane.document = None;
                        pane.states.clear();
                    }
                    self.assign_editor_pane(new_id, id, view)?;
                } else if let Some(view) = view {
                    self.set_document_view(view);
                }
            }
            LayoutAction::Focus { pane_id } => self.focus_editor_pane(pane_id)?,
            LayoutAction::Close { pane_id } => self.close_editor_pane(pane_id)?,
            LayoutAction::CloseTab { pane_id, tab_id } => {
                self.control_close_pane_tab(pane_id, tab_id)?;
            }
            LayoutAction::Assign {
                pane_id,
                tab_id,
                view,
            } => self.assign_editor_pane(pane_id, tab_id, view)?,
            LayoutAction::MoveTab {
                source_pane_id,
                pane_id,
                tab_id,
                index,
            } => {
                self.move_pane_tab(source_pane_id, pane_id, tab_id, index)?;
            }
            LayoutAction::Resize { pane_id, ratio } => {
                if !ratio.is_finite() || !(0.15..=0.85).contains(&ratio) {
                    return Err("Split ratio must be between 0.15 and 0.85".into());
                }
                if !self.editor_layout.root.resize(pane_id, ratio) {
                    return Err("Pane has no resizable parent split".into());
                }
                self.session_dirty = true;
            }
        }
        self.capture_pane_navigation();
        Ok(self.layout_status())
    }
    fn assign_editor_pane(
        &mut self,
        pane_id: u64,
        tab_id: u64,
        view: Option<DocumentView>,
    ) -> Result<(), String> {
        let (pi, ti) = self.tab_location(tab_id).ok_or("Tab no longer exists")?;
        if pi != self.active_project {
            return Err("Tab belongs to another workspace; focus its tab or workspace first; no pane changed".into());
        }
        if view == Some(DocumentView::Preview)
            && !is_markdown(self.projects[pi].tabs[ti].path.as_deref())
        {
            return Err("Preview requires a Markdown document".into());
        }
        self.focus_editor_pane(pane_id)?;
        self.keep_tab_open(tab_id);
        self.activate_tab(pi, ti);
        if let Some(view) = view {
            self.set_document_view(view);
        }
        Ok(())
    }
    pub(super) fn split_shortcuts(&mut self, _ctx: &egui::Context) -> bool {
        use keybindings::Action;
        for (binding, action) in [
            (Action::SplitRight, "right"),
            (Action::SplitDown, "down"),
            (Action::ClosePane, "close"),
            (Action::FocusLeft, "focus_left"),
            (Action::FocusRight, "focus_right"),
            (Action::FocusUp, "focus_up"),
            (Action::FocusDown, "focus_down"),
            (Action::FocusNextPane, "next"),
            (Action::FocusPreviousPane, "previous"),
            (Action::MoveTabLeft, "move_left"),
            (Action::MoveTabRight, "move_right"),
            (Action::MoveTabUp, "move_up"),
            (Action::MoveTabDown, "move_down"),
        ] {
            if self.take_shortcut(binding) {
                self.split_command(action);
                return true;
            }
        }
        false
    }
    pub(super) fn split_command(&mut self, action: &str) {
        // The hidden panes keep their arrangement while a change review is open.
        if self.change_review_visible()
            && matches!(
                action,
                "right"
                    | "down"
                    | "close"
                    | "next"
                    | "previous"
                    | "move_left"
                    | "move_right"
                    | "move_up"
                    | "move_down"
            )
        {
            self.status("Close the change review to arrange editor panes", true);
            return;
        }
        let result = match action {
            "right" => self.split_active(SplitOrientation::Vertical).map(|_| ()),
            "down" => self.split_active(SplitOrientation::Horizontal).map(|_| ()),
            "close" => self.close_editor_pane(self.editor_layout.focused),
            "focus_left" => self.focus_directional_pane(Vec2::new(-1.0, 0.0)),
            "focus_right" => self.focus_directional_pane(Vec2::new(1.0, 0.0)),
            "focus_up" => self.focus_directional_pane(Vec2::new(0.0, -1.0)),
            "focus_down" => self.focus_directional_pane(Vec2::new(0.0, 1.0)),
            "region_next" => {
                self.cycle_focus_region(false);
                Ok(())
            }
            "region_previous" => {
                self.cycle_focus_region(true);
                Ok(())
            }
            "move_left" => self.move_tab_direction(Vec2::new(-1.0, 0.0)),
            "move_right" => self.move_tab_direction(Vec2::new(1.0, 0.0)),
            "move_up" => self.move_tab_direction(Vec2::new(0.0, -1.0)),
            "move_down" => self.move_tab_direction(Vec2::new(0.0, 1.0)),
            "next" | "previous" => {
                let mut ids = Vec::new();
                self.editor_layout.root.ids(&mut ids);
                let index = ids
                    .iter()
                    .position(|id| *id == self.editor_layout.focused)
                    .unwrap_or(0);
                self.focus_editor();
                let next = if action == "previous" {
                    (index + ids.len() - 1) % ids.len()
                } else {
                    (index + 1) % ids.len()
                };
                self.focus_editor_pane(ids[next])
            }
            _ => Ok(()),
        };
        if let Err(error) = result {
            self.status(error, true);
        } else {
            self.defer_region_focus_owner();
        }
    }
    pub(super) fn split_menu_ui(&mut self, ui: &mut egui::Ui) {
        use keybindings::Action;
        ui.separator();
        for (binding, action) in [
            (Action::SplitRight, "right"),
            (Action::SplitDown, "down"),
            (Action::FocusNextRegion, "region_next"),
            (Action::FocusPreviousRegion, "region_previous"),
            (Action::FocusLeft, "focus_left"),
            (Action::FocusRight, "focus_right"),
            (Action::FocusUp, "focus_up"),
            (Action::FocusDown, "focus_down"),
            (Action::ClosePane, "close"),
        ] {
            if design::menu_item(
                ui,
                binding.label(),
                &self.shortcut_label(binding),
                "Navigate visible regions and arrange editor panes; documents stay open",
            )
            .clicked()
            {
                self.split_command(action);
                ui.close();
            }
        }
        ui.menu_button("Move tab to pane", |ui| {
            for (binding, action) in [
                (Action::MoveTabLeft, "move_left"),
                (Action::MoveTabRight, "move_right"),
                (Action::MoveTabUp, "move_up"),
                (Action::MoveTabDown, "move_down"),
            ] {
                if design::menu_item(
                    ui,
                    binding.label(),
                    &self.shortcut_label(binding),
                    "Move this tab to the neighboring pane, keeping its unsaved text and cursor",
                )
                .clicked()
                {
                    self.split_command(action);
                    ui.close();
                }
            }
        });
    }

    /// Follow the visible workspace geometry, including both sidebars and Output.
    fn focus_directional_pane(&mut self, direction: Vec2) -> Result<(), String> {
        self.focus_spatial_region(direction)
    }

    fn region_focus_blocked(&self) -> bool {
        self.picker.is_some()
            || self.settings_open
            || self.agent_marks_modal_open()
            || self.save_as.is_some()
            || self.close_action.is_some()
            || self.diff.is_some()
            || self.git_detail_open()
            || self.navigation_modal_open()
            || self.palette.is_some()
            || self.workspace_picker_open()
            || self.context_file_dialog.is_some()
            || self.vim_command.is_some()
            || self.tab_drag_active()
            || egui::Popup::is_any_open(&self.ctx)
    }

    fn region_input_owner(&self) -> RegionInputOwner {
        RegionInputOwner {
            project: self.project().map(|project| project.root.clone()),
            pane: self.current_pane_id(),
            document: self.tab().map(|tab| {
                (
                    tab.id,
                    tab.revision,
                    tab.vim.cursor,
                    tab.selection.clone(),
                    tab.vim.mode,
                )
            }),
            widget: self.ctx.memory(|memory| memory.focused()),
            vim_enabled: self.settings.vim,
            vim_swap_jk: self.settings.swap_jk,
            leader_key: self.settings.leader_key.clone(),
            view: self.tab().map(|tab| tab.agent_view),
        }
    }

    pub(super) fn resume_region_input(&mut self) {
        if self.region_focus.awaiting_mark_owner {
            return;
        }
        if self.region_focus.awaiting_region.is_some() {
            // A newly visible region must register its real widget before any
            // queued input is handed to it, including fresh input this frame.
            let events = self
                .ctx
                .input_mut(|input| std::mem::take(&mut input.events));
            self.region_focus.trailing.extend(events);
            return;
        }
        if !self.region_focus.trailing.is_empty() {
            let mut events = std::mem::take(&mut self.region_focus.trailing);
            if self.region_focus.owner.take().as_ref() != Some(&self.region_input_owner()) {
                return;
            }
            self.ctx.input_mut(|input| {
                events.append(&mut input.events);
                input.events = events;
            });
        }
    }

    /// Render prefix input in its original widget before changing ownership.
    /// Events after the shortcut resume in the destination on the next frame.
    pub(super) fn focus_region_shortcuts(&mut self, ctx: &egui::Context) -> bool {
        if self.region_focus_blocked() {
            return false;
        }
        let shortcut = ctx.input(|input| {
            input.events.iter().enumerate().find_map(|(index, event)| {
                use keybindings::Action;
                let action = [
                    (Action::FocusExplorer, RegionFocusAction::Explorer),
                    (Action::FocusEditor, RegionFocusAction::Editor),
                    (Action::FocusGit, RegionFocusAction::Git),
                    (Action::FocusOutput, RegionFocusAction::Output),
                    (Action::FocusNextRegion, RegionFocusAction::Cycle(false)),
                    (Action::FocusPreviousRegion, RegionFocusAction::Cycle(true)),
                    (
                        Action::FocusLeft,
                        RegionFocusAction::Direction("focus_left"),
                    ),
                    (
                        Action::FocusRight,
                        RegionFocusAction::Direction("focus_right"),
                    ),
                    (Action::FocusUp, RegionFocusAction::Direction("focus_up")),
                    (
                        Action::FocusDown,
                        RegionFocusAction::Direction("focus_down"),
                    ),
                    (Action::FocusNextPane, RegionFocusAction::Direction("next")),
                    (
                        Action::FocusPreviousPane,
                        RegionFocusAction::Direction("previous"),
                    ),
                    (
                        Action::MoveTabLeft,
                        RegionFocusAction::Direction("move_left"),
                    ),
                    (
                        Action::MoveTabRight,
                        RegionFocusAction::Direction("move_right"),
                    ),
                    (Action::MoveTabUp, RegionFocusAction::Direction("move_up")),
                    (
                        Action::MoveTabDown,
                        RegionFocusAction::Direction("move_down"),
                    ),
                    (Action::SplitRight, RegionFocusAction::Direction("right")),
                    (Action::SplitDown, RegionFocusAction::Direction("down")),
                    (Action::ClosePane, RegionFocusAction::Direction("close")),
                    (Action::AgentMark, RegionFocusAction::AgentMark(false)),
                    (Action::AgentMarks, RegionFocusAction::AgentMark(true)),
                ]
                .into_iter()
                .find_map(|(binding, action)| {
                    if matches!(action, RegionFocusAction::AgentMark(_))
                        && (self.vim_search_active()
                            || self.navigation_pending()
                            || self.screen_jump.is_some())
                    {
                        return None;
                    }
                    self.shortcut_matches(binding, event).then_some(action)
                })?;
                Some((index, action))
            })
        });
        if let Some((index, action)) = shortcut {
            if self.earlier_shortcut_boundary(index) {
                return false;
            }
            self.region_focus.trailing = ctx.input_mut(|input| {
                let mut trailing = input.events.split_off(index + 1);
                self.region_focus.boundary_event = input.events.pop();
                if let Some(egui::Event::Key { key, modifiers, .. }) = &self.region_focus.boundary_event
                    && modifiers.alt && !modifiers.ctrl && !modifiers.command
                    && key.name().chars().count() == 1
                    && matches!(trailing.first(), Some(egui::Event::Text(text)) if text.eq_ignore_ascii_case(key.name()))
                {
                    // Some platforms emit the printable Alt chord as text too.
                    // Remove only that character, never the user's queued suffix.
                    trailing.remove(0);
                }
                trailing
            });
            self.region_focus.pending = Some(action);
            return true;
        }
        false
    }

    pub(super) fn finish_region_focus(&mut self) {
        if let Some(action) = self.region_focus.pending.take() {
            self.region_focus.boundary_event = None;
            self.find.break_escape_sequence();
            self.remember_spatial_anchor();
            let mark_blocked = matches!(action, RegionFocusAction::AgentMark(_))
                && (self.vim_search_active()
                    || self.navigation_pending()
                    || self.screen_jump.is_some());
            if !self.region_focus_blocked() && !mark_blocked {
                match action {
                    RegionFocusAction::Explorer => self.focus_explorer(),
                    RegionFocusAction::Editor => self.focus_editor(),
                    RegionFocusAction::Git => {
                        self.show_sidebar(SidebarView::SourceControl);
                        self.focus_sidebar_region(self.sidebar_side(SidebarView::SourceControl));
                    }
                    RegionFocusAction::Output => {
                        self.output_visible = true;
                        self.focus_output_region();
                    }
                    RegionFocusAction::Cycle(backwards) => self.cycle_focus_region(backwards),
                    RegionFocusAction::Direction(action) => self.split_command(action),
                    RegionFocusAction::AgentMark(open) => {
                        // TextEdit has applied the prefix, but leaves its raw
                        // events available. A newly focused dialog must never
                        // receive the same text a second time.
                        self.ctx.input_mut(|input| input.events.clear());
                        if open {
                            self.open_agent_marks();
                        } else {
                            self.capture_agent_mark();
                        }
                        self.region_focus.awaiting_mark_owner = true;
                        self.region_focus.mark_destination_filter = open;
                    }
                }
                if !matches!(action, RegionFocusAction::AgentMark(_)) {
                    self.defer_region_focus_owner();
                }
            }
            self.region_focus.owner = Some(self.region_input_owner());
            self.ctx.request_repaint();
        }
    }

    /// Keep queued source input before a leader action in its original editor.
    /// Its suffix resumes only after rendering and establishing the new owner.
    pub(super) fn defer_agent_mark_input(&mut self, open: bool, mut trailing: Vec<egui::Event>) {
        // A leader command can precede a region shortcut already split out of
        // this frame. Replay that boundary after the earlier leader action.
        if self.region_focus.pending.is_some() {
            trailing.extend(self.region_focus.boundary_event.take());
            trailing.append(&mut self.region_focus.trailing);
        }
        self.region_focus.pending = Some(RegionFocusAction::AgentMark(open));
        self.region_focus.trailing = trailing;
    }

    pub(super) fn finish_agent_mark_input_owner(&mut self) {
        if !self.region_focus.awaiting_mark_owner {
            return;
        }
        if self.region_focus.mark_destination_filter {
            if !self.agent_marks_modal_open() {
                self.region_focus.trailing.clear();
            } else if self.ctx.memory(|memory| memory.focused())
                != Some(egui::Id::new("agent_marks_filter"))
            {
                // A new Modal first renders a sizing pass. Hold its suffix
                // until the filter actually owns keyboard input.
                self.ctx.request_repaint();
                return;
            }
        }
        self.region_focus.awaiting_mark_owner = false;
        self.region_focus.owner = Some(self.region_input_owner());
    }

    pub(super) fn cycle_focus_region(&mut self, backwards: bool) {
        self.cycle_spatial_region(backwards);
    }
    pub(super) fn editor_session_layout(&self) -> SessionLayout {
        self.serialize_project_layout(&self.editor_layout)
    }
    pub(super) fn project_session_layout(&self, root: &Path) -> Option<SessionLayout> {
        if self.layout_project.as_deref() == Some(root) {
            Some(self.editor_session_layout())
        } else {
            self.projects
                .iter()
                .find(|project| project.root == root)
                .and_then(|project| project.layout.as_ref())
                .map(|layout| self.serialize_project_layout(layout))
        }
    }
    fn serialize_project_layout(&self, layout: &EditorLayout) -> SessionLayout {
        let active = std::ptr::eq(layout, &self.editor_layout);
        let panes = layout
            .panes
            .iter()
            .map(|pane| {
                let location = pane.document.and_then(|id| self.tab_location(id));
                let states: Vec<_> = pane
                    .documents
                    .iter()
                    .filter_map(|id| {
                        let (project, index) = self.tab_location(*id)?;
                        let tab = &self.projects[project].tabs[index];
                        let current =
                            active && pane.id == layout.focused && pane.document == Some(*id);
                        let state = pane.states.get(id);
                        let source_scroll = self
                            .find
                            .restore_scroll
                            .get(&(pane.id, *id))
                            .copied()
                            .or_else(|| {
                                self.find
                                    .view_scroll
                                    .get(&(pane.id, *id))
                                    .map(|(_, offset)| *offset)
                            })
                            .map(|offset| [offset.x, offset.y])
                            .or_else(|| state.and_then(|state| state.source_scroll));
                        let preview_scroll = pane
                            .preview_ui_id
                            .and_then(|pane| self.markdown.view_scroll(*id, pane))
                            .or_else(|| state.and_then(|state| state.preview_scroll));
                        Some(SessionPaneDocument {
                            project,
                            tab: index,
                            view: if current {
                                tab.agent_view
                            } else {
                                state.map_or(tab.agent_view, |state| state.view)
                            },
                            cursor: if current {
                                tab.vim.cursor
                            } else {
                                state.map_or(tab.vim.cursor, |state| state.navigation.cursor)
                            },
                            selection: if current {
                                tab.selection.clone()
                            } else {
                                state.map_or_else(
                                    || tab.selection.clone(),
                                    |state| state.selection.clone(),
                                )
                            },
                            source_scroll,
                            preview_scroll,
                        })
                    })
                    .collect();
                let current = pane.document.and_then(|id| {
                    states
                        .iter()
                        .find(|state| self.projects[state.project].tabs[state.tab].id == id)
                });
                SessionPane {
                    id: pane.id,
                    project: location.map(|location| location.0),
                    tab: location.map(|location| location.1),
                    view: current.map_or(pane.view, |state| state.view),
                    cursor: current.map_or(0, |state| state.cursor),
                    empty_workspace: pane.empty_workspace.clone(),
                    documents: Some(
                        pane.documents
                            .iter()
                            .filter_map(|id| self.tab_location(*id))
                            .collect(),
                    ),
                    states,
                }
            })
            .collect();
        SessionLayout {
            root: layout.root.clone(),
            panes,
            focused: layout.focused,
        }
    }
    /// Old sessions may mix projects in every group. Filter each project independently,
    /// retaining its split ratios, views and all buffers, then give every pane a unique ID.
    fn decode_project_layout(
        &mut self,
        session: Option<SessionLayout>,
        project: usize,
        used: &mut std::collections::HashSet<u64>,
    ) -> EditorLayout {
        let root = self.projects[project].root.clone();
        let Some(session) = session else {
            return self.empty_project_layout(root);
        };
        let legacy_shared_strip = session
            .panes
            .iter()
            .find(|pane| pane.id == session.focused)
            .is_some_and(|pane| pane.documents.is_none());
        let mut ids = Vec::new();
        session.root.ids(&mut ids);
        let unique: std::collections::HashSet<_> = ids.iter().copied().collect();
        if ids.is_empty()
            || ids.len() > MAX_PANES
            || unique.len() != ids.len()
            || unique.contains(&0)
            || !unique.contains(&session.focused)
        {
            return self.empty_project_layout(root);
        }
        let mut panes = Vec::new();
        for id in ids {
            let mut pane = Pane::empty(id);
            if let Some(saved) = session.panes.iter().find(|pane| pane.id == id) {
                pane.empty_workspace = saved
                    .empty_workspace
                    .as_ref()
                    .filter(|owner| **owner == root)
                    .cloned();
                if let Some(documents) = &saved.documents {
                    for (pi, ti) in documents {
                        if *pi == project
                            && let Some(tab) = self.projects[project].tabs.get(*ti)
                            && !pane.documents.contains(&tab.id)
                        {
                            pane.documents.push(tab.id);
                        }
                    }
                }
                for state in &saved.states {
                    if state.project == project
                        && let Some(tab) = self.projects[project].tabs.get(state.tab)
                        && pane.documents.contains(&tab.id)
                    {
                        let mut navigation = tab.vim.navigation();
                        navigation.cursor = state.cursor;
                        pane.states.insert(
                            tab.id,
                            PaneDocumentState {
                                navigation,
                                view: state.view,
                                selection: state.selection.clone(),
                                source_scroll: state.source_scroll.filter(|offset| {
                                    offset
                                        .iter()
                                        .all(|value| value.is_finite() && *value >= 0.0)
                                }),
                                preview_scroll: state
                                    .preview_scroll
                                    .filter(|offset| offset.is_finite() && *offset >= 0.0),
                                revision: tab.revision,
                            },
                        );
                    }
                }
                if saved.project == Some(project)
                    && let Some(tab) = saved.tab.and_then(|ti| self.projects[project].tabs.get(ti))
                {
                    if !pane.documents.contains(&tab.id) {
                        pane.documents.push(tab.id);
                    }
                    pane.document = Some(tab.id);
                    pane.empty_workspace = None;
                    pane.view = saved.view;
                    let mut navigation = tab.vim.navigation();
                    navigation.cursor = saved.cursor;
                    pane.states.entry(tab.id).or_insert(PaneDocumentState {
                        navigation,
                        view: saved.view,
                        selection: saved.cursor..saved.cursor,
                        source_scroll: None,
                        preview_scroll: None,
                        revision: tab.revision,
                    });
                } else {
                    pane.document = pane.documents.first().copied();
                    pane.view = pane
                        .document
                        .and_then(|id| pane.states.get(&id))
                        .map_or(DocumentView::Source, |state| state.view);
                }
                if !pane.documents.is_empty() || pane.empty_workspace.is_some() {
                    panes.push(pane);
                }
            }
        }
        let retained = panes.iter().map(|pane| pane.id).collect();
        let Some(mut tree) = session.root.retain_panes(&retained) else {
            return self.empty_project_layout(root);
        };
        let mut focused = if retained.contains(&session.focused) {
            session.focused
        } else {
            tree.first_id()
        };
        let mut remap = HashMap::new();
        for pane in &mut panes {
            let old = pane.id;
            if used.contains(&old) {
                pane.id = self.allocate_pane_id();
            }
            used.insert(pane.id);
            remap.insert(old, pane.id);
            if focused == old {
                focused = pane.id;
            }
        }
        tree.remap_ids(&remap);
        if legacy_shared_strip && let Some(pane) = panes.iter_mut().find(|pane| pane.id == focused)
        {
            // Before pane-local groups, every open buffer appeared in one shared
            // strip. Keep that order in this project's focused group, including
            // documents also displayed by its other panes.
            pane.documents = self.projects[project]
                .tabs
                .iter()
                .map(|tab| tab.id)
                .collect();
            for tab in &self.projects[project].tabs {
                pane.states.entry(tab.id).or_insert(PaneDocumentState {
                    navigation: tab.vim.navigation(),
                    view: tab.agent_view,
                    selection: tab.selection.clone(),
                    source_scroll: None,
                    preview_scroll: None,
                    revision: tab.revision,
                });
            }
        }
        EditorLayout {
            root: tree,
            panes,
            focused,
            rects: HashMap::new(),
            groups_initialized: true,
        }
    }
    pub(super) fn restore_project_layouts(
        &mut self,
        entries: Vec<(usize, Option<SessionLayout>)>,
        active: usize,
    ) {
        // Reserve all saved IDs before creating defaults or resolving migration collisions.
        self.next_pane_id = entries
            .iter()
            .filter_map(|(_, session)| session.as_ref())
            .flat_map(|session| session.panes.iter().map(|pane| pane.id))
            .max()
            .unwrap_or(1)
            .saturating_add(1)
            .max(self.next_pane_id);
        let mut used = std::collections::HashSet::new();
        for (project, saved) in entries {
            let layout = self.decode_project_layout(saved, project, &mut used);
            self.projects[project].layout = Some(layout);
        }
        self.layout_project = None;
        self.active_project = active.min(self.projects.len().saturating_sub(1));
        self.activate_project_layout(self.active_project);
        self.initialize_pane_groups();
        self.restore_focused_pane(false);
    }
    /// Compatibility entry point for layout tests and old single-layout sessions.
    #[cfg(test)]
    pub(super) fn restore_editor_layout(&mut self, session: Option<SessionLayout>) {
        if session.is_none() {
            return;
        }
        let entries = (0..self.projects.len())
            .map(|project| (project, session.clone()))
            .collect();
        self.restore_project_layouts(entries, self.active_project);
    }
    pub(super) fn split_editor_ui(&mut self, ui: &mut egui::Ui) {
        self.synchronize_layout();
        self.begin_tab_drag_frame();
        self.focus_pane_from_pointer();
        let node = self.editor_layout.root.clone();
        let rect = ui.available_rect_before_wrap();
        self.rendering_panes = true;
        self.render_split_node(ui, &node, rect);
        self.rendering_panes = false;
        self.restore_focused_pane(false);
        self.finish_tab_drag_frame(ui);
        ui.allocate_rect(rect, egui::Sense::hover());
    }
    fn render_split_node(&mut self, ui: &mut egui::Ui, node: &PaneNode, rect: egui::Rect) {
        match node {
            PaneNode::Pane { id } => self.render_editor_pane(ui, *id, rect),
            PaneNode::Split {
                orientation,
                ratio,
                first,
                second,
            } => {
                let ratio = ratio.clamp(0.15, 0.85);
                let vertical = *orientation == SplitOrientation::Vertical;
                let length = if vertical {
                    rect.width()
                } else {
                    rect.height()
                };
                let size = (length - 5.0).max(0.0) * ratio;
                let (a, divider, b) = if vertical {
                    let x = rect.left() + size;
                    (
                        egui::Rect::from_min_max(rect.min, egui::pos2(x, rect.bottom())),
                        egui::Rect::from_min_max(
                            egui::pos2(x, rect.top()),
                            egui::pos2(x + 5.0, rect.bottom()),
                        ),
                        egui::Rect::from_min_max(egui::pos2(x + 5.0, rect.top()), rect.max),
                    )
                } else {
                    let y = rect.top() + size;
                    (
                        egui::Rect::from_min_max(rect.min, egui::pos2(rect.right(), y)),
                        egui::Rect::from_min_max(
                            egui::pos2(rect.left(), y),
                            egui::pos2(rect.right(), y + 5.0),
                        ),
                        egui::Rect::from_min_max(egui::pos2(rect.left(), y + 5.0), rect.max),
                    )
                };
                self.render_split_node(ui, first, a);
                self.render_split_node(ui, second, b);
                let response = ui.interact(
                    divider,
                    ui.id()
                        .with(("pane_divider", first.first_id(), second.first_id())),
                    if self.diff.is_some() || self.git_detail_open() {
                        egui::Sense::hover()
                    } else {
                        egui::Sense::drag()
                    },
                );
                ui.painter().rect_filled(
                    divider,
                    0.0,
                    if response.hovered() || response.dragged() {
                        ACCENT.linear_multiply(0.4)
                    } else {
                        BORDER
                    },
                );
                if response.dragged()
                    && let Some(position) = response.interact_pointer_pos()
                {
                    let next = if vertical {
                        (position.x - rect.left()) / (length - 5.0).max(1.0)
                    } else {
                        (position.y - rect.top()) / (length - 5.0).max(1.0)
                    };
                    self.editor_layout.root.resize_divider(
                        first.first_id(),
                        second.first_id(),
                        next.clamp(0.15, 0.85),
                    );
                    self.session_dirty = true;
                }
                response
                    .on_hover_cursor(if vertical {
                        egui::CursorIcon::ResizeHorizontal
                    } else {
                        egui::CursorIcon::ResizeVertical
                    })
                    .on_hover_text("Drag to resize editor panes");
            }
        }
    }
    fn render_editor_pane(&mut self, ui: &mut egui::Ui, pane_id: u64, rect: egui::Rect) {
        // Native typing in a pane drawn earlier this frame may have changed the text.
        self.shift_shared_document_positions();
        let Some(pane) = self
            .editor_layout
            .panes
            .iter()
            .find(|p| p.id == pane_id)
            .map(Pane::active_view)
        else {
            return;
        };
        self.editor_layout.rects.insert(pane_id, rect);
        let active_before = (
            self.active_project,
            self.project().map_or(0, |p| p.active_tab),
        );
        let metrics = (
            self.editor_focused,
            self.editor_rect,
            self.editor_cursor_rect,
        );
        self.render_pane = pane_id;
        let inactive = pane_id != self.editor_layout.focused;
        let mut previous = None;
        if let Some((pi, ti)) = pane.document.and_then(|id| self.tab_location(id)) {
            self.active_project = pi;
            self.projects[pi].active_tab = ti;
            let tab = &mut self.projects[pi].tabs[ti];
            if inactive {
                previous = Some((
                    tab.id,
                    tab.vim.navigation(),
                    tab.agent_view,
                    tab.selection.clone(),
                    tab.pending_selection.clone(),
                    tab.focus,
                    tab.scroll_to_cursor,
                ));
                if let Some(state) = &pane.state {
                    if tab.loading {
                        tab.vim.cursor = state.navigation.cursor;
                    } else {
                        tab.vim.restore_navigation(&tab.text, &state.navigation);
                    }
                    tab.selection = if self.settings.vim && tab.vim.mode != VimMode::Insert {
                        tab.vim
                            .selection(&tab.text)
                            .unwrap_or(tab.vim.cursor..tab.vim.cursor)
                    } else {
                        state.selection.clone()
                    };
                }
                tab.pending_selection = Some(tab.selection.clone());
                tab.focus = false;
                tab.scroll_to_cursor = false;
            }
            tab.agent_view = pane.view;
        } else if let Some(pi) = pane.empty_workspace.as_ref().and_then(|root| {
            self.projects
                .iter()
                .position(|project| &project.root == root)
        }) {
            // Rendering another pane temporarily borrows its workspace, without
            // switching the focused pane's search or exposing another document.
            self.active_project = pi;
        }
        self.tab_strip_active = pane.tab_strip_active;
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(rect)
                .id_salt(("editor_pane", pane_id)),
            |ui| {
                self.register_tab_drag_pane_ui(pane_id, ui.id());
                if let Some(pane) = self
                    .editor_layout
                    .panes
                    .iter_mut()
                    .find(|pane| pane.id == pane_id)
                    && pane.preview_ui_id != Some(ui.id())
                {
                    if let Some(previous) = pane.preview_ui_id {
                        self.markdown.forget_pane_restores(previous);
                    }
                    pane.preview_ui_id = Some(ui.id());
                    for (document, state) in &pane.states {
                        if let Some(offset) = state.preview_scroll {
                            self.markdown
                                .restore_view_scroll(*document, ui.id(), offset);
                        }
                    }
                }
                ui.set_clip_rect(rect.intersect(ui.clip_rect()));
                ui.spacing_mut().item_spacing.y = 0.0;
                // These overlays are drawn later. Disable pane actions immediately,
                // including the first frame before egui registers the modal layer.
                if self.diff.is_some() || self.git_detail_open() {
                    ui.disable();
                }
                self.editor_pane_ui(ui);
            },
        );
        self.capture_pane_navigation();
        if let Some(pane) = self
            .editor_layout
            .panes
            .iter_mut()
            .find(|p| p.id == pane_id)
        {
            pane.tab_strip_active = self.tab_strip_active;
        }
        if let Some((id, navigation, view, selection, pending, focus, scroll)) = previous
            && let Some((pi, ti)) = self.tab_location(id)
        {
            let tab = &mut self.projects[pi].tabs[ti];
            if tab.loading {
                tab.vim.cursor = navigation.cursor;
            } else {
                tab.vim.restore_navigation(&tab.text, &navigation);
            }
            tab.agent_view = view;
            tab.selection = selection;
            tab.pending_selection = pending;
            tab.focus = focus;
            tab.scroll_to_cursor = scroll;
        }
        if inactive {
            self.editor_focused = metrics.0;
            self.editor_rect = metrics.1;
            self.editor_cursor_rect = metrics.2;
        }
        self.active_project = active_before.0;
        if let Some(project) = self.projects.get_mut(active_before.0) {
            project.active_tab = active_before.1.min(project.tabs.len().saturating_sub(1));
        }
    }
}

#[cfg(test)]
#[path = "empty_workspace_tests.rs"]
mod empty_workspace_tests;
#[cfg(test)]
#[path = "tab_drag_tests.rs"]
mod tab_drag_tests;
#[cfg(test)]
#[path = "split_tests.rs"]
mod tests;

#[path = "spatial_navigation.rs"]
mod spatial_navigation;

//! One active panel per sidebar, with persisted assignments and quiet headers.
use super::*;

const MIN_SIDEBAR: f32 = 200.0;
const MAX_SIDEBAR: f32 = 520.0;
const MIN_EDITOR: f32 = 320.0;
const VIEWS: [SidebarView; 4] = [
    SidebarView::Explorer,
    SidebarView::Search,
    SidebarView::SourceControl,
    SidebarView::Review,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Side {
    Left,
    Right,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct SidebarSession {
    left: SidebarView,
    left_visible: bool,
    right: SidebarView,
    right_visible: bool,
    left_width: f32,
    right_width: f32,
    right_views: Vec<SidebarView>,
    // None migrates sessions from the former single-panel sidebars. Some([])
    // deliberately preserves an empty layout.
    open_views: Option<Vec<SidebarView>>,
    collapsed_views: Vec<SidebarView>,
}

impl Default for SidebarSession {
    fn default() -> Self {
        Self {
            left: SidebarView::Explorer,
            left_visible: true,
            right: SidebarView::SourceControl,
            right_visible: false,
            left_width: 272.0,
            right_width: 310.0,
            right_views: vec![SidebarView::SourceControl],
            open_views: None,
            collapsed_views: Vec::new(),
        }
    }
}

pub(super) struct SidebarLayout {
    pub(super) right: SidebarView,
    pub(super) right_visible: bool,
    left_width: f32,
    right_width: f32,
    right_views: Vec<SidebarView>,
    open_views: Vec<SidebarView>,
    view_rects: [Option<egui::Rect>; 4],
    collapsed: [bool; 2],
    compact_preferred: Side,
    rects: [Option<egui::Rect>; 2],
    last_widgets: [Option<egui::Id>; 4],
    available_width: Option<f32>,
}

impl Default for SidebarLayout {
    fn default() -> Self {
        let defaults = SidebarSession::default();
        Self {
            right: defaults.right,
            right_visible: defaults.right_visible,
            left_width: defaults.left_width,
            right_width: defaults.right_width,
            right_views: defaults.right_views,
            open_views: vec![SidebarView::Explorer],
            view_rects: [None; 4],
            collapsed: [false; 2],
            compact_preferred: Side::Left,
            rects: [None; 2],
            last_widgets: [None; 4],
            available_width: None,
        }
    }
}

impl SidebarView {
    fn index(self) -> usize {
        VIEWS.iter().position(|view| *view == self).unwrap()
    }

    fn title(self) -> &'static str {
        match self {
            Self::Explorer => "Explorer",
            Self::Search => "Search",
            Self::SourceControl => "Source control",
            Self::Review => "Review",
        }
    }
}

fn valid_width(width: f32, fallback: f32) -> f32 {
    if width.is_finite() {
        width.clamp(MIN_SIDEBAR, MAX_SIDEBAR)
    } else {
        fallback
    }
}

fn sidebar_focus_id(side: Side) -> egui::Id {
    egui::Id::new(("sidebar_region_focus", side == Side::Right))
}

/// Share the available width while keeping the source usable at the minimum
/// window size. The second sidebar can use the space the first leaves unused.
fn sidebar_limits(available: f32, both: bool, other_width: f32) -> (f32, f32) {
    let editor = MIN_EDITOR
        .min((available - MIN_SIDEBAR).max(120.0))
        .min(available);
    let budget = (available - editor).max(0.0);
    let other = if both {
        other_width.min(budget * 0.5)
    } else {
        0.0
    };
    let maximum = (budget - other).clamp(0.0, MAX_SIDEBAR);
    (MIN_SIDEBAR.min(maximum), maximum)
}

impl VEditApp {
    pub(super) fn sidebar_session(&self) -> SidebarSession {
        SidebarSession {
            left: self.sidebar,
            left_visible: self.sidebar_visible,
            right: self.sidebar_layout.right,
            right_visible: self.sidebar_layout.right_visible,
            left_width: self.sidebar_layout.left_width,
            right_width: self.sidebar_layout.right_width,
            right_views: self.sidebar_layout.right_views.clone(),
            open_views: Some(self.sidebar_layout.open_views.clone()),
            collapsed_views: Vec::new(),
        }
    }

    pub(super) fn restore_sidebars(
        &mut self,
        saved: Option<SidebarSession>,
        legacy: Option<SidebarView>,
    ) {
        let legacy_session = saved.is_none();
        let mut saved = saved.unwrap_or_else(|| SidebarSession {
            left: legacy.unwrap_or_default(),
            ..Default::default()
        });
        // Only the old single-panel format inferred assignments from its
        // selected contents. Explicit new assignments must survive empty sides.
        if saved.open_views.is_none() && saved.left == saved.right {
            saved.right = if saved.left == SidebarView::Explorer {
                SidebarView::SourceControl
            } else {
                SidebarView::Explorer
            };
        }
        if legacy_session {
            saved.right_views = vec![saved.right];
        }
        if saved.open_views.is_none() {
            saved.right_views.retain(|view| *view != saved.left);
            if !saved.right_views.contains(&saved.right) {
                saved.right_views.push(saved.right);
            }
        }
        let mut open_views = saved.open_views.unwrap_or_else(|| {
            let mut views = vec![saved.left];
            if saved.right_visible && saved.right_views.contains(&saved.right) {
                views.push(saved.right);
            }
            views
        });
        let mut seen = Vec::new();
        open_views.retain(|view| {
            if seen.contains(view) {
                false
            } else {
                seen.push(*view);
                true
            }
        });
        self.sidebar = saved.left;
        self.sidebar_visible = saved.left_visible;
        self.sidebar_layout = SidebarLayout {
            right: saved.right,
            right_visible: saved.right_visible,
            left_width: valid_width(saved.left_width, 272.0),
            right_width: valid_width(saved.right_width, 310.0),
            right_views: saved.right_views,
            open_views,
            view_rects: [None; 4],
            collapsed: [false; 2],
            compact_preferred: Side::Left,
            rects: [None; 2],
            last_widgets: [None; 4],
            available_width: None,
        };
        self.reconcile_sidebar_selection(Side::Left);
        self.reconcile_sidebar_selection(Side::Right);
    }

    pub(super) fn sidebar_view_visible(&self, view: SidebarView) -> bool {
        self.sidebar_views(self.sidebar_side(view)).contains(&view)
            && self
                .visible_sidebar_sides()
                .contains(&self.sidebar_side(view))
            && self.sidebar_selected_view(self.sidebar_side(view)) == Some(view)
    }

    pub(super) fn sidebar_panel_open(&self, view: SidebarView) -> bool {
        self.sidebar_layout.open_views.contains(&view)
    }

    fn sidebar_views(&self, side: Side) -> Vec<SidebarView> {
        if self.project().is_some_and(Project::is_other_files) {
            return [SidebarView::Explorer, SidebarView::Review]
                .into_iter()
                .filter(|view| {
                    self.sidebar_side(*view) == side
                        && (*view == SidebarView::Explorer
                            || self.sidebar_layout.open_views.contains(view))
                })
                .collect();
        }
        self.sidebar_layout
            .open_views
            .iter()
            .copied()
            .filter(|view| self.sidebar_side(*view) == side)
            .collect()
    }

    fn sidebar_selected_view(&self, side: Side) -> Option<SidebarView> {
        let selected = match side {
            Side::Left => self.sidebar,
            Side::Right => self.sidebar_layout.right,
        };
        let views = self.sidebar_views(side);
        views
            .contains(&selected)
            .then_some(selected)
            .or_else(|| views.first().copied())
    }

    fn select_sidebar_view(&mut self, view: SidebarView) {
        match self.sidebar_side(view) {
            Side::Left => self.sidebar = view,
            Side::Right => self.sidebar_layout.right = view,
        }
    }

    fn reconcile_sidebar_selection(&mut self, side: Side) {
        let selected = match side {
            Side::Left => self.sidebar,
            Side::Right => self.sidebar_layout.right,
        };
        let views = self.sidebar_views(side);
        if !views.contains(&selected)
            && let Some(view) = views.first()
        {
            self.select_sidebar_view(*view);
        }
    }

    fn sidebar_view_has_focus(&self, view: SidebarView) -> bool {
        if view == SidebarView::Explorer && self.explorer_keyboard_focused() {
            return true;
        }
        if view == SidebarView::Search && self.project_search_focus {
            return true;
        }
        if view == SidebarView::SourceControl && self.git_content_has_focus() {
            return true;
        }
        self.ctx
            .memory(|memory| memory.focused())
            .is_some_and(|id| {
                self.ctx.read_response(id).is_some_and(|response| {
                    self.ctx
                        .read_response(sidebar_focus_id(self.sidebar_side(view)))
                        .is_some_and(|anchor| anchor.layer_id == response.layer_id)
                        && self.sidebar_layout.view_rects[view.index()]
                            .is_some_and(|rect| rect.contains(response.rect.center()))
                })
            })
    }

    pub(super) fn sidebar_side(&self, view: SidebarView) -> Side {
        if self.sidebar_layout.right_views.contains(&view) {
            Side::Right
        } else {
            Side::Left
        }
    }

    pub(super) fn set_sidebar_visible(&mut self, side: Side, visible: bool) {
        let focused = self.focused_sidebar();
        self.reconcile_sidebar_selection(side);
        if !visible && self.focused_sidebar() == Some(side) {
            // Explorer and text inputs must not retain focus in a hidden region.
            self.focus_editor();
        }
        if !visible && self.sidebar_side(SidebarView::Search) == side {
            self.project_search_focus = false;
        }
        match side {
            Side::Left => self.sidebar_visible = visible,
            Side::Right => self.sidebar_layout.right_visible = visible,
        }
        if visible {
            self.sidebar_layout.compact_preferred = side;
        }
        self.update_sidebar_collapsed();
        if focused.is_some_and(|side| !self.visible_sidebar_sides().contains(&side)) {
            self.focus_editor();
        }
        self.session_dirty = true;
    }

    fn update_sidebar_collapsed(&mut self) {
        let Some(width) = self.sidebar_layout.available_width else {
            return;
        };
        self.sidebar_layout.collapsed = [false; 2];
        if self.sidebar_visible
            && self.sidebar_layout.right_visible
            && width < MIN_EDITOR + 2.0 * MIN_SIDEBAR
        {
            self.sidebar_layout.collapsed[if self.sidebar_layout.compact_preferred == Side::Left {
                1
            } else {
                0
            }] = true;
        }
        if width < MIN_SIDEBAR + 120.0 {
            self.sidebar_layout.collapsed =
                [self.sidebar_visible, self.sidebar_layout.right_visible];
        }
    }

    pub(super) fn visible_sidebar_sides(&self) -> Vec<Side> {
        let mut sides = Vec::new();
        if self.projects.is_empty() {
            return sides;
        }
        if self.sidebar_visible
            && !self.sidebar_layout.collapsed[0]
            && !(self.project().is_some_and(Project::is_other_files)
                && self.sidebar_views(Side::Left).is_empty())
        {
            sides.push(Side::Left);
        }
        if self.sidebar_layout.right_visible
            && !self.sidebar_layout.collapsed[1]
            && !(self.project().is_some_and(Project::is_other_files)
                && self.sidebar_views(Side::Right).is_empty())
        {
            sides.push(Side::Right);
        }
        sides
    }

    pub(super) fn focused_sidebar(&self) -> Option<Side> {
        if self.sidebar_view_visible(SidebarView::Explorer) && self.explorer_keyboard_focused() {
            return Some(self.sidebar_side(SidebarView::Explorer));
        }
        if self.sidebar_view_visible(SidebarView::Search) && self.project_search_focus {
            return Some(self.sidebar_side(SidebarView::Search));
        }
        if self.sidebar_view_visible(SidebarView::SourceControl) && self.git_content_has_focus() {
            return Some(self.sidebar_side(SidebarView::SourceControl));
        }
        let focused = self.ctx.memory(|memory| memory.focused())?;
        self.visible_sidebar_sides().into_iter().find(|side| {
            focused == sidebar_focus_id(*side)
                || self.ctx.read_response(focused).is_some_and(|response| {
                    self.ctx
                        .read_response(sidebar_focus_id(*side))
                        .is_some_and(|anchor| anchor.layer_id == response.layer_id)
                        && self.sidebar_layout.rects[if *side == Side::Left { 0 } else { 1 }]
                            .is_some_and(|rect| rect.contains(response.rect.center()))
                })
        })
    }

    pub(super) fn sidebar_region_rect(&self, side: Side) -> Option<egui::Rect> {
        if self.visible_sidebar_sides().contains(&side) {
            self.sidebar_layout.rects[if side == Side::Left { 0 } else { 1 }]
        } else {
            None
        }
    }

    pub(super) fn focus_sidebar_region(&mut self, side: Side) {
        if !self.visible_sidebar_sides().contains(&side) {
            return;
        }
        self.remember_spatial_anchor();
        let view = match side {
            Side::Left => self.sidebar,
            Side::Right => self.sidebar_layout.right,
        };
        if !self.sidebar_views(side).contains(&view) {
            self.focus_editor();
            self.ctx
                .memory_mut(|memory| memory.request_focus(sidebar_focus_id(side)));
            return;
        }
        match view {
            SidebarView::Explorer => self.focus_explorer(),
            SidebarView::Search => self.open_project_search(),
            SidebarView::SourceControl => {
                self.focus_editor();
                self.ctx
                    .memory_mut(|memory| memory.request_focus(sidebar_focus_id(side)));
                self.focus_git_content();
            }
            _ => {
                self.focus_editor();
                if let Some(tab) = self.tab_mut() {
                    tab.focus = false;
                }
                let remembered = self.sidebar_layout.last_widgets[view.index()].filter(|id| {
                    self.ctx.read_response(*id).is_some_and(|response| {
                        response.enabled()
                            && self
                                .sidebar_region_rect(side)
                                .is_some_and(|rect| rect.contains(response.rect.center()))
                    })
                });
                if let Some(widget) = remembered {
                    self.ctx.memory_mut(|memory| memory.request_focus(widget));
                } else {
                    self.ctx
                        .memory_mut(|memory| memory.request_focus(sidebar_focus_id(side)));
                }
            }
        }
    }

    pub(super) fn toggle_sidebar(&mut self, side: Side) {
        let visible = match side {
            Side::Left => self.sidebar_visible,
            Side::Right => self.sidebar_layout.right_visible,
        };
        let collapsed = self.sidebar_layout.collapsed[if side == Side::Left { 0 } else { 1 }];
        self.set_sidebar_visible(side, !visible || collapsed);
    }

    pub(super) fn show_sidebar(&mut self, view: SidebarView) {
        let side = self.sidebar_side(view);
        if let Some(previous) = self.sidebar_selected_view(side)
            && previous != view
        {
            if !self.settings_open && self.sidebar_view_has_focus(previous) {
                self.focus_editor();
            }
            if previous == SidebarView::Search {
                self.project_search_focus = false;
            }
        }
        if !self.sidebar_layout.open_views.contains(&view) {
            self.sidebar_layout.open_views.push(view);
        }
        self.select_sidebar_view(view);
        self.set_sidebar_visible(side, true);
    }

    pub(super) fn set_sidebar_view_visible(&mut self, view: SidebarView, visible: bool) {
        if visible {
            self.show_sidebar(view);
        } else if self.sidebar_layout.open_views.contains(&view) {
            let side = self.sidebar_side(view);
            if self.sidebar_view_has_focus(view) {
                self.focus_editor();
            }
            if view == SidebarView::Search {
                self.project_search_focus = false;
            }
            self.sidebar_layout
                .open_views
                .retain(|candidate| *candidate != view);
            self.reconcile_sidebar_selection(side);
            if self.sidebar_views(side).is_empty() {
                self.set_sidebar_visible(side, false);
            }
            self.session_dirty = true;
            self.ctx.request_repaint();
        }
    }

    pub(super) fn toggle_sidebar_view(&mut self, view: SidebarView) {
        if self.sidebar_view_visible(view) {
            self.set_sidebar_visible(self.sidebar_side(view), false);
        } else if view == SidebarView::Search {
            self.open_project_search();
        } else {
            self.show_sidebar(view);
        }
    }

    pub(super) fn move_sidebar(&mut self, view: SidebarView, side: Side) {
        let previous = self.sidebar_side(view);
        if !self.settings_open && self.sidebar_view_has_focus(view) {
            self.focus_editor();
            if view == SidebarView::Search {
                self.project_search_focus = false;
            }
        }
        self.sidebar_layout
            .right_views
            .retain(|candidate| *candidate != view);
        if side == Side::Right {
            self.sidebar_layout.right_views.push(view);
        }
        if previous != side {
            self.reconcile_sidebar_selection(previous);
            if self.sidebar_views(previous).is_empty() {
                self.set_sidebar_visible(previous, false);
            }
        }
        self.show_sidebar(view);
        self.session_dirty = true;
        self.ctx.request_repaint();
    }

    pub(super) fn explorer_git_layout(&mut self) {
        if !self.settings_open {
            self.focus_editor();
        }
        self.move_sidebar(SidebarView::Explorer, Side::Left);
        self.move_sidebar(SidebarView::SourceControl, Side::Right);
        self.sidebar_layout.compact_preferred = Side::Left;
        self.update_sidebar_collapsed();
        self.session_dirty = true;
    }

    pub(super) fn clear_review_sidebar(&mut self) {
        self.set_sidebar_view_visible(SidebarView::Review, false);
    }

    pub(super) fn sidebar_menu_ui(&mut self, ui: &mut egui::Ui) {
        for (side, label, binding, visible) in [
            (
                Side::Left,
                "Toggle left sidebar",
                keybindings::Action::ToggleLeftSidebar,
                self.sidebar_visible,
            ),
            (
                Side::Right,
                "Toggle right sidebar",
                keybindings::Action::ToggleRightSidebar,
                self.sidebar_layout.right_visible,
            ),
        ] {
            if design::menu_item(
                ui,
                label,
                &self.shortcut_label(binding),
                if visible {
                    "Hide this sidebar and focus the editor"
                } else {
                    "Show the panels assigned to this side"
                },
            )
            .clicked()
            {
                self.toggle_sidebar(side);
                ui.close();
            }
        }
        if design::menu_item(
            ui,
            "Arrange sidebar panels…",
            "",
            "Choose each panel's side and visibility",
        )
        .clicked()
        {
            self.open_settings_section("Layout");
            ui.close();
        }
    }

    pub(super) fn sidebar_settings_ui(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Sidebar layout").strong());
        ui.horizontal(|ui| {
            let mut left = self.sidebar_visible;
            let mut right = self.sidebar_layout.right_visible;
            if ui.checkbox(&mut left, "Left sidebar").changed() {
                self.set_sidebar_visible(Side::Left, left);
            }
            if ui.checkbox(&mut right, "Right sidebar").changed() {
                self.set_sidebar_visible(Side::Right, right);
            }
        });
        for view in VIEWS {
            ui.push_id(("sidebar_setting", view.index()), |ui| {
                ui.horizontal(|ui| {
                    let mut open = self.sidebar_layout.open_views.contains(&view);
                    if ui.checkbox(&mut open, view.title()).changed() {
                        self.set_sidebar_view_visible(view, open);
                    }
                    let mut side = self.sidebar_side(view);
                    let previous = side;
                    ui.selectable_value(&mut side, Side::Left, "Left");
                    ui.selectable_value(&mut side, Side::Right, "Right");
                    if side != previous {
                        self.move_sidebar(view, side);
                    }
                });
            });
        }
        ui.label(RichText::new("Each sidebar shows one panel. Switch with the activity rail or panel menu. Use the panel menu to move it to the other side. Drag the inner edges to resize.").small().color(MUTED));
    }

    pub(super) fn sidebars_status(&self) -> serde_json::Value {
        let visible = self.visible_sidebar_sides();
        serde_json::json!({
            "left": {"panel": self.sidebar_selected_view(Side::Left), "panels": self.sidebar_views(Side::Left), "visible": self.sidebar_visible, "effective_visible": visible.contains(&Side::Left), "width": self.sidebar_layout.left_width},
            "right": {"panel": self.sidebar_selected_view(Side::Right), "panels": self.sidebar_views(Side::Right), "visible": self.sidebar_layout.right_visible, "effective_visible": visible.contains(&Side::Right), "width": self.sidebar_layout.right_width},
            "right_panels": self.sidebar_layout.right_views,
            "collapsed_panels": [],
        })
    }

    pub(super) fn sidebar_ui(&mut self, ui: &mut egui::Ui) {
        self.sidebar_layout.view_rects = [None; 4];
        if self.projects.is_empty() {
            self.sidebar_layout.rects = [None; 2];
            return;
        }
        let focused = self.focused_sidebar();
        self.sidebar_layout.available_width = Some(ui.available_width());
        self.update_sidebar_collapsed();
        if focused.is_some_and(|side| {
            self.sidebar_layout.collapsed[if side == Side::Left { 0 } else { 1 }]
        }) {
            self.focus_editor();
        }
        if self
            .sidebar_layout
            .collapsed
            .iter()
            .any(|collapsed| *collapsed)
        {
            let can_show_sidebar = ui.available_width() >= MIN_SIDEBAR + 120.0;
            egui::Panel::top("sidebar_space_hint").show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new("Limited space — sidebar hidden.")
                            .small()
                            .color(MUTED),
                    );
                    if !can_show_sidebar {
                        ui.label(
                            RichText::new(
                                "Widen the window or reduce interface scale in Settings.",
                            )
                            .small()
                            .color(MUTED),
                        );
                    }
                    for (side, index, label) in
                        [(Side::Left, 0, "Show left"), (Side::Right, 1, "Show right")]
                    {
                        if can_show_sidebar
                            && self.sidebar_layout.collapsed[index]
                            && ui.small_button(label).clicked()
                        {
                            self.sidebar_layout.compact_preferred = side;
                            self.ctx.request_repaint();
                        }
                    }
                });
            });
        }
        if self.sidebar_visible && !self.sidebar_layout.collapsed[0] {
            let (minimum, maximum) = sidebar_limits(
                ui.available_width(),
                self.sidebar_layout.right_visible && !self.sidebar_layout.collapsed[1],
                self.sidebar_layout.right_width,
            );
            self.render_sidebar(ui, Side::Left, minimum, maximum);
        }
        if self.sidebar_layout.right_visible && !self.sidebar_layout.collapsed[1] {
            let (minimum, maximum) = sidebar_limits(ui.available_width(), false, 0.0);
            self.render_sidebar(ui, Side::Right, minimum, maximum);
        }
    }

    fn render_sidebar(&mut self, ui: &mut egui::Ui, side: Side, minimum: f32, maximum: f32) {
        let (width, panel) = match side {
            Side::Left => (self.sidebar_layout.left_width, egui::Panel::left("sidebar")),
            Side::Right => (
                self.sidebar_layout.right_width,
                egui::Panel::right("right_sidebar"),
            ),
        };
        let id = egui::Id::new(if side == Side::Left {
            "sidebar"
        } else {
            "right_sidebar"
        });
        let resize_id = id.with("__resize");
        if !self.ctx.is_being_dragged(resize_id) {
            self.ctx.data_mut(|data| {
                data.insert_persisted(
                    id,
                    egui::containers::panel::PanelState {
                        outer_rect: egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            Vec2::new(width, 1.0),
                        ),
                    },
                )
            });
        }
        let result = panel
            .default_size(width)
            .size_range(minimum..=maximum)
            .resizable(true)
            .frame(egui::Frame::new().fill(PANEL).inner_margin(Margin {
                left: 10,
                right: 10,
                top: 4,
                bottom: 8,
            }))
            .show(ui, |ui| {
                let region = ui.interact(
                    ui.max_rect(),
                    sidebar_focus_id(side),
                    egui::Sense::focusable_noninteractive(),
                );
                region.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::WidgetType::Other,
                        ui.is_enabled(),
                        format!(
                            "{} sidebar",
                            if side == Side::Left { "Left" } else { "Right" },
                        ),
                    )
                });
                self.register_region_focus_widget(region.id);
                if self.awaiting_sidebar_owner(side) {
                    self.focus_sidebar_region(side);
                }
                self.sidebar_header(ui, side);
                ui.separator();
                self.render_sidebar_panel(ui, side);
            });
        self.sidebar_layout.rects[if side == Side::Left { 0 } else { 1 }] =
            Some(result.response.rect);
        if self.focused_sidebar() == Some(side) {
            if let Some(widget) = self.ctx.memory(|memory| memory.focused())
                && widget != sidebar_focus_id(side)
                && let Some(view) = self.sidebar_selected_view(side)
            {
                self.sidebar_layout.last_widgets[view.index()] = Some(widget);
            }
            // Focused rows and fields show their own focus, so a click never
            // lights up the whole panel. The frame only marks keyboard focus
            // on the region itself, before any item inside has taken it.
            if self.sidebar_selected_view(side) != Some(SidebarView::Explorer)
                && crate::design::focus_visible(&self.ctx)
                && self
                    .ctx
                    .memory(|memory| memory.has_focus(sidebar_focus_id(side)))
            {
                ui.painter().rect_stroke(
                    result.response.rect.shrink(1.0),
                    0.0,
                    Stroke::new(1.5, ACCENT),
                    egui::StrokeKind::Inside,
                );
            }
        }
        let actual_width = result.response.rect.width();
        let stored = match side {
            Side::Left => &mut self.sidebar_layout.left_width,
            Side::Right => &mut self.sidebar_layout.right_width,
        };
        if (self.ctx.is_being_dragged(resize_id) || self.ctx.drag_stopped_id() == Some(resize_id))
            && (actual_width - *stored).abs() > 0.5
            && actual_width >= MIN_SIDEBAR
        {
            *stored = actual_width;
            self.session_dirty = true;
        }
    }

    fn sidebar_header(&mut self, ui: &mut egui::Ui, side: Side) {
        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), 24.0),
            Layout::right_to_left(Align::Center),
            |ui| {
                let selected = self.sidebar_selected_view(side);
                // A fixed hit target and painted hover cannot change layout.
                // Button frames otherwise add their hover stroke to the size.
                let (rect, menu) = ui.allocate_exact_size(Vec2::splat(22.0), egui::Sense::click());
                let popup = egui::Popup::menu(&menu);
                let focused = design::keyboard_focus(&menu);
                if menu.hovered() || focused || egui::Popup::is_id_open(ui.ctx(), popup.get_id()) {
                    ui.painter().rect_filled(rect, 4.0, HOVER);
                }
                if focused {
                    ui.painter().rect_stroke(
                        rect.shrink(1.0),
                        4.0,
                        Stroke::new(1.0, ACCENT),
                        egui::StrokeKind::Inside,
                    );
                }
                for offset in [-4.0, 0.0, 4.0] {
                    ui.painter()
                        .circle_filled(rect.center() + Vec2::new(offset, 0.0), 1.0, MUTED);
                }
                popup.show(|ui| {
                    if let Some(view) = selected {
                        self.sidebar_panel_menu(ui, view);
                        ui.separator();
                    }
                    for view in VIEWS {
                        if self.project().is_some_and(Project::is_other_files)
                            && matches!(view, SidebarView::Search | SidebarView::SourceControl)
                        {
                            continue;
                        }
                        if self.sidebar_side(view) == side
                            && selected != Some(view)
                            && ui.button(format!("Show {}", view.title())).clicked()
                        {
                            self.show_sidebar(view);
                            self.focus_sidebar_region(side);
                            ui.close();
                        }
                    }
                    ui.separator();
                    let hide = ui.button(if side == Side::Left {
                        "Hide left sidebar"
                    } else {
                        "Hide right sidebar"
                    });
                    if hide.clicked() {
                        self.set_sidebar_visible(side, false);
                        ui.close();
                    }
                });
                let menu = menu
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text("Panel actions");
                menu.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::WidgetType::Button,
                        ui.is_enabled(),
                        selected.map_or_else(
                            || "Sidebar panels".to_owned(),
                            |view| format!("{} panel actions", view.title()),
                        ),
                    )
                });
                // Measure actions first so a long title cannot expand the panel
                // response past its paint clip and leave an unpainted gutter.
                let title = selected.map_or("Sidebar", SidebarView::title);
                ui.allocate_ui_with_layout(
                    Vec2::new(ui.available_width().max(1.0), 24.0),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        if selected == Some(SidebarView::Explorer)
                            && let Some(root) = self.project().map(|project| project.root.clone())
                        {
                            self.explorer_project_heading(ui, &root);
                            return;
                        }
                        let heading = ui.add(
                            egui::Label::new(RichText::new(title).size(12.0).color(MUTED))
                                .selectable(false)
                                .truncate(),
                        );
                        heading.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::WidgetType::Label,
                                ui.is_enabled(),
                                format!("{title} panel title"),
                            )
                        });
                    },
                );
            },
        );
    }

    fn render_sidebar_panel(&mut self, ui: &mut egui::Ui, side: Side) {
        let Some(view) = self.sidebar_selected_view(side) else {
            ui.label(RichText::new("No panel selected").color(MUTED));
            ui.label(
                RichText::new("Choose a panel from the sidebar menu.")
                    .small()
                    .color(MUTED),
            );
            return;
        };
        if !self.sidebar_view_visible(view) {
            return;
        }
        let rect = ui.available_rect_before_wrap();
        self.sidebar_layout.view_rects[view.index()] = Some(rect);
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(rect)
                .id(egui::Id::new(("sidebar_view", view.index()))),
            |ui| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                match view {
                    SidebarView::Explorer => self.explorer(ui),
                    SidebarView::Search => {
                        egui::ScrollArea::vertical()
                            .id_salt("sidebar_search_content")
                            .auto_shrink([false, false])
                            .show(ui, |ui| self.project_search_ui(ui));
                    }
                    SidebarView::SourceControl => self.git_ui(ui),
                    SidebarView::Review => self.review_ui(ui),
                }
            },
        );
        ui.allocate_rect(rect, egui::Sense::hover());
    }

    fn sidebar_panel_menu(&mut self, ui: &mut egui::Ui, view: SidebarView) {
        if view == SidebarView::SourceControl && self.git_repository_menu(ui) {
            ui.separator();
        }
        for (side, label) in [
            (Side::Left, "Move panel to left"),
            (Side::Right, "Move panel to right"),
        ] {
            if ui
                .add_enabled(self.sidebar_side(view) != side, egui::Button::new(label))
                .clicked()
            {
                self.move_sidebar(view, side);
                ui.close();
            }
        }
        if ui.button("Hide panel").clicked() {
            self.set_sidebar_view_visible(view, false);
            ui.close();
        }
        ui.separator();
        if self.active_folder().is_some() && ui.button("Refresh project").clicked() {
            self.index_project(self.active_project);
            if view == SidebarView::SourceControl {
                self.refresh_git_history(self.active_project);
                self.repositories.request_refresh();
            }
            self.refresh_git(self.active_project, true);
            ui.close();
        }
        if view == SidebarView::Explorer && ui.button("New file").clicked() {
            self.new_file();
            ui.close();
        }
        if view == SidebarView::Explorer
            && ui
                .add_enabled(
                    self.explorer_has_expanded_folders(),
                    egui::Button::new("Collapse all"),
                )
                .clicked()
        {
            self.explorer_collapse_all();
            ui.close();
        }
    }

    pub(super) fn sidebar_drag_keys(&mut self) -> bool {
        false
    }
}

#[cfg(test)]
#[path = "sidebar_layout_tests.rs"]
mod tests;

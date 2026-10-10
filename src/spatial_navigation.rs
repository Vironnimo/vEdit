//! Geometry-based keyboard navigation shares the same regions as the visible UI.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Region {
    Pane(u64),
    Sidebar(sidebar_layout::Side),
    Output,
}

pub(in crate::app) fn output_command_id() -> egui::Id {
    egui::Id::new("output_command_input")
}

fn output_focus_id() -> egui::Id {
    egui::Id::new("output_region_focus")
}

/// Rank only neighbors on the requested side. The cross-axis anchor makes a
/// trip through a full-height sidebar return to the same editor row.
fn neighbor(
    origin: egui::Rect,
    anchor: egui::Pos2,
    direction: Vec2,
    regions: impl IntoIterator<Item = (Region, egui::Rect)>,
) -> Option<Region> {
    let horizontal = direction.x != 0.0;
    regions
        .into_iter()
        .filter_map(|(region, rect)| {
            let gap = if direction.x > 0.0 {
                rect.left() - origin.right()
            } else if direction.x < 0.0 {
                origin.left() - rect.right()
            } else if direction.y > 0.0 {
                rect.top() - origin.bottom()
            } else {
                origin.top() - rect.bottom()
            };
            if gap < -1.0 {
                return None;
            }
            let (cross, low, high, center) = if horizontal {
                (anchor.y, rect.top(), rect.bottom(), rect.center().y)
            } else {
                (anchor.x, rect.left(), rect.right(), rect.center().x)
            };
            let cross_gap = (low - cross).max(cross - high).max(0.0);
            Some((region, cross_gap, gap.max(0.0), (center - cross).abs(), low))
        })
        .min_by(|a, b| {
            a.1.total_cmp(&b.1)
                .then_with(|| a.2.total_cmp(&b.2))
                .then_with(|| a.3.total_cmp(&b.3))
                .then_with(|| a.4.total_cmp(&b.4))
        })
        .map(|candidate| candidate.0)
}

impl VEditApp {
    pub(in crate::app) fn begin_region_focus_frame(&mut self) {
        self.region_focus.rendered_focus_widgets.clear();
    }

    pub(in crate::app) fn register_region_focus_widget(&mut self, id: egui::Id) {
        self.region_focus.rendered_focus_widgets.insert(id);
    }

    pub(in crate::app) fn defer_region_focus_owner(&mut self) {
        if self.region_focus.awaiting_region.is_none()
            && self
                .ctx
                .memory(|memory| memory.focused())
                .is_some_and(|id| self.region_focus.rendered_focus_widgets.contains(&id))
        {
            return;
        }
        if self.region_focus.awaiting_region.is_none() {
            self.region_focus.awaiting_region = Some(self.navigation_region());
        }
        self.ctx.memory_mut(|memory| {
            if let Some(widget) = memory.focused() {
                memory.surrender_focus(widget);
            }
        });
        self.ctx.request_repaint();
    }

    pub(in crate::app) fn awaiting_sidebar_owner(&self, side: sidebar_layout::Side) -> bool {
        self.region_focus.awaiting_region == Some(Region::Sidebar(side))
    }

    pub(in crate::app) fn finish_region_render_owner(&mut self) {
        let Some(destination) = self.region_focus.awaiting_region else {
            return;
        };
        let ready = match destination {
            Region::Pane(id) => self.editor_layout.focused == id,
            Region::Sidebar(side) => self.focused_sidebar() == Some(side),
            Region::Output => self.output_keyboard_focused(),
        };
        if !ready {
            // The layout or project can disappear through an independent UI
            // action. Never redirect the waiting text into some other region.
            self.region_focus.trailing.clear();
        }
        self.region_focus.awaiting_region = None;
        self.region_focus.owner = Some(self.region_input_owner());
        self.ctx.request_repaint();
    }

    pub(in crate::app) fn remember_spatial_anchor(&mut self) {
        if self.focused_sidebar().is_none()
            && !self.output_keyboard_focused()
            && let Some(rect) = self.editor_layout.rects.get(&self.editor_layout.focused)
        {
            self.region_focus.spatial_anchor = Some(rect.center());
        }
    }

    fn navigation_region(&self) -> Region {
        if let Some(side) = self.focused_sidebar() {
            Region::Sidebar(side)
        } else if self.output_keyboard_focused() {
            Region::Output
        } else {
            Region::Pane(self.editor_layout.focused)
        }
    }

    fn navigation_regions(&self) -> Vec<(Region, egui::Rect)> {
        let mut ids = Vec::new();
        self.editor_layout.root.ids(&mut ids);
        let mut regions: Vec<_> = ids
            .into_iter()
            .filter_map(|id| {
                self.editor_layout
                    .rects
                    .get(&id)
                    .map(|rect| (Region::Pane(id), *rect))
            })
            .collect();
        for side in self.visible_sidebar_sides() {
            if let Some(rect) = self.sidebar_region_rect(side) {
                regions.push((Region::Sidebar(side), rect));
            }
        }
        if self.output_visible
            && let Some(rect) = self.region_focus.output_rect
        {
            regions.push((Region::Output, rect));
        }
        regions
    }

    fn focus_navigation_region(&mut self, region: Region) -> Result<(), String> {
        match region {
            Region::Pane(id) => {
                self.focus_editor();
                self.focus_editor_pane(id)
            }
            Region::Sidebar(side) => {
                self.focus_sidebar_region(side);
                Ok(())
            }
            Region::Output => {
                self.focus_output_region();
                Ok(())
            }
        }
    }

    pub(super) fn focus_spatial_region(&mut self, direction: Vec2) -> Result<(), String> {
        let current = self.navigation_region();
        let regions = self.navigation_regions();
        let Some(origin) = regions
            .iter()
            .find_map(|(region, rect)| (*region == current).then_some(*rect))
        else {
            return Ok(());
        };
        let anchor = if matches!(current, Region::Pane(_)) {
            origin.center()
        } else {
            self.region_focus.spatial_anchor.unwrap_or(origin.center())
        };
        self.region_focus.spatial_anchor = Some(anchor);
        if let Some(destination) = neighbor(
            origin,
            anchor,
            direction,
            regions.into_iter().filter(|(region, _)| *region != current),
        ) {
            self.focus_navigation_region(destination)?;
        }
        Ok(())
    }

    pub(super) fn move_tab_direction(&mut self, direction: Vec2) -> Result<(), String> {
        let source = self.editor_layout.focused;
        let tab = self
            .editor_layout
            .panes
            .iter()
            .find(|pane| pane.id == source)
            .and_then(|pane| pane.document)
            .ok_or("This pane has no document to move")?;
        let origin = *self
            .editor_layout
            .rects
            .get(&source)
            .ok_or("Editor layout is not visible yet")?;
        let destination = neighbor(
            origin,
            origin.center(),
            direction,
            self.editor_layout
                .rects
                .iter()
                .filter(|(id, _)| **id != source)
                .map(|(id, rect)| (Region::Pane(*id), *rect)),
        );
        let Some(Region::Pane(destination)) = destination else {
            return Err("No editor pane in that direction. Split the editor first.".into());
        };
        let index = self
            .editor_layout
            .panes
            .iter()
            .find(|pane| pane.id == destination)
            .map(|pane| pane.documents.len() - usize::from(pane.documents.contains(&tab)))
            .unwrap_or(0);
        // A directional keyboard move takes the active location with it, even
        // when the target already contains another view of this same buffer.
        self.move_pane_tab_inner(source, destination, tab, index, true, true)
    }

    pub(super) fn cycle_spatial_region(&mut self, backwards: bool) {
        let mut ids = Vec::new();
        self.editor_layout.root.ids(&mut ids);
        let mut regions: Vec<_> = ids.into_iter().map(Region::Pane).collect();
        for side in self.visible_sidebar_sides() {
            match side {
                sidebar_layout::Side::Left => regions.insert(0, Region::Sidebar(side)),
                sidebar_layout::Side::Right => regions.push(Region::Sidebar(side)),
            }
        }
        if self.output_visible {
            regions.push(Region::Output);
        }
        if regions.is_empty() {
            return;
        }
        let current = self.navigation_region();
        if let Region::Pane(id) = current
            && let Some(rect) = self.editor_layout.rects.get(&id)
        {
            self.region_focus.spatial_anchor = Some(rect.center());
        }
        let slot = regions
            .iter()
            .position(|region| *region == current)
            .unwrap_or(0);
        let next = if backwards {
            (slot + regions.len() - 1) % regions.len()
        } else {
            (slot + 1) % regions.len()
        };
        let _ = self.focus_navigation_region(regions[next]);
    }

    fn output_owns_widget(&self, widget: egui::Id) -> bool {
        widget == output_focus_id()
            || widget == output_command_id()
            || self.ctx.read_response(widget).is_some_and(|response| {
                self.ctx
                    .read_response(output_focus_id())
                    .is_some_and(|anchor| anchor.layer_id == response.layer_id)
                    && self
                        .region_focus
                        .output_rect
                        .is_some_and(|rect| rect.contains(response.rect.center()))
            })
    }

    pub(in crate::app) fn output_keyboard_focused(&self) -> bool {
        self.output_visible
            && self
                .ctx
                .memory(|memory| memory.focused())
                .is_some_and(|id| self.output_owns_widget(id))
    }

    pub(in crate::app) fn focus_output_region(&mut self) {
        if !self.output_visible {
            return;
        }
        self.remember_spatial_anchor();
        self.focus_editor();
        if let Some(tab) = self.tab_mut() {
            tab.focus = false;
        }
        let remembered = self.region_focus.output_widget.filter(|id| {
            self.region_focus.output_widget_problems == Some(self.problems_visible)
                && self
                    .ctx
                    .read_response(*id)
                    .is_some_and(|response| response.enabled())
                && self.output_owns_widget(*id)
        });
        let destination = remembered.unwrap_or_else(|| {
            if self.problems_visible {
                output_focus_id()
            } else {
                output_command_id()
            }
        });
        self.region_focus.output_focus_pending = remembered.is_none() && self.problems_visible;
        self.ctx
            .memory_mut(|memory| memory.request_focus(destination));
        self.ctx.request_repaint();
    }

    pub(in crate::app) fn clear_hidden_output_focus(&mut self) {
        if self
            .ctx
            .memory(|memory| memory.focused())
            .is_some_and(|id| self.output_owns_widget(id))
        {
            self.focus_editor();
        }
        self.region_focus.output_rect = None;
        self.region_focus.output_focus_pending = false;
    }

    pub(in crate::app) fn begin_output_region(&mut self, ui: &mut egui::Ui) {
        let region = ui.interact(
            ui.max_rect(),
            output_focus_id(),
            egui::Sense::focusable_noninteractive(),
        );
        region.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Other,
                ui.is_enabled(),
                if self.problems_visible {
                    "Problems panel"
                } else {
                    "Output panel"
                },
            )
        });
        self.register_region_focus_widget(region.id);
        if self.region_focus.awaiting_region == Some(Region::Output) {
            self.focus_output_region();
        }
    }

    pub(in crate::app) fn output_problem_focus_target(&mut self, response: &egui::Response) {
        if self.region_focus.output_focus_pending {
            response.request_focus();
            self.region_focus.output_focus_pending = false;
        }
        if response.has_focus() {
            response.scroll_to_me(Some(Align::Center));
        }
    }

    pub(in crate::app) fn finish_output_region(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        if !self.output_visible {
            self.region_focus.output_rect = None;
            return;
        }
        self.region_focus.output_rect = Some(rect);
        self.region_focus.output_focus_pending = false;
        if self.output_keyboard_focused() {
            self.region_focus.output_widget = self.ctx.memory(|memory| memory.focused());
            self.region_focus.output_widget_problems = Some(self.problems_visible);
            ui.painter().rect_stroke(
                rect.shrink(1.0),
                0.0,
                Stroke::new(1.5, ACCENT),
                egui::StrokeKind::Inside,
            );
        }
    }
}

#[cfg(test)]
#[path = "spatial_navigation_tests.rs"]
mod tests;

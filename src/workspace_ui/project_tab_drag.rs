//! Project tab gestures preserve the active workspace and commit only on drop.
use super::*;

impl VEditApp {
    pub(in crate::app) fn project_tab_pointer_interaction_active(&self) -> bool {
        self.workspace_ui.dragged_root.is_some()
            || self.ctx.input(|input| {
                input.pointer.primary_down()
                    && input.pointer.press_origin().is_some_and(|origin| {
                        self.workspace_ui
                            .header_rect
                            .is_some_and(|rect| rect.contains(origin))
                            && self
                                .workspace_ui
                                .header_tabs
                                .iter()
                                .any(|(_, rect)| rect.contains(origin))
                    })
            })
    }

    pub(in crate::app) fn project_tab_drag_keys(&mut self) -> bool {
        let Some(root) = &self.workspace_ui.dragged_root else {
            return false;
        };
        let cancelled = self.chrome_blocked()
            || !self.projects.iter().any(|project| &project.root == root)
            || self.workspace_ui.header_rect.is_none()
            || self.ctx.input(|input| {
                input.key_pressed(Key::Escape)
                    || !input.focused
                    || !input.pointer.has_pointer()
                    || (!input.pointer.primary_down() && !input.pointer.primary_released())
            });
        // As with document tabs, the entire drag input batch belongs to the
        // gesture, including text arriving after Escape.
        self.ctx.input_mut(|input| {
            input.events.retain(|event| {
                !matches!(
                    event,
                    egui::Event::Key { .. }
                        | egui::Event::Text(_)
                        | egui::Event::Paste(_)
                        | egui::Event::Copy
                        | egui::Event::Cut
                        | egui::Event::Ime(_)
                )
            });
        });
        if cancelled {
            self.workspace_ui.dragged_root = None;
            self.ctx.request_repaint();
        }
        true
    }

    pub(super) fn finish_project_tab_drag(
        &mut self,
        ui: &egui::Ui,
        output: egui::scroll_area::ScrollAreaOutput<()>,
        tabs: &[ProjectHeaderTab],
    ) {
        let Some(root) = self.workspace_ui.dragged_root.clone() else {
            return;
        };
        let rect = self.workspace_ui.header_rect.unwrap();
        let position = self.ctx.pointer_hover_pos();
        let remaining: Vec<_> = self
            .workspace_ui
            .header_tabs
            .iter()
            .filter(|(tab, _)| *tab != root)
            .collect();
        let target = position.filter(|pos| rect.contains(*pos)).map(|position| {
            remaining
                .iter()
                .position(|(_, rect)| position.x < rect.center().x)
                .unwrap_or(remaining.len())
        });
        if self.ctx.input(|input| input.pointer.primary_released()) {
            if let Some(index) = target {
                self.workspace_ui.pending = Some(WorkspaceAction::Reorder {
                    root,
                    before: remaining.get(index).map(|(root, _)| root.clone()),
                });
            }
            // Keep the gesture active through source rendering. The deferred
            // workspace action clears it after preserving the current selection.
            self.ctx.request_repaint();
            return;
        }
        self.ctx.set_cursor_icon(if target.is_some() {
            egui::CursorIcon::Grabbing
        } else {
            egui::CursorIcon::NotAllowed
        });
        if let Some(index) = target {
            let x = remaining
                .get(index)
                .map(|(_, rect)| rect.left())
                .or_else(|| remaining.last().map(|(_, rect)| rect.right()))
                .unwrap_or(rect.left())
                .clamp(rect.left() + 1.0, rect.right() - 1.0);
            ui.painter().line_segment(
                [
                    egui::pos2(x, rect.top() + 3.0),
                    egui::pos2(x, rect.bottom() - 3.0),
                ],
                Stroke::new(2.5, ACCENT),
            );
            let position = position.unwrap();
            let edge = 28.0_f32.min(rect.width() * 0.25);
            let direction = if position.x < rect.left() + edge {
                -1.0
            } else if position.x > rect.right() - edge {
                1.0
            } else {
                0.0
            };
            let maximum = (output.content_size.x - rect.width()).max(0.0);
            if direction != 0.0 && maximum > 0.0 {
                let mut state = output.state;
                let elapsed = self.ctx.input(|input| input.stable_dt.min(0.05));
                state.offset.x = (state.offset.x + direction * elapsed * 500.0).clamp(0.0, maximum);
                state.store(&self.ctx, output.id);
                self.ctx.request_repaint();
            }
        }
        if let Some(position) = position
            && let Some(tab) = tabs.iter().find(|tab| tab.root == root)
        {
            let painter = self.ctx.layer_painter(egui::LayerId::new(
                egui::Order::Tooltip,
                egui::Id::new("dragged_project_tab"),
            ));
            let label = painter.layout_no_wrap(tab.label.clone(), FontId::proportional(12.5), TEXT);
            let rect = egui::Rect::from_min_size(
                position + Vec2::new(13.0, 17.0),
                label.size() + Vec2::new(20.0, 14.0),
            );
            painter.rect_filled(rect, 5.0, SURFACE);
            painter.rect_stroke(
                rect,
                5.0,
                Stroke::new(1.0, BORDER),
                egui::StrokeKind::Inside,
            );
            painter.galley(rect.min + Vec2::new(10.0, 7.0), label, TEXT);
        }
    }
}

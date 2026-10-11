//! Tab movement is committed on a valid drop after all pane targets are rendered.
use super::*;

#[derive(Clone)]
struct DraggedTab {
    pane: u64,
    tab: u64,
    title: String,
}
struct TabStrip {
    rect: egui::Rect,
    tabs: Vec<(u64, egui::Rect)>,
}
#[derive(Default)]
pub(super) struct TabDragState {
    active: Option<DraggedTab>,
    strips: HashMap<u64, TabStrip>,
    cancelled: bool,
    pane_ui_ids: HashMap<u64, egui::Id>,
    preview_moves: HashMap<(u64, u64), egui::Id>,
}

impl VEditApp {
    #[cfg(test)]
    pub(super) fn tab_strip_rect(&self, pane_id: u64) -> Option<egui::Rect> {
        self.tab_drag.strips.get(&pane_id).map(|strip| strip.rect)
    }
    pub(super) fn tab_drag_active(&self) -> bool {
        self.tab_drag.active.is_some()
    }
    pub(super) fn tab_pointer_interaction_active(&self) -> bool {
        self.tab_drag_active()
            || self.ctx.input(|input| {
                input.pointer.primary_down()
                    && input.pointer.press_origin().is_some_and(|origin| {
                        self.tab_drag.strips.values().any(|strip| {
                            strip.rect.contains(origin)
                                && strip.tabs.iter().any(|(_, rect)| {
                                    let close = egui::Rect::from_center_size(
                                        egui::pos2(rect.right() - 16.0, rect.center().y),
                                        Vec2::splat(20.0),
                                    );
                                    rect.contains(origin) && !close.contains(origin)
                                })
                        })
                    })
            })
    }
    pub(super) fn register_tab_drag_pane_ui(&mut self, pane: u64, ui_id: egui::Id) {
        self.tab_drag.pane_ui_ids.insert(pane, ui_id);
        let transfers: Vec<_> = self
            .tab_drag
            .preview_moves
            .iter()
            .filter(|((target, _), _)| *target == pane)
            .map(|((_, tab), source)| (*tab, *source))
            .collect();
        for (tab, source) in transfers {
            self.tab_drag.preview_moves.remove(&(pane, tab));
            self.markdown.transfer_view(tab, source, ui_id);
        }
    }
    pub(super) fn transfer_tab_preview(
        &mut self,
        source: u64,
        destination: u64,
        tab: u64,
        existing: bool,
    ) {
        // A CLI batch can move through panes that have never rendered. Follow
        // the queued reading position instead of requiring an intermediate UI.
        let source_ui = self
            .tab_drag
            .preview_moves
            .remove(&(source, tab))
            .or_else(|| self.tab_drag.pane_ui_ids.get(&source).copied());
        if existing {
            return;
        }
        if let Some(source_ui) = source_ui {
            if let Some(destination_ui) = self.tab_drag.pane_ui_ids.get(&destination).copied() {
                self.markdown.transfer_view(tab, source_ui, destination_ui);
            } else {
                self.tab_drag
                    .preview_moves
                    .insert((destination, tab), source_ui);
            }
        }
    }
    #[cfg(test)]
    pub(super) fn tab_preview_scroll(&self, pane: u64, tab: u64) -> Option<f32> {
        self.tab_drag
            .pane_ui_ids
            .get(&pane)
            .and_then(|ui_id| self.markdown.view_scroll(tab, *ui_id))
    }
    #[cfg(test)]
    pub(super) fn tab_preview_table_scroll(
        &self,
        pane: u64,
        tab: u64,
        source: usize,
    ) -> Option<f32> {
        self.tab_drag
            .pane_ui_ids
            .get(&pane)
            .and_then(|ui_id| self.markdown.table_scroll(tab, *ui_id, source))
    }
    pub(super) fn tab_drag_keys(&mut self) -> bool {
        if !self.tab_drag_active() {
            return false;
        }
        let escaped = self.ctx.input(|input| input.key_pressed(Key::Escape));
        // Keep pointer movement/releases for the drop; typing never reaches the buffer.
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
            })
        });
        if escaped {
            self.tab_drag.active = None;
            self.tab_drag.cancelled = true;
            self.ctx.request_repaint();
        }
        // Remain true for this entire input batch even after Escape cancels the drag.
        true
    }
    pub(super) fn begin_tab_drag_frame(&mut self) {
        let panes = self.tab_drag_pane_ids();
        self.tab_drag
            .pane_ui_ids
            .retain(|pane, _| panes.contains(pane));
        self.tab_drag
            .preview_moves
            .retain(|(pane, _), _| panes.contains(pane));
        self.tab_drag.strips.clear();
        self.tab_drag.cancelled = false;
        if self.tab_drag.active.is_none() {
            return;
        }
        let unavailable = self
            .tab_drag
            .active
            .as_ref()
            .is_none_or(|drag| !self.pane_contains_tab(drag.pane, drag.tab));
        let modal = self.picker.is_some()
            || self.save_as.is_some()
            || self.settings_open
            || self.agent_marks_modal_open()
            || self.close_action.is_some()
            || self.palette.is_some()
            || self.diff.is_some()
            || self.git_detail_open()
            || self.context_file_dialog.is_some()
            || self.navigation_modal_open()
            || self.navigation_pending()
            || self.screen_jump.is_some()
            || self.vim_command.is_some()
            || self.vim_search_active()
            || self.workspace_picker_open()
            || egui::Popup::is_any_open(&self.ctx);
        let escaped = self
            .ctx
            .input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Escape));
        let lost_pointer = self.ctx.input(|input| {
            !input.focused
                || !input.pointer.has_pointer()
                || (!input.pointer.primary_down() && !input.pointer.primary_released())
        });
        if unavailable || modal || escaped || lost_pointer {
            self.tab_drag.active = None;
            self.tab_drag.cancelled = true;
        }
    }
    pub(super) fn start_tab_drag(&mut self, response: &egui::Response, id: u64, title: &str) {
        if !self.tab_drag.cancelled && response.drag_started_by(egui::PointerButton::Primary) {
            self.keep_tab_open(id);
            self.tab_drag.active = Some(DraggedTab {
                pane: self.current_pane_id(),
                tab: id,
                title: title.into(),
            });
            self.ctx.request_repaint();
        }
    }
    pub(super) fn register_tab_drop_strip(
        &mut self,
        output: &egui::containers::scroll_area::ScrollAreaOutput<()>,
        tabs: Vec<(u64, egui::Rect)>,
    ) {
        let pane = self.current_pane_id();
        let rect = output.inner_rect;
        self.tab_drag.strips.insert(pane, TabStrip { rect, tabs });
        if self.tab_drag.active.is_some()
            && let Some(position) = self.ctx.pointer_hover_pos().filter(|p| rect.contains(*p))
        {
            // Continue revealing tabs while a held drag rests near either strip edge.
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
    }
    pub(super) fn markdown_preview_toggle(&mut self, ui: &mut egui::Ui, preview: bool) {
        let (rect, response) = ui.allocate_exact_size(Vec2::new(30.0, 27.0), egui::Sense::click());
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::WidgetType::Checkbox,
                true,
                preview,
                "Toggle Markdown preview",
            )
        });
        let color = if preview {
            ACCENT
        } else if response.hovered() {
            TEXT
        } else {
            MUTED
        };
        if response.hovered() || response.has_focus() || preview {
            ui.painter().rect_filled(
                rect,
                4.0,
                if response.hovered() {
                    HOVER
                } else {
                    ACCENT.linear_multiply(0.10)
                },
            );
        }
        let center = rect.center();
        let points: Vec<_> = (0..=24)
            .map(|i| {
                let t = i as f32 / 24.0 * std::f32::consts::TAU;
                center + Vec2::new(t.cos() * 9.0, t.sin() * 5.4 * t.sin().abs())
            })
            .collect();
        ui.painter()
            .add(egui::Shape::line(points, Stroke::new(1.2, color)));
        ui.painter()
            .circle_stroke(center, 2.7, Stroke::new(1.2, color));
        if response.clicked() {
            self.set_document_view(if preview {
                crate::cli::DocumentView::Source
            } else {
                crate::cli::DocumentView::Preview
            });
        }
        response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text({
                let label = if preview {
                    "Edit Markdown source"
                } else {
                    "Preview Markdown"
                };
                let binding = self.shortcut_label(keybindings::Action::MarkdownPreview);
                if binding.is_empty() {
                    label.to_owned()
                } else {
                    format!("{label} · {binding}")
                }
            });
    }
    pub(super) fn finish_tab_drag_frame(&mut self, ui: &egui::Ui) {
        let Some(drag) = self.tab_drag.active.clone() else {
            return;
        };
        let position = self.ctx.pointer_hover_pos();
        let mut target = None;
        let mut marker = None;
        let mut body = None;
        if let Some(position) = position {
            for (pane, strip) in &self.tab_drag.strips {
                if !strip.rect.contains(position) {
                    continue;
                }
                let remaining: Vec<_> = strip
                    .tabs
                    .iter()
                    .filter(|(id, _)| *id != drag.tab)
                    .collect();
                let index = remaining
                    .iter()
                    .position(|(_, rect)| position.x < rect.center().x)
                    .unwrap_or(remaining.len());
                let x = remaining
                    .get(index)
                    .map(|(_, rect)| rect.left())
                    .or_else(|| remaining.last().map(|(_, rect)| rect.right()))
                    .unwrap_or(strip.rect.left());
                target = Some((*pane, index));
                marker = Some((
                    x.clamp(strip.rect.left() + 1.0, strip.rect.right() - 1.0),
                    strip.rect,
                ));
                break;
            }
            if target.is_none() {
                for (pane, rect) in self.tab_drag_pane_rects() {
                    let editor =
                        egui::Rect::from_min_max(rect.min + Vec2::new(0.0, 38.0), rect.max);
                    // Body drops append; strip drops choose an exact insertion point.
                    if pane != drag.pane
                        && editor.contains(position)
                        && let Some(strip) = self.tab_drag.strips.get(&pane)
                    {
                        target = Some((
                            pane,
                            strip.tabs.iter().filter(|(id, _)| *id != drag.tab).count(),
                        ));
                        body = Some(editor);
                        break;
                    }
                }
            }
        }
        if self.ctx.input(|input| input.pointer.primary_released()) {
            self.tab_drag.active = None;
            if let Some((pane, index)) = target
                && let Err(error) = self.drop_pane_tab(drag.pane, pane, drag.tab, index)
            {
                self.status(error, true);
            }
            self.ctx.request_repaint();
            return;
        }
        self.ctx.set_cursor_icon(if target.is_some() {
            egui::CursorIcon::Grabbing
        } else {
            egui::CursorIcon::NotAllowed
        });
        let painter = ui.painter();
        if let Some((x, rect)) = marker {
            painter.line_segment(
                [
                    egui::pos2(x, rect.top() + 3.0),
                    egui::pos2(x, rect.bottom() - 3.0),
                ],
                Stroke::new(2.5, ACCENT),
            );
        }
        if let Some(rect) = body {
            painter.rect_filled(rect.shrink(2.0), 4.0, ACCENT.linear_multiply(0.07));
            painter.rect_stroke(
                rect.shrink(2.0),
                4.0,
                Stroke::new(2.0, ACCENT),
                egui::StrokeKind::Inside,
            );
        }
        if let Some(position) = position {
            let ghost = self.ctx.layer_painter(egui::LayerId::new(
                egui::Order::Tooltip,
                egui::Id::new("vedit_dragged_tab"),
            ));
            let label = ghost.layout_no_wrap(
                drag.title.chars().take(70).collect::<String>(),
                FontId::proportional(12.0),
                TEXT,
            );
            let rect = egui::Rect::from_min_size(
                position + Vec2::new(13.0, 17.0),
                label.size() + Vec2::new(20.0, 14.0),
            );
            ghost.rect_filled(rect, 5.0, SURFACE);
            ghost.rect_stroke(
                rect,
                5.0,
                Stroke::new(1.0, BORDER),
                egui::StrokeKind::Inside,
            );
            ghost.galley(rect.min + Vec2::new(10.0, 7.0), label, TEXT);
        }
    }
}

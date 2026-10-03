//! Shared interaction and painting for Morrow's quiet controls.
//!
//! Keep colors in the active theme so hover, selection, and keyboard focus stay
//! consistent across the explorer, palettes, and toolbars.

use eframe::egui::{
    self, Color32, CursorIcon, FontId, Response, RichText, Sense, Stroke, Ui, Vec2, WidgetInfo,
    WidgetType,
};

/// A toolbar action with a visible hover/focus treatment and no resting box.
pub(crate) fn ghost_button(ui: &mut Ui, label: &str, tooltip: &str) -> Response {
    ui.scope(|ui| {
        let visuals = ui.visuals_mut();
        for visual in [
            &mut visuals.widgets.inactive,
            &mut visuals.widgets.hovered,
            &mut visuals.widgets.active,
        ] {
            visual.bg_stroke = Stroke::NONE;
            visual.corner_radius = 4.into();
        }
        ui.add(
            egui::Button::new(RichText::new(label).size(12.0))
                .frame_when_inactive(false)
                .min_size(Vec2::new(25.0, 25.0)),
        )
    })
    .inner
    .on_hover_cursor(CursorIcon::PointingHand)
    .on_hover_text(tooltip)
}

/// A menu item with a real right-aligned keyboard hint instead of spaces.
pub(crate) fn menu_item(ui: &mut Ui, label: &str, shortcut: &str, tooltip: &str) -> Response {
    let mut button = egui::Button::new(RichText::new(label).size(12.0))
        .frame_when_inactive(false)
        .min_size(Vec2::new(225.0, 27.0));
    if !shortcut.is_empty() {
        button = button.shortcut_text(RichText::new(shortcut).size(11.0));
    }
    ui.add(button)
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(tooltip)
}

/// A text menu with native popups and shared hover switching within its bar.
pub(crate) fn menu<R>(
    ui: &mut Ui,
    label: &str,
    tooltip: &str,
    contents: impl FnOnce(&mut Ui) -> R,
) -> Response {
    let button = egui::Button::new(RichText::new(label).size(12.0))
        .frame_when_inactive(false)
        .min_size(Vec2::new(38.0, 27.0));
    if egui::containers::menu::is_in_menu(ui) {
        let (response, _) = egui::containers::menu::SubMenuButton::from_button(
            button.right_text(egui::containers::menu::SubMenuButton::RIGHT_ARROW),
        )
        .ui(ui, contents);
        return response
            .on_hover_cursor(CursorIcon::PointingHand)
            .on_hover_text(tooltip);
    }

    let state_id = egui::containers::menu::find_menu_root(ui)
        .id
        .with("morrow_active_menu");
    let response = ui.add(button);
    let popup_id = egui::Popup::default_response_id(&response);
    let ctx = ui.ctx().clone();
    let active = ctx
        .data(|data| data.get_temp::<egui::Id>(state_id))
        .filter(|id| egui::Popup::is_id_open(&ctx, *id));
    // Native MenuButton in egui 0.36 only toggles on click. Once a menu is
    // open, desktop menu bars also transfer it on pointer movement. Check the
    // layer as well as the rectangle so an overlay cannot activate this bar.
    let pointer_over = ctx.pointer_hover_pos().is_some_and(|position| {
        response.rect.contains(position) && ctx.layer_id_at(position) == Some(response.layer_id)
    });
    if ui.is_enabled()
        && pointer_over
        && !response.clicked()
        && active.is_some_and(|id| id != popup_id)
    {
        egui::Popup::open_id(&ctx, popup_id);
        ctx.request_repaint();
    }

    let config = egui::containers::menu::MenuConfig::find(ui);
    egui::Popup::menu(&response)
        .close_behavior(config.close_behavior)
        .style(config.style)
        .show(contents);
    if egui::Popup::is_id_open(&ctx, popup_id) {
        ctx.data_mut(|data| data.insert_temp(state_id, popup_id));
    } else if active == Some(popup_id) || active.is_none() {
        ctx.data_mut(|data| data.remove::<egui::Id>(state_id));
    }
    response
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(tooltip)
}

/// The main action in a dialog, with themed hover and pressed states.
pub(crate) fn primary_button(ui: &mut Ui, label: &str, min_size: Vec2) -> Response {
    let accent = ui.visuals().selection.stroke.color;
    let text = ui.visuals().extreme_bg_color;
    ui.scope(|ui| {
        let visuals = ui.visuals_mut();
        visuals.override_text_color = Some(text);
        for (visual, fill) in [
            (&mut visuals.widgets.inactive, accent),
            (
                &mut visuals.widgets.hovered,
                accent.lerp_to_gamma(Color32::WHITE, 0.12),
            ),
            (
                &mut visuals.widgets.active,
                accent.lerp_to_gamma(Color32::BLACK, 0.12),
            ),
        ] {
            visual.bg_fill = fill;
            visual.weak_bg_fill = fill;
            visual.bg_stroke = Stroke::NONE;
            visual.fg_stroke = Stroke::new(1.0, text);
            visual.corner_radius = 6.into();
        }
        ui.add(
            egui::Button::new(RichText::new(label).size(12.5).strong().color(text))
                .frame(true)
                .min_size(min_size),
        )
    })
    .inner
    .on_hover_cursor(CursorIcon::PointingHand)
}

fn paint_row_background(ui: &Ui, rect: egui::Rect, response: &Response, selected: bool) {
    let visual = ui.visuals();
    if selected || response.hovered() || response.has_focus() {
        let fill = if selected {
            visual.selection.bg_fill
        } else {
            visual.widgets.hovered.weak_bg_fill
        };
        ui.painter().rect_filled(rect, 4.0, fill);
    }
    if selected {
        ui.painter().rect_filled(
            egui::Rect::from_min_max(
                rect.left_top() + Vec2::new(0.0, 5.0),
                rect.left_bottom() + Vec2::new(2.0, -5.0),
            ),
            1.0,
            visual.selection.stroke.color,
        );
    }
}

fn row_text_color(ui: &Ui, response: &Response, selected: bool) -> Color32 {
    let color = if selected || response.hovered() || response.has_focus() {
        ui.visuals().text_color()
    } else {
        ui.visuals()
            .text_color()
            .lerp_to_gamma(ui.visuals().weak_text_color(), 0.35)
    };
    enabled_color(ui, color)
}

fn enabled_color(ui: &Ui, color: Color32) -> Color32 {
    if ui.is_enabled() {
        color
    } else {
        color.gamma_multiply(0.5)
    }
}

pub(crate) fn truncated_galley(
    ui: &Ui,
    text: &str,
    font: FontId,
    color: Color32,
    width: f32,
) -> std::sync::Arc<egui::Galley> {
    ui.fonts_mut(|fonts| {
        let mut job = egui::text::LayoutJob::simple(text.into(), font, color, width.max(1.0));
        job.wrap.max_rows = 1;
        job.wrap.break_anywhere = true;
        fonts.layout_job(job)
    })
}

/// A full-width, single-line navigation row with left-aligned contents.
/// Callers with dynamic lists should wrap each row in `Ui::push_id`.
pub(crate) fn left_row(
    ui: &mut Ui,
    label: &str,
    badge: Option<(&str, Color32)>,
    selected: bool,
    tooltip: &str,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width().max(1.0), 28.0),
        Sense::click(),
    );
    response.widget_info(|| {
        WidgetInfo::selected(
            WidgetType::SelectableLabel,
            ui.is_enabled(),
            selected,
            label,
        )
    });
    if ui.is_rect_visible(rect) {
        paint_row_background(ui, rect, &response, selected);
        let mut left = rect.left() + 10.0;
        if let Some((badge, color)) = badge {
            let color = enabled_color(ui, color);
            if badge == ">" || badge == "v" {
                let center = egui::pos2(left + 10.0, rect.center().y);
                let points = if badge == ">" {
                    [
                        center + Vec2::new(-2.0, -4.0),
                        center + Vec2::new(2.0, 0.0),
                        center + Vec2::new(-2.0, 4.0),
                    ]
                } else {
                    [
                        center + Vec2::new(-4.0, -2.0),
                        center + Vec2::new(0.0, 2.0),
                        center + Vec2::new(4.0, -2.0),
                    ]
                };
                ui.painter()
                    .add(egui::Shape::line(points.into(), Stroke::new(1.25, color)));
            } else {
                let galley = truncated_galley(ui, badge, FontId::monospace(10.5), color, 24.0);
                ui.painter().galley(
                    egui::pos2(left, rect.center().y - galley.size().y * 0.5),
                    galley,
                    color,
                );
            }
            left += 31.0;
        }
        let color = row_text_color(ui, &response, selected);
        let max_width = (rect.right() - left - 10.0).max(1.0);
        let galley = truncated_galley(ui, label, FontId::proportional(12.0), color, max_width);
        ui.painter()
            .with_clip_rect(rect.intersect(ui.clip_rect()))
            .galley(
                egui::pos2(left, rect.center().y - galley.size().y * 0.5),
                galley,
                color,
            );
    }
    response
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(tooltip)
}

/// A navigation row that keeps the filename/action and its context distinct.
pub(crate) fn detail_row(
    ui: &mut Ui,
    label: &str,
    detail: &str,
    selected: bool,
    tooltip: &str,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width().max(1.0), 48.0),
        Sense::click(),
    );
    response.widget_info(|| {
        WidgetInfo::selected(
            WidgetType::SelectableLabel,
            ui.is_enabled(),
            selected,
            format!("{label}: {detail}"),
        )
    });
    if ui.is_rect_visible(rect) {
        paint_row_background(ui, rect, &response, selected);
        let left = rect.left() + 12.0;
        let width = (rect.width() - 24.0).max(1.0);
        let color = row_text_color(ui, &response, selected);
        let primary = truncated_galley(ui, label, FontId::proportional(12.0), color, width);
        let top = if detail.is_empty() {
            rect.center().y - primary.size().y * 0.5
        } else {
            rect.top() + 7.0
        };
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        painter.galley(egui::pos2(left, top), primary, color);
        if !detail.is_empty() {
            let muted = enabled_color(ui, ui.visuals().weak_text_color());
            let secondary = truncated_galley(ui, detail, FontId::proportional(10.5), muted, width);
            painter.galley(egui::pos2(left, rect.top() + 27.0), secondary, muted);
        }
    }
    response
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(tooltip)
}

/// Explorer geometry is shared by painting, virtualization and keyboard paging.
pub(crate) const EXPLORER_ROW_HEIGHT: f32 = 26.0;
const EXPLORER_INDENT: f32 = 16.0;

/// Tree metadata belongs to the visible row so guides remain correct when the
/// ScrollArea paints only a slice of the tree.
pub(crate) struct ExplorerRow<'a> {
    pub label: &'a str,
    pub depth: usize,
    /// Whether each ancestor has a following sibling. The root-level ancestor
    /// needs no trunk; subsequent ancestors continue the preceding guide column.
    pub ancestor_continuations: &'a [bool],
    pub last_sibling: bool,
    /// `None` is a file; `Some` is a closed or expanded directory.
    pub expanded: Option<bool>,
    pub file_color: Color32,
    pub selected: bool,
    pub active_file: bool,
    pub tooltip: &'a str,
}

/// A calm, full-width tree row. Disclosure, icon and filename each have their
/// own fixed column; selection never shifts with a folder's indentation.
pub(crate) fn explorer_row(ui: &mut Ui, row: ExplorerRow<'_>) -> Response {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width().max(1.0), EXPLORER_ROW_HEIGHT),
        Sense::click(),
    );
    response.widget_info(|| {
        WidgetInfo::selected(
            WidgetType::SelectableLabel,
            ui.is_enabled(),
            row.selected,
            row.label,
        )
    });
    if ui.is_rect_visible(rect) {
        let visuals = ui.visuals();
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        let highlighted = row.selected || response.has_focus();
        let fill = if highlighted {
            Some(
                visuals
                    .selection
                    .bg_fill
                    .lerp_to_gamma(visuals.panel_fill, 0.24),
            )
        } else if response.hovered() {
            Some(
                visuals
                    .widgets
                    .hovered
                    .weak_bg_fill
                    .lerp_to_gamma(visuals.panel_fill, 0.42),
            )
        } else {
            None
        };
        if let Some(fill) = fill {
            painter.rect_filled(rect.shrink2(Vec2::new(2.0, 0.0)), 3.0, fill);
        }
        if highlighted || row.active_file {
            let color = if highlighted {
                visuals.selection.stroke.color
            } else {
                visuals
                    .selection
                    .stroke
                    .color
                    .lerp_to_gamma(visuals.weak_text_color(), 0.5)
            };
            painter.rect_filled(
                egui::Rect::from_min_max(
                    rect.left_top() + Vec2::new(3.0, 6.0),
                    rect.left_bottom() + Vec2::new(5.0, -6.0),
                ),
                1.0,
                enabled_color(ui, color),
            );
        }

        let tree_left = rect.left() + 9.0;
        // Keep names usable in narrow sidebars and unusually deep paths. The
        // tooltip always exposes the complete path; ordinary levels stay exact.
        let indent = (row.depth as f32 * EXPLORER_INDENT).min((rect.width() - 112.0).max(0.0));
        let disclosure_x = tree_left + indent + 6.0;
        let guide = enabled_color(
            ui,
            visuals
                .weak_text_color()
                .lerp_to_gamma(visuals.panel_fill, 0.78),
        );
        let guide_stroke = Stroke::new(1.0, guide);
        for (level, continues) in row.ancestor_continuations.iter().enumerate().skip(1) {
            let x = tree_left + (level - 1) as f32 * EXPLORER_INDENT + 6.0;
            if *continues && x < disclosure_x - 1.0 {
                painter.line_segment(
                    [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                    guide_stroke,
                );
            }
        }
        if row.depth > 0 && indent > 0.0 {
            let parent_x = disclosure_x - EXPLORER_INDENT.min(indent);
            painter.line_segment(
                [
                    egui::pos2(parent_x, rect.top()),
                    egui::pos2(
                        parent_x,
                        if row.last_sibling {
                            rect.center().y
                        } else {
                            rect.bottom()
                        },
                    ),
                ],
                guide_stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(parent_x, rect.center().y),
                    egui::pos2(disclosure_x - 3.0, rect.center().y),
                ],
                guide_stroke,
            );
        }

        let muted = enabled_color(ui, visuals.weak_text_color());
        if let Some(expanded) = row.expanded {
            let center = egui::pos2(disclosure_x, rect.center().y);
            let points = if expanded {
                [
                    center + Vec2::new(-3.0, -1.5),
                    center + Vec2::new(0.0, 1.5),
                    center + Vec2::new(3.0, -1.5),
                ]
            } else {
                [
                    center + Vec2::new(-1.5, -3.0),
                    center + Vec2::new(1.5, 0.0),
                    center + Vec2::new(-1.5, 3.0),
                ]
            };
            painter.add(egui::Shape::line(points.into(), Stroke::new(1.15, muted)));
        }
        let icon_center = egui::pos2(disclosure_x + 18.0, rect.center().y);
        let icon_color = enabled_color(
            ui,
            if row.expanded.is_some() {
                visuals
                    .weak_text_color()
                    .lerp_to_gamma(visuals.text_color(), 0.2)
            } else {
                row.file_color
                    .lerp_to_gamma(visuals.weak_text_color(), 0.62)
            },
        );
        paint_explorer_icon(&painter, icon_center, row.expanded, icon_color);

        let left = icon_center.x + 13.0;
        let color = if highlighted || response.hovered() || row.expanded.is_some() {
            enabled_color(ui, visuals.text_color())
        } else {
            enabled_color(
                ui,
                visuals
                    .text_color()
                    .lerp_to_gamma(visuals.weak_text_color(), 0.12),
            )
        };
        let galley = truncated_galley(
            ui,
            row.label,
            FontId::proportional(12.0),
            color,
            (rect.right() - left - 8.0).max(1.0),
        );
        painter.galley(
            egui::pos2(left, rect.center().y - galley.size().y * 0.5),
            galley,
            color,
        );
    }
    response
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(row.tooltip)
}

fn paint_explorer_icon(
    painter: &egui::Painter,
    center: egui::Pos2,
    expanded: Option<bool>,
    color: Color32,
) {
    let stroke = Stroke::new(1.0, color);
    let points: &[Vec2] = match expanded {
        Some(false) => &[
            Vec2::new(-7.0, -5.0),
            Vec2::new(-2.0, -5.0),
            Vec2::new(0.0, -3.0),
            Vec2::new(7.0, -3.0),
            Vec2::new(7.0, 5.0),
            Vec2::new(-7.0, 5.0),
            Vec2::new(-7.0, -5.0),
        ],
        Some(true) => &[
            Vec2::new(-7.0, 5.0),
            Vec2::new(-7.0, -5.0),
            Vec2::new(-2.0, -5.0),
            Vec2::new(0.0, -3.0),
            Vec2::new(6.0, -3.0),
            Vec2::new(6.0, -1.0),
        ],
        None => &[
            Vec2::new(-5.0, -6.0),
            Vec2::new(2.0, -6.0),
            Vec2::new(5.0, -3.0),
            Vec2::new(5.0, 6.0),
            Vec2::new(-5.0, 6.0),
            Vec2::new(-5.0, -6.0),
        ],
    };
    painter.add(egui::Shape::line(
        points.iter().map(|offset| center + *offset).collect(),
        stroke,
    ));
    if expanded == Some(true) {
        painter.add(egui::Shape::line(
            [
                Vec2::new(-7.0, 5.0),
                Vec2::new(-4.0, -1.0),
                Vec2::new(8.0, -1.0),
                Vec2::new(5.0, 5.0),
                Vec2::new(-7.0, 5.0),
            ]
            .into_iter()
            .map(|offset| center + offset)
            .collect(),
            stroke,
        ));
    } else if expanded.is_none() {
        painter.add(egui::Shape::line(
            [
                Vec2::new(2.0, -6.0),
                Vec2::new(2.0, -3.0),
                Vec2::new(5.0, -3.0),
            ]
            .into_iter()
            .map(|offset| center + offset)
            .collect(),
            stroke,
        ));
        painter.line_segment(
            [center + Vec2::new(-2.0, 2.0), center + Vec2::new(2.0, 2.0)],
            stroke,
        );
    }
}

#[cfg(test)]
#[path = "menu_tests.rs"]
mod menu_tests;

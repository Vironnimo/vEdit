//! Review header, empty state and navigable file checklist.
use super::*;

pub(super) fn review_header(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    view: &ChangeReview,
    seen: &SeenMarks,
    context: &feedback::ReviewContext,
    actions: &mut Vec<ReviewAction>,
) {
    ui.painter().rect_filled(rect, 0.0, PANEL);
    ui.painter().line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1.0, BORDER),
    );
    let mut header = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(Vec2::new(10.0, 0.0)))
            .layout(Layout::right_to_left(Align::Center))
            .id_salt("change-review-header"),
    );
    let ui = &mut header;
    ui.spacing_mut().item_spacing.x = 10.0;
    let close = ui
        .add(
            egui::Button::new(RichText::new("×").size(16.0))
                .frame_when_inactive(false)
                .min_size(Vec2::new(26.0, 26.0)),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Close the review and return to the editor · Esc");
    close.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Close change review")
    });
    if close.clicked() {
        actions.push(ReviewAction::Close);
    }
    let refresh = ui
        .add_enabled_ui(!view.loading, |ui| {
            design::ghost_button(
                ui,
                "Refresh",
                "Compare again; seen marks and the reading position stay",
            )
        })
        .inner;
    if refresh.clicked() {
        actions.push(ReviewAction::Refresh);
    }
    let marks = design::ghost_button(
        ui,
        &format!("For agent ({})", context.marked_blocks),
        "Open marked blocks and review comments across all projects",
    );
    marks.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), "For agent")
    });
    if marks.clicked() {
        actions.push(ReviewAction::AgentMarks);
    }
    if context.stale && !view.loading {
        feedback::stale_indicator(ui);
    }
    if view.comparison.is_some() {
        let count = view.files.len();
        ui.label(
            RichText::new(format!("{} of {count} seen", view.seen_count(seen)))
                .size(11.5)
                .color(MUTED),
        )
        .on_hover_text("Space marks the current file seen and moves to the next unseen file. A file that changes again becomes unseen.");
        let (added, removed) = view.totals();
        let tip = "Lines added and removed in this comparison";
        ui.label(
            RichText::new(format!("−{}", git_panel::compact_number(removed)))
                .size(11.5)
                .color(RED),
        )
        .on_hover_text(format!("{removed} lines removed"));
        ui.label(
            RichText::new(format!("+{}", git_panel::compact_number(added)))
                .size(11.5)
                .color(GREEN),
        )
        .on_hover_text(format!("{added} lines added · {tip}"));
        let files = format!("{count} {}", if count == 1 { "file" } else { "files" });
        let label = ui.label(RichText::new(files).size(11.5).color(MUTED));
        if view.omitted_untracked > 0 {
            label.on_hover_text(format!(
                "{} more untracked files are not listed",
                view.omitted_untracked
            ));
        }
    }
    if view.loading {
        ui.add(egui::Spinner::new().size(13.0).color(MUTED))
            .on_hover_text("Comparing…");
    }
    if let Some(error) = &view.error
        && !view.files.is_empty()
    {
        ui.label(RichText::new("Refresh failed").size(11.5).color(ORANGE))
            .on_hover_text(error);
    }
    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
        let detail = view
            .comparison
            .as_ref()
            .map_or("", |comparison| comparison.detail.as_str());
        let title = ui.add(
            egui::Label::new(RichText::new(view.title()).size(13.0).strong().color(TEXT))
                .truncate(),
        );
        if !detail.is_empty() {
            title.on_hover_text(detail);
        }
    });
}

pub(super) fn empty_review(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    view: &ChangeReview,
    actions: &mut Vec<ReviewAction>,
) {
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink(24.0))
            .layout(Layout::top_down(Align::Center))
            .id_salt("change-review-empty"),
    );
    let ui = &mut child;
    ui.add_space((rect.height() * 0.25).clamp(12.0, 160.0));
    if let Some(error) = &view.error {
        ui.label(
            RichText::new("The comparison could not be loaded")
                .strong()
                .color(TEXT),
        );
        ui.add_space(6.0);
        ui.add(egui::Label::new(RichText::new(error).size(12.0).color(ORANGE)).wrap());
        ui.add_space(10.0);
        if !view.loading && design::ghost_button(ui, "Try again", "Compare again").clicked() {
            actions.push(ReviewAction::Refresh);
        }
    } else if view.loading || view.comparison.is_none() {
        ui.add(egui::Spinner::new().size(18.0).color(MUTED));
        ui.add_space(8.0);
        ui.label(RichText::new("Comparing…").size(12.0).color(MUTED));
    } else {
        ui.label(RichText::new("No changes").strong().color(TEXT));
        ui.add_space(6.0);
        let detail = match view.source {
            ReviewSource::WorkingTree => "The working tree matches the last commit.",
            ReviewSource::Commit(_) => "This commit changes no files.",
            ReviewSource::Branch { .. } => "This branch has no changes since it left its base.",
        };
        ui.label(RichText::new(detail).size(12.0).color(MUTED));
    }
}

enum ListRow {
    Folder(usize),
    File(usize, usize),
}

/// Files grouped by folder; the change set is already sorted that way.
fn list_rows(view: &ChangeReview) -> Vec<ListRow> {
    let mut rows = Vec::with_capacity(view.files.len() + 8);
    let mut folder = None;
    for (index, entry) in view.files.iter().enumerate() {
        let current = entry.folder();
        if !current.is_empty() && folder != Some(current) {
            rows.push(ListRow::Folder(index));
        }
        folder = Some(current);
        rows.push(ListRow::File(index, usize::from(!current.is_empty())));
    }
    rows
}

/// Returns true when a row or seen box was clicked.
pub(super) fn file_list(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    view: &mut ChangeReview,
    seen: &mut SeenMarks,
) -> bool {
    let rows = list_rows(view);
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(Vec2::new(4.0, 4.0)))
            .layout(Layout::top_down(Align::Min))
            .id_salt("change-review-list"),
    );
    let ui = &mut child;
    ui.spacing_mut().item_spacing.y = 0.0;
    let mut scroll = egui::ScrollArea::vertical()
        .id_salt("change-review-files")
        .auto_shrink([false, false])
        .animated(false);
    if view.follow_list
        && let Some(row) = rows
            .iter()
            .position(|row| matches!(row, ListRow::File(index, _) if *index == view.current))
    {
        let top = row as f32 * LIST_ROW_HEIGHT;
        if top < view.list_offset {
            scroll = scroll.vertical_scroll_offset(top);
        } else if top + LIST_ROW_HEIGHT > view.list_offset + view.list_height {
            scroll = scroll.vertical_scroll_offset(top + LIST_ROW_HEIGHT - view.list_height);
        }
    }
    view.follow_list = false;
    let mut jump = None;
    let mut toggle = None;
    let output = scroll.show_rows(ui, LIST_ROW_HEIGHT, rows.len(), |ui, range| {
        for row in &rows[range] {
            match *row {
                ListRow::Folder(index) => folder_row(ui, view.files[index].folder()),
                ListRow::File(index, depth) => {
                    let (clicked, toggled) = file_row(
                        ui,
                        &view.files[index],
                        depth,
                        index == view.current,
                        view.is_seen(seen, index),
                    );
                    if clicked {
                        jump = Some(index);
                    }
                    if toggled {
                        toggle = Some(index);
                    }
                }
            }
        }
    });
    view.list_offset = output.state.offset.y;
    view.list_height = output.inner_rect.height();
    if let Some(index) = toggle {
        let value = !view.is_seen(seen, index);
        view.set_seen(seen, index, value);
    } else if let Some(index) = jump {
        view.jump = Some(index);
        view.current = index;
    }
    toggle.is_some() || jump.is_some()
}

fn folder_row(ui: &mut egui::Ui, folder: &str) {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width().max(1.0), LIST_ROW_HEIGHT),
        egui::Sense::hover(),
    );
    if !ui.is_rect_visible(rect) {
        return;
    }
    let galley = design::truncated_galley(
        ui,
        &format!("{folder}/"),
        FontId::proportional(11.0),
        MUTED,
        rect.width() - 12.0,
    );
    let truncated = galley.elided;
    ui.painter().galley(
        egui::pos2(rect.left() + 6.0, rect.center().y - galley.size().y * 0.5),
        galley,
        MUTED,
    );
    if truncated {
        response.on_hover_text(folder);
    }
}

/// Returns whether the row was clicked and whether its seen box was toggled.
fn file_row(
    ui: &mut egui::Ui,
    entry: &ReviewEntry,
    depth: usize,
    current: bool,
    seen: bool,
) -> (bool, bool) {
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width().max(1.0), LIST_ROW_HEIGHT),
        egui::Sense::hover(),
    );
    let id = ui.id().with(("change-review-file", &entry.path));
    let row = ui.interact(rect, id, egui::Sense::click());
    row.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            true,
            current,
            &entry.path,
        )
    });
    let x = rect.left() + 6.0 + depth as f32 * 12.0;
    let check_rect =
        egui::Rect::from_center_size(egui::pos2(x + 6.0, rect.center().y), Vec2::splat(12.0));
    let check = ui.interact(
        check_rect.expand(4.0),
        id.with("seen"),
        egui::Sense::click(),
    );
    check.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::Checkbox,
            true,
            seen,
            format!("Seen {}", entry.path),
        )
    });
    if ui.is_rect_visible(rect) {
        if current || row.hovered() || check.hovered() {
            ui.painter()
                .rect_filled(rect, 4.0, if current { HOVER } else { SURFACE });
        }
        if design::keyboard_focus(&row) || design::keyboard_focus(&check) {
            ui.painter().rect_stroke(
                rect.shrink(1.0),
                4.0,
                Stroke::new(1.0, ACCENT),
                egui::StrokeKind::Inside,
            );
        }
        paint_seen_box(ui, check_rect, seen, check.hovered());
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        let (mark, color, _) = git_panel::change_marker(entry.status, false);
        painter.text(
            egui::pos2(x + 23.0, rect.center().y),
            egui::Align2::CENTER_CENTER,
            mark,
            FontId::monospace(10.0),
            color,
        );
        let stats_width =
            paint_list_stats(ui, &painter, rect.right() - 6.0, rect.center().y, entry);
        let color = if seen && !current {
            MUTED
        } else if current || row.hovered() {
            TEXT
        } else {
            TEXT.lerp_to_gamma(MUTED, 0.35)
        };
        let left = x + 33.0;
        let galley = design::truncated_galley(
            ui,
            entry.name(),
            FontId::proportional(12.0),
            color,
            (rect.right() - 12.0 - stats_width - left).max(1.0),
        );
        painter.galley(
            egui::pos2(left, rect.center().y - galley.size().y * 0.5),
            galley,
            color,
        );
    }
    let toggled = check.clicked();
    let clicked = row.clicked();
    row.on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(match &entry.original_path {
            Some(original) => format!("{}\nRenamed from {original}", entry.path),
            None => entry.path.clone(),
        });
    check
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(if seen {
            "Mark as not seen"
        } else {
            "Mark as seen"
        });
    (clicked, toggled)
}

fn paint_seen_box(ui: &egui::Ui, rect: egui::Rect, seen: bool, hovered: bool) {
    let painter = ui.painter();
    if seen {
        painter.rect_filled(rect, 3.0, BORDER_STRONG);
        painter.add(egui::Shape::line(
            vec![
                rect.left_center() + Vec2::new(2.5, 0.0),
                rect.center_bottom() + Vec2::new(-1.0, -3.0),
                rect.right_top() + Vec2::new(-2.5, 3.0),
            ],
            Stroke::new(1.5, TEXT),
        ));
    } else {
        painter.rect_stroke(
            rect,
            3.0,
            Stroke::new(1.0, if hovered { MUTED } else { BORDER_STRONG }),
            egui::StrokeKind::Inside,
        );
    }
}

/// Right-aligned compact line counts; returns their width.
fn paint_list_stats(
    ui: &egui::Ui,
    painter: &egui::Painter,
    right: f32,
    center: f32,
    entry: &ReviewEntry,
) -> f32 {
    let font = FontId::monospace(10.0);
    let parts: Vec<(String, Color32)> = match entry.stats {
        Some(stats) if stats.binary => vec![("binary".into(), MUTED)],
        Some(stats) => vec![
            (
                format!("+{}", git_panel::compact_number(stats.added)),
                GREEN,
            ),
            (
                format!("−{}", git_panel::compact_number(stats.removed)),
                RED,
            ),
        ],
        None => return 0.0,
    };
    let mut x = right;
    for (text, color) in parts.into_iter().rev() {
        let galley = design::truncated_galley(ui, &text, font.clone(), color, 60.0);
        x -= galley.size().x;
        painter.galley(egui::pos2(x, center - galley.size().y * 0.5), galley, color);
        x -= 5.0;
    }
    right - x - 5.0
}

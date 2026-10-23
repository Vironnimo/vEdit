//! Continuous diff viewport, row navigation and sticky file headers.
use super::input::take_review_keys;
use super::*;

/// The first new-side line below the sticky header of `file`.
fn line_at(view: &ChangeReview, file: usize, offset: f32) -> Option<usize> {
    let entry = view.files.get(file)?;
    let EntryPatch::Parsed(patch) = &entry.patch else {
        return None;
    };
    if patch.rows() == 0 {
        return None;
    }
    let local = (offset - view.tops.get(file).copied().unwrap_or(0.0)).max(0.0);
    patch.new_line_at(entry.wrapped.visible(local..local + 1.0).start)
}

pub(super) fn diff_body(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    view: &mut ChangeReview,
    seen: &mut SeenMarks,
    body_id: egui::Id,
    context: &feedback::ReviewContext,
    actions: &mut Vec<ReviewAction>,
) {
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::top_down(Align::Min))
            .id_salt("change-review-diff"),
    );
    let ui = &mut child;
    ui.spacing_mut().item_spacing.y = 0.0;
    let content_width = (rect.width() - ui.spacing().scroll.allocated_width())
        .floor()
        .max(1.0);
    let digits = view
        .files
        .iter()
        .filter_map(|entry| match &entry.patch {
            EntryPatch::Parsed(patch) => Some(patch.number_digits()),
            _ => None,
        })
        .max()
        .unwrap_or(0)
        .max(MIN_NUMBER_DIGITS);
    let metrics = RowMetrics::new(ui, digits, content_width);
    let mut tops = Vec::with_capacity(view.files.len());
    let mut bottom = 0.0;
    for entry in &mut view.files {
        if let EntryPatch::Parsed(patch) = &entry.patch {
            entry.wrapped.update(ui, patch, &metrics);
        }
        tops.push(bottom);
        bottom += FILE_HEADER_HEIGHT + entry.body_height() + FILE_GAP;
    }
    view.tops = tops;
    let viewport_height = rect.height().max(1.0);
    // The last file's header can always scroll to the top.
    let last_top = view.tops.last().copied().unwrap_or(0.0);
    let content_height = (bottom - FILE_GAP).max(last_top + viewport_height);
    let maximum = (content_height - viewport_height).max(0.0);

    let body = ui.interact(rect, body_id, egui::Sense::click());
    body.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "Change review diff")
    });
    // The list is for pointing; the keys keep working on the diff. Requested
    // after the body's interaction, so the click elsewhere does not undo it.
    if body.clicked() || body.secondary_clicked() || std::mem::take(&mut view.list_clicked) {
        body.request_focus();
    }
    feedback::validate_selection(view);
    // A click chooses a diff row, Shift+click extends the choice, and a
    // right-click outside it chooses the row under the pointer.
    let pointer = (body.clicked() || body.secondary_clicked())
        .then(|| body.interact_pointer_pos())
        .flatten()
        .map(|position| feedback::RowClick {
            position,
            extend: body.clicked() && ui.input(|input| input.modifiers.shift),
            secondary: body.secondary_clicked(),
        });
    if view.focus_pending {
        let focused = ui.ctx().memory(|memory| memory.focused());
        if focused.is_none_or(|id| id == body_id) {
            body.request_focus();
        }
        // A closing dialog can still hold the frame's focus; ask again then.
        view.focus_pending = focused.is_none();
    }
    let focused = ui.ctx().memory(|memory| memory.has_focus(body_id));
    if focused {
        ui.ctx().memory_mut(|memory| {
            memory.set_focus_lock_filter(
                body_id,
                egui::EventFilter {
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    escape: true,
                    tab: false,
                },
            )
        });
    }

    let salt = "change-review-diff-scroll";
    // ScrollArea builds its ID from the salt wrapped in IdSalt.
    let scroll_id = ui.make_persistent_id(egui::IdSalt::new(salt));
    let mut offset = view.scroll;
    if let Some(wheel) =
        render::instant_scroll_offset_axes(ui, scroll_id, egui::Vec2b::new(false, true))
    {
        offset = wheel.y;
    }
    if !view.loading
        && let Some((path, within)) = view.anchor.take()
        && let Some(index) = view.files.iter().position(|entry| entry.path == path)
    {
        let height = FILE_HEADER_HEIGHT + view.files[index].body_height();
        offset = view.tops[index] + within.min(height);
    }
    let top_layer = ui
        .ctx()
        .memory(|memory| memory.areas().top_layer_id(ui.layer_id().order));
    let mut keys = std::mem::take(&mut view.pending_commands);
    if focused
        && ui.is_enabled()
        && !egui::Popup::is_any_open(ui.ctx())
        && top_layer.is_none_or(|layer| layer == ui.layer_id())
    {
        keys.extend(take_review_keys(
            ui.ctx(),
            context.swap_jk,
            &context.bindings,
        ));
    }
    for key in keys {
        let current = view.file_at(offset);
        match key {
            ReviewKey::Lines(lines) => offset += lines * metrics.row_height,
            ReviewKey::Pages(pages) => offset += pages * viewport_height * 0.9,
            ReviewKey::Top => offset = 0.0,
            ReviewKey::Bottom => offset = maximum,
            ReviewKey::NextFile => {
                if let Some(top) = view.tops.get(current + 1) {
                    offset = *top;
                }
            }
            ReviewKey::PreviousFile => {
                offset = if offset > view.tops[current] + 0.5 {
                    view.tops[current]
                } else {
                    view.tops[current.saturating_sub(1)]
                };
            }
            ReviewKey::SeenNext => {
                view.set_seen(seen, current, true);
                let count = view.files.len();
                match (1..count)
                    .map(|step| (current + step) % count)
                    .find(|index| !view.is_seen(seen, *index))
                {
                    Some(next) => offset = view.tops[next],
                    None => actions.push(ReviewAction::Status("Every file is marked seen".into())),
                }
            }
            ReviewKey::Open => actions.push(match &view.selection {
                Some(selection) => ReviewAction::Open {
                    file: selection.file,
                    line: feedback::selection_line(view, selection),
                },
                None => ReviewAction::Open {
                    file: current,
                    line: line_at(view, current, offset),
                },
            }),
            ReviewKey::Close if view.selection.is_some() => view.selection = None,
            ReviewKey::Close => actions.push(ReviewAction::Close),
            ReviewKey::Extend(step) => {
                if let Some(target) = feedback::extend_selection(view, current, offset, step) {
                    // Keep the moving end below the sticky header and on screen.
                    let top = target - FILE_HEADER_HEIGHT;
                    let bottom = target + metrics.row_height - viewport_height;
                    offset = offset.clamp(bottom, top.max(bottom));
                }
            }
            ReviewKey::Comment => {
                if view.selection.is_none() {
                    view.selection = feedback::row_on_screen(view, current, offset);
                }
                match feedback::CommentDraft::new(view) {
                    Some(draft) => view.comment = Some(draft),
                    None => actions.push(ReviewAction::Status(
                        "Select changed or unchanged diff lines to comment on".into(),
                    )),
                }
            }
        }
        offset = offset.clamp(0.0, maximum);
    }
    if let Some(index) = view.jump.take()
        && let Some(top) = view.tops.get(index)
    {
        offset = *top;
    }
    offset = offset.clamp(0.0, maximum);

    let scroll = egui::ScrollArea::vertical()
        .id_salt(salt)
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
        .content_margin(egui::Margin::ZERO)
        .auto_shrink([false, false])
        .animated(false)
        .scroll_offset(Vec2::new(0.0, offset))
        .scroll_source(egui::scroll_area::ScrollSource {
            mouse_wheel: false,
            ..Default::default()
        });
    let keyboard = design::keyboard_focus(&body);
    let palette = diff_view::Palette::new(ui);
    let output = scroll.show_viewport(ui, |ui, viewport| {
        ui.set_min_size(Vec2::new(content_width, content_height));
        let origin = ui.max_rect().min;
        let first = view
            .tops
            .partition_point(|top| *top <= viewport.top())
            .saturating_sub(1);
        let mut visible = Vec::new();
        let mut deferred = Vec::new();
        let mut hit = None;
        for index in first..view.files.len() {
            let top = view.tops[index];
            // Request deferred patches a screen ahead of the reader.
            if top >= viewport.bottom() + viewport.height() {
                break;
            }
            let entry = &view.files[index];
            if matches!(entry.patch, EntryPatch::Deferred) {
                deferred.push(index);
            }
            if top >= viewport.bottom() {
                continue;
            }
            let body_top = top + FILE_HEADER_HEIGHT;
            let height = entry.body_height();
            if body_top + height < viewport.top() {
                continue;
            }
            visible.push(index);
            let body_rect = egui::Rect::from_min_size(
                origin + Vec2::new(0.0, body_top),
                Vec2::new(content_width, height),
            );
            match &entry.patch {
                EntryPatch::Parsed(patch) if patch.rows() > 0 => {
                    if let Some(click) = &pointer
                        && body_rect.contains(click.position)
                    {
                        let local = click.position.y - body_rect.top();
                        hit = Some((index, entry.wrapped.visible(local..local + 0.01).start));
                    }
                    for row in entry
                        .wrapped
                        .visible(viewport.top() - body_top..viewport.bottom() - body_top)
                    {
                        let row_rect = egui::Rect::from_min_max(
                            origin + Vec2::new(0.0, body_top + entry.wrapped.offset(row)),
                            origin
                                + Vec2::new(
                                    content_width,
                                    body_top + entry.wrapped.offset(row + 1),
                                ),
                        );
                        diff_view::paint_row(
                            ui,
                            &palette,
                            patch,
                            row,
                            row_rect,
                            &metrics,
                            body_id.with((index, row)),
                        );
                        if view
                            .selection
                            .as_ref()
                            .is_some_and(|selection| selection.contains(index, row))
                        {
                            feedback::paint_selected_row(ui, row_rect);
                        }
                    }
                }
                patch => {
                    if file_note(ui, body_rect, index, entry, patch) {
                        actions.push(ReviewAction::Load {
                            files: vec![index],
                            automatic: false,
                        });
                    }
                }
            }
        }
        // Headers paint above the rows. The current one sticks to the top
        // until the end of its file pushes it away.
        let current = view.file_at(viewport.top());
        let mut toggles = Vec::new();
        for index in visible {
            let top = view.tops[index];
            let y = if index == current {
                let end = top + FILE_HEADER_HEIGHT + view.files[index].body_height();
                viewport
                    .top()
                    .clamp(top, (end - FILE_HEADER_HEIGHT).max(top))
            } else {
                top
            };
            let rect = egui::Rect::from_min_size(
                origin + Vec2::new(0.0, y),
                Vec2::new(content_width, FILE_HEADER_HEIGHT),
            );
            if pointer
                .as_ref()
                .is_some_and(|click| rect.contains(click.position))
            {
                // The header covers rows scrolled beneath it.
                hit = None;
            }
            let response = file_header(
                ui,
                rect,
                &view.files[index],
                view.is_seen(seen, index),
                index == current && keyboard,
                &context.bindings,
            );
            if response.open {
                let line = match &view.files[index].patch {
                    EntryPatch::Parsed(patch) if patch.rows() > 0 => patch.new_line_at(0),
                    _ => None,
                };
                actions.push(ReviewAction::Open { file: index, line });
            }
            if let Some(value) = response.seen {
                toggles.push((index, value));
            }
        }
        (deferred, toggles, hit)
    });
    render::remember_scroll_limits(ui.ctx(), &output);
    let (deferred, toggles, hit) = output.inner;
    if let (Some(click), Some((file, row))) = (pointer, hit) {
        feedback::click_row(view, file, row, click);
    }
    if let Some(comment) = feedback::diff_menu(&body, view, &context.bindings) {
        match comment {
            Some(draft) => view.comment = Some(draft),
            None => actions.push(ReviewAction::Status(
                "Select changed or unchanged diff lines to comment on".into(),
            )),
        }
    }
    if !toggles.is_empty() {
        ui.ctx().memory_mut(|memory| memory.request_focus(body_id));
    }
    for (index, value) in toggles {
        view.set_seen(seen, index, value);
    }
    if !deferred.is_empty() && view.lazy_request.is_none() {
        actions.push(ReviewAction::Load {
            files: deferred.into_iter().take(LAZY_FILES).collect(),
            automatic: true,
        });
    }
    view.scroll = output.state.offset.y;
    let current = view.file_at(view.scroll);
    if current != view.current {
        view.current = current;
        view.follow_list = true;
    }
}

/// A placeholder in place of a patch. Returns true when "Load" was clicked.
fn file_note(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    index: usize,
    entry: &ReviewEntry,
    patch: &EntryPatch,
) -> bool {
    let changed = entry
        .stats
        .map(|stats| stats.added + stats.removed)
        .unwrap_or(0);
    let text = match patch {
        EntryPatch::Large => format!("Large diff · {changed} changed lines"),
        EntryPatch::Deferred | EntryPatch::Loading => "Loading diff…".into(),
        EntryPatch::Binary => "Binary file · no text diff".into(),
        EntryPatch::Unavailable(message) => message.clone(),
        EntryPatch::Parsed(_) => "No text changes in this comparison".into(),
    };
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(Vec2::new(14.0, 0.0)))
            .layout(Layout::left_to_right(Align::Center))
            .id_salt(("change-review-note", index)),
    );
    child.add(egui::Label::new(RichText::new(text).size(12.0).color(MUTED)).truncate());
    matches!(patch, EntryPatch::Large)
        && design::ghost_button(&mut child, "Load", "Load this file's complete diff").clicked()
}

struct HeaderResponse {
    open: bool,
    seen: Option<bool>,
}

fn file_header(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    entry: &ReviewEntry,
    seen: bool,
    keyboard_current: bool,
    bindings: &keybindings::Bindings,
) -> HeaderResponse {
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, PANEL);
    painter.line_segment(
        [rect.left_top(), rect.right_top()],
        Stroke::new(1.0, BORDER),
    );
    painter.line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1.0, BORDER),
    );
    if keyboard_current {
        painter.rect_filled(
            egui::Rect::from_min_size(rect.min, Vec2::new(2.0, rect.height())),
            0.0,
            ACCENT,
        );
    }
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(Vec2::new(12.0, 0.0)))
            .layout(Layout::right_to_left(Align::Center))
            .id_salt(("change-review-file-header", &entry.path)),
    );
    let ui = &mut child;
    ui.spacing_mut().item_spacing.x = 10.0;
    let mut checked = seen;
    let toggle = ui
        .add(egui::Checkbox::new(
            &mut checked,
            RichText::new("Seen").size(12.0).color(MUTED),
        ))
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(review_hint(
            bindings,
            keybindings::Action::ReviewSeenNext,
            "Mark the current file seen and continue to the next unseen file",
        ));
    toggle.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::Checkbox,
            true,
            checked,
            format!("Seen {}", entry.path),
        )
    });
    let deleted = entry.status == FileChange::Deleted;
    let open = ui
        .add_enabled_ui(!deleted, |ui| {
            design::ghost_button(
                ui,
                "Open",
                &review_hint(
                    bindings,
                    keybindings::Action::ReviewOpenFile,
                    "Open at the line on screen; Enter also opens the file",
                ),
            )
        })
        .inner;
    open.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            !deleted,
            format!("Open {}", entry.path),
        )
    });
    if let Some(stats) = entry.stats.filter(|stats| !stats.binary) {
        ui.label(
            RichText::new(format!("−{}", stats.removed))
                .monospace()
                .size(11.0)
                .color(RED),
        );
        ui.label(
            RichText::new(format!("+{}", stats.added))
                .monospace()
                .size(11.0)
                .color(GREEN),
        );
    }
    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let (mark, color, status) = git_panel::change_marker(entry.status, false);
        ui.label(RichText::new(mark).monospace().size(11.0).color(color))
            .on_hover_text(status);
        let folder = entry.folder();
        let mut job = egui::text::LayoutJob::default();
        if !folder.is_empty() {
            job.append(
                &format!("{folder}/"),
                0.0,
                egui::TextFormat::simple(FontId::proportional(12.5), MUTED),
            );
        }
        job.append(
            entry.name(),
            0.0,
            egui::TextFormat::simple(FontId::proportional(12.5), TEXT),
        );
        if let Some(original) = &entry.original_path {
            job.append(
                &format!("  ← {original}"),
                0.0,
                egui::TextFormat::simple(FontId::proportional(11.5), MUTED),
            );
        }
        ui.add(egui::Label::new(job).truncate())
            .on_hover_text(match &entry.original_path {
                Some(original) => format!("{}\nRenamed from {original}", entry.path),
                None => entry.path.clone(),
            });
    });
    HeaderResponse {
        open: open.clicked(),
        seen: toggle.changed().then_some(checked),
    }
}

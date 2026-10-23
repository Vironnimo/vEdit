//! Two hover contracts: egui's ordinary hints and inspectable details.
//!
//! Keep interaction here: callers supply text or a rich body and its copy value.
//! A details card owns keyboard input only after the user clicks inside it.
use eframe::egui::{self, Context, Event, Id, Rect, Response, Ui};
use std::sync::Arc;

const LEAVE_DELAY: f64 = 0.22;

#[derive(Clone)]
struct Open {
    id: Id,
    anchor: Rect,
    rect: Rect,
    pointer: egui::Pos2,
    pass: u64,
    until: f64,
    interacting: bool,
    explicit: bool,
    text: Option<Arc<str>>,
    restore_focus: Option<Id>,
}

#[derive(Clone, Default)]
struct State {
    open: Option<Open>,
    dismissed: Option<(Id, Rect)>,
    input: Vec<Event>,
}

fn state_id() -> Id {
    Id::new("vedit_hover_details")
}

fn state(ctx: &Context) -> State {
    ctx.data(|data| data.get_temp(state_id()))
        .unwrap_or_default()
}

fn save(ctx: &Context, state: State) {
    ctx.data_mut(|data| data.insert_temp(state_id(), state));
}

fn keyboard(event: &Event) -> bool {
    matches!(
        event,
        Event::Key { .. }
            | Event::Text(_)
            | Event::Copy
            | Event::Cut
            | Event::Paste(_)
            | Event::Ime(_)
    )
}

/// Before app shortcuts/source input. Hovering alone never takes keyboard input.
pub(crate) fn begin_hover_frame(ctx: &Context) {
    let mut state = state(ctx);
    state.input.clear();
    let (pos, pressed, down, escape, now) = ctx.input(|i| {
        (
            i.pointer.hover_pos(),
            i.pointer.any_pressed(),
            i.pointer.any_down(),
            i.key_pressed(egui::Key::Escape),
            i.time,
        )
    });
    if state
        .dismissed
        .is_some_and(|(_, anchor)| pos.is_none_or(|p| !anchor.contains(p)))
    {
        state.dismissed = None;
    }
    if let Some(open) = &mut state.open {
        let inside = pos.is_some_and(|p| open.rect.contains(p));
        let at_source = pos.is_some_and(|p| open.anchor.contains(p));
        if inside || at_source {
            open.until = now + LEAVE_DELAY;
        }
        if inside && pressed && !open.interacting {
            open.restore_focus = ctx.memory(|memory| memory.focused());
            open.interacting = true;
        }
        let stale = open.pass + 1 < ctx.cumulative_pass_nr();
        let outside_click = pressed && !inside;
        let left = !open.explicit && now > open.until && !(open.interacting && down);
        if escape || outside_click || stale || left {
            if !outside_click && let Some(id) = open.restore_focus {
                ctx.memory_mut(|memory| memory.request_focus(id));
            }
            if escape && open.interacting {
                // A batch containing text followed by Escape must not edit the source.
                ctx.input_mut(|i| i.events.retain(|e| !keyboard(e)));
            }
            if escape || outside_click {
                state.dismissed = Some((open.id, open.anchor));
            }
            state.open = None;
        } else if open.interacting {
            ctx.input_mut(|i| {
                i.events.retain(|event| {
                    if keyboard(event) {
                        state.input.push(event.clone());
                        false
                    } else {
                        true
                    }
                })
            });
        }
    }
    save(ctx, state);
}

pub(crate) fn end_hover_frame(ctx: &Context) {
    let mut state = state(ctx);
    state.input.clear();
    if state
        .open
        .as_ref()
        .is_some_and(|open| open.pass != ctx.cumulative_pass_nr())
    {
        state.open = None;
    }
    save(ctx, state);
}

pub(crate) fn details_dismissed(ctx: &Context, id: Id) -> bool {
    state(ctx).dismissed.is_some_and(|(closed, _)| closed == id)
}

pub(crate) fn details_open(ctx: &Context, id: Id) -> bool {
    state(ctx).open.is_some_and(|open| open.id == id)
}

/// Register a custom anchored card (e.g. an asynchronous language hover).
pub(crate) fn register_details(ctx: &Context, id: Id, anchor: Rect, rect: Rect, explicit: bool) {
    let mut state = state(ctx);
    let now = ctx.input(|i| i.time);
    let mut open = state.open.filter(|open| open.id == id).unwrap_or(Open {
        id,
        anchor,
        rect,
        pointer: ctx.pointer_hover_pos().unwrap_or(anchor.center()),
        pass: ctx.cumulative_pass_nr(),
        until: now + LEAVE_DELAY,
        interacting: false,
        explicit,
        text: None,
        restore_focus: None,
    });
    open.anchor = anchor;
    open.rect = rect;
    open.pass = ctx.cumulative_pass_nr();
    state.open = Some(open);
    ctx.request_repaint_after_secs(LEAVE_DELAY as f32);
    save(ctx, state);
}

/// Shared bounded body for plain and rich details. The returned string is the
/// complete copy value, independent of wrapping, selection and scroll position.
pub(crate) fn details_body(ui: &mut Ui, id: Id, contents: impl FnOnce(&mut Ui) -> String) {
    let ctx = ui.ctx().clone();
    let pending = ctx.data_mut(|data| {
        let mut state = data.get_temp::<State>(state_id()).unwrap_or_default();
        let input = if state.open.as_ref().is_some_and(|open| open.id == id) {
            std::mem::take(&mut state.input)
        } else {
            Vec::new()
        };
        data.insert_temp(state_id(), state);
        input
    });
    let owns_input = state(&ctx)
        .open
        .is_some_and(|open| open.id == id && open.interacting);
    // A label selection can outlive its popup in egui. Do not let a merely
    // hovered card copy or alter that old selection using the editor's keys.
    let source_input = (!owns_input).then(|| {
        ctx.input_mut(|i| {
            let events = i.events.clone();
            i.events.retain(|e| !keyboard(e));
            events
        })
    });
    ctx.input_mut(|i| i.events.extend(pending));
    ui.style_mut().interaction.selectable_labels = true;
    let width = (ctx.content_rect().width() - 32.0).clamp(100.0, 480.0);
    ui.set_max_width(width);
    let height = (ctx.content_rect().height() * 0.55).clamp(50.0, 360.0);
    // Areas remember their previous size. Give the scroll view room to grow
    // when background results replace a shorter loading message.
    ui.set_max_height(height + 48.0);
    let copy = ui
        .horizontal_wrapped(|ui| {
            let copy = ui.small_button("Copy all").clicked();
            ui.add(
                egui::Label::new(
                    egui::RichText::new("Select text · Ctrl+C · Esc to close")
                        .small()
                        .weak(),
                )
                .selectable(false),
            );
            copy
        })
        .inner;
    ui.separator();
    let text = egui::ScrollArea::vertical()
        .id_salt(id)
        .max_height(height)
        .auto_shrink([true, true])
        .show(ui, contents)
        .inner;
    if copy {
        ctx.copy_text(text);
    }
    if owns_input {
        // The source may be drawn after this card in the same frame.
        ctx.input_mut(|i| i.events.retain(|event| !keyboard(event)));
    } else if let Some(events) = source_input {
        ctx.input_mut(|i| i.events = events);
    }
}

pub(crate) trait HoverDetails: Sized {
    fn on_hover_details(self, text: impl AsRef<str>) -> Self;
    fn on_hover_details_ui(self, contents: impl FnOnce(&mut Ui) -> String) -> Self;
    fn on_hover_details_at_pointer(self, text: impl AsRef<str>) -> Self;
}

impl HoverDetails for Response {
    fn on_hover_details(self, text: impl AsRef<str>) -> Self {
        show_text(self, text.as_ref(), false)
    }
    fn on_hover_details_at_pointer(self, text: impl AsRef<str>) -> Self {
        show_text(self, text.as_ref(), true)
    }
    fn on_hover_details_ui(self, contents: impl FnOnce(&mut Ui) -> String) -> Self {
        show(self, false, contents)
    }
}

fn show_text(response: Response, text: &str, beside_pointer: bool) -> Response {
    let ctx = response.ctx.clone();
    let id = response.id;
    if text.is_empty() && !details_open(&ctx, id) {
        return response;
    }
    if text.is_empty()
        && let Some(open) = state(&ctx).open.filter(|open| open.id == id)
        && ctx
            .pointer_hover_pos()
            .is_some_and(|p| open.anchor.contains(p) && !open.rect.contains(p))
    {
        let mut current = state(&ctx);
        current.open = None;
        save(&ctx, current);
        return response;
    }
    show(response, beside_pointer, |ui| {
        let mut state = state(&ctx);
        let text = if let Some(open) = state.open.as_mut().filter(|open| open.id == id) {
            if !open.interacting
                && !text.is_empty()
                && ctx
                    .pointer_hover_pos()
                    .is_some_and(|p| open.anchor.contains(p) && !open.rect.contains(p))
            {
                open.text = Some(Arc::from(text));
            }
            open.text.get_or_insert_with(|| Arc::from(text)).clone()
        } else {
            Arc::from(text)
        };
        save(&ctx, state);
        ui.add(egui::Label::new(text.as_ref()).wrap().selectable(true));
        text.to_string()
    })
}

fn show(
    response: Response,
    beside_pointer: bool,
    contents: impl FnOnce(&mut Ui) -> String,
) -> Response {
    let ctx = &response.ctx;
    let mut current = state(ctx);
    if details_dismissed(ctx, response.id) {
        return response;
    }
    if current
        .open
        .as_ref()
        .is_some_and(|open| open.id != response.id)
    {
        return response;
    }
    if current
        .open
        .as_ref()
        .is_some_and(|open| open.anchor != response.rect)
    {
        current.open = None;
        save(ctx, current);
        return response;
    }
    let was_open = details_open(ctx, response.id);
    if !was_open && !egui::Tooltip::should_show_tooltip(&response, true) {
        return response;
    }
    if !response.enabled() {
        return response;
    }
    register_details(ctx, response.id, response.rect, Rect::NOTHING, false);
    let mut tooltip = egui::Tooltip::for_widget(&response)
        .width((ctx.content_rect().width() - 32.0).clamp(100.0, 480.0));
    if beside_pointer {
        let pointer = state(ctx).open.unwrap().pointer;
        tooltip.popup = tooltip
            .popup
            .anchor(pointer)
            .gap(12.0)
            .align(egui::emath::RectAlign::RIGHT)
            .align_alternatives(&[egui::emath::RectAlign::LEFT]);
    }
    if let Some(shown) = tooltip.show(|ui| details_body(ui, response.id, contents)) {
        register_details(ctx, response.id, response.rect, shown.response.rect, false);
    }
    response
}

#[cfg(test)]
#[path = "design_hover_tests.rs"]
mod tests;

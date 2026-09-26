//! Edge scrolling belongs to the text widget that captured the selection drag.
use eframe::egui::{self, Rect, Vec2};

const EDGE_BAND: f32 = 28.0;
const MAX_SPEED: f32 = 1600.0;

/// egui has no public stop-animation method. Its persisted State representation
/// lets us clear the old target once, without resetting scrollbar geometry.
pub(crate) fn begin_drag(ctx: &egui::Context, scroll_id: egui::Id, active: bool) {
    let first = ctx.data_mut(|data| {
        let key = scroll_id.with("selection_drag_active");
        let previous = data.get_temp::<bool>(key).unwrap_or(false);
        data.insert_temp(key, active);
        active && !previous
    });
    if first && let Some(state) = egui::scroll_area::State::load(ctx, scroll_id) {
        let cleared = serde_json::to_value(state).and_then(|mut value| {
            value["offset_target"] = serde_json::json!([null, null]);
            serde_json::from_value::<egui::scroll_area::State>(value)
        });
        if let Ok(state) = cleared {
            state.store(ctx, scroll_id);
        }
    }
}

fn axis_speed(pointer: f32, start: f32, end: f32) -> f32 {
    let band = EDGE_BAND.min((end - start).max(0.0) * 0.25);
    if band <= 0.0 || !pointer.is_finite() {
        return 0.0;
    }
    let penetration = if pointer < start + band {
        -((start + band - pointer) / band).min(3.0)
    } else if pointer > end - band {
        ((pointer - end + band) / band).min(3.0)
    } else {
        0.0
    };
    penetration.signum() * (240.0 * penetration * penetration).min(MAX_SPEED)
}

/// No selection rewriting: TextEdit resamples its captured pointer against the
/// shifted galley next frame, including when the pointer is held stationary.
pub(crate) fn scroll_selection(
    ui: &egui::Ui,
    response: &egui::Response,
    scroll_id: egui::Id,
    offset: Vec2,
    horizontal: bool,
    enabled: bool,
) {
    if !enabled || !response.dragged_by(egui::PointerButton::Primary) {
        return;
    }
    let Some((pointer, dt)) = ui.input(|input| {
        (input.focused && input.pointer.primary_down())
            .then(|| {
                input
                    .pointer
                    .latest_pos()
                    .map(|point| (point, input.stable_dt))
            })
            .flatten()
    }) else {
        return;
    };
    let viewport: Rect = ui.clip_rect().intersect(response.rect);
    if !viewport.is_positive() {
        return;
    }
    let speed = Vec2::new(
        if horizontal {
            axis_speed(pointer.x, viewport.left(), viewport.right())
        } else {
            0.0
        },
        axis_speed(pointer.y, viewport.top(), viewport.bottom()),
    );
    let limit = ui.ctx().data(|data| {
        data.get_temp::<Vec2>(scroll_id.with("document_scroll_limit"))
            .unwrap_or(Vec2::ZERO)
    });
    let delta = (offset + speed * dt.clamp(0.0, 0.05))
        .max(Vec2::ZERO)
        .min(limit)
        - offset;
    if delta != Vec2::ZERO {
        // Drag scrolling follows the pointer directly, independently of the
        // user's animation preference for ordinary navigation and wheel input.
        ui.scroll_with_delta_animation(-delta, egui::style::ScrollAnimation::none());
        ui.ctx().request_repaint();
    }
}

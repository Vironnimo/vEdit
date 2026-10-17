//! The app's single title bar, with separate interactive and native drag regions.
//!
//! Geometry stays in logical client coordinates. The native integration converts
//! it with the current viewport's pixels per point when publishing hit regions.

use eframe::egui::{
    self, Align, Align2, Color32, FontId, Layout, PointerButton, Rect, Response, Sense, Stroke,
    StrokeKind, Ui, UiBuilder, Vec2, ViewportCommand, WidgetInfo, WidgetType,
};

pub(crate) const HEIGHT: f32 = 42.0;
const CONTROL_WIDTH: f32 = 46.0;
const PROJECT_PADDING: f32 = 10.0;
const MIN_PROJECT_WIDTH: f32 = 28.0;

#[derive(Clone, Copy, Debug)]
pub(crate) struct ChromeOptions {
    /// Additional app state that blocks dragging, including dialogs opened later
    /// in the frame. Egui's existing popup and modal state is also checked here.
    pub interaction_allowed: bool,
    /// Windows handles published drag/maximize regions through native hit tests.
    /// Other platforms and headless tests use viewport commands instead.
    pub native_titlebar: bool,
    /// Native nonclient mouse events do not reach egui's normal button response.
    pub native_maximize_state: (bool, bool),
}

impl Default for ChromeOptions {
    fn default() -> Self {
        Self {
            interaction_allowed: true,
            native_titlebar: false,
            native_maximize_state: (false, false),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ChromeGeometry {
    #[cfg(test)]
    pub bar: Rect,
    /// These rectangles contain no project widgets or window controls.
    pub drag_regions: Vec<Rect>,
    #[cfg(test)]
    pub project: Rect,
    #[cfg(test)]
    pub minimize: Rect,
    #[cfg(test)]
    pub maximize: Rect,
    #[cfg(test)]
    pub close: Rect,
    /// With native hit testing, this region enables Windows' Snap Layout menu.
    /// It is removed together with the drag regions while a popup/modal is open.
    pub native_maximize: Option<Rect>,
    /// The caller must enter its existing dirty-document close flow immediately.
    pub close_requested: bool,
}

#[cfg(test)]
impl Default for ChromeGeometry {
    fn default() -> Self {
        Self {
            #[cfg(test)]
            bar: Rect::NOTHING,
            drag_regions: Vec::new(),
            #[cfg(test)]
            project: Rect::NOTHING,
            #[cfg(test)]
            minimize: Rect::NOTHING,
            #[cfg(test)]
            maximize: Rect::NOTHING,
            #[cfg(test)]
            close: Rect::NOTHING,
            native_maximize: None,
            close_requested: false,
        }
    }
}

/// Draw inside a top panel without inner margins and with `HEIGHT` as its exact
/// height. The project closure receives the space available for project tabs.
pub(crate) fn draw_top_bar(
    ui: &mut Ui,
    options: ChromeOptions,
    project_contents: impl FnOnce(&mut Ui),
) -> ChromeGeometry {
    let width = ui.available_width().max(0.0);
    let (_, bar) = ui.allocate_space(Vec2::new(width, HEIGHT));
    let controls_width = CONTROL_WIDTH.min(width / 3.0);
    let controls_left = bar.right() - controls_width * 3.0;
    let control_rect = |index: f32| {
        Rect::from_min_size(
            egui::pos2(controls_left + controls_width * index, bar.top()),
            Vec2::new(controls_width, HEIGHT),
        )
    };
    let minimize = control_rect(0.0);
    let maximize = control_rect(1.0);
    let close = control_rect(2.0);

    ui.painter().rect_filled(bar, 0.0, ui.visuals().panel_fill);
    let border = ui.visuals().widgets.noninteractive.bg_stroke;
    ui.painter().hline(bar.x_range(), bar.bottom(), border);

    let brand_width = if width >= 480.0 { 122.0 } else { 44.0 };
    let brand_right = (bar.left() + brand_width).min(controls_left);
    let brand_rect = Rect::from_min_max(bar.min, egui::pos2(brand_right, bar.bottom()));
    paint_brand(ui, brand_rect, width >= 480.0);

    // Let project tabs use the available width while preserving a dedicated drag
    // target beside them. At high UI zoom, keep room for a project-list button
    // before reserving free space; the brand also remains a native drag target.
    // Unused project space stays draggable, excluding all occupied widgets.
    let available_project_width = controls_left - brand_right;
    let project_padding = (available_project_width - MIN_PROJECT_WIDTH).clamp(0.0, PROJECT_PADDING);
    let free_drag_width =
        (available_project_width - project_padding - MIN_PROJECT_WIDTH).clamp(0.0, 80.0);
    let project_width = (available_project_width - free_drag_width - project_padding).max(0.0);
    let mut project = Rect::from_min_max(
        egui::pos2(brand_right, bar.top()),
        egui::pos2(brand_right, bar.bottom()),
    );
    if project_width >= MIN_PROJECT_WIDTH {
        let slot = Rect::from_min_size(
            egui::pos2(brand_right, bar.top()),
            Vec2::new(project_width, HEIGHT),
        );
        let mut project_ui = ui.new_child(
            UiBuilder::new()
                .id_salt("window_project")
                .max_rect(slot)
                .layout(Layout::left_to_right(Align::Center)),
        );
        project_ui.set_clip_rect(ui.clip_rect().intersect(slot));
        project_contents(&mut project_ui);
        let used_right = project_ui
            .min_rect()
            .right()
            .clamp(slot.left(), slot.right());
        project.max.x = (used_right + project_padding).min(controls_left);
    }

    // Check after drawing project widgets so a popup opened by that click cannot
    // export a native drag rectangle in this frame.
    let interaction_allowed = options.interaction_allowed
        && ui.is_enabled()
        && !egui::Popup::is_any_open(ui.ctx())
        && ui.memory(|memory| memory.allows_interaction(ui.layer_id()));
    let mut drag_regions = Vec::with_capacity(2);
    if interaction_allowed {
        for region in [
            brand_rect,
            Rect::from_min_max(
                egui::pos2(project.right(), bar.top()),
                egui::pos2(controls_left, bar.bottom()),
            ),
        ] {
            if region.width() > 0.0 {
                drag_regions.push(region);
            }
        }
    }
    let native_maximize = (interaction_allowed && options.native_titlebar).then_some(maximize);
    let maximized = ui.input(|input| input.viewport().maximized.unwrap_or(false));

    // A drag area has no keyboard focus: Tab moves between the project widgets
    // and the three window controls, without stopping on empty chrome.
    if !options.native_titlebar {
        for (index, region) in drag_regions.iter().enumerate() {
            let response = ui.interact(
                *region,
                ui.id().with(("window_drag", index)),
                Sense::CLICK | Sense::DRAG,
            );
            if response.double_clicked_by(PointerButton::Primary) {
                ui.ctx()
                    .send_viewport_cmd(ViewportCommand::Maximized(!maximized));
            } else if response.drag_started_by(PointerButton::Primary) {
                ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
            }
        }
    }

    let minimize_response = window_control(ui, minimize, Control::Minimize, (false, false));
    let maximize_response = window_control(
        ui,
        maximize,
        if maximized {
            Control::Restore
        } else {
            Control::Maximize
        },
        if native_maximize.is_some() {
            options.native_maximize_state
        } else {
            (false, false)
        },
    );
    let close_response = window_control(ui, close, Control::Close, (false, false));
    if minimize_response.clicked() {
        ui.ctx().send_viewport_cmd(ViewportCommand::Minimized(true));
    }
    // Preserve keyboard and AccessKit activation when native mouse hit testing
    // owns the maximize button, without issuing a second maximize on pointer up.
    if maximize_response.clicked()
        && (native_maximize.is_none() || !maximize_response.clicked_by(PointerButton::Primary))
    {
        ui.ctx()
            .send_viewport_cmd(ViewportCommand::Maximized(!maximized));
    }

    ChromeGeometry {
        #[cfg(test)]
        bar,
        drag_regions,
        #[cfg(test)]
        project,
        #[cfg(test)]
        minimize,
        #[cfg(test)]
        maximize,
        #[cfg(test)]
        close,
        native_maximize,
        close_requested: close_response.clicked(),
    }
}

fn paint_brand(ui: &Ui, rect: Rect, text: bool) {
    if rect.width() < 30.0 {
        return;
    }
    let center = egui::pos2(rect.left() + 23.0, rect.center().y);
    let accent = ui.visuals().selection.stroke.color;
    let diamond = |radius: f32| {
        vec![
            center + Vec2::new(0.0, -radius),
            center + Vec2::new(radius, 0.0),
            center + Vec2::new(0.0, radius),
            center + Vec2::new(-radius, 0.0),
        ]
    };
    ui.painter().add(egui::Shape::convex_polygon(
        diamond(9.0),
        Color32::TRANSPARENT,
        Stroke::new(1.4, accent),
    ));
    ui.painter().add(egui::Shape::convex_polygon(
        diamond(2.6),
        accent,
        Stroke::NONE,
    ));
    if text {
        ui.painter().text(
            egui::pos2(rect.left() + 43.0, rect.center().y),
            Align2::LEFT_CENTER,
            "vEdit",
            FontId::proportional(15.0),
            ui.visuals().text_color(),
        );
    }
}

#[derive(Clone, Copy)]
enum Control {
    Minimize,
    Maximize,
    Restore,
    Close,
}

impl Control {
    fn label(self) -> &'static str {
        match self {
            Self::Minimize => "Minimize window",
            Self::Maximize => "Maximize window",
            Self::Restore => "Restore window",
            Self::Close => "Close window",
        }
    }

    fn id(self) -> &'static str {
        match self {
            Self::Minimize => "window_minimize",
            Self::Maximize | Self::Restore => "window_maximize",
            Self::Close => "window_close",
        }
    }
}

fn window_control(
    ui: &mut Ui,
    rect: Rect,
    control: Control,
    native_state: (bool, bool),
) -> Response {
    let response = ui.interact(rect, ui.id().with(control.id()), Sense::click());
    response
        .widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), control.label()));
    let (native_hovered, native_pressed) = native_state;
    let hovered = response.hovered() || native_hovered;
    let pressed = response.is_pointer_button_down_on() || native_pressed;
    let visuals = ui.style().interact(&response);
    let close_highlight = matches!(control, Control::Close) && (hovered || pressed);
    if hovered || pressed {
        let fill = if close_highlight {
            ui.visuals().error_fg_color
        } else if pressed {
            ui.visuals().widgets.active.weak_bg_fill
        } else {
            ui.visuals().widgets.hovered.weak_bg_fill
        };
        ui.painter().rect_filled(rect, 0.0, fill);
    }
    if crate::design::keyboard_focus(&response) {
        ui.painter().rect_stroke(
            rect.shrink(3.0),
            3.0,
            ui.visuals().selection.stroke,
            StrokeKind::Inside,
        );
    }
    let icon_color = if close_highlight {
        Color32::WHITE
    } else {
        visuals.fg_stroke.color
    };
    paint_control(ui, rect, control, Stroke::new(1.15, icon_color));
    // Windows sends nonclient hover to its Snap Layout handler, rather than to
    // egui. Use the same tooltip delay without opening an interactive popup.
    let hover_id = response.id.with("native_hover_since");
    if native_hovered && !native_pressed && !response.hovered() && ui.is_enabled() {
        let now = ui.input(|input| input.time);
        let since = ui
            .ctx()
            .data_mut(|data| *data.get_temp_mut_or_insert_with(hover_id, || now));
        let remaining = f64::from(ui.style().interaction.tooltip_delay) - (now - since);
        if remaining <= 0.0 {
            egui::Tooltip::for_widget(&response).show(|ui| {
                ui.label(control.label());
            });
        } else {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs_f64(remaining));
        }
    } else {
        ui.ctx().data_mut(|data| data.remove::<f64>(hover_id));
    }
    response.on_hover_text(control.label())
}

fn paint_control(ui: &Ui, rect: Rect, control: Control, stroke: Stroke) {
    let center = rect.center();
    let size = 10.0_f32.min(rect.width() * 0.5);
    let icon = Rect::from_center_size(center, Vec2::splat(size));
    let painter = ui.painter();
    match control {
        Control::Minimize => {
            painter.line_segment(
                [
                    egui::pos2(icon.left(), center.y),
                    egui::pos2(icon.right(), center.y),
                ],
                stroke,
            );
        }
        Control::Maximize => {
            painter.rect_stroke(icon, 0.0, stroke, StrokeKind::Inside);
        }
        Control::Restore => {
            let back = icon.translate(Vec2::new(1.5, -1.5)).shrink(1.0);
            let front = icon.translate(Vec2::new(-1.5, 1.5)).shrink(1.0);
            // Draw only the exposed part of the rear window, keeping the front
            // glyph transparent over both resting and hovered backgrounds.
            painter.line_segment([back.left_top(), back.right_top()], stroke);
            painter.line_segment([back.right_top(), back.right_bottom()], stroke);
            painter.line_segment(
                [back.left_top(), egui::pos2(back.left(), front.top())],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(front.right(), back.bottom()),
                    back.right_bottom(),
                ],
                stroke,
            );
            painter.rect_stroke(front, 0.0, stroke, StrokeKind::Inside);
        }
        Control::Close => {
            painter.line_segment([icon.left_top(), icon.right_bottom()], stroke);
            painter.line_segment([icon.right_top(), icon.left_bottom()], stroke);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};

    #[derive(Default)]
    struct Fixture {
        options: ChromeOptions,
        geometry: ChromeGeometry,
        project_clicks: usize,
        close_requests: usize,
        project_popup: bool,
        modal: bool,
        fill_project_slot: bool,
        project_slot: Option<Rect>,
    }

    fn harness(width: f32, pixels_per_point: f32, fixture: Fixture) -> Harness<'static, Fixture> {
        let mut screen = Harness::builder()
            // Kittest wraps UI fixtures in an 8pt outer frame. Compensate so
            // `width` represents the app's actual, margin-free title-bar width.
            .with_size([width + 16.0, 216.0])
            .with_pixels_per_point(pixels_per_point)
            // Harness defaults to 250 ms per event frame; a three-event click
            // then takes longer than egui's 300 ms double-click interval.
            .with_step_dt(1.0 / 60.0)
            .with_os(egui::os::OperatingSystem::Windows)
            .build_ui_state(
                |ui, state: &mut Fixture| {
                    if state.modal {
                        egui::Modal::new(egui::Id::new("chrome_test_modal"))
                            .show(ui.ctx(), |ui| ui.label("Document dialog"));
                    }
                    egui::Panel::top("chrome_test")
                        .frame(egui::Frame::NONE)
                        .exact_size(HEIGHT)
                        .show(ui, |ui| {
                            state.geometry = draw_top_bar(ui, state.options, |ui| {
                                state.project_slot = Some(ui.max_rect());
                                if state.fill_project_slot {
                                    let _ = ui.add_sized(
                                        [ui.available_width(), 28.0],
                                        egui::Button::new("Project tabs"),
                                    );
                                    return;
                                }
                                let project = if ui.available_width() < 80.0 {
                                    let response =
                                        ui.add_sized([28.0, 28.0], egui::Button::new("≡"));
                                    response.widget_info(|| {
                                        WidgetInfo::labeled(
                                            WidgetType::Button,
                                            true,
                                            "Project list",
                                        )
                                    });
                                    response
                                } else {
                                    ui.add_sized([110.0, 28.0], egui::Button::new("Project picker"))
                                };
                                if project.clicked() {
                                    state.project_clicks += 1;
                                }
                                if state.project_popup {
                                    // Popup::menu already toggles on its anchor's
                                    // click. Opening it manually first closes it.
                                    egui::Popup::menu(&project).show(|ui| {
                                        let _ = ui.button("Project menu item");
                                    });
                                }
                            });
                            if state.geometry.close_requested {
                                state.close_requests += 1;
                            }
                        });
                },
                fixture,
            );
        screen.run_steps(2);
        screen
    }

    fn commands<'a>(screen: &'a Harness<'_, Fixture>) -> &'a [ViewportCommand] {
        &screen.output().viewport_output[&egui::ViewportId::ROOT].commands
    }

    fn event(screen: &mut Harness<'_, Fixture>, event: egui::Event) -> Vec<ViewportCommand> {
        screen.input_mut().events.push(event);
        screen.run_steps(1);
        commands(screen).to_vec()
    }

    fn click(screen: &mut Harness<'_, Fixture>, position: egui::Pos2) -> Vec<ViewportCommand> {
        let mut output = event(screen, egui::Event::PointerMoved(position));
        for pressed in [true, false] {
            output.extend(event(
                screen,
                egui::Event::PointerButton {
                    pos: position,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ));
        }
        output
    }

    fn drag(screen: &mut Harness<'_, Fixture>, position: egui::Pos2) -> Vec<ViewportCommand> {
        let mut output = event(screen, egui::Event::PointerMoved(position));
        output.extend(event(
            screen,
            egui::Event::PointerButton {
                pos: position,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ));
        output.extend(event(
            screen,
            egui::Event::PointerMoved(position + Vec2::new(18.0, 0.0)),
        ));
        output.extend(event(
            screen,
            egui::Event::PointerButton {
                pos: position + Vec2::new(18.0, 0.0),
                button: PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ));
        output
    }

    #[test]
    fn controls_are_accessible_and_close_enters_the_callers_guard() {
        let mut screen = harness(960.0, 1.0, Fixture::default());
        let minimize = screen.get_by_label("Minimize window").rect().center();
        assert!(click(&mut screen, minimize).contains(&ViewportCommand::Minimized(true)));

        let maximize = screen.get_by_label("Maximize window").rect().center();
        assert!(click(&mut screen, maximize).contains(&ViewportCommand::Maximized(true)));
        screen
            .input_mut()
            .viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .maximized = Some(true);
        screen.run_steps(1);
        let restore = screen.get_by_label("Restore window").rect().center();
        assert!(click(&mut screen, restore).contains(&ViewportCommand::Maximized(false)));

        let close = screen.get_by_label("Close window").rect().center();
        let output = click(&mut screen, close);
        assert_eq!(screen.state().close_requests, 1);
        assert!(!output.contains(&ViewportCommand::Close));
        assert!(!output.contains(&ViewportCommand::StartDrag));
    }

    #[test]
    fn free_space_drags_and_double_clicks_but_project_widgets_do_neither() {
        let mut screen = harness(960.0, 1.0, Fixture::default());
        let project = screen.get_by_label("Project picker").rect().center();
        let mut output = click(&mut screen, project);
        output.extend(click(&mut screen, project));
        output.extend(drag(&mut screen, project));
        assert_eq!(screen.state().project_clicks, 2);
        assert!(!output.contains(&ViewportCommand::StartDrag));
        assert!(
            !output
                .iter()
                .any(|command| matches!(command, ViewportCommand::Maximized(_)))
        );

        let free = screen.state().geometry.drag_regions[1].center();
        assert!(drag(&mut screen, free).contains(&ViewportCommand::StartDrag));
        // The earlier project clicks must leave egui's triple-click interval
        // before testing an independent title-bar double click.
        screen.run_steps(40);
        let mut output = click(&mut screen, free);
        output.extend(click(&mut screen, free));
        assert!(output.contains(&ViewportCommand::Maximized(true)));
    }

    #[test]
    fn project_popup_removes_native_regions_in_the_opening_frame() {
        for (width, scale, label) in [
            (960.0, 1.0, "Project picker"),
            (333.0, 1.5, "Project list"),
            (250.0, 2.0, "Project list"),
        ] {
            let mut screen = harness(
                width,
                scale,
                Fixture {
                    project_popup: true,
                    options: ChromeOptions {
                        native_titlebar: true,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            );
            assert!(screen.state().geometry.native_maximize.is_some());
            let project = screen.get_by_label(label).rect().center();
            let output = click(&mut screen, project);
            assert!(egui::Popup::is_any_open(&screen.ctx));
            assert!(screen.state().geometry.drag_regions.is_empty());
            assert!(screen.state().geometry.native_maximize.is_none());
            assert!(!output.contains(&ViewportCommand::StartDrag));
        }
    }

    #[test]
    fn modal_and_explicit_app_block_suppress_drag_and_double_click() {
        for modal in [false, true] {
            let mut screen = harness(
                960.0,
                1.0,
                Fixture {
                    modal,
                    options: ChromeOptions {
                        interaction_allowed: modal,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            );
            let free = egui::pos2(600.0, screen.state().geometry.bar.center().y);
            let mut output = click(&mut screen, free);
            output.extend(click(&mut screen, free));
            output.extend(drag(&mut screen, free));
            assert!(screen.state().geometry.drag_regions.is_empty());
            assert!(screen.state().geometry.native_maximize.is_none());
            assert!(!output.contains(&ViewportCommand::StartDrag));
            assert!(
                !output
                    .iter()
                    .any(|command| matches!(command, ViewportCommand::Maximized(_)))
            );
        }
    }

    #[test]
    fn native_regions_own_pointer_activation_without_losing_keyboard_or_accesskit() {
        let mut screen = harness(
            960.0,
            1.0,
            Fixture {
                options: ChromeOptions {
                    native_titlebar: true,
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        let free = screen.state().geometry.drag_regions[1].center();
        let maximize = screen.get_by_label("Maximize window").rect().center();
        let mut output = drag(&mut screen, free);
        output.extend(click(&mut screen, free));
        output.extend(click(&mut screen, free));
        output.extend(click(&mut screen, maximize));
        assert!(output.is_empty());

        screen.get_by_label("Maximize window").focus();
        screen.run_steps(1);
        let output = event(
            &mut screen,
            egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
        );
        assert!(output.contains(&ViewportCommand::Maximized(true)));
        screen.get_by_label("Maximize window").click_accesskit();
        screen.run_steps(1);
        assert!(commands(&screen).contains(&ViewportCommand::Maximized(true)));
    }

    #[test]
    fn small_and_scaled_windows_keep_all_window_controls_separate() {
        for (width, scale) in [(960.0, 1.0), (500.0, 1.5), (250.0, 2.0)] {
            let screen = harness(width, scale, Fixture::default());
            let geometry = &screen.state().geometry;
            assert_eq!(geometry.bar.height(), HEIGHT);
            assert_eq!(geometry.minimize.width(), CONTROL_WIDTH);
            assert_eq!(geometry.maximize.width(), CONTROL_WIDTH);
            assert_eq!(geometry.close.width(), CONTROL_WIDTH);
            assert_eq!(geometry.minimize.right(), geometry.maximize.left());
            assert_eq!(geometry.maximize.right(), geometry.close.left());
            assert_eq!(geometry.close.right(), geometry.bar.right());
            assert!(geometry.project.right() <= geometry.minimize.left());
            for drag in &geometry.drag_regions {
                assert!(!drag.intersect(geometry.project).is_positive());
                for control in [geometry.minimize, geometry.maximize, geometry.close] {
                    assert!(!drag.intersect(control).is_positive());
                }
            }
            assert_eq!(screen.get_by_label("Close window").rect(), geometry.close);
            if width == 250.0 {
                assert!(screen.query_by_label("Project picker").is_none());
                assert!(screen.query_by_label("Project list").is_some());
            }
        }
    }

    #[test]
    fn project_tabs_fill_available_space_and_preserve_a_real_drag_target() {
        for (width, scale, project_width, drag_width) in [
            (1440.0, 1.0, 1090.0, 80.0),
            (960.0, 1.25, 610.0, 80.0),
            (500.0, 1.5, 150.0, 80.0),
        ] {
            let mut screen = harness(
                width,
                scale,
                Fixture {
                    fill_project_slot: true,
                    ..Default::default()
                },
            );
            let geometry = screen.state().geometry.clone();
            let slot = screen.state().project_slot.unwrap();
            assert_eq!(geometry.bar.width(), width);
            assert_eq!(slot.width(), project_width);
            assert_eq!(geometry.project.right(), slot.right() + PROJECT_PADDING);
            assert_eq!(
                screen.get_by_label("Project tabs").rect().right(),
                slot.right()
            );
            assert_eq!(geometry.drag_regions.len(), 2);
            let brand = geometry.drag_regions[0];
            let free = geometry.drag_regions[1];
            assert_eq!(brand.right(), slot.left());
            assert_eq!(free.width(), drag_width);
            assert_eq!(free.right(), geometry.minimize.left());
            assert!(!free.intersect(geometry.project).is_positive());
            assert_eq!(screen.get_by_label("Close window").rect(), geometry.close);

            let tabs = screen.get_by_label("Project tabs").rect().center();
            let output = drag(&mut screen, tabs);
            assert!(!output.contains(&ViewportCommand::StartDrag));
            assert!(drag(&mut screen, free.center()).contains(&ViewportCommand::StartDrag));
            assert!(drag(&mut screen, brand.center()).contains(&ViewportCommand::StartDrag));
        }
    }

    #[test]
    fn narrow_zoomed_windows_keep_the_project_list_and_window_controls_reachable() {
        for (width, scale, project_width, free_width) in [
            (333.0, 1.5, 61.0, 80.0),
            (250.0, 2.0, 28.0, 30.0),
            (210.0, 2.0, 28.0, 0.0),
        ] {
            let mut screen = harness(width, scale, Fixture::default());
            let geometry = screen.state().geometry.clone();
            let slot = screen.state().project_slot.unwrap();
            let list = screen.get_by_label("Project list").rect();
            assert_eq!(geometry.bar.width(), width);
            assert!(list.is_finite());
            assert_eq!(list.width(), 28.0);
            assert_eq!(slot.width(), project_width);
            assert!(slot.contains_rect(list));
            assert_eq!(geometry.drag_regions[0].width(), 44.0);
            assert!(geometry.minimize.left() - geometry.project.right() >= free_width);
            assert_eq!(geometry.minimize.width(), CONTROL_WIDTH);
            assert_eq!(geometry.maximize.width(), CONTROL_WIDTH);
            assert_eq!(geometry.close.width(), CONTROL_WIDTH);
            assert_eq!(screen.get_by_label("Close window").rect(), geometry.close);
            for drag in &geometry.drag_regions {
                assert!(!drag.intersect(list).is_positive());
                for control in [geometry.minimize, geometry.maximize, geometry.close] {
                    assert!(!drag.intersect(control).is_positive());
                }
            }
            let output = click(&mut screen, list.center());
            assert_eq!(screen.state().project_clicks, 1);
            assert!(!output.contains(&ViewportCommand::StartDrag));
            let output = click(&mut screen, geometry.minimize.center());
            assert!(output.contains(&ViewportCommand::Minimized(true)));
        }
    }

    #[test]
    fn native_hover_tooltip_keeps_snap_and_drag_regions_available() {
        let mut screen = harness(
            960.0,
            1.0,
            Fixture {
                options: ChromeOptions {
                    native_titlebar: true,
                    native_maximize_state: (true, false),
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        screen
            .ctx
            .all_styles_mut(|style| style.interaction.tooltip_delay = 0.0);
        screen.run_steps(2);
        assert!(!egui::Popup::is_any_open(&screen.ctx));
        assert!(screen.state().geometry.native_maximize.is_some());
        assert!(!screen.state().geometry.drag_regions.is_empty());
    }
}

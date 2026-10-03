//! Pure geometry for a custom Windows titlebar. Coordinates remain physical
//! pixels; only Windows DPI transitions rescale previously published regions.

pub(crate) const CLIENT: i32 = 1;
pub(crate) const CAPTION: i32 = 2;
pub(crate) const MAX_BUTTON: i32 = 9;

#[derive(Default)]
pub(crate) struct CustomTitlebar {
    pub draggable: Vec<[i32; 4]>,
    pub maximize: Option<[i32; 4]>,
    pub geometry_scale: f64,
    pub maximize_hovered: bool,
    pub maximize_pressed: bool,
}

pub(crate) fn contains(rect: [i32; 4], point: [i32; 2]) -> bool {
    rect[0] <= point[0] && point[0] < rect[2] && rect[1] <= point[1] && point[1] < rect[3]
}

impl CustomTitlebar {
    pub fn begin_maximize_press(&mut self) {
        self.maximize_pressed = true;
        self.maximize_hovered = true;
    }

    pub fn release_maximize_press(
        &mut self,
        client_point: [i32; 2],
        scale: f64,
        enabled: bool,
    ) -> Option<bool> {
        if !std::mem::take(&mut self.maximize_pressed) {
            return None;
        }
        let clicked = enabled && self.hit_test(client_point, scale) == MAX_BUTTON;
        self.maximize_hovered = clicked;
        Some(clicked)
    }

    pub fn cancel_maximize_press(&mut self) -> bool {
        let pressed = std::mem::take(&mut self.maximize_pressed);
        self.maximize_hovered = false;
        pressed
    }

    pub fn hit_test(&self, client_point: [i32; 2], scale: f64) -> i32 {
        // The native DPI event can precede egui's next frame. Keep the freshly
        // resized titlebar usable until that frame publishes its new geometry.
        let scale = if self.geometry_scale > 0.0 && scale > 0.0 {
            self.geometry_scale / scale
        } else {
            1.0
        };
        let point = client_point.map(|coordinate| (coordinate as f64 * scale).floor() as i32);
        if self.maximize.is_some_and(|rect| contains(rect, point)) {
            MAX_BUTTON
        } else if self.draggable.iter().any(|&rect| contains(rect, point)) {
            CAPTION
        } else {
            CLIENT
        }
    }
}

/// Return the Windows HT* result for all eight resizing directions. Maximized
/// and fixed-size windows keep their content interactive right to the edge.
pub(crate) fn resize_hit_test(
    screen_point: [i32; 2],
    outer_rect: [i32; 4],
    border: [i32; 2],
    resizable: bool,
    maximized: bool,
) -> Option<i32> {
    if !resizable || maximized || !contains(outer_rect, screen_point) {
        return None;
    }
    let [x, y] = screen_point;
    let horizontal = border[0]
        .max(1)
        .min((outer_rect[2] - outer_rect[0]).max(2) / 2);
    let vertical = border[1]
        .max(1)
        .min((outer_rect[3] - outer_rect[1]).max(2) / 2);
    let left = x < outer_rect[0] + horizontal;
    let right = x >= outer_rect[2] - horizontal;
    let top = y < outer_rect[1] + vertical;
    let bottom = y >= outer_rect[3] - vertical;
    match (left, right, top, bottom) {
        (true, _, true, _) => Some(13), // HTTOPLEFT
        (_, true, true, _) => Some(14), // HTTOPRIGHT
        (true, _, _, true) => Some(16), // HTBOTTOMLEFT
        (_, true, _, true) => Some(17), // HTBOTTOMRIGHT
        (true, _, _, _) => Some(10),    // HTLEFT
        (_, true, _, _) => Some(11),    // HTRIGHT
        (_, _, true, _) => Some(12),    // HTTOP
        (_, _, _, true) => Some(15),    // HTBOTTOM
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_resize_edges_and_corners_work_on_negative_monitor_coordinates() {
        let window = [-1800, 300, -800, 1000];
        for (point, hit) in [
            ([-1799, 600], 10),
            ([-801, 600], 11),
            ([-1300, 301], 12),
            ([-1300, 999], 15),
            ([-1799, 301], 13),
            ([-801, 301], 14),
            ([-1799, 999], 16),
            ([-801, 999], 17),
        ] {
            assert_eq!(
                resize_hit_test(point, window, [8, 8], true, false),
                Some(hit)
            );
        }
        assert_eq!(
            resize_hit_test([-1300, 600], window, [8, 8], true, false),
            None
        );
        assert_eq!(
            resize_hit_test([-1801, 600], window, [8, 8], true, false),
            None
        );
    }

    #[test]
    fn resize_border_uses_physical_dpi_metrics_and_stops_when_maximized() {
        let window = [0, 0, 1200, 800];
        assert_eq!(resize_hit_test([9, 400], window, [8, 8], true, false), None);
        assert_eq!(
            resize_hit_test([9, 400], window, [10, 10], true, false),
            Some(10)
        );
        assert_eq!(resize_hit_test([1, 1], window, [10, 10], true, true), None);
        assert_eq!(
            resize_hit_test([1, 1], window, [10, 10], false, false),
            None
        );
    }

    #[test]
    fn titlebar_interactive_controls_are_excluded_and_maximize_takes_precedence() {
        let chrome = CustomTitlebar {
            draggable: vec![[100, 8, 900, 42]],
            maximize: Some([850, 8, 900, 42]),
            geometry_scale: 1.0,
            ..Default::default()
        };
        assert_eq!(chrome.hit_test([500, 20], 1.0), CAPTION);
        assert_eq!(chrome.hit_test([875, 20], 1.0), MAX_BUTTON);
        assert_eq!(chrome.hit_test([50, 20], 1.0), CLIENT);
        assert_eq!(chrome.hit_test([900, 20], 1.0), CLIENT);
    }

    #[test]
    fn published_titlebar_regions_survive_dpi_change_before_egui_repaints() {
        let chrome = CustomTitlebar {
            maximize: Some([1000, 10, 1100, 50]),
            geometry_scale: 1.25,
            ..Default::default()
        };
        assert_eq!(chrome.hit_test([850, 20], 1.0), MAX_BUTTON);
        assert_eq!(chrome.hit_test([1050, 25], 1.25), MAX_BUTTON);
        assert_eq!(chrome.hit_test([1300, 40], 1.5), MAX_BUTTON);
    }

    #[test]
    fn cleared_regions_leave_popup_and_modal_controls_in_client_area() {
        let chrome = CustomTitlebar::default();
        assert_eq!(chrome.hit_test([10, 10], 1.25), CLIENT);
        assert!(!chrome.maximize_hovered && !chrome.maximize_pressed);
    }

    #[test]
    fn native_maximize_click_requires_press_and_release_inside_the_current_button() {
        let mut chrome = CustomTitlebar {
            maximize: Some([850, 8, 900, 42]),
            geometry_scale: 1.0,
            ..Default::default()
        };
        assert_eq!(chrome.release_maximize_press([875, 20], 1.0, true), None);
        chrome.begin_maximize_press();
        assert_eq!(
            chrome.release_maximize_press([875, 20], 1.0, true),
            Some(true)
        );
        assert!(!chrome.maximize_pressed);
        chrome.begin_maximize_press();
        assert_eq!(
            chrome.release_maximize_press([500, 20], 1.0, true),
            Some(false)
        );
        assert!(!chrome.maximize_hovered);
        chrome.begin_maximize_press();
        chrome.maximize = None; // A popup/modal took ownership before release.
        assert_eq!(
            chrome.release_maximize_press([875, 20], 1.0, true),
            Some(false)
        );
    }

    #[test]
    fn capture_loss_or_disabled_maximize_cancels_native_button_activation() {
        let mut chrome = CustomTitlebar {
            maximize: Some([850, 8, 900, 42]),
            geometry_scale: 1.0,
            ..Default::default()
        };
        chrome.begin_maximize_press();
        assert!(chrome.cancel_maximize_press());
        assert_eq!(chrome.release_maximize_press([875, 20], 1.0, true), None);
        assert!(!chrome.maximize_hovered);
        chrome.begin_maximize_press();
        assert_eq!(
            chrome.release_maximize_press([875, 20], 1.0, false),
            Some(false)
        );
    }
}

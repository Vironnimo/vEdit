/// During an interactive move Windows has already selected the target monitor
/// and accounted for the titlebar grab position in WM_DPICHANGED's suggested
/// rectangle. Recalculating or nudging it back to the previous monitor can
/// cause repeated DPI transitions and exponential window growth on Windows 11.
/// Keep the ordinary path lazy: it must not query/nudge monitors during a drag.
pub(crate) fn outer_rect_for_dpi_change<T>(
    dragging_window: bool,
    suggested_rect: T,
    ordinary_resize: impl FnOnce() -> T,
) -> T {
    if dragging_window {
        suggested_rect
    } else {
        ordinary_resize()
    }
}

#[cfg(test)]
mod tests {
    use super::outer_rect_for_dpi_change;

    // Physical screen rectangles; the left monitor has negative x and a
    // different resolution/DPI. These coordinates must never be converted
    // through egui's UI zoom or nudged to the monitor being left.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    fn unexpected_monitor_nudge() -> Rect {
        panic!("an interactive DPI transition must use Windows' suggested rectangle")
    }

    #[test]
    fn mixed_dpi_drag_125_to_100_keeps_windows_target_rectangle() {
        let suggested = Rect {
            left: -1417,
            top: 349,
            right: -403,
            bottom: 1037,
        };
        assert_eq!(
            outer_rect_for_dpi_change(true, suggested, unexpected_monitor_nudge),
            suggested,
        );
    }

    #[test]
    fn mixed_dpi_drag_100_to_125_keeps_windows_target_rectangle() {
        let suggested = Rect {
            left: 160,
            top: 79,
            right: 1428,
            bottom: 939,
        };
        assert_eq!(
            outer_rect_for_dpi_change(true, suggested, unexpected_monitor_nudge),
            suggested,
        );
    }

    #[test]
    fn repeated_mixed_dpi_drags_never_accumulate_geometry_or_ui_zoom() {
        let left = Rect {
            left: -1417,
            top: 349,
            right: -403,
            bottom: 1037,
        };
        let right = Rect {
            left: 160,
            top: 79,
            right: 1428,
            bottom: 939,
        };
        for _ in 0..100 {
            assert_eq!(
                outer_rect_for_dpi_change(true, left, unexpected_monitor_nudge),
                left,
            );
            assert_eq!(
                outer_rect_for_dpi_change(true, right, unexpected_monitor_nudge),
                right,
            );
        }
    }

    #[test]
    fn dpi_change_outside_titlebar_drag_retains_normal_resize_behavior() {
        let suggested = Rect {
            left: 0,
            top: 0,
            right: 1200,
            bottom: 800,
        };
        let requested = Rect {
            left: 160,
            top: 79,
            right: 1428,
            bottom: 939,
        };
        let mut calls = 0;
        let actual = outer_rect_for_dpi_change(false, suggested, || {
            calls += 1;
            requested
        });
        assert_eq!(actual, requested);
        assert_eq!(calls, 1);
    }
}

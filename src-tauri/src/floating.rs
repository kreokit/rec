//! Where the floating control goes while recording (docs/m5-plan.md, choice 1). It shows
//! on the recorded display only where the app's own windows are kept out of the picture
//! (or a single window is recorded, so nothing around it is); otherwise on another display
//! that isn't recorded, and with only one display not at all (the menu bar / tray take its
//! place). The camera bubble shows only where it is kept out of the picture (or a window is
//! recorded): on another display it wouldn't help.

use crate::native::Display;

/// The control's size in points.
pub const CONTROL_SIZE: (f64, f64) = (292.0, 52.0);
/// Its distance from the display's bottom edge when nothing is remembered (above the Dock
/// or taskbar).
const BOTTOM_GAP: f64 = 96.0;

/// The display the control goes on, or `None` when it would be recorded.
pub fn control_display<'a>(
    displays: &'a [Display],
    recorded: u32,
    window_recording: bool,
    kept_out: bool,
) -> Option<&'a Display> {
    let is_recorded = |d: &Display| d.id == recorded || (recorded == 0 && d.main);
    if window_recording || kept_out {
        return displays
            .iter()
            .find(|d| is_recorded(d))
            .or_else(|| displays.first());
    }
    displays.iter().find(|d| !is_recorded(d))
}

/// Its top-left corner in global points: where it was left on this display (`saved`, from
/// the display's top-left), kept fully on the display; otherwise centred near the bottom.
pub fn control_position(display: &Display, saved: Option<(f64, f64)>) -> (f64, f64) {
    let (w, h) = CONTROL_SIZE;
    let max_x = (display.points_width - w).max(0.0);
    let max_y = (display.points_height - h).max(0.0);
    let (x, y) = saved.unwrap_or(((display.points_width - w) / 2.0, max_y - BOTTOM_GAP));
    (
        display.x + x.clamp(0.0, max_x),
        display.y + y.clamp(0.0, max_y),
    )
}

/// The camera bubble's size in points (a circle): Small, Medium, Large.
pub fn bubble_size(size: &str) -> f64 {
    match size {
        "small" => 160.0,
        "large" => 300.0,
        _ => 220.0,
    }
}

/// The display the camera bubble goes on (the recorded one), or `None` when it would be
/// recorded.
pub fn bubble_display(
    displays: &[Display],
    recorded: u32,
    window_recording: bool,
    kept_out: bool,
) -> Option<&Display> {
    if !(window_recording || kept_out) {
        return None;
    }
    control_display(displays, recorded, window_recording, kept_out)
}

/// The bubble's top-left corner in global points: where it was left (`saved`, from the
/// display's top-left), kept on the display; otherwise bottom right, above the Dock.
pub fn bubble_position(display: &Display, saved: Option<(f64, f64)>, size: f64) -> (f64, f64) {
    let max_x = (display.points_width - size).max(0.0);
    let max_y = (display.points_height - size).max(0.0);
    let (x, y) = saved.unwrap_or((max_x - 32.0, max_y - BOTTOM_GAP - 24.0));
    (
        display.x + x.clamp(0.0, max_x),
        display.y + y.clamp(0.0, max_y),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display(id: u32, main: bool, x: f64) -> Display {
        Display {
            id,
            name: format!("Display {id}"),
            width: 2880,
            height: 1800,
            main,
            x,
            y: 0.0,
            points_width: 1440.0,
            points_height: 900.0,
        }
    }

    #[test]
    fn goes_where_it_isnt_recorded() {
        let one = [display(1, true, 0.0)];
        let two = [display(1, true, 0.0), display(2, false, 1440.0)];
        // Kept out of the picture, or a window recorded: on the recorded display.
        assert_eq!(control_display(&two, 2, false, true).unwrap().id, 2);
        assert_eq!(control_display(&two, 0, false, true).unwrap().id, 1);
        assert_eq!(control_display(&one, 1, true, false).unwrap().id, 1);
        // It would be recorded: on another display, or nowhere.
        assert_eq!(control_display(&two, 1, false, false).unwrap().id, 2);
        assert_eq!(control_display(&two, 0, false, false).unwrap().id, 2);
        assert_eq!(control_display(&two, 2, false, false).unwrap().id, 1);
        assert!(control_display(&one, 1, false, false).is_none());
        assert!(control_display(&one, 0, false, false).is_none());
    }

    #[test]
    fn stays_on_its_display() {
        let d = display(2, false, 1440.0);
        // Not remembered: centred, above the Dock.
        assert_eq!(control_position(&d, None), (1440.0 + 574.0, 752.0));
        assert_eq!(control_position(&d, Some((10.0, 20.0))), (1450.0, 20.0));
        // Left partly off the display (or the display got smaller): pulled back on.
        assert_eq!(
            control_position(&d, Some((1400.0, -5.0))),
            (1440.0 + 1148.0, 0.0)
        );
        assert_eq!(control_position(&d, Some((-50.0, 2000.0))), (1440.0, 848.0));
    }

    #[test]
    fn the_bubble_shows_only_where_it_isnt_recorded() {
        let two = [display(1, true, 0.0), display(2, false, 1440.0)];
        assert_eq!(bubble_display(&two, 2, false, true).unwrap().id, 2);
        assert_eq!(bubble_display(&two, 1, true, false).unwrap().id, 1);
        // It would be recorded: not on another display either.
        assert!(bubble_display(&two, 1, false, false).is_none());
        let d = display(1, true, 0.0);
        assert_eq!(bubble_size("medium"), 220.0);
        assert_eq!(bubble_size("anything"), 220.0);
        assert_eq!(bubble_position(&d, None, 220.0), (1188.0, 560.0));
        assert_eq!(
            bubble_position(&d, Some((2000.0, 5.0)), 300.0),
            (1140.0, 5.0)
        );
    }
}

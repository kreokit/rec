//! Where everything goes in the exported picture (milestone 4). The same maths runs in the
//! webview (`src/lib/layout.ts`) to draw the preview; both are tested against the cases in
//! `src/lib/layout-cases.json`, so the preview and the export can't disagree.

use serde::{Deserialize, Serialize};

use crate::finish::{Bubble, Layout, Shape, Size};

/// A rectangle in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// The exported picture: its size, and where the camera goes (if anywhere).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Placement {
    pub width: u32,
    pub height: u32,
    /// The part of the camera picture that is shown (cut to the bubble's shape).
    pub camera_crop: Option<Rect>,
    /// Where the camera goes in the exported picture.
    pub camera: Option<Rect>,
    /// Corner radius of the camera's shape in pixels (half the size for a circle; 0 when
    /// the camera fills the picture).
    pub radius: u32,
}

/// The bubble's size as a share of the picture's shorter side.
pub fn size_share(size: Size) -> f64 {
    match size {
        Size::Small => 0.18,
        Size::Medium => 0.25,
        Size::Large => 0.33,
    }
}

/// Space kept between the bubble and the picture's edges, as a share of the shorter side.
pub const MARGIN_SHARE: f64 = 0.03;
/// A rounded bubble's corner radius as a share of its height.
pub const ROUNDED_RADIUS_SHARE: f64 = 0.16;

/// The nearest even number (H.264 with 4:2:0 colour wants even sizes and positions).
fn even(v: f64) -> u32 {
    ((v / 2.0).round() * 2.0).max(2.0) as u32
}

/// The largest rectangle of shape `aspect` (width / height) in the middle of a picture.
fn centre_crop(width: u32, height: u32, aspect: f64) -> Rect {
    let (w, h) = if width as f64 / height as f64 > aspect {
        (even(height as f64 * aspect).min(width), height)
    } else {
        (width, even(width as f64 / aspect).min(height))
    };
    Rect {
        x: (width - w) / 2 / 2 * 2,
        y: (height - h) / 2 / 2 * 2,
        width: w,
        height: h,
    }
}

/// Where things go for `layout`. `screen` and `camera` are the recorded sizes.
pub fn place(
    layout: Layout,
    bubble: &Bubble,
    screen: Option<(u32, u32)>,
    camera: Option<(u32, u32)>,
) -> Placement {
    match (layout, screen, camera) {
        (Layout::Bubble, Some((w, h)), Some((cw, ch))) => {
            let short = w.min(h) as f64;
            let bh = even(short * size_share(bubble.size));
            let (bw, aspect, radius) = match bubble.shape {
                Shape::Circle => (bh, 1.0, bh / 2),
                Shape::Rounded => (
                    even(bh as f64 * 4.0 / 3.0),
                    4.0 / 3.0,
                    (bh as f64 * ROUNDED_RADIUS_SHARE).round() as u32,
                ),
            };
            let margin = (short * MARGIN_SHARE).round();
            let room_x = (w as f64 - 2.0 * margin - bw as f64).max(0.0);
            let room_y = (h as f64 - 2.0 * margin - bh as f64).max(0.0);
            let x = ((margin + bubble.x.clamp(0.0, 1.0) * room_x) / 2.0).round() as u32 * 2;
            let y = ((margin + bubble.y.clamp(0.0, 1.0) * room_y) / 2.0).round() as u32 * 2;
            Placement {
                width: w,
                height: h,
                camera_crop: Some(centre_crop(cw, ch, aspect)),
                camera: Some(Rect {
                    x: x.min(w - bw),
                    y: y.min(h - bh),
                    width: bw,
                    height: bh,
                }),
                radius,
            }
        }
        // The camera alone, at its own size.
        (Layout::Camera, _, Some((cw, ch))) => Placement {
            width: cw,
            height: ch,
            camera_crop: Some(Rect {
                x: 0,
                y: 0,
                width: cw,
                height: ch,
            }),
            camera: Some(Rect {
                x: 0,
                y: 0,
                width: cw,
                height: ch,
            }),
            radius: 0,
        },
        (_, Some((w, h)), _) => Placement {
            width: w,
            height: h,
            camera_crop: None,
            camera: None,
            radius: 0,
        },
        // No screen and no camera: nothing to show (Finish offers no such layout).
        _ => Placement {
            width: 0,
            height: 0,
            camera_crop: None,
            camera: None,
            radius: 0,
        },
    }
}

/// The camera's shape as an 8-bit alpha mask of `width` × `height`: 255 inside, 0 outside,
/// edges smoothed over one pixel.
pub fn mask(width: u32, height: u32, radius: u32) -> Vec<u8> {
    let (w, h) = (width as f64, height as f64);
    let r = (radius as f64).min(w / 2.0).min(h / 2.0);
    let mut out = Vec::with_capacity((width * height) as usize);
    for py in 0..height {
        for px in 0..width {
            // Distance outside the rounded rectangle, from the pixel's centre.
            let (cx, cy) = (px as f64 + 0.5, py as f64 + 0.5);
            let dx = (r - cx).max(cx - (w - r)).max(0.0);
            let dy = (r - cy).max(cy - (h - r)).max(0.0);
            let outside = if r > 0.0 {
                (dx * dx + dy * dy).sqrt() - r
            } else {
                -1.0
            };
            let edge = if dx > 0.0 && dy > 0.0 {
                outside
            } else {
                // Along the straight edges: inside unless past the picture.
                -1.0
            };
            let a = (0.5 - edge).clamp(0.0, 1.0);
            out.push((a * 255.0).round() as u8);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    struct Case {
        name: String,
        layout: Layout,
        bubble: Bubble,
        screen: Option<(u32, u32)>,
        camera: Option<(u32, u32)>,
        expected: Placement,
    }

    /// The cases the TypeScript twin is tested against too.
    #[test]
    fn matches_the_shared_cases() {
        let cases: Vec<Case> =
            serde_json::from_str(include_str!("../../src/lib/layout-cases.json")).unwrap();
        assert!(cases.len() >= 6);
        for c in cases {
            assert_eq!(
                place(c.layout, &c.bubble, c.screen, c.camera),
                c.expected,
                "{}",
                c.name
            );
        }
    }

    #[test]
    fn the_bubble_stays_inside_the_picture() {
        for size in [Size::Small, Size::Medium, Size::Large] {
            for shape in [Shape::Circle, Shape::Rounded] {
                for (x, y) in [(0.0, 0.0), (1.0, 1.0), (0.5, 0.3), (1.0, 0.0)] {
                    let b = Bubble { shape, size, x, y };
                    for screen in [(1728, 1080), (1080, 1920), (640, 360), (1366, 768)] {
                        let p = place(Layout::Bubble, &b, Some(screen), Some((1280, 720)));
                        let r = p.camera.unwrap();
                        assert!(r.x + r.width <= p.width && r.y + r.height <= p.height);
                        assert!(r.x % 2 == 0 && r.y % 2 == 0 && r.width % 2 == 0);
                        let crop = p.camera_crop.unwrap();
                        assert!(crop.x + crop.width <= 1280 && crop.y + crop.height <= 720);
                    }
                }
            }
        }
    }

    #[test]
    fn a_circle_mask_is_round_and_smooth() {
        let m = mask(100, 100, 50);
        let at = |x: usize, y: usize| m[y * 100 + x];
        assert_eq!(at(50, 50), 255);
        assert_eq!(at(0, 0), 0, "corners are outside");
        assert_eq!(at(99, 99), 0);
        // The top middle touches the edge (its centre is half a pixel inside).
        assert!(at(50, 0) >= 250, "{}", at(50, 0));
        assert!(at(0, 50) >= 250);
        // Along the diagonal the edge is at 50 − 50/√2 ≈ 14.6 px: partly covered there.
        let edge = at(14, 14);
        assert!(edge > 0 && edge < 255, "{edge}");
        // A rounded rectangle: straight edges fully inside, corners cut.
        let r = mask(120, 90, 14);
        assert_eq!(r[45 * 120], 255);
        assert_eq!(r[0], 0);
    }
}

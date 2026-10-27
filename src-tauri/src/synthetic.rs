//! Generated sources for tests on Windows and Linux (the macOS recorder has its own, drawn
//! the same way in recorder.m): the screen and the camera show the test pattern that
//! testpattern.rs reads back, the microphone and the computer sound click every second.
//!
//! The pattern: a 32-bit frame counter as dark/light squares in the top row (34 cells across
//! the picture, from half a cell in; the screen's counter is the capture frame number since
//! the start, the camera's its output frame number), and a light strip below it for one
//! output frame from every whole second, when the sound clicks. With a region, the pattern
//! fills only the region and the rest of the made-up display is light, so a wrong crop
//! shows in the recording.

#![cfg_attr(
    all(target_os = "macos", not(feature = "pipe-recorder")),
    allow(dead_code)
)]

use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use crate::encoder::{Picture, PixelFormat, RATE, Rect};
use crate::native::{RecordConfig, Synthetic};
use crate::pipeline::{ALLOWANCE, Core, Source, host_seconds};

/// With `late_start`, when the made-up camera's first picture and mic's first sound come
/// after the start (the dev Mac's camera: 0.9 s, its mic: 0.35 s, measured 2026-10-27).
const CAMERA_WARM_UP: f64 = 1.0;
const MIC_WARM_UP: f64 = 0.4;
/// The made-up display's size in points (lib.rs).
pub const DISPLAY_POINTS: (f64, f64) = (960.0, 600.0);

/// Draws the pattern with counter `n` into an NV12 picture, inside `area` (the rest light)
/// or over the whole picture.
pub fn draw(p: &mut Picture, n: i64, marker: bool, area: Option<Rect>) {
    debug_assert_eq!(p.format, PixelFormat::Nv12);
    let (fw, fh) = (p.width as usize, p.height as usize);
    let (y, uv) = p.data.split_at_mut(fw * fh);
    uv.fill(128);
    let (ox, oy, w, h) = match area {
        Some(r) => {
            y.fill(235);
            (
                r.x as usize,
                r.y as usize,
                r.width as usize,
                r.height as usize,
            )
        }
        None => (0, 0, fw, fh),
    };
    let row = |y: &mut [u8], r: usize, x: usize, len: usize, v: u8| {
        let start = (oy + r) * fw + ox + x;
        y[start..start + len].fill(v);
    };
    for r in 0..h {
        row(y, r, 0, w, 128);
    }
    let cell = w / 34;
    for b in 0..32 {
        let v = if (n >> (31 - b)) & 1 == 1 { 235 } else { 16 };
        for r in cell / 2..cell / 2 + cell {
            row(y, r, cell / 2 + b * cell, cell, v);
        }
    }
    for r in cell * 2..(cell * 4).min(h) {
        row(y, r, 0, w, if marker { 235 } else { 16 });
    }
}

/// The generator: runs until the recording's stop (and 0.3 s past it, so the last slots get
/// their pictures as with a real screen).
pub struct Generator(Option<JoinHandle<()>>);

impl Source for Generator {
    fn stop(mut self: Box<Self>) {
        if let Some(t) = self.0.take() {
            let _ = t.join();
        }
    }
}

/// What the made-up screen draws on: the whole picture, or a region of the made-up display
/// (then the display at the size where the region comes out at the output size).
pub fn screen_layout(config: &RecordConfig) -> Option<(u32, u32, Option<Rect>)> {
    let s = config.screen.as_ref()?;
    Some(match s.region {
        Some(r) => {
            let scale = s.width as f64 / r.width;
            let w = (DISPLAY_POINTS.0 * scale).round() as u32 & !1;
            let h = (DISPLAY_POINTS.1 * scale).round() as u32 & !1;
            let mut rect = crate::pipeline::region_rect(&r, DISPLAY_POINTS, w, h);
            // Exactly the output size (no scaling), as on the Mac.
            rect.width = s.width;
            rect.height = s.height;
            rect.x = rect.x.min(w - s.width);
            rect.y = rect.y.min(h - s.height);
            (w, h, Some(rect))
        }
        None => (s.width, s.height, None),
    })
}

pub fn start(core: Arc<Core>, config: &RecordConfig, flags: Synthetic) -> Generator {
    let screen = screen_layout(config);
    let camera = config.camera.as_ref().map(|c| (c.width, c.height));
    let fps = config.screen.as_ref().map_or(30, |s| s.fps) as i64;
    let cfps = config
        .screen
        .as_ref()
        .map_or(fps, |s| (s.capture_fps as i64).max(fps));
    let window = config.screen.as_ref().is_some_and(|s| s.window.is_some());
    let system = config.system_audio;
    let thread = std::thread::Builder::new()
        .name("kreokit-rec-synthetic".into())
        .spawn(move || {
            let t0 = core.t0();
            let base = t0 - 0.5;
            let chunk = (RATE / cfps) as usize;
            let mut audio = vec![0f32; chunk];
            let mut stereo = vec![0f32; chunk * 2];
            for i in 0i64.. {
                let t = base + i as f64 / cfps as f64;
                if flags.fast {
                    core.set_synthetic_now(t);
                    // The encoder sets the pace: wait while half a second is queued.
                    while core.congested() {
                        std::thread::sleep(Duration::from_micros(500));
                    }
                } else {
                    let wait = t - host_seconds();
                    if wait > 0.0 {
                        std::thread::sleep(Duration::from_secs_f64(wait));
                    }
                }
                if t > core.stop_at() + 0.3 {
                    break;
                }
                let n = ((t - t0) * cfps as f64).round() as i64;
                let since = t - t0;
                let frozen = flags.still_periods
                    && since >= 0.0
                    && (5.0..7.0).contains(&since.rem_euclid(10.0));
                for (k, a) in audio.iter_mut().enumerate() {
                    let ts = t + k as f64 / RATE as f64 - t0;
                    *a = if ts >= 0.0 && ts - ts.floor() < 0.005 {
                        0.5
                    } else {
                        0.0
                    };
                }
                if !flags.late_start || t >= t0 + MIC_WARM_UP {
                    core.mic(&audio, t);
                }
                if system {
                    for (k, a) in audio.iter().enumerate() {
                        stereo[2 * k] = *a;
                        stereo[2 * k + 1] = *a;
                    }
                    core.system(&stereo, t);
                }
                // A recorded window "closes" 2 s after the start: no more pictures from it.
                if window && since >= 2.0 && !core.has_window_closed() {
                    core.window_closed();
                }
                if let Some((w, h, area)) = screen
                    && !frozen
                    && !core.has_window_closed()
                {
                    let mut p = Picture::black(w, h, PixelFormat::Nv12);
                    draw(&mut p, n.max(0), n >= 0 && n % cfps < cfps / fps, area);
                    core.screen(Arc::new(p), t);
                }
                if let Some((w, h)) = camera
                    && i % (cfps / fps) == 0
                    && (!flags.late_start || t >= t0 + CAMERA_WARM_UP)
                {
                    let c = ((t - t0) * fps as f64).round() as i64;
                    let mut p = Picture::black(w, h, PixelFormat::Nv12);
                    draw(&mut p, c.max(0), c >= 0 && c % fps == 0, None);
                    core.camera(Arc::new(p), t);
                }
                if flags.fast {
                    core.emit_up_to(t - ALLOWANCE);
                }
            }
        })
        .expect("a thread");
    Generator(Some(thread))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testpattern::Geometry;

    #[test]
    fn the_pattern_reads_back() {
        let mut p = Picture::black(340, 192, PixelFormat::Nv12);
        draw(&mut p, 0x1234_5678, true, None);
        let g = Geometry::synthetic(340, 60, false);
        let cell = g.cell as usize;
        let luma = |x: usize, y: usize| p.data[y * 340 + x];
        let mut n = 0u32;
        for b in 0..32 {
            n = n << 1 | (luma(cell / 2 + b * cell + cell / 2, cell) > 128) as u32;
        }
        assert_eq!(n, 0x1234_5678);
        assert_eq!(luma(5, cell * 3), 235);
        assert_eq!(luma(5, 150), 128);
    }
}

//! Reads the test pattern back from a recording (the synthetic sources' and the bench
//! window's, drawn as in recorder.m): the frame counter and the whole-second marker in
//! the picture, the clicks in the sound. From research/capture/analyze.py, in Rust so the
//! tests and `--bench-analyze` share it.
//!
//! A repeated picture is "unchanged" when the pattern stood still at that moment (the
//! still periods: 2 s of every 10 s), and "stale" otherwise: the screen changed but the
//! recording didn't show it.

use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::{ffmpeg, probe};

/// Where the pattern is in the recorded picture, in pixels: its top-left corner and the
/// size of one counter square. The counter row is 34 squares wide and 4 high: the bits in
/// row 1 (from half a square in), the marker strip in rows 2-3.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Geometry {
    pub x: f64,
    pub y: f64,
    pub cell: f64,
    /// Counter steps per second (the capture rate for synthetic sources).
    pub rate: f64,
    /// Whether the pattern has still periods.
    pub still_periods: bool,
}

impl Geometry {
    /// The synthetic screen's pattern: across the top of the whole picture.
    #[cfg(test)]
    pub fn synthetic(width: u32, capture_fps: u32, still_periods: bool) -> Geometry {
        Geometry {
            x: 0.0,
            y: 0.0,
            cell: (width / 34) as f64,
            rate: capture_fps as f64,
            still_periods,
        }
    }

    /// Whether counter value `n` was the last one shown before the pattern stood still
    /// (allowing for one missed refresh).
    fn still_after(&self, n: u32) -> bool {
        self.still_periods
            && [1.0, 2.0].iter().any(|d| {
                let t = (n as f64 + d) / self.rate;
                let phase = t.rem_euclid(10.0);
                (5.0..7.0).contains(&phase)
            })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub pts: f64,
    pub counter: u32,
    pub marker: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub frames: usize,
    /// Frames showing a newer counter than the frame before.
    pub fresh: usize,
    /// Repeats while the pattern stood still (nothing to show).
    pub unchanged: usize,
    /// Repeats while the pattern was changing (a picture missing).
    pub stale: usize,
    /// Counter going backwards or unreadable (should be 0).
    pub bad: usize,
    /// fresh / (fresh + stale): the share of slots that showed the screen's newest state.
    pub fresh_percent: f64,
    /// The largest jump of the counter between two frames (2 at 60 to 30 fps).
    pub max_step: u32,
    /// Output timestamps that aren't exactly one frame apart.
    pub irregular_pts: usize,
    pub markers: usize,
    pub clicks: usize,
    /// Markers with no click within 100 ms (a pause can cut a click but keep its marker;
    /// otherwise this means picture and sound are far apart).
    pub unmatched_markers: usize,
    /// Click time minus marker frame time, for each marker: picture-sound offset.
    pub sync_ms_first: Option<f64>,
    pub sync_ms_last: Option<f64>,
    pub sync_ms_min: Option<f64>,
    pub sync_ms_max: Option<f64>,
    /// How far the pattern was kept out of the recording (bench: ~0 when it was hidden).
    pub pattern_visible: f64,
}

fn run(command: &mut Command) -> Result<Vec<u8>, String> {
    let output = command.output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(output.stdout)
}

/// One gray pixel row of every frame, `width` pixels from (x, y).
fn rows(video: &Path, x: u32, y: u32, width: u32) -> Result<Vec<Vec<u8>>, String> {
    let ffmpeg = ffmpeg::locate("ffmpeg")?;
    let raw = run(Command::new(ffmpeg)
        .args(["-v", "error", "-i"])
        .arg(video)
        .args(["-fps_mode", "passthrough", "-vf"])
        .arg(format!("format=gray,crop={width}:1:{x}:{y}"))
        .args(["-f", "rawvideo", "-"]))?;
    Ok(raw.chunks(width as usize).map(<[u8]>::to_vec).collect())
}

pub fn read_frames(video: &Path, g: &Geometry) -> Result<Vec<Frame>, String> {
    let ffprobe = ffmpeg::locate("ffprobe")?;
    let pts_out = run(Command::new(ffprobe)
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries"])
        .args(["packet=pts_time", "-of", "csv=p=0"])
        .arg(video))?;
    let mut pts: Vec<f64> = String::from_utf8_lossy(&pts_out)
        .split_whitespace()
        .filter_map(|s| s.trim_end_matches(',').parse().ok())
        .collect();
    pts.sort_by(f64::total_cmp);
    let width = (g.cell * 34.0).round() as u32;
    let bits = rows(
        video,
        g.x.round() as u32,
        (g.y + g.cell).round() as u32,
        width,
    )?;
    let marks = rows(
        video,
        g.x.round() as u32,
        (g.y + g.cell * 3.0).round() as u32,
        width,
    )?;
    Ok(bits
        .iter()
        .zip(&marks)
        .enumerate()
        .map(|(i, (row, mark))| {
            let counter = (0..32).fold(0u32, |n, b| {
                let x = ((b as f64 + 1.0) * g.cell).round() as usize;
                (n << 1) | (row.get(x).is_some_and(|&v| v > 128) as u32)
            });
            let mean = mark.iter().map(|&v| v as f64).sum::<f64>() / mark.len().max(1) as f64;
            Frame {
                pts: pts.get(i).copied().unwrap_or(f64::NAN),
                counter,
                marker: mean > 128.0,
            }
        })
        .collect())
}

/// The sound as 48 kHz mono samples, from the stream's start on the file's timeline.
pub fn read_audio(audio: &Path) -> Result<(Vec<f32>, f64), String> {
    let start = probe::probe(audio)?
        .audio()
        .map(|a| a.start_s)
        .ok_or("the file has no sound")?;
    let ffmpeg = ffmpeg::locate("ffmpeg")?;
    let raw = run(Command::new(ffmpeg)
        .args(["-v", "error", "-i"])
        .arg(audio)
        .args(["-ac", "1", "-ar", "48000", "-f", "f32le", "-"]))?;
    let samples = raw
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    Ok((samples, start))
}

/// Start times (s) of the clicks: the first sample above `threshold` after `quiet` s
/// without one.
pub fn onsets(samples: &[f32], start: f64, threshold: f32, quiet: f64) -> Vec<f64> {
    let mut out = Vec::new();
    let mut quiet_until = -1i64;
    let gap = (quiet * 48_000.0) as i64;
    for (i, s) in samples.iter().enumerate() {
        if s.abs() > threshold {
            if i as i64 > quiet_until {
                out.push(start + i as f64 / 48_000.0);
            }
            quiet_until = i as i64 + gap;
        }
    }
    out
}

/// The report for a picture and its clicks (`clicks` may be empty: no sound checked).
pub fn report(frames: &[Frame], clicks: &[f64], g: &Geometry, fps: f64) -> Report {
    let mut r = Report {
        frames: frames.len(),
        clicks: clicks.len(),
        ..Default::default()
    };
    for pair in frames.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if b.counter > a.counter {
            r.fresh += 1;
            r.max_step = r.max_step.max(b.counter - a.counter);
        } else if b.counter == a.counter {
            if g.still_after(a.counter) {
                r.unchanged += 1;
            } else {
                r.stale += 1;
            }
        } else {
            r.bad += 1;
        }
        if ((b.pts - a.pts) * fps - 1.0).abs() > 0.01 {
            r.irregular_pts += 1;
        }
    }
    r.fresh_percent = if r.fresh + r.stale > 0 {
        100.0 * r.fresh as f64 / (r.fresh + r.stale) as f64
    } else {
        0.0
    };
    // A visible counter stays small; random screen content reads as a random number.
    r.pattern_visible =
        frames.iter().filter(|f| f.counter < 1_000_000).count() as f64 / frames.len().max(1) as f64;
    // The first frame of each marker run against the nearest click.
    let markers: Vec<f64> = frames
        .iter()
        .enumerate()
        .filter(|(i, f)| f.marker && (*i == 0 || !frames[i - 1].marker))
        .map(|(_, f)| f.pts)
        .collect();
    r.markers = markers.len();
    let offsets: Vec<f64> = markers
        .iter()
        .filter_map(|&m| {
            clicks
                .iter()
                .map(|&c| c - m)
                .min_by(|a, b| a.abs().total_cmp(&b.abs()))
                .filter(|d| d.abs() < 0.1)
        })
        .map(|d| (d * 1e5).round() / 100.0)
        .collect();
    r.unmatched_markers = markers.len() - offsets.len();
    r.sync_ms_first = offsets.first().copied();
    r.sync_ms_last = offsets.last().copied();
    r.sync_ms_min = offsets.iter().copied().reduce(f64::min);
    r.sync_ms_max = offsets.iter().copied().reduce(f64::max);
    r
}

/// The whole check of a synthetic recording's folder (screen.mp4 + mic.mp4).
#[cfg(test)]
pub fn analyze_dir(dir: &Path, g: &Geometry, fps: f64) -> Result<Report, String> {
    let frames = read_frames(&dir.join("screen.mp4"), g)?;
    let mic = dir.join("mic.mp4");
    let clicks = if mic.is_file() {
        let (samples, start) = read_audio(&mic)?;
        onsets(&samples, start, 0.25, 0.5)
    } else {
        Vec::new()
    };
    Ok(report(&frames, &clicks, g, fps))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(counters: &[u32]) -> Vec<Frame> {
        counters
            .iter()
            .enumerate()
            .map(|(i, &counter)| Frame {
                pts: i as f64 / 30.0,
                counter,
                marker: counter % 60 == 0,
            })
            .collect()
    }

    #[test]
    fn tells_unchanged_from_stale() {
        let g = Geometry::synthetic(640, 60, true);
        // 5 s at 60 steps per second = 300: the pattern stands still after 299.
        let r = report(
            &frames(&[294, 296, 298, 299, 299, 299, 420, 422, 422]),
            &[],
            &g,
            30.0,
        );
        assert_eq!((r.fresh, r.unchanged, r.stale, r.bad), (5, 2, 1, 0));
        assert_eq!(r.max_step, 121);
        let r = report(
            &frames(&[2, 4, 4, 6]),
            &[],
            &Geometry::synthetic(640, 60, false),
            30.0,
        );
        assert_eq!((r.fresh, r.unchanged, r.stale), (2, 0, 1));
        assert!((r.fresh_percent - 66.67).abs() < 0.01);
    }

    #[test]
    fn measures_sync_against_the_clicks() {
        let g = Geometry::synthetic(640, 60, false);
        let f = frames(&(0..60).map(|i| i * 2).collect::<Vec<_>>());
        // Markers at counter 0, 60 (frame 30, 1 s), 120 (frame 60, out of range).
        let r = report(&f, &[0.010, 1.001], &g, 30.0);
        assert_eq!(r.markers, 2);
        assert_eq!(r.sync_ms_first, Some(10.0));
        assert_eq!(r.sync_ms_last, Some(1.0));
        assert_eq!(r.irregular_pts, 0);
    }

    #[test]
    fn finds_onsets() {
        let mut s = vec![0.0f32; 48_000 * 2];
        s[480] = 0.5;
        s[500] = 0.5; // the same click
        s[48_000] = -0.6;
        assert_eq!(onsets(&s, 0.5, 0.25, 0.5), vec![0.51, 1.5]);
    }
}

//! Real-device measurements (the `bench` feature, test builds only): `--bench` records
//! through native/macos/bench.m, `--bench-analyze DIR` reads the result back and prints a
//! JSON report. Recording the real screen and microphone needs the user's OK each time.

use std::path::Path;
use std::process::Command;

use serde_json::{Value, json};

use crate::ffmpeg;
use crate::testpattern::{self, Geometry};

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn kr_bench_main(argc: i32, argv: *const *const std::ffi::c_char) -> i32;
}

/// Handles `--bench ...` and `--bench-analyze DIR`; `None` for a normal start.
pub fn main(args: &[String]) -> Option<i32> {
    match args.get(1).map(String::as_str) {
        Some("--bench-analyze") => Some(match args.get(2).map(|d| analyze(Path::new(d))) {
            Some(Ok(report)) => {
                println!("{}", serde_json::to_string_pretty(&report).unwrap());
                0
            }
            Some(Err(e)) => {
                eprintln!("{e}");
                1
            }
            None => {
                eprintln!("usage: --bench-analyze DIR");
                2
            }
        }),
        #[cfg(target_os = "macos")]
        _ if args.iter().any(|a| a == "--bench") => {
            let owned: Vec<std::ffi::CString> = args
                .iter()
                .map(|a| std::ffi::CString::new(a.as_str()).unwrap())
                .collect();
            let ptrs: Vec<_> = owned.iter().map(|a| a.as_ptr()).collect();
            Some(unsafe { kr_bench_main(ptrs.len() as i32, ptrs.as_ptr()) })
        }
        _ => None,
    }
}

pub fn analyze(dir: &Path) -> Result<Value, String> {
    let info: Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join("bench.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    if let Some(error) = info.get("error") {
        return Err(format!("the bench recording failed: {error}"));
    }
    if let Some(pattern) = info.get("pattern") {
        let g: Geometry = serde_json::from_value(pattern.clone()).map_err(|e| e.to_string())?;
        let frames = testpattern::read_frames(&dir.join("screen.mp4"), &g)?;
        let report = testpattern::report(&frames, &[], &g, 30.0);
        let lags = lag(&frames, &g);
        return Ok(json!({
            "recorder": info["status"],
            "file": report,
            "pattern_age_ms": lags,
        }));
    }
    let flash = info["flash"]
        .as_array()
        .ok_or("bench.json has no pattern or flash")?;
    let rect: Vec<u32> = flash
        .iter()
        .map(|v| v.as_f64().unwrap_or(0.0).round() as u32)
        .collect();
    let flashes = flash_times(&dir.join("screen.mp4"), &rect)?;
    let (samples, start) = testpattern::read_audio(&dir.join("mic.mp4"))?;
    let sounds = testpattern::onsets(&samples, start, click_threshold(&samples), 0.3);
    let presses: Vec<f64> = info["presses"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default();
    Ok(json!({
        "recorder": info["status"],
        "presses": presses.len(),
        "flashes": flashes.len(),
        "sounds": sounds.len(),
        // Picture minus sound: positive when the flash shows after the click is heard.
        "flash_minus_sound_ms": offsets(&flashes, &sounds),
        // Each track against the key press itself (macOS's event time).
        "sound_minus_press_ms": offsets(&sounds, &presses),
        "flash_minus_press_ms": offsets(&flashes, &presses),
    }))
}

/// The level a key click must reach: 30 dB above the room's quiet level (the 20th
/// percentile of 10 ms windows), at least -50 dBFS. Measured 2026-10-27 on the dev Mac:
/// quiet -84 dB, firm space-bar presses -3 to -27 dB peak; a share of the loudest press
/// missed most of them.
fn click_threshold(samples: &[f32]) -> f32 {
    let mut rms: Vec<f32> = samples
        .chunks(480)
        .map(|c| (c.iter().map(|x| x * x).sum::<f32>() / c.len() as f32).sqrt())
        .collect();
    if rms.is_empty() {
        return 1.0;
    }
    rms.sort_by(f32::total_cmp);
    let quiet = rms[rms.len() / 5];
    (quiet * 31.6).max(0.003_16)
}

/// How old the shown counter is at each frame: frame time minus the counter's own time,
/// relative to the smallest (only differences are meaningful; the clocks' zeros differ).
fn lag(frames: &[testpattern::Frame], g: &Geometry) -> Value {
    let mut ages: Vec<f64> = frames
        .windows(2)
        .filter(|w| w[1].counter > w[0].counter)
        .map(|w| w[1].pts - w[1].counter as f64 / g.rate)
        .collect();
    if ages.is_empty() {
        return Value::Null;
    }
    let min = ages.iter().copied().fold(f64::INFINITY, f64::min);
    ages.iter_mut().for_each(|a| *a = (*a - min) * 1000.0);
    ages.sort_by(f64::total_cmp);
    let q = |p: f64| (ages[((ages.len() - 1) as f64 * p).round() as usize] * 10.0).round() / 10.0;
    json!({ "p50": q(0.5), "p95": q(0.95), "max": q(1.0) })
}

/// For each time in `a`, the nearest one in `b` (within 0.25 s): a - b, with a summary.
fn offsets(a: &[f64], b: &[f64]) -> Value {
    let mut list: Vec<f64> = a
        .iter()
        .filter_map(|&x| {
            b.iter()
                .map(|&y| x - y)
                .min_by(|p, q| p.abs().total_cmp(&q.abs()))
                .filter(|d| d.abs() < 0.25)
        })
        .map(|d| (d * 10_000.0).round() / 10.0)
        .collect();
    let each = list.clone();
    list.sort_by(f64::total_cmp);
    if list.is_empty() {
        return json!({ "matched": 0 });
    }
    json!({
        "matched": list.len(),
        "median": list[list.len() / 2],
        "min": list[0],
        "max": list[list.len() - 1],
        "each": each,
    })
}

/// Start times of the white flashes inside `rect` ([x, y, w, h] in output pixels).
fn flash_times(video: &Path, rect: &[u32]) -> Result<Vec<f64>, String> {
    let [x, y, w, h] = rect else {
        return Err("bad flash rectangle".into());
    };
    let ffprobe = ffmpeg::locate("ffprobe")?;
    let out = Command::new(ffprobe)
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries"])
        .args(["packet=pts_time", "-of", "csv=p=0"])
        .arg(video)
        .output()
        .map_err(|e| e.to_string())?;
    let mut pts: Vec<f64> = String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .filter_map(|s| s.trim_end_matches(',').parse().ok())
        .collect();
    pts.sort_by(f64::total_cmp);
    let ffmpeg = ffmpeg::locate("ffmpeg")?;
    let out = Command::new(ffmpeg)
        .args(["-v", "error", "-i"])
        .arg(video)
        .args(["-fps_mode", "passthrough", "-vf"])
        .arg(format!(
            "crop={w}:{h}:{x}:{y},scale=1:1:flags=area,format=gray"
        ))
        .args(["-f", "rawvideo", "-"])
        .output()
        .map_err(|e| e.to_string())?;
    let luma = out.stdout;
    Ok((0..luma.len())
        .filter(|&i| luma[i] > 128 && (i == 0 || luma[i - 1] <= 128))
        .filter_map(|i| pts.get(i).copied())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_quiet_clicks_above_the_room() {
        // A quiet room (-80 dB) with a -60 dB click and a -20 dB one.
        let mut s: Vec<f32> = (0..96_000)
            .map(|i| 1e-4 * ((i % 7) as f32 - 3.0) / 3.0)
            .collect();
        s[24_000] = 0.001;
        s[72_000] = 0.1;
        let t = click_threshold(&s);
        assert!(t > 0.001 && t < 0.1, "{t}");
        assert_eq!(testpattern::onsets(&s, 0.0, t, 0.3), vec![1.5]);
    }

    #[test]
    fn pairs_nearest_events() {
        let v = offsets(&[1.02, 2.0, 5.0], &[1.0, 2.01, 3.0]);
        assert_eq!(v["matched"], 2);
        assert_eq!(v["each"], json!([20.0, -10.0]));
        assert_eq!(offsets(&[], &[1.0])["matched"], 0);
    }

    /// The fresh-frame analysis on a synthetic recording with still periods.
    #[cfg(target_os = "macos")]
    #[test]
    fn analyzes_a_fresh_bench_recording() {
        use crate::native::*;
        let dir = std::env::temp_dir().join("kreokit-rec-bench-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let rec = Recorder::start(&RecordConfig {
            dir: dir.clone(),
            screen: Some(ScreenConfig {
                region: None,
                display_id: 0,
                width: 640,
                height: 360,
                fps: 30,
                capture_fps: 60,
                api: ScreenApi::Auto,
                bitrate: 2_000_000,
            }),
            mic: None,
            camera: None,
            synthetic: Some(Synthetic {
                fast: true,
                still_periods: true,
            }),
        })
        .unwrap();
        while rec.status().elapsed < 12.0 {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        rec.stop();
        let pattern = Geometry::synthetic(640, 60, true);
        std::fs::write(
            dir.join("bench.json"),
            json!({ "pattern": pattern, "status": {} }).to_string(),
        )
        .unwrap();
        let report = analyze(&dir).unwrap();
        assert_eq!(report["file"]["stale"], 0, "{report}");
        assert_eq!(report["file"]["fresh_percent"], 100.0, "{report}");
        assert!(
            report["file"]["unchanged"].as_u64().unwrap() >= 55,
            "{report}"
        );
        // Every kept frame is the newest: its age varies by less than one capture frame.
        assert!(
            report["pattern_age_ms"]["max"].as_f64().unwrap() < 17.0,
            "{report}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

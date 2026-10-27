//! Real-device measurements (the `bench` feature, test builds only): `--bench` records
//! through native/macos/bench.m, `--bench-analyze DIR` reads the result back and prints a
//! JSON report. Recording the real screen and microphone needs the user's OK each time.

use std::path::Path;
use std::process::Command;

use serde_json::{Value, json};

use crate::check;
use crate::ffmpeg;
use crate::native::Status;
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
    if info["mode"] == "camera" {
        return analyze_claps(dir, &info);
    }
    if let Some(pattern) = info.get("pattern") {
        let g: Geometry = serde_json::from_value(pattern.clone()).map_err(|e| e.to_string())?;
        let frames = testpattern::read_frames(&dir.join("screen.mp4"), &g)?;
        let report = testpattern::report(&frames, &[], &g, 30.0);
        let lags = lag(&frames, &g);
        let mut result = json!({
            "recorder": info["status"],
            "cpu_percent": info["cpu_percent"],
            "file": report,
            "pattern_age_ms": lags,
        });
        if info["camera"] == true {
            // Every track: frames written vs in the files, start/end offsets (drift).
            let status: Option<Status> = serde_json::from_value(info["status"].clone()).ok();
            result["check"] = json!(check::check(dir, status.as_ref(), 30));
            result["camera_pts"] = json!(regular_times(&packet_times(&dir.join("camera.mp4"))?));
        }
        return Ok(result);
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

/// The clap test: each clap heard in mic.mp4 against the hands meeting in camera.mp4.
fn analyze_claps(dir: &Path, info: &Value) -> Result<Value, String> {
    let (samples, start) = testpattern::read_audio(&dir.join("mic.mp4"))?;
    let sounds = testpattern::onsets(&samples, start, click_threshold(&samples), 0.3);
    let motion = motion(&dir.join("camera.mp4"))?;
    let status: Option<Status> = serde_json::from_value(info["status"].clone()).ok();
    Ok(json!({
        "recorder": info["status"],
        "cpu_percent": info["cpu_percent"],
        "sounds": sounds.len(),
        // Picture minus sound: positive when the hands meet in the picture after the clap
        // is heard (the camera's own delay).
        "camera_minus_sound_ms": summarize(clap_offsets(&motion, &sounds)),
        "check": check::check(dir, status.as_ref(), 30),
    }))
}

/// For each clap heard, the biggest change in the picture within 250 ms of it (the hands
/// closing fastest just before they meet): its time minus the sound's, in ms. Claps the
/// camera didn't see (no change well above the usual) are left out.
fn clap_offsets(motion: &[(f64, f64)], sounds: &[f64]) -> Vec<f64> {
    let mut changes: Vec<f64> = motion.iter().map(|m| m.1).collect();
    changes.sort_by(f64::total_cmp);
    let usual = changes.get(changes.len() / 2).copied().unwrap_or(0.0);
    sounds
        .iter()
        .filter_map(|&s| {
            motion
                .iter()
                .filter(|m| (m.0 - s).abs() <= 0.25)
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .filter(|m| m.1 > 3.0 * usual.max(0.5))
                .map(|m| ((m.0 - s) * 10_000.0).round() / 10.0)
        })
        .collect()
}

/// Frame-to-frame change in `video`: the later frame's time and the mean change in
/// brightness (0-255) on a 64x36 grey copy.
fn motion(video: &Path) -> Result<Vec<(f64, f64)>, String> {
    let pts = packet_times(video)?;
    let ffmpeg = ffmpeg::locate("ffmpeg")?;
    let out = Command::new(ffmpeg)
        .args(["-v", "error", "-i"])
        .arg(video)
        .args(["-fps_mode", "passthrough", "-vf"])
        .arg("scale=64:36:flags=area,format=gray")
        .args(["-f", "rawvideo", "-"])
        .output()
        .map_err(|e| e.to_string())?;
    let frames: Vec<&[u8]> = out.stdout.chunks_exact(64 * 36).collect();
    Ok(frames
        .windows(2)
        .zip(pts.iter().skip(1))
        .map(|(w, &t)| {
            let sum: u64 = w[0]
                .iter()
                .zip(w[1])
                .map(|(a, b)| a.abs_diff(*b) as u64)
                .sum();
            (t, sum as f64 / (64.0 * 36.0))
        })
        .collect())
}

/// Each picture's time in `video`, in order.
fn packet_times(video: &Path) -> Result<Vec<f64>, String> {
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
    Ok(pts)
}

/// How regular the picture times are: frames, and steps that aren't one 1/30 s frame.
fn regular_times(pts: &[f64]) -> Value {
    let irregular = pts
        .windows(2)
        .filter(|w| ((w[1] - w[0]) * 30.0 - 1.0).abs() > 0.01)
        .count();
    json!({ "frames": pts.len(), "irregular_steps": irregular })
}

/// For each time in `a`, the nearest one in `b` (within 0.25 s): a - b, with a summary.
fn offsets(a: &[f64], b: &[f64]) -> Value {
    summarize(
        a.iter()
            .filter_map(|&x| {
                b.iter()
                    .map(|&y| x - y)
                    .min_by(|p, q| p.abs().total_cmp(&q.abs()))
                    .filter(|d| d.abs() < 0.25)
            })
            .map(|d| (d * 10_000.0).round() / 10.0)
            .collect(),
    )
}

/// Count, median, range and every value of a list of offsets (ms).
fn summarize(each: Vec<f64>) -> Value {
    let mut list = each.clone();
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
    let pts = packet_times(video)?;
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
    fn finds_the_hands_meeting_near_each_clap() {
        // 30 fps; little change, except a burst 40-73 ms after the claps at 1.0 and 2.0 s;
        // the clap at 3.0 s isn't in view.
        let motion: Vec<(f64, f64)> = (1..120)
            .map(|i| {
                let t = i as f64 / 30.0;
                let burst = [1.0, 2.0].iter().any(|c| t > c + 0.03 && t < c + 0.08);
                (t, if burst { 20.0 + t } else { 1.0 })
            })
            .collect();
        let each = clap_offsets(&motion, &[1.0, 2.0, 3.0]);
        assert_eq!(each, vec![66.7, 66.7]);
        assert_eq!(summarize(each)["median"], 66.7);
    }

    #[test]
    fn pairs_nearest_events() {
        let v = offsets(&[1.02, 2.0, 5.0], &[1.0, 2.01, 3.0]);
        assert_eq!(v["matched"], 2);
        assert_eq!(v["each"], json!([20.0, -10.0]));
        assert_eq!(offsets(&[], &[1.0])["matched"], 0);
    }

    /// The fresh-frame analysis on a synthetic recording with still periods, with the
    /// camera and mic as in the long run.
    #[cfg(target_os = "macos")]
    #[test]
    fn analyzes_a_fresh_bench_recording() {
        let _timing = crate::native::tests::alongside();
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
            mic: Some(String::new()),
            camera: Some(CameraConfig {
                id: String::new(),
                width: 320,
                height: 180,
                bitrate: 1_000_000,
            }),
            synthetic: Some(Synthetic {
                fast: true,
                still_periods: true,
                late_camera: false,
            }),
        })
        .unwrap();
        while rec.status().elapsed < 12.0 {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let (status, _) = rec.stop();
        let pattern = Geometry::synthetic(640, 60, true);
        std::fs::write(
            dir.join("bench.json"),
            json!({ "pattern": pattern, "status": status, "camera": true, "cpu_percent": 50.0 })
                .to_string(),
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
        assert_eq!(report["check"]["problems"], json!([]), "{report}");
        assert_eq!(
            report["check"]["recorder"]["camera_frames"],
            json!(status.camera_frames)
        );
        assert_eq!(report["camera_pts"]["irregular_steps"], 0, "{report}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

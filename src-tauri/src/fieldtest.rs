//! The Tests screen of the test version (`bench` feature, never in releases): the items of
//! docs/manual-checklist.md on Windows, Linux and macOS, run by clicking, for people trying
//! the test version. A small window shows the test pattern (src/components/PatternWindow.tsx:
//! the counter of testpattern.rs, a white strip and a click every second, a flash on the
//! space bar); each test records it through the real recorder, reads it back, deletes the
//! recording and keeps only the numbers. "Save results" writes them to one file to send.

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::native::{
    self, CameraConfig, Display, RecordConfig, Region, ScreenApi, ScreenConfig, Status,
};
use crate::sessions;
use crate::testpattern::{self, Geometry};

/// Where the pattern window is: on the main display, from its top-left corner, in points.
const AT: (f64, f64) = (120.0, 140.0);
const PATTERN: (f64, f64) = (680.0, 200.0);
const FLASH: (f64, f64) = (480.0, 300.0);
/// Space around the pattern in the region test, in points.
const MARGIN: f64 = 24.0;

/// The pattern window as the recording sees it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Place {
    /// Pixels per point of the main display.
    pub scale: f64,
    /// In points.
    pub width: f64,
    pub height: f64,
    /// The window to record in the window test (0: none here).
    pub window_id: u32,
}

fn main_display() -> Result<Display, String> {
    native::displays()
        .into_iter()
        .find(|d| d.main)
        .ok_or_else(|| "No display was found.".to_string())
}

/// Opens the pattern window ("pattern": the counter; "flash": the space-bar box).
#[tauri::command]
pub async fn fieldtest_pattern(app: AppHandle, mode: String) -> Result<Place, String> {
    if let Some(open) = app.get_webview_window("pattern") {
        let _ = open.destroy();
    }
    let (w, h) = if mode == "flash" { FLASH } else { PATTERN };
    let display = main_display()?;
    let window = WebviewWindowBuilder::new(&app, "pattern", WebviewUrl::App("index.html".into()))
        .title("KreoKit Rec test pattern")
        .initialization_script(format!(
            "window.__KREOKIT_PATTERN__ = {{ mode: {mode:?} }};"
        ))
        .decorations(false)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(false)
        .position(display.x + AT.0, display.y + AT.1)
        .inner_size(w, h)
        .focused(true)
        .build()
        .map_err(|e| format!("The test window can't open ({e}). Try again."))?;
    // Shown and drawing before the recording starts.
    tokio_sleep(Duration::from_millis(800)).await;
    let _ = window.set_focus();
    #[cfg(windows)]
    let window_id = window.hwnd().map_or(0, |h| h.0 as usize as u32);
    #[cfg(target_os = "macos")]
    let window_id = {
        let (tx, rx) = std::sync::mpsc::channel();
        let ns = window.ns_window().map(|p| p as usize).unwrap_or(0);
        let _ = app.run_on_main_thread(move || {
            let _ = tx.send(native::window_number(ns as *mut std::ffi::c_void));
        });
        rx.recv_timeout(Duration::from_secs(2)).unwrap_or(0)
    };
    // Linux: the system's dialog chooses the window.
    #[cfg(target_os = "linux")]
    let window_id = 1;
    Ok(Place {
        scale: display.width as f64 / display.points_width.max(1.0),
        width: w,
        height: h,
        window_id,
    })
}

#[tauri::command]
pub fn fieldtest_pattern_close(app: AppHandle) {
    if let Some(open) = app.get_webview_window("pattern") {
        let _ = open.destroy();
    }
}

async fn tokio_sleep(d: Duration) {
    let _ = tauri::async_runtime::spawn_blocking(move || std::thread::sleep(d)).await;
}

/// What this computer offers: for the results file and the first step.
#[tauri::command]
pub async fn fieldtest_info() -> Value {
    tauri::async_runtime::spawn_blocking(|| {
        let encoder = if cfg!(target_os = "macos") {
            Ok("VideoToolbox".to_string())
        } else {
            crate::encoder::video_encoder().map(String::from)
        };
        json!({
            "app": env!("CARGO_PKG_VERSION"),
            "os": std::env::consts::OS,
            "os_version": os_version(),
            "arch": std::env::consts::ARCH,
            "cpus": std::thread::available_parallelism().map_or(0, |n| n.get()),
            "permissions": {
                "screen": native::screen_access(),
                "mic": native::mic_access(),
                "camera": native::camera_access(),
            },
            "displays": native::displays(),
            "mics": native::mics().iter().map(|m| json!({ "name": m.name, "default": m.default })).collect::<Vec<_>>(),
            "cameras": native::cameras().iter().map(|c| json!({ "name": c.name, "width": c.width, "height": c.height })).collect::<Vec<_>>(),
            "computer_sound": native::system_audio_supported(),
            "windows": native::window_capture_supported(),
            "encoder": encoder.unwrap_or_else(|e| format!("none: {e}")),
        })
    })
    .await
    .unwrap_or(Value::Null)
}

fn os_version() -> String {
    let run = |cmd: &str, args: &[&str]| {
        std::process::Command::new(cmd)
            .args(args)
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default()
    };
    if cfg!(target_os = "macos") {
        run("sw_vers", &["-productVersion"])
    } else if cfg!(windows) {
        run("cmd", &["/C", "ver"])
    } else {
        let release = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
        let name = release
            .lines()
            .find_map(|l| l.strip_prefix("PRETTY_NAME="))
            .unwrap_or("Linux")
            .trim_matches('"')
            .to_string();
        let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_default();
        let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
        format!("{name}, {desktop} {session}")
    }
}

fn even(v: f64) -> u32 {
    (v.round() as u32 & !1).max(2)
}

fn screen(
    display: &Display,
    region: Option<Region>,
    window: Option<u32>,
    w: u32,
    h: u32,
) -> ScreenConfig {
    ScreenConfig {
        display_id: display.id,
        region,
        window,
        exclude_windows: Vec::new(),
        width: w,
        height: h,
        fps: 30,
        capture_fps: 60,
        api: ScreenApi::Auto,
        bitrate: sessions::video_bitrate(w, h),
    }
}

/// A folder for one test's recording, deleted after it is read back.
fn test_dir(app: &AppHandle, name: &str) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("tests")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

fn record(config: &RecordConfig, seconds: f64) -> Result<Status, String> {
    let rec = native::Recorder::start(config)?;
    let start = std::time::Instant::now();
    while start.elapsed().as_secs_f64() < seconds + 0.6 {
        std::thread::sleep(Duration::from_millis(100));
    }
    let (status, error) = rec.stop();
    match error {
        Some(e) => Err(e),
        None => Ok(status),
    }
}

/// Runs one test and returns its result: `passed`, a sentence for the screen, the numbers.
#[tauri::command]
pub async fn fieldtest_run(
    app: AppHandle,
    test: String,
    place: Option<Place>,
) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let dir = test_dir(&app, &test)?;
        let result = run(&dir, &test, place.as_ref());
        let _ = std::fs::remove_dir_all(&dir);
        result
    })
    .await
    .map_err(|e| e.to_string())?
}

fn run(dir: &std::path::Path, test: &str, place: Option<&Place>) -> Result<Value, String> {
    let display = main_display()?;
    let pattern_geometry = |x: f64, y: f64, width_px: f64| Geometry {
        x,
        y,
        cell: (width_px / 34.0).floor(),
        rate: 60.0,
        still_periods: true,
    };
    let need_place = || place.ok_or("The test window isn't open.".to_string());
    match test {
        // Item 2: the whole display at the app's 1080p, the pattern read where it is.
        "screen" => {
            let p = need_place()?;
            let (w, h) = sessions::output_size(display.width, display.height, 1080);
            let s = w as f64 / display.width as f64 * p.scale;
            let config = RecordConfig {
                hold: false,
                dir: dir.to_path_buf(),
                screen: Some(screen(&display, None, None, w, h)),
                mic: None,
                camera: None,
                system_audio: false,
                synthetic: None,
            };
            let status = record(&config, 30.0)?;
            let g = pattern_geometry(AT.0 * s, AT.1 * s, p.width * s);
            fresh_result(dir, &status, &g, "screen")
        }
        // Items 3 and 4: a region around the pattern, at the display's own pixels.
        "region" => {
            let p = need_place()?;
            let region = Region {
                x: AT.0 - MARGIN,
                y: AT.1 - MARGIN,
                width: p.width + 2.0 * MARGIN,
                height: p.height + 2.0 * MARGIN,
            };
            let (w, h) = (even(region.width * p.scale), even(region.height * p.scale));
            let config = RecordConfig {
                hold: false,
                dir: dir.to_path_buf(),
                screen: Some(screen(&display, Some(region), None, w, h)),
                mic: None,
                camera: None,
                system_audio: false,
                synthetic: None,
            };
            let status = record(&config, 20.0)?;
            let g = pattern_geometry(MARGIN * p.scale, MARGIN * p.scale, p.width * p.scale);
            fresh_result(dir, &status, &g, "region")
        }
        // Item 4b: the pattern window itself.
        "window" => {
            let p = need_place()?;
            let (w, h) = (even(p.width * p.scale), even(p.height * p.scale));
            let config = RecordConfig {
                hold: false,
                dir: dir.to_path_buf(),
                screen: Some(screen(&display, None, Some(p.window_id), w, h)),
                mic: None,
                camera: None,
                system_audio: false,
                synthetic: None,
            };
            let status = record(&config, 20.0)?;
            let g = pattern_geometry(0.0, 0.0, p.width * p.scale);
            fresh_result(dir, &status, &g, "window")
        }
        // Item 5: the space-bar box flashes, the microphone hears the key.
        "sync" => {
            let p = need_place()?;
            let (w, h) = (even(p.width * p.scale), even(p.height * p.scale));
            let screen = if cfg!(target_os = "linux") && native::window_capture_supported() {
                screen(&display, None, Some(p.window_id), w, h)
            } else {
                let region = Region {
                    x: AT.0,
                    y: AT.1,
                    width: p.width,
                    height: p.height,
                };
                screen(&display, Some(region), None, w, h)
            };
            let config = RecordConfig {
                hold: false,
                dir: dir.to_path_buf(),
                screen: Some(screen),
                mic: Some(String::new()),
                camera: None,
                system_audio: false,
                synthetic: None,
            };
            let status = record(&config, 30.0)?;
            // The middle of the box (the text is at its top and bottom).
            let rect = [w / 4, h * 3 / 8, w / 2, h / 4];
            let r = crate::bench::flash_report(dir, &rect)?;
            let median = r["flash_minus_sound_ms"]["median"].as_f64();
            let matched = r["flash_minus_sound_ms"]["matched"].as_u64().unwrap_or(0);
            // Within a frame, plus up to 2 frames of the display's own delay (M2: 32 ms).
            let passed = matched >= 10 && median.is_some_and(|m| (-40.0..=110.0).contains(&m));
            Ok(json!({
                "passed": passed,
                "summary": match median {
                    Some(m) if matched >= 10 => format!("{matched} presses matched; picture minus sound {m:.0} ms (the display adds ~30 ms)."),
                    _ => format!("Only {matched} presses were both seen and heard. Press the space bar firmly, close to the microphone."),
                },
                "recorder": status,
                "numbers": r,
            }))
        }
        // Item 6: claps in front of the camera.
        "camera" => {
            let camera = native::cameras()
                .into_iter()
                .find(|c| c.default)
                .ok_or("No camera was found. Connect one, or skip this test.")?;
            let (cw, ch) = sessions::camera_size(camera.width, camera.height);
            let config = RecordConfig {
                hold: false,
                dir: dir.to_path_buf(),
                screen: None,
                mic: Some(String::new()),
                camera: Some(CameraConfig {
                    id: camera.id.clone(),
                    width: cw,
                    height: ch,
                    bitrate: sessions::video_bitrate(cw, ch),
                }),
                system_audio: false,
                synthetic: None,
            };
            let status = record(&config, 25.0)?;
            let r = crate::bench::clap_report(dir, &status)?;
            let median = r["camera_minus_sound_ms"]["median"].as_f64();
            let matched = r["camera_minus_sound_ms"]["matched"].as_u64().unwrap_or(0);
            let problems = r["check"]["problems"].as_array().map_or(0, Vec::len);
            let passed = matched >= 5 && problems == 0 && status.camera_frames_dropped == 0;
            Ok(json!({
                "passed": passed,
                "summary": match median {
                    Some(m) if matched >= 5 => format!("{matched} claps matched; camera minus sound {m:.0} ms."),
                    _ => format!("Only {matched} claps were both seen and heard. Clap clearly in view of the camera."),
                },
                "camera": { "name": camera.name, "width": cw, "height": ch },
                "recorder": status,
                "numbers": r,
            }))
        }
        // Item 9: the pattern window clicks every second; the computer sound must have the
        // clicks, in time with the white strip.
        "sound" => {
            let p = need_place()?;
            let (w, h) = (even(p.width * p.scale), even(p.height * p.scale));
            let screen = if cfg!(target_os = "linux") && native::window_capture_supported() {
                screen(&display, None, Some(p.window_id), w, h)
            } else {
                let region = Region {
                    x: AT.0,
                    y: AT.1,
                    width: p.width,
                    height: p.height,
                };
                screen(&display, Some(region), None, w, h)
            };
            let config = RecordConfig {
                hold: false,
                dir: dir.to_path_buf(),
                screen: Some(screen),
                mic: None,
                camera: None,
                system_audio: true,
                synthetic: None,
            };
            let status = record(&config, 15.0)?;
            // The sound test's pattern doesn't stand still.
            let g = Geometry {
                still_periods: false,
                ..pattern_geometry(0.0, 0.0, p.width * p.scale)
            };
            let frames = testpattern::read_frames(&dir.join("screen.mp4"), &g)?;
            let (samples, start) = testpattern::read_audio(&dir.join("system.mp4"))?;
            let clicks = testpattern::onsets(
                &samples,
                start,
                crate::bench::click_threshold(&samples),
                0.3,
            );
            let report = testpattern::report(&frames, &clicks, &g, 30.0);
            let check = crate::check::check(dir, Some(&status), 30);
            let passed = clicks.len() >= 8 && check.problems.is_empty();
            Ok(json!({
                "passed": passed,
                "summary": if clicks.len() >= 8 {
                    format!("{} clicks recorded; sound against the white strip {:.0} to {:.0} ms.", clicks.len(), report.sync_ms_min.unwrap_or(0.0), report.sync_ms_max.unwrap_or(0.0))
                } else {
                    format!("Only {} clicks were recorded. Turn the sound on and up a little, then try again.", clicks.len())
                },
                "recorder": status,
                "file": report,
                "check": check,
            }))
        }
        _ => Err(format!("unknown test {test}")),
    }
}

/// Fresh frames against the pattern (items 2-4b): every frame readable, ≥ 99 % fresh.
fn fresh_result(
    dir: &std::path::Path,
    status: &Status,
    g: &Geometry,
    what: &str,
) -> Result<Value, String> {
    let frames = testpattern::read_frames(&dir.join("screen.mp4"), g)?;
    let report = testpattern::report(&frames, &[], g, 30.0);
    let check = crate::check::check(dir, Some(status), 30);
    let readable = report.frames > 0 && report.bad == 0;
    let passed = readable && report.fresh_percent >= 99.0 && check.problems.is_empty();
    let summary = if !readable {
        format!(
            "The test pattern couldn't be read in {} of {} pictures: the {what} recording is black or shows the wrong place.",
            report.bad.max(1),
            report.frames
        )
    } else {
        format!(
            "{:.2} % fresh pictures ({} of {}).",
            report.fresh_percent,
            report.fresh,
            report.fresh + report.stale
        )
    };
    Ok(json!({
        "passed": passed,
        "summary": summary,
        "recorder": status,
        "file": report,
        "check": check,
    }))
}

/// Writes the results into the Downloads folder (or the home folder) and returns its path.
#[tauri::command]
pub fn fieldtest_save(app: AppHandle, results: Value) -> Result<String, String> {
    let dir = app
        .path()
        .download_dir()
        .or_else(|_| app.path().home_dir())
        .map_err(|e| e.to_string())?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let path = dir.join(format!(
        "kreokit-rec-test-results-{}-{stamp}.json",
        std::env::consts::OS
    ));
    let text = serde_json::to_string_pretty(&results).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| {
        format!("The results couldn't be saved ({e}). Check that the Downloads folder exists.")
    })?;
    Ok(path.to_string_lossy().into_owned())
}

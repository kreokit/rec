#[cfg(any(windows, target_os = "linux"))]
mod audio;
mod awake;
#[cfg(feature = "bench")]
mod bench;
mod check;
mod decode;
mod disk;
mod encoder;
mod export;
mod ffmpeg;
#[cfg(feature = "bench")]
mod fieldtest;
mod finish;
mod floating;
/// Releases have no Tests screen: its commands only answer that.
#[cfg(not(feature = "bench"))]
mod fieldtest {
    const ONLY: &str = "The tests are only in the test version of KreoKit Rec.";
    #[tauri::command]
    pub fn fieldtest_pattern(mode: String) -> Result<(), String> {
        let _ = mode;
        Err(ONLY.into())
    }
    #[tauri::command]
    pub fn fieldtest_pattern_close() {}
    #[tauri::command]
    pub fn fieldtest_info() -> Option<()> {
        None
    }
    #[tauri::command]
    pub fn fieldtest_run(test: String) -> Result<(), String> {
        let _ = test;
        Err(ONLY.into())
    }
    #[tauri::command]
    pub fn fieldtest_save() -> Result<(), String> {
        Err(ONLY.into())
    }
}
mod jpeg;
mod layout;
mod loudness;
mod native;
mod pipeline;
mod polish;
mod preview;
mod probe;
mod recovery;
mod rnnoise;
mod sessions;
mod sound;
mod synthetic;
#[cfg(any(test, feature = "bench"))]
mod testpattern;
mod waveform;

use std::path::PathBuf;
#[cfg(any(target_os = "macos", windows))]
use std::sync::Arc;
#[cfg(any(target_os = "macos", windows))]
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

use native::{Access, Camera, Display, Mic, Region};
use sessions::{Live, Options, Recordings, Session};

/// Runs blocking work (the recorder, FFmpeg, file I/O) off the main thread.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| e.to_string())?
}

/// `KREOKIT_REC_SYNTHETIC=1` (debug and test builds): a made-up display and microphone,
/// and recordings from the synthetic sources, so the app can be tried without recording
/// the real screen or microphone. `=ask` also shows the permissions as not yet allowed.
fn synthetic() -> Option<String> {
    if cfg!(any(debug_assertions, feature = "bench")) {
        std::env::var("KREOKIT_REC_SYNTHETIC").ok()
    } else {
        None
    }
}

/// Whether the app's own windows can be kept out of display and region recordings.
/// `KREOKIT_REC_NO_EXCLUSION=1` (debug and test builds) acts as a system that can't, to try
/// the menu bar / tray path on a Mac that can.
fn exclusion_available() -> bool {
    let off = cfg!(any(debug_assertions, feature = "bench"))
        && std::env::var("KREOKIT_REC_NO_EXCLUSION").is_ok_and(|v| v == "1");
    !off && (synthetic().is_some() || native::exclusion_supported())
}

/// The one Recordings for the app, in the app data folder's "recordings".
fn recordings(app: &AppHandle) -> tauri::State<'_, Recordings> {
    app.state::<Recordings>()
}

#[derive(Serialize)]
struct Permissions {
    /// "macos", "windows" or "linux": which settings the texts name.
    os: &'static str,
    screen: Access,
    mic: Access,
    camera: Access,
    /// macOS 12 applies a new Screen Recording permission only after a restart.
    restart_after_screen: bool,
    /// Screen Recording was requested in an earlier run and still isn't allowed: either
    /// the box wasn't ticked, or macOS ties it to an older copy of the app (ad-hoc signed
    /// apps change their signature with every update, and the entry no longer matches).
    screen_asked_before: bool,
}

/// Remembers that Screen Recording was requested (in the app data folder), so the next
/// start can tell "never asked" from "asked, but it didn't take effect".
fn screen_marker(app: &AppHandle) -> Option<PathBuf> {
    Some(
        app.path()
            .app_data_dir()
            .ok()?
            .join("screen-permission-asked"),
    )
}

/// The macOS major version (0 elsewhere).
fn macos_major() -> u32 {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .ok()
            .and_then(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .split('.')
                    .next()?
                    .trim()
                    .parse()
                    .ok()
            })
            .unwrap_or(0)
    }
    #[cfg(not(target_os = "macos"))]
    {
        0
    }
}

#[tauri::command]
fn permissions(app: AppHandle) -> Permissions {
    if let Some(mode) = synthetic() {
        let access = if mode == "ask" || mode == "stuck" {
            Access::NotAsked
        } else {
            Access::Allowed
        };
        return Permissions {
            os: std::env::consts::OS,
            screen: access,
            mic: access,
            camera: access,
            restart_after_screen: true,
            screen_asked_before: mode == "stuck",
        };
    }
    let screen = native::screen_access();
    let marker = screen_marker(&app);
    // Kept once written: after an update, a permission given to the old copy no longer
    // matches, and this is how the next start knows to offer the reset.
    let asked = marker.as_ref().is_some_and(|m| m.is_file());
    Permissions {
        os: std::env::consts::OS,
        screen,
        mic: native::mic_access(),
        camera: native::camera_access(),
        restart_after_screen: macos_major() == 12,
        screen_asked_before: asked && screen != Access::Allowed,
    }
}

/// Shows the system's microphone question (first time only); returns the answer.
#[tauri::command]
async fn request_mic() -> Result<Access, String> {
    if synthetic().is_some() {
        return Ok(Access::Allowed);
    }
    blocking(|| Ok(native::request_mic_access())).await
}

/// Shows the system's camera question (first time only); returns the answer.
#[tauri::command]
async fn request_camera() -> Result<Access, String> {
    if synthetic().is_some() {
        return Ok(Access::Allowed);
    }
    blocking(|| Ok(native::request_camera_access())).await
}

/// The system's screen-recording request (macOS shows it once, then opens nothing).
#[tauri::command]
fn request_screen(app: AppHandle) {
    if let Some(marker) = screen_marker(&app) {
        let _ = std::fs::create_dir_all(marker.parent().unwrap());
        let _ = std::fs::write(marker, b"");
    }
    if synthetic().is_none() {
        native::request_screen_access();
    }
}

/// Removes macOS's Screen Recording entry for this app (its own entry only) and asks
/// again, so the entry matches the copy that is running now. For when allowing didn't
/// take effect.
#[tauri::command]
fn reset_screen_permission(app: AppHandle) -> Result<(), String> {
    if synthetic().is_none() {
        #[cfg(target_os = "macos")]
        {
            let id = app.config().identifier.clone();
            let ok = std::process::Command::new("tccutil")
                .args(["reset", "ScreenCapture", &id])
                .status()
                .is_ok_and(|s| s.success());
            if !ok {
                return Err("The old permission couldn't be removed automatically. In System Preferences, select KreoKit Rec under Screen Recording, click –, then click + and choose KreoKit Rec in Applications.".into());
            }
        }
    }
    request_screen(app);
    Ok(())
}

/// Opens System Preferences (System Settings) at Screen Recording, Microphone or Camera.
#[tauri::command]
fn open_privacy_settings(kind: String) -> Result<(), String> {
    let pane = match kind.as_str() {
        "screen" => "Privacy_ScreenCapture",
        "mic" => "Privacy_Microphone",
        "camera" => "Privacy_Camera",
        _ => return Err("unknown setting".into()),
    };
    #[cfg(target_os = "macos")]
    if synthetic().is_none() {
        std::process::Command::new("open")
            .arg(format!(
                "x-apple.systempreferences:com.apple.preference.security?{pane}"
            ))
            .status()
            .map_err(|e| {
                format!("System Preferences didn't open ({e}). Open it from the Apple menu, then Security & Privacy > Privacy.")
            })?;
    }
    // Windows: the camera and microphone privacy switches (the screen needs none).
    #[cfg(windows)]
    if synthetic().is_none() && kind != "screen" {
        let page = if kind == "camera" {
            "ms-settings:privacy-webcam"
        } else {
            "ms-settings:privacy-microphone"
        };
        std::process::Command::new("explorer.exe")
            .arg(page)
            .spawn()
            .map_err(|e| {
                format!("Settings didn't open ({e}). Open Settings, then Privacy & security.")
            })?;
    }
    let _ = pane;
    Ok(())
}

#[tauri::command]
fn restart_app(app: AppHandle) {
    app.restart();
}

#[derive(Serialize)]
struct Devices {
    displays: Vec<Display>,
    mics: Vec<Mic>,
    cameras: Vec<Camera>,
    /// The computer sound can be recorded; otherwise why not.
    system_audio: bool,
    system_audio_note: Option<String>,
    /// Windows can be recorded; otherwise why not, for the setup screen.
    windows: bool,
    windows_note: Option<String>,
    /// A region can be chosen; otherwise why not.
    regions: bool,
    regions_note: Option<String>,
    /// The app's own windows can be kept out of display and region recordings (otherwise
    /// the controls go to the menu bar or tray while recording).
    own_windows_kept_out: bool,
    /// "macos", "windows" or "linux".
    os: &'static str,
}

/// Why the computer sound can't be recorded here (`None`: it can).
fn system_audio_note() -> Option<String> {
    if native::system_audio_supported() {
        return None;
    }
    Some(
        if cfg!(target_os = "macos") {
            "Needs macOS 13 or newer"
        } else if cfg!(windows) {
            "No speakers or headphones found"
        } else {
            "Needs PipeWire and speakers or headphones"
        }
        .into(),
    )
}

/// Whether a region can be chosen: on Linux only on X11 without the screen portal (the
/// portal's dialog chooses a whole display or window).
fn region_support() -> (bool, Option<String>) {
    if cfg!(target_os = "linux") && native::window_capture_supported() {
        return (
            false,
            Some(
                "Choosing a region arrives later on this desktop. Record the screen or a window."
                    .into(),
            ),
        );
    }
    (true, None)
}

/// Window capture on macOS 12 (ScreenCaptureKit, 12.3+): whether it's reliable enough to
/// offer. Measured on the dev Mac (12.7.6, 2026-10-27, docs/m3-results.md): 100 % and
/// 99.38 % fresh frames, above the 99 % target; set to false if a later measurement fails.
const WINDOWS_ON_MACOS_12: bool = true;

fn window_support() -> (bool, Option<String>) {
    if !native::window_capture_supported() {
        let note = if cfg!(target_os = "macos") {
            "Recording a window needs macOS 12.3 or newer. Record a region instead."
        } else if cfg!(windows) {
            "Recording a window needs Windows 10 version 1903 or newer. Record a region instead."
        } else {
            "This desktop can't record single windows. Record the screen instead."
        };
        return (false, Some(note.into()));
    }
    if !native::system_audio_supported() && !WINDOWS_ON_MACOS_12 {
        return (
            false,
            Some("Recording a window isn't reliable on macOS 12 (it misses pictures). Record a region around the window instead, or update to macOS 13 or newer.".into()),
        );
    }
    (true, None)
}

#[tauri::command]
async fn devices() -> Result<Devices, String> {
    if synthetic().is_some() {
        return Ok(Devices {
            displays: vec![synthetic_display()],
            mics: vec![Mic {
                id: "synthetic".into(),
                name: "Test microphone".into(),
                default: true,
            }],
            cameras: vec![Camera {
                id: "synthetic".into(),
                name: "Test camera".into(),
                default: true,
                width: 1280,
                height: 720,
            }],
            system_audio: true,
            system_audio_note: None,
            windows: true,
            windows_note: None,
            regions: true,
            regions_note: None,
            own_windows_kept_out: exclusion_available(),
            os: std::env::consts::OS,
        });
    }
    blocking(|| {
        let (windows, windows_note) = window_support();
        let (regions, regions_note) = region_support();
        Ok(Devices {
            displays: native::displays(),
            mics: native::mics(),
            cameras: native::cameras(),
            system_audio: native::system_audio_supported(),
            system_audio_note: system_audio_note(),
            windows,
            windows_note,
            regions,
            regions_note,
            own_windows_kept_out: exclusion_available(),
            os: std::env::consts::OS,
        })
    })
    .await
}

/// The made-up display of synthetic test runs: small and away from the corner, so a test
/// instance's region overlay covers only this part of the real screen.
fn synthetic_display() -> Display {
    Display {
        id: 0,
        name: "Test display".into(),
        width: 1920,
        height: 1200,
        main: true,
        x: 240.0,
        y: 160.0,
        points_width: 960.0,
        points_height: 600.0,
    }
}

fn display_list() -> Vec<Display> {
    if synthetic().is_some() {
        vec![synthetic_display()]
    } else {
        native::displays()
    }
}

/// The windows that can be recorded now (asked each time the window choice opens).
#[tauri::command]
async fn windows() -> Result<Vec<native::Window>, String> {
    if synthetic().is_some() {
        return Ok(vec![native::Window {
            id: 7,
            app: "Test app".into(),
            title: "Test window".into(),
            width: 640.0,
            height: 400.0,
            scale: 2.0,
        }]);
    }
    blocking(|| Ok(native::windows())).await
}

#[derive(Serialize)]
struct Estimate {
    width: u32,
    height: u32,
    bytes_per_minute: u64,
    free_bytes: Option<u64>,
}

/// The output size and file size per minute for a display at a quality (plus the camera
/// at its size, 0 × 0 when off), and the space left for recordings.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
fn estimate(
    app: AppHandle,
    width: u32,
    height: u32,
    points_width: f64,
    region: Option<native::Region>,
    quality: u32,
    camera_width: u32,
    camera_height: u32,
    mic: bool,
    window: Option<sessions::WindowChoice>,
    system_audio: Option<bool>,
) -> Estimate {
    let (source_w, source_h) = match &window {
        Some(w) => sessions::window_size(w),
        None => sessions::source_size(width, height, points_width, region),
    };
    let (w, h) = sessions::output_size(source_w, source_h, quality);
    let camera = (camera_width > 0).then(|| sessions::camera_size(camera_width, camera_height));
    let root = recordings(&app).root().to_path_buf();
    let _ = std::fs::create_dir_all(&root);
    Estimate {
        width: w,
        height: h,
        bytes_per_minute: sessions::bytes_per_minute(
            w,
            h,
            camera,
            mic,
            system_audio.unwrap_or(false),
        ),
        free_bytes: disk::free_bytes(&root),
    }
}

/// What the region overlay needs: the display (points and pixels) and the region chosen
/// before, if any (window.__KREOKIT_REGION__ in the overlay).
#[derive(Serialize)]
struct RegionSetup<'a> {
    display: &'a Display,
    region: Option<Region>,
}

/// Opens the see-through overlay over `display` to drag a region; the answer comes back to
/// the main window as the "region-chosen" event (`region_done`).
#[tauri::command]
async fn region_open(
    app: AppHandle,
    display: Display,
    region: Option<Region>,
) -> Result<(), String> {
    if let Some(open) = app.get_webview_window("region") {
        let _ = open.destroy();
    }
    let setup = serde_json::to_string(&RegionSetup {
        display: &display,
        region,
    })
    .map_err(|e| e.to_string())?;
    let window = WebviewWindowBuilder::new(&app, "region", WebviewUrl::App("index.html".into()))
        .title("Choose a region")
        .initialization_script(format!("window.__KREOKIT_REGION__ = {setup};"))
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false)
        .position(display.x, display.y)
        .inner_size(display.points_width, display.points_height)
        .build()
        .map_err(|e| {
            format!("The region can't be chosen ({e}). Try again, or record the whole display.")
        })?;
    #[cfg(target_os = "macos")]
    {
        let ns_window = window.ns_window().map_err(|e| e.to_string())? as usize;
        window
            .run_on_main_thread(move || native::overlay_window(ns_window as *mut _))
            .map_err(|e| e.to_string())?;
    }
    window.show().map_err(|e| e.to_string())?;
    window.set_focus().map_err(|e| e.to_string())?;
    Ok(())
}

/// The overlay's answer: the region, or `None` when cancelled.
#[tauri::command]
fn region_done(app: AppHandle, region: Option<Region>) -> Result<(), String> {
    app.emit_to("main", "region-chosen", region)
        .map_err(|e| e.to_string())?;
    if let Some(window) = app.get_webview_window("region") {
        window.destroy().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
async fn monitor_start(mic_id: String) -> Result<(), String> {
    if synthetic().is_some() {
        return Ok(());
    }
    blocking(move || native::monitor_start(&mic_id)).await
}

#[tauri::command]
fn monitor_level() -> f32 {
    if synthetic().is_some() {
        // A level that moves like speech, for the meter.
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f32());
        return 0.05 + 0.25 * ((t * 3.1).sin() * (t * 7.3).sin()).abs();
    }
    native::monitor_peak()
}

#[tauri::command]
async fn monitor_stop() -> Result<(), String> {
    blocking(|| {
        native::monitor_stop();
        Ok(())
    })
    .await
}

/// The camera preview on Home: 320 pixels wide in the camera's shape (`width` × `height`).
#[tauri::command]
async fn camera_preview_start(camera_id: String, width: u32, height: u32) -> Result<(), String> {
    let (w, h) = preview_size(width, height);
    let synthetic = synthetic().is_some();
    blocking(move || native::preview_start(&camera_id, w, h, synthetic)).await
}

fn preview_size(width: u32, height: u32) -> (u32, u32) {
    if width == 0 || height == 0 {
        return (320, 180);
    }
    (
        320,
        ((320.0 * height as f64 / width as f64).round() as u32 & !1).max(2),
    )
}

/// The newest preview picture if newer than `after`: its number (8 bytes, little-endian)
/// followed by the JPEG; nothing when there is no new picture.
#[tauri::command]
fn camera_preview_frame(after: u64) -> tauri::ipc::Response {
    let mut seq = after;
    let mut out = Vec::new();
    if let Some(jpeg) = native::preview_frame(&mut seq) {
        out.extend_from_slice(&seq.to_le_bytes());
        out.extend_from_slice(&jpeg);
    }
    tauri::ipc::Response::new(out)
}

#[tauri::command]
async fn camera_preview_stop(app: AppHandle) -> Result<(), String> {
    blocking(move || {
        // While recording the bubble reads the recorder's own pictures; they end with it.
        if !recordings(&app).is_recording() {
            native::preview_stop();
        }
        Ok(())
    })
    .await
}

/// The app's own windows, to keep out of the recording where the system allows it
/// (docs/m5-results.md). macOS: their numbers, for the recorder. Windows: each window is
/// marked while recording (`on`; unmarked again at the stop, so screenshots of the app
/// work otherwise). Linux: nothing can keep them out.
#[allow(unused_variables)]
fn own_windows(app: &AppHandle, on: bool) -> Vec<u32> {
    #[cfg(target_os = "macos")]
    {
        if !on || !exclusion_available() {
            return Vec::new();
        }
        let handles: Vec<usize> = app
            .webview_windows()
            .values()
            .filter_map(|w| w.ns_window().ok().map(|p| p as usize))
            .collect();
        let (tx, rx) = std::sync::mpsc::channel();
        let _ = app.run_on_main_thread(move || {
            let numbers = handles
                .iter()
                .map(|&ns| native::window_number(ns as *mut std::ffi::c_void))
                .filter(|&n| n != 0)
                .collect::<Vec<u32>>();
            let _ = tx.send(numbers);
        });
        rx.recv_timeout(std::time::Duration::from_secs(2))
            .unwrap_or_default()
    }
    #[cfg(windows)]
    {
        if exclusion_available() {
            for window in app.webview_windows().values() {
                if let Ok(hwnd) = window.hwnd() {
                    native::exclude_from_capture(hwnd.0 as isize, on);
                }
            }
        }
        Vec::new()
    }
    #[cfg(target_os = "linux")]
    Vec::new()
}

/// Waits without holding up the async runtime.
async fn wait_ms(d: std::time::Duration) {
    let _ = tauri::async_runtime::spawn_blocking(move || std::thread::sleep(d)).await;
}

/// The countdown's size in points (a square in the middle of what is recorded).
const COUNTDOWN_SIZE: f64 = 220.0;

/// Opens the countdown over the middle of the recorded display (or region). It counts on
/// its own and then calls `countdown_done` (or `countdown_cancel` on Esc).
fn countdown_open(
    app: &AppHandle,
    display: &Display,
    region: Option<Region>,
) -> Result<(), String> {
    if let Some(open) = app.get_webview_window("countdown") {
        let _ = open.destroy();
    }
    let (cx, cy) = match region {
        Some(r) => (r.x + r.width / 2.0, r.y + r.height / 2.0),
        None => (display.points_width / 2.0, display.points_height / 2.0),
    };
    let window = WebviewWindowBuilder::new(app, "countdown", WebviewUrl::App("index.html".into()))
        .title("Countdown")
        .initialization_script("window.__KREOKIT_COUNTDOWN__ = true;")
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false)
        .position(
            display.x + cx - COUNTDOWN_SIZE / 2.0,
            display.y + cy - COUNTDOWN_SIZE / 2.0,
        )
        .inner_size(COUNTDOWN_SIZE, COUNTDOWN_SIZE)
        .build()
        .map_err(|e| {
            format!("The countdown can't open ({e}). Turn the countdown off, then record again.")
        })?;
    #[cfg(target_os = "macos")]
    {
        let ns_window = window.ns_window().map_err(|e| e.to_string())? as usize;
        window
            .run_on_main_thread(move || native::overlay_window(ns_window as *mut _))
            .map_err(|e| e.to_string())?;
    }
    window.show().map_err(|e| e.to_string())?;
    window.set_focus().map_err(|e| e.to_string())?;
    Ok(())
}

/// The display each floating window ("control", "bubble") is on while recording.
static FLOATING_ON: std::sync::Mutex<Vec<(&'static str, Display)>> =
    std::sync::Mutex::new(Vec::new());

/// Opens a floating window (`label`, its page chosen by `flag`) at (x, y) points: above
/// other windows, on every Space, and on macOS a non-activating panel, so clicking it leaves
/// the keyboard with the app being recorded (docs/m5-results.md).
fn floating_open(
    app: &AppHandle,
    label: &'static str,
    flag: &str,
    display: &Display,
    (x, y, w, h): (f64, f64, f64, f64),
) -> Result<(), String> {
    floating_close(app, label);
    let window = WebviewWindowBuilder::new(app, label, WebviewUrl::App("index.html".into()))
        .title("KreoKit Rec")
        .initialization_script(format!("window.{flag} = true;"))
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .resizable(false)
        .always_on_top(true)
        .visible_on_all_workspaces(true)
        .skip_taskbar(true)
        .focusable(false)
        .focused(false)
        .accept_first_mouse(true)
        .visible(false)
        .position(x, y)
        .inner_size(w, h)
        .build()
        .map_err(|e| format!("The recording controls can't open ({e}). Try again."))?;
    #[cfg(target_os = "macos")]
    {
        let ns_window = window.ns_window().map_err(|e| e.to_string())? as usize;
        window
            .run_on_main_thread(move || native::floating_window(ns_window as *mut _))
            .map_err(|e| e.to_string())?;
    }
    window.show().map_err(|e| e.to_string())?;
    FLOATING_ON.lock().unwrap().push((label, display.clone()));
    Ok(())
}

/// Closes a floating window; returns its display and where it was left on it (points
/// from the display's top-left) to remember.
fn floating_close(app: &AppHandle, label: &str) -> Option<(Display, (f64, f64))> {
    let window = app.get_webview_window(label)?;
    let display = {
        let mut on = FLOATING_ON.lock().unwrap();
        let i = on.iter().position(|(l, _)| *l == label);
        i.map(|i| on.remove(i).1)
    };
    let place = match (display, window.outer_position(), window.scale_factor()) {
        (Some(display), Ok(at), Ok(scale)) => {
            let at = at.to_logical::<f64>(scale);
            let offset = (at.x - display.x, at.y - display.y);
            Some((display, offset))
        }
        _ => None,
    };
    #[cfg(target_os = "macos")]
    if let Ok(ns_window) = window.ns_window() {
        // Back to the window library's own class before it closes the window.
        let ns_window = ns_window as usize;
        let (tx, rx) = std::sync::mpsc::channel();
        let _ = app.run_on_main_thread(move || {
            native::floating_window_release(ns_window as *mut _);
            let _ = tx.send(());
        });
        let _ = rx.recv_timeout(std::time::Duration::from_secs(2));
    }
    let _ = window.destroy();
    place
}

/// Moves a floating window by (dx, dy) points: dragged by hand (FloatingControl,
/// CameraBubble), as a panel that never becomes key doesn't start the system's own drag.
#[tauri::command]
fn floating_move(app: AppHandle, label: String, dx: f64, dy: f64) -> Result<(), String> {
    if label != "control" && label != "bubble" {
        return Err("Only the recording controls can be moved.".into());
    }
    let window = app
        .get_webview_window(&label)
        .ok_or("The recording controls are closed.")?;
    let scale = window.scale_factor().map_err(|e| e.to_string())?;
    let at = window
        .outer_position()
        .map_err(|e| e.to_string())?
        .to_logical::<f64>(scale);
    window
        .set_position(tauri::LogicalPosition::new(at.x + dx, at.y + dy))
        .map_err(|e| e.to_string())
}

/// Opens the floating control (time, pause, stop, mic level) where `floating.rs` says;
/// false when it would be recorded (the menu bar or tray take its place).
fn control_open(app: &AppHandle, options: &Options) -> Result<bool, String> {
    let displays = display_list();
    let Some(display) = floating::control_display(
        &displays,
        options.display_id,
        options.window.is_some(),
        exclusion_available(),
    ) else {
        return Ok(false);
    };
    let saved = read_prefs(app)
        .control_positions
        .get(&display.id.to_string())
        .copied();
    let (x, y) = floating::control_position(display, saved);
    let (w, h) = floating::CONTROL_SIZE;
    floating_open(app, "control", "__KREOKIT_CONTROL__", display, (x, y, w, h))?;
    Ok(true)
}

/// Closes the floating control and remembers where it was left on its display.
fn control_close(app: &AppHandle) {
    if let Some((display, at)) = floating_close(app, "control") {
        let _ = write_prefs(
            app,
            |p| {
                p.control_positions.insert(display.id.to_string(), at);
            },
            "Where the recording controls were left couldn't be remembered",
        );
    }
}

/// Opens the camera bubble while recording a camera, where it is kept out of the picture;
/// false otherwise. Its pictures come from the recorder (`camera_preview`).
fn bubble_open(app: &AppHandle, options: &Options) -> Result<bool, String> {
    if options.camera_id.is_none() {
        return Ok(false);
    }
    let displays = display_list();
    let Some(display) = floating::bubble_display(
        &displays,
        options.display_id,
        options.window.is_some(),
        exclusion_available(),
    ) else {
        return Ok(false);
    };
    let prefs = read_prefs(app);
    let size = floating::bubble_size(prefs.bubble_size.as_deref().unwrap_or("medium"));
    let saved = prefs.bubble_positions.get(&display.id.to_string()).copied();
    let (x, y) = floating::bubble_position(display, saved, size);
    floating_open(
        app,
        "bubble",
        "__KREOKIT_BUBBLE__",
        display,
        (x, y, size, size),
    )?;
    Ok(true)
}

fn bubble_close(app: &AppHandle) {
    if let Some((display, at)) = floating_close(app, "bubble") {
        let _ = write_prefs(
            app,
            |p| {
                p.bubble_positions.insert(display.id.to_string(), at);
            },
            "Where the camera bubble was left couldn't be remembered",
        );
    }
}

/// Small, Medium or Large; the bubble keeps its bottom-right corner, and the size is
/// remembered.
#[tauri::command]
fn bubble_resize(app: AppHandle, size: String) -> Result<(), String> {
    let window = app
        .get_webview_window("bubble")
        .ok_or("The camera bubble is closed.")?;
    let scale = window.scale_factor().map_err(|e| e.to_string())?;
    let at = window
        .outer_position()
        .map_err(|e| e.to_string())?
        .to_logical::<f64>(scale);
    let old = window
        .inner_size()
        .map_err(|e| e.to_string())?
        .to_logical::<f64>(scale)
        .width;
    let new = floating::bubble_size(&size);
    window
        .set_size(tauri::LogicalSize::new(new, new))
        .map_err(|e| e.to_string())?;
    window
        .set_position(tauri::LogicalPosition::new(
            at.x + old - new,
            at.y + old - new,
        ))
        .map_err(|e| e.to_string())?;
    write_prefs(
        &app,
        |p| p.bubble_size = Some(size),
        "The bubble's size couldn't be remembered",
    )
}

/// Hides the camera bubble for the rest of this recording (the camera keeps recording).
#[tauri::command]
fn bubble_hide(app: AppHandle) {
    bubble_close(&app);
}

/// The menu bar item (macOS) or tray icon (Windows, Linux) while recording where the
/// floating control would be recorded (docs/m5-plan.md, choice 1): the time, Pause/Resume,
/// Stop, Show KreoKit Rec. A thread keeps its time and labels current. Not on Linux
/// (THIRD-PARTY-LICENSES.md, "System libraries"): the main window steps aside to the taskbar.
#[cfg(any(target_os = "macos", windows))]
struct Tray {
    icon: tauri::tray::TrayIcon,
    pause: tauri::menu::MenuItem<tauri::Wry>,
    stop: Arc<AtomicBool>,
}

#[cfg(any(target_os = "macos", windows))]
static TRAY: std::sync::Mutex<Option<Tray>> = std::sync::Mutex::new(None);

/// Under this many minutes of space left the controls say so (docs/m5-plan.md, choice 4).
#[cfg(any(target_os = "macos", windows))]
const DISK_NOTE_MINUTES: f64 = 10.0;

/// What the menu bar shows: "● 0:12", "Paused 0:12", "Starting…".
#[cfg_attr(target_os = "linux", allow(dead_code))]
fn tray_title(state: sessions::State, elapsed: f64) -> String {
    match state {
        sessions::State::Countdown => "Starting…".into(),
        sessions::State::Paused => format!("Paused {}", format_time(elapsed)),
        _ => format!("● {}", format_time(elapsed)),
    }
}

/// 0:12, 12:05, 1:02:03.
#[cfg_attr(target_os = "linux", allow(dead_code))]
fn format_time(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[cfg(any(target_os = "macos", windows))]
fn tray_open(app: &AppHandle) -> Result<(), String> {
    use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
    use tauri::tray::TrayIconBuilder;
    tray_close();
    let e = |e: tauri::Error| e.to_string();
    let pause = MenuItem::with_id(app, "pause", "Pause", true, None::<&str>).map_err(e)?;
    let stop = MenuItem::with_id(app, "stop", "Stop", true, None::<&str>).map_err(e)?;
    let show = MenuItem::with_id(app, "show", "Show KreoKit Rec", true, None::<&str>).map_err(e)?;
    let separator = PredefinedMenuItem::separator(app).map_err(e)?;
    let menu = Menu::with_items(app, &[&pause, &stop, &separator, &show]).map_err(e)?;
    let mut builder = TrayIconBuilder::with_id("recording")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .tooltip("KreoKit Rec: recording")
        .on_menu_event(|app, event| {
            let app = app.clone();
            match event.id().as_ref() {
                "pause" => {
                    tauri::async_runtime::spawn(async move {
                        let _ = blocking(move || {
                            let r = recordings(&app);
                            let live = r.live().ok_or("Nothing is being recorded.")?;
                            match live.session.state {
                                sessions::State::Paused => r.resume(),
                                _ => r.pause(),
                            }
                        })
                        .await;
                    });
                }
                "stop" => {
                    tauri::async_runtime::spawn(async move {
                        let counting = recordings(&app)
                            .live()
                            .is_some_and(|l| l.session.state == sessions::State::Countdown);
                        if counting {
                            let _ = countdown_cancel(app).await;
                        } else {
                            let _ = stop_recording(app).await;
                        }
                    });
                }
                "show" => main_window_back(&app),
                _ => {}
            }
        });
    // The menu bar shows the time as text; Windows and Linux need an icon (time in the
    // tooltip).
    if cfg!(target_os = "macos") {
        builder = builder.title("Starting…");
    } else if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    let icon = builder.build(app).map_err(e)?;
    let stop_flag = Arc::new(AtomicBool::new(false));
    {
        let (app, stop_flag, icon, pause) =
            (app.clone(), stop_flag.clone(), icon.clone(), pause.clone());
        std::thread::spawn(move || {
            while !stop_flag.load(Ordering::SeqCst) {
                if let Some(live) = recordings(&app).live() {
                    let state = live.session.state;
                    let mut title = tray_title(state, live.status.elapsed);
                    if let Some(left) = live.minutes_left.filter(|m| *m <= DISK_NOTE_MINUTES) {
                        title.push_str(&format!(" · {} min of space left", left.floor()));
                    }
                    if cfg!(target_os = "macos") {
                        let _ = icon.set_title(Some(&title));
                    }
                    let _ = icon.set_tooltip(Some(&format!("KreoKit Rec: {title}")));
                    let _ = pause.set_text(if state == sessions::State::Paused {
                        "Resume"
                    } else {
                        "Pause"
                    });
                    let _ = pause.set_enabled(
                        state != sessions::State::Countdown && live.session.error.is_none(),
                    );
                }
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
        });
    }
    *TRAY.lock().unwrap() = Some(Tray {
        icon,
        pause,
        stop: stop_flag,
    });
    Ok(())
}

#[cfg(target_os = "linux")]
fn tray_open(_app: &AppHandle) -> Result<(), String> {
    Ok(())
}

#[cfg(target_os = "linux")]
fn tray_close() {}

#[cfg(any(target_os = "macos", windows))]
fn tray_close() {
    if let Some(tray) = TRAY.lock().unwrap().take() {
        tray.stop.store(true, Ordering::SeqCst);
        let _ = tray.icon.set_visible(false);
        drop(tray.pause);
    }
}

/// The main window steps aside while the floating control is shown...
fn main_window_aside(app: &AppHandle) {
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.minimize();
    }
}

/// ...and comes back when the recording ends.
fn main_window_back(app: &AppHandle) {
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.unminimize();
        let _ = main.show();
        let _ = main.set_focus();
    }
}

fn countdown_close(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("countdown") {
        let _ = window.destroy();
    }
}

/// Starts a recording. With a countdown, the countdown window opens first (so it is one of
/// the app's windows kept out of the picture) and the recording is held until it ends.
#[tauri::command]
async fn start_recording(
    app: AppHandle,
    mut options: Options,
    display: Option<Display>,
) -> Result<Session, String> {
    let options_countdown = options.countdown;
    if options.countdown {
        let display = display.ok_or("Choose a display, then record again.")?;
        countdown_open(&app, &display, options.region)?;
    }
    let (control, bubble) = match control_open(&app, &options)
        .and_then(|control| Ok((control, bubble_open(&app, &options)?)))
    {
        Ok(shown) => shown,
        Err(e) => {
            countdown_close(&app);
            control_close(&app);
            return Err(e);
        }
    };
    if options.countdown || control || bubble {
        // On screen before the recorder looks at which of the app's windows are shown.
        wait_ms(std::time::Duration::from_millis(150)).await;
    }
    let started = blocking({
        let app = app.clone();
        move || {
            native::monitor_stop();
            native::preview_stop();
            options.own_windows = own_windows(&app, true);
            recordings(&app).start(&options)
        }
    })
    .await;
    // Esc while the devices were still opening: the countdown is gone, so is the recording.
    if started.is_ok() && options_countdown && app.get_webview_window("countdown").is_none() {
        let app2 = app.clone();
        let _ = blocking(move || recordings(&app2).cancel()).await;
        control_close(&app);
        bubble_close(&app);
        own_windows(&app, false);
        return Err(COUNTDOWN_CANCELLED.into());
    }
    if bubble && started.is_ok() {
        let app2 = app.clone();
        let _ = blocking(move || Ok(recordings(&app2).camera_preview(BUBBLE_PICTURE_WIDTH))).await;
    }
    if started.is_err() {
        countdown_close(&app);
        control_close(&app);
        bubble_close(&app);
        own_windows(&app, false);
        return started;
    }
    shortcut_on(&app);
    if !control {
        // The floating control would be recorded: the menu bar or tray instead (on Linux
        // only the taskbar: the main window steps aside there too).
        if let Err(e) = tray_open(&app) {
            eprintln!("tray: {e}");
        }
    }
    main_window_aside(&app);
    started
}

/// The stop shortcut (docs/m5-plan.md, choice 3): ⌥⇧⌘S on macOS, Ctrl+Alt+Shift+S
/// elsewhere (⌥⌘S is "Save All" in VS Code and other apps people record). Registered only
/// while recording. macOS registers it with `RegisterEventHotKey` (global-hotkey crate): no
/// Accessibility or Input Monitoring permission.
fn stop_shortcut() -> tauri_plugin_global_shortcut::Shortcut {
    use tauri_plugin_global_shortcut::{Code, Modifiers, Shortcut};
    let keys = if cfg!(target_os = "macos") {
        Modifiers::ALT | Modifiers::SHIFT | Modifiers::SUPER
    } else {
        Modifiers::CONTROL | Modifiers::ALT | Modifiers::SHIFT
    };
    Shortcut::new(Some(keys), Code::KeyS)
}

/// The stop shortcut as the screens show it; `None` where global shortcuts don't work
/// (Linux on Wayland: an X11 key grab doesn't see other apps' keys there).
fn stop_shortcut_label() -> Option<&'static str> {
    if cfg!(target_os = "macos") {
        Some("⌥⇧⌘S")
    } else if cfg!(target_os = "linux")
        && std::env::var("XDG_SESSION_TYPE").is_ok_and(|t| t.eq_ignore_ascii_case("wayland"))
    {
        None
    } else {
        Some("Ctrl+Alt+Shift+S")
    }
}

/// While recording: the stop shortcut works everywhere. When another app already uses it,
/// the main window says so (recording goes on; Stop is on the controls).
fn shortcut_on(app: &AppHandle) {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    let Some(label) = stop_shortcut_label() else {
        return;
    };
    if app.global_shortcut().register(stop_shortcut()).is_err() {
        let _ = app.emit_to(
            "main",
            "shortcut-unavailable",
            format!("{label} is used by another app, so it can't stop this recording. Stop with the recording controls instead."),
        );
    }
}

fn shortcut_off(app: &AppHandle) {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    let _ = app.global_shortcut().unregister(stop_shortcut());
}

/// For the screens: the stop shortcut, or null where it doesn't work.
#[tauri::command]
fn stop_shortcut_text() -> Option<&'static str> {
    stop_shortcut_label()
}

/// The bubble's pictures: wide enough for the Large bubble on a 2x display.
const BUBBLE_PICTURE_WIDTH: u32 = 640;

/// What `start_recording` says when the countdown was cancelled before it had started (the
/// setup screen shows nothing for it).
const COUNTDOWN_CANCELLED: &str = "Cancelled before the recording started.";

/// The countdown is over: its window goes, then the recording begins (a moment later, so
/// the countdown isn't in the first picture where it can't be kept out).
#[tauri::command]
async fn countdown_done(app: AppHandle) -> Result<Session, String> {
    countdown_close(&app);
    wait_ms(std::time::Duration::from_millis(100)).await;
    let began = blocking({
        let app = app.clone();
        move || recordings(&app).begin()
    })
    .await;
    let _ = app.emit_to("main", "recording-began", &began.as_ref().ok());
    began
}

/// Esc during the countdown: nothing is recorded or kept.
#[tauri::command]
async fn countdown_cancel(app: AppHandle) -> Result<(), String> {
    countdown_close(&app);
    let cancelled = blocking({
        let app = app.clone();
        move || {
            let result = recordings(&app).cancel();
            own_windows(&app, false);
            result
        }
    })
    .await;
    shortcut_off(&app);
    control_close(&app);
    bubble_close(&app);
    tray_close();
    main_window_back(&app);
    let _ = app.emit_to("main", "recording-cancelled", ());
    cancelled
}

#[tauri::command]
async fn pause_recording(app: AppHandle) -> Result<Session, String> {
    blocking(move || recordings(&app).pause()).await
}

#[tauri::command]
async fn resume_recording(app: AppHandle) -> Result<Session, String> {
    blocking(move || recordings(&app).resume()).await
}

#[tauri::command]
async fn live_status(app: AppHandle) -> Result<Option<Live>, String> {
    blocking(move || Ok(recordings(&app).live())).await
}

#[tauri::command]
async fn stop_recording(app: AppHandle) -> Result<Session, String> {
    let stopped = blocking({
        let app = app.clone();
        move || {
            let stopped = recordings(&app).stop();
            own_windows(&app, false);
            stopped
        }
    })
    .await;
    shortcut_off(&app);
    control_close(&app);
    bubble_close(&app);
    tray_close();
    main_window_back(&app);
    // The stop may come from the floating control or tray: the main window opens Finish.
    if let Ok(session) = &stopped {
        let _ = app.emit_to("main", "recording-stopped", session);
    }
    stopped
}

#[tauri::command]
async fn list_recordings(app: AppHandle) -> Result<Vec<Session>, String> {
    blocking(move || Ok(recordings(&app).recent())).await
}

#[tauri::command]
async fn remove_from_recent(app: AppHandle, id: String) -> Result<(), String> {
    blocking(move || recordings(&app).remove_from_recent(&id)).await
}

#[tauri::command]
async fn delete_recording(app: AppHandle, id: String) -> Result<(), String> {
    blocking(move || recordings(&app).delete(&id)).await
}

/// Shows the recording's picture track selected in Finder (Explorer, the file manager).
#[tauri::command]
fn show_recording(app: AppHandle, id: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let dir = recordings(&app).dir(&id)?;
    let file = ["screen.mp4", "mic.mp4"]
        .into_iter()
        .map(|name| dir.join(name))
        .find(|path| path.is_file())
        .ok_or("This recording no longer exists.")?;
    app.opener()
        .reveal_item_in_dir(file)
        .map_err(|e| format!("The folder didn't open ({e}). Try again, or Report a problem."))
}

/// One recording, for Finish.
#[tauri::command]
async fn recording(app: AppHandle, id: String) -> Result<Session, String> {
    blocking(move || {
        sessions::read_session(&recordings(&app).dir(&id)?).ok_or_else(|| {
            "This recording no longer exists. Go back to Home and choose another one.".into()
        })
    })
    .await
}

/// A recording's tracks for Finish's player (absolute paths, `None` when not recorded).
#[derive(Serialize)]
struct Media {
    screen: Option<PathBuf>,
    camera: Option<PathBuf>,
    mic: Option<PathBuf>,
    system: Option<PathBuf>,
}

#[tauri::command]
async fn recording_media(app: AppHandle, id: String) -> Result<Media, String> {
    blocking(move || {
        let dir = recordings(&app).dir(&id)?;
        let file = |name: &str| Some(dir.join(name)).filter(|p| p.is_file());
        Ok(Media {
            screen: file("screen.mp4"),
            camera: file("camera.mp4"),
            mic: file("mic.mp4"),
            system: file("system.mp4"),
        })
    })
    .await
}

/// The timeline's sound levels: one byte per 10 ms (waveform.rs).
#[tauri::command]
async fn recording_waveform(app: AppHandle, id: String) -> Result<tauri::ipc::Response, String> {
    blocking(move || {
        waveform::levels(&recordings(&app).dir(&id)?)
            .map(tauri::ipc::Response::new)
            .map_err(|_| {
                "The sound couldn't be read for the timeline. You can still trim and export.".into()
            })
    })
    .await
}

/// The running sound job (sound.rs); a new one cancels it.
#[derive(Default)]
struct SoundJob(std::sync::Mutex<Option<std::sync::Arc<std::sync::atomic::AtomicBool>>>);

#[derive(Clone, Serialize)]
struct SoundProgress {
    id: String,
    /// "clean" or "loudness".
    stage: &'static str,
    fraction: f64,
}

/// The preview's sound for these choices (made if needed, with "sound-progress" events);
/// `None` without any sound track. Errors "cancelled" when a newer request replaced it.
#[tauri::command]
async fn prepare_sound(
    app: AppHandle,
    id: String,
    clean: bool,
    loudness: finish::Loudness,
) -> Result<Option<PathBuf>, String> {
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    if let Some(old) = app
        .state::<SoundJob>()
        .0
        .lock()
        .unwrap()
        .replace(cancel.clone())
    {
        old.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    blocking(move || {
        let dir = recordings(&app).dir(&id)?;
        let session = sessions::read_session(&dir)
            .ok_or("This recording no longer exists. Go back to Home and choose another one.")?;
        let duration = session.check.as_ref().map_or(session.elapsed_s, |c| c.duration_s);
        let mut last = ("", -1.0f64);
        let result = sound::preview(&dir, duration, clean, loudness, &cancel, &mut |stage, f| {
            if stage != last.0 || f - last.1 >= 0.01 || f >= 1.0 {
                last = (stage, f);
                let _ = app.emit(
                    "sound-progress",
                    SoundProgress {
                        id: id.clone(),
                        stage,
                        fraction: f,
                    },
                );
            }
        });
        result.map_err(|e| {
            if e == "cancelled" {
                e
            } else {
                format!("The sound couldn't be prepared ({e}). Try again, or switch off Clean up voice and export without it.")
            }
        })
    })
    .await
}

/// The app's remembered choices (`prefs.json` in the app data folder).
#[derive(Default, Serialize, serde::Deserialize)]
struct Prefs {
    /// Where exports go; the Movies (Videos) folder when not set or gone.
    #[serde(default)]
    export_folder: Option<PathBuf>,
    /// Count 3-2-1 before recording (on when not set).
    #[serde(default)]
    countdown: Option<bool>,
    /// Where the floating control was left, per display id: points from its top-left.
    #[serde(default)]
    control_positions: std::collections::HashMap<String, (f64, f64)>,
    /// Where the camera bubble was left, per display id, and its size.
    #[serde(default)]
    bubble_positions: std::collections::HashMap<String, (f64, f64)>,
    #[serde(default)]
    bubble_size: Option<String>,
}

fn prefs_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("prefs.json"))
}

fn read_prefs(app: &AppHandle) -> Prefs {
    prefs_path(app)
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

/// Changes the remembered choices; `failed` says what it means when they can't be saved.
fn write_prefs(
    app: &AppHandle,
    change: impl FnOnce(&mut Prefs),
    failed: &str,
) -> Result<(), String> {
    let mut prefs = read_prefs(app);
    change(&mut prefs);
    let path = prefs_path(app)?;
    let json = serde_json::to_string_pretty(&prefs).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| format!("{failed} ({e})."))
}

#[tauri::command]
fn countdown_pref(app: AppHandle) -> bool {
    read_prefs(&app).countdown.unwrap_or(true)
}

#[tauri::command]
fn set_countdown_pref(app: AppHandle, on: bool) -> Result<(), String> {
    write_prefs(
        &app,
        |p| p.countdown = Some(on),
        "The countdown choice couldn't be remembered; it's used until the app closes",
    )
}

/// The folder exports go to.
fn export_folder(app: &AppHandle) -> Result<PathBuf, String> {
    match read_prefs(app).export_folder.filter(|f| f.is_dir()) {
        Some(folder) => Ok(folder),
        None => app
            .path()
            .video_dir()
            .or_else(|_| app.path().home_dir())
            .map_err(|_| "No folder to export to was found. Choose one with Change…".to_string()),
    }
}

/// Where the export will go, and whether a file of that name is already there.
#[derive(Serialize)]
struct ExportTarget {
    folder: PathBuf,
    name: String,
    exists: bool,
    estimated_bytes: u64,
    free_bytes: Option<u64>,
}

fn export_job_parts(
    app: &AppHandle,
    id: &str,
) -> Result<(PathBuf, Session, finish::Settings, f64), String> {
    let dir = recordings(app).dir(id)?;
    let session = sessions::read_session(&dir)
        .ok_or("This recording no longer exists. Go back to Home and choose another one.")?;
    let settings = finish::read(&dir, &session);
    let duration = session
        .check
        .as_ref()
        .map_or(session.elapsed_s, |c| c.duration_s);
    Ok((dir, session, settings, duration))
}

#[tauri::command]
async fn export_target(app: AppHandle, id: String) -> Result<ExportTarget, String> {
    blocking(move || {
        let (dir, session, settings, duration_s) = export_job_parts(&app, &id)?;
        let folder = export_folder(&app)?;
        let name = export::file_name(&session);
        let job = export::Job {
            dir: &dir,
            session: &session,
            settings: &settings,
            duration_s,
        };
        Ok(ExportTarget {
            exists: export::names_in(&folder).contains(&name.to_lowercase()),
            estimated_bytes: job.estimated_bytes(),
            free_bytes: disk::free_bytes(&folder),
            folder,
            name,
        })
    })
    .await
}

/// Remembers the folder exports go to.
#[tauri::command]
async fn set_export_folder(app: AppHandle, folder: PathBuf) -> Result<(), String> {
    blocking(move || {
        if !folder.is_dir() {
            return Err("That folder can't be used. Choose another one.".into());
        }
        write_prefs(
            &app,
            |p| p.export_folder = Some(folder),
            "The folder couldn't be remembered; it's used for this export only",
        )
    })
    .await
}

/// The running export: its cancel flag, and the last finished file (for Show in folder).
#[derive(Default)]
struct ExportState {
    cancel: std::sync::Mutex<Option<std::sync::Arc<std::sync::atomic::AtomicBool>>>,
    last: std::sync::Mutex<Option<PathBuf>>,
}

#[derive(Clone, Serialize)]
struct ExportProgress {
    id: String,
    /// "sound", "picture" or "check".
    stage: &'static str,
    fraction: f64,
}

/// Exports the recording with its saved Finish choices into the export folder (events
/// "export-progress"). `replace`: replace a file of the same name; otherwise keep it and
/// use the next free name.
#[tauri::command]
async fn export_recording(
    app: AppHandle,
    id: String,
    replace: bool,
) -> Result<export::Done, String> {
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    *app.state::<ExportState>().cancel.lock().unwrap() = Some(cancel.clone());
    blocking(move || {
        let (dir, session, settings, duration_s) = export_job_parts(&app, &id)?;
        let folder = export_folder(&app)?;
        let job = export::Job {
            dir: &dir,
            session: &session,
            settings: &settings,
            duration_s,
        };
        // Room for the video, its temporary copy's overhead and the sound.
        let needed = job.estimated_bytes() + job.estimated_bytes() / 5 + 100_000_000;
        if let Some(free) = disk::free_bytes(&folder)
            && free < needed
        {
            return Err(format!(
                "There isn't enough free space in that folder: the video needs about {}, {} is free. Free up some space or choose another folder, then export again.",
                disk::gigabytes(needed),
                disk::gigabytes(free)
            ));
        }
        let _awake = awake::KeepAwake::new("exporting a video");
        let mut last = ("", -1.0f64);
        let done = export::export(&job, &folder.join(export::file_name(&session)), replace, &cancel, &mut |stage, f| {
            if stage != last.0 || f - last.1 >= 0.01 || f >= 1.0 {
                last = (stage, f);
                let _ = app.emit(
                    "export-progress",
                    ExportProgress {
                        id: id.clone(),
                        stage,
                        fraction: f,
                    },
                );
            }
        })?;
        *app.state::<ExportState>().last.lock().unwrap() = Some(done.path.clone());
        Ok(done)
    })
    .await
}

#[tauri::command]
fn cancel_export(app: AppHandle) {
    if let Some(cancel) = app.state::<ExportState>().cancel.lock().unwrap().as_ref() {
        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Shows the last exported video selected in Finder (Explorer, the file manager).
#[tauri::command]
fn show_export(app: AppHandle) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let path = app
        .state::<ExportState>()
        .last
        .lock()
        .unwrap()
        .clone()
        .filter(|p| p.is_file())
        .ok_or("The exported video is no longer there. Export it again.")?;
    app.opener()
        .reveal_item_in_dir(path)
        .map_err(|e| format!("The folder didn't open ({e}). Try again, or Report a problem."))
}

/// The recording's Finish choices (saved ones, or the first choices).
#[tauri::command]
async fn finish_settings(app: AppHandle, id: String) -> Result<finish::Settings, String> {
    blocking(move || {
        let dir = recordings(&app).dir(&id)?;
        let session = sessions::read_session(&dir)
            .ok_or("This recording no longer exists. Go back to Home and choose another one.")?;
        Ok(finish::read(&dir, &session))
    })
    .await
}

#[tauri::command]
async fn save_finish_settings(
    app: AppHandle,
    id: String,
    settings: finish::Settings,
) -> Result<(), String> {
    blocking(move || finish::write(&recordings(&app).dir(&id)?, &settings)).await
}

#[tauri::command]
async fn pending_recoveries(app: AppHandle) -> Result<Vec<Session>, String> {
    blocking(move || Ok(recovery::pending(&recordings(&app)))).await
}

#[tauri::command]
async fn recover_recording(app: AppHandle, id: String) -> Result<Session, String> {
    blocking(move || recovery::recover(&recordings(&app), &id)).await
}

#[tauri::command]
fn quit_app(app: AppHandle) {
    app.exit(0);
}

/// Where testers report problems: the bug-report form, with the app version filled in.
fn report_problem_url() -> String {
    format!(
        "https://github.com/kreokit/rec/issues/new?template=bug_report.yml&version={}",
        env!("CARGO_PKG_VERSION")
    )
}

/// Opens the bug-report page in the default browser. Only this fixed page can be opened;
/// the webview gets no general permission to open links.
#[tauri::command]
fn report_problem(app: AppHandle) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_url(report_problem_url(), None::<&str>)
        .map_err(|e| e.to_string())
}

/// `--bench ...` and `--bench-analyze DIR` in test builds (src/bench.rs); `None` for a
/// normal start.
#[cfg(feature = "bench")]
pub fn bench_main() -> Option<i32> {
    bench::main(&std::env::args().collect::<Vec<_>>())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    use tauri_plugin_global_shortcut::ShortcutState;
                    if event.state == ShortcutState::Pressed && *shortcut == stop_shortcut() {
                        let app = app.clone();
                        tauri::async_runtime::spawn(async move {
                            let counting = recordings(&app)
                                .live()
                                .is_some_and(|l| l.session.state == sessions::State::Countdown);
                            if counting {
                                let _ = countdown_cancel(app).await;
                            } else {
                                let _ = stop_recording(app).await;
                            }
                        });
                    }
                })
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            permissions,
            request_mic,
            request_camera,
            request_screen,
            reset_screen_permission,
            open_privacy_settings,
            restart_app,
            devices,
            windows,
            estimate,
            monitor_start,
            monitor_level,
            monitor_stop,
            camera_preview_start,
            camera_preview_frame,
            camera_preview_stop,
            region_open,
            region_done,
            start_recording,
            countdown_done,
            countdown_cancel,
            floating_move,
            stop_shortcut_text,
            bubble_resize,
            bubble_hide,
            countdown_pref,
            set_countdown_pref,
            pause_recording,
            resume_recording,
            live_status,
            stop_recording,
            list_recordings,
            remove_from_recent,
            delete_recording,
            show_recording,
            recording,
            finish_settings,
            recording_media,
            recording_waveform,
            prepare_sound,
            export_target,
            set_export_folder,
            export_recording,
            cancel_export,
            show_export,
            save_finish_settings,
            pending_recoveries,
            recover_recording,
            quit_app,
            report_problem,
            fieldtest::fieldtest_pattern,
            fieldtest::fieldtest_pattern_close,
            fieldtest::fieldtest_info,
            fieldtest::fieldtest_run,
            fieldtest::fieldtest_save
        ])
        .setup(|app| {
            // Linux keeps the screen portal's restore tokens there.
            #[cfg(target_os = "linux")]
            native::set_data_dir(app.path().app_data_dir()?);
            let root: PathBuf = app.path().app_data_dir()?.join("recordings");
            let mut recordings = Recordings::new(root, Box::new(sessions::NativeBackend));
            if synthetic().is_some() {
                recordings.synthetic = Some(native::Synthetic::default());
            }
            // Debug and test builds: KREOKIT_REC_DISK_STOP_AFTER_MB=N acts as if the disk
            // were almost full N MB of recording from now (to try the stop in the app).
            if cfg!(any(debug_assertions, feature = "bench")) {
                let after = std::env::var("KREOKIT_REC_DISK_STOP_AFTER_MB").ok();
                if let (Some(mb), Some(free)) = (
                    after.and_then(|v| v.parse::<u64>().ok()),
                    disk::free_bytes(recordings.root().parent().unwrap_or(recordings.root())),
                ) {
                    recordings.start_below_free = 0;
                    recordings.stop_below_free = free.saturating_sub(mb * 1_000_000);
                }
            }
            // The disk is almost full: stop and finish now, like the Stop button (the main
            // window opens Finish with the note).
            let handle = app.handle().clone();
            recordings.on_disk_full = Some(std::sync::Arc::new(move || {
                let app = handle.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = stop_recording(app).await;
                });
            }));
            app.manage(recordings);
            app.manage(SoundJob::default());
            app.manage(ExportState::default());
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // Quitting during a recording finishes its files first.
            if let tauri::RunEvent::Exit = event {
                native::monitor_stop();
                let recordings = recordings(app);
                if recordings.is_recording() {
                    let _ = recordings.stop();
                }
            }
        });
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_menu_bar_shows_the_time() {
        use crate::sessions::State;
        assert_eq!(super::tray_title(State::Recording, 12.9), "● 0:12");
        assert_eq!(super::tray_title(State::Paused, 725.0), "Paused 12:05");
        assert_eq!(super::tray_title(State::Recording, 3723.0), "● 1:02:03");
        assert_eq!(super::tray_title(State::Countdown, 0.0), "Starting…");
    }

    #[test]
    fn report_problem_opens_the_bug_form() {
        assert_eq!(
            super::report_problem_url(),
            format!(
                "https://github.com/kreokit/rec/issues/new?template=bug_report.yml&version={}",
                env!("CARGO_PKG_VERSION")
            )
        );
    }

    #[test]
    fn preview_keeps_the_camera_shape() {
        assert_eq!(super::preview_size(1280, 720), (320, 180));
        assert_eq!(super::preview_size(640, 480), (320, 240));
        assert_eq!(super::preview_size(0, 0), (320, 180));
    }
}

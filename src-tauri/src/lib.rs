#[cfg(any(windows, target_os = "linux"))]
mod audio;
mod awake;
#[cfg(feature = "bench")]
mod bench;
mod check;
mod disk;
mod encoder;
mod ffmpeg;
mod jpeg;
mod native;
mod pipeline;
mod preview;
mod probe;
mod recovery;
mod sessions;
mod synthetic;
#[cfg(any(test, feature = "bench"))]
mod testpattern;

use std::path::PathBuf;

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

/// The one Recordings for the app, in the app data folder's "recordings".
fn recordings(app: &AppHandle) -> tauri::State<'_, Recordings> {
    app.state::<Recordings>()
}

#[derive(Serialize)]
struct Permissions {
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
    /// The computer sound can be recorded (macOS 13+).
    system_audio: bool,
    /// Windows can be recorded; otherwise why not, for the setup screen.
    windows: bool,
    windows_note: Option<String>,
}

/// Window capture on macOS 12 (ScreenCaptureKit, 12.3+): whether it's reliable enough to
/// offer. Measured on the dev Mac (12.7.6, 2026-10-27, docs/m3-results.md): 100 % and
/// 99.38 % fresh frames, above the 99 % target; set to false if a later measurement fails.
const WINDOWS_ON_MACOS_12: bool = true;

fn window_support() -> (bool, Option<String>) {
    if !native::window_capture_supported() {
        return (
            false,
            Some("Recording a window needs macOS 12.3 or newer. Record a region instead.".into()),
        );
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
            displays: vec![Display {
                id: 0,
                name: "Test display".into(),
                width: 1920,
                height: 1200,
                main: true,
                // Small and away from the corner: a test instance's region overlay covers
                // only this part of the real screen.
                x: 240.0,
                y: 160.0,
                points_width: 960.0,
                points_height: 600.0,
            }],
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
            windows: true,
            windows_note: None,
        });
    }
    blocking(|| {
        let (windows, windows_note) = window_support();
        Ok(Devices {
            displays: native::displays(),
            mics: native::mics(),
            cameras: native::cameras(),
            system_audio: native::system_audio_supported(),
            windows,
            windows_note,
        })
    })
    .await
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
async fn camera_preview_stop() -> Result<(), String> {
    blocking(|| {
        native::preview_stop();
        Ok(())
    })
    .await
}

#[tauri::command]
async fn start_recording(app: AppHandle, options: Options) -> Result<Session, String> {
    blocking(move || {
        native::monitor_stop();
        native::preview_stop();
        recordings(&app).start(&options)
    })
    .await
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
    blocking(move || recordings(&app).stop()).await
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
            pause_recording,
            resume_recording,
            live_status,
            stop_recording,
            list_recordings,
            remove_from_recent,
            delete_recording,
            show_recording,
            pending_recoveries,
            recover_recording,
            quit_app,
            report_problem
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
            app.manage(recordings);
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

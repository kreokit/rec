mod awake;
#[cfg(feature = "bench")]
mod bench;
mod check;
mod disk;
mod ffmpeg;
mod native;
mod probe;
mod recovery;
mod sessions;
#[cfg(any(test, feature = "bench"))]
mod testpattern;

use std::path::PathBuf;

use serde::Serialize;
use tauri::{AppHandle, Manager};

use native::{Access, Display, Mic};
use sessions::{Live, Options, Recordings, Session};

/// Runs blocking work (the recorder, FFmpeg, file I/O) off the main thread.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| e.to_string())?
}

/// The one Recordings for the app, in the app data folder's "recordings".
fn recordings(app: &AppHandle) -> tauri::State<'_, Recordings> {
    app.state::<Recordings>()
}

#[derive(Serialize)]
struct Permissions {
    screen: Access,
    mic: Access,
    /// macOS 12 applies a new Screen Recording permission only after a restart.
    restart_after_screen: bool,
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
fn permissions() -> Permissions {
    Permissions {
        screen: native::screen_access(),
        mic: native::mic_access(),
        restart_after_screen: macos_major() == 12,
    }
}

/// Shows the system's microphone question (first time only); returns the answer.
#[tauri::command]
async fn request_mic() -> Result<Access, String> {
    blocking(|| Ok(native::request_mic_access())).await
}

/// The system's screen-recording request (macOS shows it once, then opens nothing).
#[tauri::command]
fn request_screen() {
    native::request_screen_access();
}

/// Opens System Preferences (System Settings) at Screen Recording or Microphone.
#[tauri::command]
fn open_privacy_settings(kind: String) -> Result<(), String> {
    let pane = match kind.as_str() {
        "screen" => "Privacy_ScreenCapture",
        "mic" => "Privacy_Microphone",
        _ => return Err("unknown setting".into()),
    };
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(format!(
                "x-apple.systempreferences:com.apple.preference.security?{pane}"
            ))
            .status()
            .map_err(|e| {
                format!("System Preferences didn't open ({e}). Open it from the Apple menu, then Security & Privacy > Privacy.")
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
}

#[tauri::command]
async fn devices() -> Result<Devices, String> {
    blocking(|| {
        Ok(Devices {
            displays: native::displays(),
            mics: native::mics(),
        })
    })
    .await
}

#[derive(Serialize)]
struct Estimate {
    width: u32,
    height: u32,
    bytes_per_minute: u64,
    free_bytes: Option<u64>,
}

/// The output size and file size per minute for a display at a quality, and the space
/// left for recordings.
#[tauri::command]
fn estimate(app: AppHandle, width: u32, height: u32, quality: u32, mic: bool) -> Estimate {
    let (w, h) = sessions::output_size(width, height, quality);
    let root = recordings(&app).root().to_path_buf();
    let _ = std::fs::create_dir_all(&root);
    Estimate {
        width: w,
        height: h,
        bytes_per_minute: sessions::bytes_per_minute(w, h, mic),
        free_bytes: disk::free_bytes(&root),
    }
}

#[tauri::command]
async fn monitor_start(mic_id: String) -> Result<(), String> {
    blocking(move || native::monitor_start(&mic_id)).await
}

#[tauri::command]
fn monitor_level() -> f32 {
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

#[tauri::command]
async fn start_recording(app: AppHandle, options: Options) -> Result<Session, String> {
    blocking(move || {
        native::monitor_stop();
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
            request_screen,
            open_privacy_settings,
            restart_app,
            devices,
            estimate,
            monitor_start,
            monitor_level,
            monitor_stop,
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
            let root: PathBuf = app.path().app_data_dir()?.join("recordings");
            app.manage(Recordings::new(root, Box::new(sessions::NativeBackend)));
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
}

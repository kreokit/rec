//! The system recorder: on macOS the Objective-C recorder in `native/macos` (its rules are
//! in recorder.h); on Windows and Linux the same rules in Rust (pipeline.rs), writing
//! through our FFmpeg (encoder.rs). The `pipe-recorder` feature (development) records with
//! the Rust recorder on macOS too, so its tests run on the dev Mac.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// How the screen is captured on macOS (docs/capture-research.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ScreenApi {
    /// ScreenCaptureKit on macOS 13+, CGDisplayStream on 12.
    #[default]
    Auto,
    CgDisplayStream,
    ScreenCaptureKit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScreenConfig {
    /// 0: the main display.
    pub display_id: u32,
    /// Part of the display; `None`: all of it. `width`/`height` keep its shape.
    pub region: Option<Region>,
    /// A window instead of the display (its id from [`windows`]); `width`/`height` keep its
    /// shape. When it closes, the recording goes on with its last picture.
    #[serde(default)]
    pub window: Option<u32>,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// A multiple of `fps`; one captured frame is kept per output frame.
    pub capture_fps: u32,
    pub api: ScreenApi,
    pub bitrate: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CameraConfig {
    /// `""`: the default camera.
    pub id: String,
    /// The output size: the camera's own shape ([`Camera`]).
    pub width: u32,
    pub height: u32,
    pub bitrate: u32,
}

/// Generated sources instead of real devices, for tests (see recorder.h).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Synthetic {
    /// As fast as the encoder allows, on the generator's own clock.
    pub fast: bool,
    /// The picture stands still for 2 s of every 10 s.
    pub still_periods: bool,
    /// The camera's first picture comes 1 s and the mic's first sound 0.4 s after the
    /// start, as with real devices.
    #[serde(default)]
    pub late_start: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordConfig {
    pub dir: PathBuf,
    pub screen: Option<ScreenConfig>,
    /// The microphone's id (`None`: no microphone; `Some("")`: the default one).
    pub mic: Option<String>,
    pub camera: Option<CameraConfig>,
    /// The computer's sound (macOS 13+), into system.mp4.
    #[serde(default)]
    pub system_audio: bool,
    pub synthetic: Option<Synthetic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Status {
    pub elapsed: f64,
    pub frames: i64,
    pub frames_new: i64,
    pub frames_repeated: i64,
    pub frames_late: i64,
    pub frames_dropped: i64,
    pub camera_frames: i64,
    pub camera_frames_new: i64,
    pub camera_frames_repeated: i64,
    pub camera_frames_late: i64,
    pub camera_frames_dropped: i64,
    /// Pictures the system dropped before they reached the recorder.
    pub camera_frames_lost: i64,
    pub audio_samples: i64,
    pub audio_filled: i64,
    pub audio_dropped: i64,
    pub mic_peak: f32,
    /// The computer sound (system.mp4), samples per channel; silence fills where nothing
    /// played.
    pub system_samples: i64,
    pub system_filled: i64,
    pub system_dropped: i64,
    /// The recorded window closed; the recording shows its last picture from then.
    pub window_closed: bool,
    pub window_closed_at: f64,
    pub paused: bool,
    pub failed: bool,
}

/// A window that can be recorded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Window {
    pub id: u32,
    pub app: String,
    pub title: String,
    /// In points.
    pub width: f64,
    pub height: f64,
    /// Pixels per point of the display it's on.
    pub scale: f64,
}

/// A rectangle on a display, in points from its top-left corner.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Region {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Display {
    pub id: u32,
    pub name: String,
    /// In pixels.
    pub width: u32,
    pub height: u32,
    pub main: bool,
    /// Where it is, in points (top-left of the main display, y down), and its size in
    /// points.
    #[serde(default)]
    pub x: f64,
    #[serde(default)]
    pub y: f64,
    #[serde(default)]
    pub points_width: f64,
    #[serde(default)]
    pub points_height: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mic {
    pub id: String,
    pub name: String,
    pub default: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Camera {
    pub id: String,
    pub name: String,
    pub default: bool,
    /// The largest size it delivers at 30 fps, up to 720 pixels high.
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    Allowed,
    Denied,
    NotAsked,
}

#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::{CStr, CString, c_char, c_int, c_void};
    use std::os::unix::ffi::OsStrExt;

    use super::*;

    #[repr(C)]
    struct KrRecConfig {
        dir: *const c_char,
        screen: c_int,
        display_id: u32,
        width: c_int,
        height: c_int,
        fps: c_int,
        capture_fps: c_int,
        screen_api: c_int,
        video_bitrate: c_int,
        region_x: f64,
        region_y: f64,
        region_width: f64,
        region_height: f64,
        window_id: u32,
        mic: c_int,
        mic_id: *const c_char,
        camera: c_int,
        camera_id: *const c_char,
        camera_width: c_int,
        camera_height: c_int,
        camera_bitrate: c_int,
        system_audio: c_int,
        synthetic: c_int,
        synthetic_fast: c_int,
        synthetic_static: c_int,
        synthetic_late_start: c_int,
    }

    #[repr(C)]
    #[derive(Default)]
    struct KrRecStatus {
        elapsed: f64,
        frames: i64,
        frames_new: i64,
        frames_repeated: i64,
        frames_late: i64,
        frames_dropped: i64,
        camera_frames: i64,
        camera_frames_new: i64,
        camera_frames_repeated: i64,
        camera_frames_late: i64,
        camera_frames_dropped: i64,
        camera_frames_lost: i64,
        audio_samples: i64,
        audio_filled: i64,
        audio_dropped: i64,
        mic_peak: f32,
        system_samples: i64,
        system_filled: i64,
        system_dropped: i64,
        window_closed: c_int,
        window_closed_at: f64,
        paused: c_int,
        failed: c_int,
    }

    unsafe extern "C" {
        fn kr_rec_start(config: *const KrRecConfig, error: *mut c_char, len: usize) -> *mut c_void;
        fn kr_rec_pause(rec: *mut c_void);
        fn kr_rec_resume(rec: *mut c_void);
        fn kr_rec_status(rec: *mut c_void, status: *mut KrRecStatus);
        fn kr_rec_stop(
            rec: *mut c_void,
            status: *mut KrRecStatus,
            error: *mut c_char,
            len: usize,
        ) -> c_int;
        fn kr_screen_access() -> c_int;
        fn kr_request_screen_access();
        fn kr_mic_access() -> c_int;
        fn kr_request_mic_access() -> c_int;
        fn kr_camera_access() -> c_int;
        fn kr_request_camera_access() -> c_int;
        fn kr_displays_json() -> *mut c_char;
        fn kr_mics_json() -> *mut c_char;
        fn kr_cameras_json() -> *mut c_char;
        fn kr_windows_json() -> *mut c_char;
        fn kr_system_audio_supported() -> c_int;
        fn kr_window_capture_supported() -> c_int;
        fn kr_free(s: *mut c_char);
        fn kr_overlay_window(ns_window: *mut std::ffi::c_void);
        #[cfg(feature = "bench")]
        fn kr_window_number(ns_window: *mut std::ffi::c_void) -> u32;
        fn kr_mic_monitor_start(mic_id: *const c_char, error: *mut c_char, len: usize) -> c_int;
        fn kr_mic_monitor_peak() -> f32;
        fn kr_mic_monitor_stop();
        fn kr_camera_preview_start(
            camera_id: *const c_char,
            width: c_int,
            height: c_int,
            synthetic: c_int,
            error: *mut c_char,
            len: usize,
        ) -> c_int;
        fn kr_camera_preview_frame(seq: *mut u64, jpeg: *mut *mut c_char) -> usize;
        fn kr_camera_preview_stop();
    }

    impl From<KrRecStatus> for Status {
        fn from(s: KrRecStatus) -> Self {
            Status {
                elapsed: s.elapsed,
                frames: s.frames,
                frames_new: s.frames_new,
                frames_repeated: s.frames_repeated,
                frames_late: s.frames_late,
                frames_dropped: s.frames_dropped,
                camera_frames: s.camera_frames,
                camera_frames_new: s.camera_frames_new,
                camera_frames_repeated: s.camera_frames_repeated,
                camera_frames_late: s.camera_frames_late,
                camera_frames_dropped: s.camera_frames_dropped,
                camera_frames_lost: s.camera_frames_lost,
                audio_samples: s.audio_samples,
                audio_filled: s.audio_filled,
                audio_dropped: s.audio_dropped,
                mic_peak: s.mic_peak,
                system_samples: s.system_samples,
                system_filled: s.system_filled,
                system_dropped: s.system_dropped,
                window_closed: s.window_closed != 0,
                window_closed_at: s.window_closed_at,
                paused: s.paused != 0,
                failed: s.failed != 0,
            }
        }
    }

    fn message(buffer: &[u8]) -> String {
        CStr::from_bytes_until_nul(buffer)
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// A running recording. Dropping it without [`Recorder::stop`] stops it too.
    #[cfg_attr(feature = "pipe-recorder", allow(dead_code))]
    pub struct Recorder(*mut c_void);

    // The native recorder serializes every call on its own queue.
    unsafe impl Send for Recorder {}
    unsafe impl Sync for Recorder {}

    #[cfg_attr(feature = "pipe-recorder", allow(dead_code))]
    impl Recorder {
        pub fn start(config: &RecordConfig) -> Result<Recorder, String> {
            let dir = CString::new(config.dir.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
            let mic_id =
                CString::new(config.mic.clone().unwrap_or_default()).map_err(|e| e.to_string())?;
            let camera = config.camera.as_ref();
            let camera_id = CString::new(camera.map(|c| c.id.clone()).unwrap_or_default())
                .map_err(|e| e.to_string())?;
            let screen = config.screen.as_ref();
            let region = screen.and_then(|s| s.region);
            let synthetic = config.synthetic.unwrap_or_default();
            let raw = KrRecConfig {
                dir: dir.as_ptr(),
                screen: screen.is_some() as c_int,
                display_id: screen.map_or(0, |s| s.display_id),
                width: screen.map_or(0, |s| s.width as c_int),
                height: screen.map_or(0, |s| s.height as c_int),
                fps: screen.map_or(30, |s| s.fps as c_int),
                capture_fps: screen.map_or(30, |s| s.capture_fps as c_int),
                screen_api: match screen.map(|s| s.api).unwrap_or_default() {
                    ScreenApi::Auto => 0,
                    ScreenApi::CgDisplayStream => 1,
                    ScreenApi::ScreenCaptureKit => 2,
                },
                video_bitrate: screen.map_or(0, |s| s.bitrate as c_int),
                region_x: region.map_or(0.0, |r| r.x),
                region_y: region.map_or(0.0, |r| r.y),
                region_width: region.map_or(0.0, |r| r.width),
                region_height: region.map_or(0.0, |r| r.height),
                window_id: screen.and_then(|s| s.window).unwrap_or(0),
                mic: config.mic.is_some() as c_int,
                mic_id: mic_id.as_ptr(),
                camera: camera.is_some() as c_int,
                camera_id: camera_id.as_ptr(),
                camera_width: camera.map_or(0, |c| c.width as c_int),
                camera_height: camera.map_or(0, |c| c.height as c_int),
                camera_bitrate: camera.map_or(0, |c| c.bitrate as c_int),
                system_audio: config.system_audio as c_int,
                synthetic: config.synthetic.is_some() as c_int,
                synthetic_fast: synthetic.fast as c_int,
                synthetic_static: synthetic.still_periods as c_int,
                synthetic_late_start: synthetic.late_start as c_int,
            };
            let mut error = [0u8; 1024];
            let rec = unsafe { kr_rec_start(&raw, error.as_mut_ptr().cast(), error.len()) };
            if rec.is_null() {
                Err(message(&error))
            } else {
                Ok(Recorder(rec))
            }
        }

        pub fn pause(&self) {
            unsafe { kr_rec_pause(self.0) }
        }

        pub fn resume(&self) {
            unsafe { kr_rec_resume(self.0) }
        }

        pub fn status(&self) -> Status {
            let mut status = KrRecStatus::default();
            unsafe { kr_rec_status(self.0, &mut status) };
            status.into()
        }

        /// Stops and finishes the files. The error (a source or file failed during the
        /// recording) comes with the final status: the files are kept either way.
        pub fn stop(mut self) -> (Status, Option<String>) {
            self.stop_inner()
        }

        fn stop_inner(&mut self) -> (Status, Option<String>) {
            let mut status = KrRecStatus::default();
            let mut error = [0u8; 1024];
            let result =
                unsafe { kr_rec_stop(self.0, &mut status, error.as_mut_ptr().cast(), error.len()) };
            self.0 = std::ptr::null_mut();
            (status.into(), (result != 0).then(|| message(&error)))
        }
    }

    impl Drop for Recorder {
        fn drop(&mut self) {
            if !self.0.is_null() {
                self.stop_inner();
            }
        }
    }

    pub fn screen_access() -> Access {
        if unsafe { kr_screen_access() } == 1 {
            Access::Allowed
        } else {
            Access::NotAsked
        }
    }

    pub fn request_screen_access() {
        unsafe { kr_request_screen_access() }
    }

    fn access_from(code: c_int) -> Access {
        match code {
            3 => Access::Allowed,
            0 => Access::NotAsked,
            _ => Access::Denied,
        }
    }

    pub fn mic_access() -> Access {
        access_from(unsafe { kr_mic_access() })
    }

    /// Shows the system dialog if the user wasn't asked yet; blocks until the answer.
    pub fn request_mic_access() -> Access {
        access_from(unsafe { kr_request_mic_access() })
    }

    pub fn camera_access() -> Access {
        access_from(unsafe { kr_camera_access() })
    }

    /// Shows the system dialog if the user wasn't asked yet; blocks until the answer.
    pub fn request_camera_access() -> Access {
        access_from(unsafe { kr_request_camera_access() })
    }

    fn take_json(s: *mut c_char) -> String {
        if s.is_null() {
            return "[]".into();
        }
        let json = unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned();
        unsafe { kr_free(s) };
        json
    }

    pub fn displays() -> Vec<Display> {
        serde_json::from_str(&take_json(unsafe { kr_displays_json() })).unwrap_or_default()
    }

    pub fn mics() -> Vec<Mic> {
        serde_json::from_str(&take_json(unsafe { kr_mics_json() })).unwrap_or_default()
    }

    pub fn cameras() -> Vec<Camera> {
        serde_json::from_str(&take_json(unsafe { kr_cameras_json() })).unwrap_or_default()
    }

    pub fn windows() -> Vec<Window> {
        serde_json::from_str(&take_json(unsafe { kr_windows_json() })).unwrap_or_default()
    }

    /// The computer sound can be recorded (macOS 13+).
    pub fn system_audio_supported() -> bool {
        unsafe { kr_system_audio_supported() != 0 }
    }

    /// Windows can be recorded (macOS 12.3+; whether it's reliable on 12 is measured, see
    /// docs/m3-results.md).
    pub fn window_capture_supported() -> bool {
        unsafe { kr_window_capture_supported() != 0 }
    }

    /// Opens the microphone for the level meter (no recording).
    pub fn monitor_start(mic_id: &str) -> Result<(), String> {
        let id = CString::new(mic_id).map_err(|e| e.to_string())?;
        let mut error = [0u8; 1024];
        match unsafe { kr_mic_monitor_start(id.as_ptr(), error.as_mut_ptr().cast(), error.len()) } {
            0 => Ok(()),
            _ => Err(message(&error)),
        }
    }

    pub fn monitor_peak() -> f32 {
        unsafe { kr_mic_monitor_peak() }
    }

    pub fn monitor_stop() {
        unsafe { kr_mic_monitor_stop() }
    }

    /// Starts the camera preview: small JPEG pictures `width` × `height`, ~15 a second.
    pub fn preview_start(
        camera_id: &str,
        width: u32,
        height: u32,
        synthetic: bool,
    ) -> Result<(), String> {
        let id = CString::new(camera_id).map_err(|e| e.to_string())?;
        let mut error = [0u8; 1024];
        let result = unsafe {
            kr_camera_preview_start(
                id.as_ptr(),
                width as c_int,
                height as c_int,
                synthetic as c_int,
                error.as_mut_ptr().cast(),
                error.len(),
            )
        };
        match result {
            0 => Ok(()),
            _ => Err(message(&error)),
        }
    }

    /// The newest preview picture if newer than `seq` (which is then updated).
    pub fn preview_frame(seq: &mut u64) -> Option<Vec<u8>> {
        let mut jpeg: *mut c_char = std::ptr::null_mut();
        let len = unsafe { kr_camera_preview_frame(seq, &mut jpeg) };
        if len == 0 || jpeg.is_null() {
            return None;
        }
        let bytes = unsafe { std::slice::from_raw_parts(jpeg.cast::<u8>(), len) }.to_vec();
        unsafe { kr_free(jpeg) };
        Some(bytes)
    }

    pub fn preview_stop() {
        unsafe { kr_camera_preview_stop() }
    }

    /// Puts the region overlay above the menu bar and the Dock. Main thread only.
    pub fn overlay_window(ns_window: *mut std::ffi::c_void) {
        unsafe { kr_overlay_window(ns_window) }
    }

    /// A window's CGWindowID (the test version's pattern window). Main thread only.
    #[cfg(feature = "bench")]
    pub fn window_number(ns_window: *mut std::ffi::c_void) -> u32 {
        if ns_window.is_null() {
            return 0;
        }
        unsafe { kr_window_number(ns_window) }
    }
}

#[cfg(any(not(target_os = "macos"), feature = "pipe-recorder"))]
pub use portable::Recorder;

/// The Rust recorder (pipeline.rs) with its sources: the synthetic ones, or the system's.
#[cfg(any(not(target_os = "macos"), feature = "pipe-recorder"))]
mod portable {
    use std::sync::Arc;

    use super::*;
    use crate::encoder::{PixelFormat, VideoInput};
    use crate::pipeline::{self, Core, Inputs};

    /// A running recording. Dropping it without [`Recorder::stop`] stops it too.
    pub struct Recorder(pipeline::Recorder);

    impl Recorder {
        pub fn start(config: &RecordConfig) -> Result<Recorder, String> {
            let Some(flags) = config.synthetic else {
                return system(config);
            };
            let layout = crate::synthetic::screen_layout(config);
            let nv12 = |(width, height)| VideoInput {
                width,
                height,
                format: PixelFormat::Nv12,
            };
            let inputs = Inputs {
                screen: layout.map(|(w, h, _)| nv12((w, h))),
                crop: layout.and_then(|(_, _, crop)| crop),
                camera: config.camera.as_ref().map(|c| nv12((c.width, c.height))),
            };
            let core: Arc<Core> = Core::new(config, inputs)?;
            let generator = crate::synthetic::start(core.clone(), config, flags);
            let pictures = config.screen.is_some() || config.camera.is_some();
            Ok(Recorder(pipeline::Recorder::new(
                core,
                vec![Box::new(generator)],
                pictures,
            )))
        }

        pub fn pause(&self) {
            self.0.pause()
        }

        pub fn resume(&self) {
            self.0.resume()
        }

        pub fn status(&self) -> Status {
            self.0.status()
        }

        /// Stops and finishes the files. The error (a source or file failed during the
        /// recording) comes with the final status: the files are kept either way.
        pub fn stop(self) -> (Status, Option<String>) {
            self.0.stop()
        }
    }

    /// A recording from the system's screen, camera and sound.
    #[cfg(windows)]
    fn system(config: &RecordConfig) -> Result<Recorder, String> {
        let (core, sources) = super::win::start(config)?;
        let pictures = config.screen.is_some() || config.camera.is_some();
        Ok(Recorder(pipeline::Recorder::new(core, sources, pictures)))
    }

    #[cfg(target_os = "linux")]
    fn system(config: &RecordConfig) -> Result<Recorder, String> {
        let (core, sources) = super::linux::start(config)?;
        let pictures = config.screen.is_some() || config.camera.is_some();
        Ok(Recorder(pipeline::Recorder::new(core, sources, pictures)))
    }

    /// The pipe-recorder feature on macOS: synthetic sources only.
    #[cfg(target_os = "macos")]
    fn system(_config: &RecordConfig) -> Result<Recorder, String> {
        Err("The Rust recorder records only synthetic sources on macOS.".into())
    }
}

#[cfg(windows)]
mod win;
#[cfg(windows)]
pub use win::{
    camera_access, cameras, displays, mic_access, mics, monitor_peak, monitor_start, monitor_stop,
    preview_frame, preview_start, preview_stop, request_camera_access, request_mic_access,
    request_screen_access, screen_access, system_audio_supported, window_capture_supported,
    windows,
};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::{
    camera_access, cameras, displays, mic_access, mics, monitor_peak, monitor_start, monitor_stop,
    preview_frame, preview_start, preview_stop, request_camera_access, request_mic_access,
    request_screen_access, screen_access, set_data_dir, system_audio_supported,
    window_capture_supported, windows,
};

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

    use super::*;

    /// Timing tests run alone: the fast synthetic tests keep the encoder busy, and a
    /// real-time test next to them would measure that load, not the recorder.
    static TIMING: RwLock<()> = RwLock::new(());

    /// For a test that measures real-time behaviour.
    pub fn alone() -> RwLockWriteGuard<'static, ()> {
        TIMING.write().unwrap_or_else(|e| e.into_inner())
    }

    /// For a test that records (as fast as the encoder allows) next to other such tests.
    pub fn alongside() -> RwLockReadGuard<'static, ()> {
        TIMING.read().unwrap_or_else(|e| e.into_inner())
    }

    pub fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("kreokit-rec-native-tests")
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    pub fn synthetic(dir: PathBuf, width: u32, height: u32, still: bool) -> RecordConfig {
        RecordConfig {
            dir,
            screen: Some(ScreenConfig {
                display_id: 0,
                region: None,
                window: None,
                width,
                height,
                fps: 30,
                capture_fps: 60,
                api: ScreenApi::Auto,
                bitrate: 2_000_000,
            }),
            mic: Some(String::new()),
            camera: Some(CameraConfig {
                id: String::new(),
                width: CAMERA_W,
                height: CAMERA_H,
                bitrate: 1_000_000,
            }),
            system_audio: false,
            synthetic: Some(Synthetic {
                fast: true,
                still_periods: still,
                late_start: false,
            }),
        }
    }

    /// CI Macs are VMs without a GPU or hardware encoder, so timing limits are looser there.
    pub fn on_ci() -> bool {
        std::env::var_os("CI").is_some()
    }

    /// The H.264 profiles this system's recorder may write: High from VideoToolbox, the
    /// encoder's accepted ones through FFmpeg (encoder.rs).
    fn profiles() -> &'static [&'static str] {
        if cfg!(any(not(target_os = "macos"), feature = "pipe-recorder")) {
            crate::encoder::accepted_profiles(crate::encoder::video_encoder().unwrap())
        } else {
            &["High"]
        }
    }

    /// The synthetic camera's size in the tests.
    pub const CAMERA_W: u32 = 320;
    pub const CAMERA_H: u32 = 180;

    /// The camera track: its own counter steps by one per output frame (30 fps), no gaps,
    /// and its markers line up with the mic's clicks within one frame.
    fn check_camera(dir: &std::path::Path, status: &Status) -> Report {
        let report = analyze_video(
            dir,
            "camera.mp4",
            &Geometry::synthetic(CAMERA_W, 30, false),
            30.0,
        )
        .unwrap();
        assert_eq!((report.bad, report.irregular_pts), (0, 0), "{report:?}");
        assert_eq!(report.frames as i64, status.camera_frames, "{report:?}");
        let worst = report
            .sync_ms_min
            .unwrap()
            .abs()
            .max(report.sync_ms_max.unwrap().abs());
        assert!(worst <= 1000.0 / 30.0, "{report:?}");
        report
    }

    /// Waits until `seconds` of the (synthetic) recording are written.
    pub fn wait_for(rec: &Recorder, seconds: f64) {
        let start = std::time::Instant::now();
        while rec.status().elapsed < seconds {
            assert!(start.elapsed().as_secs() < 300, "the recorder stalled");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    use crate::testpattern::{Geometry, Report, analyze_dir, analyze_video};

    fn check(dir: &std::path::Path, still: bool) -> Report {
        let report = analyze_dir(dir, &Geometry::synthetic(640, 60, still), 30.0).unwrap();
        assert_eq!((report.bad, report.irregular_pts), (0, 0), "{report:?}");
        // Picture and sound in sync within one frame, at every marker.
        let worst = report
            .sync_ms_min
            .unwrap()
            .abs()
            .max(report.sync_ms_max.unwrap().abs());
        assert!(worst <= 1000.0 / 30.0, "{report:?}");
        // A marker every second, except while the picture stands still (2 s of 10).
        let seconds = report.frames as f64 / 30.0 * if still { 0.8 } else { 1.0 };
        assert!(report.markers as f64 >= seconds - 1.5, "{report:?}");
        report
    }

    // Linux lists its displays through the portal's dialog instead (M3 step 11).
    #[cfg(not(target_os = "linux"))]
    #[test]
    fn lists_the_displays() {
        // The JSON must parse (a 1 instead of true for "main" once emptied the list).
        let displays = displays();
        assert!(!displays.is_empty());
        assert_eq!(
            displays.iter().filter(|d| d.main).count(),
            1,
            "{displays:?}"
        );
        assert!(displays.iter().all(|d| d.width > 0 && d.height > 0));
    }

    #[test]
    fn lists_the_cameras() {
        // No camera is needed (CI has none); whatever is listed has a usable size.
        let cameras = cameras();
        assert!(
            cameras
                .iter()
                .all(|c| c.width > 0 && c.height > 0 && c.height <= 720 && !c.id.is_empty())
        );
        assert!(cameras.iter().filter(|c| c.default).count() <= 1);
    }

    #[test]
    fn previews_the_synthetic_camera() {
        let _timing = alone();
        preview_start("", 320, 180, true).unwrap();
        let mut seq = 0;
        let start = std::time::Instant::now();
        let mut pictures = 0;
        while start.elapsed().as_millis() < 1000 {
            if let Some(jpeg) = preview_frame(&mut seq) {
                assert_eq!(&jpeg[..2], &[0xFF, 0xD8], "a JPEG");
                pictures += 1;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        // Nothing new without a new picture.
        let before = seq;
        preview_stop();
        assert_eq!(preview_frame(&mut seq), None);
        assert_eq!(seq, before);
        // CI Macs are VMs without a GPU: Core Image draws the JPEGs in software there.
        let least = if on_ci() { 4 } else { 10 };
        assert!(
            (least..=17).contains(&pictures),
            "{pictures} pictures in 1 s"
        );
    }

    #[test]
    fn records_synthetic_screen_camera_and_mic() {
        let _timing = alongside();
        let dir = temp_dir("basic");
        let rec = Recorder::start(&synthetic(dir.clone(), 640, 360, false)).unwrap();
        wait_for(&rec, 10.0);
        let (status, error) = rec.stop();
        assert_eq!(error, None);
        // Every slot shows a new picture: the synthetic screen changes 60 times a second.
        assert!(status.frames >= 300, "{status:?}");
        assert_eq!(status.frames_repeated, 0, "{status:?}");
        assert_eq!(status.frames_new, status.frames);
        assert_eq!(status.frames_dropped, 0);
        assert_eq!((status.audio_filled, status.audio_dropped), (0, 0));
        // Picture and sound end together (within a frame).
        let video = status.frames as f64 / 30.0;
        let audio = status.audio_samples as f64 / 48_000.0;
        assert!((video - audio).abs() <= 1.0 / 30.0, "{video} vs {audio}");
        // In the files: one capture frame kept per slot (the counter steps by 2), none
        // missing, none repeated.
        let report = check(&dir, false);
        assert_eq!(report.unmatched_markers, 0, "{report:?}");
        assert_eq!(report.frames as i64, status.frames);
        assert_eq!(
            (report.stale, report.unchanged, report.max_step),
            (0, 0, 2),
            "{report:?}"
        );
        // The camera: every slot a new picture (its counter steps by one), as many frames as
        // the screen, none lost.
        assert_eq!(status.camera_frames, status.frames, "{status:?}");
        assert_eq!(status.camera_frames_new, status.camera_frames, "{status:?}");
        assert_eq!(
            (status.camera_frames_dropped, status.camera_frames_lost),
            (0, 0)
        );
        let camera = check_camera(&dir, &status);
        assert_eq!(
            (
                camera.stale,
                camera.unchanged,
                camera.max_step,
                camera.unmatched_markers
            ),
            (0, 0, 1, 0),
            "{camera:?}"
        );
        // The encoder settings the export relies on.
        let screen = crate::probe::probe(&dir.join("screen.mp4")).unwrap();
        let video = screen.video().unwrap();
        assert_eq!(video.codec, "h264");
        assert!(
            profiles().contains(&video.profile.as_deref().unwrap_or("")),
            "{video:?}"
        );
        assert_eq!(video.pix_fmt.as_deref(), Some("yuv420p"));
        assert_eq!(
            video.color,
            Some(["bt709".into(), "bt709".into(), "bt709".into()])
        );
        assert_eq!((video.width, video.height), (Some(640), Some(360)));
        let camera = crate::probe::probe(&dir.join("camera.mp4")).unwrap();
        let cv = camera.video().unwrap();
        assert_eq!(cv.codec, "h264");
        assert!(
            profiles().contains(&cv.profile.as_deref().unwrap_or("")),
            "{cv:?}"
        );
        assert_eq!((cv.width, cv.height), (Some(CAMERA_W), Some(CAMERA_H)));
        let mic = crate::probe::probe(&dir.join("mic.mp4")).unwrap();
        let audio = mic.audio().unwrap();
        assert_eq!(audio.codec, "aac");
        assert_eq!((audio.sample_rate, audio.channels), (Some(48_000), Some(1)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn paces_frames_in_real_time() {
        let _timing = alone();
        // The timer-driven pacer, as with a real screen: synthetic sources on the host clock.
        let dir = temp_dir("realtime");
        let mut config = synthetic(dir.clone(), 640, 360, false);
        config.synthetic = Some(Synthetic::default());
        let rec = Recorder::start(&config).unwrap();
        wait_for(&rec, 4.0);
        let (status, error) = rec.stop();
        assert_eq!(error, None);
        let report = check(&dir, false);
        late_only(&report, status.frames_late, &format!("{status:?}"));
        assert!(
            (report.frames as f64 / 30.0 - status.elapsed).abs() < 0.1,
            "{report:?} {status:?}"
        );
        let camera = check_camera(&dir, &status);
        late_only(&camera, status.camera_frames_late, "camera");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_region_is_cut_out_of_the_whole_display() {
        let _timing = alongside();
        // The made-up display (960 × 600 points) is captured whole at 2 pixels per point and
        // the region cut out on the GPU, as on a real Mac. The pattern is drawn only inside
        // the region and the rest is white: a wrong crop shows as white or a broken counter.
        let dir = temp_dir("region");
        let mut config = synthetic(dir.clone(), 640, 360, false);
        config.screen.as_mut().unwrap().region = Some(Region {
            x: 100.5,
            y: 51.0,
            width: 320.0,
            height: 180.0,
        });
        let rec = Recorder::start(&config).unwrap();
        wait_for(&rec, 4.0);
        let (status, error) = rec.stop();
        assert_eq!(error, None);
        assert_eq!(status.frames_new, status.frames, "{status:?}");
        check(&dir, false);
        // Inside the edges, below the counter rows: the pattern's grey, not the white around it.
        let output = std::process::Command::new(crate::ffmpeg::locate("ffmpeg").unwrap())
            .args(["-v", "error", "-ss", "2", "-i"])
            .arg(dir.join("screen.mp4"))
            .args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "gray", "-"])
            .output()
            .unwrap();
        let luma = |x: usize, y: usize| output.stdout[y * 640 + x];
        for (x, y) in [(2, 200), (637, 200), (320, 357), (2, 357), (637, 357)] {
            assert!(luma(x, y).abs_diff(128) < 12, "{x},{y}: {}", luma(x, y));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn records_the_computer_sound() {
        let _timing = alongside();
        // The made-up computer sound clicks with the mic: stereo AAC in system.mp4, as long
        // as the picture, each click on its whole second.
        let dir = temp_dir("system");
        let mut config = synthetic(dir.clone(), 640, 360, false);
        config.system_audio = true;
        let rec = Recorder::start(&config).unwrap();
        wait_for(&rec, 5.0);
        let (status, error) = rec.stop();
        assert_eq!(error, None);
        assert_eq!((status.system_filled, status.system_dropped), (0, 0));
        let video = status.frames as f64 / 30.0;
        let sound = status.system_samples as f64 / 48_000.0;
        assert!((video - sound).abs() <= 1.0 / 30.0, "{video} vs {sound}");
        let info = crate::probe::probe(&dir.join("system.mp4")).unwrap();
        let audio = info.audio().unwrap();
        assert_eq!(
            (audio.codec.as_str(), audio.sample_rate, audio.channels),
            ("aac", Some(48_000), Some(2))
        );
        let (samples, start) = crate::testpattern::read_audio(&dir.join("system.mp4")).unwrap();
        let clicks = crate::testpattern::onsets(&samples, start, 0.1, 0.3);
        assert!(clicks.len() >= 4, "{clicks:?}");
        for c in &clicks {
            assert!((c - c.round()).abs() <= 1.0 / 30.0, "{clicks:?}");
        }
        let report = crate::check::check(&dir, Some(&status), 30);
        assert!(report.problems.is_empty(), "{report:?}");
        assert!(report.system.is_some(), "{report:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_closed_window_keeps_its_last_picture() {
        let _timing = alongside();
        // The made-up window closes 2 s in: the recording goes on to the stop with its last
        // picture, and the check says when the window closed.
        let dir = temp_dir("window");
        let mut config = synthetic(dir.clone(), 640, 400, false);
        config.screen.as_mut().unwrap().window = Some(7);
        let rec = Recorder::start(&config).unwrap();
        wait_for(&rec, 5.0);
        let (status, error) = rec.stop();
        assert_eq!(error, None);
        assert!(status.window_closed, "{status:?}");
        assert!((status.window_closed_at - 2.0).abs() < 0.1, "{status:?}");
        assert!(status.frames as f64 >= 5.0 * 30.0 - 1.0, "{status:?}");
        assert!(status.frames_repeated as f64 >= 2.9 * 30.0, "{status:?}");
        let report = crate::check::check(&dir, Some(&status), 30);
        assert!(report.problems.is_empty(), "{report:?}");
        assert!(
            report
                .notes
                .iter()
                .any(|n| n.starts_with("The window closed at 0:02")),
            "{report:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn devices_starting_late_fill_the_start() {
        let _timing = alongside();
        // A real camera's first picture comes ~0.9 s after the start: the slots before it
        // show it, then every slot gets its own picture, and the camera keeps the screen's
        // length. The mic's first sound comes ~0.35 s after the start: silence before it,
        // not reported as a gap.
        let dir = temp_dir("late-start");
        let mut config = synthetic(dir.clone(), 640, 360, false);
        config.synthetic = Some(Synthetic {
            fast: true,
            late_start: true,
            ..Synthetic::default()
        });
        let rec = Recorder::start(&config).unwrap();
        wait_for(&rec, 5.0);
        let (status, error) = rec.stop();
        assert_eq!(error, None);
        assert_eq!(status.camera_frames, status.frames, "{status:?}");
        assert_eq!(status.camera_frames_repeated, 30, "{status:?}");
        assert_eq!(
            status.camera_frames_new,
            status.camera_frames - 30,
            "{status:?}"
        );
        assert_eq!((status.audio_filled, status.audio_dropped), (0, 0));
        let video = status.frames as f64 / 30.0;
        let audio = status.audio_samples as f64 / 48_000.0;
        assert!((video - audio).abs() <= 1.0 / 30.0, "{video} vs {audio}");
        let report = crate::check::check(&dir, Some(&status), 30);
        assert!(report.problems.is_empty(), "{report:?}");
        assert!(report.notes.is_empty(), "{report:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A real-time recording repeats a slot only when its frame arrived after the slot was
    /// written, never when it came on time; and that stays rare. Late frames happen when the
    /// machine pauses the generator: ~100 ms pauses on the dev Mac even with nothing else
    /// running, more on CI's VMs. The recorder counts them; a real screen behaves the same.
    fn late_only(report: &Report, late: i64, what: &str) {
        assert!(
            report.stale + report.unchanged <= late as usize,
            "repeated slots without late frames: {report:?} {what}"
        );
        assert!(
            late as usize <= report.frames / 10,
            "too many late frames ({late}): {report:?} {what}"
        );
    }

    /// Run by `survives_being_killed` in a child process: records until killed.
    #[test]
    #[ignore]
    fn crash_child() {
        let Some(dir) = std::env::var_os("KREOKIT_REC_CRASH_DIR") else {
            return;
        };
        let dir = PathBuf::from(dir);
        let mut config = synthetic(dir.clone(), 640, 360, false);
        config.synthetic = Some(Synthetic::default());
        let rec = Recorder::start(&config).unwrap();
        loop {
            // What was recorded so far and how many frames came late, for the parent.
            let status = rec.status();
            let line = format!("{} {}", status.elapsed, status.frames_late);
            std::fs::write(dir.join("elapsed.tmp"), line).unwrap();
            std::fs::rename(dir.join("elapsed.tmp"), dir.join("elapsed")).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    #[test]
    fn survives_being_killed() {
        let _timing = alone();
        let dir = temp_dir("crash");
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", "native::tests::crash_child"])
            .env("KREOKIT_REC_CRASH_DIR", &dir)
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let progress = |field: usize| -> f64 {
            std::fs::read_to_string(dir.join("elapsed"))
                .ok()
                .and_then(|s| s.split(' ').nth(field)?.parse().ok())
                .unwrap_or(0.0)
        };
        let elapsed = || progress(0);
        let start = std::time::Instant::now();
        while elapsed() < 6.5 {
            assert!(start.elapsed().as_secs() < 60, "the child didn't record");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        child.kill().unwrap(); // SIGKILL: no chance to finish the files
        child.wait().unwrap();
        let recorded = elapsed();
        let late = progress(1) as i64;
        // Every file plays as it is, and loses at most 2 s (3 s on CI's VMs, whose
        // software encoder runs behind).
        let limit = if on_ci() { 3.0 } else { 2.0 };
        for name in ["screen.mp4", "camera.mp4", "mic.mp4"] {
            let info = crate::probe::probe(&dir.join(name)).unwrap();
            eprintln!("{name}: {:.2} s of {recorded:.2} s kept", info.duration_s);
            assert!(
                recorded - info.duration_s <= limit && info.duration_s > 3.0,
                "{name}: {} of {recorded} s",
                info.duration_s
            );
        }
        let report = analyze_dir(&dir, &Geometry::synthetic(640, 60, false), 30.0).unwrap();
        assert_eq!(report.bad, 0, "{report:?}");
        late_only(&report, late, "killed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An hour on the synthetic sources' own clock (a few minutes): drift must stay under
    /// one frame. `cargo test -- --ignored simulated_hour`.
    #[test]
    #[ignore]
    fn simulated_hour() {
        let _timing = alongside();
        let dir = temp_dir("hour");
        let rec = Recorder::start(&synthetic(dir.clone(), 340, 192, false)).unwrap();
        wait_for(&rec, 1200.0);
        rec.pause();
        rec.resume();
        wait_for(&rec, 3600.0);
        let (status, error) = rec.stop();
        assert_eq!(error, None);
        let report = analyze_dir(&dir, &Geometry::synthetic(340, 60, false), 30.0).unwrap();
        eprintln!("{status:?}\n{report:?}");
        assert_eq!(
            (report.stale, report.bad, report.irregular_pts),
            (0, 0, 0),
            "{report:?}"
        );
        assert!(report.markers >= 3598, "{report:?}");
        let worst = report
            .sync_ms_min
            .unwrap()
            .abs()
            .max(report.sync_ms_max.unwrap().abs());
        assert!(worst <= 1000.0 / 30.0, "{report:?}");
        // The camera over the same hour: no gaps, no drift.
        let camera = check_camera(&dir, &status);
        eprintln!("camera: {camera:?}");
        assert_eq!(camera.stale, 0, "{camera:?}");
        assert!(camera.markers >= 3598, "{camera:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn repeats_the_picture_only_when_nothing_changed() {
        let _timing = alongside();
        let dir = temp_dir("still");
        let rec = Recorder::start(&synthetic(dir.clone(), 640, 360, true)).unwrap();
        wait_for(&rec, 20.0);
        let (status, error) = rec.stop();
        assert_eq!(error, None);
        let report = check(&dir, true);
        assert_eq!(report.unmatched_markers, 0, "{report:?}");
        // Two still periods of 2 s: about 120 repeated frames, all while nothing changed.
        assert_eq!(report.stale, 0, "{report:?}");
        assert!((115..=125).contains(&report.unchanged), "{report:?}");
        assert_eq!(status.frames_repeated as usize, report.unchanged);
        assert_eq!(report.fresh_percent, 100.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pause_and_resume_keep_sync() {
        let _timing = alongside();
        let dir = temp_dir("pause");
        let rec = Recorder::start(&synthetic(dir.clone(), 640, 360, false)).unwrap();
        for (until, pause) in [(3.3, true), (3.3, false), (7.1, true), (7.1, false)] {
            if pause {
                wait_for(&rec, until);
                rec.pause();
                // The generator runs on while paused; nothing of it is recorded.
                std::thread::sleep(std::time::Duration::from_millis(30));
                assert!(rec.status().paused);
            } else {
                rec.resume();
            }
        }
        wait_for(&rec, 12.0);
        let (status, error) = rec.stop();
        assert_eq!(error, None);
        let report = check(&dir, false);
        // The files are continuous (no gap, regular timestamps) and the sound still lines up
        // with the picture after both pauses; the counter jumps over the paused time.
        assert_eq!(report.stale, 0, "{report:?}");
        assert!(report.max_step > 2, "{report:?}");
        // At most one marker per pause loses its click to the pause.
        assert!(report.unmatched_markers <= 2, "{report:?}");
        let video = status.frames as f64 / 30.0;
        let audio = status.audio_samples as f64 / 48_000.0;
        assert!((video - audio).abs() <= 1.0 / 30.0, "{video} vs {audio}");
        // The pauses aren't counted. (Against the recorder's own length: on a slow machine
        // the fast synthetic clock runs on past 12 s before the test's stop arrives.)
        assert!(status.elapsed >= 12.0, "{status:?}");
        assert!(
            (video - status.elapsed).abs() <= 1.0 / 30.0 + 1e-6,
            "{video} vs {}",
            status.elapsed
        );
        // The camera pauses with the screen and stays in sync too.
        assert_eq!(status.camera_frames, status.frames, "{status:?}");
        let camera = check_camera(&dir, &status);
        assert_eq!(camera.stale, 0, "{camera:?}");
        assert!(camera.unmatched_markers <= 2, "{camera:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

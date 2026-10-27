//! The system recorder: on macOS the Objective-C recorder in `native/macos` (its rules are
//! in recorder.h); Windows and Linux follow in milestone 3.

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
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordConfig {
    pub dir: PathBuf,
    pub screen: Option<ScreenConfig>,
    /// The microphone's id (`None`: no microphone; `Some("")`: the default one).
    pub mic: Option<String>,
    pub camera: Option<CameraConfig>,
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
    pub paused: bool,
    pub failed: bool,
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
        mic: c_int,
        mic_id: *const c_char,
        camera: c_int,
        camera_id: *const c_char,
        camera_width: c_int,
        camera_height: c_int,
        camera_bitrate: c_int,
        synthetic: c_int,
        synthetic_fast: c_int,
        synthetic_static: c_int,
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
        fn kr_free(s: *mut c_char);
        fn kr_overlay_window(ns_window: *mut std::ffi::c_void);
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
    pub struct Recorder(*mut c_void);

    // The native recorder serializes every call on its own queue.
    unsafe impl Send for Recorder {}
    unsafe impl Sync for Recorder {}

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
                mic: config.mic.is_some() as c_int,
                mic_id: mic_id.as_ptr(),
                camera: camera.is_some() as c_int,
                camera_id: camera_id.as_ptr(),
                camera_width: camera.map_or(0, |c| c.width as c_int),
                camera_height: camera.map_or(0, |c| c.height as c_int),
                camera_bitrate: camera.map_or(0, |c| c.bitrate as c_int),
                synthetic: config.synthetic.is_some() as c_int,
                synthetic_fast: synthetic.fast as c_int,
                synthetic_static: synthetic.still_periods as c_int,
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
}

#[cfg(not(target_os = "macos"))]
pub use other::*;

/// Windows and Linux: recording arrives in milestone 3.
#[cfg(not(target_os = "macos"))]
mod other {
    use super::*;

    const NOT_YET: &str = "Recording on this system arrives in a later version of KreoKit Rec. For now it records on macOS.";

    pub struct Recorder;

    impl Recorder {
        pub fn start(_config: &RecordConfig) -> Result<Recorder, String> {
            Err(NOT_YET.into())
        }
        pub fn pause(&self) {}
        pub fn resume(&self) {}
        pub fn status(&self) -> Status {
            Status::default()
        }
        pub fn stop(self) -> (Status, Option<String>) {
            (Status::default(), None)
        }
    }

    pub fn screen_access() -> Access {
        Access::Allowed
    }
    pub fn request_screen_access() {}
    pub fn mic_access() -> Access {
        Access::Allowed
    }
    pub fn request_mic_access() -> Access {
        Access::Allowed
    }
    pub fn camera_access() -> Access {
        Access::Allowed
    }
    pub fn request_camera_access() -> Access {
        Access::Allowed
    }
    pub fn displays() -> Vec<Display> {
        Vec::new()
    }
    pub fn mics() -> Vec<Mic> {
        Vec::new()
    }
    pub fn cameras() -> Vec<Camera> {
        Vec::new()
    }
    pub fn monitor_start(_mic_id: &str) -> Result<(), String> {
        Err(NOT_YET.into())
    }
    pub fn monitor_peak() -> f32 {
        0.0
    }
    pub fn monitor_stop() {}
    pub fn preview_start(_id: &str, _w: u32, _h: u32, _synthetic: bool) -> Result<(), String> {
        Err(NOT_YET.into())
    }
    pub fn preview_frame(_seq: &mut u64) -> Option<Vec<u8>> {
        None
    }
    pub fn preview_stop() {}
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

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
            synthetic: Some(Synthetic {
                fast: true,
                still_periods: still,
            }),
        }
    }

    /// CI Macs are VMs without a GPU or hardware encoder, so timing limits are looser there.
    pub fn on_ci() -> bool {
        std::env::var_os("CI").is_some()
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
        assert_eq!(
            (video.codec.as_str(), video.profile.as_deref()),
            ("h264", Some("High"))
        );
        assert_eq!(video.pix_fmt.as_deref(), Some("yuv420p"));
        assert_eq!(
            video.color,
            Some(["bt709".into(), "bt709".into(), "bt709".into()])
        );
        assert_eq!((video.width, video.height), (Some(640), Some(360)));
        let camera = crate::probe::probe(&dir.join("camera.mp4")).unwrap();
        let cv = camera.video().unwrap();
        assert_eq!(
            (cv.codec.as_str(), cv.profile.as_deref()),
            ("h264", Some("High"))
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
        // The timer-driven pacer, as with a real screen: synthetic sources on the host clock.
        let dir = temp_dir("realtime");
        let mut config = synthetic(dir.clone(), 640, 360, false);
        config.synthetic = Some(Synthetic::default());
        let rec = Recorder::start(&config).unwrap();
        wait_for(&rec, 4.0);
        let (status, error) = rec.stop();
        assert_eq!(error, None);
        let report = check(&dir, false);
        // On a loaded CI VM (no GPU, no hardware encoder, every test at once) a few slots
        // can repeat; on a real Mac none may.
        let allowed = if on_ci() { report.frames / 20 } else { 0 };
        assert!(
            report.stale + report.unchanged <= allowed,
            "{report:?} {status:?}"
        );
        assert!(
            (report.frames as f64 / 30.0 - status.elapsed).abs() < 0.1,
            "{report:?} {status:?}"
        );
        let camera = check_camera(&dir, &status);
        assert!(camera.stale + camera.unchanged <= allowed, "{camera:?}");
        let _ = std::fs::remove_dir_all(&dir);
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
            // What was recorded so far, for the parent to compare with the files.
            let elapsed = rec.status().elapsed;
            std::fs::write(dir.join("elapsed.tmp"), elapsed.to_string()).unwrap();
            std::fs::rename(dir.join("elapsed.tmp"), dir.join("elapsed")).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    #[test]
    fn survives_being_killed() {
        let dir = temp_dir("crash");
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", "native::tests::crash_child"])
            .env("KREOKIT_REC_CRASH_DIR", &dir)
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let elapsed = || -> f64 {
            std::fs::read_to_string(dir.join("elapsed"))
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.0)
        };
        let start = std::time::Instant::now();
        while elapsed() < 6.5 {
            assert!(start.elapsed().as_secs() < 60, "the child didn't record");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        child.kill().unwrap(); // SIGKILL: no chance to finish the files
        child.wait().unwrap();
        let recorded = elapsed();
        // Every file plays as it is, and loses at most 2 s.
        for name in ["screen.mp4", "camera.mp4", "mic.mp4"] {
            let info = crate::probe::probe(&dir.join(name)).unwrap();
            eprintln!("{name}: {:.2} s of {recorded:.2} s kept", info.duration_s);
            assert!(
                recorded - info.duration_s <= 2.0 && info.duration_s > 3.0,
                "{name}: {} of {recorded} s",
                info.duration_s
            );
        }
        let report = analyze_dir(&dir, &Geometry::synthetic(640, 60, false), 30.0).unwrap();
        assert_eq!((report.stale, report.bad), (0, 0), "{report:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An hour on the synthetic sources' own clock (a few minutes): drift must stay under
    /// one frame. `cargo test -- --ignored simulated_hour`.
    #[test]
    #[ignore]
    fn simulated_hour() {
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

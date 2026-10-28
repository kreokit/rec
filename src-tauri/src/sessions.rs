//! Recording sessions: one folder per recording in the app data folder (`screen.mp4`,
//! `camera.mp4`, `mic.mp4`, `session.json`), the states idle → recording ⇄ paused → finishing → done, and
//! the Recent list.
//!
//! `session.json` is rewritten (atomically) on every change and once a second while
//! recording, so after a crash the next start knows what was being recorded and for how
//! long (recovery.rs).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::check::{self, Check};
use crate::native::{CameraConfig, RecordConfig, Region, ScreenApi, ScreenConfig, Status};
use crate::{awake, disk};

/// Recording stops by itself when less than this is free (the files stay playable).
pub const STOP_BELOW_FREE: u64 = 500_000_000;
/// A recording doesn't start with less than this free.
pub const START_BELOW_FREE: u64 = 2_000_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// The countdown runs: devices open, nothing recorded yet (`begin` starts it, `cancel`
    /// throws it away).
    Countdown,
    Recording,
    Paused,
    /// Stopped; the files are being finished and checked.
    Finishing,
    Done,
    /// Finished after a crash (recovery.rs).
    Recovered,
    /// Nothing usable (e.g. the app crashed within the first second).
    Failed,
}

impl State {
    /// Still marked as running: after a restart this means the app stopped unexpectedly.
    pub fn unfinished(self) -> bool {
        matches!(self, State::Recording | State::Paused | State::Finishing)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScreenInfo {
    pub display: String,
    /// Part of the display (points); `None`: all of it.
    #[serde(default)]
    pub region: Option<Region>,
    /// A window instead ("App – Title"); `None`: the display.
    #[serde(default)]
    pub window: Option<String>,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// The app's own windows were kept out of the picture (otherwise they're recorded where
    /// they show).
    #[serde(default)]
    pub own_windows_kept_out: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CameraInfo {
    pub name: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    /// The folder's name, e.g. "2026-10-27-143205".
    pub id: String,
    /// Unix milliseconds.
    pub created_at: u64,
    pub state: State,
    pub screen: Option<ScreenInfo>,
    /// The microphone's name.
    pub mic: Option<String>,
    #[serde(default)]
    pub camera: Option<CameraInfo>,
    /// The computer's sound was recorded too (system.mp4).
    #[serde(default)]
    pub system_audio: bool,
    /// Seconds recorded (pauses excluded); while recording, as of the last heartbeat.
    pub elapsed_s: f64,
    pub pauses: u32,
    pub check: Option<Check>,
    /// What went wrong, in words for the user.
    pub error: Option<String>,
    /// Removed from Recent (the files are kept).
    #[serde(default)]
    pub hidden: bool,
}

/// What the setup screen chose.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Options {
    pub display_id: u32,
    pub display_name: String,
    pub display_width: u32,
    pub display_height: u32,
    /// 1080 or 720.
    pub quality: u32,
    /// `None`: no microphone; `Some("")`: the default one.
    pub mic_id: Option<String>,
    pub mic_name: Option<String>,
    /// `None`: no camera; `Some("")`: the default one.
    #[serde(default)]
    pub camera_id: Option<String>,
    #[serde(default)]
    pub camera_name: Option<String>,
    /// The camera's size (`Camera` in native.rs): recorded as it is.
    #[serde(default)]
    pub camera_width: u32,
    #[serde(default)]
    pub camera_height: u32,
    /// The display's width in points (`Display` in native.rs), to turn the region into
    /// pixels.
    #[serde(default)]
    pub display_points_width: f64,
    /// Part of the display; `None`: all of it.
    #[serde(default)]
    pub region: Option<Region>,
    /// A window instead of the display.
    #[serde(default)]
    pub window: Option<WindowChoice>,
    /// The computer's sound (macOS 13+).
    #[serde(default)]
    pub system_audio: bool,
    /// Start after a countdown: held until `begin` (RecordConfig::hold).
    #[serde(default)]
    pub countdown: bool,
    /// The app's own windows to keep out of the picture (set by the app, not the setup
    /// screen).
    #[serde(skip)]
    pub own_windows: Vec<u32>,
}

/// The window the setup screen chose (`Window` in native.rs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowChoice {
    pub id: u32,
    /// "App – Title", for the user.
    pub name: String,
    /// In points, and the display's pixels per point.
    pub width: f64,
    pub height: f64,
    pub scale: f64,
}

/// The smallest region, in points each way.
pub const MIN_REGION: f64 = 64.0;

/// What is captured, in pixels: the region on a display `width` × `height` pixels and
/// `points_width` points wide, or the whole display.
pub fn source_size(
    width: u32,
    height: u32,
    points_width: f64,
    region: Option<Region>,
) -> (u32, u32) {
    match region {
        Some(r) if points_width > 0.0 => {
            let scale = width as f64 / points_width;
            (
                (r.width * scale).round() as u32,
                (r.height * scale).round() as u32,
            )
        }
        _ => (width, height),
    }
}

/// The output size for a display at a quality: the display's shape, `quality` pixels high
/// (or the display's own size if smaller), even numbers, at most 3840 wide.
pub fn output_size(width: u32, height: u32, quality: u32) -> (u32, u32) {
    let scale = (quality as f64 / height as f64)
        .min(3840.0 / width as f64)
        .min(1.0);
    let even = |v: f64| ((v.round() as u32) & !1).max(2);
    (even(width as f64 * scale), even(height as f64 * scale))
}

/// The camera's recording size: its own (up to 720p, as the system reports it), even
/// numbers; 1280x720 when unknown.
pub fn camera_size(width: u32, height: u32) -> (u32, u32) {
    if width == 0 || height == 0 {
        return (1280, 720);
    }
    output_size(width, height, 720)
}

/// H.264 bits per second for a size at 30 fps (≈ 7.3 Mbit/s at 1728x1080).
pub fn video_bitrate(width: u32, height: u32) -> u32 {
    (width as f64 * height as f64 * 30.0 * 0.13) as u32
}

/// The expected file size per minute: screen + camera (`None`: off) + 128 kbit/s sound.
pub fn bytes_per_minute(
    width: u32,
    height: u32,
    camera: Option<(u32, u32)>,
    mic: bool,
    system_audio: bool,
) -> u64 {
    let camera = camera.map_or(0, |(w, h)| video_bitrate(w, h) as u64);
    let sound = if mic { 128_000 } else { 0 } + if system_audio { 256_000 } else { 0 };
    (video_bitrate(width, height) as u64 + camera + sound) / 8 * 60
}

/// A window's size in pixels, as big as it is on screen.
pub fn window_size(w: &WindowChoice) -> (u32, u32) {
    (
        (w.width * w.scale).round().max(2.0) as u32,
        (w.height * w.scale).round().max(2.0) as u32,
    )
}

/// A running recording as the backend sees it (native.rs, or a fake in tests).
pub trait Capture: Send + Sync {
    fn pause(&self);
    fn resume(&self);
    fn status(&self) -> Status;
    fn stop(self: Box<Self>) -> (Status, Option<String>);
    /// Whether the app's own windows (`ScreenConfig::exclude_windows`) are kept out.
    fn own_windows_kept_out(&self) -> bool {
        false
    }
    /// Small pictures of the recorded camera for the bubble (`width` wide); false without.
    fn camera_preview(&self, _width: u32) -> bool {
        false
    }
}

pub trait Backend: Send + Sync {
    fn start(&self, config: &RecordConfig) -> Result<Box<dyn Capture>, String>;
}

pub struct NativeBackend;

impl Capture for crate::native::Recorder {
    fn pause(&self) {
        crate::native::Recorder::pause(self)
    }
    fn resume(&self) {
        crate::native::Recorder::resume(self)
    }
    fn status(&self) -> Status {
        crate::native::Recorder::status(self)
    }
    fn stop(self: Box<Self>) -> (Status, Option<String>) {
        crate::native::Recorder::stop(*self)
    }
    fn own_windows_kept_out(&self) -> bool {
        crate::native::Recorder::own_windows_kept_out(self)
    }
    fn camera_preview(&self, width: u32) -> bool {
        crate::native::Recorder::preview(self, width)
    }
}

impl Backend for NativeBackend {
    fn start(&self, config: &RecordConfig) -> Result<Box<dyn Capture>, String> {
        Ok(Box::new(crate::native::Recorder::start(config)?))
    }
}

/// While recording: what the recording screen shows.
#[derive(Debug, Clone, Serialize)]
pub struct Live {
    pub session: Session,
    pub status: Status,
    pub free_bytes: Option<u64>,
    /// Minutes until the disk-space stop at this recording's size per minute.
    pub minutes_left: Option<f64>,
}

struct Active {
    session: Session,
    dir: PathBuf,
    capture: Box<dyn Capture>,
    /// The expected size per minute (`bytes_per_minute`), for the minutes left.
    bytes_per_minute: u64,
}

pub struct Recordings {
    root: PathBuf,
    backend: Box<dyn Backend>,
    /// Every recording from synthetic sources (dev and test builds, `KREOKIT_REC_SYNTHETIC`).
    pub synthetic: Option<crate::native::Synthetic>,
    /// `START_BELOW_FREE` and `STOP_BELOW_FREE` (changed only by measurements).
    pub start_below_free: u64,
    pub stop_below_free: u64,
    /// Called (from the heartbeat thread) when the disk is almost full: the recording is
    /// paused for good by then, and the app stops and finishes it (`stop`).
    pub on_disk_full: Option<Arc<dyn Fn() + Send + Sync>>,
    active: Arc<Mutex<Option<Active>>>,
    heartbeat: Mutex<Option<(Arc<AtomicBool>, JoinHandle<()>)>>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// "2026-10-27-143205" in local time.
fn folder_name(ms: u64) -> String {
    #[cfg(unix)]
    {
        let secs = (ms / 1000) as libc::time_t;
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        // SAFETY: valid pointers; localtime_r is thread-safe.
        unsafe { libc::localtime_r(&secs, &mut tm) };
        format!(
            "{:04}-{:02}-{:02}-{:02}{:02}{:02}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec
        )
    }
    #[cfg(windows)]
    {
        // GetLocalTime; ms is the current time when a recording starts.
        #[repr(C)]
        #[derive(Default)]
        struct SystemTime {
            year: u16,
            month: u16,
            day_of_week: u16,
            day: u16,
            hour: u16,
            minute: u16,
            second: u16,
            milliseconds: u16,
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetLocalTime(time: *mut SystemTime);
        }
        let _ = ms;
        let mut t = SystemTime::default();
        // SAFETY: a valid pointer to a SYSTEMTIME-shaped struct.
        unsafe { GetLocalTime(&mut t) };
        format!(
            "{:04}-{:02}-{:02}-{:02}{:02}{:02}",
            t.year, t.month, t.day, t.hour, t.minute, t.second
        )
    }
}

pub fn write_session(dir: &Path, session: &Session) -> Result<(), String> {
    let json = serde_json::to_string_pretty(session).map_err(|e| e.to_string())?;
    let temp = dir.join("session.json.tmp");
    if let Err(e) = fs::write(&temp, json) {
        let _ = fs::remove_file(&temp);
        return Err(format!("cannot save the recording's details: {e}"));
    }
    fs::rename(&temp, dir.join("session.json"))
        .map_err(|e| format!("cannot save the recording's details: {e}"))
}

/// What the recording says when its details couldn't be saved at the stop (the disk was
/// full): the files are finished and checked anyway, and the next start offers to recover it.
/// Minutes of recording before the disk-space stop, at `bytes_per_minute`.
pub fn minutes_left(free: u64, stop_below_free: u64, bytes_per_minute: u64) -> f64 {
    free.saturating_sub(stop_below_free) as f64 / bytes_per_minute.max(1) as f64
}

/// The note on a recording stopped by the disk-space stop.
pub const DISK_FULL_STOP: &str = "The recording stopped by itself because the disk is almost full. Everything up to that moment is kept. Free up some space before the next recording.";

const DETAILS_NOT_SAVED: &str = "The disk was full when the recording stopped. Its files are kept: free up some space, then reopen KreoKit Rec to recover it.";

pub fn read_session(dir: &Path) -> Option<Session> {
    serde_json::from_str(&fs::read_to_string(dir.join("session.json")).ok()?).ok()
}

impl Recordings {
    pub fn new(root: PathBuf, backend: Box<dyn Backend>) -> Recordings {
        Recordings {
            root,
            backend,
            synthetic: None,
            start_below_free: START_BELOW_FREE,
            stop_below_free: STOP_BELOW_FREE,
            on_disk_full: None,
            active: Arc::new(Mutex::new(None)),
            heartbeat: Mutex::new(None),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The folder of recording `id`; refuses ids that would point elsewhere.
    pub fn dir(&self, id: &str) -> Result<PathBuf, String> {
        if id.is_empty() || id.contains(['/', '\\']) || id.starts_with('.') {
            return Err("unknown recording".into());
        }
        Ok(self.root.join(id))
    }

    pub fn is_recording(&self) -> bool {
        self.active.lock().unwrap().is_some()
    }

    pub fn start(&self, options: &Options) -> Result<Session, String> {
        self.start_with(options, self.synthetic)
    }

    /// `synthetic`: generated sources instead of the real screen and microphone (tests).
    pub fn start_with(
        &self,
        options: &Options,
        synthetic: Option<crate::native::Synthetic>,
    ) -> Result<Session, String> {
        let mut active = self.active.lock().unwrap();
        if active.is_some() {
            return Err("A recording is already running. Stop it first.".into());
        }
        fs::create_dir_all(&self.root)
            .map_err(|e| format!("The recordings folder can't be created ({e}). Check that the disk isn't full or read-only."))?;
        if disk::free_bytes(&self.root).is_some_and(|free| free < self.start_below_free) {
            return Err(format!(
                "Less than {} of disk space is free. Free up some space, then record again.",
                disk::gigabytes(self.start_below_free)
            ));
        }
        let created_at = now_ms();
        let mut id = folder_name(created_at);
        let mut n = 2;
        while self.root.join(&id).exists() {
            id = format!("{}-{n}", folder_name(created_at));
            n += 1;
        }
        if options.window.is_none()
            && let Some(r) = options.region
            && (r.width < MIN_REGION || r.height < MIN_REGION)
        {
            return Err(format!(
                "The region is too small to record. Choose one at least {MIN_REGION} × {MIN_REGION} points."
            ));
        }
        let dir = self.root.join(&id);
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        if options.system_audio
            && self.synthetic.is_none()
            && !crate::native::system_audio_supported()
        {
            return Err("Recording the computer sound needs macOS 13 or newer. Turn it off, then record again.".into());
        }
        let window = options.window.as_ref();
        let region = if window.is_some() {
            None
        } else {
            options.region
        };
        let (source_w, source_h) = match window {
            Some(w) => window_size(w),
            None => source_size(
                options.display_width,
                options.display_height,
                options.display_points_width,
                region,
            ),
        };
        let (width, height) = output_size(source_w, source_h, options.quality);
        let camera = options
            .camera_id
            .as_ref()
            .map(|_| camera_size(options.camera_width, options.camera_height));
        let mut session = Session {
            id,
            created_at,
            state: if options.countdown {
                State::Countdown
            } else {
                State::Recording
            },
            screen: Some(ScreenInfo {
                display: options.display_name.clone(),
                region,
                window: window.map(|w| w.name.clone()),
                width,
                height,
                fps: 30,
                own_windows_kept_out: false,
            }),
            mic: options.mic_id.as_ref().map(|_| {
                options
                    .mic_name
                    .clone()
                    .unwrap_or_else(|| "Microphone".into())
            }),
            camera: camera.map(|(width, height)| CameraInfo {
                name: options
                    .camera_name
                    .clone()
                    .unwrap_or_else(|| "Camera".into()),
                width,
                height,
            }),
            system_audio: options.system_audio,
            elapsed_s: 0.0,
            pauses: 0,
            check: None,
            error: None,
            hidden: false,
        };
        write_session(&dir, &session)?;
        let config = RecordConfig {
            hold: options.countdown,
            dir: dir.clone(),
            screen: Some(ScreenConfig {
                display_id: options.display_id,
                region,
                window: window.map(|w| w.id),
                exclude_windows: options.own_windows.clone(),
                width,
                height,
                fps: 30,
                capture_fps: 60,
                api: ScreenApi::Auto,
                bitrate: video_bitrate(width, height),
            }),
            mic: options.mic_id.clone(),
            camera: camera.map(|(width, height)| CameraConfig {
                id: options.camera_id.clone().unwrap_or_default(),
                width,
                height,
                bitrate: video_bitrate(width, height),
            }),
            system_audio: options.system_audio,
            synthetic,
        };
        let capture = match self.backend.start(&config) {
            Ok(capture) => capture,
            Err(e) => {
                // Nothing was recorded: no folder to recover later.
                let _ = fs::remove_dir_all(&dir);
                return Err(e);
            }
        };
        if let Some(screen) = session.screen.as_mut() {
            screen.own_windows_kept_out = capture.own_windows_kept_out();
            let _ = write_session(&dir, &session);
        }
        let bytes_per_minute = bytes_per_minute(
            width,
            height,
            camera,
            options.mic_id.is_some(),
            options.system_audio,
        );
        *active = Some(Active {
            session: session.clone(),
            dir,
            capture,
            bytes_per_minute,
        });
        drop(active);
        self.start_heartbeat();
        Ok(session)
    }

    /// Once a second: the length so far into session.json, and a safe stop before the
    /// disk is full. Holds the keep-awake guard for the whole recording.
    fn start_heartbeat(&self) {
        let stop = Arc::new(AtomicBool::new(false));
        let active = self.active.clone();
        let flag = stop.clone();
        let stop_below_free = self.stop_below_free;
        let on_disk_full = self.on_disk_full.clone();
        let handle = std::thread::spawn(move || {
            let _awake = awake::KeepAwake::new("recording");
            while !flag.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(250));
                let mut guard = active.lock().unwrap();
                let Some(a) = guard.as_mut() else { break };
                let mut full = false;
                let status = a.capture.status();
                a.session.elapsed_s = status.elapsed;
                let _ = write_session(&a.dir, &a.session);
                if a.session.error.is_none()
                    && a.session.state != State::Countdown
                    && disk::free_bytes(&a.dir).is_some_and(|free| free < stop_below_free)
                {
                    a.session.error = Some(DISK_FULL_STOP.into());
                    // Nothing more is written; the app stops and finishes it right away
                    // (`on_disk_full`), with room to spare (docs/m5-results.md, step 7).
                    a.capture.pause();
                    a.session.state = State::Paused;
                    let _ = write_session(&a.dir, &a.session);
                    full = true;
                }
                drop(guard);
                if full {
                    if let Some(stop) = &on_disk_full {
                        stop();
                    }
                    break;
                }
            }
        });
        *self.heartbeat.lock().unwrap() = Some((stop, handle));
    }

    fn stop_heartbeat(&self) {
        if let Some((stop, handle)) = self.heartbeat.lock().unwrap().take() {
            stop.store(true, Ordering::SeqCst);
            let _ = handle.join();
        }
    }

    pub fn pause(&self) -> Result<Session, String> {
        self.change(|a| {
            if a.session.state == State::Recording {
                a.capture.pause();
                a.session.state = State::Paused;
                a.session.pauses += 1;
            }
        })
    }

    pub fn resume(&self) -> Result<Session, String> {
        self.change(|a| {
            if a.session.state == State::Paused && a.session.error.is_none() {
                a.capture.resume();
                a.session.state = State::Recording;
            }
        })
    }

    /// The bubble's pictures: small JPEGs of the recorded camera (`width` wide).
    pub fn camera_preview(&self, width: u32) -> bool {
        self.active
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|a| a.capture.camera_preview(width))
    }

    /// The countdown is over: the recording starts now.
    pub fn begin(&self) -> Result<Session, String> {
        self.change(|a| {
            if a.session.state == State::Countdown {
                a.capture.resume();
                a.session.state = State::Recording;
            }
        })
    }

    /// The countdown was cancelled: nothing was recorded, so nothing is kept.
    pub fn cancel(&self) -> Result<(), String> {
        let mut guard = self.active.lock().unwrap();
        match guard.as_ref().map(|a| a.session.state) {
            Some(State::Countdown) => {}
            // Still starting (start_recording throws it away once it has started).
            None => return Ok(()),
            Some(_) => return Err("The recording has already started. Stop it instead.".into()),
        }
        let Active { dir, capture, .. } = guard.take().unwrap();
        drop(guard);
        let _ = capture.stop();
        self.stop_heartbeat();
        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    /// Folders left by a countdown the app didn't finish (it quit or crashed): nothing was
    /// recorded in them.
    pub fn remove_stale_countdowns(&self) {
        if self.is_recording() {
            return;
        }
        for session in self.all() {
            if session.state == State::Countdown {
                let _ = fs::remove_dir_all(self.root.join(&session.id));
            }
        }
    }

    fn change(&self, f: impl FnOnce(&mut Active)) -> Result<Session, String> {
        let mut guard = self.active.lock().unwrap();
        let a = guard.as_mut().ok_or("Nothing is being recorded.")?;
        f(a);
        a.session.elapsed_s = a.capture.status().elapsed;
        write_session(&a.dir, &a.session)?;
        Ok(a.session.clone())
    }

    pub fn live(&self) -> Option<Live> {
        let guard = self.active.lock().unwrap();
        let a = guard.as_ref()?;
        let status = a.capture.status();
        let mut session = a.session.clone();
        session.elapsed_s = status.elapsed;
        let free_bytes = disk::free_bytes(&a.dir);
        Some(Live {
            session,
            status,
            free_bytes,
            minutes_left: free_bytes
                .map(|free| minutes_left(free, self.stop_below_free, a.bytes_per_minute)),
        })
    }

    /// Stops, finishes the files and checks them (check.rs). The session is kept even when
    /// something failed: whatever was recorded stays playable.
    pub fn stop(&self) -> Result<Session, String> {
        // Out of the shared state first: the heartbeat sees that and ends by itself.
        let Active {
            mut session,
            dir,
            capture,
            ..
        } = self
            .active
            .lock()
            .unwrap()
            .take()
            .ok_or("Nothing is being recorded.")?;
        session.state = State::Finishing;
        // A full disk can't even take this: the files are finished below regardless (never
        // lose a recording, docs/m5-results.md step 7).
        let _ = write_session(&dir, &session);
        let (status, error) = capture.stop();
        self.stop_heartbeat();
        session.elapsed_s = status.elapsed;
        if session.error.is_none() {
            session.error = error;
        }
        let result = check::check(&dir, Some(&status), 30);
        session.state = if result.playable() {
            State::Done
        } else {
            State::Failed
        };
        session.check = Some(result);
        if write_session(&dir, &session).is_err() {
            // session.json still says it was recording: the next start offers to recover it.
            session.error = Some(DETAILS_NOT_SAVED.into());
        }
        Ok(session)
    }

    /// Every recording folder's session, newest first.
    pub fn all(&self) -> Vec<Session> {
        let mut list: Vec<Session> = fs::read_dir(&self.root)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| read_session(&entry.path()))
            .collect();
        list.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        list
    }

    /// The Recent list: finished recordings not removed from it.
    pub fn recent(&self) -> Vec<Session> {
        let active = self
            .active
            .lock()
            .unwrap()
            .as_ref()
            .map(|a| a.session.id.clone());
        self.all()
            .into_iter()
            .filter(|s| {
                !s.hidden
                    && !s.state.unfinished()
                    && s.state != State::Countdown
                    && Some(&s.id) != active.as_ref()
            })
            .collect()
    }

    pub fn remove_from_recent(&self, id: &str) -> Result<(), String> {
        let dir = self.dir(id)?;
        let mut session = read_session(&dir).ok_or("This recording no longer exists.")?;
        session.hidden = true;
        write_session(&dir, &session)
    }

    /// Deletes a recording's folder (only ever inside the recordings folder).
    pub fn delete(&self, id: &str) -> Result<(), String> {
        let dir = self.dir(id)?;
        if self
            .active
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|a| a.session.id == id)
        {
            return Err("This recording is still running. Stop it first.".into());
        }
        if !dir.join("session.json").is_file() {
            return Err("This recording no longer exists.".into());
        }
        fs::remove_dir_all(&dir).map_err(|e| {
            format!("The recording can't be deleted ({e}). Close any app that has it open, then try again.")
        })
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::process::Command;
    use std::sync::atomic::AtomicU32;
    use std::time::Instant;

    pub fn temp_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("kreokit-rec-sessions-tests")
            .join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Writes real files at stop (ffmpeg test sources) as long as the recording ran, on
    /// every system: the session logic is tested without a screen.
    pub struct FakeBackend {
        pub starts: AtomicU32,
    }

    struct FakeCapture {
        dir: PathBuf,
        start: Instant,
        paused_at: Mutex<Option<Instant>>,
        paused_total: Mutex<Duration>,
        mic: bool,
        kept_out: bool,
    }

    impl FakeCapture {
        fn elapsed(&self) -> f64 {
            let end = self.paused_at.lock().unwrap().unwrap_or_else(Instant::now);
            (end - self.start - *self.paused_total.lock().unwrap()).as_secs_f64()
        }
    }

    impl Capture for FakeCapture {
        fn pause(&self) {
            self.paused_at
                .lock()
                .unwrap()
                .get_or_insert_with(Instant::now);
        }
        fn resume(&self) {
            if let Some(at) = self.paused_at.lock().unwrap().take() {
                *self.paused_total.lock().unwrap() += at.elapsed();
            }
        }
        fn status(&self) -> Status {
            Status {
                elapsed: self.elapsed(),
                frames: (self.elapsed() * 30.0).ceil() as i64,
                ..Default::default()
            }
        }
        fn stop(self: Box<Self>) -> (Status, Option<String>) {
            let status = self.status();
            make_tracks(&self.dir, status.frames as f64 / 30.0, self.mic, false);
            (status, None)
        }
        fn own_windows_kept_out(&self) -> bool {
            self.kept_out
        }
    }

    /// screen.mp4 (+ mic.mp4) of `seconds`, fragmented like the recorder's files.
    pub fn make_tracks(dir: &Path, seconds: f64, mic: bool, fragmented: bool) {
        let ffmpeg = crate::ffmpeg::locate("ffmpeg").unwrap();
        let frag: &[&str] = if fragmented {
            &["-movflags", "frag_keyframe+empty_moov+default_base_moof"]
        } else {
            &[]
        };
        let video = Command::new(&ffmpeg)
            .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
            .arg(format!("testsrc=size=320x180:rate=30:duration={seconds}"))
            .args(["-c:v", "mpeg4", "-g", "30"])
            .args(frag)
            .arg(dir.join("screen.mp4"))
            .status()
            .unwrap();
        assert!(video.success());
        if mic {
            let audio = Command::new(&ffmpeg)
                .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
                .arg(format!("sine=sample_rate=48000:duration={seconds}"))
                .args(["-c:a", "aac", "-ac", "1", "-frag_duration", "1000000"])
                .args(frag)
                .arg(dir.join("mic.mp4"))
                .status()
                .unwrap();
            assert!(audio.success());
        }
    }

    /// A camera track like `make_tracks`' screen, `seconds` long.
    pub fn make_camera(dir: &Path, seconds: f64, fragmented: bool) {
        let ffmpeg = crate::ffmpeg::locate("ffmpeg").unwrap();
        let frag: &[&str] = if fragmented {
            &["-movflags", "frag_keyframe+empty_moov+default_base_moof"]
        } else {
            &[]
        };
        let video = Command::new(&ffmpeg)
            .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
            .arg(format!("testsrc2=size=320x180:rate=30:duration={seconds}"))
            .args(["-c:v", "mpeg4", "-g", "30"])
            .args(frag)
            .arg(dir.join("camera.mp4"))
            .status()
            .unwrap();
        assert!(video.success());
    }

    impl Backend for FakeBackend {
        fn start(&self, config: &RecordConfig) -> Result<Box<dyn Capture>, String> {
            self.starts.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(FakeCapture {
                dir: config.dir.clone(),
                start: Instant::now(),
                paused_at: Mutex::new(None),
                paused_total: Mutex::new(Duration::ZERO),
                mic: config.mic.is_some(),
                kept_out: config
                    .screen
                    .as_ref()
                    .is_some_and(|s| !s.exclude_windows.is_empty()),
            }))
        }
    }

    pub fn options() -> Options {
        Options {
            display_id: 0,
            display_name: "Built-in display".into(),
            display_width: 2880,
            display_height: 1800,
            quality: 1080,
            mic_id: Some(String::new()),
            mic_name: Some("MacBook Pro Microphone".into()),
            camera_id: None,
            camera_name: None,
            camera_width: 0,
            camera_height: 0,
            display_points_width: 1440.0,
            region: None,
            window: None,
            system_audio: false,
            countdown: false,
            own_windows: Vec::new(),
        }
    }

    pub fn fake(root: &Path) -> Recordings {
        Recordings::new(
            root.to_path_buf(),
            Box::new(FakeBackend {
                starts: AtomicU32::new(0),
            }),
        )
    }

    #[test]
    fn sizes_follow_the_display_shape() {
        assert_eq!(output_size(2880, 1800, 1080), (1728, 1080));
        assert_eq!(output_size(2880, 1800, 720), (1152, 720));
        assert_eq!(output_size(1920, 1080, 1080), (1920, 1080));
        // Never larger than the display, never wider than 3840.
        assert_eq!(output_size(1280, 800, 1080), (1280, 800));
        assert_eq!(output_size(5120, 1440, 1080), (3840, 1080));
        assert_eq!(output_size(1366, 768, 720), (1280, 720));
        // About 55 MB a minute at 1728x1080 with the microphone.
        let mb = bytes_per_minute(1728, 1080, None, true, false) / 1_000_000;
        assert!((50..60).contains(&mb), "{mb}");
        // The camera at 720p adds about 27 MB.
        let mb = bytes_per_minute(1728, 1080, Some((1280, 720)), true, false) / 1_000_000;
        assert!((78..88).contains(&mb), "{mb}");
        assert_eq!(camera_size(1280, 720), (1280, 720));
        assert_eq!(camera_size(640, 480), (640, 480));
        assert_eq!(camera_size(1279, 721), (1276, 720));
        assert_eq!(camera_size(0, 0), (1280, 720));
    }

    /// Needs ffmpeg: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn records_pauses_and_stops() {
        let root = temp_root("lifecycle");
        let recordings = fake(&root);
        let mut opts = options();
        opts.own_windows = vec![42];
        let session = recordings.start(&opts).unwrap();
        assert_eq!(session.state, State::Recording);
        assert_eq!(session.screen.as_ref().unwrap().width, 1728);
        // What the recorder says about the app's own windows is kept with the recording.
        assert!(session.screen.as_ref().unwrap().own_windows_kept_out);
        let on_disk = read_session(&root.join(&session.id)).unwrap();
        assert!(on_disk.screen.unwrap().own_windows_kept_out);
        assert!(
            recordings.start(&options()).is_err(),
            "one recording at a time"
        );
        std::thread::sleep(Duration::from_millis(600));
        assert_eq!(recordings.pause().unwrap().state, State::Paused);
        std::thread::sleep(Duration::from_millis(400));
        let on_disk = read_session(&root.join(&session.id)).unwrap();
        assert_eq!((on_disk.state, on_disk.pauses), (State::Paused, 1));
        assert_eq!(recordings.resume().unwrap().state, State::Recording);
        std::thread::sleep(Duration::from_millis(600));
        let live = recordings.live().unwrap();
        assert!(live.free_bytes.is_some());
        let done = recordings.stop().unwrap();
        assert_eq!(done.state, State::Done, "{done:?}");
        // The pause isn't counted.
        assert!((done.elapsed_s - 1.2).abs() < 0.15, "{}", done.elapsed_s);
        assert!(done.check.as_ref().unwrap().problems.is_empty(), "{done:?}");
        assert_eq!(read_session(&root.join(&done.id)).unwrap(), done);
        assert!(recordings.stop().is_err());
        assert_eq!(recordings.recent().len(), 1);

        recordings.remove_from_recent(&done.id).unwrap();
        assert!(recordings.recent().is_empty());
        assert!(
            root.join(&done.id).join("screen.mp4").is_file(),
            "files kept"
        );
        recordings.delete(&done.id).unwrap();
        assert!(!root.join(&done.id).exists());
        assert!(recordings.delete("../outside").is_err());
        let _ = fs::remove_dir_all(&root);
    }

    /// Keeps what it was asked to record, then refuses (no files needed).
    struct Keeps(Arc<Mutex<Option<RecordConfig>>>);
    impl Backend for Keeps {
        fn start(&self, config: &RecordConfig) -> Result<Box<dyn Capture>, String> {
            *self.0.lock().unwrap() = Some(config.clone());
            Err("stop here".into())
        }
    }

    /// Recordings whose starts are kept: the second value holds the last one's config.
    fn keeping(root: &Path) -> (Recordings, Arc<Mutex<Option<RecordConfig>>>) {
        let kept = Arc::new(Mutex::new(None));
        let recordings = Recordings::new(root.to_path_buf(), Box::new(Keeps(kept.clone())));
        (recordings, kept)
    }

    /// Needs ffmpeg: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn a_countdown_holds_the_recording_until_it_begins() {
        let root = temp_root("countdown");
        let recordings = fake(&root);
        let mut opts = options();
        opts.countdown = true;
        let session = recordings.start(&opts).unwrap();
        assert_eq!(session.state, State::Countdown);
        // Not offered for recovery and not in Recent while the countdown runs.
        assert!(recordings.recent().is_empty());
        assert!(recordings.pause().unwrap().state == State::Countdown);
        assert_eq!(recordings.begin().unwrap().state, State::Recording);
        assert!(recordings.cancel().is_err(), "too late to cancel");
        assert_eq!(recordings.stop().unwrap().pauses, 0);

        // Cancelled: nothing kept.
        let session = recordings.start(&opts).unwrap();
        recordings.cancel().unwrap();
        assert!(!root.join(&session.id).exists());
        assert!(recordings.live().is_none());

        // Left behind by a crash during the countdown: removed at the next start.
        let stale = root.join("2026-01-01-000000");
        fs::create_dir_all(&stale).unwrap();
        let mut left = session.clone();
        left.id = "2026-01-01-000000".into();
        write_session(&stale, &left).unwrap();
        assert!(crate::recovery::pending(&recordings).is_empty());
        assert!(!stale.exists());
        let _ = fs::remove_dir_all(&root);
    }

    /// Measurement (docs/m5-results.md, step 7) on the volume in `KREOKIT_MEASURE_DISK` (a
    /// small disk image, never a real disk), synthetic sources as fast as the encoder allows:
    /// A, the disk-space stop set 60 MB below the free space at the start (what the stop and
    /// finishing do, what they need); B, the volume filled up to 40 MB free and the stop off
    /// (what a really full disk does to the files). Only with that set:
    /// `KREOKIT_MEASURE_DISK=DIR cargo test --lib measure_disk_full -- --ignored --nocapture`
    #[test]
    #[ignore]
    #[cfg(target_os = "macos")]
    fn measure_disk_full() {
        let Some(volume) = std::env::var_os("KREOKIT_MEASURE_DISK") else {
            eprintln!("set KREOKIT_MEASURE_DISK to a scratch volume to measure");
            return;
        };
        let volume = PathBuf::from(volume);
        for scenario in ["A", "B"] {
            let root = volume.join("recordings");
            let filler = volume.join("filler");
            let _ = fs::remove_dir_all(&root);
            let _ = fs::remove_file(&filler);
            fs::create_dir_all(&root).unwrap();
            let mut recordings = Recordings::new(root.clone(), Box::new(NativeBackend));
            recordings.synthetic = Some(crate::native::Synthetic {
                fast: true,
                ..Default::default()
            });
            recordings.start_below_free = 0;
            if scenario == "A" {
                recordings.stop_below_free = disk::free_bytes(&root).unwrap() - 10_000_000;
            } else {
                recordings.stop_below_free = 0;
                let leave = 40_000_000;
                let size = disk::free_bytes(&root).unwrap() - leave;
                let f = fs::File::create(&filler).unwrap();
                f.set_len(size).unwrap(); // sparse on APFS: written out below
                drop(f);
                let status = Command::new("dd")
                    .args(["if=/dev/zero", "bs=1m", "conv=notrunc"])
                    .arg(format!("of={}", filler.display()))
                    .arg(format!("count={}", size / 1_048_576))
                    .status()
                    .unwrap();
                assert!(status.success());
            }
            let mut opts = options();
            opts.camera_id = Some("synthetic".into());
            (opts.camera_width, opts.camera_height) = (1280, 720);
            let free_start = disk::free_bytes(&root).unwrap();
            let session = recordings.start(&opts).unwrap();
            let started = Instant::now();
            let (mut lowest, mut trigger, mut failed_at) = (u64::MAX, None, None);
            while started.elapsed() < Duration::from_secs(300) {
                let live = recordings.live().unwrap();
                let free = live.free_bytes.unwrap_or(0);
                lowest = lowest.min(free);
                if live.status.failed && failed_at.is_none() {
                    failed_at = Some((free, live.status.elapsed));
                }
                if live.session.error.is_some() || (scenario == "B" && live.status.failed) {
                    trigger = Some((free, live.status.elapsed, started.elapsed().as_secs_f64()));
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            let free_at_stop_call = disk::free_bytes(&root).unwrap();
            let stopping = Instant::now();
            let done = recordings.stop();
            let stop_s = stopping.elapsed().as_secs_f64();
            let free_after = disk::free_bytes(&root).unwrap();
            let dir = root.join(&session.id);
            // The files as they are, read back like the app's check does.
            let files_check = crate::check::check(&dir, None, 30);
            let sizes: Vec<(String, u64)> = fs::read_dir(&dir)
                .unwrap()
                .flatten()
                .map(|e| {
                    (
                        e.file_name().to_string_lossy().into(),
                        e.metadata().unwrap().len(),
                    )
                })
                .collect();
            println!(
                "{}",
                serde_json::json!({
                    "scenario": scenario,
                    "free_start": free_start,
                    "trigger": trigger,
                    "writer_failed_at": failed_at,
                    "lowest_free_seen": lowest,
                    "free_at_stop_call": free_at_stop_call,
                    "stop_seconds": stop_s,
                    "free_after_stop": free_after,
                    "finishing_used_bytes": free_at_stop_call as i64 - free_after as i64,
                    "stopped": done.as_ref().map(|d| (d.state, d.elapsed_s, d.error.clone(), d.check.clone())).map_err(|e| e.clone()),
                    "files": sizes,
                    "files_check": files_check,
                })
            );
            let _ = fs::remove_dir_all(&root);
            let _ = fs::remove_file(&filler);
        }
    }

    #[test]
    fn counts_the_minutes_until_the_disk_space_stop() {
        let per_minute = bytes_per_minute(1728, 1080, None, true, false);
        let left = minutes_left(
            STOP_BELOW_FREE + 10 * per_minute,
            STOP_BELOW_FREE,
            per_minute,
        );
        assert!((left - 10.0).abs() < 1e-9, "{left}");
        assert_eq!(minutes_left(100, STOP_BELOW_FREE, per_minute), 0.0);
    }

    /// Needs ffmpeg: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn stops_by_itself_when_the_disk_is_almost_full() {
        let root = temp_root("disk-full");
        let mut recordings = fake(&root);
        let called = Arc::new(AtomicU32::new(0));
        let c = called.clone();
        recordings.on_disk_full = Some(Arc::new(move || {
            c.fetch_add(1, Ordering::SeqCst);
        }));
        recordings.stop_below_free = u64::MAX; // "almost full" at once
        recordings.start(&options()).unwrap();
        let start = Instant::now();
        while called.load(Ordering::SeqCst) == 0 {
            assert!(start.elapsed() < Duration::from_secs(5), "never called");
            std::thread::sleep(Duration::from_millis(20));
        }
        let live = recordings.live().unwrap();
        assert_eq!(live.session.state, State::Paused);
        assert_eq!(live.minutes_left, Some(0.0));
        // What the app does when called: stop and finish; the recording keeps its note.
        let done = recordings.stop().unwrap();
        assert_eq!(done.state, State::Done);
        assert_eq!(done.error.as_deref(), Some(DISK_FULL_STOP));
        assert_eq!(called.load(Ordering::SeqCst), 1, "once");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn hands_the_own_windows_to_the_recorder() {
        let root = temp_root("own-windows");
        let (recordings, kept) = keeping(&root);
        let mut opts = options();
        opts.own_windows = vec![7, 9];
        assert!(recordings.start(&opts).is_err());
        let config = kept.lock().unwrap().take().unwrap();
        assert_eq!(config.screen.unwrap().exclude_windows, vec![7, 9]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn records_the_chosen_camera_at_its_size() {
        let root = temp_root("camera");
        let (recordings, kept) = keeping(&root);
        let mut opts = options();
        opts.camera_id = Some("cam-1".into());
        opts.camera_name = Some("FaceTime HD Camera".into());
        (opts.camera_width, opts.camera_height) = (1280, 720);
        assert!(recordings.start(&opts).is_err());
        let config = kept.lock().unwrap().take().unwrap();
        let camera = config.camera.unwrap();
        assert_eq!(
            (camera.id.as_str(), camera.width, camera.height),
            ("cam-1", 1280, 720)
        );
        assert_eq!(camera.bitrate, video_bitrate(1280, 720));
        // Without a camera: none recorded.
        assert!(recordings.start(&options()).is_err());
        assert_eq!(kept.lock().unwrap().take().unwrap().camera, None);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_region_is_recorded_in_its_own_shape() {
        // 2880 × 1800 pixels, 1440 points wide: 2 pixels per point.
        let region = Region {
            x: 100.0,
            y: 50.0,
            width: 800.0,
            height: 450.0,
        };
        assert_eq!(source_size(2880, 1800, 1440.0, Some(region)), (1600, 900));
        assert_eq!(source_size(2880, 1800, 1440.0, None), (2880, 1800));
        // Not known (an old caller): the whole display.
        assert_eq!(source_size(2880, 1800, 0.0, Some(region)), (2880, 1800));

        let root = temp_root("region");
        let (recordings, kept) = keeping(&root);
        let mut opts = options();
        opts.region = Some(region);
        assert!(recordings.start(&opts).is_err());
        let screen = kept.lock().unwrap().take().unwrap().screen.unwrap();
        assert_eq!(screen.region, Some(region));
        // Smaller than 1080p: recorded at its own size.
        assert_eq!((screen.width, screen.height), (1600, 900));
        opts.quality = 720;
        assert!(recordings.start(&opts).is_err());
        let screen = kept.lock().unwrap().take().unwrap().screen.unwrap();
        assert_eq!((screen.width, screen.height), (1280, 720));

        // Too small: refused before anything starts.
        opts.region = Some(Region {
            width: 40.0,
            ..region
        });
        let error = recordings.start(&opts).unwrap_err();
        assert!(error.starts_with("The region is too small"), "{error}");
        assert!(kept.lock().unwrap().is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_failed_start_leaves_nothing_behind() {
        struct Refuses;
        impl Backend for Refuses {
            fn start(&self, _: &RecordConfig) -> Result<Box<dyn Capture>, String> {
                Err("The screen can't be recorded.".into())
            }
        }
        let root = temp_root("refused");
        let recordings = Recordings::new(root.clone(), Box::new(Refuses));
        assert_eq!(
            recordings.start(&options()).unwrap_err(),
            "The screen can't be recorded."
        );
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
        assert!(!recordings.is_recording());
        let _ = fs::remove_dir_all(&root);
    }
}

//! Recording sessions: one folder per recording in the app data folder (`screen.mp4`,
//! `mic.mp4`, `session.json`), the states idle → recording ⇄ paused → finishing → done, and
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
use crate::native::{RecordConfig, ScreenApi, ScreenConfig, Status};
use crate::{awake, disk};

/// Recording stops by itself when less than this is free (the files stay playable).
pub const STOP_BELOW_FREE: u64 = 500_000_000;
/// A recording doesn't start with less than this free.
pub const START_BELOW_FREE: u64 = 2_000_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
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
    pub width: u32,
    pub height: u32,
    pub fps: u32,
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

/// H.264 bits per second for a size at 30 fps (≈ 7.3 Mbit/s at 1728x1080).
pub fn video_bitrate(width: u32, height: u32) -> u32 {
    (width as f64 * height as f64 * 30.0 * 0.13) as u32
}

/// The expected file size per minute: picture + 128 kbit/s sound.
pub fn bytes_per_minute(width: u32, height: u32, mic: bool) -> u64 {
    (video_bitrate(width, height) as u64 + if mic { 128_000 } else { 0 }) / 8 * 60
}

/// A running recording as the backend sees it (native.rs, or a fake in tests).
pub trait Capture: Send + Sync {
    fn pause(&self);
    fn resume(&self);
    fn status(&self) -> Status;
    fn stop(self: Box<Self>) -> (Status, Option<String>);
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
}

struct Active {
    session: Session,
    dir: PathBuf,
    capture: Box<dyn Capture>,
}

pub struct Recordings {
    root: PathBuf,
    backend: Box<dyn Backend>,
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
    #[cfg(not(unix))]
    {
        format!("recording-{ms}")
    }
}

pub fn write_session(dir: &Path, session: &Session) -> Result<(), String> {
    let json = serde_json::to_string_pretty(session).map_err(|e| e.to_string())?;
    let temp = dir.join("session.json.tmp");
    fs::write(&temp, json).map_err(|e| format!("cannot save the recording's details: {e}"))?;
    fs::rename(&temp, dir.join("session.json"))
        .map_err(|e| format!("cannot save the recording's details: {e}"))
}

pub fn read_session(dir: &Path) -> Option<Session> {
    serde_json::from_str(&fs::read_to_string(dir.join("session.json")).ok()?).ok()
}

impl Recordings {
    pub fn new(root: PathBuf, backend: Box<dyn Backend>) -> Recordings {
        Recordings {
            root,
            backend,
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
        self.start_with(options, None)
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
        if disk::free_bytes(&self.root).is_some_and(|free| free < START_BELOW_FREE) {
            return Err(format!(
                "Less than {} of disk space is free. Free up some space, then record again.",
                disk::gigabytes(START_BELOW_FREE)
            ));
        }
        let created_at = now_ms();
        let mut id = folder_name(created_at);
        let mut n = 2;
        while self.root.join(&id).exists() {
            id = format!("{}-{n}", folder_name(created_at));
            n += 1;
        }
        let dir = self.root.join(&id);
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let (width, height) = output_size(
            options.display_width,
            options.display_height,
            options.quality,
        );
        let session = Session {
            id,
            created_at,
            state: State::Recording,
            screen: Some(ScreenInfo {
                display: options.display_name.clone(),
                width,
                height,
                fps: 30,
            }),
            mic: options.mic_id.as_ref().map(|_| {
                options
                    .mic_name
                    .clone()
                    .unwrap_or_else(|| "Microphone".into())
            }),
            elapsed_s: 0.0,
            pauses: 0,
            check: None,
            error: None,
            hidden: false,
        };
        write_session(&dir, &session)?;
        let config = RecordConfig {
            dir: dir.clone(),
            screen: Some(ScreenConfig {
                display_id: options.display_id,
                width,
                height,
                fps: 30,
                capture_fps: 60,
                api: ScreenApi::Auto,
                bitrate: video_bitrate(width, height),
            }),
            mic: options.mic_id.clone(),
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
        *active = Some(Active {
            session: session.clone(),
            dir,
            capture,
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
        let handle = std::thread::spawn(move || {
            let _awake = awake::KeepAwake::new("recording");
            while !flag.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(250));
                let mut guard = active.lock().unwrap();
                let Some(a) = guard.as_mut() else { break };
                let status = a.capture.status();
                a.session.elapsed_s = status.elapsed;
                let _ = write_session(&a.dir, &a.session);
                if a.session.error.is_none()
                    && disk::free_bytes(&a.dir).is_some_and(|free| free < STOP_BELOW_FREE)
                {
                    a.session.error = Some(
                        "The recording stopped because the disk is almost full. Everything up to that moment is kept.".into(),
                    );
                    // Paused for good: the files stay as they are until Stop finishes them.
                    a.capture.pause();
                    a.session.state = State::Paused;
                    let _ = write_session(&a.dir, &a.session);
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
        Some(Live {
            session,
            status,
            free_bytes: disk::free_bytes(&a.dir),
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
        } = self
            .active
            .lock()
            .unwrap()
            .take()
            .ok_or("Nothing is being recorded.")?;
        session.state = State::Finishing;
        write_session(&dir, &session)?;
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
        write_session(&dir, &session)?;
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
            .filter(|s| !s.hidden && !s.state.unfinished() && Some(&s.id) != active.as_ref())
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

    impl Backend for FakeBackend {
        fn start(&self, config: &RecordConfig) -> Result<Box<dyn Capture>, String> {
            self.starts.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(FakeCapture {
                dir: config.dir.clone(),
                start: Instant::now(),
                paused_at: Mutex::new(None),
                paused_total: Mutex::new(Duration::ZERO),
                mic: config.mic.is_some(),
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
        let mb = bytes_per_minute(1728, 1080, true) / 1_000_000;
        assert!((50..60).contains(&mb), "{mb}");
    }

    /// Needs ffmpeg: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn records_pauses_and_stops() {
        let root = temp_root("lifecycle");
        let recordings = fake(&root);
        let session = recordings.start(&options()).unwrap();
        assert_eq!(session.state, State::Recording);
        assert_eq!(session.screen.as_ref().unwrap().width, 1728);
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

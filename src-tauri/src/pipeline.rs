//! The recorder on Windows and Linux: the same rules as the macOS recorder
//! (native/macos/recorder.m is the reference, recorder.h the contract), in Rust, writing
//! through FFmpeg (encoder.rs). Sources (synthetic.rs, the system capture in native.rs)
//! hand in pictures and sound stamped on the host clock ([`host_seconds`]).
//!
//! Every picture track keeps one picture per output slot (1/fps): the newest one captured
//! before the slot's middle, or the previous picture again when nothing new arrived. Sound
//! is placed on the output timeline by its timestamp: a gap becomes silence, an overlap is
//! dropped. Pauses cut every track at the same moments.
//!
//! Threads: one lock over the state (the macOS recorder's serial queue); a pacer thread
//! writes the slots in real time; each track's FFmpeg has its own writer thread.

#![cfg_attr(
    all(target_os = "macos", not(feature = "pipe-recorder")),
    allow(dead_code)
)]

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::encoder::{self, Kind, Picture, RATE, Rect, VideoInput, Writer};
use crate::native::{RecordConfig, Status};

/// How long a slot waits for its picture to arrive (capture latency) before it is written.
pub const ALLOWANCE: f64 = 0.08;
/// A picture exactly on a slot boundary belongs to the next slot.
const TIE: f64 = 1e-6;
/// A small gap or overlap in a sound source's timestamps (jitter) is ignored up to this.
const AUDIO_TOLERANCE: i64 = 96; // 2 ms

/// Seconds on the system's monotonic clock, which the capture APIs stamp their media with:
/// CLOCK_MONOTONIC on Linux (PipeWire, V4L2), the performance counter on Windows
/// (Windows.Graphics.Capture, Media Foundation, WASAPI).
pub fn host_seconds() -> f64 {
    #[cfg(unix)]
    {
        let mut ts = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: a valid pointer.
        unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
        ts.tv_sec as f64 + ts.tv_nsec as f64 * 1e-9
    }
    #[cfg(windows)]
    {
        unsafe extern "system" {
            fn QueryPerformanceCounter(count: *mut i64) -> i32;
            fn QueryPerformanceFrequency(frequency: *mut i64) -> i32;
        }
        let (mut count, mut frequency) = (0i64, 1i64);
        // SAFETY: valid pointers.
        unsafe {
            QueryPerformanceCounter(&mut count);
            QueryPerformanceFrequency(&mut frequency);
        }
        count as f64 / frequency as f64
    }
}

/// A captured picture waiting for its slot.
struct Frame {
    picture: Arc<Picture>,
    t: f64,
    /// A region: `picture` is already cut out.
    cropped: bool,
}

/// One picture track (screen or camera).
struct Pacer {
    writer: Arc<Writer>,
    camera: bool,
    input: VideoInput,
    /// A region: cut out of each picture when it is first written.
    crop: Option<Rect>,
    slot: i64,
    frames: VecDeque<Frame>,
    current: Option<Frame>,
    last_boundary: f64,
    written: i64,
    fresh: i64,
    repeated: i64,
    late: i64,
    lost: i64,
}

impl Pacer {
    fn new(writer: Arc<Writer>, camera: bool, input: VideoInput, crop: Option<Rect>) -> Pacer {
        Pacer {
            writer,
            camera,
            input,
            crop,
            slot: 0,
            frames: VecDeque::new(),
            current: None,
            last_boundary: f64::NEG_INFINITY,
            written: 0,
            fresh: 0,
            repeated: 0,
            late: 0,
            lost: 0,
        }
    }

    /// Makes a picture what the writer reads: the region cut out, or a picture of another
    /// size (a resized window) placed top-left on black.
    fn prepare(&self, f: &mut Frame) {
        if f.cropped {
            return;
        }
        f.cropped = true;
        let (w, h) = (self.input.width, self.input.height);
        let rect = match self.crop {
            Some(r) => r,
            None if (f.picture.width, f.picture.height) != (w, h) => Rect {
                x: 0,
                y: 0,
                width: w,
                height: h,
            },
            None => return,
        };
        f.picture = Arc::new(f.picture.crop(rect));
    }
}

/// One sound track: 48 kHz float samples, interleaved when stereo.
struct AudioTrack {
    writer: Arc<Writer>,
    channels: usize,
    /// The next output sample frame.
    next: i64,
    samples: i64,
    filled: i64,
    dropped: i64,
}

struct State {
    t0: f64,
    fps: u32,
    paused_total: f64,
    pause_start: Option<f64>,
    pauses: Vec<(f64, f64)>,
    stop_at: f64,
    screen: Option<Pacer>,
    camera: Option<Pacer>,
    mic: Option<AudioTrack>,
    system: Option<AudioTrack>,
    peak_now: f32,
    peak_prev: f32,
    peak_count: i64,
    window_closed: bool,
    window_closed_at: f64,
    failed: bool,
    error: Option<String>,
}

/// The pictures each picture track reads from its source, and a region to cut out of the
/// screen's (then the screen writer reads the region's size).
#[derive(Debug, Clone, Copy, Default)]
pub struct Inputs {
    pub screen: Option<VideoInput>,
    pub crop: Option<Rect>,
    pub camera: Option<VideoInput>,
}

/// The shared state of one recording; sources hold it and hand their media in.
pub struct Core {
    state: Mutex<State>,
    /// The synthetic sources' own clock (synthetic `fast`), as f64 bits.
    fast: bool,
    synth_now: AtomicU64,
    writers: Vec<Arc<Writer>>,
}

impl Core {
    /// Creates the files and starts their FFmpeg. The recording's zero is 0.5 s from now.
    pub fn new(config: &RecordConfig, inputs: Inputs) -> Result<Arc<Core>, String> {
        let mut writers = Vec::new();
        let made = Core::make(config, inputs, &mut writers);
        if made.is_err() {
            // The files already started are finished (their FFmpeg waits for its input).
            for w in &writers {
                w.finish();
            }
        }
        made
    }

    fn make(
        config: &RecordConfig,
        inputs: Inputs,
        writers: &mut Vec<Arc<Writer>>,
    ) -> Result<Arc<Core>, String> {
        let fps = config.screen.as_ref().map_or(30, |s| s.fps);
        let space = "Check that the disk has space.";
        let video = |name: &str, what, input: VideoInput, w, h, bitrate| {
            let encoder = encoder::video_encoder()?;
            let kind = Kind::Video {
                input,
                width: w,
                height: h,
                fps,
                bitrate,
                encoder,
            };
            Writer::start(config.dir.join(name), kind, what).map_err(|e| format!("{e} {space}"))
        };
        let screen = match (&config.screen, inputs.screen) {
            (Some(s), Some(input)) => {
                let read = match inputs.crop {
                    Some(r) => VideoInput {
                        width: r.width,
                        height: r.height,
                        ..input
                    },
                    None => input,
                };
                let w = video("screen.mp4", "screen", read, s.width, s.height, s.bitrate)?;
                writers.push(w.clone());
                Some(Pacer::new(w, false, read, inputs.crop))
            }
            _ => None,
        };
        let camera = match (&config.camera, inputs.camera) {
            (Some(c), Some(input)) => {
                let w = video("camera.mp4", "camera", input, c.width, c.height, c.bitrate)?;
                writers.push(w.clone());
                Some(Pacer::new(w, true, input, None))
            }
            _ => None,
        };
        let audio = |name: &str, what, channels| {
            Writer::start(config.dir.join(name), Kind::Audio { channels }, what)
                .map_err(|e| format!("{e} {space}"))
        };
        let mut mic = None;
        if config.mic.is_some() {
            let w = audio("mic.mp4", "microphone", 1)?;
            writers.push(w.clone());
            mic = Some(AudioTrack::new(w, 1));
        }
        let mut system = None;
        if config.system_audio {
            let w = audio("system.mp4", "computer sound", 2)?;
            writers.push(w.clone());
            system = Some(AudioTrack::new(w, 2));
        }
        let now = host_seconds();
        let fast = config.synthetic.is_some_and(|s| s.fast);
        Ok(Arc::new(Core {
            state: Mutex::new(State {
                t0: now + 0.5,
                fps,
                paused_total: 0.0,
                pause_start: None,
                pauses: Vec::new(),
                stop_at: f64::INFINITY,
                screen,
                camera,
                mic,
                system,
                peak_now: 0.0,
                peak_prev: 0.0,
                peak_count: 0,
                window_closed: false,
                window_closed_at: 0.0,
                failed: false,
                error: None,
            }),
            fast,
            synth_now: AtomicU64::new(now.to_bits()),
            writers: writers.clone(),
        }))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The recording's clock: host time, or the synthetic sources' own.
    pub fn now(&self) -> f64 {
        if self.fast {
            f64::from_bits(self.synth_now.load(Ordering::Relaxed))
        } else {
            host_seconds()
        }
    }

    pub fn set_synthetic_now(&self, t: f64) {
        self.synth_now.store(t.to_bits(), Ordering::Relaxed);
    }

    pub fn fast(&self) -> bool {
        self.fast
    }

    pub fn t0(&self) -> f64 {
        self.lock().t0
    }

    pub fn stop_at(&self) -> f64 {
        self.lock().stop_at
    }

    /// Some writer has half a second or more waiting (the synthetic sources slow down).
    pub fn congested(&self) -> bool {
        self.writers.iter().any(|w| w.queued() > w.limit / 2)
    }

    pub fn screen(&self, picture: Arc<Picture>, t: f64) {
        let mut s = self.lock();
        let st = &mut *s;
        if let Some(p) = st.screen.as_mut() {
            let nb = next_boundary(st.t0, st.fps, st.paused_total, st.pause_start, p);
            ingest(p, picture, t, nb);
        }
    }

    pub fn camera(&self, picture: Arc<Picture>, t: f64) {
        let mut s = self.lock();
        let st = &mut *s;
        if let Some(p) = st.camera.as_mut() {
            let nb = next_boundary(st.t0, st.fps, st.paused_total, st.pause_start, p);
            ingest(p, picture, t, nb);
        }
    }

    /// Pictures the camera dropped before they reached us.
    pub fn camera_lost(&self, n: i64) {
        if let Some(p) = self.lock().camera.as_mut() {
            p.lost += n;
        }
    }

    /// Mono microphone samples starting at host time `t`.
    pub fn mic(&self, samples: &[f32], t: f64) {
        let mut st = self.lock();
        level(&mut st, samples);
        if let Some(mut a) = st.mic.take() {
            audio(&st, &mut a, samples, t);
            st.mic = Some(a);
        }
    }

    /// Interleaved stereo computer sound starting at host time `t`.
    pub fn system(&self, samples: &[f32], t: f64) {
        let mut st = self.lock();
        if let Some(mut a) = st.system.take() {
            audio(&st, &mut a, samples, t);
            st.system = Some(a);
        }
    }

    /// The recorded window closed: the recording goes on with its last picture.
    pub fn window_closed(&self) {
        let now = self.now();
        let mut st = self.lock();
        if !st.window_closed {
            st.window_closed = true;
            let end = st.pause_start.unwrap_or(now.min(st.stop_at));
            st.window_closed_at = (end - st.t0 - st.paused_total).max(0.0);
        }
    }

    pub fn has_window_closed(&self) -> bool {
        self.lock().window_closed
    }

    /// A source failed: the recording stops writing, and the stop returns `message`.
    pub fn fail(&self, message: &str) {
        fail(&mut self.lock(), message);
    }

    /// Writes every slot whose boundary is before `limit` (and before a pause or the stop).
    pub fn emit_up_to(&self, limit: f64) {
        let now = self.now();
        let mut s = self.lock();
        let st = &mut *s;
        for camera in [false, true] {
            let mut p = if camera {
                st.camera.take()
            } else {
                st.screen.take()
            };
            if let Some(p) = p.as_mut() {
                emit(st, p, limit, now);
            }
            if camera {
                st.camera = p;
            } else {
                st.screen = p;
            }
        }
    }

    pub fn pause(&self) {
        let now = self.now();
        let mut st = self.lock();
        if st.pause_start.is_none() && now > st.t0 {
            st.pause_start = Some(now);
        }
    }

    pub fn resume(&self) {
        let now = self.now();
        let mut st = self.lock();
        if let Some(p) = st.pause_start.take() {
            st.pauses.push((p, now));
            st.paused_total += now - p;
        }
    }

    pub fn status(&self) -> Status {
        let now = self.now();
        status(&self.lock(), now, &self.writers)
    }

    /// Nothing after now is recorded (or after the pause, when paused).
    pub fn begin_stop(&self) {
        let now = self.now();
        let mut st = self.lock();
        st.stop_at = st.t0.max(st.pause_start.unwrap_or(now));
    }

    /// After the sources stopped: writes the last slots, pads every sound track to the
    /// picture's end and finishes the files.
    pub fn finish(&self) -> (Status, Option<String>) {
        self.emit_up_to(f64::INFINITY);
        {
            let mut s = self.lock();
            let st = &mut *s;
            let picture = st.screen.as_ref().or(st.camera.as_ref());
            if let Some(p) = picture {
                let end = (p.written as f64 * RATE as f64 / st.fps as f64).round() as i64;
                for a in [st.mic.as_mut(), st.system.as_mut()].into_iter().flatten() {
                    pad(a, end);
                }
            }
        }
        let finishing: Vec<_> = self
            .writers
            .iter()
            .map(|w| {
                let w = w.clone();
                std::thread::spawn(move || w.finish())
            })
            .collect();
        for f in finishing {
            let _ = f.join();
        }
        let now = self.now();
        let st = self.lock();
        let status = status(&st, now, &self.writers);
        let error = st
            .error
            .clone()
            .or_else(|| self.writers.iter().find_map(|w| w.failure()));
        (status, error)
    }

    pub fn sync(&self) {
        for w in &self.writers {
            w.sync();
        }
    }
}

impl AudioTrack {
    fn new(writer: Arc<Writer>, channels: usize) -> AudioTrack {
        AudioTrack {
            writer,
            channels,
            next: 0,
            samples: 0,
            filled: 0,
            dropped: 0,
        }
    }
}

fn fail(st: &mut State, message: &str) {
    if st.error.is_none() {
        st.error = Some(message.to_string());
    }
    st.failed = true;
}

/// The host time of a track's next output slot.
fn slot_time(t0: f64, fps: u32, paused_total: f64, p: &Pacer) -> f64 {
    t0 + p.slot as f64 / fps as f64 + paused_total
}

/// The next slot's boundary: it takes the newest picture captured before slot time + half
/// a frame.
fn next_boundary(t0: f64, fps: u32, paused_total: f64, pause: Option<f64>, p: &Pacer) -> f64 {
    let hk = slot_time(t0, fps, paused_total, p);
    if pause.is_some_and(|ps| hk >= ps) {
        return f64::INFINITY;
    }
    hk + 0.5 / fps as f64
}

fn ingest(p: &mut Pacer, picture: Arc<Picture>, t: f64, next_boundary: f64) {
    if t < p.last_boundary - TIE {
        p.late += 1;
        if p.current.as_ref().is_some_and(|c| t <= c.t) {
            return;
        }
    }
    let i = p.frames.iter().rposition(|f| f.t <= t).map_or(0, |i| i + 1);
    p.frames.insert(
        i,
        Frame {
            picture,
            t,
            cropped: false,
        },
    );
    // Only the newest picture before the next slot's boundary can still be used.
    while p.frames.len() >= 2 && p.frames[1].t < next_boundary - TIE {
        p.frames.pop_front();
    }
}

fn emit(st: &mut State, p: &mut Pacer, limit: f64, now: f64) {
    while !st.failed {
        let hk = slot_time(st.t0, st.fps, st.paused_total, p);
        if st.pause_start.is_some_and(|ps| hk >= ps) || hk >= st.stop_at {
            break;
        }
        let boundary = hk + 0.5 / st.fps as f64;
        if boundary > limit {
            break;
        }
        let mut pick = None;
        while p.frames.front().is_some_and(|f| f.t < boundary - TIE) {
            pick = p.frames.pop_front();
        }
        if let Some(mut f) = pick {
            p.prepare(&mut f);
            p.current = Some(f);
            p.fresh += 1;
        } else if p.current.is_some() {
            p.repeated += 1;
        } else if let Some(first) = p.frames.front() {
            // Slots before a source's first picture show that picture. It stays queued, so
            // its own slot still gets it as new.
            let mut f = Frame {
                picture: first.picture.clone(),
                t: first.t,
                cropped: false,
            };
            p.prepare(&mut f);
            p.current = Some(f);
            p.repeated += 1;
        } else {
            // Measured from now, not from the slot (which stays at the first one).
            if now - st.t0 - st.paused_total > 3.0 {
                fail(
                    st,
                    if p.camera {
                        "No picture arrived from the camera. Check that KreoKit Rec may use the camera and that no other app is using it, then try again."
                    } else {
                        "No picture arrived from the screen. Check that KreoKit Rec may record the screen, then try again."
                    },
                );
            }
            break; // waits for the first picture
        }
        let picture = p.current.as_ref().expect("set above").picture.clone();
        p.writer.picture(picture); // a full queue repeats the previous one (counted)
        p.written += 1;
        p.slot += 1;
        p.last_boundary = boundary;
    }
}

fn level(st: &mut State, s: &[f32]) {
    for v in s {
        st.peak_now = st.peak_now.max(v.abs());
        st.peak_count += 1;
        if st.peak_count >= RATE / 10 {
            st.peak_prev = st.peak_now;
            st.peak_now = 0.0;
            st.peak_count = 0;
        }
    }
}

fn write_audio(a: &mut AudioTrack, s: Option<&[f32]>, n: i64) {
    if n <= 0 {
        return;
    }
    // A full queue writes silence in their place (counted), so what follows stays in sync.
    if a.writer.samples(s, n) {
        a.samples += n;
    }
    a.next += n;
}

/// Silence up to output sample `end`, not counted as a gap.
fn pad(a: &mut AudioTrack, end: i64) {
    while a.next < end {
        write_audio(a, None, (end - a.next).min(RATE / 10));
    }
}

/// Time removed by the pauses that ended before host time h.
fn paused_before(st: &State, h: f64) -> f64 {
    st.pauses
        .iter()
        .filter(|(_, r)| *r <= h)
        .map(|(p, r)| r - p)
        .sum()
}

fn keeps_sample_at(st: &State, h: f64) -> bool {
    if h < st.t0 || h >= st.stop_at {
        return false;
    }
    if st.pause_start.is_some_and(|p| h >= p) {
        return false;
    }
    !st.pauses.iter().any(|(p, r)| h >= *p && h < *r)
}

/// Samples starting at host time h: cut at the start, the pauses and the stop, and placed
/// on the output timeline right after the previous ones. A real gap (the source sent
/// nothing) becomes silence, so the sound never slides against the picture.
fn audio(st: &State, a: &mut AudioTrack, s: &[f32], h: f64) {
    let n = (s.len() / a.channels) as i64;
    let mut i = 0;
    while i < n {
        while i < n && !keeps_sample_at(st, h + i as f64 / RATE as f64) {
            i += 1;
        }
        let start = i;
        while i < n && keeps_sample_at(st, h + i as f64 / RATE as f64) {
            i += 1;
        }
        if i == start {
            break;
        }
        let hs = h + start as f64 / RATE as f64;
        let pos = ((hs - st.t0 - paused_before(st, hs)) * RATE as f64).round() as i64;
        let mut skip = 0;
        if pos > a.next + AUDIO_TOLERANCE {
            // Before the first sound it's the source starting up, not a gap to report.
            if a.next > 0 {
                a.filled += pos - a.next;
            }
            pad(a, pos);
        } else if pos < a.next - AUDIO_TOLERANCE {
            skip = (a.next - pos).min(i - start);
            a.dropped += skip;
        }
        let from = ((start + skip) as usize) * a.channels;
        let to = (i as usize) * a.channels;
        write_audio(a, Some(&s[from..to]), i - start - skip);
    }
}

fn status(st: &State, now: f64, writers: &[Arc<Writer>]) -> Status {
    let end = st.pause_start.unwrap_or(now.min(st.stop_at));
    let mut s = Status {
        elapsed: (end - st.t0 - st.paused_total).max(0.0),
        paused: st.pause_start.is_some(),
        mic_peak: st.peak_now.max(st.peak_prev),
        window_closed: st.window_closed,
        window_closed_at: st.window_closed_at,
        failed: st.failed || writers.iter().any(|w| w.failure().is_some()),
        ..Status::default()
    };
    if let Some(p) = &st.screen {
        s.frames = p.written;
        s.frames_new = p.fresh;
        s.frames_repeated = p.repeated;
        s.frames_late = p.late;
        s.frames_dropped = p.writer.dropped();
    }
    if let Some(p) = &st.camera {
        s.camera_frames = p.written;
        s.camera_frames_new = p.fresh;
        s.camera_frames_repeated = p.repeated;
        s.camera_frames_late = p.late;
        s.camera_frames_dropped = p.writer.dropped();
        s.camera_frames_lost = p.lost;
    }
    if let Some(a) = &st.mic {
        s.audio_samples = a.samples;
        s.audio_filled = a.filled;
        s.audio_dropped = a.dropped + a.writer.dropped();
    }
    if let Some(a) = &st.system {
        s.system_samples = a.samples;
        s.system_filled = a.filled;
        s.system_dropped = a.dropped + a.writer.dropped();
    }
    s
}

/// A running source (the synthetic generator, a system capture): stopped at the stop.
pub trait Source: Send {
    fn stop(self: Box<Self>);
}

/// A running recording on this pipeline.
pub struct Recorder {
    core: Arc<Core>,
    sources: Mutex<Vec<Box<dyn Source>>>,
    running: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl Recorder {
    /// Starts the pacer (real time) and the 2-second fsync around a core whose sources run.
    pub fn new(core: Arc<Core>, sources: Vec<Box<dyn Source>>, pictures: bool) -> Recorder {
        let running = Arc::new(AtomicBool::new(true));
        let mut threads = Vec::new();
        let fps = core.lock().fps;
        if !core.fast() && pictures {
            // Each slot is written once its picture had time to arrive.
            let (c, r) = (core.clone(), running.clone());
            threads.push(std::thread::spawn(move || {
                let interval = Duration::from_secs_f64(0.5 / fps as f64);
                while r.load(Ordering::Relaxed) {
                    c.emit_up_to(c.now() - ALLOWANCE);
                    std::thread::sleep(interval);
                }
            }));
        }
        let (c, r) = (core.clone(), running.clone());
        threads.push(std::thread::spawn(move || {
            let mut ticks = 0;
            while r.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(50));
                ticks += 1;
                if ticks % 40 == 0 {
                    c.sync();
                }
            }
        }));
        Recorder {
            core,
            sources: Mutex::new(sources),
            running,
            threads,
        }
    }

    pub fn pause(&self) {
        self.core.pause()
    }

    pub fn resume(&self) {
        self.core.resume()
    }

    pub fn status(&self) -> Status {
        self.core.status()
    }

    /// Stops and finishes the files; the error (a source or file failed) comes with the
    /// final status, and the files are kept either way.
    pub fn stop(mut self) -> (Status, Option<String>) {
        self.stop_inner()
    }

    fn stop_inner(&mut self) -> (Status, Option<String>) {
        self.core.begin_stop();
        // Late pictures and sound still arrive for a moment.
        if !self.core.fast() {
            std::thread::sleep(Duration::from_millis(250));
        }
        let sources: Vec<_> = self.sources.lock().unwrap().drain(..).collect();
        for s in sources {
            s.stop();
        }
        self.running.store(false, Ordering::Relaxed);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
        self.core.finish()
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        if self.running.load(Ordering::Relaxed) {
            self.stop_inner();
        }
    }
}

/// Where a region's pixels are on a display `width` × `height` pixels whose size in points
/// is `points` (the region is in points): even, inside the display.
pub fn region_rect(
    region: &crate::native::Region,
    points: (f64, f64),
    width: u32,
    height: u32,
) -> Rect {
    let s = width as f64 / points.0.max(1.0);
    let even = |v: f64| (v.round() as i64 & !1).max(0) as u32;
    let w = even(region.width * s).clamp(2, width & !1);
    let h = even(region.height * s).clamp(2, height & !1);
    Rect {
        x: even(region.x * s).min((width - w) & !1),
        y: even(region.y * s).min((height - h) & !1),
        width: w,
        height: h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_region_lands_on_even_pixels_inside_the_display() {
        let r = crate::native::Region {
            x: 100.5,
            y: 51.0,
            width: 320.0,
            height: 180.0,
        };
        assert_eq!(
            region_rect(&r, (960.0, 600.0), 1920, 1200),
            Rect {
                x: 200,
                y: 102,
                width: 640,
                height: 360
            }
        );
        // Past the right edge: moved inside.
        let r = crate::native::Region { x: 900.0, ..r };
        assert_eq!(region_rect(&r, (960.0, 600.0), 1920, 1200).x, 1280);
    }
}

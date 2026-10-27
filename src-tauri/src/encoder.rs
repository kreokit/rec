//! The Windows and Linux writer: one FFmpeg sidecar per track, fed raw pictures (NV12 or
//! BGRA, exactly one per output frame) or 32-bit float sound over its stdin, writing a
//! fragmented MP4 (keyframe and fragment every second). We do the timing (pipeline.rs);
//! FFmpeg only encodes. If Rec is killed, FFmpeg sees its stdin close and finishes the file.
//!
//! Each track has a queue of about a second in front of its FFmpeg; when it is full, new
//! media is dropped and counted, never silently, and a dropped picture or sound keeps its
//! place (the previous picture again, silence) so what follows stays in sync.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Sound: 48 kHz, as everywhere in Rec.
pub const RATE: i64 = 48_000;
/// Up to this much media waits in front of each encoder; beyond it, new media is dropped.
pub const QUEUE_SECONDS: f64 = 1.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// Y plane, then interleaved U/V at half size (video range, BT.709).
    Nv12,
    /// 8-bit B, G, R, A (full range RGB; converted to BT.709 by FFmpeg).
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    Bgra,
}

impl PixelFormat {
    fn ffmpeg(self) -> &'static str {
        match self {
            PixelFormat::Nv12 => "nv12",
            PixelFormat::Bgra => "bgra",
        }
    }
}

/// One picture, tightly packed (no row padding).
#[derive(Debug, Clone, PartialEq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    pub data: Vec<u8>,
}

/// A rectangle in pixels (even origin and size for NV12).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Picture {
    pub fn size(width: u32, height: u32, format: PixelFormat) -> usize {
        let (w, h) = (width as usize, height as usize);
        match format {
            PixelFormat::Nv12 => w * h + w * h.div_ceil(2),
            PixelFormat::Bgra => w * h * 4,
        }
    }

    /// A black picture.
    pub fn black(width: u32, height: u32, format: PixelFormat) -> Picture {
        let mut data = vec![0u8; Picture::size(width, height, format)];
        match format {
            PixelFormat::Nv12 => {
                let y = width as usize * height as usize;
                data[..y].fill(16);
                data[y..].fill(128);
            }
            PixelFormat::Bgra => data.chunks_exact_mut(4).for_each(|p| p[3] = 255),
        }
        Picture {
            width,
            height,
            format,
            data,
        }
    }

    /// The part `r` of this picture (clipped to it); the rest of the result is black.
    pub fn crop(&self, r: Rect) -> Picture {
        let mut out = Picture::black(r.width, r.height, self.format);
        let (sw, sh) = (self.width as usize, self.height as usize);
        let (ow, oh) = (r.width as usize, r.height as usize);
        let (x, y) = (r.x as usize, r.y as usize);
        if x >= sw || y >= sh {
            return out;
        }
        let cols = ow.min(sw - x);
        let rows = oh.min(sh - y);
        match self.format {
            PixelFormat::Bgra => {
                for row in 0..rows {
                    let from = ((y + row) * sw + x) * 4;
                    out.data[row * ow * 4..][..cols * 4]
                        .copy_from_slice(&self.data[from..from + cols * 4]);
                }
            }
            PixelFormat::Nv12 => {
                for row in 0..rows {
                    let from = (y + row) * sw + x;
                    out.data[row * ow..][..cols].copy_from_slice(&self.data[from..from + cols]);
                }
                let (suv, ouv) = (sw * sh, ow * oh);
                let (x2, y2) = (x & !1, y / 2);
                let cols = cols & !1;
                for row in 0..rows.div_ceil(2).min(sh.div_ceil(2) - y2) {
                    let from = suv + (y2 + row) * sw + x2;
                    out.data[ouv + row * ow..][..cols]
                        .copy_from_slice(&self.data[from..from + cols]);
                }
            }
        }
        out
    }
}

/// What a picture track's FFmpeg reads: every picture has this size and format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoInput {
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
}

/// A track's FFmpeg: what it reads and writes.
#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Video {
        input: VideoInput,
        /// The size written (FFmpeg scales when the input differs).
        width: u32,
        height: u32,
        fps: u32,
        bitrate: u32,
        encoder: &'static str,
    },
    Audio {
        channels: u32,
    },
}

/// The fragmented MP4 every track is written as: a crash keeps everything up to the last
/// fragment (one a second; the video's keyframes too).
const MP4: &[&str] = &[
    "-movflags",
    "+frag_keyframe+empty_moov+default_base_moof",
    "-frag_duration",
    "1000000",
    "-flush_packets",
    "1",
    "-f",
    "mp4",
];

/// The FFmpeg arguments for a track written to `output`.
pub fn args(kind: &Kind, output: &Path) -> Vec<String> {
    let mut a: Vec<String> = ["-hide_banner", "-loglevel", "error", "-y"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let push = |a: &mut Vec<String>, items: &[&str]| a.extend(items.iter().map(|s| s.to_string()));
    match kind {
        Kind::Video {
            input,
            width,
            height,
            fps,
            bitrate,
            encoder,
        } => {
            let vaapi = *encoder == "h264_vaapi";
            if vaapi {
                push(&mut a, &["-vaapi_device", VAAPI_DEVICE]);
            }
            let size = format!("{}x{}", input.width, input.height);
            push(
                &mut a,
                &["-f", "rawvideo", "-pix_fmt", input.format.ffmpeg()],
            );
            push(&mut a, &["-s", &size, "-framerate", &fps.to_string()]);
            if input.format == PixelFormat::Nv12 {
                // Already BT.709 video range (the synthetic sources, NV12 cameras).
                push(&mut a, &["-color_range", "tv", "-colorspace", "bt709"]);
            }
            push(&mut a, &["-i", "pipe:0"]);
            // Into BT.709 video-range 4:2:0 at the output size (only where it differs); a
            // picture of another shape (Linux: the portal's choice) is letterboxed.
            let mut filters = Vec::new();
            if input.format != PixelFormat::Nv12 || (input.width, input.height) != (*width, *height)
            {
                filters.push(format!(
                    "scale={width}:{height}:flags=bicubic:out_color_matrix=bt709:out_range=tv:force_original_aspect_ratio=decrease:force_divisible_by=2,pad={width}:{height}:(ow-iw)/2:(oh-ih)/2"
                ));
            }
            // The frames carry the colour description the encoder writes.
            filters.push(
                "format=nv12,setparams=range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709"
                    .into(),
            );
            if vaapi {
                filters.push("hwupload".into());
            }
            push(&mut a, &["-vf", &filters.join(",")]);
            push(&mut a, &["-c:v", encoder, "-profile:v", "high"]);
            push(&mut a, &["-b:v", &bitrate.to_string()]);
            // A keyframe every second.
            let gop = fps.to_string();
            push(&mut a, &["-g", &gop, "-keyint_min", &gop]);
            if *encoder == "h264_videotoolbox" {
                // Small sizes need Apple's software encoder (development only).
                push(&mut a, &["-allow_sw", "1"]);
            }
            if *encoder == "h264_mf" {
                // The software encoder too where there's no hardware one (VMs).
                push(&mut a, &["-rate_control", "cbr"]);
            }
            push(
                &mut a,
                &[
                    "-colorspace",
                    "bt709",
                    "-color_primaries",
                    "bt709",
                    "-color_trc",
                    "bt709",
                    "-color_range",
                    "tv",
                ],
            );
        }
        Kind::Audio { channels } => {
            let channels = channels.to_string();
            push(&mut a, &["-f", "f32le", "-ar", "48000", "-ac", &channels]);
            push(&mut a, &["-i", "pipe:0", "-c:a", "aac"]);
            let bitrate = if channels == "1" { "128000" } else { "192000" };
            push(&mut a, &["-b:a", bitrate]);
        }
    }
    push(&mut a, MP4);
    a.push(output.to_string_lossy().into_owned());
    a
}

const VAAPI_DEVICE: &str = "/dev/dri/renderD128";

/// H.264 encoders to try, in order (as KreoKit Clips): the system's (Media Foundation on
/// Windows, VideoToolbox on macOS for the dev feature, VA-API on Linux where a GPU is), then
/// OpenH264; libx264 (GPL) only in debug builds, for a developer's GPL FFmpeg.
/// `KREOKIT_DEV_ENCODER` (debug builds): `release` = this order without libx264, or one name.
fn candidates() -> Vec<&'static str> {
    let mut all = Vec::new();
    if cfg!(target_os = "macos") {
        all.push("h264_videotoolbox");
    }
    if cfg!(target_os = "windows") {
        all.push("h264_mf");
    }
    if cfg!(target_os = "linux") && Path::new(VAAPI_DEVICE).exists() {
        all.push("h264_vaapi");
    }
    let dev = if cfg!(debug_assertions) {
        std::env::var("KREOKIT_DEV_ENCODER").unwrap_or_default()
    } else {
        String::new()
    };
    match dev.trim() {
        "" | "default" => {
            if cfg!(debug_assertions) {
                all.push("libx264");
            }
            all.push("libopenh264");
            all
        }
        "release" => {
            all.push("libopenh264");
            all
        }
        "mf" | "h264_mf" => vec!["h264_mf"],
        "vaapi" | "h264_vaapi" => vec!["h264_vaapi"],
        "videotoolbox" | "h264_videotoolbox" => vec!["h264_videotoolbox"],
        "openh264" | "libopenh264" => vec!["libopenh264"],
        "x264" | "libx264" => vec!["libx264"],
        _ => vec!["libopenh264"],
    }
}

/// The H.264 profiles accepted from `encoder` (as KreoKit Clips decided): OpenH264 also
/// Constrained Baseline (FFmpeg's wrapper may ignore `-profile high`), Media Foundation
/// High or Main.
#[cfg(test)]
pub fn accepted_profiles(encoder: &str) -> &'static [&'static str] {
    match encoder {
        "h264_mf" => &["High", "Main"],
        "libopenh264" => &["High", "Main", "Constrained Baseline"],
        _ => &["High"],
    }
}

/// The first encoder that works here: each candidate encodes two small pictures (a listed
/// encoder can still fail, e.g. Media Foundation without its codec pack, VA-API without a
/// driver). Found once per run.
pub fn video_encoder() -> Result<&'static str, String> {
    static FOUND: OnceLock<Result<&'static str, String>> = OnceLock::new();
    FOUND
        .get_or_init(|| {
            let ffmpeg = crate::ffmpeg::locate("ffmpeg").map_err(|_| NO_FFMPEG.to_string())?;
            let mut tried = Vec::new();
            for encoder in candidates() {
                match try_encoder(&ffmpeg, encoder) {
                    Ok(()) => return Ok(encoder),
                    Err(e) => tried.push(format!("{encoder}: {e}")),
                }
            }
            Err(format!(
                "No video encoder works on this computer ({}). Use Report a problem so we can look at it.",
                tried.join("; ")
            ))
        })
        .clone()
}

const NO_FFMPEG: &str = "KreoKit Rec's video encoder is missing. Install KreoKit Rec again.";

fn try_encoder(ffmpeg: &Path, encoder: &'static str) -> Result<(), String> {
    let kind = Kind::Video {
        input: VideoInput {
            width: 320,
            height: 180,
            format: PixelFormat::Nv12,
        },
        width: 320,
        height: 180,
        fps: 30,
        bitrate: 500_000,
        encoder,
    };
    let mut args = args(&kind, Path::new("-"));
    // To nowhere: replace the MP4 output by the null muxer.
    let mp4 = args.len() - 1 - MP4.len();
    args.truncate(mp4);
    args.extend(["-f", "null", "-"].map(String::from));
    let mut child = Command::new(ffmpeg)
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let picture = Picture::black(320, 180, PixelFormat::Nv12);
    if let Some(mut stdin) = child.stdin.take() {
        for _ in 0..2 {
            let _ = stdin.write_all(&picture.data);
        }
    }
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(last_line(&String::from_utf8_lossy(&output.stderr)))
    }
}

fn last_line(s: &str) -> String {
    s.lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("it stopped")
        .trim()
        .to_string()
}

enum Item {
    Picture(Arc<Picture>),
    Samples(Vec<f32>),
    /// Silence: this many sample frames.
    Silence(i64),
}

struct Queue {
    items: VecDeque<(Item, i64)>,
    closed: bool,
    /// The picture enqueued last (a repeat of it costs nothing).
    last: Option<Arc<Picture>>,
}

/// One track's file and its FFmpeg.
pub struct Writer {
    pub path: PathBuf,
    channels: i64,
    frame_bytes: usize,
    queue: Mutex<Queue>,
    wake: Condvar,
    /// Video: pictures; sound: sample frames.
    queued: AtomicI64,
    pub limit: i64,
    dropped: AtomicI64,
    failure: Mutex<Option<String>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Writer {
    /// Starts FFmpeg for `kind`, writing `path`. `what` names the track in messages
    /// ("screen", "camera", "microphone", "computer sound").
    pub fn start(path: PathBuf, kind: Kind, what: &'static str) -> Result<Arc<Writer>, String> {
        let ffmpeg = crate::ffmpeg::locate("ffmpeg").map_err(|_| NO_FFMPEG.to_string())?;
        let _ = std::fs::remove_file(&path);
        let (limit, channels, frame_bytes) = match &kind {
            Kind::Video { fps, input, .. } => (
                (*fps as f64 * QUEUE_SECONDS) as i64,
                0,
                Picture::size(input.width, input.height, input.format),
            ),
            Kind::Audio { channels } => ((RATE as f64 * QUEUE_SECONDS) as i64, *channels as i64, 0),
        };
        let mut child = Command::new(&ffmpeg)
            .args(args(&kind, &path))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                format!("The {what} recording can't start ({e}). Install KreoKit Rec again.")
            })?;
        let stdin = child.stdin.take().expect("piped");
        let writer = Arc::new(Writer {
            path,
            channels,
            frame_bytes,
            queue: Mutex::new(Queue {
                items: VecDeque::new(),
                closed: false,
                last: None,
            }),
            wake: Condvar::new(),
            queued: AtomicI64::new(0),
            limit,
            dropped: AtomicI64::new(0),
            failure: Mutex::new(None),
            thread: Mutex::new(None),
        });
        let w = writer.clone();
        let thread = std::thread::Builder::new()
            .name(format!("kreokit-rec-{what}"))
            .spawn(move || w.run(child, stdin, what))
            .map_err(|e| e.to_string())?;
        *writer.thread.lock().unwrap() = Some(thread);
        Ok(writer)
    }

    pub fn queued(&self) -> i64 {
        self.queued.load(Ordering::Relaxed)
    }

    pub fn dropped(&self) -> i64 {
        self.dropped.load(Ordering::Relaxed)
    }

    pub fn failure(&self) -> Option<String> {
        self.failure.lock().unwrap().clone()
    }

    fn push(&self, item: Item, cost: i64) {
        let mut q = self.queue.lock().unwrap();
        if q.closed {
            return;
        }
        self.queued.fetch_add(cost, Ordering::Relaxed);
        q.items.push_back((item, cost));
        self.wake.notify_one();
    }

    /// Queues a picture (it must have the input's size). Returns false when the queue was
    /// full: then the previous picture takes its place, so the timing stays.
    pub fn picture(&self, picture: Arc<Picture>) -> bool {
        let mut q = self.queue.lock().unwrap();
        let repeat = q.last.as_ref().is_some_and(|l| Arc::ptr_eq(l, &picture));
        let full = !repeat && self.queued() + 1 > self.limit;
        let (item, cost) = match (full, q.last.clone()) {
            (false, _) => (picture.clone(), if repeat { 0 } else { 1 }),
            (true, Some(last)) => (last, 0),
            // Nothing before it: can't happen with a full queue.
            (true, None) => (picture.clone(), 1),
        };
        if full {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        } else {
            q.last = Some(picture);
        }
        drop(q);
        self.push(Item::Picture(item), cost);
        !full
    }

    /// Queues `frames` sample frames (interleaved when stereo), or silence for `None`.
    /// Returns false when the queue was full: then silence takes their place.
    pub fn samples(&self, samples: Option<&[f32]>, frames: i64) -> bool {
        if frames <= 0 {
            return true;
        }
        match samples {
            Some(s) if self.queued() + frames <= self.limit => {
                let n = frames as usize * self.channels as usize;
                self.push(Item::Samples(s[..n].to_vec()), frames);
                true
            }
            Some(_) => {
                self.dropped.fetch_add(frames, Ordering::Relaxed);
                self.push(Item::Silence(frames), 0);
                false
            }
            None => {
                self.push(Item::Silence(frames), 0);
                true
            }
        }
    }

    /// The writer thread: hands each item to FFmpeg as soon as it takes it.
    fn run(self: Arc<Self>, mut child: Child, stdin: ChildStdin, what: &'static str) {
        // FFmpeg's messages, for a failure.
        let mut stderr = child.stderr.take().expect("piped");
        let log = std::thread::spawn(move || {
            let mut s = String::new();
            let _ = stderr.read_to_string(&mut s);
            s
        });
        let mut stdin = Some(stdin);
        let mut broken = None;
        let zeros = vec![0u8; 4096];
        loop {
            let (item, cost) = {
                let mut q = self.queue.lock().unwrap();
                loop {
                    if let Some(next) = q.items.pop_front() {
                        break next;
                    }
                    if q.closed {
                        break (Item::Silence(-1), 0);
                    }
                    q = self.wake.wait(q).unwrap();
                }
            };
            if let Item::Silence(-1) = item {
                break;
            }
            if broken.is_none()
                && let Some(pipe) = stdin.as_mut()
            {
                let result = match &item {
                    Item::Picture(p) if p.data.len() == self.frame_bytes => pipe.write_all(&p.data),
                    Item::Picture(p) => Err(std::io::Error::other(format!(
                        "a picture of {}×{} doesn't fit",
                        p.width, p.height
                    ))),
                    Item::Samples(s) => {
                        let bytes: Vec<u8> = s.iter().flat_map(|v| v.to_le_bytes()).collect();
                        pipe.write_all(&bytes)
                    }
                    Item::Silence(frames) => {
                        let mut left = *frames as usize * self.channels as usize * 4;
                        let mut r = Ok(());
                        while left > 0 && r.is_ok() {
                            let n = left.min(zeros.len());
                            r = pipe.write_all(&zeros[..n]);
                            left -= n;
                        }
                        r
                    }
                };
                if let Err(e) = result {
                    broken = Some(e.to_string());
                }
            }
            self.queued.fetch_sub(cost, Ordering::Relaxed);
        }
        // The end of the input: FFmpeg finishes the file.
        drop(stdin);
        let deadline = Instant::now() + Duration::from_secs(30);
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(5))
                }
                _ => {
                    let _ = child.kill();
                    break child.wait().ok();
                }
            }
        };
        let log = log.join().unwrap_or_default();
        if broken.is_some() || !status.is_some_and(|s| s.success()) {
            let detail = if log.trim().is_empty() {
                broken.unwrap_or_else(|| "the encoder stopped".into())
            } else {
                last_line(&log)
            };
            *self.failure.lock().unwrap() = Some(format!(
                "The {what} recording stopped early ({detail}). What was recorded until then is kept. If it happens again, use Report a problem."
            ));
        }
    }

    /// What the system still holds in memory goes to the disk (power loss).
    pub fn sync(&self) {
        if let Ok(file) = std::fs::OpenOptions::new().write(true).open(&self.path) {
            let _ = file.sync_data();
        }
    }

    /// Writes what is queued, lets FFmpeg finish the file and waits for it (up to 30 s).
    pub fn finish(&self) {
        {
            let mut q = self.queue.lock().unwrap();
            q.closed = true;
            self.wake.notify_one();
        }
        if let Some(thread) = self.thread.lock().unwrap().take() {
            let _ = thread.join();
        }
        self.sync();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_is_lgpl() {
        for encoder in ["h264_mf", "h264_vaapi", "libopenh264", "h264_videotoolbox"] {
            for format in [PixelFormat::Nv12, PixelFormat::Bgra] {
                let kind = Kind::Video {
                    input: VideoInput {
                        width: 1920,
                        height: 1080,
                        format,
                    },
                    width: 1280,
                    height: 720,
                    fps: 30,
                    bitrate: 6_000_000,
                    encoder,
                };
                assert_eq!(
                    crate::ffmpeg::gpl_only(&args(&kind, Path::new("s.mp4"))),
                    None
                );
            }
        }
        for channels in [1, 2] {
            let kind = Kind::Audio { channels };
            assert_eq!(
                crate::ffmpeg::gpl_only(&args(&kind, Path::new("m.mp4"))),
                None
            );
        }
        // Release builds never pick libx264.
        if !cfg!(debug_assertions) {
            assert!(!candidates().contains(&"libx264"));
        }
    }

    #[test]
    fn crops_nv12_and_bgra() {
        let mut p = Picture::black(8, 4, PixelFormat::Nv12);
        for (i, v) in p.data.iter_mut().enumerate() {
            *v = i as u8;
        }
        let c = p.crop(Rect {
            x: 2,
            y: 2,
            width: 4,
            height: 2,
        });
        assert_eq!(&c.data[..8], &[18, 19, 20, 21, 26, 27, 28, 29]);
        // The chroma row of rows 2-3: U/V pairs from x 2.
        assert_eq!(
            &c.data[8..],
            &[32 + 8 + 2, 32 + 8 + 3, 32 + 8 + 4, 32 + 8 + 5]
        );
        // Outside the picture: black.
        let edge = p.crop(Rect {
            x: 6,
            y: 0,
            width: 4,
            height: 2,
        });
        assert_eq!(&edge.data[..4], &[6, 7, 16, 16]);
        let mut b = Picture::black(3, 2, PixelFormat::Bgra);
        b.data[4 * 4..4 * 5].copy_from_slice(&[1, 2, 3, 4]);
        let c = b.crop(Rect {
            x: 1,
            y: 1,
            width: 1,
            height: 1,
        });
        assert_eq!(c.data, vec![1, 2, 3, 4]);
    }

    #[test]
    fn a_full_queue_keeps_the_timing() {
        // Without FFmpeg: the queue's own rules.
        let w = Writer {
            path: PathBuf::new(),
            channels: 1,
            frame_bytes: 0,
            queue: Mutex::new(Queue {
                items: VecDeque::new(),
                closed: false,
                last: None,
            }),
            wake: Condvar::new(),
            queued: AtomicI64::new(0),
            limit: 2,
            dropped: AtomicI64::new(0),
            failure: Mutex::new(None),
            thread: Mutex::new(None),
        };
        let a = Arc::new(Picture::black(2, 2, PixelFormat::Nv12));
        let b = Arc::new(Picture::black(2, 2, PixelFormat::Nv12));
        let c = Arc::new(Picture::black(2, 2, PixelFormat::Nv12));
        assert!(w.picture(a.clone()));
        assert!(w.picture(a.clone())); // a repeat costs nothing
        assert!(w.picture(b.clone()));
        assert!(!w.picture(c)); // full: b again in its place
        assert_eq!((w.queued(), w.dropped()), (2, 1));
        let q = w.queue.lock().unwrap();
        assert_eq!(q.items.len(), 4);
        assert!(matches!(&q.items[3].0, Item::Picture(p) if Arc::ptr_eq(p, &b)));
        drop(q);
        assert!(!w.samples(Some(&[0.0; 4]), 4));
        assert_eq!(w.dropped(), 5);
    }
}

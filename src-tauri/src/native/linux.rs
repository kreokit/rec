//! Linux capture (docs/capture-research.md): the screen through xdg-desktop-portal's
//! ScreenCast (ashpd) and a PipeWire stream, the system's own dialog choosing the display or
//! window (remembered with a restore token); FFmpeg's x11grab only on X11 desktops without
//! that portal. Cameras through V4L2 (NV12 or YUYV at 30 fps), microphones and the computer
//! sound through cpal's PipeWire host (audio.rs).
//!
//! Pictures are stamped with the host clock (CLOCK_MONOTONIC) when they arrive; V4L2
//! cameras stamp their own on the same clock.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::Duration;

use ashpd::desktop::PersistMode;
use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
use pipewire as pw;
use pw::spa;
use v4l::video::Capture as _;

use super::*;
use crate::encoder::{Picture, PixelFormat, Rect, VideoInput};
use crate::pipeline::{Core, Inputs, Source, host_seconds, region_rect};

const NO_PORTAL: &str = "This desktop doesn't offer screen recording to apps (the \"ScreenCast\" desktop portal is missing). Install xdg-desktop-portal with the portal for your desktop (GNOME, KDE or wlroots), log out and in again, then try again.";
const NO_PIPEWIRE: &str = "Screen recording needs PipeWire, which isn't running. Install or start PipeWire (most desktops since 2022 have it), log out and in again, then try again.";
const CAMERA_DENIED: &str = "If it says permission denied: add yourself to the \"video\" group (sudo usermod -aG video $USER), log out and in again, then try again.";

/// Where the portal's restore tokens are kept (the app's data folder, set at start-up).
static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

pub fn set_data_dir(dir: PathBuf) {
    let _ = DATA_DIR.set(dir);
}

fn token_file(window: bool) -> Option<PathBuf> {
    let name = if window {
        "screencast-window.token"
    } else {
        "screencast-display.token"
    };
    DATA_DIR.get().map(|d| d.join(name))
}

fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())
}

/// What the ScreenCast portal offers (none without the portal).
fn portal_sources() -> Option<ashpd::enumflags2::BitFlags<SourceType>> {
    static FOUND: OnceLock<Option<ashpd::enumflags2::BitFlags<SourceType>>> = OnceLock::new();
    *FOUND.get_or_init(|| {
        let rt = runtime().ok()?;
        rt.block_on(async {
            let proxy = tokio::time::timeout(Duration::from_secs(3), Screencast::new())
                .await
                .ok()?
                .ok()?;
            proxy.available_source_types().await.ok()
        })
    })
}

fn x11() -> bool {
    std::env::var_os("DISPLAY").is_some()
        && std::env::var("XDG_SESSION_TYPE").map_or(true, |t| t != "wayland")
}

/// Whether x11grab records here: X11 and no ScreenCast portal.
fn use_x11grab() -> bool {
    portal_sources().is_none() && x11()
}

// --- Permissions ------------------------------------------------------------------------

/// Linux asks in the portal's dialog when recording starts; cameras and microphones need no
/// permission (the "video" group decides for cameras, see [`CAMERA_DENIED`]).
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

// --- Displays and windows ----------------------------------------------------------------

/// The X11 screen's size, from x11grab (ffprobe of one picture).
fn x11_size() -> Option<(u32, u32)> {
    static SIZE: OnceLock<Option<(u32, u32)>> = OnceLock::new();
    *SIZE.get_or_init(|| {
        let ffprobe = crate::ffmpeg::locate("ffprobe").ok()?;
        let display = std::env::var("DISPLAY").ok()?;
        let out = Command::new(ffprobe)
            .args(["-v", "error", "-f", "x11grab", "-i", &display])
            .args(["-show_entries", "stream=width,height", "-of", "csv=p=0"])
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let mut it = text.trim().split(',').map(|v| v.trim().parse::<u32>().ok());
        Some((it.next()??, it.next()??))
    })
}

/// One entry: the portal's dialog chooses the display when recording starts (its size is
/// known only then; a picture of another shape is letterboxed, encoder.rs).
pub fn displays() -> Vec<Display> {
    let (portal, size) = (portal_sources(), x11_size());
    if portal.is_none() && !(x11() && size.is_some()) {
        return Vec::new();
    }
    let (width, height) = size.unwrap_or((1920, 1080));
    vec![Display {
        id: 0,
        name: if portal.is_some() {
            "The screen you choose when recording starts".into()
        } else {
            "Screen".into()
        },
        width,
        height,
        main: true,
        x: 0.0,
        y: 0.0,
        points_width: width as f64,
        points_height: height as f64,
    }]
}

pub fn window_capture_supported() -> bool {
    portal_sources().is_some_and(|s| s.contains(SourceType::Window))
}

/// One entry: the portal's dialog chooses the window when recording starts.
pub fn windows() -> Vec<Window> {
    if !window_capture_supported() {
        return Vec::new();
    }
    vec![Window {
        id: 1,
        app: String::new(),
        title: "The window you choose when recording starts".into(),
        width: 1280.0,
        height: 720.0,
        scale: 1.0,
    }]
}

// --- Screen: the portal and PipeWire ------------------------------------------------------

/// The core, once the recording knows the screen's size (pictures before it are dropped).
type CoreCell = Arc<OnceLock<Arc<Core>>>;

/// Asks the portal for a display or window (the dialog, unless a restore token answers),
/// and returns the PipeWire node and connection.
async fn open_portal(
    window: bool,
) -> Result<
    (
        u32,
        std::os::fd::OwnedFd,
        ashpd::desktop::Session<Screencast>,
    ),
    String,
> {
    let proxy = Screencast::new().await.map_err(|_| NO_PORTAL.to_string())?;
    let session = proxy
        .create_session(Default::default())
        .await
        .map_err(|e| format!("The screen recording couldn't start ({e}). Try again."))?;
    let cursor = match proxy.available_cursor_modes().await {
        Ok(modes) if modes.contains(CursorMode::Embedded) => CursorMode::Embedded,
        _ => CursorMode::Hidden,
    };
    let token = token_file(window).and_then(|f| std::fs::read_to_string(f).ok());
    let kind = if window {
        SourceType::Window
    } else {
        SourceType::Monitor
    };
    proxy
        .select_sources(
            &session,
            SelectSourcesOptions::default()
                .set_cursor_mode(cursor)
                .set_sources(ashpd::enumflags2::BitFlags::from(kind))
                .set_multiple(false)
                .set_restore_token(token.as_deref())
                .set_persist_mode(PersistMode::ExplicitlyRevoked),
        )
        .await
        .map_err(|e| format!("The screen recording couldn't start ({e}). Try again."))?;
    let response = proxy
        .start(&session, None, Default::default())
        .await
        .and_then(|r| r.response())
        .map_err(|e| match e {
            ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled) => {
                "No screen was chosen. Record again and choose a screen or window in the dialog."
                    .to_string()
            }
            e => format!("The screen recording couldn't start ({e}). Try again."),
        })?;
    if let (Some(t), Some(f)) = (response.restore_token(), token_file(window)) {
        if let Some(dir) = f.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(f, t);
    }
    let stream = response
        .streams()
        .first()
        .ok_or("No screen was chosen. Record again and choose a screen or window in the dialog.")?
        .to_owned();
    let fd = proxy
        .open_pipe_wire_remote(&session, Default::default())
        .await
        .map_err(|_| NO_PIPEWIRE.to_string())?;
    Ok((stream.pipe_wire_node_id(), fd, session))
}

/// Copies a BGRx/BGRA or RGBx/RGBA picture with `stride` bytes a row into packed BGRA.
fn bgra_from(src: &[u8], width: u32, height: u32, stride: usize, rgb: bool) -> Option<Picture> {
    let (w, h) = (width as usize, height as usize);
    if stride < w * 4 || src.len() < stride * (h - 1) + w * 4 {
        return None;
    }
    let mut data = Vec::with_capacity(w * h * 4);
    for row in 0..h {
        let line = &src[row * stride..row * stride + w * 4];
        if rgb {
            for p in line.chunks_exact(4) {
                data.extend([p[2], p[1], p[0], 255]);
            }
        } else {
            data.extend_from_slice(line);
        }
    }
    Some(Picture {
        width,
        height,
        format: PixelFormat::Bgra,
        data,
    })
}

enum Quit {
    Now,
}

/// The PipeWire stream of the portal's choice, on its own thread.
struct PortalCapture {
    quit: Option<pw::channel::Sender<Quit>>,
    thread: Option<JoinHandle<()>>,
}

impl Source for PortalCapture {
    fn stop(mut self: Box<Self>) {
        if let Some(q) = self.quit.take() {
            let _ = q.send(Quit::Now);
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Opens the portal (waiting for the user's choice) and the PipeWire stream; returns once
/// the picture's size is known.
fn start_portal(window: bool, cell: CoreCell) -> Result<(PortalCapture, (u32, u32)), String> {
    let (sized, result) = std::sync::mpsc::channel::<Result<(u32, u32), String>>();
    let (quit, quit_rx) = pw::channel::channel::<Quit>();
    let thread = std::thread::Builder::new()
        .name("kreokit-rec-screen".into())
        .spawn(move || {
            // The runtime and the session live as long as the stream (the portal ends the
            // recording when its session closes).
            let rt = match runtime() {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = sized.send(Err(e));
                    return;
                }
            };
            let (node, fd, session) = match rt.block_on(open_portal(window)) {
                Ok(v) => v,
                Err(e) => {
                    let _ = sized.send(Err(e));
                    return;
                }
            };
            if let Err(e) = run_stream(node, fd, window, cell, sized.clone(), quit_rx) {
                let _ = sized.send(Err(format!("{NO_PIPEWIRE} ({e})")));
            }
            let _ = rt.block_on(session.close());
        })
        .map_err(|e| e.to_string())?;
    let mut capture = PortalCapture {
        quit: Some(quit),
        thread: Some(thread),
    };
    // The dialog waits for the user.
    match result.recv_timeout(Duration::from_secs(300)) {
        Ok(Ok(size)) => Ok((capture, size)),
        Ok(Err(e)) => {
            Box::new(capture).stop();
            Err(e)
        }
        Err(_) => {
            if let Some(q) = capture.quit.take() {
                let _ = q.send(Quit::Now);
            }
            Err("No screen was chosen in time. Record again and choose a screen or window in the dialog.".into())
        }
    }
}

fn run_stream(
    node: u32,
    fd: std::os::fd::OwnedFd,
    window: bool,
    cell: CoreCell,
    sized: std::sync::mpsc::Sender<Result<(u32, u32), String>>,
    quit: pw::channel::Receiver<Quit>,
) -> Result<(), pw::Error> {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&mainloop, None)?;
    let core = context.connect_fd_rc(fd, None)?;
    let stream = pw::stream::StreamRc::new(
        core,
        "kreokit-rec",
        pw::properties::properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )?;
    let format = Arc::new(Mutex::new(spa::param::video::VideoInfoRaw::default()));
    let (f1, f2) = (format.clone(), format);
    let (c1, c2) = (cell.clone(), cell);
    let told = Arc::new(AtomicBool::new(false));
    let _listener = stream
        .add_local_listener_with_user_data(())
        .state_changed(move |_, _, _, new| {
            // A recorded window that closes ends its stream: its last picture stays.
            if window
                && matches!(
                    new,
                    pw::stream::StreamState::Unconnected | pw::stream::StreamState::Error(_)
                )
                && let Some(core) = c1.get()
            {
                core.window_closed();
            }
        })
        .param_changed(move |_, _, id, param| {
            let Some(param) = param else { return };
            if id != spa::param::ParamType::Format.as_raw() {
                return;
            }
            let mut f = f1.lock().unwrap();
            if f.parse(param).is_ok() && !told.swap(true, Ordering::Relaxed) {
                let size = f.size();
                let _ = sized.send(Ok((size.width, size.height)));
            }
        })
        .process(move |stream, _| {
            let t = host_seconds();
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let Some(core) = c2.get() else { return };
            let datas = buffer.datas_mut();
            let Some(d) = datas.first_mut() else { return };
            let (size, stride, offset) = (
                d.chunk().size() as usize,
                d.chunk().stride().max(0) as usize,
                d.chunk().offset() as usize,
            );
            if size == 0 {
                return; // only the pointer moved
            }
            let f = *f2.lock().unwrap();
            let (w, h) = (f.size().width, f.size().height);
            let rgb = matches!(
                f.format(),
                spa::param::video::VideoFormat::RGBx | spa::param::video::VideoFormat::RGBA
            );
            let Some(bytes) = d.data() else { return };
            let end = (offset + size).min(bytes.len());
            let stride = if stride == 0 { w as usize * 4 } else { stride };
            if let Some(p) = bgra_from(&bytes[offset.min(end)..end], w, h, stride, rgb) {
                core.screen(Arc::new(p), t);
            }
        })
        .register()?;
    // Raw BGRx/RGBx (or with alpha) at up to 60 fps, any size.
    use spa::param::format::{FormatProperties, MediaSubtype, MediaType};
    use spa::param::video::VideoFormat;
    let obj = spa::pod::object!(
        spa::utils::SpaTypes::ObjectParamFormat,
        spa::param::ParamType::EnumFormat,
        spa::pod::property!(FormatProperties::MediaType, Id, MediaType::Video),
        spa::pod::property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
        spa::pod::property!(
            FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            VideoFormat::BGRx,
            VideoFormat::BGRx,
            VideoFormat::BGRA,
            VideoFormat::RGBx,
            VideoFormat::RGBA
        ),
        spa::pod::property!(
            FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            spa::utils::Rectangle {
                width: 1920,
                height: 1080
            },
            spa::utils::Rectangle {
                width: 1,
                height: 1
            },
            spa::utils::Rectangle {
                width: 8192,
                height: 8192
            }
        ),
        spa::pod::property!(
            FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            spa::utils::Fraction { num: 60, denom: 1 },
            spa::utils::Fraction { num: 0, denom: 1 },
            spa::utils::Fraction {
                num: 1000,
                denom: 1
            }
        ),
    );
    let values: Vec<u8> = spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(obj),
    )
    .map_err(|_| pw::Error::CreationFailed)?
    .0
    .into_inner();
    let mut params = [spa::pod::Pod::from_bytes(&values).ok_or(pw::Error::CreationFailed)?];
    stream.connect(
        spa::utils::Direction::Input,
        Some(node),
        pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
        &mut params,
    )?;
    let ml = mainloop.clone();
    let _quit = quit.attach(mainloop.loop_(), move |_| ml.quit());
    mainloop.run();
    let _ = stream.disconnect();
    Ok(())
}

// --- Screen: x11grab (X11 without the portal) ----------------------------------------------

/// FFmpeg's x11grab at 60 fps, read raw from its output.
struct X11Capture {
    child: Arc<Mutex<Child>>,
    thread: Option<JoinHandle<()>>,
}

impl Source for X11Capture {
    fn stop(mut self: Box<Self>) {
        let _ = self.child.lock().unwrap().kill();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        let _ = self.child.lock().unwrap().wait();
    }
}

fn x11grab_args(display: &str, area: Rect) -> Vec<String> {
    let input = format!("{display}+{},{}", area.x, area.y);
    let size = format!("{}x{}", area.width, area.height);
    [
        "-hide_banner",
        "-loglevel",
        "error",
        "-f",
        "x11grab",
        "-draw_mouse",
        "1",
        "-framerate",
        "60",
        "-video_size",
        &size,
        "-i",
        &input,
        "-f",
        "rawvideo",
        "-pix_fmt",
        "bgra",
        "-",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

fn start_x11grab(core: Arc<Core>, area: Rect) -> Result<X11Capture, String> {
    let ffmpeg = crate::ffmpeg::locate("ffmpeg").map_err(|_| {
        "KreoKit Rec's video encoder is missing. Install KreoKit Rec again.".to_string()
    })?;
    let display = std::env::var("DISPLAY").map_err(|_| NO_PORTAL.to_string())?;
    let mut child = Command::new(ffmpeg)
        .args(x11grab_args(&display, area))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("The screen can't be recorded ({e}). Try again."))?;
    let mut out = child.stdout.take().expect("piped");
    let child = Arc::new(Mutex::new(child));
    let thread = std::thread::spawn(move || {
        let size = Picture::size(area.width, area.height, PixelFormat::Bgra);
        loop {
            let mut data = vec![0u8; size];
            if out.read_exact(&mut data).is_err() {
                break; // stopped
            }
            let p = Picture {
                width: area.width,
                height: area.height,
                format: PixelFormat::Bgra,
                data,
            };
            core.screen(Arc::new(p), host_seconds());
        }
    });
    Ok(X11Capture {
        child,
        thread: Some(thread),
    })
}

// --- Cameras (V4L2) -------------------------------------------------------------------------

const NV12: v4l::FourCC = v4l::FourCC { repr: *b"NV12" };
const YUYV: v4l::FourCC = v4l::FourCC { repr: *b"YUYV" };

/// The camera's format the recorder uses: the largest up to 720 pixels high at 30 fps (or
/// at its fastest), NV12 before YUYV. Cameras offering only compressed pictures (MJPEG) are
/// not used yet.
fn best_format(dev: &v4l::Device) -> Option<(v4l::FourCC, u32, u32, bool)> {
    let mut best: Option<(v4l::FourCC, u32, u32, bool)> = None;
    for desc in dev.enum_formats().ok()? {
        if desc.fourcc != NV12 && desc.fourcc != YUYV {
            continue;
        }
        for size in dev.enum_framesizes(desc.fourcc).unwrap_or_default() {
            for d in size.size.to_discrete() {
                if d.height > 720 || d.width < 2 || d.height < 2 {
                    continue;
                }
                let thirty = dev
                    .enum_frameintervals(desc.fourcc, d.width, d.height)
                    .unwrap_or_default()
                    .iter()
                    .any(|i| match &i.interval {
                        v4l::frameinterval::FrameIntervalEnum::Discrete(f) => {
                            f.numerator > 0 && f.denominator as f64 / f.numerator as f64 >= 29.9
                        }
                        v4l::frameinterval::FrameIntervalEnum::Stepwise(s) => {
                            s.min.numerator > 0
                                && s.min.denominator as f64 / s.min.numerator as f64 >= 29.9
                        }
                    });
                let better = best.is_none_or(|(bf, bw, bh, bt)| {
                    (thirty, d.width * d.height, desc.fourcc == NV12) > (bt, bw * bh, bf == NV12)
                });
                if better {
                    best = Some((desc.fourcc, d.width & !1, d.height & !1, thirty));
                }
            }
        }
    }
    best
}

fn video_nodes() -> Vec<(String, String)> {
    let mut nodes: Vec<_> = v4l::context::enum_devices()
        .into_iter()
        .filter_map(|n| {
            let path = n.path().to_string_lossy().into_owned();
            let dev = v4l::Device::with_path(&path).ok()?;
            let caps = dev.query_caps().ok()?;
            if !caps
                .capabilities
                .contains(v4l::capability::Flags::VIDEO_CAPTURE)
            {
                return None;
            }
            best_format(&dev)?;
            Some((path, caps.card))
        })
        .collect();
    nodes.sort();
    nodes
}

pub fn cameras() -> Vec<Camera> {
    video_nodes()
        .into_iter()
        .enumerate()
        .filter_map(|(i, (path, name))| {
            let dev = v4l::Device::with_path(&path).ok()?;
            let (_, width, height, _) = best_format(&dev)?;
            Some(Camera {
                id: path,
                name,
                default: i == 0,
                width,
                height,
            })
        })
        .collect()
}

/// YUYV (4:2:2, packed) to NV12; the chroma of each pair of rows from the first.
fn nv12_from_yuyv(src: &[u8], width: u32, height: u32, stride: usize) -> Option<Picture> {
    let (w, h) = (width as usize, height as usize);
    if stride < w * 2 || src.len() < stride * (h - 1) + w * 2 {
        return None;
    }
    let mut data = vec![0u8; Picture::size(width, height, PixelFormat::Nv12)];
    let (y, uv) = data.split_at_mut(w * h);
    for row in 0..h {
        let line = &src[row * stride..row * stride + w * 2];
        for x in 0..w {
            y[row * w + x] = line[x * 2];
        }
        if row % 2 == 0 {
            for x in 0..w / 2 {
                uv[(row / 2) * w + x * 2] = line[x * 4 + 1];
                uv[(row / 2) * w + x * 2 + 1] = line[x * 4 + 3];
            }
        }
    }
    Some(Picture {
        width,
        height,
        format: PixelFormat::Nv12,
        data,
    })
}

/// NV12 with `stride` bytes a row, packed.
fn nv12_packed(src: &[u8], width: u32, height: u32, stride: usize) -> Option<Picture> {
    let (w, h) = (width as usize, height as usize);
    let rows = h + h.div_ceil(2);
    if stride < w || src.len() < stride * (rows - 1) + w {
        return None;
    }
    let mut data = Vec::with_capacity(w * rows);
    for row in 0..rows {
        data.extend_from_slice(&src[row * stride..row * stride + w]);
    }
    Some(Picture {
        width,
        height,
        format: PixelFormat::Nv12,
        data,
    })
}

struct CameraCapture {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Source for CameraCapture {
    fn stop(mut self: Box<Self>) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Opens the camera on its own thread and calls `each` with every picture and its host
/// time until stopped.
fn run_camera(
    path: String,
    mut each: impl FnMut(Picture, f64) + Send + 'static,
    on_error: impl FnOnce(String) + Send + 'static,
) -> Result<CameraCapture, String> {
    use v4l::io::traits::CaptureStream;
    let stop = Arc::new(AtomicBool::new(false));
    let (opened, result) = std::sync::mpsc::channel::<Result<(), String>>();
    let s = stop.clone();
    let thread = std::thread::spawn(move || {
        let fail = |e: std::io::Error| {
            format!(
                "The camera can't be opened ({e}). Close other apps that use it. {CAMERA_DENIED}"
            )
        };
        let dev = match v4l::Device::with_path(&path) {
            Ok(d) => d,
            Err(e) => {
                let _ = opened.send(Err(fail(e)));
                return;
            }
        };
        let Some((fourcc, w, h, _)) = best_format(&dev) else {
            let _ = opened.send(Err(
                "This camera offers no picture format KreoKit Rec can use yet. Try another camera."
                    .into(),
            ));
            return;
        };
        let format = match dev.set_format(&v4l::Format::new(w, h, fourcc)) {
            Ok(f) => f,
            Err(e) => {
                let _ = opened.send(Err(fail(e)));
                return;
            }
        };
        let _ = dev.set_params(&v4l::video::capture::Parameters::with_fps(30));
        let mut stream =
            match v4l::io::mmap::Stream::with_buffers(&dev, v4l::buffer::Type::VideoCapture, 4) {
                Ok(st) => st,
                Err(e) => {
                    let _ = opened.send(Err(fail(e)));
                    return;
                }
            };
        stream.set_timeout(Duration::from_millis(500));
        let _ = opened.send(Ok(()));
        let stride = format.stride as usize;
        let mut offset = None;
        let mut errors = 0;
        while !s.load(Ordering::Relaxed) {
            match stream.next() {
                Ok((buf, meta)) => {
                    errors = 0;
                    let used = (meta.bytesused as usize).min(buf.len());
                    let picture = if format.fourcc == NV12 {
                        nv12_packed(&buf[..used], format.width & !1, format.height & !1, stride)
                    } else {
                        nv12_from_yuyv(&buf[..used], format.width & !1, format.height & !1, stride)
                    };
                    let ts = meta.timestamp.sec as f64 + meta.timestamp.usec as f64 * 1e-6;
                    // V4L2 stamps on CLOCK_MONOTONIC; another clock is tied to it here.
                    let offset = *offset.get_or_insert_with(|| {
                        let d = host_seconds() - ts;
                        if d.abs() < 0.5 { 0.0 } else { d }
                    });
                    if let Some(p) = picture {
                        each(p, ts + offset);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut || errors < 10 => errors += 1,
                Err(e) => {
                    on_error(e.to_string());
                    return;
                }
            }
        }
    });
    match result.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(())) => Ok(CameraCapture {
            stop,
            thread: Some(thread),
        }),
        Ok(Err(e)) => Err(e),
        Err(_) => {
            Err("The camera didn't start. Close other apps that use it, then try again.".into())
        }
    }
}

fn camera_path(id: &str) -> Result<String, String> {
    if !id.is_empty() {
        return Ok(id.to_string());
    }
    video_nodes()
        .into_iter()
        .next()
        .map(|(p, _)| p)
        .ok_or_else(|| "No camera was found. Connect one, or record without the camera.".into())
}

fn start_camera(core: Arc<Core>, camera_id: &str) -> Result<CameraCapture, String> {
    let (c, f) = (core.clone(), core);
    run_camera(
        camera_path(camera_id)?,
        move |p, t| c.camera(Arc::new(p), t),
        move |e| {
            f.fail(&format!(
                "The camera stopped ({e}). The recording so far is kept. Check that it's still connected, then record again."
            ))
        },
    )
}

pub fn preview_start(
    camera_id: &str,
    width: u32,
    height: u32,
    synthetic: bool,
) -> Result<(), String> {
    if synthetic {
        return crate::preview::start_synthetic(width, height);
    }
    let path = camera_path(camera_id)?;
    crate::preview::start(move |publisher, stop, opened| {
        let mut last = 0.0;
        let capture = run_camera(
            path,
            move |p, t| {
                if t - last >= crate::preview::INTERVAL.as_secs_f64() {
                    last = t;
                    publisher.publish(crate::preview::jpeg(&p, width, height));
                }
            },
            |_| {},
        );
        match capture {
            Ok(c) => {
                let _ = opened.send(Ok(()));
                while !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Box::new(c).stop();
            }
            Err(e) => {
                let _ = opened.send(Err(e));
            }
        }
    })
}

pub fn preview_frame(seq: &mut u64) -> Option<Vec<u8>> {
    crate::preview::frame(seq)
}

pub fn preview_stop() {
    crate::preview::stop()
}

// --- Sound -----------------------------------------------------------------------------------

const MIC_DENIED: &str =
    "Check that it's connected and not muted in the sound settings, then try again.";

pub fn mics() -> Vec<Mic> {
    crate::audio::mics()
}

pub fn system_audio_supported() -> bool {
    crate::audio::system_audio_supported()
}

pub fn monitor_start(mic_id: &str) -> Result<(), String> {
    crate::audio::monitor_start(mic_id, MIC_DENIED)
}

pub fn monitor_peak() -> f32 {
    crate::audio::monitor_peak()
}

pub fn monitor_stop() {
    crate::audio::monitor_stop()
}

// --- A recording ---------------------------------------------------------------------------

/// Starts the files and the sources a recording needs. With the portal, its dialog comes
/// first (the screen's size is known only after it).
pub fn start(config: &RecordConfig) -> Result<(Arc<Core>, Vec<Box<dyn Source>>), String> {
    let mut inputs = Inputs::default();
    let mut sources: Vec<Box<dyn Source>> = Vec::new();
    let cell: CoreCell = Arc::new(OnceLock::new());
    let mut x11_area = None;
    if let Some(s) = &config.screen {
        let window = s.window.is_some();
        let (w, h) = if use_x11grab() && !window {
            let (w, h) = x11_size().ok_or_else(|| NO_PORTAL.to_string())?;
            let area = match s.region {
                Some(r) => region_rect(&r, (w as f64, h as f64), w, h),
                None => Rect {
                    x: 0,
                    y: 0,
                    width: w & !1,
                    height: h & !1,
                },
            };
            x11_area = Some(area);
            (area.width, area.height)
        } else {
            if portal_sources().is_none() {
                return Err(NO_PORTAL.into());
            }
            let (capture, (w, h)) = start_portal(window, cell.clone())?;
            sources.push(Box::new(capture));
            // A region of the chosen display, in its pixels.
            if let (Some(r), false) = (s.region, window) {
                let display = displays().into_iter().next();
                let points =
                    display.map_or((w as f64, h as f64), |d| (d.points_width, d.points_height));
                inputs.crop = Some(region_rect(&r, points, w, h));
            }
            (w, h)
        };
        inputs.screen = Some(VideoInput {
            width: w,
            height: h,
            format: PixelFormat::Bgra,
        });
    }
    if let Some(c) = &config.camera {
        inputs.camera = Some(VideoInput {
            width: c.width,
            height: c.height,
            format: PixelFormat::Nv12,
        });
    }
    let core = match Core::new(config, inputs) {
        Ok(c) => c,
        Err(e) => {
            for s in sources {
                s.stop();
            }
            return Err(e);
        }
    };
    let _ = cell.set(core.clone());
    let started = (|| -> Result<(), String> {
        if let Some(area) = x11_area {
            sources.push(Box::new(start_x11grab(core.clone(), area)?));
        }
        if let Some(mic) = &config.mic {
            sources.push(Box::new(crate::audio::start_mic(
                core.clone(),
                mic,
                MIC_DENIED,
            )?));
        }
        if let Some(c) = &config.camera {
            sources.push(Box::new(start_camera(core.clone(), &c.id)?));
        }
        if config.system_audio {
            sources.push(Box::new(crate::audio::start_system(core.clone())?));
        }
        Ok(())
    })();
    if let Err(e) = started {
        core.begin_stop();
        for s in sources {
            s.stop();
        }
        let _ = core.finish();
        return Err(e);
    }
    Ok((core, sources))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::tests::{alone, temp_dir};

    #[test]
    fn converts_camera_pictures() {
        // 4×2 YUYV, rows of 8 bytes plus 2 of padding.
        let row = |y0: u8| vec![y0, 1, y0 + 1, 2, y0 + 2, 3, y0 + 3, 4, 0, 0];
        let src: Vec<u8> = [row(10), row(20)].concat();
        let p = nv12_from_yuyv(&src, 4, 2, 10).unwrap();
        assert_eq!(p.data, vec![10, 11, 12, 13, 20, 21, 22, 23, 1, 2, 3, 4]);
        let src: Vec<u8> = (0..18).collect();
        assert_eq!(
            nv12_packed(&src, 4, 2, 6).unwrap().data,
            vec![0, 1, 2, 3, 6, 7, 8, 9, 12, 13, 14, 15]
        );
        let rgbx = [1, 2, 3, 0, 4, 5, 6, 0];
        assert_eq!(
            bgra_from(&rgbx, 2, 1, 8, true).unwrap().data,
            vec![3, 2, 1, 255, 6, 5, 4, 255]
        );
    }

    #[test]
    fn x11grab_is_lgpl() {
        let args = x11grab_args(
            ":0",
            Rect {
                x: 10,
                y: 20,
                width: 640,
                height: 360,
            },
        );
        assert_eq!(crate::ffmpeg::gpl_only(&args), None);
        assert!(args.contains(&":0+10,20".to_string()));
    }

    /// CI runs the tests under Xvfb (no portal): the real X11 path through x11grab.
    #[test]
    fn records_the_x11_screen() {
        if !use_x11grab() || x11_size().is_none() {
            eprintln!("no X11 display without a portal here; skipped");
            return;
        }
        let _timing = alone();
        let dir = temp_dir("linux-x11");
        let config = RecordConfig {
            dir: dir.clone(),
            screen: Some(ScreenConfig {
                display_id: 0,
                region: Some(Region {
                    x: 8.0,
                    y: 8.0,
                    width: 640.0,
                    height: 360.0,
                }),
                window: None,
                width: 640,
                height: 360,
                fps: 30,
                capture_fps: 60,
                api: ScreenApi::Auto,
                bitrate: 2_000_000,
            }),
            mic: None,
            camera: None,
            system_audio: false,
            synthetic: None,
        };
        let rec = crate::native::Recorder::start(&config).unwrap();
        std::thread::sleep(Duration::from_secs(3));
        let (status, error) = rec.stop();
        assert_eq!(error, None, "{status:?}");
        assert!(status.frames >= 75 && status.frames_new >= 60, "{status:?}");
        assert_eq!(status.frames_dropped, 0, "{status:?}");
        let info = crate::probe::probe(&dir.join("screen.mp4")).unwrap();
        let v = info.video().unwrap();
        assert_eq!((v.width, v.height), (Some(640), Some(360)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

//! Windows capture (docs/capture-research.md): Windows.Graphics.Capture for displays,
//! regions and windows (through `windows-capture`, MIT), DXGI desktop duplication for
//! displays where it is missing; cameras through Media Foundation's source reader (NV12,
//! 30 fps); microphones and the computer sound through cpal's WASAPI (audio.rs).
//!
//! Every picture and sound is stamped on the performance counter, which
//! [`host_seconds`](crate::pipeline::host_seconds) reads: Windows.Graphics.Capture's
//! SystemRelativeTime and WASAPI's positions are on it; a camera's timestamps are tied to it
//! at its first picture.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread::JoinHandle;
use std::time::Duration;

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, HMONITOR, MONITORINFO};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW,
};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, WDA_NONE,
};
use windows::core::{HSTRING, PWSTR};
use windows_capture::capture::{Context, GraphicsCaptureApiHandler};
use windows_capture::dxgi_duplication_api::{DxgiDuplicationApi, Error as DxgiError};
use windows_capture::frame::Frame;
use windows_capture::graphics_capture_api::{GraphicsCaptureApi, InternalCaptureControl};
use windows_capture::monitor::Monitor;
use windows_capture::settings::{
    ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
    MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
};

use super::*;
use crate::encoder::{Picture, PixelFormat, Rect, VideoInput};
use crate::pipeline::{Core, Inputs, Source, host_seconds, region_rect};

const CAMERA_DENIED: &str = "If Windows blocks it: open Settings → Privacy & security → Camera, turn on \"Let desktop apps access your camera\", then try again.";
const MIC_DENIED: &str = "If Windows blocks it: open Settings → Privacy & security → Microphone, turn on \"Let desktop apps access your microphone\", then try again.";

fn com() {
    // Each thread that uses COM; an apartment set up before is fine as it is.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
}

fn media_foundation() -> Result<(), String> {
    static STARTED: OnceLock<Result<(), String>> = OnceLock::new();
    STARTED
        .get_or_init(|| {
            com();
            unsafe { MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET) }.map_err(|e| e.message())
        })
        .clone()
}

// --- Permissions ------------------------------------------------------------------------

/// A text value from the registry.
fn registry_text(root: HKEY, key: &str, value: &str) -> Option<String> {
    let mut buffer = [0u16; 64];
    let mut size = (buffer.len() * 2) as u32;
    unsafe {
        RegGetValueW(
            root,
            &HSTRING::from(key),
            &HSTRING::from(value),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut size),
        )
    }
    .ok()
    .ok()?;
    let len = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
    Some(String::from_utf16_lossy(&buffer[..len]))
}

/// Windows' privacy switch for `capability` ("webcam", "microphone"): denied when the
/// switch for all apps or for desktop apps is off.
fn consent(capability: &str) -> Access {
    let base = format!(
        "Software\\Microsoft\\Windows\\CurrentVersion\\CapabilityAccessManager\\ConsentStore\\{capability}"
    );
    for root in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
        for key in [base.clone(), format!("{base}\\NonPackaged")] {
            if registry_text(root, &key, "Value").as_deref() == Some("Deny") {
                return Access::Denied;
            }
        }
    }
    Access::Allowed
}

/// Windows doesn't ask desktop apps before they record the screen.
pub fn screen_access() -> Access {
    Access::Allowed
}
pub fn request_screen_access() {}
pub fn mic_access() -> Access {
    consent("microphone")
}
/// Windows has no dialog for desktop apps: the privacy switch decides.
pub fn request_mic_access() -> Access {
    mic_access()
}
pub fn camera_access() -> Access {
    consent("webcam")
}
pub fn request_camera_access() -> Access {
    camera_access()
}

// --- Displays and windows ----------------------------------------------------------------

fn monitors() -> Vec<Monitor> {
    Monitor::enumerate().unwrap_or_default()
}

fn primary() -> Option<Monitor> {
    Monitor::primary().ok()
}

/// A monitor's rectangle in pixels and its scale (pixels per point).
fn monitor_geometry(m: &Monitor) -> (RECT, f64) {
    let handle = HMONITOR(m.as_raw_hmonitor());
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let _ = unsafe { GetMonitorInfoW(handle, &mut info) };
    let (mut dx, mut dy) = (96u32, 96u32);
    let _ = unsafe { GetDpiForMonitor(handle, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) };
    (info.rcMonitor, dx.max(96) as f64 / 96.0)
}

/// `id`: its place in the list from 1; 0: the main display.
fn monitor(id: u32) -> Option<Monitor> {
    if id == 0 {
        primary()
    } else {
        Monitor::from_index(id as usize).ok()
    }
}

pub fn displays() -> Vec<Display> {
    let main = primary().map(|m| m.as_raw_hmonitor());
    monitors()
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let (rect, scale) = monitor_geometry(m);
            let (w, h) = (
                m.width().unwrap_or((rect.right - rect.left) as u32),
                m.height().unwrap_or((rect.bottom - rect.top) as u32),
            );
            Display {
                id: i as u32 + 1,
                name: m.name().unwrap_or_else(|_| format!("Display {}", i + 1)),
                width: w,
                height: h,
                main: Some(m.as_raw_hmonitor()) == main,
                x: rect.left as f64 / scale,
                y: rect.top as f64 / scale,
                points_width: w as f64 / scale,
                points_height: h as f64 / scale,
            }
        })
        .collect()
}

fn hwnd(id: u32) -> HWND {
    // Window handles fit in 32 bits, also in 64-bit processes.
    HWND(id as usize as *mut c_void)
}

/// What a window looks like on screen (without the invisible resize border), in pixels.
fn window_bounds(window: HWND) -> Option<RECT> {
    let mut rect = RECT::default();
    unsafe {
        DwmGetWindowAttribute(
            window,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&mut rect as *mut RECT).cast(),
            std::mem::size_of::<RECT>() as u32,
        )
    }
    .ok()?;
    Some(rect)
}

pub fn window_capture_supported() -> bool {
    GraphicsCaptureApi::is_supported().unwrap_or(false)
}

pub fn windows() -> Vec<Window> {
    if !window_capture_supported() {
        return Vec::new();
    }
    let own = unsafe { GetCurrentProcessId() };
    windows_capture::window::Window::enumerate()
        .unwrap_or_default()
        .into_iter()
        .filter(|w| w.process_id().ok() != Some(own))
        .filter_map(|w| {
            let handle = HWND(w.as_raw_hwnd());
            let title = w.title().ok().filter(|t| !t.trim().is_empty())?;
            let rect = window_bounds(handle)?;
            let scale = unsafe { GetDpiForWindow(handle) }.max(96) as f64 / 96.0;
            let (width, height) = (
                (rect.right - rect.left) as f64 / scale,
                (rect.bottom - rect.top) as f64 / scale,
            );
            if width < 64.0 || height < 64.0 {
                return None;
            }
            let app = w
                .process_name()
                .map(|n| n.trim_end_matches(".exe").to_string())
                .unwrap_or_default();
            Some(Window {
                id: handle.0 as usize as u32,
                app,
                title,
                width,
                height,
                scale,
            })
        })
        .collect()
}

// --- Screen capture -----------------------------------------------------------------------

struct Flags {
    core: Arc<Core>,
    /// A region of the display, in its pixels (copied off the GPU alone).
    crop: Option<Rect>,
}

struct Handler {
    core: Arc<Core>,
    crop: Option<Rect>,
    buffer: Vec<u8>,
}

impl GraphicsCaptureApiHandler for Handler {
    type Flags = Flags;
    type Error = String;

    fn new(ctx: Context<Flags>) -> Result<Self, String> {
        Ok(Handler {
            core: ctx.flags.core,
            crop: ctx.flags.crop,
            buffer: Vec::new(),
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        _control: InternalCaptureControl,
    ) -> Result<(), String> {
        // SystemRelativeTime: the performance counter, in 100 ns.
        let t = frame.timestamp().map_err(|e| e.message())?.Duration as f64 * 1e-7;
        let (fw, fh) = (frame.width(), frame.height());
        let buffer = match self.crop {
            Some(r) => frame.buffer_crop(
                r.x.min(fw.saturating_sub(2)),
                r.y.min(fh.saturating_sub(2)),
                (r.x + r.width).min(fw),
                (r.y + r.height).min(fh),
            ),
            None => frame.buffer(),
        };
        let Ok(buffer) = buffer else {
            return Ok(()); // the next picture, then
        };
        let (width, height) = (buffer.width(), buffer.height());
        let data = buffer.as_nopadding_buffer(&mut self.buffer).to_vec();
        self.core.screen(
            Arc::new(Picture {
                width,
                height,
                format: PixelFormat::Bgra,
                data,
            }),
            t,
        );
        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), String> {
        // The recorded window closed: the recording keeps its last picture.
        self.core.window_closed();
        Ok(())
    }
}

/// A running Windows.Graphics.Capture session.
struct Wgc(Option<windows_capture::capture::CaptureControl<Handler, String>>);

impl Source for Wgc {
    fn stop(mut self: Box<Self>) {
        if let Some(c) = self.0.take() {
            let _ = c.stop();
        }
    }
}

fn start_wgc<T>(item: T, core: Arc<Core>, crop: Option<Rect>) -> Result<Wgc, String>
where
    T: TryInto<windows_capture::settings::GraphicsCaptureItemType> + Clone + Send + 'static,
{
    // Up to 60 updates a second (only changes are sent); older Windows refuse a custom
    // interval (Server 2022, measured in M1): then the system's default.
    let mut last = String::new();
    for interval in [
        MinimumUpdateIntervalSettings::Custom(Duration::from_millis(16)),
        MinimumUpdateIntervalSettings::Default,
    ] {
        let settings = Settings::new(
            item.clone(),
            CursorCaptureSettings::Default,
            DrawBorderSettings::Default,
            SecondaryWindowSettings::Default,
            interval,
            DirtyRegionSettings::Default,
            ColorFormat::Bgra8,
            Flags {
                core: core.clone(),
                crop,
            },
        );
        match Handler::start_free_threaded(settings) {
            Ok(control) => return Ok(Wgc(Some(control))),
            Err(e) => last = e.to_string(),
        }
    }
    Err(last)
}

/// DXGI desktop duplication, where Windows.Graphics.Capture is missing (whole displays).
struct Dxgi {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Source for Dxgi {
    fn stop(mut self: Box<Self>) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn qpc_frequency() -> f64 {
    unsafe extern "system" {
        fn QueryPerformanceFrequency(frequency: *mut i64) -> i32;
    }
    let mut f = 1i64;
    unsafe { QueryPerformanceFrequency(&mut f) };
    f as f64
}

fn start_dxgi(m: Monitor, core: Arc<Core>, crop: Option<Rect>) -> Result<Dxgi, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let (opened, result) = std::sync::mpsc::channel();
    let s = stop.clone();
    let thread = std::thread::spawn(move || {
        let mut dup = match DxgiDuplicationApi::new(m) {
            Ok(d) => {
                let _ = opened.send(Ok(()));
                d
            }
            Err(e) => {
                let _ = opened.send(Err(e.to_string()));
                return;
            }
        };
        let frequency = qpc_frequency();
        let mut buffer = Vec::new();
        while !s.load(Ordering::Relaxed) {
            match dup.acquire_next_frame(50) {
                Ok(mut frame) => {
                    // 0 when only the mouse pointer moved.
                    let present = frame.frame_info().LastPresentTime;
                    if present == 0 {
                        continue;
                    }
                    let t = present as f64 / frequency;
                    let mapped = match crop {
                        Some(r) => frame.buffer_crop(r.x, r.y, r.x + r.width, r.y + r.height),
                        None => frame.buffer(),
                    };
                    if let Ok(b) = mapped {
                        let (width, height) = (b.width(), b.height());
                        let data = b.as_nopadding_buffer(&mut buffer).to_vec();
                        core.screen(
                            Arc::new(Picture {
                                width,
                                height,
                                format: PixelFormat::Bgra,
                                data,
                            }),
                            t,
                        );
                    }
                }
                Err(DxgiError::Timeout) => {}
                Err(DxgiError::AccessLost) => match dup.recreate() {
                    Ok(d) => dup = d,
                    Err(_) => {
                        // A secure desktop or a display change: try again shortly.
                        std::thread::sleep(Duration::from_millis(100));
                        match monitor(0).map(DxgiDuplicationApi::new) {
                            Some(Ok(d)) => dup = d,
                            _ => {
                                core.fail("The screen can't be recorded any more (the display changed). The recording so far is kept.");
                                return;
                            }
                        }
                    }
                },
                Err(e) => {
                    core.fail(&format!("The screen recording stopped ({e}). The recording so far is kept; record again."));
                    return;
                }
            }
        }
    });
    match result.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(())) => Ok(Dxgi {
            stop,
            thread: Some(thread),
        }),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("the screen didn't answer".into()),
    }
}

// --- Cameras (Media Foundation) -----------------------------------------------------------

/// A camera as Media Foundation lists it.
struct Device {
    name: String,
    id: String,
    activate: IMFActivate,
}

fn string(a: &IMFActivate, key: &windows::core::GUID) -> String {
    let mut value = PWSTR::null();
    let mut len = 0u32;
    if unsafe { a.GetAllocatedString(key, &mut value, &mut len) }.is_err() {
        return String::new();
    }
    let s = unsafe { value.to_string() }.unwrap_or_default();
    unsafe { CoTaskMemFree(Some(value.0 as *const c_void)) };
    s
}

fn video_devices() -> Vec<Device> {
    if media_foundation().is_err() {
        return Vec::new();
    }
    com();
    let mut attributes = None;
    if unsafe { MFCreateAttributes(&mut attributes, 1) }.is_err() {
        return Vec::new();
    }
    let Some(attributes) = attributes else {
        return Vec::new();
    };
    let _ = unsafe {
        attributes.SetGUID(
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
        )
    };
    let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0u32;
    if unsafe { MFEnumDeviceSources(&attributes, &mut list, &mut count) }.is_err() || list.is_null()
    {
        return Vec::new();
    }
    let mut devices = Vec::new();
    for i in 0..count as usize {
        if let Some(a) = unsafe { (*list.add(i)).take() } {
            devices.push(Device {
                name: string(&a, &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME),
                id: string(&a, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK),
                activate: a,
            });
        }
    }
    unsafe { CoTaskMemFree(Some(list as *const c_void)) };
    devices
}

const STREAM: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;

fn packed(t: &IMFMediaType, key: &windows::core::GUID) -> (u32, u32) {
    let v = unsafe { t.GetUINT64(key) }.unwrap_or(0);
    ((v >> 32) as u32, v as u32)
}

/// The camera's format the recorder uses: the largest up to 720 pixels high at 30 fps (or
/// the largest up to 720 at all), preferring formats that need no decoding.
fn best_format(reader: &IMFSourceReader) -> Option<(IMFMediaType, u32, u32, bool)> {
    let mut best: Option<(IMFMediaType, u32, u32, bool, u32)> = None;
    for i in 0.. {
        let Ok(t) = (unsafe { reader.GetNativeMediaType(STREAM, i) }) else {
            break;
        };
        let (w, h) = packed(&t, &MF_MT_FRAME_SIZE);
        let (num, den) = packed(&t, &MF_MT_FRAME_RATE);
        if h == 0 || h > 720 || den == 0 {
            continue;
        }
        let thirty = num as f64 / den as f64 >= 29.9;
        let subtype = unsafe { t.GetGUID(&MF_MT_SUBTYPE) }.unwrap_or_default();
        let cost = if subtype == MFVideoFormat_NV12 {
            0
        } else if subtype == MFVideoFormat_YUY2 {
            1
        } else {
            2
        };
        let better = match &best {
            None => true,
            Some((_, bw, bh, bt, bc)) => {
                (thirty, w * h, std::cmp::Reverse(cost)) > (*bt, bw * bh, std::cmp::Reverse(*bc))
            }
        };
        if better {
            best = Some((t, w, h, thirty, cost));
        }
    }
    best.map(|(t, w, h, thirty, _)| (t, w & !1, h & !1, thirty))
}

/// An open camera giving NV12 at its best format.
struct CameraReader {
    reader: IMFSourceReader,
    source: IMFMediaSource,
    width: u32,
    height: u32,
}

impl Drop for CameraReader {
    fn drop(&mut self) {
        let _ = unsafe { self.source.Shutdown() };
    }
}

fn open_camera(camera_id: &str) -> Result<CameraReader, String> {
    media_foundation()?;
    com();
    let devices = video_devices();
    let device = devices
        .iter()
        .find(|d| d.id == camera_id)
        .or_else(|| devices.first())
        .ok_or_else(|| {
            "No camera was found. Connect one, or record without the camera.".to_string()
        })?;
    let source: IMFMediaSource = unsafe { device.activate.ActivateObject() }.map_err(|e| {
        format!(
            "The camera can't be opened ({}). Close other apps that use it. {CAMERA_DENIED}",
            e.message()
        )
    })?;
    let mut attributes = None;
    unsafe { MFCreateAttributes(&mut attributes, 2) }.map_err(|e| e.message())?;
    let attributes = attributes.ok_or("no attributes")?;
    unsafe {
        // Converts YUY2 and decoded MJPEG pictures to NV12.
        let _ = attributes.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1);
    }
    let reader =
        unsafe { MFCreateSourceReaderFromMediaSource(&source, &attributes) }.map_err(|e| {
            format!(
                "The camera can't be opened ({}). Close other apps that use it, then try again.",
                e.message()
            )
        })?;
    let (native, width, height, _) = best_format(&reader).ok_or_else(|| {
        "This camera offers no picture size KreoKit Rec can use. Try another camera.".to_string()
    })?;
    let nv12 = (|| -> windows::core::Result<()> {
        unsafe {
            reader.SetCurrentMediaType(STREAM, None, &native)?;
            let out = MFCreateMediaType()?;
            out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            out.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
            out.SetUINT64(&MF_MT_FRAME_SIZE, (width as u64) << 32 | height as u64)?;
            out.SetUINT64(&MF_MT_FRAME_RATE, 30u64 << 32 | 1)?;
            reader.SetCurrentMediaType(STREAM, None, &out)
        }
    })();
    nv12.map_err(|e| {
        format!(
            "The camera's pictures can't be converted ({}). Try another camera.",
            e.message()
        )
    })?;
    Ok(CameraReader {
        reader,
        source,
        width,
        height,
    })
}

impl CameraReader {
    /// The next picture and its timestamp (100 ns, the source's clock); None at a gap.
    fn read(&self) -> Result<Option<(Picture, i64)>, String> {
        let (mut flags, mut ts) = (0u32, 0i64);
        let mut sample = None;
        unsafe {
            self.reader.ReadSample(
                STREAM,
                0,
                None,
                Some(&mut flags),
                Some(&mut ts),
                Some(&mut sample),
            )
        }
        .map_err(|e| e.message())?;
        if flags & MF_SOURCE_READERF_ERROR.0 as u32 != 0
            || flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0
        {
            return Err("the camera stopped".into());
        }
        let Some(sample) = sample else {
            return Ok(None);
        };
        let buffer = unsafe { sample.ConvertToContiguousBuffer() }.map_err(|e| e.message())?;
        let mut ptr = std::ptr::null_mut();
        let mut len = 0u32;
        unsafe { buffer.Lock(&mut ptr, None, Some(&mut len)) }.map_err(|e| e.message())?;
        let bytes = unsafe { std::slice::from_raw_parts(ptr, len as usize) };
        let picture = nv12_from(bytes, self.width, self.height);
        let _ = unsafe { buffer.Unlock() };
        Ok(picture.map(|p| (p, ts)))
    }
}

/// An NV12 picture from a buffer whose rows may be padded (its stride follows from its
/// length).
fn nv12_from(bytes: &[u8], width: u32, height: u32) -> Option<Picture> {
    let (w, h) = (width as usize, height as usize);
    let size = Picture::size(width, height, PixelFormat::Nv12);
    if bytes.len() < size {
        return None;
    }
    let data = if bytes.len() == size {
        bytes.to_vec()
    } else {
        let stride = bytes.len() / (h + h.div_ceil(2));
        if stride < w {
            return None;
        }
        let mut data = Vec::with_capacity(size);
        for row in 0..h + h.div_ceil(2) {
            data.extend_from_slice(&bytes[row * stride..row * stride + w]);
        }
        data
    };
    Some(Picture {
        width,
        height,
        format: PixelFormat::Nv12,
        data,
    })
}

pub fn cameras() -> Vec<Camera> {
    let devices = video_devices();
    devices
        .iter()
        .enumerate()
        .filter_map(|(i, d)| {
            let reader = open_camera(&d.id).ok()?;
            Some(Camera {
                id: d.id.clone(),
                name: d.name.clone(),
                default: i == 0,
                width: reader.width,
                height: reader.height,
            })
        })
        .collect()
}

/// Reads a camera on its own thread until stopped.
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
/// time until `stop`.
fn run_camera(
    camera_id: String,
    each: impl FnMut(Picture, f64) + Send + 'static,
    on_error: impl FnOnce(String) + Send + 'static,
) -> Result<CameraCapture, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let (opened, result) = std::sync::mpsc::channel();
    let s = stop.clone();
    let mut each = each;
    let thread = std::thread::spawn(move || {
        let reader = match open_camera(&camera_id) {
            Ok(r) => {
                let _ = opened.send(Ok(()));
                r
            }
            Err(e) => {
                let _ = opened.send(Err(e));
                return;
            }
        };
        // The camera's clock, tied to the host clock at its first picture.
        let mut offset = None;
        while !s.load(Ordering::Relaxed) {
            match reader.read() {
                Ok(Some((picture, ts))) => {
                    let t = ts as f64 * 1e-7;
                    let offset = *offset.get_or_insert_with(|| host_seconds() - t);
                    each(picture, t + offset);
                }
                Ok(None) => {}
                Err(e) => {
                    on_error(e);
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

fn start_camera(core: Arc<Core>, camera_id: &str) -> Result<CameraCapture, String> {
    let (c, f) = (core.clone(), core);
    run_camera(
        camera_id.to_string(),
        move |p, t| c.camera(Arc::new(p), t),
        move |e| {
            f.fail(&format!(
                "The camera stopped ({e}). The recording so far is kept. Check that no other app took the camera, then record again."
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
    let id = camera_id.to_string();
    crate::preview::start(move |publisher, stop, opened| {
        let mut last = 0.0;
        let capture = run_camera(
            id,
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

pub fn mics() -> Vec<Mic> {
    crate::audio::mics()
}

/// Windows 10 version 2004 (build 19041) and newer keep marked windows out of every
/// capture (`WDA_EXCLUDEFROMCAPTURE`); older builds would show them as black boxes.
pub fn exclusion_supported() -> bool {
    registry_text(
        HKEY_LOCAL_MACHINE,
        "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion",
        "CurrentBuildNumber",
    )
    .and_then(|b| b.trim().parse::<u32>().ok())
    .is_some_and(|build| build >= 19041)
}

/// Marks one of the app's windows to be kept out of screen captures (all apps' captures,
/// screenshots included, so only while recording). False when Windows refused.
pub fn exclude_from_capture(hwnd: isize, on: bool) -> bool {
    let affinity = if on { WDA_EXCLUDEFROMCAPTURE } else { WDA_NONE };
    unsafe { SetWindowDisplayAffinity(HWND(hwnd as *mut c_void), affinity) }.is_ok()
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

/// Starts the files and the sources a recording needs.
pub fn start(config: &RecordConfig) -> Result<(Arc<Core>, Vec<Box<dyn Source>>), String> {
    let mut inputs = Inputs::default();
    let bgra = |width, height| VideoInput {
        width,
        height,
        format: PixelFormat::Bgra,
    };
    // The display (and region, in its pixels), or the window.
    let mut display = None;
    if let Some(s) = &config.screen {
        if s.window.is_some() {
            inputs.screen = Some(bgra(s.width, s.height));
        } else {
            let m = monitor(s.display_id).ok_or_else(|| {
                "That display isn't connected any more. Choose another one.".to_string()
            })?;
            let (rect, scale) = monitor_geometry(&m);
            let (w, h) = (
                m.width().unwrap_or((rect.right - rect.left) as u32),
                m.height().unwrap_or((rect.bottom - rect.top) as u32),
            );
            let crop = s
                .region
                .map(|r| region_rect(&r, (w as f64 / scale, h as f64 / scale), w, h));
            inputs.screen = Some(match crop {
                Some(r) => bgra(r.width, r.height),
                None => bgra(w & !1, h & !1),
            });
            display = Some((m, crop));
        }
    }
    if let Some(c) = &config.camera {
        inputs.camera = Some(VideoInput {
            width: c.width,
            height: c.height,
            format: PixelFormat::Nv12,
        });
    }
    let core = Core::new(config, inputs)?;
    let mut sources: Vec<Box<dyn Source>> = Vec::new();
    let started = (|| -> Result<(), String> {
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
        if let Some(s) = &config.screen {
            let screen_error = |e: String| {
                format!(
                    "The screen can't be recorded ({e}). Try again; if it keeps happening, use Report a problem."
                )
            };
            if let Some(id) = s.window {
                let window = windows_capture::window::Window::from_raw_hwnd(hwnd(id).0);
                if !window.is_valid() {
                    return Err("That window isn't open any more. Choose another one.".into());
                }
                let wgc = start_wgc(window, core.clone(), None).map_err(|e| {
                    format!("That window can't be recorded ({e}). Choose another window, or record the display.")
                })?;
                sources.push(Box::new(wgc));
            } else if let Some((m, crop)) = display {
                if window_capture_supported() {
                    match start_wgc(m, core.clone(), crop) {
                        Ok(wgc) => sources.push(Box::new(wgc)),
                        // DXGI duplication records the same display.
                        Err(_) => sources.push(Box::new(
                            start_dxgi(m, core.clone(), crop).map_err(screen_error)?,
                        )),
                    }
                } else {
                    sources.push(Box::new(
                        start_dxgi(m, core.clone(), crop).map_err(screen_error)?,
                    ));
                }
            }
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

    /// The real display (CI's Windows VM has one, ~32 Hz; docs/capture-research.md).
    fn screen(dir: std::path::PathBuf, region: Option<Region>, w: u32, h: u32) -> RecordConfig {
        RecordConfig {
            dir,
            screen: Some(ScreenConfig {
                display_id: 0,
                region,
                window: None,
                exclude_windows: Vec::new(),
                width: w,
                height: h,
                fps: 30,
                capture_fps: 60,
                api: ScreenApi::Auto,
                bitrate: 2_000_000,
            }),
            mic: None,
            camera: None,
            system_audio: false,
            synthetic: None,
        }
    }

    fn records(config: &RecordConfig, w: u32, h: u32) {
        let rec = crate::native::Recorder::start(config).unwrap();
        std::thread::sleep(Duration::from_secs(3));
        let (status, error) = rec.stop();
        assert_eq!(error, None, "{status:?}");
        // Only changes are sent: at least the first picture, then repeats.
        assert!(status.frames >= 75 && status.frames_new >= 1, "{status:?}");
        assert_eq!(status.frames_dropped, 0, "{status:?}");
        let info = crate::probe::probe(&config.dir.join("screen.mp4")).unwrap();
        let v = info.video().unwrap();
        assert_eq!((v.width, v.height), (Some(w), Some(h)));
        assert!(
            (info.duration_s - status.frames as f64 / 30.0).abs() < 0.1,
            "{info:?}"
        );
    }

    #[test]
    fn records_the_real_display() {
        let _timing = alone();
        let dir = temp_dir("win-display");
        records(&screen(dir.clone(), None, 640, 360), 640, 360);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn records_a_region_of_the_real_display() {
        let _timing = alone();
        let dir = temp_dir("win-region");
        let region = Region {
            x: 10.0,
            y: 20.0,
            width: 320.0,
            height: 180.0,
        };
        records(&screen(dir.clone(), Some(region), 320, 180), 320, 180);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn duplicates_the_display_where_windows_graphics_capture_is_missing() {
        let _timing = alone();
        let dir = temp_dir("win-dxgi");
        let config = screen(dir.clone(), None, 640, 360);
        let m = monitor(0).unwrap();
        let (w, h) = (m.width().unwrap(), m.height().unwrap());
        let inputs = Inputs {
            screen: Some(VideoInput {
                width: w & !1,
                height: h & !1,
                format: PixelFormat::Bgra,
            }),
            ..Inputs::default()
        };
        let core = Core::new(&config, inputs).unwrap();
        let dxgi = start_dxgi(m, core.clone(), None).unwrap();
        let rec = crate::pipeline::Recorder::new(core, vec![Box::new(dxgi)], true);
        std::thread::sleep(Duration::from_secs(3));
        let (status, error) = rec.stop();
        assert_eq!(error, None, "{status:?}");
        assert!(status.frames >= 75, "{status:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lists_the_windows_with_sizes() {
        for w in windows() {
            assert!(
                w.width >= 64.0 && w.height >= 64.0 && w.scale >= 1.0,
                "{w:?}"
            );
            assert!(!w.title.is_empty());
        }
    }

    #[test]
    fn repacks_padded_camera_rows() {
        // 4×2 NV12 with rows padded to 6 bytes.
        let bytes: Vec<u8> = (0..18).collect();
        let p = nv12_from(&bytes, 4, 2).unwrap();
        assert_eq!(p.data, vec![0, 1, 2, 3, 6, 7, 8, 9, 12, 13, 14, 15]);
        assert_eq!(nv12_from(&bytes[..10], 4, 2), None);
    }
}

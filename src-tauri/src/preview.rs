//! The camera preview on the setup screen (Windows and Linux; macOS draws its own in
//! recorder.m): small JPEG pictures about 15 times a second, without recording.

#![cfg_attr(
    all(target_os = "macos", not(feature = "pipe-recorder")),
    allow(dead_code)
)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::encoder::{Picture, PixelFormat};

/// About 15 pictures a second.
pub const INTERVAL: Duration = Duration::from_millis(66);

/// Where a running preview puts its newest picture.
#[derive(Clone, Default)]
pub struct Publisher(Arc<Mutex<(u64, Vec<u8>)>>);

impl Publisher {
    pub fn publish(&self, jpeg: Vec<u8>) {
        let mut latest = self.0.lock().unwrap();
        latest.0 += 1;
        latest.1 = jpeg;
    }
}

struct Running {
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
    latest: Publisher,
}

static PREVIEW: Mutex<Option<Running>> = Mutex::new(None);

/// Stops any preview and runs `run` on its own thread until `stop` is set; `run` publishes
/// pictures. An error from `opened` (sent once the source is open) is returned here.
pub fn start(
    run: impl FnOnce(Publisher, Arc<AtomicBool>, std::sync::mpsc::Sender<Result<(), String>>)
    + Send
    + 'static,
) -> Result<(), String> {
    stop();
    let stop = Arc::new(AtomicBool::new(false));
    let latest = Publisher::default();
    let (opened, result) = std::sync::mpsc::channel();
    let (s, l) = (stop.clone(), latest.clone());
    let thread = std::thread::spawn(move || run(l, s, opened));
    match result.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(())) => {
            *PREVIEW.lock().unwrap() = Some(Running {
                stop,
                thread,
                latest,
            });
            Ok(())
        }
        Ok(Err(e)) => {
            stop.store(true, Ordering::Relaxed);
            let _ = thread.join();
            Err(e)
        }
        Err(_) => {
            stop.store(true, Ordering::Relaxed);
            Err("The camera didn't start. Close other apps that use it, then try again.".into())
        }
    }
}

/// The newest picture if it is newer than `seq` (which is then updated).
pub fn frame(seq: &mut u64) -> Option<Vec<u8>> {
    let preview = PREVIEW.lock().unwrap();
    let latest = preview.as_ref()?.latest.0.lock().unwrap();
    if latest.0 <= *seq || latest.1.is_empty() {
        return None;
    }
    *seq = latest.0;
    Some(latest.1.clone())
}

pub fn stop() {
    let running = PREVIEW.lock().unwrap().take();
    if let Some(r) = running {
        r.stop.store(true, Ordering::Relaxed);
        let _ = r.thread.join();
    }
}

/// The made-up camera: the test pattern, counting up.
pub fn start_synthetic(width: u32, height: u32) -> Result<(), String> {
    start(move |publisher, stop, opened| {
        let _ = opened.send(Ok(()));
        let mut n = 0;
        while !stop.load(Ordering::Relaxed) {
            let mut p = Picture::black(width & !1, height & !1, PixelFormat::Nv12);
            crate::synthetic::draw(&mut p, n, n % 15 == 0, None);
            publisher.publish(jpeg(&p, width, height));
            n += 1;
            std::thread::sleep(INTERVAL);
        }
    })
}

/// A picture as a JPEG `width` × `height` (nearest pixel; it's a small preview).
pub fn jpeg(p: &Picture, width: u32, height: u32) -> Vec<u8> {
    let (w, h) = (width.max(1) as usize, height.max(1) as usize);
    let (pw, ph) = (p.width as usize, p.height as usize);
    let mut rgb = Vec::with_capacity(w * h * 3);
    for oy in 0..h {
        let y = oy * ph / h;
        for ox in 0..w {
            let x = ox * pw / w;
            let (r, g, b) = match p.format {
                PixelFormat::Bgra => {
                    let i = (y * pw + x) * 4;
                    (p.data[i + 2], p.data[i + 1], p.data[i])
                }
                PixelFormat::Nv12 => {
                    let luma = p.data[y * pw + x] as f32;
                    let c = pw * ph + (y / 2) * pw + (x & !1);
                    let (u, v) = (p.data[c] as f32 - 128.0, p.data[c + 1] as f32 - 128.0);
                    // BT.709, video range.
                    let l = (luma - 16.0) * 1.164;
                    let clamp = |v: f32| v.round().clamp(0.0, 255.0) as u8;
                    (
                        clamp(l + 1.793 * v),
                        clamp(l - 0.213 * u - 0.533 * v),
                        clamp(l + 2.112 * u),
                    )
                }
            };
            rgb.extend([r, g, b]);
        }
    }
    crate::jpeg::encode(&rgb, w as u32, h as u32, 75)
}

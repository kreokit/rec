//! Sound capture on Windows and Linux through cpal (Apache-2.0): microphones, and the
//! computer sound (WASAPI loopback on Windows: an output device opened for input; on Linux
//! PipeWire's capture of the default output, docs/capture-research.md).
//!
//! Whatever the device sends (sample format, rate, channels) becomes 48 kHz float, mono for
//! a microphone and stereo for the computer sound, stamped on the host clock: WASAPI stamps
//! with the performance counter and PipeWire with CLOCK_MONOTONIC, both what
//! [`host_seconds`] reads; another clock is tied to the host clock at the first buffer.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Data, Sample, SampleFormat};

use crate::encoder::RATE;
use crate::native::Mic;
use crate::pipeline::{Core, Source, host_seconds};

/// The host cpal uses: PipeWire first on Linux (the PulseAudio protocol fails without a
/// cookie on PipeWire-only systems), then the system's default (WASAPI, ALSA).
fn host() -> cpal::Host {
    #[cfg(target_os = "linux")]
    if let Ok(host) = cpal::host_from_id(cpal::HostId::PipeWire) {
        return host;
    }
    cpal::default_host()
}

fn name(device: &cpal::Device) -> String {
    device
        .description()
        .map(|d| d.name().to_string())
        .unwrap_or_else(|_| "Microphone".into())
}

fn id(device: &cpal::Device) -> String {
    device.id().map(|i| i.to_string()).unwrap_or_default()
}

/// The microphones (and other inputs) the system lists.
pub fn mics() -> Vec<Mic> {
    let host = host();
    let default = host.default_input_device().map(|d| id(&d));
    let Ok(devices) = host.input_devices() else {
        return Vec::new();
    };
    devices
        .filter(|d| d.default_input_config().is_ok())
        .map(|d| {
            let id = id(&d);
            Mic {
                default: default.as_deref() == Some(id.as_str()),
                name: name(&d),
                id,
            }
        })
        // Monitors of outputs are the computer sound, not microphones (PipeWire, PulseAudio).
        .filter(|m| !m.name.to_lowercase().starts_with("monitor of"))
        .collect()
}

fn mic_device(mic_id: &str) -> Option<cpal::Device> {
    let host = host();
    if mic_id.is_empty() {
        return host.default_input_device();
    }
    let wanted: cpal::DeviceId = mic_id.parse().ok()?;
    host.device_by_id(&wanted)
        .or_else(|| host.default_input_device())
}

/// The computer sound can be recorded: an output device to capture.
pub fn system_audio_supported() -> bool {
    host().default_output_device().is_some()
}

/// Streaming linear resampler from the device's rate to 48 kHz, `channels` interleaved.
struct Resampler {
    /// Input frames per output frame.
    step: f64,
    /// Where the next output frame falls, in input frames from the last one kept.
    pos: f64,
    last: Vec<f32>,
}

impl Resampler {
    fn new(rate: u32, channels: usize) -> Resampler {
        Resampler {
            step: rate as f64 / RATE as f64,
            pos: 1.0,
            last: vec![0.0; channels],
        }
    }

    /// Resamples `input`; returns the output and how far (in input frames) its first frame
    /// is after the first input frame.
    fn run(&mut self, input: &[f32], out: &mut Vec<f32>) -> f64 {
        let c = self.last.len();
        let n = input.len() / c;
        out.clear();
        if (self.step - 1.0).abs() < 1e-9 {
            out.extend_from_slice(input);
            return 0.0;
        }
        let first = self.pos - 1.0;
        // Frame k of [last, input...]: k = 0 is the last frame kept.
        let at = |k: usize, ch: usize| {
            if k == 0 {
                self.last[ch]
            } else {
                input[(k - 1) * c + ch]
            }
        };
        while self.pos <= n as f64 {
            let k = self.pos.floor() as usize;
            let frac = (self.pos - k as f64) as f32;
            for ch in 0..c {
                let a = at(k, ch);
                let b = if k < n { at(k + 1, ch) } else { a };
                out.push(a + (b - a) * frac);
            }
            self.pos += self.step;
        }
        self.pos -= n as f64;
        if n > 0 {
            self.last.copy_from_slice(&input[(n - 1) * c..n * c]);
        }
        first
    }
}

/// Float samples of any format cpal delivers.
fn floats(data: &Data, out: &mut Vec<f32>) -> bool {
    out.clear();
    macro_rules! take {
        ($t:ty) => {
            match data.as_slice::<$t>() {
                Some(s) => out.extend(s.iter().map(|v| v.to_sample::<f32>())),
                None => return false,
            }
        };
    }
    match data.sample_format() {
        SampleFormat::F32 => take!(f32),
        SampleFormat::F64 => take!(f64),
        SampleFormat::I16 => take!(i16),
        SampleFormat::I32 => take!(i32),
        SampleFormat::U8 => take!(u8),
        SampleFormat::U16 => take!(u16),
        SampleFormat::I8 => take!(i8),
        _ => return false,
    }
    true
}

/// Mixes interleaved `from` channels into `to` channels (1: the average; 2: mono doubled, or
/// the first two of more).
fn remix(input: &[f32], from: usize, to: usize, out: &mut Vec<f32>) {
    out.clear();
    for frame in input.chunks_exact(from) {
        match (from, to) {
            (f, t) if f == t => out.extend_from_slice(frame),
            (_, 1) => out.push(frame.iter().sum::<f32>() / from as f32),
            (1, _) => out.extend([frame[0], frame[0]]),
            _ => out.extend([frame[0], frame[1]]),
        }
    }
}

/// Where a capture sends its 48 kHz samples, with their host time.
pub type Sink = Box<dyn FnMut(&[f32], f64) + Send>;

/// A running sound capture; its stream lives on its own thread (cpal streams aren't Send
/// everywhere).
pub struct Capture {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Capture {
    fn stop_now(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Source for Capture {
    fn stop(mut self: Box<Self>) {
        self.stop_now();
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop_now();
    }
}

/// The config to open `device` with: 48 kHz float where offered, else the default one.
fn config(device: &cpal::Device, loopback: bool) -> Result<cpal::SupportedStreamConfig, String> {
    let default = if loopback {
        device.default_output_config()
    } else {
        device.default_input_config()
    }
    .map_err(|e| e.to_string())?;
    if loopback || default.sample_rate() == RATE as u32 {
        return Ok(default);
    }
    let ranges = device
        .supported_input_configs()
        .map_err(|e| e.to_string())?;
    let best = ranges
        .filter(|r| r.channels() == default.channels())
        .filter(|r| (r.min_sample_rate()..=r.max_sample_rate()).contains(&(RATE as u32)))
        .max_by_key(|r| r.sample_format() == SampleFormat::F32);
    Ok(best.map_or(default, |r| r.with_sample_rate(RATE as u32)))
}

/// Opens `device` (`loopback`: the computer sound) and sends `channels` × 48 kHz to `sink`.
fn open(
    device: cpal::Device,
    loopback: bool,
    channels: usize,
    mut sink: Sink,
) -> Result<Capture, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let (opened, result) = std::sync::mpsc::channel::<Result<(), String>>();
    let s = stop.clone();
    let thread = std::thread::Builder::new()
        .name("kreokit-rec-sound".into())
        .spawn(move || {
            let supported = match config(&device, loopback) {
                Ok(c) => c,
                Err(e) => {
                    let _ = opened.send(Err(e));
                    return;
                }
            };
            let from = supported.channels() as usize;
            let rate = supported.sample_rate();
            let mut resampler = Resampler::new(rate, channels);
            let (mut raw, mut mixed, mut out) = (Vec::new(), Vec::new(), Vec::new());
            // Seconds to add to the stream's clock to get host time (0: it is the host clock).
            let mut offset: Option<f64> = None;
            let failed = Arc::new(Mutex::new(None::<String>));
            let f = failed.clone();
            let stream = device.build_input_stream_raw(
                supported.config(),
                supported.sample_format(),
                move |data: &Data, info: &cpal::InputCallbackInfo| {
                    if !floats(data, &mut raw) || from == 0 {
                        return;
                    }
                    let ts = info.timestamp();
                    let capture = ts.capture.as_nanos() as f64 * 1e-9;
                    let callback = ts.callback.as_nanos() as f64 * 1e-9;
                    let offset = *offset.get_or_insert_with(|| {
                        let d = host_seconds() - callback;
                        if d.abs() < 0.5 { 0.0 } else { d }
                    });
                    remix(&raw, from, channels, &mut mixed);
                    let lead = resampler.run(&mixed, &mut out);
                    if !out.is_empty() {
                        sink(&out, capture + offset + lead / rate as f64);
                    }
                },
                move |e| {
                    let mut f = f.lock().unwrap();
                    if f.is_none() {
                        *f = Some(e.to_string());
                    }
                },
                Some(Duration::from_secs(5)),
            );
            let stream = match stream.map_err(|e| e.to_string()).and_then(|st| {
                st.play().map_err(|e| e.to_string())?;
                Ok(st)
            }) {
                Ok(st) => st,
                Err(e) => {
                    let _ = opened.send(Err(e));
                    return;
                }
            };
            let _ = opened.send(Ok(()));
            while !s.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(20));
            }
            drop(stream);
        })
        .map_err(|e| e.to_string())?;
    let mut capture = Capture {
        stop,
        thread: Some(thread),
    };
    match result.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(())) => Ok(capture),
        Ok(Err(e)) => {
            capture.stop_now();
            Err(e)
        }
        Err(_) => {
            capture.stop_now();
            Err("it didn't answer".into())
        }
    }
}

/// Records the microphone `mic_id` ("": the default) into the recording.
pub fn start_mic(core: Arc<Core>, mic_id: &str, denied: &str) -> Result<Capture, String> {
    let device = mic_device(mic_id).ok_or_else(|| {
        "No microphone was found. Connect one, or record without the microphone.".to_string()
    })?;
    open(device, false, 1, Box::new(move |s, t| core.mic(s, t)))
        .map_err(|e| format!("The microphone can't be opened ({e}). {denied}"))
}

/// Records the computer sound (the default output) into the recording.
pub fn start_system(core: Arc<Core>) -> Result<Capture, String> {
    let device = host().default_output_device().ok_or_else(|| {
        "No speakers or headphones were found, so the computer sound can't be recorded. Record without it.".to_string()
    })?;
    open(device, true, 2, Box::new(move |s, t| core.system(s, t))).map_err(|e| {
        format!("The computer sound can't be recorded ({e}). Record without it, and tell us with Report a problem.")
    })
}

/// The level meter on the setup screen: peak of the last 100 ms.
struct Monitor {
    _capture: Capture,
    peak: Arc<Mutex<(f32, f32, i64)>>,
}

static MONITOR: Mutex<Option<Monitor>> = Mutex::new(None);

pub fn monitor_start(mic_id: &str, denied: &str) -> Result<(), String> {
    monitor_stop();
    let device = mic_device(mic_id)
        .ok_or_else(|| "No microphone was found. Connect one, then try again.".to_string())?;
    let peak = Arc::new(Mutex::new((0f32, 0f32, 0i64)));
    let p = peak.clone();
    let capture = open(
        device,
        false,
        1,
        Box::new(move |s, _| {
            let mut p = p.lock().unwrap();
            for v in s {
                p.0 = p.0.max(v.abs());
                p.2 += 1;
                if p.2 >= RATE / 10 {
                    p.1 = p.0;
                    p.0 = 0.0;
                    p.2 = 0;
                }
            }
        }),
    )
    .map_err(|e| format!("The microphone can't be opened ({e}). {denied}"))?;
    *MONITOR.lock().unwrap() = Some(Monitor {
        _capture: capture,
        peak,
    });
    Ok(())
}

pub fn monitor_peak() -> f32 {
    MONITOR.lock().unwrap().as_ref().map_or(0.0, |m| {
        let p = m.peak.lock().unwrap();
        p.0.max(p.1)
    })
}

pub fn monitor_stop() {
    let monitor = MONITOR.lock().unwrap().take();
    drop(monitor);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resamples_to_48k_in_pieces() {
        // 1 s of a 44.1 kHz ramp, in uneven pieces: 48 000 frames out, still a ramp.
        let mut r = Resampler::new(44_100, 1);
        let input: Vec<f32> = (0..44_100).map(|i| i as f32).collect();
        let mut all = Vec::new();
        let mut out = Vec::new();
        for piece in input.chunks(441 * 3 + 7) {
            r.run(piece, &mut out);
            all.extend_from_slice(&out);
        }
        assert!((all.len() as i64 - 48_000).abs() <= 1, "{}", all.len());
        for (k, v) in all.iter().enumerate().skip(1).take(47_000) {
            let expected = k as f32 * 44_100.0 / 48_000.0;
            assert!((v - expected).abs() < 0.01, "{k}: {v} vs {expected}");
        }
    }

    #[test]
    fn mixes_channels() {
        let mut out = Vec::new();
        remix(&[0.2, 0.4, 1.0, 0.0], 2, 1, &mut out);
        assert_eq!(out, vec![0.3, 0.5]);
        remix(&[0.5], 1, 2, &mut out);
        assert_eq!(out, vec![0.5, 0.5]);
        remix(&[0.1, 0.2, 0.3, 0.4, 0.5, 0.6], 6, 2, &mut out);
        assert_eq!(out, vec![0.1, 0.2]);
    }
}

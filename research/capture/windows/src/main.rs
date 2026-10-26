//! wincap: M1 capture research prototype for Windows (docs/capture-research.md).
//!
//! Measures how each candidate API delivers frames and samples, without encoding:
//!
//!     wincap list
//!     wincap wgc SECONDS [--readback]    Windows.Graphics.Capture, primary monitor
//!     wincap dxgi SECONDS [--readback]   DXGI Desktop Duplication, primary monitor
//!     wincap audio SECONDS               WASAPI loopback (default output) + default mic
//!
//! `--readback` copies every frame to CPU memory, as an encoder fed from system memory
//! would need. Prints one JSON object per run. The CI workflow puts a moving picture on
//! screen first: both screen APIs deliver frames only when something changes.

use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// Frame-interval statistics from arrival times in seconds.
fn interval_stats(times: &[f64], seconds: f64) -> String {
    let mut gaps: Vec<f64> = times.windows(2).map(|w| (w[1] - w[0]) * 1000.0).collect();
    gaps.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pick = |q: f64| gaps.get(((gaps.len() as f64 - 1.0) * q).round() as usize).copied().unwrap_or(0.0);
    format!(
        "\"frames\": {}, \"fps\": {:.2}, \"interval_ms_p50\": {:.2}, \"interval_ms_p99\": {:.2}, \"interval_ms_max\": {:.2}, \"gaps_over_50ms\": {}",
        times.len(),
        times.len() as f64 / seconds,
        pick(0.5),
        pick(0.99),
        gaps.last().copied().unwrap_or(0.0),
        gaps.iter().filter(|g| **g > 50.0).count()
    )
}

fn audio_device_json(device: Option<cpal::Device>, input: bool) -> String {
    let Some(device) = device else { return "null".into() };
    let name = device.description().map(|d| d.name().to_string()).unwrap_or_else(|e| format!("? {e}"));
    let config = if input { device.default_input_config() } else { device.default_output_config() };
    match config {
        Ok(c) => format!(
            "{{\"name\": {:?}, \"rate\": {}, \"channels\": {}, \"format\": \"{:?}\"}}",
            name,
            c.sample_rate(),
            c.channels(),
            c.sample_format()
        ),
        Err(e) => format!("{{\"name\": {name:?}, \"error\": {:?}}}", e.to_string()),
    }
}

/// Counts samples from `device` for `seconds`; on Windows an *output* device opened as
/// input is WASAPI loopback (cpal does that transparently).
fn count_samples(device: &cpal::Device, loopback: bool, seconds: f64) -> String {
    let config = if loopback { device.default_output_config() } else { device.default_input_config() };
    let config = match config {
        Ok(c) => c,
        Err(e) => return format!("{{\"error\": {:?}}}", e.to_string()),
    };
    let rate = config.sample_rate() as f64;
    let channels = config.channels() as usize;
    let counted = std::sync::Arc::new(std::sync::Mutex::new((0usize, 0usize, None::<Instant>)));
    let c2 = counted.clone();
    let stream = device.build_input_stream::<f32, _, _>(
        config.config(),
        move |data: &[f32], _| {
            let mut c = c2.lock().unwrap();
            c.0 += data.len() / channels;
            c.1 += 1;
            c.2.get_or_insert_with(Instant::now);
        },
        |e| eprintln!("audio error: {e}"),
        None,
    );
    let stream = match stream {
        Ok(s) => s,
        Err(e) => return format!("{{\"error\": {:?}}}", e.to_string()),
    };
    if let Err(e) = stream.play() {
        return format!("{{\"error\": {:?}}}", e.to_string());
    }
    std::thread::sleep(Duration::from_secs_f64(seconds));
    drop(stream);
    let (samples, callbacks, first) = *counted.lock().unwrap();
    let span = first.map(|f| f.elapsed().as_secs_f64()).unwrap_or(0.0);
    // Loopback delivers nothing while nothing plays; the expected count is rate × time.
    format!(
        "{{\"samples\": {samples}, \"callbacks\": {callbacks}, \"expected\": {:.0}, \"ratio\": {:.4}}}",
        rate * span,
        if span > 0.0 { samples as f64 / (rate * span) } else { 0.0 }
    )
}

fn audio(seconds: f64) {
    let host = cpal::default_host();
    let out = host.default_output_device();
    let mic = host.default_input_device();
    let loop_json = out.as_ref().map(|d| count_samples(d, true, seconds)).unwrap_or("null".into());
    let mic_json = mic.as_ref().map(|d| count_samples(d, false, seconds)).unwrap_or("null".into());
    println!("{{\"api\": \"wasapi\", \"seconds\": {seconds}, \"loopback\": {loop_json}, \"mic\": {mic_json}}}");
}

fn list() {
    let host = cpal::default_host();
    println!(
        "{{\"audio_host\": \"{:?}\", \"default_output\": {}, \"default_input\": {}}}",
        host.id(),
        audio_device_json(host.default_output_device(), false),
        audio_device_json(host.default_input_device(), true)
    );
    #[cfg(windows)]
    win::list();
}

#[cfg(windows)]
mod win {
    use super::interval_stats;
    use std::time::Instant;
    use windows_capture::capture::{Context, GraphicsCaptureApiHandler};
    use windows_capture::dxgi_duplication_api::{DxgiDuplicationApi, Error as DxgiError};
    use windows_capture::frame::Frame;
    use windows_capture::graphics_capture_api::InternalCaptureControl;
    use windows_capture::monitor::Monitor;
    use windows_capture::settings::{
        ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
        MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
    };

    pub fn list() {
        match Monitor::enumerate() {
            Ok(monitors) => {
                for m in monitors {
                    println!(
                        "{{\"monitor\": {:?}, \"width\": {:?}, \"height\": {:?}}}",
                        m.name().unwrap_or_default(),
                        m.width().ok(),
                        m.height().ok()
                    );
                }
            }
            Err(e) => println!("{{\"monitors_error\": {:?}}}", e.to_string()),
        }
    }

    struct Wgc {
        seconds: f64,
        readback: bool,
        start: Instant,
        arrivals: Vec<f64>,
        system_times: Vec<f64>,
        size: (u32, u32),
        touched: u64,
    }

    impl GraphicsCaptureApiHandler for Wgc {
        type Flags = (f64, bool);
        type Error = Box<dyn std::error::Error + Send + Sync>;

        fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
            Ok(Self {
                seconds: ctx.flags.0,
                readback: ctx.flags.1,
                start: Instant::now(),
                arrivals: Vec::new(),
                system_times: Vec::new(),
                size: (0, 0),
                touched: 0,
            })
        }

        fn on_frame_arrived(&mut self, frame: &mut Frame, control: InternalCaptureControl) -> Result<(), Self::Error> {
            self.arrivals.push(self.start.elapsed().as_secs_f64());
            self.system_times.push(frame.timestamp()?.Duration as f64 / 1e7);
            self.size = (frame.width(), frame.height());
            if self.readback {
                let mut buffer = frame.buffer()?;
                let raw = buffer.as_raw_buffer();
                self.touched += raw[raw.len() / 2] as u64;
            }
            if self.start.elapsed().as_secs_f64() >= self.seconds {
                let n = self.arrivals.len();
                let wall = self.arrivals[n - 1] - self.arrivals[0];
                let sys = self.system_times[n - 1] - self.system_times[0];
                println!(
                    "{{\"api\": \"wgc\", \"readback\": {}, \"size\": \"{}x{}\", {}, \"frame_clock_vs_wall\": {:.5}}}",
                    self.readback,
                    self.size.0,
                    self.size.1,
                    interval_stats(&self.system_times, self.seconds),
                    if wall > 0.0 { sys / wall } else { 0.0 }
                );
                control.stop();
            }
            Ok(())
        }
    }

    pub fn wgc(seconds: f64, readback: bool) {
        // Ask for up to 60 updates per second (the system still sends only changes). Older
        // Windows can't set the interval: then run with the system's default.
        for interval in [
            MinimumUpdateIntervalSettings::Custom(std::time::Duration::from_millis(16)),
            MinimumUpdateIntervalSettings::Default,
        ] {
            let custom = matches!(interval, MinimumUpdateIntervalSettings::Custom(_));
            let settings = Settings::new(
                Monitor::primary().expect("no primary monitor"),
                CursorCaptureSettings::WithCursor,
                DrawBorderSettings::WithoutBorder,
                SecondaryWindowSettings::Default,
                interval,
                DirtyRegionSettings::Default,
                ColorFormat::Bgra8,
                (seconds, readback),
            );
            match Wgc::start(settings) {
                Ok(()) => return,
                Err(e) => println!("{{\"api\": \"wgc\", \"custom_interval\": {custom}, \"error\": {:?}}}", e.to_string()),
            }
        }
    }

    pub fn dxgi(seconds: f64, readback: bool) {
        let monitor = Monitor::primary().expect("no primary monitor");
        let mut dup = match DxgiDuplicationApi::new(monitor) {
            Ok(d) => d,
            Err(e) => {
                println!("{{\"api\": \"dxgi\", \"error\": {:?}}}", e.to_string());
                return;
            }
        };
        let start = Instant::now();
        let (mut presents, mut timeouts, mut lost, mut touched) = (Vec::new(), 0, 0, 0u64);
        while start.elapsed().as_secs_f64() < seconds {
            match dup.acquire_next_frame(33) {
                Ok(mut frame) => {
                    // LastPresentTime is 0 when only the cursor moved.
                    if frame.frame_info().LastPresentTime != 0 {
                        presents.push(start.elapsed().as_secs_f64());
                        if readback {
                            let mut buffer = frame.buffer().expect("map frame");
                            let raw = buffer.as_raw_buffer();
                            touched += raw[raw.len() / 2] as u64;
                        }
                    }
                }
                Err(DxgiError::Timeout) => timeouts += 1,
                Err(DxgiError::AccessLost) => {
                    lost += 1;
                    dup = dup.recreate().expect("recreate duplication");
                }
                Err(e) => {
                    println!("{{\"api\": \"dxgi\", \"error\": {:?}}}", e.to_string());
                    return;
                }
            }
        }
        let _ = touched;
        println!(
            "{{\"api\": \"dxgi\", \"readback\": {readback}, {}, \"timeouts\": {timeouts}, \"access_lost\": {lost}}}",
            interval_stats(&presents, seconds)
        );
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let seconds = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(10.0);
    let readback = args.iter().any(|a| a == "--readback");
    let _ = readback;
    match args.get(1).map(String::as_str) {
        Some("list") => list(),
        Some("audio") => audio(seconds),
        #[cfg(windows)]
        Some("wgc") => win::wgc(seconds, readback),
        #[cfg(windows)]
        Some("dxgi") => win::dxgi(seconds, readback),
        _ => {
            eprintln!("usage: wincap list | wgc SECONDS [--readback] | dxgi SECONDS [--readback] | audio SECONDS");
            std::process::exit(2);
        }
    }
}

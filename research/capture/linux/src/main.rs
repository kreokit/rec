//! lincap: M1 capture research prototype for Linux (docs/capture-research.md).
//!
//!     lincap portal SECONDS   xdg-desktop-portal ScreenCast -> PipeWire video frames
//!     lincap audio SECONDS    PulseAudio protocol (also served by pipewire-pulse):
//!                             the default sink's monitor (computer sound) + default mic
//!
//! Counts frames and samples without encoding and prints one JSON object per run. The
//! portal stream, like the other systems' screen APIs, only sends frames when the screen
//! changes, so the CI workflow plays a moving picture first.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use ashpd::desktop::PersistMode;
use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use pipewire as pw;
use pw::{properties::properties, spa};

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

async fn open_portal() -> ashpd::Result<(u32, Option<(i32, i32)>, std::os::fd::OwnedFd)> {
    let proxy = Screencast::new().await?;
    let session = proxy.create_session(Default::default()).await?;
    proxy
        .select_sources(
            &session,
            SelectSourcesOptions::default()
                .set_cursor_mode(CursorMode::Embedded)
                .set_sources(ashpd::enumflags2::BitFlags::from(SourceType::Monitor))
                .set_multiple(false)
                .set_persist_mode(PersistMode::DoNot),
        )
        .await?;
    let response = proxy.start(&session, None, Default::default()).await?.response()?;
    let stream = response.streams().first().expect("the portal returned no stream").to_owned();
    let fd = proxy.open_pipe_wire_remote(&session, Default::default()).await?;
    Ok((stream.pipe_wire_node_id(), stream.size(), fd))
}

struct Frames {
    start: Option<Instant>,
    arrivals: Vec<f64>,
    format: spa::param::video::VideoInfoRaw,
    empty: usize,
}

fn portal(seconds: f64) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let opened = Instant::now();
    let (node, size, fd) = match runtime.block_on(open_portal()) {
        Ok(v) => v,
        Err(e) => {
            println!("{{\"api\": \"portal\", \"error\": {:?}}}", e.to_string());
            return;
        }
    };
    eprintln!("portal gave node {node}, size {size:?} after {:.2} s", opened.elapsed().as_secs_f64());

    // Watchdog: report and stop even if no frame ever arrives.
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs_f64(seconds + 15.0));
        println!("{{\"api\": \"portal\", \"node\": {node}, \"error\": \"no frames within {} s\"}}", seconds + 15.0);
        std::process::exit(1);
    });

    pw::init();
    let mainloop = pw::main_loop::MainLoopBox::new(None).unwrap();
    let context = pw::context::ContextBox::new(mainloop.loop_(), None).unwrap();
    let core = context.connect_fd(fd, None).unwrap();
    let stream = pw::stream::StreamBox::new(
        &core,
        "lincap",
        properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )
    .unwrap();
    let state = Rc::new(RefCell::new(Frames { start: None, arrivals: Vec::new(), format: Default::default(), empty: 0 }));
    let s2 = state.clone();
    let _listener = stream
        .add_local_listener_with_user_data(())
        .param_changed(move |_, _, id, param| {
            let Some(param) = param else { return };
            if id != pw::spa::param::ParamType::Format.as_raw() {
                return;
            }
            let _ = s2.borrow_mut().format.parse(param);
        })
        .process(move |stream, _| {
            let Some(mut buffer) = stream.dequeue_buffer() else { return };
            let mut st = state.borrow_mut();
            let datas = buffer.datas_mut();
            if datas.is_empty() || datas[0].chunk().size() == 0 {
                st.empty += 1; // e.g. cursor-only updates
                return;
            }
            let now = Instant::now();
            let start = *st.start.get_or_insert(now);
            st.arrivals.push(now.duration_since(start).as_secs_f64());
            if now.duration_since(start).as_secs_f64() >= seconds {
                let f = &st.format;
                println!(
                    "{{\"api\": \"portal\", \"node\": {node}, \"format\": \"{:?}\", \"size\": \"{}x{}\", \"negotiated_fps\": \"{}/{}\", {}, \"empty_buffers\": {}}}",
                    f.format(),
                    f.size().width,
                    f.size().height,
                    f.framerate().num,
                    f.framerate().denom,
                    interval_stats(&st.arrivals, seconds),
                    st.empty
                );
                std::process::exit(0);
            }
        })
        .register()
        .unwrap();

    // Ask for raw BGRx/RGBx at up to 60 fps, any size.
    let obj = pw::spa::pod::object!(
        pw::spa::utils::SpaTypes::ObjectParamFormat,
        pw::spa::param::ParamType::EnumFormat,
        pw::spa::pod::property!(pw::spa::param::format::FormatProperties::MediaType, Id, pw::spa::param::format::MediaType::Video),
        pw::spa::pod::property!(pw::spa::param::format::FormatProperties::MediaSubtype, Id, pw::spa::param::format::MediaSubtype::Raw),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            pw::spa::param::video::VideoFormat::BGRx,
            pw::spa::param::video::VideoFormat::BGRx,
            pw::spa::param::video::VideoFormat::RGBx,
            pw::spa::param::video::VideoFormat::BGRA,
            pw::spa::param::video::VideoFormat::RGBA,
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            pw::spa::utils::Rectangle { width: 1920, height: 1080 },
            pw::spa::utils::Rectangle { width: 1, height: 1 },
            pw::spa::utils::Rectangle { width: 8192, height: 8192 }
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            pw::spa::utils::Fraction { num: 60, denom: 1 },
            pw::spa::utils::Fraction { num: 0, denom: 1 },
            pw::spa::utils::Fraction { num: 1000, denom: 1 }
        ),
    );
    let values: Vec<u8> = pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(obj),
    )
    .unwrap()
    .0
    .into_inner();
    let mut params = [spa::pod::Pod::from_bytes(&values).unwrap()];
    stream
        .connect(
            spa::utils::Direction::Input,
            Some(node),
            pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .unwrap();
    mainloop.run();
}

/// Counts samples (and the peak level) from `device` for `seconds`.
fn count_samples(device: &cpal::Device, seconds: f64) -> String {
    let name = device.description().map(|d| d.name().to_string()).unwrap_or_default();
    let config = match device.default_input_config() {
        Ok(c) => c,
        Err(e) => return format!("{{\"name\": {name:?}, \"error\": {:?}}}", e.to_string()),
    };
    let rate = config.sample_rate() as f64;
    let channels = config.channels() as usize;
    let counted = std::sync::Arc::new(std::sync::Mutex::new((0usize, 0f32, None::<Instant>)));
    let c2 = counted.clone();
    let stream = device.build_input_stream::<f32, _, _>(
        config.config(),
        move |data: &[f32], _| {
            let mut c = c2.lock().unwrap();
            c.0 += data.len() / channels;
            c.1 = data.iter().fold(c.1, |m, s| m.max(s.abs()));
            c.2.get_or_insert_with(Instant::now);
        },
        |e| eprintln!("audio error: {e}"),
        None,
    );
    let stream = match stream {
        Ok(s) => s,
        Err(e) => return format!("{{\"name\": {name:?}, \"error\": {:?}}}", e.to_string()),
    };
    if let Err(e) = stream.play() {
        return format!("{{\"name\": {name:?}, \"error\": {:?}}}", e.to_string());
    }
    std::thread::sleep(Duration::from_secs_f64(seconds));
    drop(stream);
    let (samples, peak, first) = *counted.lock().unwrap();
    let span = first.map(|f| f.elapsed().as_secs_f64()).unwrap_or(0.0);
    format!(
        "{{\"name\": {name:?}, \"rate\": {rate}, \"channels\": {channels}, \"samples\": {samples}, \"ratio\": {:.4}, \"peak\": {peak:.3}}}",
        if span > 0.0 { samples as f64 / (rate * span) } else { 0.0 }
    )
}

fn audio(seconds: f64) {
    let host = match cpal::host_from_id(cpal::HostId::PulseAudio) {
        Ok(h) => h,
        Err(e) => {
            println!("{{\"api\": \"pulseaudio\", \"error\": {:?}}}", e.to_string());
            return;
        }
    };
    let devices: Vec<cpal::Device> = host.devices().map(|d| d.collect()).unwrap_or_default();
    let names: Vec<String> = devices.iter().map(|d| d.description().map(|x| x.name().to_string()).unwrap_or_default()).collect();
    eprintln!("devices: {names:?}");
    // The monitor of the default sink is what the computer is playing.
    let monitor = devices
        .iter()
        .find(|d| d.description().map(|x| x.name().to_lowercase().contains("monitor")).unwrap_or(false));
    let monitor_json = monitor.map(|d| count_samples(d, seconds)).unwrap_or("null".into());
    let mic_json = host.default_input_device().map(|d| count_samples(&d, seconds)).unwrap_or("null".into());
    println!("{{\"api\": \"pulseaudio\", \"seconds\": {seconds}, \"devices\": {names:?}, \"monitor\": {monitor_json}, \"default_input\": {mic_json}}}");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let seconds = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(10.0);
    match args.get(1).map(String::as_str) {
        Some("portal") => portal(seconds),
        Some("audio") => audio(seconds),
        _ => {
            eprintln!("usage: lincap portal SECONDS | audio SECONDS");
            std::process::exit(2);
        }
    }
}

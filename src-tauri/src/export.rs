//! Export (milestone 4): the trimmed recording composed in its layout, with the finished
//! sound, written as one MP4 (H.264 + AAC 48 kHz) and checked after writing.
//!
//! 1. The sound for exactly the trimmed part (`sound::make`: clean-up, mix, loudness, AAC),
//!    so the preset is met on what is exported.
//! 2. One FFmpeg run: each picture track seeked to the trim's start (frame-accurate), the
//!    camera cut and scaled to its place (layout.rs) with the bubble's smooth mask, laid
//!    over the screen, encoded as the recording was (encoder.rs: VideoToolbox / Media
//!    Foundation / VA-API / OpenH264, BT.709, the recording's bitrate); the sound copied in.
//! 3. The file read back (as KreoKit Clips does): size, H.264 profile, yuv420p, BT.709,
//!    faststart, AAC 48 kHz, every stream as long as the trim, the loudness and true peak.
//! 4. Written next to its destination under a temporary name, then moved into place,
//!    never over another file unless the user chose Replace (from Clips and Cutout).

use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;

use crate::encoder;
use crate::ffmpeg;
use crate::finish::{Layout, Settings};
use crate::layout::{self, Placement};
use crate::sessions::{self, Session};
use crate::sound::{self, Mix, RATE};

/// The sound's sample rate in the export.
pub const AUDIO_RATE: u32 = 48_000;
/// How far the loudness may be from the preset (the presets promise ±0.5 LU).
pub const LOUDNESS_TOLERANCE_LU: f64 = 0.5;
/// A true peak at least this high means the limiter is holding the sound back.
pub const PEAK_HELD_DB: f64 = -2.0;

/// The sound stayed quieter than the preset because its peaks are at the limit: very
/// spiky sound (clicks, hard taps; measured 2026-10-27 on the synthetic clicks, 24 dB
/// between peak and loudness, which stop at −22 LUFS for −16). Never going over −1 dBTP
/// wins over the loudness: louder would distort it.
pub fn held_by_peaks(target: Option<f64>, lufs: Option<f64>, true_peak_db: f64) -> bool {
    matches!((target, lufs), (Some(t), Some(l)) if l < t - LOUDNESS_TOLERANCE_LU)
        && true_peak_db >= PEAK_HELD_DB
}
pub const CANCELLED: &str = "cancelled";

/// What the export needs to know about the recording and the choices.
pub struct Job<'a> {
    pub dir: &'a Path,
    pub session: &'a Session,
    pub settings: &'a Settings,
    /// The recording's length (the check's).
    pub duration_s: f64,
}

impl Job<'_> {
    pub fn fps(&self) -> u32 {
        self.session.screen.as_ref().map_or(30, |s| s.fps.max(1))
    }

    /// The trim, in whole frames.
    pub fn trim(&self) -> (f64, f64) {
        let fps = self.fps() as f64;
        let start = (self.settings.start_s * fps).round() / fps;
        let end = (self.settings.end_s.unwrap_or(self.duration_s) * fps).round() / fps;
        (
            start,
            end.min((self.duration_s * fps).round() / fps)
                .max(start + 1.0 / fps),
        )
    }

    pub fn length_s(&self) -> f64 {
        let (start, end) = self.trim();
        end - start
    }

    pub fn placement(&self) -> Placement {
        layout::place(
            self.settings.layout,
            &self.settings.bubble,
            self.session.screen.as_ref().map(|s| (s.width, s.height)),
            self.session.camera.as_ref().map(|c| (c.width, c.height)),
        )
    }

    fn has_sound(&self) -> bool {
        self.dir.join("mic.mp4").is_file() || self.dir.join("system.mp4").is_file()
    }

    /// The expected size in bytes (picture at the recording's bitrate, AAC sound).
    pub fn estimated_bytes(&self) -> u64 {
        let p = self.placement();
        let bits = sessions::video_bitrate(p.width, p.height) as f64 + 192_000.0;
        (bits / 8.0 * self.length_s()) as u64
    }
}

/// "Recording-2026-10-27-1432.mp4" for the recording "2026-10-27-143205".
pub fn file_name(session: &Session) -> String {
    let stamp = session.id.get(..15).unwrap_or(&session.id);
    format!("Recording-{stamp}.mp4")
}

/// The picture's filter graph for `job`, ending in `[v]`. Inputs: 0 screen, 1 camera,
/// 2 the bubble's mask (only for the bubble).
pub fn graph(job: &Job) -> String {
    let p = job.placement();
    let fps = job.fps();
    // The picture as exported carries BT.709 video range (the encoder writes the same).
    let tags =
        "format=yuv420p,setparams=range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709";
    let camera = |crop: layout::Rect, w: u32, h: u32, alpha: bool| {
        format!(
            "[1:v]setpts=PTS-STARTPTS,fps={fps},crop={}:{}:{}:{},scale={w}:{h}:flags=bicubic:in_color_matrix=bt709:out_color_matrix=bt709,format={}",
            crop.width,
            crop.height,
            crop.x,
            crop.y,
            if alpha { "yuva420p" } else { "yuv420p" }
        )
    };
    match (job.settings.layout, p.camera, p.camera_crop) {
        (Layout::Bubble, Some(cam), Some(crop)) => format!(
            "[0:v]setpts=PTS-STARTPTS,fps={fps},format=yuv420p[s];\
             {}[c];[2:v]format=gray,fps={fps}[m];[c][m]alphamerge[b];\
             [s][b]overlay={}:{}:format=yuv420:eof_action=repeat,{tags}[v]",
            camera(crop, cam.width, cam.height, true),
            cam.x,
            cam.y
        ),
        (Layout::Camera, Some(cam), Some(crop)) => {
            format!("{},{tags}[v]", camera(crop, cam.width, cam.height, false))
        }
        _ => format!("[0:v]setpts=PTS-STARTPTS,fps={fps},{tags}[v]"),
    }
}

/// Encoder and output arguments (after the graph): the picture as the recording was
/// encoded, the sound (input `sound_input`, already AAC) copied, the trim's length,
/// faststart.
pub fn output_args(
    encoder: &str,
    width: u32,
    height: u32,
    fps: u32,
    length_s: f64,
    sound_input: Option<usize>,
) -> Vec<String> {
    let mut a: Vec<String> = Vec::new();
    let mut push = |items: &[&str]| a.extend(items.iter().map(|s| s.to_string()));
    push(&["-map", "[v]"]);
    let gop = fps.to_string();
    let bitrate = sessions::video_bitrate(width, height).to_string();
    push(&["-c:v", encoder, "-profile:v", "high", "-b:v", &bitrate]);
    push(&["-g", &gop, "-keyint_min", &gop]);
    if encoder == "h264_videotoolbox" {
        // Apple's software encoder where there's no hardware one (VMs, small sizes).
        push(&["-allow_sw", "1"]);
    }
    if encoder == "h264_mf" {
        push(&["-rate_control", "cbr"]);
    }
    push(&[
        "-colorspace",
        "bt709",
        "-color_primaries",
        "bt709",
        "-color_trc",
        "bt709",
        "-color_range",
        "tv",
    ]);
    if let Some(i) = sound_input {
        push(&["-map", &format!("{i}:a:0"), "-c:a", "copy"]);
    }
    push(&["-t", &format!("{length_s:.6}")]);
    push(&["-movflags", "+faststart", "-f", "mp4"]);
    a
}

/// The bubble's mask as a PGM image (FFmpeg reads it as a still that repeats).
fn write_mask(path: &Path, width: u32, height: u32, radius: u32) -> Result<(), String> {
    let mut bytes = format!("P5\n{width} {height}\n255\n").into_bytes();
    bytes.extend(layout::mask(width, height, radius));
    fs::write(path, bytes).map_err(|e| format!("cannot write the bubble's shape: {e}"))
}

/// What ffprobe says about an exported file (Clips' `Facts`).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Facts {
    pub width: u32,
    pub height: u32,
    pub video_codec: String,
    pub profile: String,
    pub pix_fmt: String,
    pub color_space: String,
    pub color_primaries: String,
    pub color_transfer: String,
    pub color_range: String,
    pub audio_codec: String,
    pub sample_rate: u32,
    pub container_s: f64,
    pub video_s: f64,
    pub audio_s: f64,
    /// The index (`moov`) comes before the media data, so playback starts at once.
    pub faststart: bool,
}

pub fn probe_output(path: &Path) -> Result<Facts, String> {
    let ffprobe = ffmpeg::locate("ffprobe")?;
    let output = Command::new(&ffprobe)
        .args([
            "-v",
            "error",
            "-show_streams",
            "-show_format",
            "-of",
            "json",
        ])
        .arg(path)
        .output()
        .map_err(|e| format!("failed to run {}: {e}", ffprobe.display()))?;
    if !output.status.success() {
        return Err(format!(
            "cannot read the exported file: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).map_err(|e| format!("ffprobe output: {e}"))?;
    let text = |v: &serde_json::Value, key: &str| v[key].as_str().unwrap_or_default().to_string();
    let secs = |v: &serde_json::Value| {
        v["duration"]
            .as_str()
            .and_then(|d| d.parse::<f64>().ok())
            .unwrap_or(0.0)
    };
    let streams = json["streams"].as_array().cloned().unwrap_or_default();
    let of = |kind: &str| streams.iter().find(|s| s["codec_type"] == kind).cloned();
    let mut facts = Facts {
        container_s: secs(&json["format"]),
        faststart: moov_first(path)?,
        ..Facts::default()
    };
    if let Some(v) = of("video") {
        facts.width = v["width"].as_u64().unwrap_or(0) as u32;
        facts.height = v["height"].as_u64().unwrap_or(0) as u32;
        facts.video_codec = text(&v, "codec_name");
        facts.profile = text(&v, "profile");
        facts.pix_fmt = text(&v, "pix_fmt");
        facts.color_space = text(&v, "color_space");
        facts.color_primaries = text(&v, "color_primaries");
        facts.color_transfer = text(&v, "color_transfer");
        facts.color_range = text(&v, "color_range");
        facts.video_s = secs(&v);
    }
    if let Some(a) = of("audio") {
        facts.audio_codec = text(&a, "codec_name");
        facts.sample_rate = text(&a, "sample_rate").parse().unwrap_or(0);
        facts.audio_s = secs(&a);
    }
    Ok(facts)
}

/// Whether the file's top-level `moov` box comes before its `mdat` (from Clips).
fn moov_first(path: &Path) -> Result<bool, String> {
    use std::io::{Seek, SeekFrom};
    let mut file = fs::File::open(path).map_err(|e| format!("cannot read the export: {e}"))?;
    let len = file.metadata().map_err(|e| e.to_string())?.len();
    let mut at = 0u64;
    while at + 8 <= len {
        file.seek(SeekFrom::Start(at)).map_err(|e| e.to_string())?;
        let mut head = [0u8; 16];
        file.read_exact(&mut head[..8]).map_err(|e| e.to_string())?;
        let mut size = u32::from_be_bytes(head[..4].try_into().unwrap()) as u64;
        if size == 1 {
            file.read_exact(&mut head[8..]).map_err(|e| e.to_string())?;
            size = u64::from_be_bytes(head[8..].try_into().unwrap());
        }
        match &head[4..8] {
            b"moov" => return Ok(true),
            b"mdat" => return Ok(false),
            _ => {}
        }
        if size < 8 {
            break;
        }
        at += size;
    }
    Ok(false)
}

/// What the finished file must be. `loudness`: the target (`None` for Keep) and what was
/// read back from the file, when it has sound.
pub fn check(
    facts: &Facts,
    encoder: &str,
    placement: &Placement,
    length_s: f64,
    fps: u32,
    sound: Option<(Option<f64>, Option<f64>, f64)>,
) -> Result<(), String> {
    let mut problems = Vec::new();
    let mut expect = |ok: bool, what: String| {
        if !ok {
            problems.push(what);
        }
    };
    expect(
        (facts.width, facts.height) == (placement.width, placement.height),
        format!(
            "size {}×{} instead of {}×{}",
            facts.width, facts.height, placement.width, placement.height
        ),
    );
    expect(
        facts.video_codec == "h264",
        format!("video codec {}", facts.video_codec),
    );
    expect(
        encoder::accepted_profiles(encoder).contains(&facts.profile.as_str()),
        format!("H.264 profile {} from {encoder}", facts.profile),
    );
    expect(
        facts.pix_fmt == "yuv420p",
        format!("pixel format {}", facts.pix_fmt),
    );
    for (name, value) in [
        ("colour matrix", &facts.color_space),
        ("colour primaries", &facts.color_primaries),
        ("transfer", &facts.color_transfer),
    ] {
        expect(value == "bt709", format!("{name} {value}"));
    }
    expect(
        facts.color_range == "tv",
        format!("colour range {}", facts.color_range),
    );
    expect(
        facts.faststart,
        "the index isn't at the start (faststart)".into(),
    );
    // Within a frame (and an AAC frame for the sound).
    let frame = 1.0 / fps as f64;
    let slack = frame.max(1024.0 / AUDIO_RATE as f64) + 0.01;
    let mut lengths = vec![("file", facts.container_s), ("picture", facts.video_s)];
    match sound {
        Some((target, lufs, peak)) => {
            expect(
                facts.audio_codec == "aac",
                format!("audio codec {}", facts.audio_codec),
            );
            expect(
                facts.sample_rate == AUDIO_RATE,
                format!("sample rate {}", facts.sample_rate),
            );
            lengths.push(("sound", facts.audio_s));
            if let (Some(t), Some(l)) = (target, lufs) {
                expect(
                    (l - t).abs() <= LOUDNESS_TOLERANCE_LU || held_by_peaks(target, lufs, peak),
                    format!("loudness {l:.1} LUFS instead of {t:.0}"),
                );
            }
            expect(
                peak <= sound::MAX_TRUE_PEAK_DB + 0.05,
                format!("true peak {peak:.1} dBTP, over −1"),
            );
        }
        None => expect(
            facts.audio_codec.is_empty(),
            "a sound track that shouldn't be there".into(),
        ),
    }
    for (name, s) in lengths {
        expect(
            (s - length_s).abs() <= slack,
            format!("{name} lasts {s:.3} s instead of {length_s:.3} s"),
        );
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "The exported video isn't right ({}). Export again; if it happens again, use Report a problem.",
            problems.join(", ")
        ))
    }
}

/// What the export made.
#[derive(Debug, Clone, Serialize)]
pub struct Done {
    pub path: PathBuf,
    pub facts: Facts,
    pub encoder: String,
    pub lufs: Option<f64>,
    pub true_peak_db: Option<f64>,
    /// Quieter than the preset because louder would distort the peaks.
    pub held_by_peaks: bool,
}

/// Exports `job` to `dest`. `replace`: an existing file there is replaced; otherwise it's
/// kept and the export gets the first free name (`-2`, `-3`…). `progress` gets a stage
/// ("sound", "picture", "check") and 0–1.
pub fn export(
    job: &Job,
    dest: &Path,
    replace: bool,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&'static str, f64),
) -> Result<Done, String> {
    let ffmpeg = ffmpeg::locate("ffmpeg")?;
    let encoder = encoder::video_encoder()?;
    let placement = job.placement();
    let (start, _) = job.trim();
    let length = job.length_s();
    let fps = job.fps();
    let work = job.dir.join("export-work");
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work).map_err(|e| format!("cannot prepare the export: {e}"))?;
    let tmp = temp_path(dest);
    let result = (|| {
        // 1. The sound.
        let mut made = None;
        let sound_path = work.join("sound.m4a");
        if job.has_sound() {
            let frames = (job.duration_s * RATE as f64).round() as u64;
            let voice = sound::voice(
                job.dir,
                job.settings.clean_voice,
                frames,
                cancel,
                &mut |f| progress("sound", f * 0.5),
            )?;
            let mix = Mix {
                voice,
                system: Some(job.dir.join("system.mp4")).filter(|p| p.is_file()),
                start: (start * RATE as f64).round() as u64,
                frames: (length * RATE as f64).round() as u64,
            };
            let target = job.settings.loudness.lufs();
            made = Some(sound::make(&mix, target, &sound_path, cancel, &mut |f| {
                progress("sound", 0.5 + f * 0.5)
            })?);
        }

        // 2. The picture, with the sound.
        let mut inputs: Vec<String> = Vec::new();
        let mut add = |args: &[&str], path: &Path| {
            inputs.extend(args.iter().map(|s| s.to_string()));
            inputs.push("-i".into());
            inputs.push(path.to_string_lossy().into_owned());
        };
        let seek = format!("{start:.6}");
        // Inputs 0 and 1 are always the screen and the camera (a missing one is replaced by
        // the other, unused), so the graph's numbering stays the same.
        let screen = job.dir.join("screen.mp4");
        let camera = job.dir.join("camera.mp4");
        let first = if screen.is_file() { &screen } else { &camera };
        add(&["-ss", &seek], first);
        add(
            &["-ss", &seek],
            if camera.is_file() { &camera } else { first },
        );
        let mut next = 2;
        if job.settings.layout == Layout::Bubble
            && let (Some(cam), true) = (placement.camera, placement.camera.is_some())
        {
            let mask = work.join("mask.pgm");
            write_mask(&mask, cam.width, cam.height, placement.radius)?;
            add(&["-loop", "1", "-framerate", &fps.to_string()], &mask);
            next = 3;
        }
        let sound_input = made.map(|_| next);
        if made.is_some() {
            add(&[], &sound_path);
        }
        let args: Vec<String> = [
            "-nostdin",
            "-y",
            "-v",
            "error",
            "-nostats",
            "-progress",
            "pipe:1",
        ]
        .iter()
        .map(|s| s.to_string())
        .chain(inputs)
        .chain(["-filter_complex".to_string(), graph(job)])
        .chain(output_args(
            encoder,
            placement.width,
            placement.height,
            fps,
            length,
            sound_input,
        ))
        .chain([tmp.to_string_lossy().into_owned()])
        .collect();
        run(&ffmpeg, &args, length, cancel, &mut |f| {
            progress("picture", f)
        })?;

        // 3. The checks.
        progress("check", 0.0);
        let facts = probe_output(&tmp)?;
        let read = if made.is_some() {
            Some(sound::read_back(&tmp)?)
        } else {
            None
        };
        check(
            &facts,
            encoder,
            &placement,
            length,
            fps,
            read.map(|(lufs, peak)| (job.settings.loudness.lufs(), lufs, peak)),
        )?;
        progress("check", 1.0);
        Ok((facts, read))
    })();
    let _ = fs::remove_dir_all(&work);
    match result {
        Ok((facts, read)) => {
            // 4. Into place.
            let path = place(&tmp, dest, replace)?;
            Ok(Done {
                path,
                facts,
                encoder: encoder.to_string(),
                lufs: read.and_then(|r| r.0),
                true_peak_db: read.map(|r| r.1),
                held_by_peaks: read.is_some_and(|(lufs, peak)| {
                    held_by_peaks(job.settings.loudness.lufs(), lufs, peak)
                }),
            })
        }
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// Runs FFmpeg with `-progress pipe:1`, reporting 0–1 of `length_s`.
fn run(
    ffmpeg: &Path,
    args: &[String],
    length_s: f64,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f64),
) -> Result<(), String> {
    let mut child = Command::new(ffmpeg)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to run {}: {e}", ffmpeg.display()))?;
    // Collect errors on a thread so a chatty stderr can never block FFmpeg.
    let mut stderr = child.stderr.take().unwrap();
    let errors = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });
    for line in BufReader::new(child.stdout.take().unwrap()).lines() {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(CANCELLED.into());
        }
        let Ok(line) = line else { break };
        if let Some(us) = line
            .strip_prefix("out_time_us=")
            .and_then(|v| v.trim().parse::<f64>().ok())
        {
            progress((us / 1e6 / length_s.max(0.001)).clamp(0.0, 0.99));
        }
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    let errors = errors.join().unwrap_or_default();
    if cancel.load(Ordering::Relaxed) {
        return Err(CANCELLED.into());
    }
    if !status.success() {
        return Err(format!(
            "The video couldn't be written ({}). Check that the folder is writable and the disk isn't full, then export again.",
            errors.trim()
        ));
    }
    progress(1.0);
    Ok(())
}

/// Moves the finished temporary file `tmp` to `dest` (from Clips). With `replace` an
/// existing file is replaced; otherwise it is never touched: the export gets the first free
/// name of `dest`, `dest-2`, `dest-3`… (a file can appear while the export runs). The
/// temporary file is gone afterwards either way.
pub fn place(tmp: &Path, dest: &Path, replace: bool) -> Result<PathBuf, String> {
    let result = if replace {
        replace_with(tmp, dest).map(|()| dest.to_path_buf())
    } else {
        place_new(tmp, dest)
    };
    let _ = fs::remove_file(tmp);
    result
}

fn replace_with(tmp: &Path, dest: &Path) -> Result<(), String> {
    let fail = |e: std::io::Error| {
        format!(
            "The video couldn't be saved there ({e}). Choose another folder, then export again."
        )
    };
    match fs::rename(tmp, dest) {
        Ok(()) => Ok(()),
        // Windows can't rename over an existing file.
        Err(_) if dest.is_file() => {
            fs::remove_file(dest).map_err(fail)?;
            fs::rename(tmp, dest).map_err(fail)
        }
        Err(e) => Err(fail(e)),
    }
}

fn place_new(tmp: &Path, dest: &Path) -> Result<PathBuf, String> {
    let stem = dest
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let ext = dest
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    for n in 1.. {
        let candidate = match n {
            1 => dest.to_path_buf(),
            n => dest.with_file_name(format!("{stem}-{n}.{ext}")),
        };
        // A hard link fails if the name exists, so nothing is ever replaced; folders
        // without hard links (FAT, some network drives) fall back to a checked rename.
        match fs::hard_link(tmp, &candidate) {
            Ok(()) => return Ok(candidate),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) if candidate.exists() => continue,
            Err(_) => {
                return fs::rename(tmp, &candidate).map(|()| candidate.clone()).map_err(|e| {
                    format!("The video couldn't be saved there ({e}). Choose another folder, then export again.")
                });
            }
        }
    }
    unreachable!()
}

/// Lower-cased file names in `folder` (macOS and Windows compare names without case).
pub fn names_in(folder: &Path) -> HashSet<String> {
    fs::read_dir(folder)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().to_lowercase())
                .collect()
        })
        .unwrap_or_default()
}

/// A temporary name for `dest` in its folder (so placing it is a rename on one drive),
/// hidden and not ending in `.mp4`, so it's never taken for a finished video.
pub fn temp_path(dest: &Path) -> PathBuf {
    let name = dest.file_name().unwrap_or_default().to_string_lossy();
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    dest.with_file_name(format!(
        ".{name}.{}-{unique}.kreokit-tmp",
        std::process::id()
    ))
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::finish::{Bubble, Loudness, Shape, Size};
    use crate::sessions::{CameraInfo, ScreenInfo, State};

    pub fn session(screen: (u32, u32), camera: Option<(u32, u32)>, mic: bool) -> Session {
        Session {
            id: "2026-10-27-143205".into(),
            created_at: 0,
            state: State::Done,
            screen: Some(ScreenInfo {
                display: "Test display".into(),
                region: None,
                window: None,
                width: screen.0,
                height: screen.1,
                fps: 30,
            }),
            mic: mic.then(|| "Test microphone".into()),
            camera: camera.map(|(width, height)| CameraInfo {
                name: "Test camera".into(),
                width,
                height,
            }),
            system_audio: false,
            elapsed_s: 0.0,
            pauses: 0,
            check: None,
            error: None,
            hidden: false,
        }
    }

    fn settings(layout: Layout, bubble: Bubble) -> Settings {
        Settings {
            start_s: 0.0,
            end_s: None,
            layout,
            bubble,
            clean_voice: false,
            loudness: Loudness::Tutorial,
        }
    }

    #[test]
    fn names_the_file_after_the_recording() {
        assert_eq!(
            file_name(&session((1728, 1080), None, true)),
            "Recording-2026-10-27-1432.mp4"
        );
    }

    #[test]
    fn every_command_is_lgpl() {
        let s = session((1728, 1080), Some((1280, 720)), true);
        let dir = Path::new("/tmp");
        for layout in [Layout::Screen, Layout::Bubble, Layout::Camera] {
            let set = settings(layout, Bubble::default());
            let job = Job {
                dir,
                session: &s,
                settings: &set,
                duration_s: 10.0,
            };
            for encoder in ["h264_videotoolbox", "h264_mf", "h264_vaapi", "libopenh264"] {
                let mut args = vec!["-filter_complex".to_string(), graph(&job)];
                args.extend(output_args(encoder, 1728, 1080, 30, 10.0, Some(3)));
                assert_eq!(ffmpeg::gpl_only(&args), None, "{layout:?} {encoder}");
            }
        }
    }

    #[test]
    fn the_trim_is_in_whole_frames() {
        let s = session((1728, 1080), None, true);
        let mut set = settings(Layout::Screen, Bubble::default());
        set.start_s = 1.01;
        set.end_s = Some(5.02);
        let job = Job {
            dir: Path::new("/tmp"),
            session: &s,
            settings: &set,
            duration_s: 10.0,
        };
        let (a, b) = job.trim();
        assert!(
            (a - 1.0).abs() < 1e-9 && (b - 151.0 / 30.0).abs() < 1e-9,
            "{a} {b}"
        );
    }

    #[test]
    fn places_without_overwriting() {
        let dir = crate::sessions::tests::temp_root("export-place");
        let dest = dir.join("Recording.mp4");
        fs::write(&dest, "old").unwrap();
        let tmp = temp_path(&dest);
        fs::write(&tmp, "new").unwrap();
        let placed = place(&tmp, &dest, false).unwrap();
        assert_eq!(placed, dir.join("Recording-2.mp4"));
        assert_eq!(fs::read_to_string(&dest).unwrap(), "old");
        assert!(!tmp.exists());
        let tmp = temp_path(&dest);
        fs::write(&tmp, "newer").unwrap();
        assert_eq!(place(&tmp, &dest, true).unwrap(), dest);
        assert_eq!(fs::read_to_string(&dest).unwrap(), "newer");
    }

    /// A recording folder: a blue screen and a red camera picture, a speech-like mic.
    pub fn colour_recording(
        name: &str,
        seconds: f64,
        screen: (u32, u32),
        camera: (u32, u32),
    ) -> PathBuf {
        let dir = crate::sessions::tests::temp_root(name);
        let ffmpeg = ffmpeg::locate("ffmpeg").unwrap();
        for (file, colour, (w, h)) in [
            ("screen.mp4", "0x2040c0", screen),
            ("camera.mp4", "0xd02020", camera),
        ] {
            let ok = Command::new(&ffmpeg)
                .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
                .arg(format!("color=c={colour}:s={w}x{h}:r=30:d={seconds}"))
                .args(["-c:v", "mpeg4", "-q:v", "2", "-g", "30"])
                .arg(dir.join(file))
                .status()
                .unwrap()
                .success();
            assert!(ok);
        }
        let speech: Vec<f32> = crate::loudness::tests::speechy(seconds, -26.0)
            .into_iter()
            .step_by(2)
            .collect();
        let mut e = std::process::Command::new(&ffmpeg)
            .args(sound::input_args(1))
            .args(sound::AAC_ARGS)
            .arg(dir.join("mic.mp4"))
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        use std::io::Write;
        let bytes: Vec<u8> = speech.iter().flat_map(|s| s.to_le_bytes()).collect();
        e.stdin.take().unwrap().write_all(&bytes).unwrap();
        assert!(e.wait().unwrap().success());
        dir
    }

    /// One decoded RGB frame of `path` at `at` seconds.
    pub fn frame(path: &Path, at: f64, w: u32, h: u32) -> Vec<u8> {
        let out = Command::new(ffmpeg::locate("ffmpeg").unwrap())
            .args(["-v", "error", "-ss", &format!("{at}"), "-i"])
            .arg(path)
            .args([
                "-frames:v",
                "1",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgb24",
                "pipe:1",
            ])
            .output()
            .unwrap();
        assert_eq!(out.stdout.len(), (w * h * 3) as usize);
        out.stdout
    }

    /// Exports a bubble layout and checks every pixel class: the camera inside the bubble,
    /// the screen at the bubble's corners (a circle) and outside it; plus every check.
    #[test]
    fn exports_the_bubble_where_the_preview_shows_it() {
        let (screen, camera) = ((640, 360), (320, 240));
        let dir = colour_recording("export-bubble", 3.0, screen, camera);
        let s = session(screen, Some(camera), true);
        let mut set = settings(
            Layout::Bubble,
            Bubble {
                shape: Shape::Circle,
                size: Size::Large,
                x: 0.25,
                y: 0.75,
            },
        );
        set.start_s = 0.5;
        set.end_s = Some(2.5);
        let job = Job {
            dir: &dir,
            session: &s,
            settings: &set,
            duration_s: 3.0,
        };
        let dest = dir.join("out").join(file_name(&s));
        fs::create_dir_all(dest.parent().unwrap()).unwrap();
        let done = export(&job, &dest, false, &AtomicBool::new(false), &mut |_, _| {}).unwrap();
        assert_eq!(done.path, dest);
        assert!((done.facts.video_s - 2.0).abs() < 0.05, "{:?}", done.facts);
        assert!((done.lufs.unwrap() + 16.0).abs() <= 0.5, "{:?}", done.lufs);
        assert!(!dir.join("export-work").exists());

        let p = job.placement();
        let cam = p.camera.unwrap();
        let px = frame(&done.path, 1.0, 640, 360);
        let at = |x: u32, y: u32| {
            let i = ((y * 640 + x) * 3) as usize;
            (px[i] as i32, px[i + 1] as i32, px[i + 2] as i32)
        };
        let red = |(r, g, b): (i32, i32, i32)| r > 160 && g < 80 && b < 80;
        let blue = |(r, g, b): (i32, i32, i32)| b > 150 && r < 80 && g < 110;
        let (cx, cy) = (cam.x + cam.width / 2, cam.y + cam.height / 2);
        assert!(red(at(cx, cy)), "bubble centre {:?}", at(cx, cy));
        // Inside near the edge, along the axes.
        assert!(red(at(cam.x + 3, cy)) && red(at(cx, cam.y + cam.height - 4)));
        // The box's corners are outside the circle: the screen shows.
        assert!(
            blue(at(cam.x + 2, cam.y + 2)),
            "corner {:?}",
            at(cam.x + 2, cam.y + 2)
        );
        assert!(blue(at(cam.x + cam.width - 3, cam.y + cam.height - 3)));
        // Outside the box.
        assert!(blue(at(cam.x.saturating_sub(4), cy)) && blue(at(600, 20)));
    }

    #[test]
    fn exports_screen_only_and_camera_only_at_their_sizes() {
        let (screen, camera) = ((640, 360), (320, 240));
        let dir = colour_recording("export-layouts", 2.0, screen, camera);
        let s = session(screen, Some(camera), true);
        for (layout, size) in [(Layout::Screen, screen), (Layout::Camera, camera)] {
            let set = settings(layout, Bubble::default());
            let job = Job {
                dir: &dir,
                session: &s,
                settings: &set,
                duration_s: 2.0,
            };
            let dest = dir.join(format!("{layout:?}.mp4"));
            let done = export(&job, &dest, true, &AtomicBool::new(false), &mut |_, _| {}).unwrap();
            assert_eq!((done.facts.width, done.facts.height), size);
            let px = frame(&done.path, 1.0, size.0, size.1);
            let mid = (((size.1 / 2) * size.0 + size.0 / 2) * 3) as usize;
            let (r, b) = (px[mid], px[mid + 2]);
            match layout {
                Layout::Camera => assert!(r > 160 && b < 80, "camera {r} {b}"),
                _ => assert!(b > 150 && r < 80, "screen {r} {b}"),
            }
        }
    }

    /// Clicks only (the synthetic microphone's kind of sound): the preset can't be reached
    /// under −1 dBTP, so the export is as loud as it can be and says so.
    #[test]
    fn spiky_sound_is_held_by_its_peaks_not_distorted() {
        let dir = colour_recording("export-clicks", 4.0, (640, 360), (320, 240));
        let mut clicks = vec![0.0f32; 4 * 48_000];
        for second in 0..4 {
            // A hard tap: a 3 kHz burst dying away within 5 ms.
            for i in 0..240 {
                let t = i as f32 / 48_000.0;
                clicks[second * 48_000 + 2_400 + i] =
                    0.6 * (-t / 0.001).exp() * (2.0 * std::f32::consts::PI * 3_000.0 * t).sin();
            }
        }
        let ffmpeg = ffmpeg::locate("ffmpeg").unwrap();
        let mut e = Command::new(&ffmpeg)
            .args(sound::input_args(1))
            .args(sound::AAC_ARGS)
            .arg(dir.join("mic.mp4"))
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        use std::io::Write;
        let bytes: Vec<u8> = clicks.iter().flat_map(|s| s.to_le_bytes()).collect();
        e.stdin.take().unwrap().write_all(&bytes).unwrap();
        assert!(e.wait().unwrap().success());
        let s = session((640, 360), None, true);
        let set = settings(Layout::Screen, Bubble::default());
        let job = Job {
            dir: &dir,
            session: &s,
            settings: &set,
            duration_s: 4.0,
        };
        let done = export(
            &job,
            &dir.join("clicks.mp4"),
            true,
            &AtomicBool::new(false),
            &mut |_, _| {},
        )
        .unwrap();
        assert!(
            done.held_by_peaks,
            "{:?} {:?}",
            done.lufs, done.true_peak_db
        );
        assert!(done.lufs.unwrap() < -16.5);
        assert!(done.true_peak_db.unwrap() <= sound::MAX_TRUE_PEAK_DB + 0.05);
        assert!(!held_by_peaks(Some(-16.0), Some(-16.2), -1.1), "on target");
        assert!(
            !held_by_peaks(Some(-16.0), Some(-22.0), -6.0),
            "quiet without a reason"
        );
    }

    #[test]
    fn a_cancelled_export_leaves_nothing() {
        let dir = colour_recording("export-cancel", 2.0, (640, 360), (320, 240));
        let s = session((640, 360), None, true);
        let set = settings(Layout::Screen, Bubble::default());
        let job = Job {
            dir: &dir,
            session: &s,
            settings: &set,
            duration_s: 2.0,
        };
        let out = dir.join("out");
        fs::create_dir_all(&out).unwrap();
        let err = export(
            &job,
            &out.join("x.mp4"),
            false,
            &AtomicBool::new(true),
            &mut |_, _| {},
        )
        .unwrap_err();
        assert_eq!(err, CANCELLED);
        assert_eq!(fs::read_dir(&out).unwrap().count(), 0);
        assert!(!dir.join("export-work").exists());
    }

    /// Writes raw mono 48 kHz samples as an AAC track.
    fn write_sound(path: &Path, samples: &[f32]) {
        let mut e = Command::new(ffmpeg::locate("ffmpeg").unwrap())
            .args(sound::input_args(1))
            .args(sound::AAC_ARGS)
            .arg(path)
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        use std::io::Write;
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        e.stdin.take().unwrap().write_all(&bytes).unwrap();
        assert!(e.wait().unwrap().success());
    }

    /// The tests' oracle for sync (CLAUDE.md "Tests"): the screen flashes white on every
    /// whole second and the microphone taps at the same moments over a quiet noise floor.
    /// After a trim that starts between seconds, the voice clean-up (RNNoise and its delay
    /// removed), the loudness gain, AAC and the bubble layout, every flash in the export must
    /// still match its tap within one frame.
    #[test]
    fn picture_and_sound_stay_in_sync_through_the_export() {
        let dir = crate::sessions::tests::temp_root("export-sync");
        let ffmpeg = ffmpeg::locate("ffmpeg").unwrap();
        for (file, colour, size) in [
            ("screen.mp4", "black", "640x360"),
            ("camera.mp4", "0xd02020", "320x240"),
        ] {
            let flash = if file == "screen.mp4" {
                r",drawbox=x=0:y=0:w=iw:h=ih:color=white:t=fill:enable='eq(mod(n\,30)\,0)'"
            } else {
                ""
            };
            let ok = Command::new(&ffmpeg)
                .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
                .arg(format!("color=c={colour}:s={size}:r=30:d=7{flash}"))
                .args(["-c:v", "mpeg4", "-q:v", "2", "-g", "30"])
                .arg(dir.join(file))
                .status()
                .unwrap()
                .success();
            assert!(ok);
        }
        let mut x = 11u64;
        let mut mic: Vec<f32> = (0..7 * 48_000)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                ((x >> 40) as f32 / (1u64 << 23) as f32 - 1.0) * 0.003
            })
            .collect();
        for second in 0..7 {
            for i in 0..480 {
                let t = i as f32 / 48_000.0;
                mic[second * 48_000 + i] +=
                    0.5 * (-t / 0.002).exp() * (2.0 * std::f32::consts::PI * 2_000.0 * t).sin();
            }
        }
        write_sound(&dir.join("mic.mp4"), &mic);

        let s = session((640, 360), Some((320, 240)), true);
        let mut set = settings(
            Layout::Bubble,
            Bubble {
                shape: Shape::Circle,
                size: Size::Small,
                x: 1.0,
                y: 1.0,
            },
        );
        set.start_s = 0.5;
        set.end_s = Some(6.5);
        set.clean_voice = true;
        let job = Job {
            dir: &dir,
            session: &s,
            settings: &set,
            duration_s: 7.0,
        };
        let done = export(
            &job,
            &dir.join("sync.mp4"),
            true,
            &AtomicBool::new(false),
            &mut |_, _| {},
        )
        .unwrap();

        // Flashes: frames whose top-left corner is light.
        let frames = Command::new(&ffmpeg)
            .args(["-v", "error", "-i"])
            .arg(&done.path)
            .args([
                "-vf",
                "crop=64:64:0:0",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "gray",
                "pipe:1",
            ])
            .output()
            .unwrap()
            .stdout;
        let flashes: Vec<f64> = frames
            .chunks_exact(64 * 64)
            .enumerate()
            .filter(|(_, f)| f.iter().map(|&v| v as u32).sum::<u32>() / (64 * 64) > 128)
            .map(|(i, _)| i as f64 / 30.0)
            .collect();
        assert_eq!(flashes.len(), 6, "{flashes:?}");
        // Taps: where each second's sound first reaches half its loudest.
        let sound = crate::decode::tests::read_all(&done.path, 1, RATE);
        let taps: Vec<f64> = sound
            .chunks(48_000)
            .filter_map(|second| {
                let peak = second.iter().fold(0.0f32, |m, v| m.max(v.abs()));
                second.iter().position(|v| v.abs() >= peak * 0.5)
            })
            .enumerate()
            .map(|(k, i)| k as f64 + i as f64 / 48_000.0)
            .collect();
        let mut offsets = Vec::new();
        for flash in &flashes {
            let nearest = taps
                .iter()
                .map(|t| t - flash)
                .min_by(|a, b| a.abs().total_cmp(&b.abs()))
                .unwrap();
            offsets.push(format!("{:+.1}", nearest * 1000.0));
            assert!(
                nearest.abs() <= 1.0 / 30.0,
                "flash at {flash:.3} s: tap {nearest:+.4} s away ({taps:?})"
            );
        }
        eprintln!("tap − flash in the export (ms): {}", offsets.join(" "));
        // The first flash is the recording's 1.0 s, half a second in.
        assert!((flashes[0] - 0.5).abs() < 0.02, "{flashes:?}");
    }

    /// Export speed on this computer: 10 min of a moving 1728×1080 screen with a 720p
    /// camera bubble and speech-like sound, as the dev Mac records them (VideoToolbox, the
    /// recording's bitrates). Run under `/usr/bin/time -l` for the CPU of FFmpeg too:
    /// `cargo test --lib measure_ten_minute_export -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn measure_ten_minute_export() {
        let seconds = 600.0;
        let dir = crate::sessions::tests::temp_root("export-measure");
        let ffmpeg = ffmpeg::locate("ffmpeg").unwrap();
        let started = std::time::Instant::now();
        for (file, source, w, h) in [
            ("screen.mp4", "testsrc2", 1728, 1080),
            ("camera.mp4", "testsrc", 1280, 720),
        ] {
            let ok = Command::new(&ffmpeg)
                .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
                .arg(format!("{source}=s={w}x{h}:r=30:d={seconds}"))
                .args(["-c:v", "h264_videotoolbox", "-b:v"])
                .arg(sessions::video_bitrate(w, h).to_string())
                .args(["-g", "30", "-pix_fmt", "yuv420p"])
                .arg(dir.join(file))
                .status()
                .unwrap()
                .success();
            assert!(ok);
        }
        let speech: Vec<f32> = crate::loudness::tests::speechy(seconds, -28.0)
            .into_iter()
            .step_by(2)
            .collect();
        write_sound(&dir.join("mic.mp4"), &speech);
        eprintln!(
            "made the 10 min recording in {:.0} s",
            started.elapsed().as_secs_f64()
        );

        let s = session((1728, 1080), Some((1280, 720)), true);
        let mut set = settings(Layout::Bubble, Bubble::default());
        set.clean_voice = true;
        let job = Job {
            dir: &dir,
            session: &s,
            settings: &set,
            duration_s: seconds,
        };
        let mut stages: Vec<(&str, std::time::Instant)> = Vec::new();
        let started = std::time::Instant::now();
        let done = export(
            &job,
            &dir.join("measure.mp4"),
            true,
            &AtomicBool::new(false),
            &mut |stage, _| {
                if stages.last().is_none_or(|(s, _)| *s != stage) {
                    stages.push((stage, std::time::Instant::now()));
                }
            },
        )
        .unwrap();
        let total = started.elapsed().as_secs_f64();
        let mut previous = started;
        for (stage, at) in &stages {
            eprintln!("  before {stage}: {:.1} s", (*at - previous).as_secs_f64());
            previous = *at;
        }
        eprintln!(
            "export of 10:00 took {total:.1} s ({:.1}× real time) with {}; {:.1} LUFS, {:.1} dBTP, {} MB",
            seconds / total,
            done.encoder,
            done.lufs.unwrap_or(f64::NAN),
            done.true_peak_db.unwrap_or(f64::NAN),
            fs::metadata(&done.path).unwrap().len() / 1_000_000
        );
        let _ = fs::remove_dir_all(&dir);
    }
}

//! Finish's sound (milestone 4): the microphone, cleaned up with RNNoise (blended at
//! Voice's measured Fast Strength) when the user wants it, mixed with the computer sound,
//! brought to the loudness preset by Voice's gain + true-peak limiter, and encoded as AAC.
//!
//! The clean-up is the slow part, so the cleaned microphone is kept in the recording's
//! folder (lossless FLAC) and reused. The preview plays the whole recording made this way
//! (`sound-<clean>-<loudness>.m4a`); the export makes the sound again for exactly the
//! trimmed part, so its loudness is measured on what is exported.

use std::collections::VecDeque;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use crate::decode::Decoder;
use crate::ffmpeg;
use crate::finish::Loudness;
use crate::loudness::{self, Meter, Source};
use crate::polish::{CEILING_DBTP, Chain, Stages};
use crate::rnnoise::{self, Denoiser, FRAME};

pub const RATE: u32 = 48_000;
/// Voice's Fast Strength (measured there 2026-10-21: the noise left is still 16 dB down
/// and speech keeps 17.8 dB SI-SDR instead of 16.4 at full strength).
pub const STRENGTH: f32 = 0.85;
/// The presets promise true peaks at or under this.
pub const MAX_TRUE_PEAK_DB: f64 = -1.0;
/// AAC as the export writes it.
pub const AAC_BITRATE: &str = "192k";
/// Only the loudness gain and the limiter of Voice's chain.
pub const GAIN_ONLY: Stages = Stages {
    rumble: false,
    de_ess: false,
    compress: false,
};
/// The cleaned microphone, kept for the next preview or export.
const CLEANED: &str = "voice-clean.flac";
/// Frames per block while streaming.
const BLOCK: usize = 4_800;

impl Loudness {
    pub fn lufs(self) -> Option<f64> {
        match self {
            Loudness::Social => Some(-14.0),
            Loudness::Tutorial => Some(-16.0),
            Loudness::Keep => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Loudness::Social => "social",
            Loudness::Tutorial => "tutorial",
            Loudness::Keep => "keep",
        }
    }
}

fn cancelled(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("cancelled".into())
    } else {
        Ok(())
    }
}

/// RNNoise over a mono stream, blended with the original at [`STRENGTH`], with the
/// denoiser's delay removed: output sample `i` lines up with input sample `i`.
pub struct Cleaner {
    denoiser: Denoiser,
    pending: Vec<f32>,
    /// Input samples not yet matched with their cleaned output.
    originals: VecDeque<f32>,
    /// Cleaned samples still to drop for the delay.
    skip: usize,
}

impl Default for Cleaner {
    fn default() -> Self {
        Cleaner {
            denoiser: Denoiser::new(),
            pending: Vec::with_capacity(FRAME),
            originals: VecDeque::new(),
            skip: rnnoise::DELAY,
        }
    }
}

impl Cleaner {
    pub fn push(&mut self, input: &[f32], out: &mut Vec<f32>) {
        for &s in input {
            self.pending.push(s);
            self.originals.push_back(s);
            if self.pending.len() == FRAME {
                self.frame(out);
            }
        }
    }

    fn frame(&mut self, out: &mut Vec<f32>) {
        let mut frame = [0.0f32; FRAME];
        frame.copy_from_slice(&self.pending);
        self.pending.clear();
        self.denoiser.process(&mut frame);
        for cleaned in frame {
            if self.skip > 0 {
                self.skip -= 1;
                continue;
            }
            let Some(original) = self.originals.pop_front() else {
                break;
            };
            out.push(original + (cleaned - original) * STRENGTH);
        }
    }

    /// The last samples: silence pushes them through the delay.
    pub fn finish(&mut self, out: &mut Vec<f32>) {
        while !self.originals.is_empty() {
            self.pending.push(0.0);
            if self.pending.len() == FRAME {
                self.frame(out);
            }
        }
    }
}

/// An FFmpeg that encodes raw float samples from its stdin into `output`.
struct Encoder {
    child: Child,
    stdin: Option<ChildStdin>,
    stderr: Option<JoinHandle<String>>,
}

impl Encoder {
    fn start(channels: u32, codec_args: &[&str], output: &Path) -> Result<Self, String> {
        let ffmpeg = ffmpeg::locate("ffmpeg")?;
        let mut child = Command::new(&ffmpeg)
            .args(input_args(channels))
            .args(codec_args)
            .arg(output)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("failed to run {}: {e}", ffmpeg.display()))?;
        let stdin = child.stdin.take();
        let mut stderr = child.stderr.take().unwrap();
        let stderr = std::thread::spawn(move || {
            let mut text = String::new();
            let _ = stderr.read_to_string(&mut text);
            text
        });
        Ok(Encoder {
            child,
            stdin,
            stderr: Some(stderr),
        })
    }

    fn write(&mut self, samples: &[f32]) -> Result<(), String> {
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        self.stdin
            .as_mut()
            .unwrap()
            .write_all(&bytes)
            .map_err(|e| format!("the sound couldn't be written: {e}"))
    }

    fn finish(mut self) -> Result<(), String> {
        drop(self.stdin.take());
        let status = self.child.wait().map_err(|e| e.to_string())?;
        let stderr = self
            .stderr
            .take()
            .and_then(|h| h.join().ok())
            .unwrap_or_default();
        if status.success() {
            Ok(())
        } else {
            Err(format!(
                "FFmpeg couldn't write the sound: {}",
                stderr.trim()
            ))
        }
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        if self.stdin.is_some() {
            drop(self.stdin.take());
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// The encoder's input: raw 48 kHz floats on stdin.
pub fn input_args(channels: u32) -> Vec<String> {
    [
        "-nostdin",
        "-v",
        "error",
        "-y",
        "-f",
        "f32le",
        "-ar",
        "48000",
        "-ac",
        &channels.to_string(),
        "-i",
        "pipe:0",
    ]
    .map(String::from)
    .to_vec()
}

/// Lossless, for the kept cleaned microphone.
pub const FLAC_ARGS: [&str; 4] = ["-c:a", "flac", "-sample_fmt", "s16"];
/// AAC 48 kHz stereo, as exported.
pub const AAC_ARGS: [&str; 8] = [
    "-c:a",
    "aac",
    "-b:a",
    AAC_BITRATE,
    "-movflags",
    "+faststart",
    "-f",
    "mp4",
];

/// The microphone cleaned up (kept in the recording's folder; made once). `frames` is
/// the recording's length, for progress.
pub fn cleaned_mic(
    dir: &Path,
    frames: u64,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f64),
) -> Result<PathBuf, String> {
    let path = dir.join(CLEANED);
    if path.is_file() {
        return Ok(path);
    }
    let temp = dir.join("voice-clean.flac.part");
    let result = (|| {
        let mut decoder = Decoder::open(&dir.join("mic.mp4"), 1, RATE)?;
        let mut encoder = Encoder::start(1, &[&FLAC_ARGS[..], &["-f", "flac"]].concat(), &temp)?;
        let mut cleaner = Cleaner::default();
        let (mut input, mut output) = (Vec::new(), Vec::new());
        let mut done = 0u64;
        loop {
            cancelled(cancel)?;
            input.clear();
            output.clear();
            if decoder.read(&mut input, BLOCK)? == 0 {
                cleaner.finish(&mut output);
                encoder.write(&output)?;
                break;
            }
            cleaner.push(&input, &mut output);
            encoder.write(&output)?;
            done += input.len() as u64;
            progress((done as f64 / frames.max(1) as f64).min(1.0));
        }
        encoder.finish()
    })();
    match result {
        Ok(()) => fs::rename(&temp, &path)
            .map_err(|e| e.to_string())
            .map(|_| path),
        Err(e) => {
            let _ = fs::remove_file(&temp);
            Err(e)
        }
    }
}

/// The recording's sound for a stretch of it, as stereo 48 kHz: the voice (the cleaned or
/// recorded microphone, mono, on both sides) plus the computer sound. Exactly `frames`
/// long from `start` (silence where the tracks end early).
pub struct Mix {
    pub voice: Option<PathBuf>,
    pub system: Option<PathBuf>,
    pub start: u64,
    pub frames: u64,
}

impl Source for Mix {
    fn channels(&self) -> usize {
        2
    }

    fn frames(&self) -> u64 {
        self.frames
    }

    fn for_each_block(
        &self,
        each: &mut dyn FnMut(&[f32]) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut voice = self
            .voice
            .as_ref()
            .map(|p| Decoder::open(p, 1, RATE))
            .transpose()?;
        let mut system = self
            .system
            .as_ref()
            .map(|p| Decoder::open(p, 2, RATE))
            .transpose()?;
        let (mut v, mut s, mut out) = (Vec::new(), Vec::new(), Vec::new());
        let end = self.start + self.frames;
        let mut position = 0u64;
        while position < end {
            let want = BLOCK.min((end - position) as usize);
            v.clear();
            s.clear();
            if let Some(d) = voice.as_mut() {
                d.read(&mut v, want)?;
            }
            if let Some(d) = system.as_mut() {
                d.read(&mut s, want * 2)?;
            }
            // Skip what lies before the start.
            let skip = self.start.saturating_sub(position).min(want as u64) as usize;
            out.clear();
            for i in skip..want {
                let mono = v.get(i).copied().unwrap_or(0.0);
                out.push(mono + s.get(2 * i).copied().unwrap_or(0.0));
                out.push(mono + s.get(2 * i + 1).copied().unwrap_or(0.0));
            }
            if !out.is_empty() {
                each(&out)?;
            }
            position += want as u64;
        }
        Ok(())
    }
}

/// What making the sound measured: before, and the encoded result read back.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct Made {
    pub input_lufs: Option<f64>,
    pub lufs: Option<f64>,
    pub true_peak_db: f64,
}

/// Loudness and true peak of an encoded file (decoded as stereo 48 kHz).
pub fn read_back(path: &Path) -> Result<(Option<f64>, f64), String> {
    let mut decoder = Decoder::open(path, 2, RATE)?;
    let mut meter = Meter::new(2, true)?;
    let mut block = Vec::new();
    loop {
        block.clear();
        if decoder.read(&mut block, BLOCK * 2)? == 0 {
            break;
        }
        meter.add(&block)?;
    }
    Ok((meter.lufs(), meter.true_peak_db(2)))
}

/// Makes `mix` at `target` LUFS (`None`: keep its loudness) into `output` (AAC in MP4).
/// AAC can push true peaks a little over the limiter's ceiling, so the result is read
/// back and, when over −1 dBTP, made again with the ceiling lowered by the excess (as
/// Voice's export does).
pub fn make(
    mix: &Mix,
    target: Option<f64>,
    output: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f64),
) -> Result<Made, String> {
    let measured = loudness::measure(mix, target, GAIN_ONLY, cancel, &mut |f| progress(f * 0.6))?;
    let mut ceiling = CEILING_DBTP;
    for attempt in 0..3 {
        let mut encoder = Encoder::start(2, &AAC_ARGS, output)?;
        let mut chain = Chain::new(2, measured.params);
        chain.set_ceiling_db(ceiling);
        let mut skip = chain.latency() * 2;
        let mut buffer = Vec::new();
        let mut done = 0u64;
        let base = 0.6 + attempt as f64 * 0.1;
        mix.for_each_block(&mut |block| {
            cancelled(cancel)?;
            buffer.clear();
            buffer.extend_from_slice(block);
            chain.process(&mut buffer);
            let drop = skip.min(buffer.len());
            skip -= drop;
            encoder.write(&buffer[drop..])?;
            done += (block.len() / 2) as u64;
            progress((base + 0.3 * done as f64 / mix.frames.max(1) as f64).min(0.99));
            Ok(())
        })?;
        let mut tail = Vec::new();
        chain.flush(&mut tail);
        encoder.write(&tail[skip.min(tail.len())..])?;
        encoder.finish()?;
        let (lufs, true_peak_db) = read_back(output)?;
        if true_peak_db <= MAX_TRUE_PEAK_DB || attempt == 2 {
            progress(1.0);
            return Ok(Made {
                input_lufs: measured.input_lufs,
                lufs,
                true_peak_db,
            });
        }
        ceiling -= (true_peak_db - MAX_TRUE_PEAK_DB) as f32 + 0.1;
    }
    unreachable!()
}

/// The preview's sound file for these choices.
pub fn preview_path(dir: &Path, clean: bool, loudness: Loudness) -> PathBuf {
    dir.join(format!(
        "sound-{}-{}.m4a",
        if clean { "clean" } else { "plain" },
        loudness.name()
    ))
}

/// Where the voice comes from: the cleaned microphone (made if needed), the recorded one,
/// or none.
pub fn voice(
    dir: &Path,
    clean: bool,
    frames: u64,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f64),
) -> Result<Option<PathBuf>, String> {
    let mic = dir.join("mic.mp4");
    if !mic.is_file() {
        return Ok(None);
    }
    if clean {
        cleaned_mic(dir, frames, cancel, progress).map(Some)
    } else {
        Ok(Some(mic))
    }
}

/// The whole recording's sound with these choices, for the preview: made once per
/// choice (the previous choice's file is removed). `None` without any sound track.
pub fn preview(
    dir: &Path,
    duration_s: f64,
    clean: bool,
    loudness: Loudness,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&'static str, f64),
) -> Result<Option<PathBuf>, String> {
    let path = preview_path(dir, clean, loudness);
    let system = Some(dir.join("system.mp4")).filter(|p| p.is_file());
    let mic = dir.join("mic.mp4").is_file();
    if !mic && system.is_none() {
        return Ok(None);
    }
    if path.is_file() {
        return Ok(Some(path));
    }
    let frames = (duration_s * RATE as f64).round() as u64;
    let voice = voice(dir, clean, frames, cancel, &mut |f| progress("clean", f))?;
    let temp = path.with_extension("m4a.part");
    let mix = Mix {
        voice,
        system,
        start: 0,
        frames,
    };
    if let Err(e) = make(&mix, loudness.lufs(), &temp, cancel, &mut |f| {
        progress("loudness", f)
    }) {
        let _ = fs::remove_file(&temp);
        return Err(e);
    }
    fs::rename(&temp, &path).map_err(|e| e.to_string())?;
    // Only the current choice's file is kept.
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with("sound-") && entry.path() != path {
            let _ = fs::remove_file(entry.path());
        }
    }
    Ok(Some(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::tests::read_all;
    use crate::loudness::tests::speechy;

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|s| s * s).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    #[test]
    fn every_command_is_lgpl() {
        for args in [
            [input_args(1), FLAC_ARGS.map(String::from).to_vec()].concat(),
            [input_args(2), AAC_ARGS.map(String::from).to_vec()].concat(),
        ] {
            assert_eq!(ffmpeg::gpl_only(&args), None, "{args:?}");
        }
    }

    #[test]
    fn the_cleaner_keeps_time_and_length() {
        // Speech-like bursts: cleaned and blended, the result lines up with the input and
        // keeps its length; steady noise between the bursts gets quieter.
        let speech: Vec<f32> = speechy(3.0, -20.0).into_iter().step_by(2).collect();
        let mut cleaner = Cleaner::default();
        let mut out = Vec::new();
        for chunk in speech.chunks(1000) {
            cleaner.push(chunk, &mut out);
        }
        cleaner.finish(&mut out);
        assert_eq!(out.len(), speech.len());
        // Lined up: the best match is at lag 0.
        let corr = |lag: usize| -> f32 {
            speech[48_000..96_000]
                .iter()
                .zip(&out[48_000 + lag..96_000 + lag])
                .map(|(a, b)| a * b)
                .sum()
        };
        let best = (0..200)
            .max_by(|&a, &b| corr(a).total_cmp(&corr(b)))
            .unwrap();
        assert!(best <= 1, "lag {best}");
    }

    #[test]
    fn the_cleaner_lowers_noise_by_the_strength() {
        let mut x = 7u64;
        let noise: Vec<f32> = (0..48_000 * 3)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                ((x >> 40) as f32 / (1u64 << 23) as f32 - 1.0) * 0.05
            })
            .collect();
        let mut cleaner = Cleaner::default();
        let mut out = Vec::new();
        cleaner.push(&noise, &mut out);
        cleaner.finish(&mut out);
        // At 85 % at most 15 % of the noise stays (−16.5 dB) plus what RNNoise leaves.
        let (before, after) = (rms(&noise[96_000..]), rms(&out[96_000..]));
        assert!(after < before * 0.2, "{before} -> {after}");
        assert!(after > before * 0.1, "the blend keeps 15 %: {after}");
    }

    /// A recording folder with a mic track of speech-like sound (and computer sound).
    fn recording(name: &str, seconds: f64, level_db: f32, system: bool) -> PathBuf {
        let dir = crate::sessions::tests::temp_root(name);
        let wav = |file: &str, samples: &[f32], channels: u32| {
            let mut e = Encoder::start(channels, &AAC_ARGS, &dir.join(file)).unwrap();
            e.write(samples).unwrap();
            e.finish().unwrap();
        };
        let stereo = speechy(seconds, level_db);
        let mono: Vec<f32> = stereo.iter().copied().step_by(2).collect();
        wav("mic.mp4", &mono, 1);
        if system {
            let quiet: Vec<f32> = stereo.iter().map(|s| s * 0.1).collect();
            wav("system.mp4", &quiet, 2);
        }
        dir
    }

    #[test]
    fn the_preview_sound_meets_each_preset() {
        let dir = recording("sound-preview", 12.0, -30.0, true);
        let cancel = AtomicBool::new(false);
        for (clean, loudness) in [
            (true, Loudness::Tutorial),
            (false, Loudness::Social),
            (true, Loudness::Keep),
        ] {
            let path = preview(&dir, 12.0, clean, loudness, &cancel, &mut |_, _| {})
                .unwrap()
                .unwrap();
            assert_eq!(path, preview_path(&dir, clean, loudness));
            let (lufs, peak) = read_back(&path).unwrap();
            if let Some(target) = loudness.lufs() {
                assert!(
                    (lufs.unwrap() - target).abs() <= 0.5,
                    "{loudness:?}: {lufs:?}"
                );
            }
            assert!(peak <= MAX_TRUE_PEAK_DB, "{loudness:?}: {peak}");
            // Full length (AAC adds no more than a frame).
            let samples = read_all(&path, 2, RATE);
            assert!(
                (samples.len() as f64 / 2.0 - 12.0 * 48_000.0).abs() < 2048.0,
                "{}",
                samples.len()
            );
        }
        assert!(dir.join(CLEANED).is_file(), "the cleaned mic is kept");
        let sounds = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("sound-"))
            .count();
        assert_eq!(sounds, 1, "only the current choice's file is kept");
    }

    #[test]
    fn a_stretch_is_cut_exactly() {
        let dir = recording("sound-stretch", 6.0, -20.0, false);
        let whole = read_all(&dir.join("mic.mp4"), 1, RATE);
        let mix = Mix {
            voice: Some(dir.join("mic.mp4")),
            system: None,
            start: 48_000 + 123,
            frames: 96_000,
        };
        let mut got = Vec::new();
        mix.for_each_block(&mut |b| {
            got.extend_from_slice(b);
            Ok(())
        })
        .unwrap();
        assert_eq!(got.len(), 96_000 * 2);
        for i in [0, 1000, 50_000, 95_999] {
            assert_eq!(got[2 * i], whole[48_123 + i]);
            assert_eq!(got[2 * i + 1], whole[48_123 + i]);
        }
        // Past the end: silence fills to the length asked for.
        let late = Mix {
            start: 5 * 48_000,
            frames: 96_000,
            ..mix
        };
        let mut n = 0;
        late.for_each_block(&mut |b| {
            n += b.len();
            Ok(())
        })
        .unwrap();
        assert_eq!(n, 96_000 * 2);
    }
}

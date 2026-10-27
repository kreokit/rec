//! A recording's sound as 32-bit float samples, streamed from FFmpeg (from KreoKit Clips'
//! `AudioStream`): Finish reads the microphone and computer sound for the waveform and the
//! voice processing without ever holding a long recording in memory or writing a WAV.

use std::io::{BufReader, Read};
use std::path::Path;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::thread::JoinHandle;

use crate::ffmpeg;

/// The arguments after the input: `channels` × `rate` raw floats on stdout. Silence fills
/// in before sound that starts after the recording's 0:00, so sample 0 is 0:00 on the
/// recording's clock, as in the player.
pub fn output_args(channels: u32, rate: u32) -> Vec<String> {
    [
        "-vn",
        "-ac",
        &channels.to_string(),
        "-ar",
        &rate.to_string(),
        "-af",
        "aresample=async=1:first_pts=0",
        "-f",
        "f32le",
        "-acodec",
        "pcm_f32le",
        "pipe:1",
    ]
    .map(String::from)
    .to_vec()
}

/// A running FFmpeg that decodes a file's sound. Dropping it stops FFmpeg.
pub struct Decoder {
    child: Child,
    stdout: BufReader<ChildStdout>,
    stderr: Option<JoinHandle<String>>,
    /// Bytes of a sample split across two reads.
    pending: Vec<u8>,
    finished: bool,
}

impl Decoder {
    /// Interleaved samples, `channels` × `rate`.
    pub fn open(path: &Path, channels: u32, rate: u32) -> Result<Self, String> {
        let ffmpeg = ffmpeg::locate("ffmpeg")?;
        let mut child = Command::new(&ffmpeg)
            .args(["-nostdin", "-v", "error", "-i"])
            .arg(path)
            .args(output_args(channels, rate))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("failed to run {}: {e}", ffmpeg.display()))?;
        let stdout = BufReader::with_capacity(1 << 20, child.stdout.take().unwrap());
        // Read stderr on the side so a chatty FFmpeg can never block on a full pipe.
        let mut stderr = child.stderr.take().unwrap();
        let stderr = std::thread::spawn(move || {
            let mut text = String::new();
            let _ = stderr.read_to_string(&mut text);
            text
        });
        Ok(Self {
            child,
            stdout,
            stderr: Some(stderr),
            pending: Vec::new(),
            finished: false,
        })
    }

    /// Appends up to `max` samples to `out`; returns how many. 0 means the sound ended
    /// (and FFmpeg succeeded; a decoding error is returned instead).
    pub fn read(&mut self, out: &mut Vec<f32>, max: usize) -> Result<usize, String> {
        if self.finished || max == 0 {
            return Ok(0);
        }
        let mut bytes = std::mem::take(&mut self.pending);
        let want = max * 4;
        let start = bytes.len();
        bytes.resize(want, 0);
        let mut filled = start;
        while filled < want {
            match self.stdout.read(&mut bytes[filled..]) {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(format!("cannot read the sound: {e}")),
            }
        }
        bytes.truncate(filled);
        let whole = filled / 4 * 4;
        self.pending = bytes.split_off(whole);
        let before = out.len();
        out.extend(
            bytes
                .chunks_exact(4)
                .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
        );
        let n = out.len() - before;
        if n == 0 {
            self.finished = true;
            self.finish()?;
        }
        Ok(n)
    }

    /// Waits for FFmpeg and turns a failure into a readable error.
    fn finish(&mut self) -> Result<(), String> {
        let status = self.child.wait().map_err(|e| e.to_string())?;
        let stderr = self
            .stderr
            .take()
            .and_then(|h| h.join().ok())
            .unwrap_or_default();
        if status.success() {
            Ok(())
        } else {
            Err(format!("FFmpeg couldn't read the sound: {}", stderr.trim()))
        }
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// All of a file's sound at once (tests only; the app streams it).
    pub fn read_all(path: &Path, channels: u32, rate: u32) -> Vec<f32> {
        let mut decoder = Decoder::open(path, channels, rate).unwrap();
        let mut out = Vec::new();
        while decoder.read(&mut out, 1 << 16).unwrap() > 0 {}
        out
    }

    #[test]
    fn the_command_is_lgpl() {
        assert_eq!(ffmpeg::gpl_only(&output_args(2, 48_000)), None);
    }

    #[test]
    fn decodes_a_recordings_sound() {
        let dir = crate::sessions::tests::temp_root("decode");
        crate::sessions::tests::make_tracks(&dir, 2.0, true, false);
        let samples = read_all(&dir.join("mic.mp4"), 1, 16_000);
        assert!(
            (31_000..=33_500).contains(&samples.len()),
            "{}",
            samples.len()
        );
        assert!(samples.iter().any(|s| s.abs() > 0.05), "there is sound");
    }
}

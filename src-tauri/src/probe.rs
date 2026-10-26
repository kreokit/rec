//! `probe`: what a recorded track file contains, read with ffprobe (from KreoKit Voice and
//! Clips). The after-stop check and recovery need each stream's start, length and frame
//! count, and the encoder settings the export must keep.

use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::ffmpeg;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaInfo {
    /// The container's length (all streams).
    pub duration_s: f64,
    pub streams: Vec<StreamInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamInfo {
    /// "video" or "audio".
    pub kind: String,
    pub codec: String,
    /// e.g. "High" for H.264, "LC" for AAC.
    pub profile: Option<String>,
    /// Where the stream starts on the file's timeline, in seconds.
    pub start_s: f64,
    pub duration_s: f64,
    /// Packets read: frames for video, AAC frames (1024 samples) for audio.
    pub packets: u64,
    // Video
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub pix_fmt: Option<String>,
    /// [matrix, primaries, transfer], e.g. "bt709" each.
    pub color: Option<[String; 3]>,
    // Audio
    pub sample_rate: Option<u32>,
    pub channels: Option<u32>,
}

impl MediaInfo {
    pub fn video(&self) -> Option<&StreamInfo> {
        self.streams.iter().find(|s| s.kind == "video")
    }

    pub fn audio(&self) -> Option<&StreamInfo> {
        self.streams.iter().find(|s| s.kind == "audio")
    }
}

pub fn probe(path: &Path) -> Result<MediaInfo, String> {
    if !path.is_file() {
        return Err(format!("{} is missing", path.display()));
    }
    let ffprobe = ffmpeg::locate("ffprobe")?;
    let output = Command::new(&ffprobe)
        .args(probe_args())
        .arg(path)
        .output()
        .map_err(|e| format!("failed to run {}: {e}", ffprobe.display()))?;
    if !output.status.success() {
        return Err(format!(
            "{} can't be read ({})",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    parse_ffprobe_json(&String::from_utf8_lossy(&output.stdout))
}

/// Whole sections rather than -show_entries (section names differ between FFmpeg
/// versions); `-count_packets` reads the file through to count the frames.
fn probe_args() -> [&'static str; 7] {
    [
        "-v",
        "error",
        "-count_packets",
        "-show_streams",
        "-show_format",
        "-of",
        "json",
    ]
}

#[derive(Deserialize)]
struct FfprobeOutput {
    #[serde(default)]
    streams: Vec<FfprobeStream>,
    format: Option<FfprobeFormat>,
}

#[derive(Deserialize)]
struct FfprobeStream {
    codec_type: Option<String>,
    codec_name: Option<String>,
    profile: Option<String>,
    start_time: Option<String>,
    duration: Option<String>,
    nb_read_packets: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    pix_fmt: Option<String>,
    color_space: Option<String>,
    color_primaries: Option<String>,
    color_transfer: Option<String>,
    sample_rate: Option<String>,
    channels: Option<u32>,
}

#[derive(Deserialize)]
struct FfprobeFormat {
    duration: Option<String>,
}

fn number(value: &Option<String>) -> Option<f64> {
    value
        .as_deref()?
        .parse::<f64>()
        .ok()
        .filter(|s| s.is_finite())
}

/// Parses the JSON printed by the ffprobe invocation in [`probe`].
pub fn parse_ffprobe_json(json: &str) -> Result<MediaInfo, String> {
    let parsed: FfprobeOutput =
        serde_json::from_str(json).map_err(|e| format!("unexpected ffprobe output: {e}"))?;
    let streams = parsed
        .streams
        .iter()
        .filter(|s| matches!(s.codec_type.as_deref(), Some("video" | "audio")))
        .map(|s| {
            let color = match (&s.color_space, &s.color_primaries, &s.color_transfer) {
                (Some(m), Some(p), Some(t)) => Some([m.clone(), p.clone(), t.clone()]),
                _ => None,
            };
            StreamInfo {
                kind: s.codec_type.clone().unwrap_or_default(),
                codec: s.codec_name.clone().unwrap_or_default(),
                profile: s.profile.clone(),
                start_s: number(&s.start_time).unwrap_or(0.0),
                duration_s: number(&s.duration).unwrap_or(0.0),
                packets: number(&s.nb_read_packets).unwrap_or(0.0) as u64,
                width: s.width,
                height: s.height,
                pix_fmt: s.pix_fmt.clone(),
                color,
                sample_rate: number(&s.sample_rate).map(|r| r as u32),
                channels: s.channels,
            }
        })
        .collect::<Vec<_>>();
    if streams.is_empty() {
        return Err("the file has no picture or sound".into());
    }
    let duration_s = parsed
        .format
        .as_ref()
        .and_then(|f| number(&f.duration))
        .or_else(|| streams.iter().map(|s| s.duration_s).reduce(f64::max))
        .unwrap_or(0.0);
    Ok(MediaInfo {
        duration_s,
        streams,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ffprobe -count_packets output for a recorder screen track (VideoToolbox H.264).
    const SCREEN: &str = r#"{
        "streams": [{
            "codec_type": "video", "codec_name": "h264", "profile": "High",
            "width": 1920, "height": 1080, "pix_fmt": "yuv420p",
            "color_space": "bt709", "color_primaries": "bt709", "color_transfer": "bt709",
            "start_time": "0.000000", "duration": "10.000000", "nb_read_packets": "300"
        }],
        "format": { "duration": "10.000000" }
    }"#;

    #[test]
    fn parses_a_screen_track() {
        let info = parse_ffprobe_json(SCREEN).unwrap();
        let video = info.video().unwrap();
        assert_eq!(video.packets, 300);
        assert_eq!((video.width, video.height), (Some(1920), Some(1080)));
        assert_eq!(video.profile.as_deref(), Some("High"));
        assert_eq!(
            video.color,
            Some(["bt709".into(), "bt709".into(), "bt709".into()])
        );
        assert_eq!(info.duration_s, 10.0);
        assert!(info.audio().is_none());
    }

    #[test]
    fn parses_a_mic_track() {
        let json = r#"{
            "streams": [{ "codec_type": "audio", "codec_name": "aac", "profile": "LC",
                          "sample_rate": "48000", "channels": 1, "start_time": "0.021333",
                          "duration": "9.984000", "nb_read_packets": "469" }],
            "format": {}
        }"#;
        let info = parse_ffprobe_json(json).unwrap();
        let audio = info.audio().unwrap();
        assert_eq!((audio.sample_rate, audio.channels), (Some(48_000), Some(1)));
        assert_eq!(audio.packets, 469);
        assert!((audio.start_s - 0.021333).abs() < 1e-9);
        // No container duration: the longest stream's.
        assert_eq!(info.duration_s, 9.984);
    }

    #[test]
    fn rejects_empty_files_and_bad_json() {
        assert!(parse_ffprobe_json(r#"{"streams": [{"codec_type": "data"}]}"#).is_err());
        assert!(parse_ffprobe_json("not json").is_err());
        assert!(probe(Path::new("/no/such/file.mp4")).is_err());
    }

    #[test]
    fn the_probe_command_is_lgpl() {
        assert_eq!(ffmpeg::gpl_only(&probe_args()), None);
    }

    /// Needs ffmpeg and ffprobe: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn probes_a_real_file() {
        let dir = std::env::temp_dir().join("kreokit-rec-probe-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("clip.mp4");
        let status = Command::new(ffmpeg::locate("ffmpeg").unwrap())
            .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
            .arg("testsrc=size=320x240:rate=30:duration=2")
            .args(["-f", "lavfi", "-i", "sine=sample_rate=48000:duration=2"])
            .args(["-c:v", "mpeg4", "-c:a", "aac", "-ac", "1"])
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
        let info = probe(&path).unwrap();
        assert_eq!(info.video().unwrap().packets, 60);
        assert_eq!(info.audio().unwrap().sample_rate, Some(48_000));
        assert!((info.duration_s - 2.0).abs() < 0.1, "{}", info.duration_s);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

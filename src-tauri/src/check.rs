//! The check after every recording (and after a recovery): every track read back with
//! ffprobe, each picture track has the frames the recorder wrote, the camera starts and
//! ends with the screen (within one frame), and the sound starts and ends with the picture
//! (within one frame, plus one AAC frame at the end since AAC stores sound in 1024-sample
//! blocks), and the screen picture isn't black all through.

use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::ffmpeg;
use crate::native::Status;
use crate::probe::{self, MediaInfo};

/// One AAC frame at 48 kHz.
const AAC_FRAME_S: f64 = 1024.0 / 48_000.0;

/// Every track a recording can have: file, picture or sound, and its name for the user.
pub const TRACKS: [(&str, bool, &str); 3] = [
    ("screen.mp4", true, "screen"),
    ("camera.mp4", true, "camera"),
    ("mic.mp4", false, "microphone"),
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrackCheck {
    pub codec: String,
    pub start_s: f64,
    pub duration_s: f64,
    /// Frames (picture) or AAC frames (sound).
    pub packets: u64,
}

impl TrackCheck {
    pub fn end_s(&self) -> f64 {
        self.start_s + self.duration_s
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Check {
    /// The longest track's length.
    pub duration_s: f64,
    pub screen: Option<TrackCheck>,
    pub camera: Option<TrackCheck>,
    pub mic: Option<TrackCheck>,
    /// Sound minus picture (the screen, or the camera without one) at the start and at the
    /// end, in ms.
    pub start_offset_ms: Option<f64>,
    pub end_offset_ms: Option<f64>,
    /// Camera minus screen at the start and at the end, in ms.
    pub camera_start_offset_ms: Option<f64>,
    pub camera_end_offset_ms: Option<f64>,
    /// The recorder's own counts (not known after a recovery).
    pub recorder: Option<Status>,
    /// What is wrong with the recording, for the user.
    pub problems: Vec<String>,
    /// Worth knowing, not wrong.
    pub notes: Vec<String>,
}

impl Check {
    /// At least one track can be played.
    pub fn playable(&self) -> bool {
        self.tracks().next().is_some()
    }

    /// The tracks that can be played.
    pub fn tracks(&self) -> impl Iterator<Item = &TrackCheck> {
        [&self.screen, &self.camera, &self.mic]
            .into_iter()
            .flatten()
    }

    /// The track in `file` (one of [`TRACKS`]), if it can be played.
    pub fn get(&self, file: &str) -> Option<&TrackCheck> {
        match file {
            "screen.mp4" => self.screen.as_ref(),
            "camera.mp4" => self.camera.as_ref(),
            _ => self.mic.as_ref(),
        }
    }

    fn slot(&mut self, file: &str) -> &mut Option<TrackCheck> {
        match file {
            "screen.mp4" => &mut self.screen,
            "camera.mp4" => &mut self.camera,
            _ => &mut self.mic,
        }
    }
}

fn track(info: &MediaInfo, video: bool) -> Option<TrackCheck> {
    let stream = if video { info.video() } else { info.audio() }?;
    Some(TrackCheck {
        codec: stream.codec.clone(),
        start_s: stream.start_s,
        duration_s: if stream.duration_s > 0.0 {
            stream.duration_s
        } else {
            info.duration_s
        },
        packets: stream.packets,
    })
}

fn round_ms(seconds: f64) -> f64 {
    (seconds * 10_000.0).round() / 10.0
}

/// A picture track has fewer frames than the recorder wrote.
fn missing(problems: &mut Vec<String>, name: &str, track: &Option<TrackCheck>, written: i64) {
    if let Some(t) = track {
        let expected = written.max(0) as u64;
        if t.packets + 1 < expected {
            problems.push(format!(
                "{} of {expected} frames are missing from the {name} track.",
                expected - t.packets
            ));
        }
    }
}

/// Checks the tracks in `dir`. `status` is the recorder's final status (`None` after a
/// recovery); `fps` the picture's frame rate.
pub fn check(dir: &Path, status: Option<&Status>, fps: u32) -> Check {
    let mut c = Check {
        recorder: status.copied(),
        ..Default::default()
    };
    let frame = 1.0 / fps as f64;
    for (file, video, name) in TRACKS {
        let path = dir.join(file);
        if !path.exists() {
            continue;
        }
        match probe::probe(&path) {
            Ok(info) => match track(&info, video) {
                Some(t) if t.packets > 0 => *c.slot(file) = Some(t),
                _ => c.problems.push(format!("The {name} track is empty.")),
            },
            Err(_) => c.problems.push(format!("The {name} track can't be read.")),
        }
    }
    c.duration_s = c.tracks().map(TrackCheck::end_s).fold(0.0, f64::max);

    if let (Some(s), Some(cam)) = (&c.screen, &c.camera) {
        let start = cam.start_s - s.start_s;
        let end = cam.end_s() - s.end_s();
        c.camera_start_offset_ms = Some(round_ms(start));
        c.camera_end_offset_ms = Some(round_ms(end));
        if start.abs() > frame + 1e-3 || end.abs() > frame + 1e-3 {
            c.problems.push(format!(
                "Screen and camera don't line up: the camera starts {:+.0} ms and ends {:+.0} ms from the screen.",
                start * 1000.0,
                end * 1000.0
            ));
        }
    }
    if let (Some(p), Some(m)) = (c.screen.as_ref().or(c.camera.as_ref()), &c.mic) {
        let start = m.start_s - p.start_s;
        let end = m.end_s() - p.end_s();
        c.start_offset_ms = Some(round_ms(start));
        c.end_offset_ms = Some(round_ms(end));
        if start.abs() > frame + 1e-3 || end.abs() > frame + AAC_FRAME_S + 1e-3 {
            c.problems.push(format!(
                "Picture and sound don't line up: the sound starts {:+.0} ms and ends {:+.0} ms from the picture.",
                start * 1000.0,
                end * 1000.0
            ));
        }
    }

    // A capture that records black (a region through CGDisplayStream's source rectangle
    // on macOS 12 did, 2026-10-27; a missing permission can too) must not pass unnoticed.
    if let Some(s) = &c.screen
        && looks_black(&dir.join("screen.mp4"), s) == Some(true)
    {
        c.problems.push(
            "The screen picture is black. Check that KreoKit Rec may record the screen (Screen Recording in the system's privacy settings). If it may and you recorded a region, record the whole display instead and tell us with Report a problem.".into(),
        );
    }

    if let Some(st) = status {
        missing(
            &mut c.problems,
            "screen",
            &c.screen,
            st.frames - st.frames_dropped,
        );
        missing(
            &mut c.problems,
            "camera",
            &c.camera,
            st.camera_frames - st.camera_frames_dropped,
        );
        if st.frames_dropped > 0 {
            c.notes.push(format!(
                "{} screen frames couldn't be saved in time (the computer was busy); the picture skips there, the sound stays in sync.",
                st.frames_dropped
            ));
        }
        if st.camera_frames_dropped > 0 {
            c.notes.push(format!(
                "{} camera frames couldn't be saved in time (the computer was busy); the camera skips there, the sound stays in sync.",
                st.camera_frames_dropped
            ));
        }
        // The camera itself skipped pictures (too little light for 30 fps, a busy USB bus):
        // the last picture is kept there, so the timing stays right.
        let lost = st.camera_frames_lost as f64 / fps as f64;
        if lost >= 0.5 {
            c.notes.push(format!(
                "The camera sent no new picture for {lost:.1} s in total; the last picture is kept there. More light or another USB port can help."
            ));
        }
        let filled = st.audio_filled as f64 / 48_000.0;
        if filled >= 0.05 {
            c.notes.push(format!(
                "The microphone sent no sound for {filled:.1} s; silence fills that time so the sound stays in sync."
            ));
        }
        let dropped = st.audio_dropped as f64 / 48_000.0;
        if dropped >= 0.05 {
            c.notes.push(format!(
                "{dropped:.1} s of microphone sound couldn't be saved in time (the computer was busy)."
            ));
        }
    }
    c
}

/// Whether a picture track is black all through: frames at a quarter, half and three
/// quarters of it, each at least 97 % black (the mouse pointer alone can be drawn on a black
/// picture). `None` when it can't be read.
fn looks_black(path: &Path, track: &TrackCheck) -> Option<bool> {
    let ffmpeg = ffmpeg::locate("ffmpeg").ok()?;
    for part in [0.25, 0.5, 0.75] {
        let at = track.start_s + track.duration_s * part;
        let output = Command::new(&ffmpeg)
            .args(["-v", "error", "-ss", &format!("{at:.3}"), "-i"])
            .arg(path)
            .args(["-frames:v", "1", "-vf", "scale=64:36", "-f", "rawvideo"])
            .args(["-pix_fmt", "gray", "-"])
            .output()
            .ok()?;
        let pixels = output.stdout;
        if !output.status.success() || pixels.is_empty() {
            return None;
        }
        // Video-range black is 16.
        let black = pixels.iter().filter(|&&v| v <= 24).count();
        if black * 100 < pixels.len() * 97 {
            return Some(false);
        }
    }
    Some(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::tests::{make_camera, make_tracks, temp_root};

    /// Needs ffmpeg: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn flags_a_black_screen_picture() {
        let dir = temp_root("check-black");
        make_tracks(&dir, 2.0, false, false);
        let c = check(&dir, None, 30);
        assert!(c.problems.is_empty(), "{c:?}");
        // Black with only a small "mouse pointer" drawn, as the macOS 12 region capture did.
        let status = Command::new(ffmpeg::locate("ffmpeg").unwrap())
            .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
            .arg("color=black:size=608x128:rate=30:duration=2")
            .args(["-vf", "drawbox=x=333:y=99:w=40:h=28:color=white:t=fill"])
            .args(["-c:v", "mpeg4", "-q:v", "2"])
            .arg(dir.join("screen.mp4"))
            .status()
            .unwrap();
        assert!(status.success());
        let c = check(&dir, None, 30);
        assert_eq!(c.problems.len(), 1, "{c:?}");
        assert!(
            c.problems[0].starts_with("The screen picture is black."),
            "{c:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Needs ffmpeg: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn passes_matching_tracks_and_flags_a_mismatch() {
        let dir = temp_root("check");
        make_tracks(&dir, 3.0, true, false);
        let status = Status {
            elapsed: 3.0,
            frames: 90,
            ..Default::default()
        };
        let c = check(&dir, Some(&status), 30);
        assert!(c.problems.is_empty(), "{c:?}");
        assert!(c.playable());
        assert_eq!(c.screen.as_ref().unwrap().packets, 90);
        assert!(
            c.end_offset_ms.unwrap().abs() <= 1000.0 / 30.0 + 21.4,
            "{c:?}"
        );

        // The recorder wrote 120 frames, the file has 90; and the sound is a second short.
        let status = Status {
            frames: 120,
            audio_filled: 48_000,
            ..status
        };
        std::fs::remove_file(dir.join("mic.mp4")).unwrap();
        make_tracks(&dir, 2.0, true, false);
        make_tracks(&dir, 3.0, false, false); // screen back to 3 s, mic stays 2 s
        let c = check(&dir, Some(&status), 30);
        assert_eq!(c.problems.len(), 2, "{c:?}");
        assert!(
            c.problems[0].starts_with("Picture and sound don't line up"),
            "{c:?}"
        );
        assert_eq!(
            c.problems[1],
            "30 of 120 frames are missing from the screen track."
        );
        assert_eq!(c.notes.len(), 1, "{c:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Needs ffmpeg: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn checks_the_camera_against_the_screen() {
        let dir = temp_root("check-camera");
        make_tracks(&dir, 3.0, true, false);
        make_camera(&dir, 3.0, false);
        let status = Status {
            elapsed: 3.0,
            frames: 90,
            camera_frames: 90,
            ..Default::default()
        };
        let c = check(&dir, Some(&status), 30);
        assert!(c.problems.is_empty(), "{c:?}");
        assert_eq!(c.camera.as_ref().unwrap().packets, 90);
        assert_eq!(c.camera_start_offset_ms, Some(0.0));
        assert!(c.camera_end_offset_ms.unwrap().abs() < 1.0, "{c:?}");

        // The camera is a second short, and 30 of the frames the recorder wrote are gone;
        // the camera also skipped pictures for 1 s.
        make_camera(&dir, 2.0, false);
        let status = Status {
            camera_frames: 90,
            camera_frames_lost: 30,
            ..status
        };
        let c = check(&dir, Some(&status), 30);
        assert_eq!(c.problems.len(), 2, "{c:?}");
        assert!(
            c.problems[0].starts_with("Screen and camera don't line up"),
            "{c:?}"
        );
        assert_eq!(
            c.problems[1],
            "30 of 90 frames are missing from the camera track."
        );
        assert!(
            c.notes[0].starts_with("The camera sent no new picture for 1.0 s"),
            "{c:?}"
        );

        // Camera and mic without a screen: the sound is checked against the camera.
        std::fs::remove_file(dir.join("screen.mp4")).unwrap();
        make_camera(&dir, 3.0, false);
        let c = check(&dir, None, 30);
        assert!(c.problems.is_empty(), "{c:?}");
        assert!(c.start_offset_ms.is_some() && c.camera_start_offset_ms.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reads_checks_saved_before_the_camera() {
        let old = r#"{"duration_s":3.0,"screen":null,"mic":null,"start_offset_ms":null,
            "end_offset_ms":null,"recorder":null,"problems":[],"notes":[]}"#;
        let c: Check = serde_json::from_str(old).unwrap();
        assert_eq!((c.duration_s, c.camera), (3.0, None));
    }

    #[test]
    fn an_empty_folder_is_not_playable() {
        let dir = temp_root("check-empty");
        let c = check(&dir, None, 30);
        assert!(!c.playable());
        std::fs::write(dir.join("screen.mp4"), b"not a video").unwrap();
        let c = check(&dir, None, 30);
        assert_eq!(c.problems, ["The screen track can't be read."]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

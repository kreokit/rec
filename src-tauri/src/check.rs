//! The check after every recording (and after a recovery): both tracks read back with
//! ffprobe, the picture has the frames the recorder wrote, and the sound starts and ends
//! with the picture (within one frame, plus one AAC frame at the end since AAC stores
//! sound in 1024-sample blocks).

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::native::Status;
use crate::probe::{self, MediaInfo};

/// One AAC frame at 48 kHz.
const AAC_FRAME_S: f64 = 1024.0 / 48_000.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrackCheck {
    pub codec: String,
    pub start_s: f64,
    pub duration_s: f64,
    /// Frames (picture) or AAC frames (sound).
    pub packets: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Check {
    /// The longer track's length.
    pub duration_s: f64,
    pub screen: Option<TrackCheck>,
    pub mic: Option<TrackCheck>,
    /// Sound minus picture at the start and at the end, in ms.
    pub start_offset_ms: Option<f64>,
    pub end_offset_ms: Option<f64>,
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
        self.screen.is_some() || self.mic.is_some()
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

/// Checks the tracks in `dir`. `status` is the recorder's final status (`None` after a
/// recovery); `fps` the picture's frame rate.
pub fn check(dir: &Path, status: Option<&Status>, fps: u32) -> Check {
    let mut c = Check {
        recorder: status.copied(),
        ..Default::default()
    };
    let frame = 1.0 / fps as f64;
    for (name, video) in [("screen.mp4", true), ("mic.mp4", false)] {
        let path = dir.join(name);
        if !path.exists() {
            continue;
        }
        let what = if video { "picture" } else { "sound" };
        match probe::probe(&path) {
            Ok(info) => match track(&info, video) {
                Some(t) if t.packets > 0 => {
                    if video {
                        c.screen = Some(t)
                    } else {
                        c.mic = Some(t)
                    }
                }
                _ => c.problems.push(format!("The {what} track is empty.")),
            },
            Err(_) => c.problems.push(format!("The {what} track can't be read.")),
        }
    }
    c.duration_s = [&c.screen, &c.mic]
        .into_iter()
        .flatten()
        .map(|t| t.start_s + t.duration_s)
        .fold(0.0, f64::max);

    if let (Some(s), Some(m)) = (&c.screen, &c.mic) {
        let start = m.start_s - s.start_s;
        let end = (m.start_s + m.duration_s) - (s.start_s + s.duration_s);
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

    if let Some(st) = status {
        if let Some(s) = &c.screen {
            let expected = (st.frames - st.frames_dropped).max(0) as u64;
            if s.packets + 1 < expected {
                c.problems.push(format!(
                    "{} of {expected} frames are missing from the picture track.",
                    expected - s.packets
                ));
            }
        }
        if st.frames_dropped > 0 {
            c.notes.push(format!(
                "{} frames couldn't be saved in time (the computer was busy); the picture skips there, the sound stays in sync.",
                st.frames_dropped
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::tests::{make_tracks, temp_root};

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
            "30 of 120 frames are missing from the picture track."
        );
        assert_eq!(c.notes.len(), 1, "{c:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_folder_is_not_playable() {
        let dir = temp_root("check-empty");
        let c = check(&dir, None, 30);
        assert!(!c.playable());
        std::fs::write(dir.join("screen.mp4"), b"not a video").unwrap();
        let c = check(&dir, None, 30);
        assert_eq!(c.problems, ["The picture track can't be read."]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

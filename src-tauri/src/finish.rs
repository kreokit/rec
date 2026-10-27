//! Finish (milestone 4): what the user chose for a recording before exporting it: the trim,
//! the layout, the voice clean-up and the loudness preset. Kept in `finish.json` in the
//! recording's folder, so reopening a recording brings the choices back.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::sessions::Session;

const FILE: &str = "finish.json";

/// Which tracks make the picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layout {
    Screen,
    /// The screen with the camera in a bubble.
    Bubble,
    Camera,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shape {
    Circle,
    /// A rounded 4:3 rectangle.
    Rounded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Size {
    Small,
    Medium,
    Large,
}

/// The camera bubble. The position is a fraction (0–1) of the room left for it inside the
/// margin each way, so it stays in place when the size changes: (1, 1) is bottom right.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Bubble {
    pub shape: Shape,
    pub size: Size,
    pub x: f64,
    pub y: f64,
}

impl Default for Bubble {
    /// The user's choice (docs/m4-plan.md): a Medium circle, bottom right.
    fn default() -> Self {
        Bubble {
            shape: Shape::Circle,
            size: Size::Medium,
            x: 1.0,
            y: 1.0,
        }
    }
}

/// The loudness preset (the presets of KreoKit Voice).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Loudness {
    /// −14 LUFS: TikTok, Reels, Shorts, YouTube.
    Social,
    /// −16 LUFS (the default: Rec is mostly for tutorials and demos).
    Tutorial,
    /// The level as recorded.
    Keep,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// Seconds from the recording's start.
    pub start_s: f64,
    /// `None`: to the end.
    pub end_s: Option<f64>,
    pub layout: Layout,
    pub bubble: Bubble,
    /// RNNoise on the microphone (at Voice's Fast Strength).
    pub clean_voice: bool,
    pub loudness: Loudness,
}

impl Settings {
    /// The first choices for a recording: the bubble when there are a screen and a camera.
    pub fn default_for(session: &Session) -> Settings {
        let layout = match (session.screen.is_some(), session.camera.is_some()) {
            (true, true) => Layout::Bubble,
            (false, true) => Layout::Camera,
            _ => Layout::Screen,
        };
        Settings {
            start_s: 0.0,
            end_s: None,
            layout,
            bubble: Bubble::default(),
            clean_voice: session.mic.is_some(),
            loudness: Loudness::Tutorial,
        }
    }

    /// The same choices made possible for this recording: a layout needs its tracks, the
    /// trim stays inside the recording and keeps at least one frame.
    pub fn fitted(mut self, session: &Session, duration_s: f64) -> Settings {
        let layout_ok = match self.layout {
            Layout::Screen => session.screen.is_some(),
            Layout::Bubble => session.screen.is_some() && session.camera.is_some(),
            Layout::Camera => session.camera.is_some(),
        };
        if !layout_ok {
            self.layout = Settings::default_for(session).layout;
        }
        self.bubble.x = self.bubble.x.clamp(0.0, 1.0);
        self.bubble.y = self.bubble.y.clamp(0.0, 1.0);
        let frame = 1.0 / session.screen.as_ref().map_or(30, |s| s.fps.max(1)) as f64;
        if duration_s > 0.0 {
            self.start_s = self.start_s.clamp(0.0, (duration_s - frame).max(0.0));
            self.end_s = self
                .end_s
                .map(|end| end.clamp(self.start_s + frame, duration_s))
                .filter(|end| *end < duration_s);
        }
        self.clean_voice &= session.mic.is_some();
        self
    }
}

/// The recording's Finish choices: saved ones, or the first choices.
pub fn read(dir: &Path, session: &Session) -> Settings {
    let duration = session
        .check
        .as_ref()
        .map_or(session.elapsed_s, |c| c.duration_s);
    fs::read_to_string(dir.join(FILE))
        .ok()
        .and_then(|json| serde_json::from_str::<Settings>(&json).ok())
        .unwrap_or_else(|| Settings::default_for(session))
        .fitted(session, duration)
}

pub fn write(dir: &Path, settings: &Settings) -> Result<(), String> {
    let json = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    let temp = dir.join("finish.json.tmp");
    fs::write(&temp, json)
        .and_then(|_| fs::rename(&temp, dir.join(FILE)))
        .map_err(|e| format!("Your Finish choices couldn't be saved ({e}). Check that the disk isn't full, then try again."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::{CameraInfo, ScreenInfo, State};

    fn session(camera: bool, mic: bool) -> Session {
        Session {
            id: "2026-10-27-143205".into(),
            created_at: 0,
            state: State::Done,
            screen: Some(ScreenInfo {
                display: "Built-in display".into(),
                region: None,
                window: None,
                width: 1728,
                height: 1080,
                fps: 30,
            }),
            mic: mic.then(|| "Mic".into()),
            camera: camera.then(|| CameraInfo {
                name: "FaceTime HD Camera".into(),
                width: 1280,
                height: 720,
            }),
            system_audio: false,
            elapsed_s: 10.0,
            pauses: 0,
            check: None,
            error: None,
            hidden: false,
        }
    }

    #[test]
    fn first_choices_follow_the_tracks() {
        let with_camera = Settings::default_for(&session(true, true));
        assert_eq!(with_camera.layout, Layout::Bubble);
        assert_eq!(with_camera.bubble, Bubble::default());
        assert!(with_camera.clean_voice);
        assert_eq!(with_camera.loudness, Loudness::Tutorial);
        let screen_only = Settings::default_for(&session(false, false));
        assert_eq!(screen_only.layout, Layout::Screen);
        assert!(!screen_only.clean_voice);
        let mut camera_only = session(true, true);
        camera_only.screen = None;
        assert_eq!(Settings::default_for(&camera_only).layout, Layout::Camera);
    }

    #[test]
    fn saved_choices_are_fitted_to_the_recording() {
        let s = session(false, true);
        let saved = Settings {
            start_s: -1.0,
            end_s: Some(30.0),
            layout: Layout::Bubble,
            bubble: Bubble {
                x: 2.0,
                ..Bubble::default()
            },
            ..Settings::default_for(&s)
        };
        let fitted = saved.fitted(&s, 10.0);
        assert_eq!(fitted.layout, Layout::Screen);
        assert_eq!(fitted.start_s, 0.0);
        assert_eq!(
            fitted.end_s, None,
            "the end of the recording means no end trim"
        );
        assert_eq!(fitted.bubble.x, 1.0);
        // The trim keeps at least one frame.
        let tight = Settings {
            start_s: 9.99,
            end_s: Some(5.0),
            ..Settings::default_for(&s)
        }
        .fitted(&s, 10.0);
        assert!((tight.start_s - (10.0 - 1.0 / 30.0)).abs() < 1e-9);
        assert_eq!(tight.end_s, None);
    }

    #[test]
    fn saves_and_reads_back() {
        let dir = std::env::temp_dir().join("kreokit-rec-finish-tests");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let s = session(true, true);
        assert_eq!(read(&dir, &s), Settings::default_for(&s));
        let chosen = Settings {
            start_s: 1.5,
            end_s: Some(8.0),
            layout: Layout::Camera,
            bubble: Bubble {
                shape: Shape::Rounded,
                size: Size::Large,
                x: 0.0,
                y: 0.5,
            },
            clean_voice: false,
            loudness: Loudness::Social,
        };
        write(&dir, &chosen).unwrap();
        assert_eq!(read(&dir, &s), chosen);
        // A damaged file gives the first choices again.
        fs::write(dir.join(FILE), "{").unwrap();
        assert_eq!(read(&dir, &s), Settings::default_for(&s));
    }
}

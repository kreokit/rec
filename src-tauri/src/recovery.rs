//! Crash recovery: a recording whose session.json still says "recording" (or paused, or
//! finishing) when the app starts was cut off by a crash, a forced quit or a power loss.
//! Its fragmented MP4 tracks play up to the last complete fragment (≤ 1-2 s lost); recovery
//! rewrites each into a normal MP4 (stream copy, the index at the front, the half-written
//! last frame dropped), cuts every track to the shortest one (they were cut off at
//! different fragments) and checks the result like a normal stop.

use std::fs;
use std::path::Path;
use std::process::Command;

use crate::check;
use crate::ffmpeg;
use crate::probe;
use crate::sessions::{Recordings, Session, State, read_session, write_session};

/// Recordings cut off by a crash, newest first.
pub fn pending(recordings: &Recordings) -> Vec<Session> {
    if recordings.is_recording() {
        // Only checked at start; a running recording is never "pending".
        return Vec::new();
    }
    recordings
        .all()
        .into_iter()
        .filter(|s| s.state.unfinished())
        .collect()
}

fn remux_args(input: &Path, output: &Path, until: Option<f64>) -> Vec<String> {
    let mut args: Vec<String> = [
        "-nostdin",
        "-v",
        "error",
        "-y",
        "-fflags",
        "+discardcorrupt",
        "-i",
    ]
    .map(String::from)
    .to_vec();
    args.push(input.to_string_lossy().into_owned());
    if let Some(t) = until {
        args.extend(["-t".into(), format!("{t:.6}")]);
    }
    args.extend(["-map", "0", "-c", "copy", "-movflags", "+faststart"].map(String::from));
    args.push(output.to_string_lossy().into_owned());
    args
}

/// Rewrites one track in place, up to `until` seconds if given. Ok(false): nothing in it
/// could be saved (the original is left as it was).
fn remux_track(path: &Path, until: Option<f64>) -> Result<bool, String> {
    if !path.is_file() {
        return Ok(false);
    }
    let temp = path.with_extension("recovering.mp4");
    let ffmpeg = ffmpeg::locate("ffmpeg")?;
    let ok = Command::new(ffmpeg)
        .args(remux_args(path, &temp, until))
        .output()
        .map_err(|e| e.to_string())?
        .status
        .success();
    let usable =
        ok && probe::probe(&temp).is_ok_and(|info| info.streams.iter().any(|s| s.packets > 0));
    if !usable {
        let _ = fs::remove_file(&temp);
        return Ok(false);
    }
    fs::rename(&temp, path).map_err(|e| {
        format!("The recovered recording can't be saved ({e}). Check that the disk isn't full, then try again.")
    })?;
    Ok(true)
}

/// Recovers recording `id`: its tracks are finished and checked, and it joins Recent.
pub fn recover(recordings: &Recordings, id: &str) -> Result<Session, String> {
    let dir = recordings.dir(id)?;
    let mut session = read_session(&dir).ok_or("This recording no longer exists.")?;
    if !session.state.unfinished() {
        return Ok(session);
    }
    let mut any = false;
    for (name, _, _) in check::TRACKS {
        any |= remux_track(&dir.join(name), None)?;
    }
    let fps = session.screen.as_ref().map_or(30, |s| s.fps);
    let mut result = check::check(&dir, None, fps);
    // Every track ends where the shortest one does; except the computer sound, which may end
    // early (it sends nothing while nothing plays) and is only cut.
    let ends =
        check::TRACKS.map(|(name, _, _)| (name, result.get(name).map(check::TrackCheck::end_s)));
    let end = ends
        .iter()
        .filter(|(name, _)| *name != "system.mp4")
        .filter_map(|(_, e)| *e)
        .fold(f64::INFINITY, f64::min);
    let mut cut = false;
    for (name, track_end) in ends {
        if track_end.is_some_and(|e| e > end + 1.0 / fps as f64) {
            remux_track(&dir.join(name), Some(end))?;
            cut = true;
        }
    }
    if cut {
        result = check::check(&dir, None, fps);
    }
    if any && result.playable() {
        session.state = State::Recovered;
        session.elapsed_s = result.duration_s;
    } else {
        session.state = State::Failed;
        session.error = Some(
            "Nothing could be recovered: the recording stopped before its first second was saved."
                .into(),
        );
    }
    session.check = Some(result);
    write_session(&dir, &session)?;
    Ok(session)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::tests::{fake, make_camera, make_tracks, temp_root};
    use crate::sessions::{ScreenInfo, Session};

    fn crashed(root: &Path, id: &str, elapsed: f64) -> std::path::PathBuf {
        let dir = root.join(id);
        fs::create_dir_all(&dir).unwrap();
        let session = Session {
            id: id.into(),
            created_at: 1,
            state: State::Recording,
            screen: Some(ScreenInfo {
                display: "Built-in display".into(),
                region: None,
                window: None,
                width: 320,
                height: 180,
                fps: 30,
            }),
            mic: Some("Microphone".into()),
            camera: None,
            system_audio: false,
            elapsed_s: elapsed,
            pauses: 0,
            check: None,
            error: None,
            hidden: false,
        };
        write_session(&dir, &session).unwrap();
        dir
    }

    fn cut(path: &Path, keep: f64) {
        let data = fs::read(path).unwrap();
        fs::write(path, &data[..(data.len() as f64 * keep) as usize]).unwrap();
    }

    #[test]
    fn the_remux_is_lgpl() {
        let args = remux_args(Path::new("in.mp4"), Path::new("out.mp4"), Some(5.0));
        assert_eq!(ffmpeg::gpl_only(&args), None);
    }

    /// Needs ffmpeg: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn recovers_a_cut_off_recording() {
        let root = temp_root("recover");
        let dir = crashed(&root, "2026-10-27-143200", 6.0);
        make_tracks(&dir, 6.0, true, true);
        cut(&dir.join("screen.mp4"), 0.93);
        cut(&dir.join("mic.mp4"), 0.93);
        let recordings = fake(&root);
        assert!(
            recordings.recent().is_empty(),
            "not in Recent before recovery"
        );
        let found = pending(&recordings);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].elapsed_s, 6.0);

        let session = recover(&recordings, &found[0].id).unwrap();
        assert_eq!(session.state, State::Recovered, "{session:?}");
        let check = session.check.as_ref().unwrap();
        // At most 2 s lost, and the files are normal MP4s now.
        assert!(
            session.elapsed_s > 4.0 && session.elapsed_s <= 6.0,
            "{session:?}"
        );
        assert!(probe::probe(&dir.join("screen.mp4")).is_ok());
        assert!(check.screen.is_some() && check.mic.is_some(), "{check:?}");
        assert!(pending(&recordings).is_empty());
        assert_eq!(recordings.recent().len(), 1);
        assert!(!dir.join("screen.recovering.mp4").exists());
        let _ = fs::remove_dir_all(&root);
    }

    /// Needs ffmpeg: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn cuts_every_track_to_the_shortest() {
        let root = temp_root("recover-camera");
        let dir = crashed(&root, "2026-10-27-143400", 6.0);
        make_tracks(&dir, 6.0, true, true);
        // The camera's writer was a fragment behind when the app was killed.
        make_camera(&dir, 4.0, true);
        // The computer sound sent nothing for its last 3 s (nothing played): it doesn't
        // decide where the others end.
        make_system(&dir, 1.0);
        let recordings = fake(&root);
        let session = recover(&recordings, "2026-10-27-143400").unwrap();
        assert_eq!(session.state, State::Recovered, "{session:?}");
        let check = session.check.as_ref().unwrap();
        assert!(check.problems.is_empty(), "{check:?}");
        for t in [&check.screen, &check.camera, &check.mic] {
            let t = t.as_ref().unwrap();
            assert!((t.end_s() - 4.0).abs() <= 1.0 / 30.0 + 0.03, "{check:?}");
        }
        assert!(check.system.as_ref().unwrap().end_s() < 1.1, "{check:?}");
        let _ = fs::remove_dir_all(&root);
    }

    /// A computer sound track (stereo AAC) `seconds` long, fragmented.
    fn make_system(dir: &Path, seconds: f64) {
        let status = std::process::Command::new(ffmpeg::locate("ffmpeg").unwrap())
            .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
            .arg(format!("sine=sample_rate=48000:duration={seconds}"))
            .args(["-c:a", "aac", "-ac", "2", "-frag_duration", "1000000"])
            .args(["-movflags", "frag_keyframe+empty_moov+default_base_moof"])
            .arg(dir.join("system.mp4"))
            .status()
            .unwrap();
        assert!(status.success());
    }

    /// Needs ffmpeg: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn reports_when_nothing_was_saved() {
        let root = temp_root("recover-empty");
        let dir = crashed(&root, "2026-10-27-143300", 0.4);
        make_tracks(&dir, 2.0, false, true);
        // Only the file's header was written.
        cut(&dir.join("screen.mp4"), 0.02);
        let recordings = fake(&root);
        let session = recover(&recordings, "2026-10-27-143300").unwrap();
        assert_eq!(session.state, State::Failed);
        assert!(
            session
                .error
                .as_deref()
                .unwrap()
                .starts_with("Nothing could be recovered")
        );
        // Offered no more; can be deleted from Recent.
        assert!(pending(&recordings).is_empty());
        recordings.delete("2026-10-27-143300").unwrap();
        let _ = fs::remove_dir_all(&root);
    }

    /// Run by `recovers_after_kill_9` in a child process: records until killed.
    #[test]
    #[ignore]
    fn kill_child() {
        let Some(root) = std::env::var_os("KREOKIT_REC_KILL_ROOT") else {
            return;
        };
        let recordings = Recordings::new(root.into(), Box::new(crate::sessions::NativeBackend));
        let mut options = crate::sessions::tests::options();
        options.display_width = 640;
        options.display_height = 360;
        options.camera_id = Some(String::new());
        options.camera_name = Some("Test camera".into());
        options.camera_width = 640;
        options.camera_height = 360;
        recordings
            .start_with(&options, Some(crate::native::Synthetic::default()))
            .unwrap();
        loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }

    /// The whole path: the real recorder (synthetic sources) and session code in a child
    /// process, `kill -9` after about 6 s, then recovery as at the next start.
    #[test]
    fn recovers_after_kill_9() {
        let _timing = crate::native::tests::alone();
        let root = temp_root("kill9");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", "recovery::tests::kill_child"])
            .env("KREOKIT_REC_KILL_ROOT", &root)
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let recordings = fake(&root);
        let started = std::time::Instant::now();
        let elapsed = || recordings.all().first().map_or(0.0, |s| s.elapsed_s);
        while elapsed() < 6.0 {
            assert!(started.elapsed().as_secs() < 60, "the child didn't record");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        // Up to one heartbeat (0.25 s) more than session.json says was recorded.
        std::thread::sleep(std::time::Duration::from_millis(250));
        child.kill().unwrap();
        child.wait().unwrap();
        let found = pending(&recordings);
        assert_eq!(found.len(), 1);
        let before = found[0].elapsed_s + 0.25;
        let session = recover(&recordings, &found[0].id).unwrap();
        assert_eq!(session.state, State::Recovered, "{session:?}");
        let lost = before - session.elapsed_s;
        eprintln!(
            "recovered {:.2} s of about {before:.2} s",
            session.elapsed_s
        );
        // CI's VMs have no hardware encoder: the writer runs behind there
        // (docs/capture-research.md), so up to 3 s; real computers must stay within 2 s.
        let limit = if std::env::var_os("CI").is_some() {
            3.0
        } else {
            2.0
        };
        assert!(lost <= limit, "lost {lost:.2} s: {session:?}");
        let check = session.check.unwrap();
        assert!(check.problems.is_empty(), "{check:?}");
        assert!(check.screen.is_some() && check.camera.is_some() && check.mic.is_some());
        let _ = fs::remove_dir_all(&root);
    }
}

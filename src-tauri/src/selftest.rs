//! `rec --self-test`: a hidden mode for release CI and the user's first try of a release
//! (milestone 6, the Clips pattern). It runs the app's own code inside the shipped binary
//! with its bundled FFmpeg on the synthetic sources (the test pattern with a frame counter
//! as screen and camera, a click every second as the mic; never a real device) and exits
//! non-zero on any failure. It runs before Tauri starts, so it needs no window or display.
//!
//! 1. FFmpeg is the bundled one (release builds) and an LGPL build.
//! 2. Record through `Recordings` and the real recorder, in real time: 1080p screen, 720p
//!    camera and mic, held under a countdown, begun, paused and resumed, about 10 s. The
//!    after-recording check finds no problems; the frame counter shows no missing picture
//!    except where the recorder counted a late one; every second's marker is within one
//!    frame of its click; H.264 yuv420p BT.709 and AAC 48 kHz.
//! 3. Recover a deliberately interrupted recording: a child process (`--self-test-child`)
//!    records and is killed; the recovery as at the next start keeps all but the last 2 s
//!    (3 s on CI's VMs, as the tests).
//! 4. Export the first recording through Finish's code: the bubble (Medium circle, bottom
//!    right), a trim, voice clean-up and Tutorial −16 LUFS, with export's own checks; then
//!    the bubble at its place with its circle (a pixel check on the camera's marker strip)
//!    and picture and sound in sync through the export.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use crate::check::Check;
use crate::export::{self, Job};
use crate::finish::Settings;
use crate::native::{Status, Synthetic};
use crate::sessions::{NativeBackend, Options, Recordings, Session, State};
use crate::testpattern::{self, Geometry, Report};
use crate::{ffmpeg, probe, recovery};

/// Seconds recorded before and after the pause.
const BEFORE_PAUSE: f64 = 4.0;
const RECORDED: f64 = 10.0;
/// The pause's length (wall clock).
const PAUSE: Duration = Duration::from_millis(1500);
/// The interrupted recording is killed after this much.
const BEFORE_KILL: f64 = 5.0;
/// One frame at 30 fps, in ms: the sync limit.
const FRAME_MS: f64 = 1000.0 / 30.0;

/// Runs the self-test if `args` (without the program name) start with `--self-test`, or
/// the recording it interrupts with `--self-test-child <folder>`.
pub fn run_if_requested(args: &[String]) -> Option<ExitCode> {
    match args.first().map(String::as_str) {
        Some("--self-test") => {}
        Some("--self-test-child") => return Some(child(args.get(1)?)),
        _ => return None,
    }
    if args.len() > 1 {
        eprintln!("usage: rec --self-test");
        return Some(ExitCode::FAILURE);
    }
    let started = Instant::now();
    let work = std::env::temp_dir().join(format!("kreokit-rec-self-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    let result = std::fs::create_dir_all(&work)
        .map_err(|e| format!("cannot create {}: {e}", work.display()))
        .and_then(|()| run(&work));
    let _ = std::fs::remove_dir_all(&work);
    println!("took {:.1?}", started.elapsed());
    Some(match result {
        Ok(()) => {
            println!("self-test: ok");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("self-test: FAILED: {e}");
            ExitCode::FAILURE
        }
    })
}

/// The setup of every self-test recording: a 1080p display, a 720p camera, the mic.
fn options(countdown: bool) -> Options {
    Options {
        display_id: 0,
        display_name: "Test pattern".into(),
        display_width: 1920,
        display_height: 1080,
        quality: 1080,
        mic_id: Some(String::new()),
        mic_name: Some("Test clicks".into()),
        camera_id: Some(String::new()),
        camera_name: Some("Test pattern".into()),
        camera_width: 1280,
        camera_height: 720,
        display_points_width: 1920.0,
        region: None,
        window: None,
        system_audio: false,
        countdown,
        own_windows: Vec::new(),
    }
}

/// Real-time synthetic sources: the recorder's pacing, as with real devices.
fn recordings(root: PathBuf) -> Recordings {
    let mut recordings = Recordings::new(root, Box::new(NativeBackend));
    recordings.synthetic = Some(Synthetic::default());
    recordings
}

fn run(work: &Path) -> Result<(), String> {
    let ffmpeg = ffmpeg::locate("ffmpeg")?;
    let ffprobe = ffmpeg::locate("ffprobe")?;
    println!("ffmpeg:   {}", ffmpeg.display());
    println!("ffprobe:  {}", ffprobe.display());
    if !cfg!(debug_assertions) {
        bundled(&ffmpeg)?;
        bundled(&ffprobe)?;
        lgpl(&ffmpeg)?;
    }

    let recordings = recordings(work.join("recordings"));
    let session = record(&recordings)?;
    let dir = recordings.dir(&session.id)?;
    recording_matches(&dir, &session)?;
    interrupted(work)?;
    export_bubble(&dir, &session, work)
}

/// Release builds must run the FFmpeg they bundle, not one found on the system.
fn bundled(tool: &Path) -> Result<(), String> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    let overridden = ["KREOKIT_FFMPEG", "KREOKIT_FFPROBE"]
        .iter()
        .any(|var| std::env::var_os(var).is_some());
    if overridden || tool.parent() == exe_dir.as_deref() {
        Ok(())
    } else {
        Err(format!(
            "{} isn't the bundled one (next to the executable)",
            tool.display()
        ))
    }
}

/// The bundled FFmpeg must be an LGPL build.
fn lgpl(ffmpeg: &Path) -> Result<(), String> {
    let out = Command::new(ffmpeg)
        .args(["-hide_banner", "-version"])
        .output()
        .map_err(|e| format!("cannot run {}: {e}", ffmpeg.display()))?;
    let text = String::from_utf8_lossy(&out.stdout);
    for flag in ["--enable-gpl", "--enable-nonfree", "--enable-version3"] {
        if text.contains(flag) {
            return Err(format!("the bundled FFmpeg was built with {flag}"));
        }
    }
    println!("ffmpeg:   LGPL build");
    Ok(())
}

/// Waits until the recording has `seconds` recorded (its own count).
fn wait_recorded(recordings: &Recordings, seconds: f64) -> Result<(), String> {
    let started = Instant::now();
    loop {
        let elapsed = recordings
            .all()
            .first()
            .map_or(0.0, |s: &Session| s.elapsed_s);
        if elapsed >= seconds {
            return Ok(());
        }
        if started.elapsed() > Duration::from_secs(120) {
            return Err(format!(
                "the recording stalled at {elapsed:.2} s (waiting for {seconds} s)"
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Step 2: countdown, record, pause, resume, stop.
fn record(recordings: &Recordings) -> Result<Session, String> {
    let session = recordings.start(&options(true))?;
    if session.state != State::Countdown {
        return Err(format!(
            "the countdown didn't hold the recording: {session:?}"
        ));
    }
    // The countdown: the devices are open, nothing is recorded.
    std::thread::sleep(Duration::from_millis(1000));
    recordings.begin()?;
    wait_recorded(recordings, BEFORE_PAUSE)?;
    recordings.pause()?;
    std::thread::sleep(PAUSE);
    recordings.resume()?;
    wait_recorded(recordings, RECORDED)?;
    let session = recordings.stop()?;
    println!(
        "recorded: {:.2} s, {} pause(s), state {:?}",
        session.elapsed_s, session.pauses, session.state
    );
    if session.state != State::Done || session.error.is_some() || session.pauses != 1 {
        return Err(format!("the recording didn't finish cleanly: {session:?}"));
    }
    let check = session
        .check
        .as_ref()
        .ok_or("the recording wasn't checked")?;
    no_problems("recording", check)?;
    if check.screen.is_none() || check.camera.is_none() || check.mic.is_none() {
        return Err(format!("a track is missing: {check:?}"));
    }
    // About 10 s: the pause recorded nothing.
    if (check.duration_s - session.elapsed_s).abs() > 0.2 || session.elapsed_s > RECORDED + 1.0 {
        return Err(format!(
            "the tracks are {:.2} s long for {:.2} s recorded (the pause must add nothing)",
            check.duration_s, session.elapsed_s
        ));
    }
    Ok(session)
}

fn no_problems(what: &str, check: &Check) -> Result<(), String> {
    for note in &check.notes {
        println!("note:     {note}");
    }
    if check.problems.is_empty() {
        Ok(())
    } else {
        Err(format!("the {what}'s check: {}", check.problems.join(" ")))
    }
}

/// Step 2, in the files: the frame counters, sync and the encoder settings.
fn recording_matches(dir: &Path, session: &Session) -> Result<(), String> {
    let status = session
        .check
        .as_ref()
        .and_then(|c| c.recorder)
        .ok_or("the recorder's counts are missing")?;
    let (screen_w, camera_w) = (
        session.screen.as_ref().map_or(0, |s| s.width),
        session.camera.as_ref().map_or(0, |c| c.width),
    );
    let screen = pattern(dir, "screen.mp4", screen_w, 60.0)?;
    show("screen", &screen);
    counted(
        "screen",
        &screen,
        status.frames,
        status.frames_late,
        &status,
    )?;
    let camera = pattern(dir, "camera.mp4", camera_w, 30.0)?;
    show("camera", &camera);
    counted(
        "camera",
        &camera,
        status.camera_frames,
        status.camera_frames_late,
        &status,
    )?;
    for (file, w, h) in [("screen.mp4", 1920, 1080), ("camera.mp4", 1280, 720)] {
        let info = probe::probe(&dir.join(file))?;
        let v = info.video().ok_or(format!("{file} has no picture"))?;
        let ok = v.codec == "h264"
            && v.pix_fmt.as_deref() == Some("yuv420p")
            && (v.width, v.height) == (Some(w), Some(h))
            && v.color == Some(["bt709".into(), "bt709".into(), "bt709".into()]);
        if !ok {
            return Err(format!(
                "{file} isn't H.264 yuv420p BT.709 at {w}×{h}: {v:?}"
            ));
        }
    }
    let mic = probe::probe(&dir.join("mic.mp4"))?;
    let a = mic.audio().ok_or("mic.mp4 has no sound")?;
    if a.codec != "aac" || a.sample_rate != Some(48_000) {
        return Err(format!("mic.mp4 isn't AAC 48 kHz: {a:?}"));
    }
    println!("tracks:   H.264 yuv420p BT.709 1920×1080 + 1280×720, AAC 48 kHz");
    Ok(())
}

/// The test pattern of `video` in `dir` (`width` wide, its counter stepping `rate` times a
/// second) against the mic's clicks.
fn pattern(dir: &Path, video: &str, width: u32, rate: f64) -> Result<Report, String> {
    let g = geometry(width, rate);
    let frames = testpattern::read_frames(&dir.join(video), &g)?;
    let (samples, start) = testpattern::read_audio(&dir.join("mic.mp4"))?;
    let clicks = testpattern::onsets(&samples, start, 0.25, 0.5);
    Ok(testpattern::report(&frames, &clicks, &g, 30.0))
}

fn geometry(width: u32, rate: f64) -> Geometry {
    Geometry {
        x: 0.0,
        y: 0.0,
        cell: (width / 34) as f64,
        rate,
        still_periods: false,
    }
}

fn show(what: &str, r: &Report) {
    println!(
        "{what:<9} {} frames, {} repeated, {} markers ({} without a click), sound − picture {:?}…{:?} ms",
        r.frames, r.stale, r.markers, r.unmatched_markers, r.sync_ms_min, r.sync_ms_max
    );
}

/// No unreadable picture, as many frames as the recorder wrote, repeats only where it
/// counted a late frame (and few of those), and every marker within a frame of its click
/// (the pause may cut one click and keep its marker).
fn counted(what: &str, r: &Report, frames: i64, late: i64, status: &Status) -> Result<(), String> {
    let fail = |why: &str| Err(format!("{what}: {why}: {r:?} {status:?}"));
    if r.bad > 0 || r.irregular_pts > 0 {
        return fail("unreadable pictures or irregular timestamps");
    }
    if r.frames as i64 != frames {
        return fail("the file doesn't hold the frames the recorder wrote");
    }
    if r.stale > late as usize || late as usize > r.frames / 10 {
        return fail("pictures are missing");
    }
    sync(what, r, 1)
}

fn sync(what: &str, r: &Report, unmatched: usize) -> Result<(), String> {
    let worst = r
        .sync_ms_min
        .zip(r.sync_ms_max)
        .map(|(a, b)| a.abs().max(b.abs()));
    let seconds = r.frames as f64 / 30.0;
    match worst {
        Some(w)
            if w <= FRAME_MS
                && r.unmatched_markers <= unmatched
                && r.markers as f64 >= seconds - 2.0 =>
        {
            Ok(())
        }
        _ => Err(format!(
            "{what}: picture and sound are more than a frame apart, or markers are missing: {r:?}"
        )),
    }
}

/// Step 3: a child process records until it is killed, then the recovery.
fn interrupted(work: &Path) -> Result<(), String> {
    let root = work.join("interrupted");
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut child = Command::new(exe)
        .arg("--self-test-child")
        .arg(&root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot start the recording to interrupt: {e}"))?;
    let recordings = recordings(root);
    let waited = wait_recorded(&recordings, BEFORE_KILL);
    // Up to one heartbeat (0.25 s) more than session.json says was recorded.
    std::thread::sleep(Duration::from_millis(250));
    let _ = child.kill();
    let _ = child.wait();
    waited?;
    let found = recovery::pending(&recordings);
    let [pending] = found.as_slice() else {
        return Err(format!("{} recordings to recover, not 1", found.len()));
    };
    let before = pending.elapsed_s + 0.25;
    let session = recovery::recover(&recordings, &pending.id)?;
    let lost = before - session.elapsed_s;
    println!(
        "recovered {:.2} s of about {before:.2} s (lost {lost:.2} s)",
        session.elapsed_s
    );
    let limit = if std::env::var_os("CI").is_some() {
        3.0
    } else {
        2.0
    };
    if session.state != State::Recovered || lost > limit {
        return Err(format!(
            "the interrupted recording lost {lost:.2} s (at most {limit} s): {session:?}"
        ));
    }
    let check = session
        .check
        .as_ref()
        .ok_or("the recovery wasn't checked")?;
    no_problems("recovered recording", check)?;
    if check.screen.is_none() || check.camera.is_none() || check.mic.is_none() {
        return Err(format!("the recovery lost a track: {check:?}"));
    }
    Ok(())
}

/// The recording that `interrupted` kills: records until then.
fn child(root: &str) -> ExitCode {
    let recordings = recordings(PathBuf::from(root));
    if let Err(e) = recordings.start(&options(false)) {
        eprintln!("self-test child: {e}");
        return ExitCode::FAILURE;
    }
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// Step 4: Finish's choices, the export and its checks, the bubble and sync in the file.
fn export_bubble(dir: &Path, session: &Session, work: &Path) -> Result<(), String> {
    let duration_s = session.check.as_ref().map_or(0.0, |c| c.duration_s);
    let mut settings = Settings::default_for(session);
    settings.start_s = 1.5;
    settings.end_s = Some(duration_s - 0.5);
    let settings = settings.fitted(session, duration_s);
    let job = Job {
        dir,
        session,
        settings: &settings,
        duration_s,
    };
    let dest = work.join("exports").join(export::file_name(session));
    std::fs::create_dir_all(dest.parent().unwrap()).map_err(|e| e.to_string())?;
    let done = export::export(&job, &dest, false, &AtomicBool::new(false), &mut |_, _| {})?;
    println!(
        "exported: {}×{} {:.2} s with {} ({} {}), {:?} LUFS, true peak {:?} dBTP{}",
        done.facts.width,
        done.facts.height,
        done.facts.container_s,
        done.encoder,
        done.facts.profile,
        done.facts.pix_fmt,
        done.lufs,
        done.true_peak_db,
        if done.held_by_peaks {
            " (held by its peaks)"
        } else {
            ""
        }
    );
    // A second export never overwrites the first.
    let again = export::export(&job, &dest, false, &AtomicBool::new(false), &mut |_, _| {})?;
    if again.path == done.path || !done.path.is_file() {
        return Err(format!(
            "the second export overwrote the first: {}",
            again.path.display()
        ));
    }

    // The bubble: its place and its circle, on the camera's marker strip.
    let p = job.placement();
    let (cam, crop) = p
        .camera
        .zip(p.camera_crop)
        .ok_or("the export has no camera bubble")?;
    let camera_w = session.camera.as_ref().map_or(0, |c| c.width);
    let cell = (camera_w / 34) as f64;
    // The strip (rows 2-3 of cells), in the bubble.
    let y = cam.y + (3.0 * cell * cam.height as f64 / crop.height as f64).round() as u32;
    let rows = gray_rows(&done.path, cam.x, y, cam.width)?;
    let centre = (cam.width / 2) as usize;
    let edge = 8usize;
    let ok = rows
        .iter()
        .filter(|r| {
            r.len() == cam.width as usize
                && (r[centre] as i32 - 128).abs() > 60
                && (r[edge] as i32 - 128).abs() < 16
        })
        .count();
    println!(
        "bubble:   at ({}, {}) {}×{}, the camera's strip inside the circle on {ok} of {} frames",
        cam.x,
        cam.y,
        cam.width,
        cam.height,
        rows.len()
    );
    if rows.is_empty() || ok * 10 < rows.len() * 9 {
        return Err("the camera bubble isn't where the layout puts it, or isn't round".into());
    }

    // Picture and sound in sync through the trim, the clean-up, the loudness and AAC.
    let g = geometry(done.facts.width, 60.0);
    let frames = testpattern::read_frames(&done.path, &g)?;
    let (samples, start) = testpattern::read_audio(&done.path)?;
    let peak = samples.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    let clicks = testpattern::onsets(&samples, start, peak * 0.5, 0.5);
    let report = testpattern::report(&frames, &clicks, &g, 30.0);
    show("export", &report);
    if report.bad > 0 || report.irregular_pts > 0 {
        return Err(format!(
            "the export's pictures are out of order: {report:?}"
        ));
    }
    sync("export", &report, 1)
}

/// One gray pixel row (`width` from `x`, at `y`) of every frame of `video`.
fn gray_rows(video: &Path, x: u32, y: u32, width: u32) -> Result<Vec<Vec<u8>>, String> {
    let out = Command::new(ffmpeg::locate("ffmpeg")?)
        .args(["-v", "error", "-i"])
        .arg(video)
        .arg("-vf")
        .arg(format!("format=gray,crop={width}:1:{x}:{y}"))
        .args(["-f", "rawvideo", "-"])
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(out
        .stdout
        .chunks(width as usize)
        .map(<[u8]>::to_vec)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_only_when_asked() {
        assert!(run_if_requested(&[]).is_none());
        assert!(run_if_requested(&["--bench".into()]).is_none());
        // `--self-test-child` needs its folder.
        assert!(run_if_requested(&["--self-test-child".into()]).is_none());
        assert_eq!(
            run_if_requested(&["--self-test".into(), "extra".into()]),
            Some(ExitCode::FAILURE)
        );
    }

    #[test]
    fn the_sync_rule_allows_one_frame() {
        let mut r = Report {
            frames: 300,
            markers: 10,
            sync_ms_min: Some(-20.0),
            sync_ms_max: Some(30.0),
            ..Default::default()
        };
        assert!(sync("x", &r, 1).is_ok());
        r.sync_ms_max = Some(40.0);
        assert!(sync("x", &r, 1).is_err());
        r.sync_ms_max = Some(30.0);
        r.markers = 5;
        assert!(sync("x", &r, 1).is_err());
    }
}

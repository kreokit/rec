# M5 results (Polish)

MacBook Pro, Intel i5, macOS 12.7.6, built-in display recorded at 1728×1080 (60 fps capture,
one frame kept per 1/30 s slot). Real screen, camera and mic with the user's OK
(2026-10-27); every recording was deleted after the analysis, only the numbers are kept.

## Step 1: ScreenCaptureKit with Rec's windows kept out (macOS 12)

`--bench fresh --exclude`: a striped stand-in for the floating control (8-pixel black/white
stripes flipping twice a second) sits 4 points under the counter, inside the region's
margin, and the recorder is asked to keep it out. The analysis counts the frames where the
stripes show. The CGDisplayStream run is the control (it can't exclude windows, so the
stripes must show there).

| Run | Fresh | Stale | Stand-in shown | CPU (% of one core) |
|---|---|---|---|---|
| CGDisplayStream, 60 s (control) | 99.86 % | 2 | **1801 of 1801 frames** | 13.8 % |
| ScreenCaptureKit, 60 s | 99.03 % | 14 | 0 of 1802 | 19.1 % |
| ScreenCaptureKit, 60 s | 99.31 % | 10 | 0 of 1802 | 19.4 % |
| ScreenCaptureKit, 60 s | 99.45 % | 8 | 0 of 1802 | 19.1 % |
| ScreenCaptureKit, region, 60 s | 99.72 % | 4 | 0 of 1802 | 19.2 % |
| ScreenCaptureKit, region, 60 s | 99.45 % | 8 | 0 of 1802 | 16.9 % |
| ScreenCaptureKit + camera 720p + mic, 10 min | 99.45 % | 79 | 0 of 18030 | 26.7 % |

- **Exclusion works on macOS 12:** the stand-in never reached the file (it showed in every
  frame of the control run, so the check sees it when it's there).
- **Regions through ScreenCaptureKit's `sourceRect` work on 12** (the counter is read back
  at the right place in every frame; not black, unlike CGDisplayStream's source rectangle).
- **Cost:** ScreenCaptureKit is 0.4–0.8 points less fresh than CGDisplayStream and uses
  about 5 points more of one core. The 10-minute run (26.7 %) is close to M3's
  CGDisplayStream + camera + mic run (25 %).
- 10-minute run: no frames dropped on any track, all tracks 601.0 s, start/end offsets 0 ms,
  camera times regular; 2 screen and 81 camera frames late (repeated).
- **One dropout at 293.2 s, a hardware fault of the dev Mac, not the capture:** the whole
  picture was black for 1.6 s and the mic sent nothing for 2.35 s. The power log shows
  "Display is turned off" at that second, a lid event (`AppleACPILid`) and the display back
  on a second later, with the audio hardware going through sleep/wake. The user confirmed
  the Mac has a display fault (the screen goes black at some lid angles; real sleep shows
  the lock screen). Idle display sleep isn't the cause: recordings hold
  `PreventUserIdleDisplaySleep` (`awake.rs`; Windows `ES_DISPLAY_REQUIRED`, Linux
  `systemd-inhibit --what=idle:sleep`). The recorder kept going: the picture stayed
  in step and the mic gap was filled with silence (the check noted it). That frame
  stretch is the one "bad" counter read (the counter wasn't visible).

**Decision (by the plan's rule, ≥ 99 % fresh in every run):** on macOS 12, displays and
regions are recorded with ScreenCaptureKit, Rec's windows excluded, **whenever a Rec window
(floating control, camera bubble) is shown on the recorded display**; otherwise CGDisplayStream
stays (fresher, cheaper). The margin is small (lowest run 99.03 %), so step 10 repeats a
run with the real floating control and bubble; if that falls under 99 %, macOS 12 falls back
to the menu bar (choice 1).

## Step 4: the floating control doesn't take the keyboard (macOS 12)

Check copy with synthetic sources; a small probe app (`focusprobe`, scratchpad) holds the
keyboard in a text field, then the control is clicked and the probe reports whether it is
still the active app with the key window.

| Control window | Click on Pause | Click on its background |
|---|---|---|
| Tauri `focusable(false)` only | **Rec becomes active** (probe loses the keyboard) | – |
| + non-activating panel class and style | Rec becomes active | – (not measured with the control confirmed on screen) |
| + the window server's "prevents activation" flag | **probe keeps the keyboard**, Pause works | probe keeps it |

- An NSWindow can't be non-activating; the control's window becomes an `NSPanel` subclass in
  place (`window.m`), and after that class change the style alone doesn't reach the window
  server: AppKit's `_setPreventsActivation:` (checked with `respondsToSelector:`) does. The
  original class is put back before the window closes: closing it as the panel raised an
  Objective-C exception in tao's event loop (crash, the recording itself was finished and
  kept).
- Tauri's drag region doesn't move a window that never becomes key: the control is dragged
  by hand (pointer moves → `control_move`); dragged 100 × 338 points, it moved exactly that
  far, the probe kept the keyboard, and the place was remembered for that display.
- No click → the probe stays active (the control's polling doesn't activate the app).

## Step 7: disk space near full (dev Mac, synthetic sources, a scratch disk image)

`measure_disk_full` (sessions.rs, only with `KREOKIT_MEASURE_DISK`): 1728×1080 screen + 720p
camera + mic, as fast as the encoder allows, on a 2.6 GB APFS sparse image in the scratchpad
(deleted afterwards). The synthetic picture compresses to almost nothing (≈ 13 kB/s of
recording), so the thresholds were moved for the measurement instead of filling 1.5 GB.

| Scenario | What happened |
|---|---|
| A: the disk-space stop 10 MB below the free space at the start | stopped 87 s in (672 s of recording); **finishing took 0.65 s and 128 KB**; screen, camera and mic 672.4 s each, check clean |
| A, first run (stop never reached in 5 min) | finishing took 0.9 s and 2 MB; 2596.6 s, check clean |
| B: the image filled to 40 MB free, the stop off | the writers failed at **20 MB "free"** (APFS keeps a reserve); the stop then **failed** before checking ("cannot save the recording's details", a 0-byte `session.json.tmp` left) |
| B again, after the fix below | the stop finishes and checks: files play, 1443.0 of 1444.2 s (the last unfinished fragment lost, as in a crash); the recording says the disk was full and is offered for recovery at the next start |

- **Fixed:** the stop no longer gives up when `session.json` can't be written: it finishes
  and checks the files anyway, removes the half-written temporary file, and the recording's
  note says the disk was full ("free up some space, then reopen KreoKit Rec to recover it").
- **Margin:** finishing needs ≤ 2 MB and the heartbeat checks 4 times a second, so the stop
  at **0.5 GB stays** (also leaves room for the system and for `session.json`).
- **The stop now finishes by itself** (the user's choice): at 0.5 GB the recording pauses,
  the app stops and finishes it like the Stop button, and Finish opens with the note. Tried
  in the check copy (`KREOKIT_REC_DISK_STOP_AFTER_MB`, debug/test builds): the control showed
  the red space note, the recording stopped by itself, Finish opened, check clean.
- Warnings by minutes left at the recording's size per minute: a note at 10 min, red at 2
  (recording screen, floating control, menu bar); Home warns under 30 min of room.

## Step 9: the stop shortcut

- `tauri-plugin-global-shortcut` 2.4.1 and `global-hotkey` 0.8.0: MIT OR Apache-2.0. On macOS
  it registers with Carbon's `RegisterEventHotKey` (an event tap only for media keys), so
  **no Accessibility or Input Monitoring permission**: checked in the check copy, no prompt.
- ⌥⇧⌘S (Ctrl+Alt+Shift+S elsewhere) is registered only while recording. Pressed in the check
  copy (synthetic sources), it stopped the recording, Finish opened, check clean.
- Another app holding the keys: the recording goes on and the main window says so. Linux on
  Wayland: not offered (Home says to stop with the recording controls).

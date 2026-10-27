# M5 plan (Polish)

**Status:** approved by the user 2026-10-27 (choices below; OK given for the real-screen
measurements in steps 1 and 8). **M5 is done when it works and
is measured on the dev Mac (macOS 12), the Windows/Linux parts pass on CI, and the user has
tried it.** Real screen, camera or mic only with the user's OK in the chat. Order = commit
order; every commit builds and passes `cargo test` and `pnpm test` on its own and is pushed.

## The main problem: Rec's own windows on macOS 12

Everything M5 puts on screen while recording (countdown, floating control, camera bubble) is
recorded too on macOS 12 for displays and regions: CGDisplayStream ignores `sharingType` none
(M1). The camera bubble matters most: Finish adds the bubble at export, so a bubble also baked
into the screen picture would appear **twice**.

What each system can do:

| | Keep Rec's windows out | Where M5 puts the controls |
|---|---|---|
| macOS 13+ | ScreenCaptureKit `excludingWindows` (all Rec windows) | floating control + bubble, kept out |
| macOS 12, display/region | nothing (CGDisplayStream) — unless step 1 says otherwise | see choice 1 |
| any system, **window** recording | not needed: only the chosen window is recorded | floating control + bubble |
| Windows 10 2004+ | `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` (WGC and DXGI honour it) | floating control + bubble, kept out |
| Windows before 2004 | nothing (`WDA_MONITOR` shows a black box) | as macOS 12 |
| Linux (portal, x11grab) | no API | as macOS 12 |

**Step 1 measures one way out on macOS 12:** M1 found that ScreenCaptureKit on 12.7.6
delivers frames *only* when a window is excluded, and M3 measured SCK whole-display capture
at **99.93 % fresh** at 60→30. If ScreenCaptureKit with Rec's windows excluded holds ≥ 99 %
fresh over 10 minutes (screen + camera + mic, CPU comparable to CGDisplayStream), displays on
macOS 12 switch to it whenever a Rec window is on the recorded display, and macOS 12 gets the
same experience as 13+. Regions on 12 would need the same crop path on SCK (measured too:
black-picture check). If it fails, the fallback in choice 1 applies.

## Choices (confirmed by the user)

1. **Without exclusion (macOS 12 if step 1 fails, Windows before 2004, Linux): menu bar /
   tray.** While recording a display or region, Rec's main window hides; the controls live in
   the **menu bar** (macOS) / **tray** (Windows, Linux): time, Pause/Resume, Stop. The
   floating control and bubble are shown only on a second display that isn't recorded (put
   there automatically) or when recording a window.
2. **Countdown:** 3-2-1 by default, "No countdown" in Home's options (remembered). Esc or
   "Cancel" during the countdown cancels without recording anything.
3. **Stop shortcut: ⌥⇧⌘S** on macOS, **Ctrl+Alt+Shift+S** on Windows/Linux (⌥⌘S is "Save
   All" in VS Code and other apps people record tutorials of). Shown on Home and in the
   floating control. If the system or another app already owns it, Home says so and
   recording still works with the buttons. No change-the-shortcut setting in v0.1.
4. **Disk warnings** (by minutes left at the current rate, from `bytes_per_minute`): a calm
   note at **10 min left**, a red warning at **2 min left**, in the floating control / menu
   bar and the main window; Home warns before Record when the free space holds **less than
   30 min**. **Near full: the recording stops and finishes by itself, then opens Finish**
   with a note. Never lose a recording: the margin is set by the step 7 measurement so the
   finishing always fits.
5. **60 fps only if it measures fast enough** (step 8); otherwise Home keeps "30 fps".

## Steps

1. **Measure ScreenCaptureKit with Rec's windows excluded on macOS 12** (user's OK needed):
   `--bench fresh --api sck --exclude-own` with a floating test window on the recorded
   display; 3 × 60 s, then 10 min with camera + mic; fresh frames, CPU, and a screenshot-free
   pixel check that the excluded window never shows up in the file. Same for a region. Result
   in `docs/m5-results.md`; decides choice 1.
2. **Excluding Rec's windows (backend).** One list of Rec windows (main, floating control,
   bubble, countdown) handed to the recorder: macOS 13+ (and 12 if step 1 passes)
   `excludingWindows` by window number in `recorder.m`; Windows `WDA_EXCLUDEFROMCAPTURE`
   on each HWND when the build is ≥ 2004 (checked at runtime, else none); Linux none. The
   session records which windows were kept out; `RecordConfig` says whether exclusion is
   available so the UI chooses (choice 1). Tests: the synthetic recorder with a "Rec window"
   drawn into the synthetic display, kept out when excluded.
3. **Countdown.** A see-through always-on-top window on the recorded display (or around the
   region), 3-2-1, Esc/Cancel. The recorder is **prepared during the countdown** (devices
   opened, camera warmed up so its ~0.9 s start doesn't eat the first second) and starts
   writing when the countdown window is gone (ordered out + one display refresh, measured so
   no countdown frame reaches the file). Home option "Countdown: 3 s / Off".
4. **Floating control.** A small borderless always-on-top window (all Spaces, full-screen
   auxiliary, no shadow via `window.m`), draggable, position remembered per display: red
   dot + time, Pause/Resume, Stop, mic level, disk note (step 7). It must **not take focus**
   from the app being recorded (non-activating; measured with a click while typing in
   TextEdit on a test copy). The main window minimises at the start and comes back after
   Stop with Finish. The current Recording screen stays as the fallback (and for choice 1a).
5. **Menu bar / tray fallback** (choice 1a): Tauri's tray (`tray-icon` feature, no new
   crate) with the time as title on macOS, Pause/Resume, Stop; on Linux only where an
   AppIndicator is available (checked on CI with `libayatana-appindicator3`), otherwise the
   main window stays minimised and the shortcut stops.
6. **Camera bubble while recording.** A round always-on-top window (Small/Medium/Large,
   draggable, mirrored like a mirror — that's preview only; Finish's default stays not
   mirrored). Its pictures come **from the recorder's own camera** (a preview tap on the
   camera output, JPEG at ~15 fps through the existing `camera_preview_frame` path), not a
   second capture session, so the recorded camera never competes with it. Shown only where
   step 2 keeps it out (or window recordings). Hiding it doesn't touch the camera track.
7. **Disk: measure, then warn.** On a small APFS disk image in the scratchpad (`hdiutil`,
   ~3 GB), the check copy records synthetic sources with its app data on that image until the
   disk fills: measure what happens at 0.5 GB (pause, Stop, how long the finishing takes, does
   every track play, how much the moov/finish needs) and whether 0.5 GB is the right margin
   at 1080p60 + camera. Then: the warnings from choice 4, Home's "less than 30 min" warning,
   and the chosen end: **stop and finish by itself** (instead of "paused for good"), then
   open Finish with a note; the stop margin is raised if the measurement says finishing
   needs more. Never fills the user's disk.
8. **60 fps** (user's OK needed): `--bench fresh --record-fps 60` on the dev Mac, 3 × 60 s
   and 10 min with camera + mic: fresh frames, CPU, encoder queue / overload count. If it
   passes, Home offers 60 fps there (decided per machine by a short self-measurement, or per
   system if that is unreliable — decided by the numbers); bitrate and size per minute
   updated; Finish/export keep the frame rate as recorded. Windows/Linux: 60 fps only after
   friends' numbers (until then 30).
9. **Global stop shortcut.** `tauri-plugin-global-shortcut` (check licences with cargo-deny
   and `scripts/third_party.py`). macOS: it uses `RegisterEventHotKey`, which needs **no
   Accessibility/Input Monitoring permission** — verified on the test app before choosing it.
   Registered only while recording (so it never steals the key from other apps otherwise).
   Linux Wayland: global shortcuts don't work through X11 grabs; Home says "Stop shortcut
   isn't available here" (portal `GlobalShortcuts` is backlog).
10. **Tests + in-app check + docs.** Synthetic: countdown never in the file, excluded windows
    never in the file, shortcut stops (simulated), disk-full path. In-app check on the check
    copy (port 1500, window-only screenshots of the floating control and bubble, stopped
    afterwards). `docs/m5-results.md`, CLAUDE.md, manual checklist items for friends
    (exclusion on 13+/Windows, tray on Linux, shortcut, 60 fps). Test app built for the user.

## Not in M5

- Changing the shortcut, a pause shortcut, Linux Wayland shortcuts (portal) — backlog.
- Countdown sound, click highlights, keystroke display — backlog.

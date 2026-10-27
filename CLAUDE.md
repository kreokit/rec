# KreoKit Rec: build spec for Claude Code

Read this before every task. It is the source of truth for scope and architecture.
**Status (2026-10-27):** M1 done (`docs/capture-research.md`). **M2 done**: screen + mic on macOS (native recorder `src-tauri/native/macos`, sessions, check, recovery, permissions, Home, recording screen); synthetic tests (simulated hour, kill -9 recovery) and real-device measurements (`docs/m2-results.md`: 99.86 % fresh at 60→30 fps, sync within a frame plus ≈32 ms display delay, crash loss 0.15–0.4 s); the user tried the test app and everything worked. **M3** (camera + computer sound + window/region + Windows/Linux, plan `docs/m3-plan.md`, approved): **the macOS part (steps 1–8) is done and measured on the dev Mac** (`docs/m3-results.md`); **the Windows/Linux part (steps 9–14) is built and passes on CI** (2026-10-27, "Windows and Linux" at the end of this file), and the **test version for friends** (Tests screen, "Test version" workflow) is built. **M3 code done (2026-10-27).** Friends' results (Windows, Linux, macOS 13+) are tracked separately in `docs/m3-results.md` ("The test version for friends"); they don't block M4 but are **required before the M6 release**, and whatever they find gets fixed first. **M4** (Finish + export, plan `docs/m4-plan.md`, approved): built and measured on the dev Mac with synthetic recordings (`docs/m4-results.md`, "Finish and export" below); **the user tried it with a real recording: trim, bubble, voice and export all work. M4 done (2026-10-27).** **Next: M5** ("Next: M5" below). See "Capture choices" and "Testing on the dev Mac" below.
App #5 of the KreoKit suite. Published: KreoKit Captions (`kreokit/captions`), Cutout (`kreokit/cutout`), Clips (`kreokit/clips`), Voice (`kreokit/voice`). KreoKit Design was dropped and archived. Reuse the published apps wherever possible.

## What we are building

A cross-platform desktop app that records the **screen + camera + microphone** (and computer sound where the system allows it) into a clean, shareable MP4, fully offline:
choose what to record → 3-2-1 → record (pause/resume) → stop → quick finish (trim, layout, clean voice) → export MP4.

Target users: creators and people making tutorials, demos, reaction videos and quick explainers. English-first UI.

**Keep v0.1 small and solid.** A recorder is judged on one thing: it must never lose a recording, never drift out of sync and never drop frames. Fewer features that always work beat many features that sometimes fail.

## Hard rules (same as the other apps)

- **No proprietary code or assets.** Build only from public docs, open-source libraries and observed behaviour. Never copy another product's name, logo, icons or exact UI.
- **Offline first.** No servers, no accounts, no telemetry, no cloud uploads. Nothing leaves the computer.
- **Licences must stay MIT/Apache-compatible.** Crates and libraries: permissive only (check each with `cargo-deny`, record in `THIRD-PARTY-LICENSES.md`). Model rule (applies to all KreoKit apps): weights need an explicit MIT/Apache/BSD licence from the publisher; nothing in the model's own licence or card may restrict commercial use; training data checked and documented; prefer the cleaner option. Model mirrors are pre-releases; download only by tag.
- **FFmpeg: LGPL-only sidecars**, built and checked exactly as Clips/Voice do; a test keeps GPL-only filters and encoders out of every command.
- Every commit builds and passes `cargo test` and `pnpm test` on its own. Small topic-based commits. **Push after every step.**
- **Never modify the user's real files or projects for testing, not even temporarily.** Test instances use their own app data, cache **and WebKit** folders (separate identifier). Never delete anything outside your own test folders, including `~/Library` paths. Stop your dev server and test app afterwards so the dev port is free.
- **Synthetic input only into our own test instance.** Never send real mouse or keyboard events (CGEvent, osascript key codes, …) unless the target is a window of your own test instance, confirmed **frontmost at the target point and at the expected position right before each event** (the topmost on-screen window under the point belongs to the test instance, its bounds match; for normal windows the test instance is also the active app). If the check fails, skip the event and report it. Never click by screen coordinates alone. Use the checked click tool (`safeclick`, scratchpad) for every event.
- **Privacy:** never record or screenshot the user's real screen, camera or microphone without asking first in the chat. For automated tests use synthetic sources. Only screenshot the KreoKit window.
- **Don't guess about the user's apps.** Report a bug in another KreoKit app only if it is reproduced, not because similar code exists.
- Error messages say what the user can do next, never internal file names.
- macOS 12 WebKit needs `-webkit-` prefixed CSS (e.g. `-webkit-user-select`); a test checks every stylesheet. Text-selection highlight must never appear when dragging in the UI.

## Lessons from the other apps (must follow)

- **Release builds use a newer macOS SDK than dev builds.** Run `check_macos_binary.py` on every macOS release build. APIs newer than macOS 12 must be weak-linked and checked at runtime.
- **Dev ≠ release.** Dev builds use the Mac's GPL FFmpeg; releases use our LGPL FFmpeg. Test the real release encoder path in dev (`KREOKIT_DEV_ENCODER`, as in Clips), and the release `--self-test` must run it.
- Measure, don't guess: when a result looks wrong, find the cause with a measurement before changing code.
- Release workflow: build → all checks (bundle, licences, macOS binary check, `--self-test`) → upload only if everything passed → **draft pre-release** → the user tests the Intel macOS build on macOS 12 → publish as a pre-release → update the `kreokit/.github` profile README row (link to `/releases`, published apps listed first) and the "How to install" section.
- Dev port: **1490**; test instances **1500** (Captions 1420, Cutout 1430, Clips 1440, Voice 1450/1460, Design 1470/1480 archived).

## Reuse

Copy (don't reinvent):
- From **Clips**: FFmpeg sidecars + `verify_ffmpeg.py`, probe, H.264 encoder order and checks (VideoToolbox / Media Foundation / OpenH264; High or Main; yuv420p; BT.709; faststart), timeline + waveform, export with ffprobe checks, `awake.rs`, `KREOKIT_DEV_ENCODER`, release workflow and `--self-test` pattern.
- From **Voice**: RNNoise v0.2 (Fast) noise removal compiled in, loudness measurement and normalisation (−16 / −14 LUFS presets, true peak ≤ −1 dBTP), Recent list with "Remove from Recent".
- From **Cutout/Clips**: safe no-overwrite writing via a temporary file, conflicts first (Replace / Keep both / Cancel).
- From all: design tokens, UI kit, app shell, licence checks, issue templates, "Report a problem", brand assets, "Release checklist" in CLAUDE.md.

## Capture: decide in M1 with measurements

The hard part. Each system is different; research and measure before committing.

- **macOS:** ScreenCaptureKit (macOS 12.3+ for video; **system audio only on macOS 13+**) vs AVFoundation screen capture (works on 12, deprecated on newer macOS). Camera and mic via AVFoundation. On macOS 12 the app records screen + camera + mic; computer sound is shown as "Needs macOS 13 or newer". Never ask users to install a virtual audio driver in v0.1.
- **Windows:** Windows.Graphics.Capture or DXGI Desktop Duplication for the screen, WASAPI loopback for computer sound, Media Foundation for camera and mic.
- **Linux:** xdg-desktop-portal + PipeWire (Wayland and X11), with x11grab as a fallback on X11. PulseAudio/PipeWire monitor for computer sound.
- Options to compare: native APIs via permissive Rust crates, our own small native code, or FFmpeg input devices in our LGPL build (avfoundation, gdigrab/ddagrab, x11grab). Pick per system what is most reliable, keeping licences permissive.
- Measure on the dev machine (Intel i5, macOS 12): CPU use, dropped frames and A/V drift for 1080p30 screen + 720p camera + mic over 10 minutes.

## Capture choices (decided in M1/M2, confirmed by the user)

Details and numbers: `docs/capture-research.md`, `docs/m2-results.md`, `docs/m3-results.md`.

| | Screen | Camera, mic | Computer sound | Encode + write |
|---|---|---|---|---|
| **macOS 13+** | ScreenCaptureKit | AVFoundation | ScreenCaptureKit audio | VideoToolbox via AVAssetWriter, fragmented MP4 |
| **macOS 12** | CGDisplayStream for displays and regions; ScreenCaptureKit (12.3+) only for windows | AVFoundation | "Needs macOS 13 or newer" | same |
| **Windows 10/11** | Windows.Graphics.Capture (default update interval on older builds), DXGI duplication as fallback | Media Foundation camera; mic via WASAPI (cpal) | WASAPI loopback (cpal) | raw frames into our LGPL FFmpeg: Media Foundation `h264_mf`, then OpenH264 |
| **Linux** | xdg-desktop-portal ScreenCast + PipeWire (ashpd, pipewire); x11grab only on X11 without a portal | V4L2 (NV12/YUYV) camera; mic via cpal's PipeWire host | PipeWire capture-sink via cpal first; PulseAudio monitor only without PipeWire (cookie-less clients are rejected by pipewire-pulse) | raw frames into our LGPL FFmpeg: VA-API, then OpenH264 |

- Native capture code on every system, never FFmpeg input devices (gdigrab can't hold 30 fps; avfoundation has no SCK/window exclusion; x11grab only as the X11 fallback above). On Linux (and Windows, if measured fine) our LGPL FFmpeg sidecar only *encodes* frames we captured; it also does Finish/export.
- macOS code is our own Objective-C (`native/macos`, compiled with `cc`), C interface to Rust; APIs newer than 12 weak-linked + runtime checks.
- Screen: **capture at 60 fps, keep one frame per 1/30 s slot** (30 fps capture gave only 98.1 % fresh). A frame exactly on a slot boundary goes to the next slot. Capture NV12 (`420v`) wherever offered (BGRA doubles CPU).
- One fragmented MP4 per source, all on the host clock (`CMSyncConvertTime` for AVCaptureSession buffers); keyframe 1 s, fragment 1 s, regular `fsync`; a ≈1 s queue in front of each writer, never drop on "not ready"; overload is counted and shown, never silent.
- Camera (M3): fixed frame rate (min = max frame duration, kept inside the camera's own range: the FaceTime camera offers 30 fps as 1/30.00003 s and throws on 1/30). Its first picture comes ~0.9 s after the start; the slots before it show that picture. Measured against the mic: −4.9 ms, so no delay correction (`docs/m3-results.md`).
- Window (M3): ScreenCaptureKit window stream (`initWithDesktopIndependentWindow`; CGDisplayStream can't record windows), at the window's size on screen; shows the window even when covered. When it closes, the recording keeps its last picture and the check notes when. Measured on macOS 12: 100 % / 99.38 % fresh, so it's offered there (`WINDOWS_ON_MACOS_12` in lib.rs; set false with a note if a measurement fails). Rec's own windows aren't listed.
- Computer sound (M3, macOS 13+): ScreenCaptureKit audio on the screen's stream (or a sound-only stream), `system.mp4`, AAC stereo 48 kHz. Every sound track is padded with silence to the picture's end at the stop (the computer sound sends nothing while nothing plays); silence before a source's first sound is start-up, not a reported gap. Recovery never cuts the other tracks to a short computer-sound track. Built and unit-tested with the synthetic sound; **not tried on real 13+ hardware yet** (friends' checklist).
- Region (M3): output in the region's shape. **macOS 12: the whole display is captured at the size where the region comes out at the output size, and each written frame is cut out with a VTPixelTransferSession** (clean-aperture crop) before the encoder, so nothing outside the region reaches the disk; CGDisplayStream's own source rectangle records black on 12 (measured 2026-10-27). Cost: +1.5 points of one core vs whole display (`docs/m3-results.md`). ScreenCaptureKit (13+) uses its `sourceRect` (not verifiable here: friends' checklist). The check flags a black screen picture after every recording. Chosen in a see-through overlay window (`region_open`, `RegionOverlay.tsx`) above the menu bar (`native/macos/window.m`); needs Tauri's `macos-private-api` (transparent windows; fine outside the App Store). The overlay confirms with "Use region"/Enter and returns to Home, where Record starts (Home holds camera/mic).

## Finish and export (M4, done 2026-10-27)

Details and numbers: `docs/m4-results.md`.

- After Stop → **Finish** (`Finish.tsx`); Recent → "Finish & export". Choices per recording in `finish.json` (`finish.rs`; defaults: bubble when there's a camera, Medium circle bottom right, not mirrored, clean-up on, Tutorial −16).
- **Preview:** the tracks play directly (macOS 12 WebKit plays/seeks the fragmented MP4s, measured; no remux), sound leads (`lib/player.ts`), the leader is read only after a seek lands (WebKit reports the old time/`ended` meanwhile). Asset protocol limited to `$APPDATA/recordings/*/*.mp4|m4a`.
- **Layout maths** in `layout.rs` and its twin `lib/layout.ts`, both tested against `src/lib/layout-cases.json` (regenerate with `scripts/layout_cases.py`, an independent version). Change one → change all three.
- **Sound** (`sound.rs`): RNNoise (`rnnoise.rs`, `rnnoise/`, built by build.rs exactly as Voice) at 85 % with its delay removed, mic only, cached as `voice-clean.flac`; mixed with the computer sound; Voice's `polish.rs` (copied unchanged; gain + limiter only) and `loudness.rs` (trimmed copy). AAC read back and re-made with a lower ceiling if over −1 dBTP. Preview file `sound-<clean>-<loudness>.m4a` (one kept).
- **Export** (`export.rs`): sound made for exactly the trim and copied in; frame-accurate input seeking; bubble mask as a PGM FFmpeg loops; encoder order from `encoder.rs`; checks after writing as Clips plus loudness; `place()` never overwrites unless Replace. **Spiky sound** that can't reach the preset under −1 dBTP is accepted only when its true peak is at the limit (`held_by_peaks`), and the result says so.
- Exports go to Movies (Videos) or the folder in `prefs.json`. **In-app checks must point the check copy's `prefs.json` at a scratchpad folder first**, never the user's Movies.
- Build with `CARGO_INCREMENTAL=0` while disk is low (the incremental cache grew to 2.9 GB).

## Next: M5 (Polish)

Milestone 5 from "Milestones": countdown, floating controls, global stop shortcut, excluding KreoKit windows from capture, disk-space warnings, 60 fps where measured fast enough; plus the **camera bubble on screen while recording** (Screens 2, moved here from M3). Write `docs/m5-plan.md` and **show the plan to the user before coding**, as for M3/M4. Where things stand:

- **Recording screen** (`Recording.tsx`): big time, Pause/Resume, Stop, mic level, a low-disk warning under 2 GB and a "stuck" warning; it says the window is recorded too and to minimise it. No countdown, no floating control, no shortcut yet.
- **Disk:** `sessions.rs` refuses to start under 2 GB free (`START_BELOW_FREE`) and the recording stops itself under 0.5 GB (`STOP_BELOW_FREE`); Home shows free space and room ("about 1 h 30 min"). M5 adds the early, clear warnings (and what happens at the stop) — measure how the stop behaves before changing it.
- **Excluding Rec's windows (measured in M1, `docs/capture-research.md`):** `sharingType` none does **not** hide windows from CGDisplayStream or AVCaptureScreenInput on macOS 12, so on 12 the windows must be hidden/minimised (or the floating control kept off the recorded display); on 13+ ScreenCaptureKit's `excludingWindows` can keep them out (`recorder.m` builds its filters with `excludingWindows:@[]` today); Windows has `WDA_EXCLUDEFROMCAPTURE` (10 2004+); Linux portal: no exclusion API. ScreenCaptureKit on 12.7.6 delivers no frames unless at least one window is excluded (only used for windows on 12).
- **60 fps:** the screen is already captured at 60 fps and one frame kept per 1/30 s slot (`docs/m2-results.md`, 99.86 % fresh); recording *at* 60 fps needs a new measurement (fresh frames, CPU, encoder load) on the dev Mac with the user's OK, and is offered only where it measures fast enough. Home's Quality shows "30 fps" today.
- **Global shortcut:** no plugin yet (`tauri-plugin-global-shortcut` is MIT/Apache; check with cargo-deny and THIRD-PARTY-LICENSES via `scripts/third_party.py`). macOS may need Accessibility/Input Monitoring for some shortcut kinds: check before choosing.
- **Camera bubble while recording:** the camera preview pipeline exists (`camera_preview_start/frame`, small JPEGs at ~15 fps, `CameraPreview.tsx`); a floating always-on-top bubble window would reuse it, and like the floating control it is itself recorded on macOS 12 unless kept out (see above).
- **Testing:** in-app checks with the check copy (`com.kreokit.rec.claude-check`, built into the old scratchpad `target-check` folder) and `KREOKIT_REC_SYNTHETIC=1`; real screen/camera/mic only with the user's OK each time. Disk is tight (≈ 4 GB free on 2026-10-27): `CARGO_INCREMENTAL=0`, reuse old scratchpad target folders.
- **CI note:** CI runs the ignored tests (`--include-ignored`), so measurement-only tests must be gated by an environment variable (as `KREOKIT_MEASURE`). Commits b94a239 and b05f02f fail Linux CI for that reason (fixed in f551d9b).

## Testing on the dev Mac (test app)

- **Test app:** `test-build/KreoKit Rec Test.app` (git-ignored), identifier `com.kreokit.rec.claude-test`, product name "KreoKit Rec Test", `bench` feature. Build it only with `CARGO_TARGET_DIR=<scratchpad>/target-test scripts/build-test-app.sh` (config `scripts/test-app.json`). Its usage texts come from `src-tauri/Info.plist` (macOS won't ask without them).
- **Signed with a local test-only certificate** "KreoKit Rec Test (local)" (self-signed, login keychain, created 2026-10-27 with the user's OK, never for releases; the user clicked "Always Allow" so codesign signs without asking). Designated requirement = identifier + certificate leaf, so **Screen Recording, Microphone and Camera survive rebuilds**. Before 2026-10-27 it was ad-hoc signed (requirement = `cdhash`): every rebuild lost the permissions. If the script says the signature isn't certificate-based, stop and tell the user; never fall back to ad-hoc silently. Never give a helper tool the test app's bundle ID (LaunchServices + a different signature → permission loop). Never use the real identifier `com.kreokit.rec` for tests.
- **Automated in-app checks** use a separate copy (`rec-claude-test`, own app data, cache and WebKit folders) with `KREOKIT_REC_SYNTHETIC=1` (made-up display + mic + camera, synthetic recordings; the made-up display is 960×600 points at (240, 160), so a region overlay covers only that part of the real screen — screenshot it with `screencapture -l` of its own window, layer 1000) or `=ask` (permissions shown as missing); no real devices. Test instances on port 1500; stop them afterwards. The webview's accessibility tree is empty: clicks go through a small CGEvent tool in the scratchpad.
- **Real devices only with the user's OK in the chat, each time.** Delete the recordings afterwards, keep only the numbers.
- **Bench (test builds, `--features bench`):** `rec --bench check` (permissions + devices, no recording); `rec --bench fresh --out DIR [--seconds 60] [--api auto|cgd|sck] [--capture-fps 60] [--camera] [--mic] [--region|--window]` (counter window stepping 60×/s, still 2 s of every 10 s; reports fresh / stale / unchanged, CPU; `--region` records a region around it, `--window` the window itself); `rec --bench sync --out DIR --mic` (key-tap sync test); `rec --bench camera --out DIR` (clap test, camera + mic); `rec --bench-analyze DIR` prints the JSON report. Start the test app with `open -n -W --stdout FILE --stderr FILE "test-build/KreoKit Rec Test.app" --args …` (`/dev/stdout` makes the launch fail). Don't compile or run anything heavy during a measurement.
- **In-app clicks:** bring the test copy to the front (`osascript … set frontmost`) and confirm it's frontmost before every click, or the click lands in the user's own window.
- **Key-tap sync test** (done together with the user; a speaker clap was too quiet): the user presses the space bar firmly about once a second for 45 s; each press flashes a window white. The analysis finds clicks in the sound (30 dB above the room, not by the loudest press; the room is −84 dB, the user's mic input volume is 33 %) and flashes in the picture, and compares both to macOS's key event time. M2 result: click −6.7 ms, flash +31.6 ms (≈32 ms is the display's own delay), so the recording keeps picture and sound within one frame. M3 repeats it for the camera (a visible tap/clap in front of the camera against the mic click).
- **Screen Recording permission loop (fixed in M2):** when two ad-hoc copies share a bundle ID, or after any update of an ad-hoc app, System Preferences ties the permission to the wrong copy and ticking the box does nothing. Rec writes `screen-permission-asked` in its app data when it asks; if the next start still has no permission, the Permissions screen offers **Reset and try again** (`tccutil reset ScreenCapture <own identifier>`, its own entry only) and falls back to the – / + steps in System Preferences. macOS 12 applies Screen Recording only after a restart (the screen says so).

## Recording design (for reliability)

- **Each source is its own track while recording** (screen video, camera video, mic, computer sound), encoded in real time with the hardware encoder and written as **fragmented MP4** (or similar crash-safe format), so a crash or power loss keeps everything recorded up to the last second.
- All tracks share one clock; A/V sync is checked after every recording (drift must stay under one frame over an hour).
- The final layout (screen + camera bubble, etc.) is **composed after recording** in the Finish step, so the user can change the layout and nothing is baked in during capture.
- Crash recovery: on the next start, unfinished recordings are found and offered: "Recover recording from 14:32 (12:05 long)".
- Disk: show free space before and during recording; warn early and stop safely before the disk is full. Show the expected size per minute.
- Pause/resume without gaps or desync. Keep the computer awake while recording.

## Screens (v0.1)

1. **Home / Record setup:**
   - What to record: full screen (choose display), a window, or a region.
   - Camera: off / choose camera, with live preview. Microphone: choose mic, live level meter. Computer sound: on/off (or "Needs macOS 13 or newer").
   - Quality: 1080p or 720p, 30 fps (60 fps if the machine measured fast enough).
   - Big Record button; recent recordings below (with "Remove from Recent" and "Delete recording…").
2. **Recording:** 3-2-1 countdown, a small floating control (time, pause/resume, stop, mic level), a camera bubble preview on screen if the camera is on, a global shortcut to stop. The KreoKit windows themselves are excluded from the capture where the system allows it.
3. **Finish:**
   - Preview with a timeline; trim start and end (cutting in the middle is backlog).
   - Layout: screen only, screen + camera bubble (circle or rounded, corner, size, draggable), camera only.
   - Voice: "Clean up voice" (RNNoise Fast from Voice) and loudness preset (Social −14 / Tutorial & podcast −16 / keep).
   - Export MP4 (H.264 + AAC 48 kHz, same quality as recorded), naming `<recording>-YYYY-MM-DD-HHMM.mp4`, safe writing, checks after writing, "Show in folder".
4. **Permissions screen:** explains each permission (screen, camera, mic) in plain words, shows which are missing, and opens the right system settings page. On macOS, explain that the app must be restarted after allowing screen recording.

## Milestones

1. **Scaffold + capture research:** repo from Clips/Voice foundations, dev port 1490. Research and small measured prototypes for capture on each system (CI for Windows/Linux where possible, the dev Mac for macOS 12 with the user's permission). **Stop and present the findings and choices before building M2.**
2. **Screen + mic on macOS:** permissions screen, record full screen + mic into crash-safe tracks, pause/resume, stop, sync check, crash recovery, recent recordings.
3. **Camera + computer sound + Windows/Linux:** camera track with preview, computer sound where supported, window/region capture, the same features on Windows and Linux.
4. **Finish + export:** trim, layouts with camera bubble, voice cleanup + loudness, compose and export with checks.
5. **Polish:** countdown, floating controls, global stop shortcut, excluding KreoKit windows from capture, disk-space warnings, 60 fps where measured fast enough.
6. **Release:** reuse the release workflow; `--self-test` with synthetic sources (a generated moving test pattern + tone/speech as fake screen, camera and mic): record 10 s through the real recording path, pause/resume, recover a deliberately interrupted recording, compose a bubble layout, export and check size, duration, sync and encoder settings; draft pre-release v0.1.0-alpha.

## Tests (our oracle)

- A **fake capture source** (generated video with a frame counter and timestamps, generated audio with clicks at known times) used in unit and CI tests, so the whole pipeline is tested without a real screen or camera.
- Sync: the clicks in the audio line up with the matching video frames within one frame, also after pause/resume and after a 1-hour simulated recording.
- No dropped frames in the fake source at 1080p30 (frame counter has no gaps).
- Crash safety: kill the recorder mid-recording; the recovered file plays and is at most 1–2 seconds shorter.
- Export: exact size, H.264 settings, AAC 48 kHz, duration matches the trim, bubble at the right position (pixel check), no overwrites.
- Manual checklist for real devices (per OS), filled in by the user during testing: `docs/manual-checklist.md` (started in M3; includes a real region recording, since synthetic sources can't see a black capture). The `--self-test` runs synthetic sources, so it also runs the black-picture check on its recording, and the release checklist includes the manual real-capture items.

## Dev environment

- macOS 12 Monterey, Intel Core i5 (CPU only), 16 GB RAM, one built-in camera and mic. Free disk ~13 GB (2026-10-27) (run `cargo clean` in finished apps if needed).
- Windows 10/11, Linux (AppImage + .deb), macOS Apple Silicon (tested by friends).

## Release checklist

- Every item of `docs/manual-checklist.md` passed on each system it names (dev Mac on macOS 12, a macOS 13+ Mac, Windows, Linux), including the real region recording (a black or wrong-place region must fail it).
- After publishing, update the kreokit/.github profile README row (link to `/releases`, published apps first) and the "How to install" section.

## Windows and Linux (M3 part B, steps 9–14: done, passing on CI, 2026-10-27)

Details and CI results: `docs/m3-results.md` ("M3 part B"); plan `docs/m3-plan.md` part B. The dev Mac can't run Windows or Linux: **build and test them only on CI**; real-device numbers come from friends' test version.

- **The Rust recorder** (`src/pipeline.rs`): the macOS recorder's rules ported exactly (recorder.m stays the reference): slots, late/repeated, sound placed by timestamp, pauses, padding, status. Sources hand in `encoder::Picture`s (NV12 or BGRA) and 48 kHz float sound stamped on `pipeline::host_seconds()` (QPC on Windows, CLOCK_MONOTONIC on Linux). `native.rs`'s `portable::Recorder` wraps it; `native/win.rs` and `native/linux.rs` offer the same device/permission API as the macOS module.
- **The writer** (`src/encoder.rs`): one LGPL FFmpeg sidecar per track, raw pictures/PCM over stdin → fragmented MP4 (keyframe + fragment 1 s, `-flush_packets 1`), BT.709 tags via `setparams`; scaling and letterboxing in FFmpeg. A full queue repeats the previous picture or writes silence (counted). Encoder found once by test-encoding: `h264_mf` (Windows), `h264_vaapi` where `/dev/dri/renderD128` exists, OpenH264; libx264 only in debug builds; `KREOKIT_DEV_ENCODER` as in Clips. Every command passes `ffmpeg::gpl_only`.
- **Synthetic sources** in Rust (`src/synthetic.rs`, the same pattern as recorder.m); the recorder tests in `native.rs` and the kill -9 tests run on all three CI systems. On the dev Mac: `cargo test --features pipe-recorder native::` runs them against the Rust recorder (CI's macOS job does too).
- **Windows** (`native/win.rs`): `windows-capture` (MIT) for Windows.Graphics.Capture (displays, regions copied off the GPU alone, windows by HWND; `on_closed` keeps the last picture), DXGI duplication fallback; Media Foundation source reader for cameras (NV12, largest ≤ 720p at 30 fps); cpal WASAPI for mics and loopback (`src/audio.rs`, shared with Linux: format conversion, linear resampling to 48 kHz, host timestamps). Privacy switches read from the ConsentStore registry keys; the permission buttons open `ms-settings:privacy-webcam` / `-microphone`. CI records its real display (WGC, region, DXGI).
- **Linux** (`native/linux.rs`): ashpd ScreenCast + PipeWire stream (the system dialog chooses display or window; restore tokens in the app data folder, `native::set_data_dir`); a region is cut from the chosen display (not offered in the UI with the portal yet); x11grab through the sidecar only on X11 without the portal (CI tests it under Xvfb). V4L2 cameras (NV12/YUYV; MJPEG-only cameras not yet). cpal's PipeWire host needs **PipeWire ≥ 0.3.53**: CI and the Linux builds use **Ubuntu 24.04**; build deps `libpipewire-0.3-dev libspa-0.2-dev libasound2-dev clang`. Keep-awake via `systemd-inhibit`.
- **Preview JPEGs** on Windows/Linux come from our own baseline encoder (`src/jpeg.rs`): `jpeg-encoder` is also IJG-licensed, which cargo-deny refuses.
- **Test version for friends**: `.github/workflows/test-build.yml` ("Test version", run by hand) builds "KreoKit Rec Test" (`com.kreokit.rec.test`, `tauri.testversion.conf.json`, `--features bench`) for Windows (NSIS), Linux (AppImage + .deb), macOS Apple Silicon + Intel (DMG), as workflow artifacts. It opens on the **Tests screen** (`src/fieldtest.rs`, `src/components/TestScreen.tsx`, the pattern window `PatternWindow.tsx`) which runs `docs/manual-checklist.md` by clicking, deletes each recording after reading it back, and saves one results JSON to Downloads.
- **Open (friends' computers):** everything that needs real devices: WGC at 1080p/60 on real GPUs, Media Foundation cameras, WASAPI mic/loopback, the portal + PipeWire path on GNOME/KDE, V4L2 cameras; on macOS 13+ the ScreenCaptureKit `sourceRect` region, computer sound and window capture. Their results go into `docs/m3-results.md`; fix what they find before M4.

## Backlog (not v0.1)

- Cutting in the middle, zoom-in on clicks, cursor highlight, keystroke display
- Vertical (9:16) export for TikTok with the camera on top
- Captions with Whisper (from Captions), send to Clips
- Computer sound on macOS 12 via a virtual audio driver
- Scheduled recordings, webcam-only background blur (only with a model that passes the licence rule)

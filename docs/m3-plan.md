# Proposed M3 plan (camera + computer sound + window/region + Windows/Linux)

**Status:** approved by the user 2026-10-27. **M3 is done when it works and is measured on the dev Mac (macOS 12).** Friends' tests on Windows, Linux and macOS 13+ run in parallel and are required before the M6 release, not for M3. Order = commit order; every
commit builds and passes `cargo test` and `pnpm test` on its own and is pushed. Capture
choices are the ones in CLAUDE.md ("Capture choices"); nothing new is chosen without
measuring first.

## A. macOS (dev Mac, macOS 12)

1. **Camera track.** AVCaptureSession camera in `native/macos/recorder.m`: NV12, 1280×720
   (or the camera's nearest size), **fixed 30 fps** (min = max frame duration, so it doesn't
   halve in low light), retimed to the host clock, own `camera.mp4` (H.264, same writer,
   queue and `fsync` as the screen). Same slot rule as the screen: one frame per 1/30 s,
   missing slots counted. The synthetic source gets a fake camera (second counter
   pattern) so the tests cover three tracks.
2. **Camera permission + choice.** `NSCameraUsageDescription` in `src-tauri/Info.plist`,
   a Camera row on the Permissions screen (same pattern as the mic: ask, open settings,
   plain-words text), camera list + "Off" on Home.
3. **Live camera preview on Home.** Small frames (≈320×180 JPEG, 15 fps) from the native
   side to the webview. Same mechanism on all three systems, so it doesn't depend on the
   webview's own camera support (WebKit on macOS 12, WebView2, WebKitGTK all differ). The
   preview stops while the setup screen is hidden. (The on-screen camera bubble while
   recording stays in M5 with the floating controls.)
4. **Computer sound on macOS 13+.** *(Built 2026-10-27, unit-tested with the synthetic
   sound; real 13+ check by friends, `docs/manual-checklist.md` item 9.)* ScreenCaptureKit audio (weak-linked, runtime check)
   into its own `system.mp4` (AAC 48 kHz stereo). On macOS 12 the toggle shows
   "Needs macOS 13 or newer". **Can't be tried on the dev Mac**: built and unit-tested
   here, checked on CI macOS 14/15 as far as permissions allow there, and on a friend's
   macOS 13+ Mac (manual checklist).
5. **Region capture.** *(Done 2026-10-27. CGDisplayStream's source rectangle recorded black
   on macOS 12, so on 12 the whole display is captured and the region cut out on the GPU
   (VTPixelTransferSession) before the encoder: 100 % fresh, +1.5 points of CPU, see
   `docs/m3-results.md`. ScreenCaptureKit (13+) keeps its source rectangle; checked by
   friends, `docs/manual-checklist.md`.)* Choosing it: a see-through overlay on the chosen display,
   drag a rectangle, size shown, Enter/Record to confirm, Esc to cancel. Output keeps the
   region's shape, scaled to 1080p/720p at most, even sizes.
6. **Window capture.** *(Done 2026-10-27: measured on macOS 12 at 100 % / 99.38 % fresh, so
   offered there; `docs/m3-results.md`.)* ScreenCaptureKit window stream (as OBS does on macOS 12.5+;
   CGDisplayStream can't do windows). Window list with app name + title (thumbnails later).
   Before relying on it on macOS 12, measure fresh frames with `--bench fresh --api sck`
   on a window, **only with the user's OK**. If it can't reach ≥ 99 % on 12, the window
   option says so on 12 (or uses a region around the window) — decided by the
   measurement. When the window closes or minimises, the recording keeps going with the
   last picture and the check notes it.
7. **Check + recovery for every track.** *(Done 2026-10-27 for the camera; computer sound
   joins with step 4.)* `check.rs` and `recovery.rs` handle camera and
   computer sound: all tracks start together, same length (± one frame), camera slots
   without gaps, sync of every audio track against the picture; recovery cuts all tracks
   to the shortest. Pause/resume applies to every track.
8. **Real-device test on the dev Mac (with the user's OK):** *(Done 2026-10-27 for the
   camera: `docs/m3-results.md`; camera − mic −4.9 ms, so no correction; the region crop
   failed at first and was redone in step 5, then measured.)* camera fresh-frame and timing
   run (10 min screen 1080p + camera 720p + mic; CPU, missing frames, drift), and the
   **camera tap/clap test**: the user claps or taps the desk in view of the camera about
   once a second; the bench finds the hands meeting (largest frame-to-frame change) and
   the click in the mic, and reports camera − mic offset. Target: within one frame after
   subtracting a fixed camera delay, which the app then corrects automatically (camera
   delay is real and constant per camera; measured and stored, like other recorders do).
   Results in `docs/m3-results.md`; recordings deleted.

## B. Windows and Linux

9. **One writer for Windows and Linux: raw frames into our LGPL FFmpeg sidecar.** We do
   the slot timing ourselves (exactly one NV12 frame per 1/30 s, PCM audio with sample
   counts), so FFmpeg gets constant-rate input and only encodes: Clips' encoder order
   (Media Foundation `h264_mf` on Windows; VA-API, then OpenH264 on Linux), fragmented
   MP4 (keyframe + fragment 1 s), one process per track. If Rec is killed, FFmpeg sees the
   pipe close and finishes the last fragment. Built and tested first with the synthetic
   sources on all three CI systems (fresh frames, sync, simulated hour, kill -9 loss),
   before any real capture code. (Media Foundation's own fragmented-MP4 sink would avoid
   the pipe on Windows; only worth it if the pipe measures badly.)
10. **Windows capture** in Rust (`windows` crate, MIT/Apache): Windows.Graphics.Capture for
    displays, windows and regions (crop), DXGI duplication fallback for displays; camera
    and mic via Media Foundation (`IMFSourceReader`, NV12, fixed 30 fps), computer sound
    via cpal WASAPI loopback. Own windows excluded later (M5, `WDA_EXCLUDEFROMCAPTURE`).
    Permissions screen: camera/mic privacy switches (`ms-settings:privacy-webcam`,
    `privacy-microphone`) with plain-words text; screen needs none.
11. **Linux capture** in Rust: xdg-desktop-portal ScreenCast (`ashpd`) + PipeWire stream
    (`pipewire`), with a restore token so the system dialog doesn't come back every time;
    the system dialog is the "what to record" step (display/window/region as the portal
    offers). x11grab through the sidecar only on X11 without a ScreenCast portal. Camera
    via V4L2 (`v4l` crate, MIT; NV12/YUYV/MJPEG converted to NV12), mic and computer sound
    via cpal (PipeWire host first, PulseAudio monitor only without PipeWire). Plain-words
    errors when the portal or PipeWire is missing.
12. **Same features on all three:** sessions, pause/resume, check, recovery, Recent, disk
    estimate, keep-awake — shared Rust code already; the UI hides what a system can't do
    and says why.
13. **CI on all three systems** with the fake sources through the real writer (Windows
    and Linux runners have no screen worth measuring, no camera, no audio devices: the
    research showed this). `cargo-deny` + `THIRD-PARTY-LICENSES.md` for every new crate.
14. **Bench builds for friends** as CI artifacts (Windows .exe, Linux AppImage, macOS
    13+): `--bench check|fresh|sync` ported, plus the manual checklist (`docs/manual-checklist.md`: permissions,
    1080p30 fresh frames, camera, computer sound, key-tap sync, kill-and-recover). These
    runs happen in parallel and must pass before the M6 release (not needed for M3).

## Test app and its permissions

The camera code and the camera usage text need a new build of `KreoKit Rec Test.app`.
It is ad-hoc signed today, so its permissions are tied to the exact binary: **any rebuild
loses Screen Recording + Microphone**, and every later M3 rebuild would lose them again.

Approved: sign the test app with a **local self-signed code-signing certificate**
("KreoKit Rec Test (local)", created once in the login keychain, never used for releases).
macOS then ties the permissions to the identifier + certificate instead of the binary, so
the user allows Screen Recording, Microphone and Camera **once more** after the first
signed build, and they survive every rebuild after that. Same identifier
`com.kreokit.rec.claude-test`, same place `test-build/`. Removable from Keychain Access any
time.

## Not in M3

Finish/export and the composed layout (M4); floating controls, countdown, camera bubble
while recording, global stop shortcut, excluding Rec's windows, disk-full stop, 60 fps
output (M5). Disk: ~13 GB free on the dev Mac; Windows and Linux are built only on CI.

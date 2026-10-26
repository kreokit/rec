# Capture research (milestone 1)

How KreoKit Rec captures the screen, camera, microphone and computer sound on each system,
decided by measurement. Prototypes, scripts and how to rerun them:
[research/capture/](../research/capture/). CI runs: the "Capture research" workflow.

**Status:** everything that can run without real devices is measured. The macOS 12
comparison of the three screen APIs on the real screen, the real camera and mic, and
window exclusion are prepared (`research/capture/macos/run-real.sh`) but **not run**: they
record the user's real screen, camera and microphone and wait for the user's OK.

## What a recorder needs

CLAUDE.md: never lose a recording, never drift out of sync, never drop frames. Per option:

1. Works on our oldest targets: macOS 12 on Intel, Windows 10, Ubuntu 22.04/24.04 on X11
   and Wayland.
2. Delivers every frame at 30 fps (60 later) at low CPU, stamped on a clock the other
   tracks share.
3. Can pick a display, window or region, and keep KreoKit's own windows out (M5).
4. Licence: permissive code (MIT, Apache-2.0, BSD, Zlib), or the LGPL FFmpeg sidecar.

## Recommendation (short)

| | Screen | Camera, mic | Computer sound | Encode + write |
|---|---|---|---|---|
| **macOS 13+** | ScreenCaptureKit | AVFoundation | ScreenCaptureKit audio | VideoToolbox via AVAssetWriter, fragmented MP4 |
| **macOS 12** | to decide with the real-screen test: ScreenCaptureKit, CGDisplayStream or AVCaptureScreenInput | AVFoundation | "Needs macOS 13 or newer" | same |
| **Windows 10/11** | Windows.Graphics.Capture, DXGI duplication as fallback | Media Foundation (M3) | WASAPI loopback (cpal) | Media Foundation H.264 (M3) |
| **Linux** | xdg-desktop-portal ScreenCast + PipeWire; x11grab on X11 without a portal | V4L2/PipeWire (M3) | PipeWire capture-sink (cpal); PulseAudio monitor as fallback | OpenH264 / VA-API in our LGPL FFmpeg |

- **Native capture code, not FFmpeg input devices**, on every system. FFmpeg stays for the
  Finish step (compose + export), as in Clips. Reasons below per system.
- macOS capture as **our own small Objective-C** compiled into the app with the `cc`
  crate (like Voice compiles RNNoise), behind a small C interface called from Rust.
  ScreenCaptureKit weak-linked and checked at runtime (macOS 12.3+). The prototypes in
  `research/capture/macos/` are written that way already.
- **One fragmented MP4 per source**, all on the host clock. Measured below: no drops, no
  drift, under 1.1 s lost on `kill -9`.

## The recording path (measured)

`capbench` feeds synthetic sources (moving picture with a frame counter; clicks once per
second on the mic) through the planned path: VideoToolbox H.264 (High, BT.709, no
B-frames, keyframe every 1 s) via AVAssetWriter into fragmented MP4 (fragment every 1 s),
one file per source, all stamped with the host clock from the same start instant.
`analyze.py` reads the counter and clicks back out of the files.

**Dev Mac** (macOS 12.7.6, MacBookPro13,2, Intel i5-6267U 2 cores / 4 threads, Iris 550),
10 minutes, screen 1728×1080 (1080p for its 16:10 panel) + camera 1280×720 + mic 48 kHz:

| Run | Frames written (screen / camera) | Missing | A/V offset over 10 min | CPU while recording |
|---|---|---|---|---|
| NV12 input (like ScreenCaptureKit, camera) | 18000 / 18000 | 0 / 0 | 0.0 ms, drift 0.0 ms | process 17 % of one core; system +16.6 % of all 4 threads |
| BGRA input (like AVCaptureScreenInput default) | 18000 / 17999 | 0 / 1 | 0.0 ms, drift 0.0 ms | process 36 % of one core; system +15.4 % |

- File sizes for this content (8 / 3 Mbit/s targets): screen 179–200 MB, camera 95 MB,
  mic 0.8 MB per 10 minutes ≈ **28–30 MB per minute**. Worst case at the targets:
  ≈ 83 MB per minute. The app shows its own estimate (CLAUDE.md "Disk").
- The one camera frame lost in the BGRA run was dropped because the writer said "not
  ready" right after a 339 ms stall (another build was running on the Mac). **The app
  must not drop on "not ready"**: a short queue (≈ 1 s) in front of each writer.
- BGRA doubles our own CPU (colour conversion moves into our process): capture NV12
  (`420v`) wherever the API offers it.
- **Crash safety** (`kill -9` at a random moment, 12 runs, all files open and decode, no
  missing frames, sync 0.0 ms):

  | Keyframe / fragment interval | Lost at the end |
  |---|---|
  | 2 s / 1 s | 0.73–1.03 s |
  | 1 s / 1 s | 0.37–0.93 s (CI Macs: 0.9–1.07 s) |
  | 1 s / 0.5 s | 0.23–0.53 s |
  | 0.5 s / 0.5 s | 0.10–0.23 s |
  | fragments off | **nothing playable** (no `moov`) |

  → keyframe 1 s, fragment 1 s meets "at most 1–2 s shorter". `kill -9` doesn't test
  power loss (data the OS hasn't written to disk yet); M2 adds a regular `fsync`.
- **Overload is detectable.** On the CI Intel Mac (a VM without a hardware encoder) the
  writer fell behind: 142 of 900 screen frames missing in 30 s. `analyze.py` found exactly
  the frames capbench counted as "not ready". The app must notice this and tell the user
  (lower quality, or a clear warning), never drop frames silently.
- Building with the newest SDK (15.5) for macOS 12: works; ScreenCaptureKit is weak-linked
  (`otool -L`: "weak").

## macOS

| Option | Available | Status in the macOS 15.5 SDK | Own-window exclusion | Licence |
|---|---|---|---|---|
| ScreenCaptureKit | 12.3+ (audio 13+, picker 14+) | current | content filter `excludingWindows` | system framework |
| CGDisplayStream | 10.8+ | `SCREEN_CAPTURE_OBSOLETE(10.8, 14.0, 15.0)`: deprecated in 14, obsolete in 15 | window `sharingType` none (to measure) | system framework |
| AVCaptureScreenInput | 10.7+ | not marked deprecated; Apple's docs: "Starting in macOS 12.3, use ScreenCaptureKit" | `sharingType` none (to measure) | system framework |
| FFmpeg `avfoundation` | needs `--enable-avfoundation` in our LGPL build (LGPL) | uses AVCaptureScreenInput internally | none | LGPL sidecar |

- **OBS** (observed in its source, `plugins/mac-capture`): ScreenCaptureKit only from
  macOS 12.5, and on macOS 12 only for *window* capture (for displays it creates no
  stream there; an OBS commit gives "too many open issues on macOS 12" as the reason);
  displays on macOS 12 go through CGDisplayStream; on macOS 13+ ScreenCaptureKit for
  everything. So ScreenCaptureKit display capture on macOS 12 must
  prove itself on the real screen before we rely on it.
- **FFmpeg avfoundation: no.** It is AVCaptureScreenInput behind an extra process and pipe,
  with no ScreenCaptureKit, no window exclusion, no host-clock alignment with our other
  tracks; FFmpeg has no ScreenCaptureKit or PipeWire input at all.
- Camera and mic: AVCaptureSession, buffers retimed from the session's clock to the host
  clock (`CMSyncConvertTime`). The camera's frame rate is fixed (min = max frame
  duration), otherwise it halves in low light.
- Permissions: a command-line tool's permission belongs to the app that started it (the
  editor or Terminal). The test tool is therefore a tiny ad-hoc-signed app with its own
  identifier, `com.kreokit.rec.capreal-test`; the real app has its own anyway. On macOS 12,
  Screen Recording permission applies only after the app restarts (Permissions screen).

**Waiting for the user's OK** (`run-real.sh`, ≈ 16 minutes in all):

1. `permissions` (3 s each): makes macOS ask for capreal's screen, camera and mic access.
2. `screen` (60 s each): ScreenCaptureKit vs CGDisplayStream vs AVCaptureScreenInput at
   1728×1080, 30 fps. A small window shows a frame counter from the host clock;
   `analyze.py` reads it back, so every lost screen frame is counted.
3. `exclude` (10 s each): can that window be kept out of the capture (M5)?
4. `full` (10 min): the best screen API + camera 720p + mic, clicking through the speakers
   once per second, so the mic measures A/V drift end to end through real hardware.

The analysis reads only the counter's pixel rows and the click times; nothing is viewed or
uploaded, and the recordings are deleted afterwards.

## Windows

CI runners: Windows Server 2022 (build 20348, Windows 10 21H2 generation) and 2025, a
1024×768 Hyper-V display that refreshes at ≈ 32 Hz and no audio devices. They show
whether an API works and its CPU cost, not 30/60 fps on real hardware. A moving window was
shown during each run (every API sends frames only when the screen changes).

| Option | 2022 | 2025 | Notes |
|---|---|---|---|
| Windows.Graphics.Capture (`windows-capture`, MIT) | 32.8 fps; readback 23.0 fps | 28.1 fps; readback 30.0 fps | A custom minimum update interval is refused on 2022: use the default there. Windows, cursor and own-window exclusion (`WDA_EXCLUDEFROMCAPTURE`, Windows 10 2004+) |
| DXGI Desktop Duplication | 31.5 fps (0 % CPU); readback 24.2 fps | 36.1 fps; readback 33.2 fps (2.7 % of one core) | Whole monitors only; fallback |
| FFmpeg `gdigrab` (our LGPL build) | 24 fps capture only; 22 fps + 20 drops with encoding | 28 fps; 25 fps + 41 drops | **Can't hold 30 fps even at 1024×768**: rejected |
| FFmpeg `ddagrab` (our LGPL build) | fails: no D3D11 device | fails | Same API as DXGI duplication, needs a GPU |
| WASAPI loopback + mic (cpal, Apache-2.0) | no audio devices on the runner | same | needs a real PC |

Both screen APIs are capped by the VM's ≈ 32 Hz display, which explains the readback
differences. Recommendation: Windows.Graphics.Capture in our own Rust code (the `windows`
crate, MIT/Apache, or `windows-capture`), DXGI duplication where it is unavailable, cpal
WASAPI loopback for computer sound. Encoding (Media Foundation's hardware H.264 into
fragmented MP4 in-process, or raw frames into the FFmpeg sidecar) is measured in M3 on a
real PC; a friend's Windows 10/11 machine is needed for 1080p/60 fps and for audio.

## Linux

Ubuntu 24.04 runner, 4 threads.

**X11** (Xvfb 1920×1080, moving picture), our LGPL FFmpeg (BtbN build, as Clips ships):
devices x11grab, pulse, v4l2, kmsgrab present; encoders libopenh264, h264_vaapi, aac.

| Run (20 s) | Frames | CPU |
|---|---|---|
| x11grab 1080p30, capture only | 600 / 600 | 0.6 % of one core |
| x11grab 1080p30 + OpenH264 8 Mbit/s → fragmented MP4 | 600 / 600 | 42 % of one core (Constrained Baseline, as Clips accepts for OpenH264) |

**Wayland** (headless sway 1.9, PipeWire 1.0.5, xdg-desktop-portal + its wlr backend):

- Portal ScreenCast: **not measurable on the runner.** Our request goes through
  xdg-desktop-portal to its wlr backend and the compositor, which then fails: "unable to
  receive a valid format from wlr_screencopy". sway's GLES renderer would need a DRM
  render node; the runner has none and its kernel has no `vgem`. The headless pixman
  renderer is what remains, and screencopy offers it no format. So the frame delivery
  needs a real GNOME/KDE desktop (M3, friends' machines); `lincap portal` is ready for it.
- Computer sound:

  | Path | Result |
  |---|---|
  | cpal PipeWire host, input stream on the default output (capture-sink) | **works**: 48 kHz stereo, tone recorded, sample rate 1.001× real time |
  | PulseAudio protocol (cpal's pure-Rust client) via pipewire-pulse, with `~/.config/pulse/cookie` | works: the default sink's monitor records the tone |
  | the same **without** a cookie file | **fails** ("server error: Invalid") |

  pipewire-pulse requires a 256-byte cookie in AUTH but never checks it
  (`do_command_auth`, PipeWire 1.0.5); the client sends an empty one when the file is
  missing, as on a fresh PipeWire-only system. → PipeWire first; PulseAudio only on
  systems without PipeWire, where PulseAudio itself creates the cookie.
- Recommendation: portal + PipeWire in our own Rust code (`ashpd` MIT, `pipewire` MIT, the
  system's libpipewire MIT). The portal also works in X11 sessions of GNOME and KDE;
  x11grab only on X11 desktops without a ScreenCast portal. The portal asks the user to
  pick a screen or window itself (and can remember it with a restore token), so the
  Linux "what to record" step is the system dialog.

## Licences

Everything in the recommendation is permissive or a system library:

| Component | Licence | Used for |
|---|---|---|
| ScreenCaptureKit, AVFoundation, CoreGraphics, VideoToolbox | macOS system frameworks | macOS capture + encoding |
| `windows` (Microsoft) | MIT OR Apache-2.0 | Windows APIs |
| `windows-capture` | MIT | Windows.Graphics.Capture + DXGI (optional; may write our own on `windows`) |
| `cpal` | Apache-2.0 | WASAPI loopback, PipeWire capture-sink, mics |
| `pulseaudio` (pure-Rust client, via cpal) | MIT | PulseAudio fallback |
| `ashpd` | MIT | xdg-desktop-portal |
| `pipewire` / libpipewire | MIT / MIT | PipeWire streams |
| `objc2-*` (if Rust bindings are preferred over our Objective-C) | Zlib OR Apache-2.0 OR MIT | macOS |
| FFmpeg x11grab (xcbgrab), OpenH264, VA-API | LGPL sidecar (OpenH264: BSD, Cisco binary) | Linux X11 fallback, encoding |

Every crate goes through `cargo-deny` when it enters the app (M2/M3); the research
prototypes are not shipped.

## Open questions

1. macOS 12: which screen API (real-screen test) and whether own-window exclusion works.
2. The real mic's clock against the host clock over 10 minutes (drift the app must
   correct by resampling or timestamping).
3. Windows encoding path and real 1080p30/60 fps (needs a real PC).
4. Wayland on real GNOME/KDE desktops (portal dialog, restore token) — friends' machines.

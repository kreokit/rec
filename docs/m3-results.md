# M3 measurements on the dev Mac (2026-10-27)

MacBook Pro, Intel i5, macOS 12.7.6, built-in display recorded at 1728×1080, built-in
FaceTime HD camera at 1280×720, built-in microphone, 30 fps output. Recorded with the
user's OK through the app's own recorder (test build `KreoKit Rec Test`, `--features
bench`); all recordings deleted afterwards, only the numbers kept.

## Camera against microphone (clap test)

45 s of camera + mic (`--bench camera`); the user clapped about once a second in view of
the camera. 44 claps heard; 31 matched to the hands meeting in the picture (the largest
frame-to-frame change within ±250 ms of the sound, well above the usual change).

| | Median | 10–90 % | Within one frame (±33 ms) |
|---|---|---|---|
| Hands meeting in the picture − clap in the sound | **−4.9 ms** | −17.9 to +12.1 ms | 29 of 31 |

One match at −230 ms is another movement picked by the detector, one at −34 ms just
outside a frame. The spread is the 30 fps grid (±17 ms), centred on zero.

**Decision (by the measurement):** this camera has no delay of its own worth correcting,
so **no camera-delay correction is added**; the camera track lines up with the sound as
recorded. External webcams can differ: the friends' bench builds (step 14) run the same
clap test, and a per-camera correction is added only if one measures a real delay.

## Long run: screen + camera + mic, 10 minutes

`--bench fresh --camera --mic --seconds 600`: the counter window (60 steps a second,
still 2 s of every 10 s) in the top-left corner; the user worked normally. My own compile
(about 70 s) overlapped the first two minutes.

| | Result |
|---|---|
| Screen, fresh frames | **99.70 %** (14 450 fresh, 44 stale, 3 534 unchanged while the counter stood still); timestamps exactly 1/30 s apart |
| Screen, counter age at capture | median 33 ms, 95 % 50 ms, max 67 ms |
| Camera | 18 029 frames: 17 814 new, 215 repeating (30 before the camera's first picture, the rest pictures that came late), 0 skipped by the camera; **5 dropped at 0.97 s** (fixed, see below) |
| Microphone | no gap, nothing dropped |
| Tracks together | all three start at 0.0; camera ends with the screen (0.0 ms), sound 8.8 ms before |
| CPU (whole process, % of one core) | **25.3 %** |
| Size | screen 246 MB, camera 270 MB, sound 10 MB: about 3.1 GB an hour |

The check after the run found no problems (only the note about the 5 dropped camera
frames).

A 20 s run after the fixes (screen + camera + mic): camera 0 dropped, mic no gap.

## Region capture on the real screen

`--bench fresh --region` records only a region 24 points around the counter window and
reads the counter back: if the crop were in the wrong place, the counter couldn't be read.

**First try, CGDisplayStream's source rectangle (what macOS 12 used): black.** Only the
mouse pointer was drawn, and 2–4 new pictures arrived in 20 s although the counter changes
60 times a second. Tried 598×120 and 16-aligned 608×128 output, with a destination
rectangle, and BGRA instead of NV12: all black. The units were right (the header: display
points from the top-left). The synthetic tests couldn't see it: they don't use macOS's
capture.

**Now: whole display captured, region cut out on the GPU** (as OBS does: it captures the
whole display and crops). The display is captured at the size where the region comes out
at the output size; each written frame is cut out with VideoToolbox's pixel transfer before
the encoder, so nothing outside the region reaches the disk. Four 30 s runs, screen only,
Mac otherwise idle; the region at the display's own 2 pixels per point (996×202), so the
whole display is captured at its full 2880×1800, the heaviest case:

| Run | Output | Fresh frames | Counter read back | CPU (% of one core) |
|---|---|---|---|---|
| Whole display | 1728×1080 | 100 % | every frame | 15.2 % |
| **Region** | 996×202 | **100 %** | **every frame (0 unreadable)** | **16.0 %** |
| Whole display | 1728×1080 | 100 % | every frame | 15.5 % |
| **Region** | 996×202 | **100 %** | **every frame (0 unreadable)** | **17.6 %** |

The region costs about **1.5 points of one core more** than the whole display (16.8 % vs
15.4 % on average), although it captures 2.8× the pixels: light enough on the i5, so no
crop at export is needed. Timestamps regular, no late or dropped frames in any run.

A black screen picture can't pass unnoticed any more: the check after every recording
reads three frames back and reports a black picture (`check.rs`), and the manual
checklist (`docs/manual-checklist.md`) has a real region recording.

## Window capture on macOS 12

`--bench fresh --window`: the counter window recorded as a window through ScreenCaptureKit
(12.3+), at the display's own pixels (900×106), 60 s each, screen only, Mac otherwise idle.
For comparison the whole display through ScreenCaptureKit (`--api sck`).

| Run | Fresh frames | Stale | Counter read back | CPU (% of one core) |
|---|---|---|---|---|
| Window | **100 %** | 0 | every frame | 16.2 % |
| Window | **99.38 %** | 9 | every frame | 17.2 % |
| Whole display, ScreenCaptureKit | 99.93 % | 1 | every frame | 14.6 % |

**Decision (by the measurement):** above the 99 % target, so recording a window is offered on
macOS 12 (`WINDOWS_ON_MACOS_12` in `lib.rs`; if a later measurement fails, it's switched off
there with a note to record a region instead). When the window closes, the recording keeps its
last picture and the check says when (tested with the synthetic sources).

## Computer sound (macOS 13+)

Built and unit-tested with a synthetic computer sound (clicks on the whole second, stereo AAC
48 kHz, in sync with the picture); not measurable on the dev Mac (macOS 12, where the switch
reads "Needs macOS 13 or newer"). Checked by friends with item 9 of `docs/manual-checklist.md`.

## Found and fixed during the tests

- **Crash when the camera started:** the FaceTime camera offers 30 fps as 1/30.00003 s and
  throws on an exact 1/30. The frame duration now stays inside the camera's own range, and
  a refusal can no longer crash the app.
- **Empty camera file:** the camera's first picture comes ~0.9 s after the start, so the
  first slot never got one and the camera wrote nothing. Slots before the first picture
  now show it. The "no picture arrived" error never fired either (it measured from a slot
  that never moved); it does now.
- **"The microphone sent no sound for 0.4 s" after every recording:** the mic's first sound
  comes 0.35 s after the start. That silence still keeps the sound in sync, but isn't
  reported any more.
- **The window bench couldn't be read:** a window 450.5 points wide becomes 900 pixels (even),
  but the reader asked for a 901-pixel strip; it now reads 33 squares.
- **Region recorded black on macOS 12:** redone as whole display + GPU crop (above).
- **5 camera frames dropped at the start:** the ~28 slots filled with the camera's first
  picture overflowed the encoder's queue while it was starting. Repeated pictures don't
  count against the queue any more (0 dropped afterwards).

# M3 part B: Windows and Linux on CI (2026-10-27)

The dev Mac can't run Windows or Linux; these are CI's results (GitHub runners: Windows
Server 2022/2025 VM, Ubuntu 24.04 VM; no GPU, no camera, no audio devices). Real-device
numbers come from the test version's Tests screen on friends' computers (below).

**The Rust recorder** (`src/pipeline.rs`, the macOS recorder's rules) **with our LGPL FFmpeg
sidecar** (`src/encoder.rs`: raw pictures/PCM over stdin → fragmented MP4) passes the whole
recorder suite on both, with the synthetic sources (`src/synthetic.rs`, the same pattern as
recorder.m): every slot a new picture, picture and sound within a frame, pauses, still
periods, a late camera/mic start, a closed window, the region cut-out, the computer sound.
The same suite also passes on the dev Mac with `--features pipe-recorder` (VideoToolbox and
OpenH264 through FFmpeg).

| | Windows (Media Foundation `h264_mf`) | Linux (OpenH264) | dev Mac, pipe recorder (OpenH264) |
|---|---|---|---|
| Simulated hour (drift < 1 frame, no stale frames, ≥ 3598 markers) | passed, 144 s | passed | passed, 201 s |
| kill -9 (each file ≤ 3 s short on CI, ≤ 2 s on real computers) | passed | passed | 0.09 s per file |
| kill -9 through sessions + recovery | passed | passed | 0.31 s |

Killing Rec doesn't stop the FFmpeg processes: they see their input end and finish the file,
so only what was still queued in Rec (≤ 80 ms + the queue) is lost.

**Real capture on CI:**
- Windows: Windows.Graphics.Capture records the VM's display and a region of it (only the
  region is copied off the GPU), and DXGI duplication records the display: 3 s each, the
  right size, nothing dropped. Window listing works. Cameras, microphones and the computer
  sound need a real PC.
- Linux: under Xvfb without a ScreenCast portal, x11grab records a region of the X screen
  (3 s, every slot a new picture). The portal + PipeWire path can't be tried on CI (M1: the
  headless compositor offers no format); it needs a real GNOME/KDE desktop.

**Choices made while building (to confirm on friends' computers):**
- Picture timestamps: Windows.Graphics.Capture's SystemRelativeTime and DXGI's
  LastPresentTime are on the performance counter, the host clock there; Linux portal frames
  are stamped when they arrive (CLOCK_MONOTONIC); cameras (Media Foundation, V4L2) are tied
  to the host clock at their first picture unless their clock already is it.
- Microphones on Windows go through cpal's WASAPI (like the computer sound) instead of Media
  Foundation: one sound path, the same performance-counter timestamps.
- Sound from devices that don't run at 48 kHz is resampled linearly (fine for speech; a
  device at 48 kHz, the usual mix format, isn't resampled).
- Linux cameras: NV12 or YUYV only; cameras that offer only MJPEG aren't used yet.
- Linux needs PipeWire 0.3.53+ (cpal's PipeWire host): Ubuntu 24.04, Fedora 36+, Debian 12
  and newer. The Linux builds are made on Ubuntu 24.04.
- Linux with the portal: the system's dialog chooses the display or window; a region isn't
  offered there yet (the setup screen says so).

## The test version for friends

"KreoKit Rec Test" (`com.kreokit.rec.test`, the "Test version" workflow) opens on a Tests
screen that runs the manual checklist by clicking (`docs/manual-checklist.md`) and saves
one results file. Results go here, per computer, before the M6 release:

| Computer | Tests | Result |
|---|---|---|
| (none yet) | | |

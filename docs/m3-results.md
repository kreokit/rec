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
- **Region recorded black on macOS 12:** redone as whole display + GPU crop (above).
- **5 camera frames dropped at the start:** the ~28 slots filled with the camera's first
  picture overflowed the encoder's queue while it was starting. Repeated pictures don't
  count against the queue any more (0 dropped afterwards).

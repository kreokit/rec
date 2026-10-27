# M4 results (Finish + export)

Plan: `docs/m4-plan.md` (approved 2026-10-27). Everything below was measured on the dev Mac
(Intel i5, macOS 12) with synthetic recordings; no real screen, camera or microphone was
recorded. The export tests run on CI on all three systems.

## What Finish does

- After Stop the recording opens in **Finish**; Recent has **Finish & export**. The choices
  are saved per recording in `finish.json` (trim, layout, bubble, clean-up, loudness).
- **Preview + trim:** the tracks play straight from the recording, the sound leading and the
  pictures kept in step; a timeline with the waveform (mic + computer sound, kept as
  `waveform.bin`) and trim handles that snap to whole frames; I/O set the trim at the
  playhead, Space plays inside the trim.
- **Layouts:** screen; screen + camera bubble (circle or rounded 4:3, Small/Medium/Large =
  18/25/33 % of the shorter side, corner buttons or dragging, 3 % margin); camera alone at
  the camera's own size. `layout.rs` (export) and `layout.ts` (preview) do the same maths and
  are both tested against `src/lib/layout-cases.json`, worked out independently by
  `scripts/layout_cases.py`.
- **Voice:** RNNoise v0.2 from Voice (85 % Strength, its 20 ms delay removed) on the
  microphone only, kept as `voice-clean.flac`; mixed with the computer sound; Voice's chain
  (gain + true-peak limiter only) and measure-and-correct loop bring the mix to the preset.
  The preview plays the whole recording made this way (`sound-<clean>-<loudness>.m4a`, one
  file kept); a new choice cancels the running job.
- **Export:** the sound is made again for exactly the trimmed part (so the preset is met on
  what is exported) and copied in; the pictures are seeked frame-accurately, the camera cut
  and scaled with a smooth mask and laid over the screen; encoder order and settings as the
  recording (BT.709, the recording's bitrate, 1 s keyframes, faststart). Read back after
  writing (size, profile, yuv420p, BT.709, faststart, AAC 48 kHz, every stream within a frame
  of the trim, loudness ±0.5 LU, true peak ≤ −1 dBTP), then moved into place: conflicts are
  asked first (Replace / Keep both / Cancel), never overwritten otherwise. Movies folder by
  default, the chosen folder remembered (`prefs.json`); free space checked first; computer
  kept awake; Cancel leaves nothing behind.

## Measurements

### Preview in macOS 12 WebKit

- WebKit plays and seeks the **fragmented AVAssetWriter tracks directly** (duration and
  seekable range right, seeks land), checked with a WKWebView probe and in the app. **No
  remux copy is made** (saves disk space).
- Picture against the sound clock while playing (frame counter read from screenshots):
  1.633 s at "0:01.6", 3.467 s at "0:03.5"; the camera 2.63 / 3.90 / 5.37 s at "0:02.7 /
  0:03.9 / 0:05.3"; a seek to 0:10 shows frame 10.017 s.
- **Bug found and fixed:** right after a seek WebKit can still report the old time and
  `ended` (logged: `ended` and `seeking` true together), which made Play jump to the end.
  The leader is now read only once the seek has landed; 6 of 6 seek-then-play runs right
  afterwards.
- Trim 2.0–14.0 s: Play starts at 2.0 and stops at 14.017 s (frame 841 of 60 fps capture).

### Sync through the export (test `picture_and_sound_stay_in_sync_through_the_export`)

Screen flashes on whole seconds, the mic taps with them over a noise floor; trim from 0.5 s,
clean-up on, Tutorial −16, AAC, bubble: **tap − flash +0.0 ms for all six** with
VideoToolbox, the release encoder order and OpenH264. (An unremoved RNNoise delay would show
as +20 ms, mishandled AAC priming as about ±21 ms.)

### In-app export (synthetic recording, release encoder order)

- 1728×1080 High, yuv420p, BT.709 tv, 30 fps, AAC LC 48 kHz stereo; 12.000 s for a 12.0 s
  trim; the export's first frame is the recording's 2.000 s, its 0:05 the recording's 7.000 s.
- Bubble where the preview shows it (also the pixel test `exports_the_bubble_where_the_preview_shows_it`).
- Keep both → `…-2.mp4`; Replace then Cancel → the original unchanged (same MD5), no
  temporary file left.

### Spiky sound

The synthetic microphone is clicks only: 24 dB between peak and loudness. At −16 its peaks
would reach about +8 dBTP; the limiter holds them at −1.1 dBTP and the loudness stops at
**−22 LUFS**. Decision: never going over −1 dBTP wins (louder would distort). The check
accepts a quieter result **only** when the true peak is at the limit (≥ −2 dBTP); the result
dialog says the sound is a little quieter and why. Real speech (12–18 dB) reaches the presets
(tests with speech-like sound: within ±0.5 LU).

### Speed (ignored test `measure_ten_minute_export`)

10:00 of a moving 1728×1080 screen + 720p camera bubble + speech-like mic, clean-up on,
Tutorial −16: **213.7 s, 2.8× faster than real time** (VideoToolbox; −16.0 LUFS, −1.1 dBTP,
558 MB): sound 73.9 s (including RNNoise over the 10 min, no preview made before), picture
136.3 s, checks 3.5 s. Above the plan's ≈2× threshold, so no copy-without-re-encoding path.

## Notes

- The preview's sound is measured over the whole recording, the export's over the trimmed
  part, so after a big trim the export can be slightly louder or quieter than the preview;
  the export is the one checked against the preset.
- "Keep" leaves the loudness as recorded but still keeps true peaks under −1 dBTP (the mic
  and computer sound are added together), as Voice's "Off" does.
- The user tried the test app with a real recording (2026-10-27): trim, bubble, voice and export all work. **M4 done.**

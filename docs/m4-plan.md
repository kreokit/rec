# M4 plan (Finish + export)

**Status:** approved by the user 2026-10-27. **M4 is done when it works and is measured on
the dev Mac (macOS 12) with synthetic recordings, and the export tests pass on CI on all
three systems.** Real screen, camera or mic only with the user's OK in the chat. Order =
commit order; every commit builds and passes `cargo test` and `pnpm test` on its own and is
pushed.

## Choices (confirmed by the user)

1. **Default loudness preset: Tutorial & podcast −16 LUFS.** (Social −14, Keep.)
2. **Exports go to the Movies folder** by default, with "Change…"; the chosen folder is
   remembered.
3. **After Stop the recording opens in Finish** (the check result is shown there, not in a
   toast). Recent gets **"Finish & export"**.
4. **Bubble: circle, bottom right, Medium** (25 % of the picture's shorter side), **not
   mirrored** (text in view reads correctly).
5. **"Clean up voice" is on by default**, RNNoise at Voice's measured Fast Strength of
   **85 %**.

## Steps

1. **Finish settings + screen shell.** `finish.json` per recording (trim, layout, voice,
   preset), so reopening keeps the choices. Finish screen and navigation: after Stop, from
   Recent ("Finish & export"); recovered recordings open the same way.
2. **Preview + trim.** First measure whether macOS 12 WebKit plays our fragmented MP4
   tracks smoothly as they are. If yes: play them directly (camera muted, kept in step
   with the screen video; the sound as audio). If not: Finish makes a quick copy remux into
   a normal MP4 (costs disk space, so only if needed). Timeline with the mic waveform
   (Clips' `waveform.rs`), start/end handles snapping to whole frames, play/pause,
   arrow and space keys. One trim for all tracks (they share one clock); each track's
   measured start from the check is applied.
3. **Layouts.** Screen only, screen + camera bubble, camera only (only "screen only"
   without a camera). Bubble: circle or rounded (rounded 4:3 crop), Small/Medium/Large,
   corner preset + dragging; position stored relative to the picture, kept inside a
   margin. Geometry in Rust `layout.rs` with a TypeScript twin, both tested against the
   same values, so the CSS preview and the export can't disagree. Camera only: the camera
   fills the output picture (cropped to fit).
4. **Voice: clean-up + loudness (from Voice).** RNNoise v0.2 (vendored C, `build.rs`,
   pinned model + checksum), `ebur128`, the loudness gain + true-peak limiter part of
   Voice's chain with its measure-and-correct loop. Clean-up applies to the **mic only**
   (blended at 85 %); then mic + computer sound are mixed and the preset is applied to the
   mix (±0.5 LU, true peak ≤ −1 dBTP); "Keep" leaves the level as recorded. One processed
   sound file per settings combination in the recording's folder, made in the background
   with progress; the preview switches to it when ready. Voice's de-esser/compressor and a
   computer-sound volume slider stay out (backlog).
5. **Compose + export (backend).** One FFmpeg graph: trim, scale the screen, the camera
   cut to its bubble with a smooth-edged mask drawn in Rust, overlay, processed sound.
   Every command passes `gpl_only`. Encoder order from Clips (VideoToolbox / Media
   Foundation / OpenH264), High or Main, yuv420p, BT.709, faststart, bitrate as recorded,
   AAC 48 kHz stereo; `KREOKIT_DEV_ENCODER`. Safe writing via a temporary file, conflicts
   first (Replace / Keep both / Cancel), free space checked against the estimated size,
   progress, Cancel, keep awake. Name `<recording>-YYYY-MM-DD-HHMM.mp4`.
6. **Checks after writing.** ffprobe: size, codec, profile, pixel format, colour tags,
   faststart, length = trim ± 1 frame, AAC 48 kHz; loudness and true peak measured
   against the preset. A failure says what to do next. "Show in folder".
7. **Export UI.** Export panel (folder, name, conflict dialog, progress, Cancel, result).
   In-app check on the test copy with synthetic sources (port 1500, window-only
   screenshots, stopped afterwards).
8. **Tests + measurements**, then `docs/m4-results.md` and CLAUDE.md. Synthetic tracks:
   exact size and encoder settings, length matches the trim, **bubble pixel check**
   (camera pattern inside the expected circle, screen at its corners and outside), sync
   (known clicks still match their frames after the trim), loudness target, RNNoise
   lowers synthetic noise, nothing overwritten, GPL check on every command; on CI for all
   three systems. On the dev Mac (synthetic): export speed and CPU for 10 min 1080p +
   720p bubble, voice processing time. If export is slower than ≈2× real time, measure a
   copy-without-re-encoding path for "screen only, no trim" before deciding. Then the
   user tries the test app (with their own recording only if they want to).

## Not in M4

Camera bubble while recording, floating controls, disk warnings (M5). Cutting in the
middle, 9:16 export (backlog).

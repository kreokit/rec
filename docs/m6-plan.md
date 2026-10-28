# M6 plan: release v0.1.0-alpha (draft)

Written 2026-10-28. The user asked for M6 up to the **draft** pre-release without waiting
for approval of this plan. Nothing is published; the user tests the Intel macOS build on
macOS 12 first (Release checklist in CLAUDE.md). Push after every step.

No real screen, camera or microphone is recorded in M6: the self-test and every check use
the synthetic sources.

## Steps

1. **This plan.**
2. **`rec --self-test`** (`src/selftest.rs`, the Clips pattern: a hidden mode that runs
   before Tauri starts, needs no window, exits non-zero on any failure). It runs the shipped
   binary with its bundled FFmpeg on the synthetic sources (moving test pattern with a frame
   counter as screen and camera, a click every second as the mic):
   1. FFmpeg is the bundled one (release builds) and an LGPL build.
   2. **Record** through the app's own path (`Recordings`, the real recorder, real-time
      synthetic sources): 1080p screen + 720p camera + mic, with the countdown hold, a
      pause and a resume, about 10 s recorded. The after-recording check (`check.rs`) must
      find no problems; the frame counter must show no missing or repeated frame except
      where the recorder counted a late one; every second's marker within one frame of its
      click; H.264 yuv420p BT.709 and AAC 48 kHz.
   3. **Recover** a deliberately interrupted recording: a child process (`rec
      --self-test-child`) records and is killed; the recovery as at the next start must
      keep all of it but the last 2 s (3 s on CI's VMs, as the tests) with no problems.
   4. **Finish + export** the first recording through Finish's own code: bubble layout
      (Medium circle bottom right), trim, voice clean-up, Tutorial −16 LUFS. Export's own
      checks (size, H.264 profile/yuv420p/BT.709, AAC 48 kHz, duration = trim, faststart,
      loudness, no overwrite), plus: the bubble at its place with its circle (pixel check
      on the camera's marker strip), and picture-sound sync through the export (markers
      against clicks, within one frame).
   `testpattern.rs` becomes part of every build for this. Tested by a unit test and by
   running the debug build on the dev Mac (synthetic sources only).
3. **Release workflow** (`.github/workflows/release.yml`), copied from Voice's/Clips' with
   Rec's build from `test-build.yml`: checks job (tests, cargo-deny, THIRD-PARTY) → draft
   pre-release, never Latest → builds for macOS Apple Silicon, macOS Intel, Windows,
   Linux (Ubuntu 24.04 for PipeWire ≥ 0.3.53) → bundle contents (licences, FFmpeg,
   Info.plist usage texts, real identifier `com.kreokit.rec`), no AppIndicator libraries
   in the Linux bundles, `check_macos_binary.py` (macOS 12), `--self-test` → upload only
   if everything passed.
4. **Release notes against the real build:** after a first run, compare the file names
   and requirements in `docs/release-notes/v0.1.0-alpha.md` with the uploaded assets and
   fix whichever is wrong; keep "Tested so far" true.
5. **Draft v0.1.0-alpha:** run the workflow, check the draft (pre-release, not Latest, all
   files present), download the Intel `.dmg`, mount it in the scratchpad and run its
   `--self-test` on the dev Mac (macOS 12, Intel), without installing it.
6. **CLAUDE.md status** and the one command for the user's test.

## After the draft (needs the user, not part of this run)

- The user installs the Intel build, runs `--self-test`, and goes through
  `docs/manual-checklist.md` on the dev Mac with the real screen (including the real region
  recording).
- Then publish as a pre-release (never Latest; check `gh release list` afterwards), update
  README's status/downloads and the kreokit/.github profile row and "How to install".

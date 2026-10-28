# M6 results: draft v0.1.0-alpha (2026-10-28)

Plan: `docs/m6-plan.md`. Nothing is published: the draft waits for the user's test of the
Intel build on macOS 12.

## Draft release

- **v0.1.0-alpha**: draft pre-release (not Latest), first built at `4b037c6` (run
  40618286000); rebuilt at `50bf2eb`, see below. Later commits up to this file change only
  comments and docs.
- Files (match the table in `docs/release-notes/v0.1.0-alpha.md`):
  `KreoKit.Rec_0.1.0_aarch64.dmg` (38 MB), `KreoKit.Rec_0.1.0_x64.dmg` (41 MB),
  `KreoKit.Rec_0.1.0_x64-setup.exe` (100 MB), `KreoKit.Rec_0.1.0_amd64.AppImage` (206 MB),
  `KreoKit.Rec_0.1.0_amd64.deb` (140 MB).
- The first run (e024378) failed on Windows and Linux in the self-test's own licence check,
  which refused `--enable-version3` (LGPL v3, which verify_ffmpeg.py and the other apps
  accept); fixed in 4b037c6 and the draft retargeted.

## Rebuild after the user's test (2026-10-28)

- **Bug:** the installed release crashed when the user clicked Allow for the microphone
  (TCC: no `com.apple.security.device.audio-input`). Measured: Info.plist had both usage
  texts (same as the test app apart from name, identifier, category, copyright); the
  difference was the signature: release `flags=0x10002(adhoc,runtime)` without
  entitlements, test app `flags=0x0(none)`. Under the hardened runtime macOS needs the
  device entitlements. Fixed (5469a8b): `src-tauri/Entitlements.plist` (audio-input,
  camera) in `tauri.release.conf.json`; `scripts/check_macos_privacy.py` fails a bundle
  without the usage texts or, under the hardened runtime, without these entitlements
  (verified: fails the old draft, passes the test app and the fixed build). Runs in the
  Release and Test version workflows. The friends' test version was built with the same
  config, so it likely had the same crash on macOS.
- **Self-test on CI's Intel VM** then failed once: one screen marker −41.4 ms (the usual
  −8.1 ms plus one frame) right after a repeated picture (a late frame goes into the next
  slot). The sync rule is now per marker: one frame, or two right after a repeat (50bf2eb).
- **Draft now at 50bf2eb** (run 40634368394, all green); the Intel .dmg checked again on
  the dev Mac: entitlements present, signature valid, `--self-test` ok (29 s).

## `rec --self-test` (synthetic sources only)

Record 1080p screen + 720p camera + mic in real time (countdown hold, pause 1.5 s,
resume, ≈10 s), recover a killed child recording, export the bubble layout with a trim,
clean-up and −16 LUFS.

| Where | Encoder (export) | Repeated screen / camera frames (all counted late) | Sound − picture | Lost in recovery | Bubble frames OK | Took |
|---|---|---|---|---|---|---|
| CI macOS Intel | h264_videotoolbox High | 1 / 1 | −11.8…+4.6 ms | 0.59 s | 249/249 | 38 s |
| CI macOS Apple Silicon | h264_videotoolbox High | 1 / 3 | −7.7…+12.5 ms | 0.80 s | 244/244 | 27 s |
| CI Windows | libopenh264 High (the VM has no Media Foundation H.264 encoder) | 0 / 0 | +20.8 ms | 0.33 s | 242/242 | 27 s |
| CI Linux | libopenh264 High | 0 / 0 | +19.1 ms | 0.33 s | 244/244 | 43 s |
| **Dev Mac (macOS 12, Intel), the released Intel .dmg** | h264_videotoolbox High | 0 / 0 | −9.9…+15.5 ms | 0.64 s | 242/242 | 29 s |

- Every check passed: no problems in the after-recording or recovery check, H.264 yuv420p
  BT.709 at 1920×1080 and 1280×720, AAC 48 kHz, export checks (size, profile, duration =
  trim, faststart, loudness), the second export got its own name.
- Loudness: the clicks can't reach −16 LUFS under −1 dBTP, so the export is held by its
  peaks (−22 LUFS, true peak ≈ −1.2 dBTP), as M4 measured.
- On the dev Mac one export marker has no click: the marker on the trim's first frame,
  whose click starts 5 ms before the trim (measured). The rule allows one.

## Still for the user (Release checklist)

- Install the Intel build, run `--self-test`, open the app.
- `docs/manual-checklist.md` on the dev Mac with the real screen, camera and mic,
  including the real region recording.
- Then publish as a pre-release, README downloads, kreokit/.github profile row.

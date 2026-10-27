# M2 measurements on the dev Mac (2026-10-27)

MacBook Pro, Intel i5, macOS 12.7.6, built-in 2880×1800 display recorded at 1728×1080,
30 fps output, built-in microphone. Recorded with the user's OK through the app's own
recorder (test build `KreoKit Rec Test`, `--features bench`); all recordings deleted
afterwards, only the numbers kept.

## Fresh frames (target ≥ 99 % while the screen changes)

A counter window stepping 60 times a second, standing still 2 s of every 10 s
(`--bench fresh`, 60 s each, CGDisplayStream). A repeated picture is "unchanged" when the
counter stood still, "stale" when it had moved on but the recording didn't show it.

| Capture rate | Frames | Fresh | Stale | Unchanged (still) | Fresh share | Late / dropped |
|---|---|---|---|---|---|---|
| **60 fps, one kept per 1/30 s** | 1793 | 1436 | **2** | 354 | **99.86 %** | 0 / 0 |
| 30 fps | 1792 | 1410 | 27 | 354 | 98.1 % | 0 / 0 |

Timestamps exactly 1/30 s apart in both. 60 fps capture is the default.

## Sound against picture (key-tap test)

The user pressed the space bar firmly about once a second for 45 s (`--bench sync`); each
press flashed a window white. 68 presses, 67 flashes found in the picture, 54 clicks in the
sound (room at −84 dB, presses −3 to −27 dB peak). Times relative to macOS's key event:

| | Median | 10–90 % |
|---|---|---|
| Click in the sound − key event | **−6.7 ms** | −7.5 to −5.4 ms |
| Flash in the picture − key event | **+31.6 ms** | +16.5 to +49.1 ms |
| Flash − click (as a viewer sees it) | +38 ms | +23 to +55 ms |

Reading it:
- **Sound track:** each click lands within ~2 ms of the same place every time, 6–7 ms
  before macOS's event time (the key clicks as it goes down; the event follows). The mic
  timing is exact.
- **Display delay:** the flash needs to be drawn and shown by macOS: ≈ 32 ms (about two
  60 Hz refreshes) after the key event. That is the screen's own delay, not the
  recording's.
- **The recording's own picture offset:** the flash's spread around those 32 ms (±16–17 ms)
  is the 30 fps grid (a picture is shown at the slot nearest its capture, at most half a
  frame away), centred on zero. So the recording itself puts picture and sound within one
  frame of each other, as the synthetic tests measured exactly (0.0 ms). A direct
  measurement would log each frame's capture time against the slot time; not done here,
  since that needs a rebuild and new permissions.

## Crash safety (target ≤ 1–2 s lost)

Recording from the app's Record button, `kill -9` after 20.15–20.4 s (session.json heartbeat),
reopened: "Recover recording from 02:17 (0:20 long)" → Recover: **20.0 s recovered (0.15–0.4 s
lost)**, picture 600 frames, both tracks start at 0.0, check clean.

## Found and fixed during the test

- The screen list was empty (`"main": 1` instead of `true` in the native JSON).
- Screen Recording permission loop: two ad-hoc signed copies shared the bundle ID; System
  Preferences tied the permission to the wrong one. Same effect after any update of an
  ad-hoc signed app. Rec now remembers that it asked and offers Reset and try again
  (tccutil reset for its own ID), with the – / + steps as a fallback.
- The key-click detector's threshold (now 30 dB above the room).

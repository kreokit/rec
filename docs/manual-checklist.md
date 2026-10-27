# Manual checklist for real devices

The automated tests use made-up sources: they go through the recorder, the encoder and the
check, but **not through the system's real capture**. A real capture can fail in ways they
can't see (on macOS 12 a region recorded black and every test passed). These items are run
by hand on real devices before every release, on each system: the dev Mac (macOS 12), a
macOS 13+ Mac, Windows and Linux (`CLAUDE.md`, "Release checklist"). Each item records the
real screen, camera or microphone: only with the person's OK, recordings deleted after.

The bench commands run the test build (`--features bench`); on macOS start it as the app
bundle, e.g. `open -n -W --stdout out.log --stderr out.log "KreoKit Rec Test.app" --args
--bench …`, then `rec --bench-analyze DIR` prints the result.

**The test version for other people** ("KreoKit Rec Test", identifier `com.kreokit.rec.test`,
built by the "Test version" workflow for Windows, Linux and macOS) opens on a **Tests screen**
(`src-tauri/src/fieldtest.rs`, `src/components/TestScreen.tsx`) that runs these items by
clicking, on every system: a small window shows the test pattern, each test records it
through the real recorder, reads it back, deletes the recording, and keeps only the numbers;
**Save results** writes one JSON file to the Downloads folder to send back. The tests and
the items they cover:

| Test on the screen | Items | Passes when |
|---|---|---|
| Whole screen (30 s) | 2 | pattern read in every picture, fresh ≥ 99 %, check clean |
| Part of the screen (20 s) | 3, 4 | the same for a region around the pattern (a black or shifted region fails) |
| One window (20 s) | 4b | the same for the pattern window recorded as a window |
| Sound and picture together (30 s, space bar) | 5 | ≥ 10 presses matched; flash − click between −40 and +110 ms |
| Camera and sound together (25 s, claps) | 6 | ≥ 5 claps matched, no camera frames dropped, check clean (the median is noted) |
| Computer sound (15 s, the window clicks) | 9 | ≥ 8 clicks in the computer sound, check clean |

On Linux with the ScreenCast portal (Wayland, GNOME, KDE) the screen and region tests are
left out (the portal's dialog chooses whole displays or windows), and the window, sync and
sound tests ask in the system's dialog: choose the window "KreoKit Rec test pattern".
Item 1 is the computer summary at the top of the screen (in the results file), items 7 and 8
stay manual (and are covered on CI by the simulated hour and the kill -9 tests).

| # | What | How | Passes when |
|---|---|---|---|
| 1 | Permissions and devices | `--bench check` | screen, mic and camera allowed; the displays, mics and cameras listed |
| 2 | Whole display, fresh frames | `--bench fresh --seconds 60` | fresh ≥ 99 %, no unreadable frames, timestamps regular |
| 3 | **Region on the real screen** | `--bench fresh --region --seconds 30` (a region around the counter window) | the counter is read back in **every** frame (`bad` 0, `pattern_visible` 1.0) and fresh ≥ 99 %; a black picture or a crop in the wrong place fails here |
| 4 | Region in the app | Home → Region…, drag a rectangle over a window that changes, record 10 s, play it | the recording shows exactly that rectangle, not black, not shifted; the check after it is clean |
| 4b | Window | `--bench fresh --window --seconds 60` (the counter window as a window), then in the app: Home → Window, record a window 10 s while covering it with another, then close it | fresh ≥ 99 %; the recording shows only that window, also while covered; after closing, the last picture stays and the check notes when |
| 5 | Sound against picture | `--bench sync --mic` (press space about once a second) | flash − click within one frame plus the display's delay |
| 6 | Camera against sound | `--bench camera` (clap about once a second in view) | most claps within one frame; note the median (a camera delay to correct if it's large) |
| 7 | Long run | `--bench fresh --camera --mic --seconds 600` | fresh ≥ 99 %, no camera frames dropped after the start, tracks end within one frame, check clean; note the CPU |
| 8 | Crash safety | record from the app, kill it (`kill -9`), reopen, Recover | at most 1–2 s lost, the check after recovery clean |
| 9 | Computer sound (macOS 13+, Windows, Linux) | record with Computer sound on while a video plays | the sound is in the recording and in sync |

Results go into the milestone's results doc (e.g. `docs/m3-results.md`) with the date and
the system.

On macOS 13+ item 3 matters most: there the region uses ScreenCaptureKit's own
`sourceRect`, which the dev Mac (macOS 12) can't try.

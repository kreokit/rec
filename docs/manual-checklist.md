# Manual checklist for real devices

The automated tests use made-up sources: they go through the recorder, the encoder and the
check, but **not through the system's real capture**. A real capture can fail in ways they
can't see (on macOS 12 a region recorded black and every test passed). These items are run
by hand on real devices before every release, on each system: the dev Mac (macOS 12), a
macOS 13+ Mac, Windows and Linux (`CLAUDE.md`, "Release checklist"). Each item records the
real screen, camera or microphone: only with the person's OK, recordings deleted after.

The bench commands run the test build (`--features bench`); on macOS start it as the app
bundle, e.g. `open -n -W --stdout out.log --stderr out.log "KreoKit Rec Test.app" --args
--bench …`, then `rec --bench-analyze DIR` prints the result. On Windows and Linux the bench comes
with M3 step 14 (bench builds for friends).

| # | What | How | Passes when |
|---|---|---|---|
| 1 | Permissions and devices | `--bench check` | screen, mic and camera allowed; the displays, mics and cameras listed |
| 2 | Whole display, fresh frames | `--bench fresh --seconds 60` | fresh ≥ 99 %, no unreadable frames, timestamps regular |
| 3 | **Region on the real screen** | `--bench fresh --region --seconds 30` (a region around the counter window) | the counter is read back in **every** frame (`bad` 0, `pattern_visible` 1.0) and fresh ≥ 99 %; a black picture or a crop in the wrong place fails here |
| 4 | Region in the app | Home → Region…, drag a rectangle over a window that changes, record 10 s, play it | the recording shows exactly that rectangle, not black, not shifted; the check after it is clean |
| 5 | Sound against picture | `--bench sync --mic` (press space about once a second) | flash − click within one frame plus the display's delay |
| 6 | Camera against sound | `--bench camera` (clap about once a second in view) | most claps within one frame; note the median (a camera delay to correct if it's large) |
| 7 | Long run | `--bench fresh --camera --mic --seconds 600` | fresh ≥ 99 %, no camera frames dropped after the start, tracks end within one frame, check clean; note the CPU |
| 8 | Crash safety | record from the app, kill it (`kill -9`), reopen, Recover | at most 1–2 s lost, the check after recovery clean |
| 9 | Computer sound (macOS 13+, Windows, Linux) | record with Computer sound on while a video plays | the sound is in the recording and in sync |

Results go into the milestone's results doc (e.g. `docs/m3-results.md`) with the date and
the system.

On macOS 13+ item 3 matters most: there the region uses ScreenCaptureKit's own
`sourceRect`, which the dev Mac (macOS 12) can't try.

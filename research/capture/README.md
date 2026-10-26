# Capture research prototypes (milestone 1)

Small measured prototypes behind [docs/capture-research.md](../../docs/capture-research.md).
Not part of the app build.

- `macos/capbench.m`: records synthetic sources (moving picture with a frame counter, mic
  clicks once per second) through the planned recording path: VideoToolbox H.264 via
  AVAssetWriter into fragmented MP4, one file per source, all on the host clock. Prints
  CPU use, dropped frames and timing; writes `progress.txt` for crash tests.
- `macos/capreal.m` (+ `make-capreal-app.sh`): the same recording path fed by the **real**
  screen (ScreenCaptureKit or AVCaptureScreenInput), camera and mic. Only run with the
  user's permission. It shows a small window with the frame counter (driven by the host
  clock) so dropped screen frames are measurable, and `--beep` clicks through the speakers
  on every sync marker so the mic recording measures A/V drift end to end.
- `common.h`: the writer, encoder settings and CPU measurement both tools share.
- `analyze.py`: reads the frame counter and clicks back from the files: missing or
  repeated frames, timestamp errors, A/V offset and drift, frames lost after `kill -9`.

```bash
clang -fobjc-arc -O2 -mmacosx-version-min=12.0 research/capture/macos/capbench.m -o capbench \
  -framework Foundation -framework AVFoundation -framework CoreMedia \
  -framework CoreVideo -framework VideoToolbox
./capbench --out run --seconds 600 --screen 1728x1080 --camera 1280x720 --mic --keyframe 1
python3 -I research/capture/analyze.py run
```

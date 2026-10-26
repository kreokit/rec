# FFmpeg sidecar binaries

Release builds bundle `ffmpeg` and `ffprobe` as Tauri sidecars, one pair per target, named
`ffmpeg-<target-triple>[.exe]` / `ffprobe-<target-triple>[.exe]`. The sidecar list lives in
`../tauri.release.conf.json` (Tauri refuses to start, even in `tauri dev`, when a listed
sidecar is missing). Development uses the FFmpeg on your system instead (`PATH`, or
`KREOKIT_FFMPEG` / `KREOKIT_FFPROBE`; see `src/ffmpeg.rs`).

The binaries are not committed. CI puts them here with

```bash
python3 scripts/fetch_ffmpeg.py <target-triple>   # download or build, checksum-verified
python3 scripts/verify_ffmpeg.py <target-triple>  # licence + encoders + a recovery remux
```

## Licence

The same Windows and Linux builds as KreoKit Captions, Clips and Voice; on macOS, Clips'
build without its captions stack. All bundled FFmpeg builds are **LGPL**: built without
`--enable-gpl` and `--enable-nonfree`, so without libx264, libx265 or any other GPL-only
library. `scripts/verify_ffmpeg.py` fails the release if that ever changes: it checks the
build configuration, that `ffmpeg -L` shows the GNU Lesser General Public License, that
every encoder and muxer Rec uses is there (H.264: VideoToolbox / Media Foundation /
OpenH264; AAC), and that a fragmented MP4 cut off mid-write remuxes with its complete
frames unchanged, as crash recovery does.

FFmpeg runs as a separate program; KreoKit doesn't link to it. Its licence text and this
source information ship with the app (THIRD-PARTY-LICENSES.md).

## Exact sources

FFmpeg **9.0.2** on every platform.

### Windows x64 and Linux x64: BtbN FFmpeg-Builds, LGPL static variant

- Release: <https://github.com/BtbN/FFmpeg-Builds/releases/tag/autobuild-2026-09-30-13-08>
  (a month-end build; BtbN keeps these for two years)
- FFmpeg revision: `n9.0.2-17-g2a571b6068` (the 9.0.2 release plus 17 fixes from the
  `release/9.0` branch)
- Licence: **LGPL-3.0-or-later** (configured with `--enable-version3`); includes libass,
  freetype, fribidi, harfbuzz, fontconfig and libopenh264; `--disable-libx264
  --disable-libx265`
- Build scripts and full dependency list: <https://github.com/BtbN/FFmpeg-Builds>

| Target | File | SHA-256 |
|---|---|---|
| `x86_64-pc-windows-msvc` | `ffmpeg-n9.0.2-17-g2a571b6068-win64-lgpl-9.0.zip` | `6b264b9e6019103f601d98c292bd332fd87acf1c5e941ddff4fb71760fe63432` |
| `x86_64-unknown-linux-gnu` | `ffmpeg-n9.0.2-17-g2a571b6068-linux64-lgpl-9.0.tar.xz` | `2d41cbea0ca1a15029b638330740f78d6f5aeb062355bf425433fc938705350a` |

### macOS (Apple Silicon and Intel): built from source in CI

No trustworthy LGPL macOS builds exist, so `scripts/build-ffmpeg-macos.sh` builds one
from the official tarball (static, macOS 12.0 minimum, `--disable-autodetect`,
VideoToolbox and AudioToolbox; no GPL or nonfree parts, no external libraries). Licence:
**LGPL-2.1-or-later**.

| Component | Version | Source | SHA-256 | Licence |
|---|---|---|---|---|
| FFmpeg | 9.0.2 | <https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz> | `8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e` | LGPL-2.1-or-later |

## Updating

Change the version, URL and SHA-256 in `scripts/fetch_ffmpeg.py` (BtbN) or
`scripts/build-ffmpeg-macos.sh` (macOS), update this file and THIRD-PARTY-LICENSES.md, and
let CI run `verify_ffmpeg.py`.

## TODO

- [ ] The BtbN executables are large (~130 MB each, statically linked with every
      dependency). A minimal custom build with only what KreoKit uses would make the
      Windows and Linux installers much smaller.

# Third-party licences

KreoKit Rec is free software, dual-licensed under MIT or Apache-2.0. It includes or
uses the third-party components below, each under its own licence.

## AI models

None yet. "Clean up voice" (milestone 4) will compile in RNNoise v0.2 exactly as KreoKit
Voice does, listed here with its licence and training data when it lands.

## FFmpeg (bundled program)

KreoKit Rec bundles `ffmpeg` and `ffprobe` from **FFmpeg 9.0.2** as separate programs
(sidecars); the app runs them to check and recover recordings (and, from milestone 4, to
export), it does not link to them.

- Licence: **GNU Lesser General Public License** (LGPL-2.1-or-later for the macOS builds,
  LGPL-3.0-or-later for the Windows and Linux builds). No GPL or non-free components are
  enabled. Full text: `licenses/FFMPEG-LICENSE.txt` in the app, and
  <https://www.gnu.org/licenses/lgpl-3.0.html>.
- Source code: FFmpeg <https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz>. The exact builds,
  their sources and SHA-256 checksums are listed in
  [src-tauri/binaries/README.md](src-tauri/binaries/README.md):
  - Windows / Linux: BtbN FFmpeg-Builds, LGPL variant, release `autobuild-2026-09-30-13-08`
    (revision `n9.0.2-17-g2a571b6068`), <https://github.com/BtbN/FFmpeg-Builds>
  - macOS: built from source by `scripts/build-ffmpeg-macos.sh` in this repository
- You may replace the bundled FFmpeg with your own build: put it next to the app's
  executable under the same name, or point `KREOKIT_FFMPEG` / `KREOKIT_FFPROBE` at it.

The Windows and Linux builds contain further LGPL-compatible libraries enabled by BtbN's
LGPL variant (among them libopenh264, BSD-2-Clause, for H.264 export); the complete list is
in `ffmpeg -buildconf` and in BtbN's build scripts. On macOS, H.264 and AAC are encoded by
Apple's VideoToolbox and AudioToolbox (part of macOS).

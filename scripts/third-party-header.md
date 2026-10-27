# Third-party licences

KreoKit Rec is free software, dual-licensed under MIT or Apache-2.0. It includes or
uses the third-party components below, each under its own licence.

## AI models

### RNNoise v0.2 ("Clean up voice", compiled into the app, as in KreoKit Voice)

- **Code and weights:** Xiph.Org's RNNoise, © 2017 Mozilla, © 2007–2017 Jean-Marc Valin,
  © 2005–2017 Xiph.Org Foundation, © 2003–2004 Mark Borgerding, **BSD-3-Clause**
  (`src-tauri/rnnoise/COPYING`). Library sources from <https://gitlab.xiph.org/xiph/rnnoise>
  at commit `70f1d256acd4b34a572f999a05c87bf00b67730d`; model
  `rnnoise_data-0a8755f8e2d834eff6a54714ecc7d75f9932e845df35f8b59bc52a7cfe6e8b37.tar.gz`
  from <https://media.xiph.org/rnnoise/models/> (2025-01-30; mirrored unchanged in KreoKit
  Voice's `models-v1` release for the build), checked against that SHA-256.
- **Training data** (Xiph: "trained using only the publicly available datasets listed"; the
  repository's `datasets.txt` and README):
  - Speech: Google's crowd-sourced TTS corpora on OpenSLR (resources 30, 32, 37, 41–44,
    61, 63–66, 69–80, 83, 86), **CC BY-SA 4.0**; Hi-Fi Multi-Speaker English TTS (OpenSLR
    109, from LibriVox), **CC BY 4.0**.
  - Noise: DEMAND (Thiemann, Ito, Vincent 2013, <https://zenodo.org/records/1227121>),
    **CC BY 4.0**; the RNNoise noise contributions (recordings donated through Xiph's demo
    page), **CC0**; synthetic noise made by Xiph; `foreground_noise` ("e.g. keyboard
    sounds"), source not documented yet.
  - Room impulse responses: Xiph's `measured_rirs`, source not documented yet.
  - The two undocumented sets were asked about in
    <https://github.com/xiph/rnnoise/issues/287>.
- The original 2017 RNNoise weights (trained partly on the commercial NTT speech database)
  are **not** used.
- The licence ships with the app as `licenses/RNNOISE-COPYING.txt`.

## System libraries (not bundled)

- **No tray icon on Linux:** the AppIndicator library a Linux tray icon needs depends on
  `libayatana-indicator` (**GPL-3.0**), which Tauri's AppImage would bundle. The tray is
  built only for macOS (menu bar) and Windows; on Linux, where the floating control would
  be recorded, the app's window steps aside to the taskbar instead. The Linux test build
  fails if any AppIndicator library ends up in the AppImage or the .deb's dependencies.

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

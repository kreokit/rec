#!/usr/bin/env bash
# Builds an LGPL FFmpeg (ffmpeg + ffprobe) for macOS from pinned source tarballs.
#
#   scripts/build-ffmpeg-macos.sh <x86_64|arm64> <output-dir>
#
# From KreoKit Clips, without the captions stack (Rec draws no text): FFmpeg with
# --disable-autodetect and only what KreoKit Rec needs: VideoToolbox / AudioToolbox
# encoders (Finish/export), and the decoders, demuxers, muxers and filters FFmpeg enables by
# default (recovery remux, checks, composing the layout). No --enable-gpl, no
# --enable-nonfree, no libx264. Minimum macOS 12.0.
#
# Needs: Xcode command line tools, nasm (x86_64 only).
# Versions and checksums are listed in src-tauri/binaries/README.md; keep them in sync.
set -euo pipefail

ARCH="${1:?arch: x86_64 or arm64}"
OUT="$(mkdir -p "${2:?output dir}" && cd "$2" && pwd)"
MIN_MACOS="12.0"

# name version url sha256
SOURCES=(
  "ffmpeg 9.0.2 https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz 8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e"
)

WORK="$OUT/work"
rm -rf "$WORK"
mkdir -p "$WORK"

export MACOSX_DEPLOYMENT_TARGET="$MIN_MACOS"
export CFLAGS="-arch $ARCH -mmacosx-version-min=$MIN_MACOS -O2"
export CXXFLAGS="$CFLAGS"
export LDFLAGS="-arch $ARCH -mmacosx-version-min=$MIN_MACOS"

fetch() {
  local name="$1" version="$2" url="$3" sha="$4"
  local file="$WORK/$(basename "$url")"
  echo "== $name $version"
  curl -fsSL -o "$file" "$url"
  echo "$sha  $file" | shasum -a 256 -c -
  tar -xf "$file" -C "$WORK"
}

for source in "${SOURCES[@]}"; do
  # shellcheck disable=SC2086
  fetch $source
done

ASM=()
[[ "$ARCH" == "arm64" ]] || ASM=(--x86asmexe=nasm)

# -liconv: the system's libiconv, named explicitly (on the macos-15 runners of 2026-10
# configure found iconv without it and the link then failed).
(cd "$WORK/ffmpeg-9.0.2" &&
  ./configure --prefix="$OUT" --arch="$ARCH" --target-os=darwin \
    --cc="clang -arch $ARCH" --cxx="clang++ -arch $ARCH" \
    --extra-cflags="-mmacosx-version-min=$MIN_MACOS" \
    --extra-ldflags="-mmacosx-version-min=$MIN_MACOS" \
    --extra-libs="-liconv" \
    ${ASM[@]+"${ASM[@]}"} \
    --disable-debug --disable-doc --disable-ffplay --disable-network \
    --disable-autodetect \
    --enable-videotoolbox --enable-audiotoolbox \
    --enable-zlib --enable-iconv &&
  make -j"$(sysctl -n hw.ncpu)" &&
  make install)

# Licence summary plus the full LGPL 2.1 text, shipped with the app.
cat "$WORK/ffmpeg-9.0.2/LICENSE.md" "$WORK/ffmpeg-9.0.2/COPYING.LGPLv2.1" > "$OUT/LICENSE.md"

# The executables must only depend on system libraries and frameworks.
for tool in ffmpeg ffprobe; do
  if otool -L "$OUT/bin/$tool" | tail -n +2 | grep -vE "^\s+(/usr/lib/|/System/Library/)"; then
    echo "error: $tool links to non-system libraries" >&2
    exit 1
  fi
done
echo "built $OUT/bin/ffmpeg and ffprobe for $ARCH (macOS $MIN_MACOS+)"

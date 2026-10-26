#!/usr/bin/env python3
"""Checks the bundled ffmpeg/ffprobe sidecars for one target before they ship.

    python3 scripts/verify_ffmpeg.py <rust-target-triple>

From KreoKit Voice and Clips, for what Rec uses (src-tauri/src/check.rs, recovery.rs; the
Finish export arrives in milestone 4). Fails unless:
- the build is LGPL: no --enable-gpl / --enable-nonfree / libx264 / libx265 in its
  configuration, and `ffmpeg -L` prints the GNU Lesser General Public License
- every encoder and muxer Rec uses exists (ENCODERS, MUXERS)
- (macOS) the executables run on macOS 12
- a fragmented MP4 cut off mid-write (as after a crash) remuxes into a normal MP4 with
  every complete fragment's frames copied unchanged, as recovery does

The release binary's --self-test then records and recovers through the app's own code.
"""

import json
import platform
import re
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BINARIES = ROOT / "src-tauri" / "binaries"

FORBIDDEN_CONFIG = ["--enable-gpl", "--enable-nonfree", "--enable-libx264", "--enable-libx265"]
# The H.264 encoders of the Finish export (Clips' order per system) and AAC.
ENCODERS = {
    "darwin": ["h264_videotoolbox", "aac", "aac_at"],
    "windows": ["h264_mf", "libopenh264", "aac"],
    "linux": ["libopenh264", "aac"],
}
MUXERS = ["mp4", "mov"]
FILTERS: list[str] = []


def run(*args: str, cwd: Path | None = None) -> str:
    result = subprocess.run(args, capture_output=True, text=True, cwd=cwd)
    if result.returncode != 0:
        raise RuntimeError(f"{' '.join(args[:3])}… failed:\n{result.stderr[-2000:]}")
    return result.stdout + result.stderr


def fail(message: str) -> None:
    sys.exit(f"FAIL: {message}")


def listed(output: str, name: str, kind: str) -> bool:
    flags = {"encoder": r"[AV]\S*", "muxer": r"\s*D?E", "filter": r"\S+"}[kind]
    return re.search(rf"^\s*{flags}\s+{re.escape(name)}\s", output, re.M) is not None


def main() -> None:
    # Windows runners print in cp1252, which has no "→".
    sys.stdout.reconfigure(encoding="utf-8")
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    triple = sys.argv[1]
    exe = ".exe" if "windows" in triple else ""
    os_name = "darwin" if "apple" in triple else "windows" if "windows" in triple else "linux"
    ffmpeg = BINARIES / f"ffmpeg-{triple}{exe}"
    ffprobe = BINARIES / f"ffprobe-{triple}{exe}"
    for tool in (ffmpeg, ffprobe):
        if not tool.is_file():
            fail(f"missing {tool.relative_to(ROOT)}; run scripts/fetch_ffmpeg.py {triple}")

    version = run(str(ffmpeg), "-hide_banner", "-version").splitlines()[0]
    config = run(str(ffmpeg), "-hide_banner", "-buildconf")
    bad = [flag for flag in FORBIDDEN_CONFIG if flag in config]
    if bad:
        fail(f"not an LGPL build: {', '.join(bad)}")
    licence = " ".join(run(str(ffmpeg), "-hide_banner", "-L").split())
    if "GNU Lesser General Public License" not in licence:
        fail("ffmpeg -L does not show the LGPL")
    run(str(ffprobe), "-hide_banner", "-version")

    encoders = run(str(ffmpeg), "-hide_banner", "-encoders")
    missing = [e for e in ENCODERS[os_name] if not listed(encoders, e, "encoder")]
    muxers = run(str(ffmpeg), "-hide_banner", "-muxers")
    missing += [f"muxer {m}" for m in MUXERS if not listed(muxers, m, "muxer")]
    filters = run(str(ffmpeg), "-hide_banner", "-filters")
    missing += [f"filter {f}" for f in FILTERS if not listed(filters, f, "filter")]
    if missing:
        fail(f"missing: {', '.join(missing)}")

    if os_name == "darwin" and platform.system() == "Darwin":
        for tool in (ffmpeg, ffprobe):
            minos = re.search(r"minos (\S+)", run("otool", "-l", str(tool)))
            if not minos or tuple(map(int, minos.group(1).split("."))) > (12, 0):
                fail(f"{tool.name} needs macOS {minos.group(1) if minos else '?'}, not 12.0")

    results = export_test(ffmpeg.resolve(), ffprobe.resolve(), os_name)
    print(f"OK {triple}: {version}")
    print(f"   LGPL, encoders {ENCODERS[os_name]}, {len(MUXERS)} muxers")
    for line in results:
        print(f"   {line}")


def framemd5(ffmpeg: Path, path: Path) -> list[str]:
    out = run(str(ffmpeg), "-nostdin", "-v", "error", "-i", str(path), "-map", "0:v:0",
              "-c", "copy", "-f", "framemd5", "-")
    return [line.rsplit(",", 1)[-1].strip() for line in out.splitlines() if not line.startswith("#")]


def export_test(ffmpeg: Path, ffprobe: Path, os_name: str) -> list[str]:
    """A 10 s fragmented MP4 (1 s fragments, like the recorder's), cut off in the middle of
    its last fragment, then remuxed as recovery.rs does: stream copy, faststart, and the
    half-written last frame dropped (+discardcorrupt)."""
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        source = work / "screen.mp4"
        run(str(ffmpeg), "-nostdin", "-y", "-v", "error",
            "-f", "lavfi", "-i", "testsrc=size=320x240:rate=30:duration=10",
            "-c:v", "mpeg4", "-g", "30", "-movflags", "frag_keyframe+empty_moov+default_base_moof",
            str(source))
        whole = framemd5(ffmpeg, source)
        data = source.read_bytes()
        cut = work / "cut.mp4"
        cut.write_bytes(data[: int(len(data) * 0.95)])
        out = work / "recovered.mp4"
        run(str(ffmpeg), "-nostdin", "-y", "-v", "error", "-fflags", "+discardcorrupt", "-i", str(cut),
            "-map", "0", "-c", "copy", "-movflags", "+faststart", str(out))
        kept = framemd5(ffmpeg, out)
        if len(whole) != 300 or not kept or kept != whole[: len(kept)]:
            fail(f"recovery remux: {len(kept)} of {len(whole)} frames, or the picture changed")
        if len(whole) - len(kept) > 60:
            fail(f"recovery remux lost {len(whole) - len(kept)} frames (more than 2 s)")
        duration = float(run(str(ffprobe), "-v", "error", "-show_entries", "format=duration",
                             "-of", "csv=p=0", str(out)).strip())
        return [f"recovery remux: {len(kept)} of {len(whole)} frames kept unchanged, {duration:.2f} s"]


if __name__ == "__main__":
    main()

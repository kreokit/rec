#!/usr/bin/env python3
"""Downloads the LGPL FFmpeg build KreoKit Clips ships for Windows/Linux (same BtbN pin and
SHA-256 as kreokit/clips scripts/fetch_ffmpeg.py), so the CI research measures the FFmpeg
capture devices exactly as a release would contain them.

    python3 research/capture/fetch_ffmpeg_lgpl.py win64|linux64 OUT_DIR
"""

import hashlib
import io
import sys
import tarfile
import urllib.request
import zipfile
from pathlib import Path

TAG = "autobuild-2026-09-30-13-08"
NAME = "ffmpeg-n9.0.2-17-g2a571b6068-{platform}-lgpl-9.0"
BUILDS = {
    "win64": (".zip", "6b264b9e6019103f601d98c292bd332fd87acf1c5e941ddff4fb71760fe63432"),
    "linux64": (".tar.xz", "2d41cbea0ca1a15029b638330740f78d6f5aeb062355bf425433fc938705350a"),
}


def main() -> None:
    platform, out = sys.argv[1], Path(sys.argv[2])
    ext, sha256 = BUILDS[platform]
    folder = NAME.format(platform=platform)
    url = f"https://github.com/BtbN/FFmpeg-Builds/releases/download/{TAG}/{folder}{ext}"
    print(f"downloading {url}", flush=True)
    data = urllib.request.urlopen(url).read()
    digest = hashlib.sha256(data).hexdigest()
    if digest != sha256:
        sys.exit(f"checksum mismatch: expected {sha256}, got {digest}")
    out.mkdir(parents=True, exist_ok=True)
    if ext == ".zip":
        with zipfile.ZipFile(io.BytesIO(data)) as z:
            for info in z.infolist():
                if info.filename.startswith(f"{folder}/bin/") and not info.is_dir():
                    (out / Path(info.filename).name).write_bytes(z.read(info))
    else:
        with tarfile.open(fileobj=io.BytesIO(data), mode="r:xz") as t:
            for member in t.getmembers():
                if member.name.startswith(f"{folder}/bin/") and member.isfile():
                    target = out / Path(member.name).name
                    target.write_bytes(t.extractfile(member).read())
                    target.chmod(0o755)
    print("extracted:", sorted(p.name for p in out.iterdir()))


if __name__ == "__main__":
    main()

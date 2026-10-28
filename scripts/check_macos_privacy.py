#!/usr/bin/env python3
"""Checks that a macOS .app may use the microphone and camera without being ended by macOS.

    python3 scripts/check_macos_privacy.py <app> [...]

Fails unless, for every .app given:
- Info.plist has a non-empty NSMicrophoneUsageDescription and NSCameraUsageDescription
  (macOS ends an app that asks without them);
- if the main executable is signed with the hardened runtime, its entitlements grant
  com.apple.security.device.audio-input and com.apple.security.device.camera (without
  them macOS ends the app when it uses the mic or camera: the v0.1.0-alpha draft crashed
  on "Allow" for the microphone, 2026-10-28).
"""

import plistlib
import subprocess
import sys
from pathlib import Path

USAGE_KEYS = ["NSMicrophoneUsageDescription", "NSCameraUsageDescription"]
ENTITLEMENTS = [
    "com.apple.security.device.audio-input",
    "com.apple.security.device.camera",
]


def check(app: Path) -> list[str]:
    problems = []
    with open(app / "Contents" / "Info.plist", "rb") as f:
        info = plistlib.load(f)
    for key in USAGE_KEYS:
        text = str(info.get(key, "")).strip()
        if text:
            print(f"ok  {key}: {text}")
        else:
            problems.append(f"Info.plist has no {key}")

    exe = app / "Contents" / "MacOS" / info["CFBundleExecutable"]
    details = subprocess.run(
        ["codesign", "-dv", str(exe)], capture_output=True, text=True
    ).stderr
    flags = next((l for l in details.splitlines() if "flags=" in l), "")
    runtime = "runtime" in flags
    print(f"signature: {flags.strip() or 'none'}")
    out = subprocess.run(
        ["codesign", "-d", "--entitlements", "-", "--xml", str(exe)],
        capture_output=True,
    ).stdout
    # codesign on macOS 12 ends the XML with a NUL byte.
    xml = out.strip(b"\0 \n")
    granted = plistlib.loads(xml) if xml else {}
    for key in ENTITLEMENTS:
        if granted.get(key) is True:
            print(f"ok  {key}")
        elif runtime:
            problems.append(f"hardened runtime without the {key} entitlement")
    return problems


def main() -> int:
    apps = [Path(a) for a in sys.argv[1:]]
    if not apps:
        print(__doc__)
        return 2
    failed = False
    for app in apps:
        print(f"== {app}")
        for problem in check(app):
            print(f"FAIL {problem}")
            failed = True
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())

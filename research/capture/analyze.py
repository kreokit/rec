#!/usr/bin/env python3
"""Checks a capbench recording (docs/capture-research.md): reads the frame counter drawn
into every synthetic video frame, finds the clicks in the synthetic mic track and reports
dropped or repeated frames, timestamp regularity and A/V sync over time.

    python3 -I research/capture/analyze.py DIR [--fps 30]

Needs ffmpeg and ffprobe on PATH. Standard library only.
"""

import array
import json
import subprocess
import sys
from pathlib import Path

CELL = 16  # counter square size, as in capbench.m
BITS = 32


def run(args: list[str]) -> bytes:
    return subprocess.run(args, capture_output=True, check=True).stdout


def probe(path: Path) -> dict:
    return json.loads(run(["ffprobe", "-v", "error", "-show_streams", "-show_format", "-of", "json", str(path)]))


def video_frames(path: Path, height: int) -> list[tuple[float, int, bool]]:
    """(pts seconds, decoded counter, marker on) for every frame in decode order."""
    pts = [float(line.split(",")[0]) for line in run([
        "ffprobe", "-v", "error", "-select_streams", "v:0", "-show_entries", "frame=pts_time",
        "-of", "csv=p=0", str(path)]).decode().split() if line.strip()]
    width = CELL * 34
    # One pixel row through the counter squares' centres, plus one through the marker strip.
    rows = run([
        "ffmpeg", "-v", "error", "-i", str(path), "-fps_mode", "passthrough",
        "-vf", f"format=gray,crop={width}:1:0:{CELL}", "-f", "rawvideo", "-"])
    marks = run([
        "ffmpeg", "-v", "error", "-i", str(path), "-fps_mode", "passthrough",
        "-vf", f"format=gray,crop={width}:1:0:{height - CELL}", "-f", "rawvideo", "-"])
    frames = []
    for i in range(len(rows) // width):
        row = rows[i * width:(i + 1) * width]
        n = 0
        for b in range(BITS):
            x = CELL // 2 + b * CELL + CELL // 2
            n = (n << 1) | (1 if row[x] > 128 else 0)
        mark = marks[i * width:(i + 1) * width]
        frames.append((pts[i] if i < len(pts) else float("nan"), n, sum(mark) / width > 128))
    return frames


def clicks(path: Path, rate: int = 48000) -> list[float]:
    """Start times (s) of the clicks: the first sample above 0.4 after 0.1 s of quiet."""
    pcm = run(["ffmpeg", "-v", "error", "-i", str(path), "-ac", "1", "-ar", str(rate), "-f", "s16le", "-"])
    out, quiet_until = [], -1
    samples = array.array("h")
    samples.frombytes(pcm[: len(pcm) // 2 * 2])
    threshold = int(0.4 * 32767)
    for i, s in enumerate(samples):
        if abs(s) > threshold:
            if i > quiet_until:
                out.append(i / rate)
            quiet_until = i + rate // 10
    return out


def analyze_video(path: Path, fps: int) -> dict:
    info = probe(path)
    stream = next(s for s in info["streams"] if s["codec_type"] == "video")
    frames = video_frames(path, int(stream["height"]))
    counters = [n for _, n, _ in frames]
    gaps, repeats = 0, 0
    missing_runs = []
    for a, b in zip(counters, counters[1:]):
        if b == a:
            repeats += 1
        elif b > a + 1:
            gaps += b - a - 1
            missing_runs.append((a + 1, b - 1))
    # Each frame's timestamp should be (counter / fps) after the first frame's.
    first_pts, first_n = frames[0][0], frames[0][1]
    timing_err = [abs((p - first_pts) - (n - first_n) / fps) for p, n, _ in frames]
    return {
        "file": path.name,
        "codec": stream["codec_name"],
        "profile": stream.get("profile"),
        "size": f'{stream["width"]}x{stream["height"]}',
        "pix_fmt": stream.get("pix_fmt"),
        "color": [stream.get("color_space"), stream.get("color_primaries"), stream.get("color_transfer")],
        "duration": float(info["format"]["duration"]),
        "frames": len(frames),
        "first_counter": counters[0],
        "last_counter": counters[-1],
        "missing_frames": gaps,
        "missing_runs": missing_runs[:10],
        "repeated_frames": repeats,
        "pts_vs_counter_max_error_ms": round(1000 * max(timing_err), 3),
        "markers": [(p, n) for p, n, m in frames if m],
    }


def main() -> None:
    args = sys.argv[1:]
    fps = 30
    if "--fps" in args:
        fps = int(args[args.index("--fps") + 1])
    folder = Path(args[0])
    report: dict = {"videos": []}
    for name in ("screen", "camera"):
        for ext in ("mp4", "mov"):
            path = folder / f"{name}.{ext}"
            if path.exists():
                report["videos"].append(analyze_video(path, fps))
    mic = next((folder / f"mic.{e}" for e in ("mp4", "mov") if (folder / f"mic.{e}").exists()), None)
    if mic:
        info = probe(mic)
        times = clicks(mic)
        report["mic"] = {"duration": float(info["format"]["duration"]), "clicks": len(times)}
        # Sync: each marker frame (every whole second) against the nearest click. A
        # constant offset is fixable latency; a changing one is drift.
        for video in report["videos"]:
            offsets = []
            for pts, _ in video["markers"]:
                if times:
                    nearest = min(times, key=lambda t: abs(t - pts))
                    offsets.append(nearest - pts)
            if offsets:
                video["sync_offset_ms_first"] = round(1000 * offsets[0], 2)
                video["sync_offset_ms_last"] = round(1000 * offsets[-1], 2)
                video["sync_offset_ms_min_max"] = [round(1000 * min(offsets), 2), round(1000 * max(offsets), 2)]
                video["sync_drift_ms"] = round(1000 * (offsets[-1] - offsets[0]), 2)
    # After kill -9: capbench's progress file holds the newest frame it had handed over.
    progress = folder / "progress.txt"
    if progress.exists() and progress.read_text().strip() and report["videos"]:
        newest = int(progress.read_text().split()[0])
        kept = report["videos"][0]["last_counter"]
        report["crash"] = {"newest_frame": newest, "lost_frames": newest - kept,
                           "lost_seconds": round((newest - kept) / fps, 2)}
    for video in report["videos"]:
        video["markers"] = len(video["markers"])
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()

"""Writes src/lib/layout-cases.json: layout cases worked out independently of layout.rs and
layout.ts (both are tested against this file). Run: python3 scripts/layout_cases.py"""
import json
from decimal import Decimal, ROUND_HALF_UP

SHARE = {"small": 0.18, "medium": 0.25, "large": 0.33}
MARGIN, RADIUS = 0.03, 0.16


def rnd(v):  # round half away from zero, as Rust's f64::round and the TS twin
    return int(Decimal(repr(v)).quantize(Decimal(1), rounding=ROUND_HALF_UP))


def even(v):
    return max(2, rnd(v / 2) * 2)


def crop(w, h, aspect):
    if w / h > aspect:
        cw, ch = min(even(h * aspect), w), h
    else:
        cw, ch = w, min(even(w / aspect), h)
    return {"x": (w - cw) // 2 // 2 * 2, "y": (h - ch) // 2 // 2 * 2, "width": cw, "height": ch}


def place(layout, b, screen, camera):
    if layout == "bubble" and screen and camera:
        w, h = screen
        short = min(w, h)
        bh = even(short * SHARE[b["size"]])
        if b["shape"] == "circle":
            bw, aspect, radius = bh, 1.0, bh // 2
        else:
            bw, aspect, radius = even(bh * 4 / 3), 4 / 3, rnd(bh * RADIUS)
        m = rnd(short * MARGIN)
        rx, ry = max(0, w - 2 * m - bw), max(0, h - 2 * m - bh)
        x = rnd((m + b["x"] * rx) / 2) * 2
        y = rnd((m + b["y"] * ry) / 2) * 2
        return {"width": w, "height": h, "camera_crop": crop(*camera, aspect),
                "camera": {"x": min(x, w - bw), "y": min(y, h - bh), "width": bw, "height": bh}, "radius": radius}
    if layout == "camera" and camera:
        cw, ch = camera
        full = {"x": 0, "y": 0, "width": cw, "height": ch}
        return {"width": cw, "height": ch, "camera_crop": full, "camera": dict(full), "radius": 0}
    if screen:
        return {"width": screen[0], "height": screen[1], "camera_crop": None, "camera": None, "radius": 0}
    return {"width": 0, "height": 0, "camera_crop": None, "camera": None, "radius": 0}


def bubble(shape="circle", size="medium", x=1.0, y=1.0):
    return {"shape": shape, "size": size, "x": x, "y": y}


CASES = [
    ("medium circle, bottom right, 1728x1080", "bubble", bubble(), [1728, 1080], [1280, 720]),
    ("small circle, top left", "bubble", bubble(size="small", x=0.0, y=0.0), [1728, 1080], [1280, 720]),
    ("large rounded, dragged", "bubble", bubble("rounded", "large", 0.37, 0.61), [1920, 1080], [1280, 720]),
    ("rounded on a 4:3 camera", "bubble", bubble("rounded", "medium", 0.0, 1.0), [1152, 720], [640, 480]),
    ("tall region", "bubble", bubble(size="medium", x=0.5, y=0.5), [606, 1080], [1280, 720]),
    ("odd window size", "bubble", bubble("rounded", "small", 1.0, 0.0), [1366, 768], [1280, 720]),
    ("camera only", "camera", bubble(), [1728, 1080], [1280, 720]),
    ("screen only", "screen", bubble(), [1728, 1080], [1280, 720]),
    ("bubble without a camera falls back to the screen", "bubble", bubble(), [1728, 1080], None),
]

out = [{"name": n, "layout": l, "bubble": b, "screen": s, "camera": c, "expected": place(l, b, s, c)}
       for n, l, b, s, c in CASES]
with open("src/lib/layout-cases.json", "w") as f:
    json.dump(out, f, indent=2)
    f.write("\n")
print(json.dumps(out[0]["expected"]))

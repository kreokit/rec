import type { Region } from "./api";

/** The smallest region, in points each way (MIN_REGION in sessions.rs). */
export const MIN_REGION = 64;

/** What a drag changes: a new rectangle, the whole one, or one corner. */
export type Grip = "new" | "move" | "nw" | "ne" | "sw" | "se";

const clamp = (v: number, lo: number, hi: number) => Math.min(Math.max(v, lo), hi);

/**
 * The region after dragging `grip` from `from` to `to` (points on the display), starting
 * from `start` (the region when the drag began; for "new", ignored). Stays on the display
 * (`width` × `height` points), whole points.
 */
export function dragRegion(
  grip: Grip,
  start: Region | null,
  from: { x: number; y: number },
  to: { x: number; y: number },
  width: number,
  height: number,
): Region {
  const px = clamp(to.x, 0, width);
  const py = clamp(to.y, 0, height);
  if (grip === "new" || !start) {
    const fx = clamp(from.x, 0, width);
    const fy = clamp(from.y, 0, height);
    return round({ x: Math.min(fx, px), y: Math.min(fy, py), width: Math.abs(px - fx), height: Math.abs(py - fy) });
  }
  if (grip === "move") {
    const x = clamp(start.x + to.x - from.x, 0, width - start.width);
    const y = clamp(start.y + to.y - from.y, 0, height - start.height);
    return round({ ...start, x, y });
  }
  // A corner: the opposite corner stays where it is.
  const left = start.x;
  const top = start.y;
  const right = start.x + start.width;
  const bottom = start.y + start.height;
  const ax = grip === "nw" || grip === "sw" ? right : left;
  const ay = grip === "nw" || grip === "ne" ? bottom : top;
  return round({ x: Math.min(ax, px), y: Math.min(ay, py), width: Math.abs(px - ax), height: Math.abs(py - ay) });
}

function round(r: Region): Region {
  return { x: Math.round(r.x), y: Math.round(r.y), width: Math.round(r.width), height: Math.round(r.height) };
}

export function bigEnough(r: Region | null): r is Region {
  return !!r && r.width >= MIN_REGION && r.height >= MIN_REGION;
}

/** The region in pixels, as the recorder captures it (source_size in sessions.rs). */
export function regionPixels(r: Region, displayWidth: number, pointsWidth: number): { width: number; height: number } {
  const scale = pointsWidth > 0 ? displayWidth / pointsWidth : 1;
  return { width: Math.round(r.width * scale), height: Math.round(r.height * scale) };
}

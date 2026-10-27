/**
 * Where everything goes in the exported picture: the twin of src-tauri/src/layout.rs, used
 * to draw the preview. Both are tested against layout-cases.json, so the preview and the
 * export can't disagree.
 */
import type { Bubble, BubbleSize, Layout } from "./api";

export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface Placement {
  width: number;
  height: number;
  /** The part of the camera picture that is shown. */
  camera_crop: Rect | null;
  /** Where the camera goes in the exported picture. */
  camera: Rect | null;
  radius: number;
}

export const SIZE_SHARE: Record<BubbleSize, number> = { small: 0.18, medium: 0.25, large: 0.33 };
export const MARGIN_SHARE = 0.03;
export const ROUNDED_RADIUS_SHARE = 0.16;

/** The nearest even number (all values here are positive, so Math.round matches Rust). */
const even = (v: number) => Math.max(2, Math.round(v / 2) * 2);
const clamp01 = (v: number) => Math.min(Math.max(v, 0), 1);

function centreCrop(width: number, height: number, aspect: number): Rect {
  const [w, h] =
    width / height > aspect ? [Math.min(even(height * aspect), width), height] : [width, Math.min(even(width / aspect), height)];
  return { x: Math.floor((width - w) / 4) * 2, y: Math.floor((height - h) / 4) * 2, width: w, height: h };
}

/** The bubble's geometry without its position: size, margin and the room to move in. */
export function bubbleBox(bubble: Bubble, width: number, height: number) {
  const short = Math.min(width, height);
  const bh = even(short * SIZE_SHARE[bubble.size]);
  const bw = bubble.shape === "circle" ? bh : even((bh * 4) / 3);
  const margin = Math.round(short * MARGIN_SHARE);
  return {
    width: bw,
    height: bh,
    margin,
    roomX: Math.max(0, width - 2 * margin - bw),
    roomY: Math.max(0, height - 2 * margin - bh),
  };
}

export function place(
  layout: Layout,
  bubble: Bubble,
  screen: [number, number] | null,
  camera: [number, number] | null,
): Placement {
  if (layout === "bubble" && screen && camera) {
    const [w, h] = screen;
    const box = bubbleBox(bubble, w, h);
    const circle = bubble.shape === "circle";
    const radius = circle ? box.height / 2 : Math.round(box.height * ROUNDED_RADIUS_SHARE);
    const x = Math.round((box.margin + clamp01(bubble.x) * box.roomX) / 2) * 2;
    const y = Math.round((box.margin + clamp01(bubble.y) * box.roomY) / 2) * 2;
    return {
      width: w,
      height: h,
      camera_crop: centreCrop(camera[0], camera[1], circle ? 1 : 4 / 3),
      camera: { x: Math.min(x, w - box.width), y: Math.min(y, h - box.height), width: box.width, height: box.height },
      radius,
    };
  }
  if (layout === "camera" && camera) {
    const full = { x: 0, y: 0, width: camera[0], height: camera[1] };
    return { width: camera[0], height: camera[1], camera_crop: full, camera: { ...full }, radius: 0 };
  }
  if (screen) return { width: screen[0], height: screen[1], camera_crop: null, camera: null, radius: 0 };
  return { width: 0, height: 0, camera_crop: null, camera: null, radius: 0 };
}

/**
 * The bubble position (fractions of the room) that puts its top-left corner at (`left`,
 * `top`) in picture pixels: for dragging.
 */
export function positionAt(bubble: Bubble, width: number, height: number, left: number, top: number) {
  const box = bubbleBox(bubble, width, height);
  return {
    x: box.roomX > 0 ? clamp01((left - box.margin) / box.roomX) : 0,
    y: box.roomY > 0 ? clamp01((top - box.margin) / box.roomY) : 0,
  };
}

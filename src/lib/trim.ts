/**
 * Trim maths for Finish (src-tauri/src/finish.rs fits the saved trim the same way): the
 * start and end snap to whole frames and keep at least one frame between them.
 */

export interface Trim {
  start: number;
  /** Seconds; the recording's length when not trimmed. */
  end: number;
}

/** The nearest whole frame. */
export function snap(seconds: number, fps: number): number {
  return Math.round(seconds * fps) / fps;
}

/** Moves the start to `seconds` (snapped), inside [0, end − 1 frame]. */
export function withStart(trim: Trim, seconds: number, fps: number): Trim {
  const start = Math.min(Math.max(snap(seconds, fps), 0), trim.end - 1 / fps);
  return { ...trim, start: Math.max(0, start) };
}

/** Moves the end to `seconds` (snapped), inside [start + 1 frame, duration]. */
export function withEnd(trim: Trim, seconds: number, fps: number, duration: number): Trim {
  const end = Math.max(Math.min(snap(seconds, fps), duration), trim.start + 1 / fps);
  return { ...trim, end: Math.min(end, duration) };
}

/** The end as finish.json keeps it: null when it's the recording's end. */
export function savedEnd(trim: Trim, duration: number, fps: number): number | null {
  return trim.end >= duration - 0.5 / fps ? null : trim.end;
}

/** "0:04.2", "12:05.0", "1:02:03.4": tenths, for the trim times. */
export function formatTime(seconds: number): string {
  const tenths = Math.max(0, Math.round(seconds * 10));
  const h = Math.floor(tenths / 36_000);
  const m = Math.floor((tenths % 36_000) / 600);
  const s = ((tenths % 600) / 10).toFixed(1).padStart(4, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${s}` : `${m}:${s}`;
}

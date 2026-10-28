/** Times, sizes and levels as the screens show them. */
import type { Session } from "./api";

/** "0:04", "12:05", "1:02:03" (from KreoKit Voice). */
export function formatDuration(seconds: number): string {
  const total = Math.max(0, Math.floor(seconds));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = String(total % 60).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${s}` : `${m}:${s}`;
}

/** "820 MB", "1.4 GB", "21 GB". */
export function formatBytes(bytes: number): string {
  if (bytes < 1e9) return `${Math.max(1, Math.round(bytes / 1e6))} MB`;
  const gb = bytes / 1e9;
  return gb < 10 ? `${gb.toFixed(1)} GB` : `${Math.round(gb)} GB`;
}

/** "about 6 h", "about 45 min": how long the free space lasts. */
export function formatRoom(freeBytes: number, bytesPerMinute: number): string {
  const minutes = Math.floor(freeBytes / bytesPerMinute);
  if (minutes < 60) return `about ${minutes} min`;
  const hours = Math.floor(minutes / 60);
  return hours < 10 ? `about ${hours} h ${minutes % 60} min` : `about ${hours} h`;
}

/** Under this many minutes of disk space left: a calm note; under DISK_ALERT_MINUTES: red. */
export const DISK_NOTE_MINUTES = 10;
export const DISK_ALERT_MINUTES = 2;
/** Home warns before recording when the space holds less than this. */
export const DISK_HOME_MINUTES = 30;

/** Space a recording stops at (sessions.rs STOP_BELOW_FREE). */
export const STOP_BELOW_FREE = 500_000_000;

/** Minutes of recording the free space holds before the disk-space stop. */
export function roomMinutes(freeBytes: number, bytesPerMinute: number): number {
  return Math.max(0, freeBytes - STOP_BELOW_FREE) / Math.max(1, bytesPerMinute);
}

/** The disk note while recording, or null with enough space. */
export function diskNote(minutesLeft: number | null | undefined): { text: string; alert: boolean } | null {
  if (minutesLeft == null || minutesLeft > DISK_NOTE_MINUTES) return null;
  const minutes = Math.max(0, Math.floor(minutesLeft));
  return {
    text:
      minutes < 1
        ? "Less than a minute of disk space left. The recording stops by itself before the disk is full."
        : `About ${minutes} min of disk space left. The recording stops by itself before the disk is full.`,
    alert: minutesLeft <= DISK_ALERT_MINUTES,
  };
}

/** "14:32" (local time). */
export function clockTime(ms: number): string {
  const d = new Date(ms);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

/** "Recording 27 Oct, 14:32". */
export function recordingTitle(session: Session): string {
  const d = new Date(session.created_at);
  return `Recording ${d.toLocaleDateString("en-GB", { day: "numeric", month: "short" })}, ${clockTime(session.created_at)}`;
}

/** A microphone peak (0–1) on the meter's scale: −60 dBFS to 0, as 0–1. */
export function meterLevel(peak: number): number {
  if (peak <= 0) return 0;
  return Math.min(1, Math.max(0, (20 * Math.log10(peak) + 60) / 60));
}

/** The check's result in one line for the Recent list and the "saved" message. */
export function checkSummary(session: Session): { text: string; ok: boolean } {
  if (session.state === "failed") return { text: session.error ?? "Nothing was recorded.", ok: false };
  const problems = session.check?.problems ?? [];
  if (problems.length) return { text: problems[0], ok: false };
  const both = session.check?.screen && session.check?.mic;
  return { text: both ? "Picture and sound checked: in sync" : "Checked: plays correctly", ok: true };
}

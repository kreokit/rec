/** Export texts and progress for the Finish screen (src-tauri/src/export.rs). */
import type { ExportDone, ExportProgress } from "./api";
import { formatDuration } from "./format";

/** Share of the whole export each stage takes (the picture is the long part). */
const STAGES: Record<ExportProgress["stage"], [number, number]> = {
  sound: [0, 0.3],
  picture: [0.3, 0.95],
  check: [0.95, 1],
};

/** The whole export's progress (0–1) from a stage's. */
export function exportFraction(p: ExportProgress): number {
  const [from, to] = STAGES[p.stage];
  return from + (to - from) * Math.min(Math.max(p.fraction, 0), 1);
}

export function exportStageLabel(stage: ExportProgress["stage"]): string {
  return { sound: "Preparing the sound…", picture: "Writing the video…", check: "Checking the video…" }[stage];
}

/** "Movies" for "/Users/me/Movies". */
export function folderName(path: string): string {
  return path.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || path;
}

/** "1728 × 1080 · 0:12 · −16.0 LUFS · checked" */
export function summary(done: ExportDone): string {
  const parts = [`${done.facts.width} × ${done.facts.height}`, formatDuration(Math.round(done.facts.video_s))];
  if (done.lufs !== null) parts.push(`${done.lufs.toFixed(1).replace("-", "−")} LUFS`);
  parts.push("checked");
  return parts.join(" · ");
}

import { describe, expect, it } from "vitest";
import type { ExportDone } from "./api";
import { exportFraction, folderName, summary } from "./exporting";

describe("exporting", () => {
  it("adds up the stages", () => {
    expect(exportFraction({ id: "a", stage: "sound", fraction: 0 })).toBe(0);
    expect(exportFraction({ id: "a", stage: "sound", fraction: 1 })).toBeCloseTo(0.3);
    expect(exportFraction({ id: "a", stage: "picture", fraction: 0.5 })).toBeCloseTo(0.625);
    expect(exportFraction({ id: "a", stage: "check", fraction: 1 })).toBe(1);
  });

  it("names the folder", () => {
    expect(folderName("/Users/me/Movies")).toBe("Movies");
    expect(folderName("C:\\Users\\me\\Videos\\")).toBe("Videos");
  });

  it("sums up the result", () => {
    const done = {
      path: "/Users/me/Movies/Recording-2026-10-27-1432.mp4",
      facts: { width: 1728, height: 1080, video_s: 12.03 },
      encoder: "h264_videotoolbox",
      lufs: -16.04,
      true_peak_db: -1.3,
      held_by_peaks: false,
    } as ExportDone;
    expect(summary(done)).toBe("1728 × 1080 · 0:12 · −16.0 LUFS · checked");
    expect(summary({ ...done, lufs: null })).toBe("1728 × 1080 · 0:12 · checked");
  });
});

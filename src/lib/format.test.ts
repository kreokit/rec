import { describe, expect, it } from "vitest";
import type { Session } from "./api";
import { checkSummary, diskNote, formatBytes, formatDuration, formatRoom, meterLevel, roomMinutes } from "./format";

const session = (over: Partial<Session> = {}): Session => ({
  id: "2026-10-27-143200",
  created_at: 0,
  state: "done",
  screen: { display: "Built-in display", region: null, width: 1728, height: 1080, fps: 30 },
  mic: "MacBook Pro Microphone",
  camera: null,
  elapsed_s: 725,
  pauses: 0,
  check: {
    duration_s: 725,
    screen: { codec: "h264", start_s: 0, duration_s: 725, packets: 21750 },
    camera: null,
    mic: { codec: "aac", start_s: 0, duration_s: 725.01, packets: 33985 },
    start_offset_ms: 0,
    end_offset_ms: 10,
    camera_start_offset_ms: null,
    camera_end_offset_ms: null,
    recorder: null,
    problems: [],
    notes: [],
  },
  error: null,
  hidden: false,
  ...over,
});

describe("format", () => {
  it("formats durations", () => {
    expect(formatDuration(4.9)).toBe("0:04");
    expect(formatDuration(725)).toBe("12:05");
    expect(formatDuration(3723)).toBe("1:02:03");
  });

  it("formats sizes and how long the space lasts", () => {
    expect(formatBytes(820e6)).toBe("820 MB");
    expect(formatBytes(1.44e9)).toBe("1.4 GB");
    expect(formatBytes(21.2e9)).toBe("21 GB");
    expect(formatRoom(21e9, 55e6)).toBe("about 6 h 21 min");
    expect(formatRoom(2e9, 55e6)).toBe("about 36 min");
    expect(formatRoom(200e9, 55e6)).toBe("about 60 h");
  });

  it("maps the microphone peak to the meter", () => {
    expect(meterLevel(0)).toBe(0);
    expect(meterLevel(1)).toBe(1);
    expect(meterLevel(0.001)).toBeCloseTo(0);
    expect(meterLevel(0.1)).toBeCloseTo(2 / 3);
  });

  it("summarises the check", () => {
    expect(checkSummary(session())).toEqual({ text: "Picture and sound checked: in sync", ok: true });
    const problem = "Picture and sound don't line up: the sound starts +0 ms and ends +90 ms from the picture.";
    expect(checkSummary(session({ check: { ...session().check!, problems: [problem] } }))).toEqual({
      text: problem,
      ok: false,
    });
    expect(checkSummary(session({ state: "failed", error: "Nothing could be recovered." })).ok).toBe(false);
  });
});

describe("disk space", () => {
  it("counts the room before the disk-space stop", () => {
    expect(roomMinutes(500_000_000 + 30 * 56e6, 56e6)).toBeCloseTo(30);
    expect(roomMinutes(100, 56e6)).toBe(0);
  });
  it("notes 10 minutes and less, alerts at 2", () => {
    expect(diskNote(null)).toBeNull();
    expect(diskNote(10.5)).toBeNull();
    expect(diskNote(9.7)).toEqual({ text: expect.stringContaining("About 9 min"), alert: false });
    expect(diskNote(2)?.alert).toBe(true);
    expect(diskNote(0.4)?.text).toContain("Less than a minute");
  });
});

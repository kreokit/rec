import { describe, expect, it } from "vitest";
import { formatTime, savedEnd, snap, withEnd, withStart } from "./trim";

describe("trim", () => {
  it("snaps to whole frames", () => {
    expect(snap(1.01, 30)).toBeCloseTo(1, 9);
    expect(snap(1.02, 30)).toBeCloseTo(31 / 30, 9);
  });

  it("keeps the start before the end and inside the recording", () => {
    const trim = { start: 0, end: 10 };
    expect(withStart(trim, -3, 30).start).toBe(0);
    expect(withStart(trim, 4.5, 30).start).toBeCloseTo(4.5, 9);
    expect(withStart(trim, 12, 30).start).toBeCloseTo(10 - 1 / 30, 9);
  });

  it("keeps the end after the start and inside the recording", () => {
    const trim = { start: 2, end: 10 };
    expect(withEnd(trim, 20, 30, 10).end).toBe(10);
    expect(withEnd(trim, 1, 30, 10).end).toBeCloseTo(2 + 1 / 30, 9);
    expect(withEnd(trim, 7.5, 30, 10).end).toBeCloseTo(7.5, 9);
  });

  it("saves the recording's end as no end trim", () => {
    expect(savedEnd({ start: 0, end: 10 }, 10, 30)).toBeNull();
    expect(savedEnd({ start: 0, end: 9.99 }, 10, 30)).toBeNull();
    expect(savedEnd({ start: 0, end: 9 }, 10, 30)).toBe(9);
  });

  it("formats times with tenths", () => {
    expect(formatTime(4.24)).toBe("0:04.2");
    expect(formatTime(725)).toBe("12:05.0");
    expect(formatTime(3723.4)).toBe("1:02:03.4");
    expect(formatTime(59.96)).toBe("1:00.0");
  });
});

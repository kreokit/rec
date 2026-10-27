import { describe, expect, it } from "vitest";
import { DRIFT_S, correction, playFrom } from "./player";

describe("player", () => {
  it("puts a follower back in step only when it drifts", () => {
    expect(correction(10, 10.03)).toBeNull();
    expect(correction(10, 10 - DRIFT_S - 0.01)).toBe(10);
    expect(correction(10, 12)).toBe(10);
  });

  it("plays inside the trim", () => {
    const trim = { start: 2, end: 8 };
    expect(playFrom(5, trim, 30)).toBe(5);
    expect(playFrom(1, trim, 30)).toBe(2);
    expect(playFrom(8, trim, 30)).toBe(2);
    expect(playFrom(8 - 0.5 / 30, trim, 30)).toBe(2);
  });
});

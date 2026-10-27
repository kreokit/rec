import { describe, expect, it } from "vitest";
import { bigEnough, dragRegion, regionPixels } from "./region";

describe("dragRegion", () => {
  const start = { x: 100, y: 100, width: 400, height: 300 };

  it("draws a new region in any direction, kept on the display", () => {
    expect(dragRegion("new", null, { x: 300, y: 200 }, { x: 100, y: 50 }, 1440, 900)).toEqual({
      x: 100,
      y: 50,
      width: 200,
      height: 150,
    });
    expect(dragRegion("new", null, { x: 1400, y: 880 }, { x: 1600, y: 1000 }, 1440, 900)).toEqual({
      x: 1400,
      y: 880,
      width: 40,
      height: 20,
    });
  });

  it("moves without leaving the display", () => {
    expect(dragRegion("move", start, { x: 0, y: 0 }, { x: 50.4, y: -20 }, 1440, 900)).toEqual({ ...start, x: 150, y: 80 });
    expect(dragRegion("move", start, { x: 0, y: 0 }, { x: 5000, y: -5000 }, 1440, 900)).toEqual({
      ...start,
      x: 1040,
      y: 0,
    });
  });

  it("resizes from a corner, the opposite one staying put, and flips past it", () => {
    expect(dragRegion("se", start, { x: 500, y: 400 }, { x: 600, y: 450 }, 1440, 900)).toEqual({
      x: 100,
      y: 100,
      width: 500,
      height: 350,
    });
    expect(dragRegion("nw", start, { x: 100, y: 100 }, { x: 600, y: 500 }, 1440, 900)).toEqual({
      x: 500,
      y: 400,
      width: 100,
      height: 100,
    });
  });
});

describe("region size", () => {
  it("needs 64 points each way", () => {
    expect(bigEnough(null)).toBe(false);
    expect(bigEnough({ x: 0, y: 0, width: 64, height: 63 })).toBe(false);
    expect(bigEnough({ x: 0, y: 0, width: 64, height: 64 })).toBe(true);
  });

  it("turns points into the display's pixels", () => {
    expect(regionPixels({ x: 0, y: 0, width: 800, height: 450 }, 2880, 1440)).toEqual({ width: 1600, height: 900 });
    expect(regionPixels({ x: 0, y: 0, width: 800, height: 450 }, 1920, 0)).toEqual({ width: 800, height: 450 });
  });
});

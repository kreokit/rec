import { describe, expect, it } from "vitest";
import type { Bubble, Layout } from "./api";
import cases from "./layout-cases.json";
import { Placement, place, positionAt } from "./layout";

interface Case {
  name: string;
  layout: Layout;
  bubble: Bubble;
  screen: [number, number] | null;
  camera: [number, number] | null;
  expected: Placement;
}

describe("layout", () => {
  it.each((cases as Case[]).map((c) => [c.name, c] as const))("%s (shared with layout.rs)", (_, c) => {
    expect(place(c.layout, c.bubble, c.screen, c.camera)).toEqual(c.expected);
  });

  it("turns a dragged position back into the same place", () => {
    const bubble: Bubble = { shape: "circle", size: "medium", x: 0.37, y: 0.8 };
    const p = place("bubble", bubble, [1728, 1080], [1280, 720]);
    const back = positionAt(bubble, 1728, 1080, p.camera!.x, p.camera!.y);
    const again = place("bubble", { ...bubble, ...back }, [1728, 1080], [1280, 720]);
    expect(again.camera).toEqual(p.camera);
  });

  it("keeps a drag past the edges inside the picture", () => {
    const bubble: Bubble = { shape: "rounded", size: "large", x: 0.5, y: 0.5 };
    expect(positionAt(bubble, 1728, 1080, -500, 5000)).toEqual({ x: 0, y: 1 });
  });

  // Dragged far past every edge and corner, in every shape and size: the bubble stays
  // inside the picture with its margin, and a circle stays round (radius = half its size).
  it.each([
    [1728, 1080],
    [1080, 1920],
    [1280, 720],
  ])("a bubble dragged past the edges stays inside a %i×%i picture", (w, h) => {
    const short = Math.min(w, h);
    const margin = Math.round(short * 0.03);
    for (const shape of ["circle", "rounded"] as const) {
      for (const size of ["small", "medium", "large"] as const) {
        for (const [left, top] of [
          [-9999, -9999],
          [9999, -9999],
          [-9999, 9999],
          [9999, 9999],
          [w / 2, -9999],
          [w / 2, 9999],
          [-9999, h / 2],
          [9999, h / 2],
        ]) {
          const start: Bubble = { shape, size, x: 0.5, y: 0.5 };
          const p = place("bubble", { ...start, ...positionAt(start, w, h, left, top) }, [w, h], [1280, 720]);
          const c = p.camera!;
          const what = `${shape} ${size} dragged to (${left}, ${top}): ${JSON.stringify(c)}`;
          expect(c.x, what).toBeGreaterThanOrEqual(margin - 1);
          expect(c.y, what).toBeGreaterThanOrEqual(margin - 1);
          expect(c.x + c.width, what).toBeLessThanOrEqual(w - margin + 1);
          expect(c.y + c.height, what).toBeLessThanOrEqual(h - margin + 1);
          if (shape === "circle") {
            expect(c.width, what).toBe(c.height);
            expect(p.radius, what).toBe(c.height / 2);
          }
        }
      }
    }
  });
});

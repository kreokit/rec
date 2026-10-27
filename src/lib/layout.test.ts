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
});

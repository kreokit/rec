import { useCallback, useEffect, useRef, useState } from "react";
import { Display, Region, api } from "../lib/api";
import { Grip, MIN_REGION, bigEnough, dragRegion, regionPixels } from "../lib/region";
import { Button } from "../ui";
import "../styles/region.css";

export interface RegionSetup {
  display: Display;
  region: Region | null;
}

const CORNERS: Grip[] = ["nw", "ne", "sw", "se"];

/**
 * The see-through overlay over one display (its own window, `region_open`): drag a
 * rectangle, move it or pull a corner; Enter or "Use region" keeps it, Esc cancels.
 */
export function RegionOverlay({ setup }: { setup: RegionSetup }) {
  const { display } = setup;
  const width = display.points_width;
  const height = display.points_height;
  const [region, setRegion] = useState<Region | null>(setup.region);
  const drag = useRef<{ grip: Grip; start: Region | null; from: { x: number; y: number } } | null>(null);
  const [dragging, setDragging] = useState(false);

  const done = useCallback((chosen: Region | null) => void api.regionDone(chosen), []);
  const usable = bigEnough(region);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") done(null);
      if (e.key === "Enter" && usable) done(region);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [done, region, usable]);

  const begin = (grip: Grip) => (e: React.PointerEvent) => {
    if (e.button !== 0) return;
    e.stopPropagation();
    (e.currentTarget as Element).setPointerCapture(e.pointerId);
    drag.current = { grip, start: region, from: { x: e.clientX, y: e.clientY } };
    setDragging(true);
  };
  const move = (e: React.PointerEvent) => {
    const d = drag.current;
    if (!d) return;
    setRegion(dragRegion(d.grip, d.start, d.from, { x: e.clientX, y: e.clientY }, width, height));
  };
  const end = () => {
    drag.current = null;
    setDragging(false);
  };

  const pixels = region && regionPixels(region, display.width, width);
  // The bar goes under the region, or above it, or inside at the bottom when neither fits.
  const barTop = region
    ? region.y + region.height + 56 <= height
      ? region.y + region.height + 12
      : region.y >= 56
        ? region.y - 44
        : region.y + region.height - 44
    : 0;

  return (
    <div
      className={dragging ? "region-overlay dragging" : "region-overlay"}
      onPointerDown={begin("new")}
      onPointerMove={move}
      onPointerUp={end}
      onPointerCancel={end}
    >
      {region ? (
        <>
          {/* The dimmed rest of the display, around the region. */}
          <div className="region-dim" style={{ left: 0, top: 0, width, height: region.y }} />
          <div className="region-dim" style={{ left: 0, top: region.y + region.height, width, bottom: 0 }} />
          <div className="region-dim" style={{ left: 0, top: region.y, width: region.x, height: region.height }} />
          <div
            className="region-dim"
            style={{ left: region.x + region.width, top: region.y, right: 0, height: region.height }}
          />
          <div
            className={usable ? "region-box" : "region-box too-small"}
            style={{ left: region.x, top: region.y, width: region.width, height: region.height }}
            onPointerDown={begin("move")}
          >
            {CORNERS.map((c) => (
              <span key={c} className={`region-grip region-grip-${c}`} onPointerDown={begin(c)} />
            ))}
          </div>
          {!dragging && (
            <div
              className="region-bar"
              style={{ left: Math.min(Math.max(region.x, 12), width - 332), top: barTop }}
              onPointerDown={(e) => e.stopPropagation()}
            >
              <span className="region-size">
                {usable && pixels ? `${pixels.width} × ${pixels.height}` : `At least ${MIN_REGION} × ${MIN_REGION}`}
              </span>
              <Button variant="secondary" onClick={() => done(null)}>
                Cancel
              </Button>
              <Button variant="primary" disabled={!usable} onClick={() => done(region)}>
                Use region
              </Button>
            </div>
          )}
        </>
      ) : (
        <>
          <div className="region-dim" style={{ inset: 0 }} />
          <div className="region-hint">
            <strong>Drag over the part of the screen to record</strong>
            <span>Press Esc to cancel</span>
          </div>
        </>
      )}
    </div>
  );
}

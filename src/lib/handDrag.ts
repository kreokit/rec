import { useRef } from "react";
import { api } from "./api";

/**
 * Drags a floating window ("control", "bubble") by its background: the window moves by how
 * far the pointer went on screen (the panels never become the key window, so the system's
 * own window drag doesn't start). `onClick` runs for a press that didn't move.
 */
export function useHandDrag(label: "control" | "bubble", onClick?: () => void) {
  const drag = useRef<{ x: number; y: number; moved: boolean } | null>(null);
  const pending = useRef({ dx: 0, dy: 0, queued: false });

  const onPointerDown = (e: React.PointerEvent) => {
    if (e.button !== 0 || (e.target as Element).closest("button")) return;
    (e.currentTarget as Element).setPointerCapture(e.pointerId);
    drag.current = { x: e.screenX, y: e.screenY, moved: false };
  };
  const onPointerMove = (e: React.PointerEvent) => {
    const from = drag.current;
    if (!from) return;
    const p = pending.current;
    const dx = e.screenX - from.x;
    const dy = e.screenY - from.y;
    if (!dx && !dy) return;
    p.dx += dx;
    p.dy += dy;
    drag.current = { x: e.screenX, y: e.screenY, moved: true };
    if (p.queued) return;
    p.queued = true;
    requestAnimationFrame(() => {
      const { dx, dy } = p;
      Object.assign(p, { dx: 0, dy: 0, queued: false });
      if (dx || dy) void api.floatingMove(label, dx, dy);
    });
  };
  const onPointerUp = () => {
    const was = drag.current;
    drag.current = null;
    if (was && !was.moved) onClick?.();
  };

  return { onPointerDown, onPointerMove, onPointerUp, onPointerCancel: () => (drag.current = null) };
}

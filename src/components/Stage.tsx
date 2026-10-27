import { RefObject, useLayoutEffect, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import type { Bubble, Layout, Media } from "../lib/api";
import { place, positionAt } from "../lib/layout";

/**
 * The preview: the picture as it will be exported (layout.ts places the camera exactly as
 * layout.rs does for the export), scaled to fit. The bubble can be dragged; `onBubble` gets
 * its new position while dragging, `onBubbleDone` once it's let go.
 */
export function Stage({
  media,
  layout,
  bubble,
  screenSize,
  cameraSize,
  screenRef,
  cameraRef,
  onToggle,
  onBubble,
  onBubbleDone,
}: {
  media: Media;
  layout: Layout;
  bubble: Bubble;
  screenSize: [number, number] | null;
  cameraSize: [number, number] | null;
  screenRef: RefObject<HTMLVideoElement | null>;
  cameraRef: RefObject<HTMLVideoElement | null>;
  onToggle: () => void;
  onBubble: (position: { x: number; y: number }) => void;
  onBubbleDone: () => void;
}) {
  const stage = useRef<HTMLDivElement>(null);
  const [room, setRoom] = useState({ width: 0, height: 0 });
  const grab = useRef<{ dx: number; dy: number } | null>(null);

  useLayoutEffect(() => {
    const el = stage.current;
    if (!el) return;
    const observer = new ResizeObserver(([entry]) =>
      setRoom({ width: entry.contentRect.width, height: entry.contentRect.height }),
    );
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  const p = place(layout, bubble, screenSize, cameraSize);
  const scale = p.width && p.height ? Math.min(room.width / p.width, room.height / p.height) : 0;
  const pct = (v: number, of: number) => `${(v / of) * 100}%`;
  const cam = p.camera;
  const crop = p.camera_crop;
  const inBubble = layout === "bubble" && cam && crop;

  // The camera picture inside its box: the crop scaled to the box, so exactly the part the
  // export keeps is shown.
  const cameraStyle =
    cam && crop && cameraSize
      ? {
          width: pct(cameraSize[0], crop.width),
          height: pct(cameraSize[1], crop.height),
          left: pct(-crop.x, crop.width),
          top: pct(-crop.y, crop.height),
        }
      : undefined;

  const pictureAt = (clientX: number, clientY: number) => {
    const box = stage.current?.querySelector(".stage-picture")?.getBoundingClientRect();
    return box ? { x: (clientX - box.left) / scale, y: (clientY - box.top) / scale } : null;
  };

  return (
    <div ref={stage} className="finish-stage" aria-label="Preview" onClick={onToggle}>
      <div className="stage-picture" style={{ width: p.width * scale, height: p.height * scale }}>
        {media.screen && (
          <video
            ref={screenRef}
            className={layout === "camera" ? "stage-screen hidden" : "stage-screen"}
            src={convertFileSrc(media.screen)}
            preload="auto"
            muted
            playsInline
          />
        )}
        {media.camera && (
          <div
            className={inBubble ? "stage-camera bubble" : layout === "camera" ? "stage-camera" : "stage-camera hidden"}
            style={
              cam
                ? {
                    left: pct(cam.x, p.width),
                    top: pct(cam.y, p.height),
                    width: pct(cam.width, p.width),
                    height: pct(cam.height, p.height),
                    borderRadius: p.radius * scale,
                  }
                : undefined
            }
            title={inBubble ? "Drag to move the camera" : undefined}
            onClick={(e) => inBubble && e.stopPropagation()}
            onPointerDown={(e) => {
              if (!inBubble || e.button !== 0) return;
              e.stopPropagation();
              e.currentTarget.setPointerCapture(e.pointerId);
              const at = pictureAt(e.clientX, e.clientY);
              if (at) grab.current = { dx: at.x - cam.x, dy: at.y - cam.y };
            }}
            onPointerMove={(e) => {
              const at = grab.current && pictureAt(e.clientX, e.clientY);
              if (!at || !grab.current) return;
              onBubble(positionAt(bubble, p.width, p.height, at.x - grab.current.dx, at.y - grab.current.dy));
            }}
            onPointerUp={() => {
              if (grab.current) onBubbleDone();
              grab.current = null;
            }}
            onPointerCancel={() => (grab.current = null)}
          >
            <video
              ref={cameraRef}
              className="stage-camera-video"
              style={cameraStyle}
              src={convertFileSrc(media.camera)}
              preload="auto"
              muted
              playsInline
            />
          </div>
        )}
      </div>
    </div>
  );
}

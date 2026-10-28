import { useEffect, useState } from "react";
import { X } from "lucide-react";
import { api } from "../lib/api";
import { useHandDrag } from "../lib/handDrag";
import { ICON_STROKE, toast } from "../ui";
import { usePreviewPictures } from "./CameraPreview";
import "../styles/bubble.css";

type Size = "small" | "medium" | "large";
const SIZES: { value: Size; label: string }[] = [
  { value: "small", label: "S" },
  { value: "medium", label: "M" },
  { value: "large", label: "L" },
];

/**
 * The camera bubble while recording (its own window, `bubble_open` in lib.rs): the recorded
 * camera, mirrored like a mirror (the recording keeps the camera's own orientation; the
 * bubble in the video is made in Finish). Kept out of the recording. Drag it to move it; a
 * click shows its size and Hide for a few seconds.
 */
export function CameraBubble() {
  const src = usePreviewPictures(true);
  const [tools, setTools] = useState(false);
  const drag = useHandDrag("bubble", () => setTools((t) => !t));

  useEffect(() => {
    if (!tools) return;
    const timer = window.setTimeout(() => setTools(false), 4000);
    return () => window.clearTimeout(timer);
  }, [tools]);

  const act = (work: () => Promise<unknown>) => () =>
    void work().catch((e) => toast(String(e), "error"));

  return (
    <div className="recording-bubble" role="img" aria-label="Camera" {...drag}>
      {src ? <img src={src} alt="" draggable={false} /> : <span className="bubble-wait">Camera</span>}
      {tools && (
        <div className="bubble-tools">
          {SIZES.map((s) => (
            <button
              key={s.value}
              type="button"
              className="bubble-tool"
              aria-label={`${s.value} bubble`}
              onClick={act(() => api.bubbleResize(s.value))}
            >
              {s.label}
            </button>
          ))}
          <button type="button" className="bubble-tool" aria-label="Hide the bubble" onClick={act(() => api.bubbleHide())}>
            <X size={14} strokeWidth={ICON_STROKE} aria-hidden />
          </button>
        </div>
      )}
    </div>
  );
}

import { useEffect, useState } from "react";
import { VideoOff } from "lucide-react";
import { Camera, api } from "../lib/api";
import { ICON_STROKE } from "../ui";

// Start and stop run one after the other, so switching cameras never stops the new preview.
let order: Promise<unknown> = Promise.resolve();
const inOrder = <T,>(work: () => Promise<T>): Promise<T> => {
  const next = order.then(work, work);
  order = next.catch(() => {});
  return next;
};

/**
 * The chosen camera, live (about 15 pictures a second, mirrored like a mirror; the
 * recording keeps the camera's own orientation).
 */
export function CameraPreview({ camera }: { camera: Camera | undefined }) {
  const [src, setSrc] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setSrc(null);
    setError(null);
    if (!camera) return;
    let live = true;
    let seq = 0;
    let url: string | null = null;
    let timer = 0;
    const poll = async () => {
      try {
        const bytes = new Uint8Array(await api.cameraPreviewFrame(seq));
        if (live && bytes.length > 8) {
          seq = Number(new DataView(bytes.buffer, bytes.byteOffset, 8).getBigUint64(0, true));
          const next = URL.createObjectURL(new Blob([bytes.subarray(8)], { type: "image/jpeg" }));
          if (url) URL.revokeObjectURL(url);
          url = next;
          setSrc(next);
        }
      } catch {
        // The next poll tries again.
      }
      if (live) timer = window.setTimeout(() => void poll(), 60);
    };
    inOrder(() => api.cameraPreviewStart(camera.id, camera.width, camera.height)).then(
      () => live && void poll(),
      (e) => live && setError(String(e)),
    );
    return () => {
      live = false;
      window.clearTimeout(timer);
      if (url) URL.revokeObjectURL(url);
      void inOrder(() => api.cameraPreviewStop());
    };
  }, [camera]);

  return (
    <div
      className="camera-preview"
      style={camera ? { aspectRatio: `${camera.width} / ${camera.height}` } : undefined}
      aria-label={camera ? `${camera.name} preview` : "Camera off"}
      role="img"
    >
      {src ? (
        <img src={src} alt="" draggable={false} />
      ) : (
        <span className="camera-preview-empty">
          {!camera && <VideoOff size={18} strokeWidth={ICON_STROKE} aria-hidden />}
          {error ?? (camera ? "Starting the camera…" : "Camera off")}
        </span>
      )}
    </div>
  );
}

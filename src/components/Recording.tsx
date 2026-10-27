import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { Pause, Play, Square, TriangleAlert, X } from "lucide-react";
import { Live, Session, api } from "../lib/api";
import { formatBytes, formatDuration } from "../lib/format";
import { Button, ICON_STROKE, toast } from "../ui";
import { LevelMeter } from "./LevelMeter";
import { TopBar } from "./TopBar";
import "../styles/recording.css";

/** Warn when the disk has less than this left (the recording stops itself at 0.5 GB). */
const LOW_SPACE = 2e9;

/**
 * While recording: the time, pause/resume, stop and the microphone level. During the
 * countdown (its own window, Countdown.tsx): "Starting…" and Cancel.
 */
export function Recording({
  session,
  onDone,
  onCancelled,
}: {
  session: Session;
  onDone: (session: Session) => void;
  /** The countdown was cancelled: nothing was recorded. */
  onCancelled: () => void;
}) {
  const [live, setLive] = useState<Live | null>(null);
  const [stopping, setStopping] = useState(false);
  const stoppingRef = useRef(false);

  useEffect(() => {
    const poll = () =>
      void api.liveStatus().then((l) => {
        if (!stoppingRef.current) setLive(l);
      }, () => {});
    poll();
    const timer = window.setInterval(poll, 250);
    return () => window.clearInterval(timer);
  }, []);

  useEffect(() => {
    const stop = listen("recording-cancelled", () => onCancelled());
    return () => void stop.then((f) => f());
  }, [onCancelled]);

  // Stopped from the floating control.
  useEffect(() => {
    const stop = listen<Session>("recording-stopped", (e) => {
      if (stoppingRef.current) return; // stopped here: `stop` hands it on
      stoppingRef.current = true;
      onDone(e.payload);
    });
    return () => void stop.then((f) => f());
  }, [onDone]);

  const current = live?.session ?? session;
  const counting = current.state === "countdown";
  const paused = current.state === "paused";
  const stuck = !!current.error; // paused for good (disk almost full)

  const toggle = async () => {
    try {
      const next = paused ? await api.resumeRecording() : await api.pauseRecording();
      setLive((l) => (l ? { ...l, session: next } : l));
    } catch (e) {
      toast(String(e), "error");
    }
  };

  const stop = async () => {
    stoppingRef.current = true;
    setStopping(true);
    try {
      onDone(await api.stopRecording());
    } catch (e) {
      toast(String(e), "error");
      stoppingRef.current = false;
      setStopping(false);
    }
  };

  const failed = live?.status.failed;
  const lowSpace = live?.free_bytes != null && live.free_bytes < LOW_SPACE;

  return (
    <div className="screen">
      <TopBar title={<span className="app-name">KreoKit Rec</span>} />
      <main className="recording">
        <div className={paused || counting ? "rec-state paused" : "rec-state"}>
          <span className="rec-dot" aria-hidden />
          {stopping ? "Finishing the recording…" : counting ? "Starting…" : paused ? "Paused" : "Recording"}
        </div>
        <div className="rec-time" aria-live="off">
          {formatDuration(live?.status.elapsed ?? 0)}
        </div>
        <div className="rec-meta">
          {current.screen?.window ?? current.screen?.display}
          {current.screen?.region ? " (region)" : ""} · {current.screen?.width} × {current.screen?.height}
          {current.camera ? ` · ${current.camera.name}` : ""}
          {current.mic ? ` · ${current.mic}` : " · No microphone"}
          {current.system_audio ? " · Computer sound" : ""}
        </div>
        {current.mic && (
          <div className="rec-level">
            <LevelMeter peak={live?.status.mic_peak ?? 0} off={paused} />
          </div>
        )}

        <div className="rec-controls">
          {counting ? (
            <Button variant="secondary" icon={X} onClick={() => void api.countdownCancel()}>
              Cancel
            </Button>
          ) : (
            <>
              <Button
                variant="secondary"
                icon={paused ? Play : Pause}
                disabled={stopping || stuck}
                onClick={() => void toggle()}
              >
                {paused ? "Resume" : "Pause"}
              </Button>
              <Button variant="primary" icon={Square} disabled={stopping} onClick={() => void stop()}>
                Stop
              </Button>
            </>
          )}
        </div>

        {(failed || stuck || lowSpace) && (
          <p className="rec-warning" role="alert">
            <TriangleAlert size={16} strokeWidth={ICON_STROKE} aria-hidden />
            {current.error ??
              (failed
                ? "Something stopped working while recording. Stop now: everything recorded so far is kept and checked."
                : `Only ${formatBytes(live!.free_bytes!)} of disk space left. The recording stops by itself before the disk is full.`)}
          </p>
        )}

        <p className="rec-hint">
          {current.screen?.window
            ? "Only the chosen window is recorded, even when others cover it. If it closes, the recording keeps its last picture."
            : current.screen?.own_windows_kept_out
              ? "KreoKit Rec's own windows are kept out of the video."
              : "This window is recorded too while it's on screen. Minimize it (⌘M) if you don't want it in the video, and click KreoKit Rec in the Dock to come back."}
        </p>
      </main>
    </div>
  );
}

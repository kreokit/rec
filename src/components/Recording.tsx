import { useEffect, useRef, useState } from "react";
import { Pause, Play, Square, TriangleAlert } from "lucide-react";
import { Live, Session, api } from "../lib/api";
import { formatBytes, formatDuration } from "../lib/format";
import { Button, ICON_STROKE, toast } from "../ui";
import { LevelMeter } from "./LevelMeter";
import { TopBar } from "./TopBar";
import "../styles/recording.css";

/** Warn when the disk has less than this left (the recording stops itself at 0.5 GB). */
const LOW_SPACE = 2e9;

/**
 * While recording: the time, pause/resume, stop and the microphone level. (The countdown,
 * floating control and stop shortcut come in milestone 5.)
 */
export function Recording({ session, onDone }: { session: Session; onDone: (session: Session) => void }) {
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

  const current = live?.session ?? session;
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
        <div className={paused ? "rec-state paused" : "rec-state"}>
          <span className="rec-dot" aria-hidden />
          {stopping ? "Finishing the recording…" : paused ? "Paused" : "Recording"}
        </div>
        <div className="rec-time" aria-live="off">
          {formatDuration(live?.status.elapsed ?? 0)}
        </div>
        <div className="rec-meta">
          {current.screen?.display} · {current.screen?.width} × {current.screen?.height}
          {current.mic ? ` · ${current.mic}` : " · No microphone"}
        </div>
        {current.mic && (
          <div className="rec-level">
            <LevelMeter peak={live?.status.mic_peak ?? 0} off={paused} />
          </div>
        )}

        <div className="rec-controls">
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
          This window is recorded too while it's on screen. Minimize it (⌘M) if you don't want it in the video, and
          click KreoKit Rec in the Dock to come back.
        </p>
      </main>
    </div>
  );
}

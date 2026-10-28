import { useEffect, useState } from "react";
import { Pause, Play, Square, X } from "lucide-react";
import { Live, api } from "../lib/api";
import { diskNote, formatDuration } from "../lib/format";
import { useHandDrag } from "../lib/handDrag";
import { ICON_STROKE, toast } from "../ui";
import { LevelMeter } from "./LevelMeter";
import "../styles/control.css";

/**
 * The floating control while recording (its own window, `control_open` in lib.rs): time,
 * microphone level, pause/resume and stop. It never takes the keyboard from the app being
 * recorded, and it is kept out of the recording (or shown only where it isn't recorded).
 * Drag it by its background; where it was left is remembered per display.
 */
export function FloatingControl() {
  const [live, setLive] = useState<Live | null>(null);
  const [stopping, setStopping] = useState(false);

  useEffect(() => {
    const poll = () => void api.liveStatus().then(setLive, () => {});
    poll();
    const timer = window.setInterval(poll, 250);
    return () => window.clearInterval(timer);
  }, []);

  const session = live?.session;
  const counting = session?.state === "countdown";
  const paused = session?.state === "paused";
  const stuck = !!session?.error;
  const disk = diskNote(live?.minutes_left);

  const run = (action: () => Promise<unknown>) => () =>
    void action().catch((e) => {
      toast(String(e), "error");
      setStopping(false);
    });

  const drag = useHandDrag("control");

  const stop = run(async () => {
    setStopping(true);
    // The main window opens Finish; this window closes.
    await api.stopRecording();
  });

  return (
    <div className="control" {...drag}>
      <span className={paused || counting ? "control-dot paused" : "control-dot"} aria-hidden />
      <span className="control-time">
        {stopping ? "Finishing…" : counting ? "Starting…" : formatDuration(live?.status.elapsed ?? 0)}
      </span>
      {disk ? (
        <span className={disk.alert ? "control-disk alert" : "control-disk"} title={disk.text}>
          {live!.minutes_left! < 1 ? "<1" : Math.floor(live!.minutes_left!)} min of space left
        </span>
      ) : session?.mic && (
        <div className="control-level">
          <LevelMeter peak={live?.status.mic_peak ?? 0} off={paused || counting} />
        </div>
      )}
      {counting ? (
        <button
          type="button"
          className="ui-icon-button ui-button-secondary control-button"
          aria-label="Cancel"
          title="Cancel"
          onClick={run(() => api.countdownCancel())}
        >
          <X size={16} strokeWidth={ICON_STROKE} aria-hidden />
        </button>
      ) : (
        <>
          <button
            type="button"
            className="ui-icon-button ui-button-secondary control-button"
            aria-label={paused ? "Resume" : "Pause"}
            title={paused ? "Resume" : "Pause"}
            disabled={stopping || stuck || !session}
            onClick={run(() => (paused ? api.resumeRecording() : api.pauseRecording()))}
          >
            {paused ? (
              <Play size={16} strokeWidth={ICON_STROKE} aria-hidden />
            ) : (
              <Pause size={16} strokeWidth={ICON_STROKE} aria-hidden />
            )}
          </button>
          <button
            type="button"
            className="ui-icon-button ui-button-primary control-button"
            aria-label="Stop"
            title="Stop"
            disabled={stopping || !session}
            onClick={stop}
          >
            <Square size={16} strokeWidth={ICON_STROKE} aria-hidden />
          </button>
        </>
      )}
    </div>
  );
}

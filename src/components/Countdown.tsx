import { useEffect, useState } from "react";
import { api } from "../lib/api";
import { toast } from "../ui";
import "../styles/countdown.css";

/** Seconds counted before the recording begins. */
export const COUNTDOWN_SECONDS = 3;

/**
 * The 3-2-1 over the middle of what is recorded (its own window, `start_recording` opens
 * it). It is kept out of the recording where the system allows it, and closes just before
 * the recording begins. Esc cancels: nothing is recorded.
 */
export function Countdown() {
  const [left, setLeft] = useState(COUNTDOWN_SECONDS);

  useEffect(() => {
    if (left > 0) {
      const timer = window.setTimeout(() => setLeft(left - 1), 1000);
      return () => window.clearTimeout(timer);
    }
    api.countdownDone().catch((e) => toast(String(e), "error"));
  }, [left]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") void api.countdownCancel();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  return (
    <div className="countdown" role="timer" aria-live="assertive">
      <span className="countdown-number">{Math.max(left, 1)}</span>
      <span className="countdown-hint">Esc to cancel</span>
    </div>
  );
}

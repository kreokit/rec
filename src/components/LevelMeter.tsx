import { useEffect, useState } from "react";
import { api } from "../lib/api";
import { meterLevel } from "../lib/format";

/** The microphone's level as a bar: −60 to 0 dBFS; red near clipping. */
export function LevelMeter({ peak, off }: { peak: number; off?: boolean }) {
  const level = off ? 0 : meterLevel(peak);
  return (
    <div
      className={off ? "level-meter off" : peak >= 0.89 ? "level-meter clipping" : "level-meter"}
      role="meter"
      aria-label="Microphone level"
      aria-valuemin={0}
      aria-valuemax={1}
      aria-valuenow={Math.round(level * 100) / 100}
    >
      <div className="level-fill" style={{ width: `${level * 100}%` }} />
    </div>
  );
}

/** The level meter's microphone (opened by the setup screen), read 20 times a second. */
export function useMonitorPeak(active: boolean): number {
  const [peak, setPeak] = useState(0);
  useEffect(() => {
    if (!active) return setPeak(0);
    const timer = window.setInterval(() => void api.monitorLevel().then(setPeak, () => {}), 50);
    return () => window.clearInterval(timer);
  }, [active]);
  return peak;
}

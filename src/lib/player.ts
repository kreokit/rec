/**
 * Finish's player: the recording's tracks are separate media elements (screen, camera,
 * microphone, computer sound) played together. The first one with sound leads (sound
 * jumps are heard, a picture jump barely seen); the others follow it and are put back in
 * step when they drift. Playing stays inside the trim.
 */
import { RefObject, useCallback, useEffect, useRef, useState } from "react";
import type { Trim } from "./trim";

/** A follower further than this from the leader is put back in step (seconds). */
export const DRIFT_S = 0.08;

/** The time a follower should be put to, or null while it's close enough. */
export function correction(leader: number, follower: number, drift = DRIFT_S): number | null {
  return Math.abs(leader - follower) > drift ? leader : null;
}

/** Where Play starts: the trim's start when the playhead is outside it or at its end. */
export function playFrom(time: number, trim: Trim, fps: number): number {
  return time < trim.start || time >= trim.end - 1 / fps ? trim.start : time;
}

export interface Player {
  time: number;
  playing: boolean;
  play: () => void;
  pause: () => void;
  toggle: () => void;
  seek: (seconds: number) => void;
}

/** `refs` in order of preference for the lead (sound tracks first). */
export function usePlayer(refs: RefObject<HTMLMediaElement | null>[], trim: Trim, fps: number): Player {
  const [time, setTime] = useState(trim.start);
  const [playing, setPlaying] = useState(false);
  const timeRef = useRef(trim.start);
  const trimRef = useRef(trim);
  trimRef.current = trim;

  const elements = useCallback(() => refs.map((r) => r.current).filter((e): e is HTMLMediaElement => !!e), [refs]);

  const seek = useCallback(
    (seconds: number) => {
      timeRef.current = seconds;
      setTime(seconds);
      for (const el of elements()) el.currentTime = seconds;
    },
    [elements],
  );

  const pause = useCallback(() => {
    for (const el of elements()) el.pause();
    setPlaying(false);
  }, [elements]);

  const play = useCallback(() => {
    const from = playFrom(timeRef.current, trimRef.current, fps);
    if (from !== timeRef.current) seek(from);
    for (const el of elements()) void el.play().catch(() => undefined);
    setPlaying(true);
  }, [elements, fps, seek]);

  useEffect(() => {
    if (!playing) return;
    let frame = 0;
    const tick = () => {
      const [leader, ...followers] = elements();
      // While a seek is under way WebKit can still report the old time and `ended`
      // (measured: a jump to the end right after a seek), so the leader is read only once
      // it has arrived.
      if (leader && !leader.seeking) {
        const t = leader.currentTime;
        if (t >= trimRef.current.end || leader.ended) {
          pause();
          seek(trimRef.current.end);
          return;
        }
        for (const f of followers) {
          const to = correction(t, f.currentTime);
          if (to !== null) f.currentTime = to;
        }
        timeRef.current = t;
        setTime(t);
      }
      frame = requestAnimationFrame(tick);
    };
    frame = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(frame);
  }, [playing, elements, pause, seek]);

  return { time, playing, play, pause, toggle: () => (playing ? pause() : play()), seek };
}

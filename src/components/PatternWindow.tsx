import { useEffect, useRef, useState } from "react";

/** What the test window shows (fieldtest.rs opens it). */
export interface PatternSetup {
  /** "pattern": the counter; "sound": the counter and a click every second; "flash": the space-bar box. */
  mode: "pattern" | "sound" | "flash";
}

/** Counter steps a second, as testpattern.rs reads them. */
const RATE = 60;

/**
 * The counter for the time since the start; with `still`, it stands still for 2 s of every
 * 10 s (on the step before the 5th second's strip, so the strip doesn't stay white).
 */
export function counter(since: number, still: boolean): number {
  const phase = since % 10;
  const t = still && phase >= 5 && phase < 7 ? since - (phase - 5) - 0.5 / RATE : since;
  return Math.max(0, Math.floor(t * RATE));
}

/**
 * The test version's pattern window: the test pattern of src-tauri/src/testpattern.rs drawn
 * at the screen's own pixels (grey, 32 counter squares in the top row, a white strip for one
 * picture at each whole second), or a box that flashes white at each press of the space bar.
 */
export function PatternWindow({ setup }: { setup: PatternSetup }) {
  return setup.mode === "flash" ? <FlashBox /> : <Pattern clicks={setup.mode === "sound"} />;
}

function Pattern({ clicks }: { clicks: boolean }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const [needsClick, setNeedsClick] = useState(false);
  const audio = useRef<AudioContext | null>(null);

  useEffect(() => {
    const c = canvas.current;
    if (!c) return;
    const ratio = window.devicePixelRatio || 1;
    c.width = Math.round(window.innerWidth * ratio);
    c.height = Math.round(window.innerHeight * ratio);
    const g = c.getContext("2d");
    if (!g) return;
    const cell = Math.floor(c.width / 34);
    const zero = performance.now();
    let frame = 0;
    let lastSecond = -1;
    const draw = () => {
      const since = (performance.now() - zero) / 1000;
      const n = counter(since, !clicks);
      g.fillStyle = "rgb(128,128,128)";
      g.fillRect(0, 0, c.width, c.height);
      for (let b = 0; b < 32; b++) {
        g.fillStyle = (n >>> (31 - b)) & 1 ? "#fff" : "#000";
        g.fillRect(Math.floor(cell / 2) + b * cell, Math.floor(cell / 2), cell, cell);
      }
      g.fillStyle = n % RATE < 2 ? "#fff" : "#000";
      g.fillRect(0, cell * 2, c.width, cell * 2);
      // A click with the white strip of each whole second (scheduled ahead on the sound clock).
      const second = Math.floor(since);
      const ctx = audio.current;
      if (clicks && ctx && ctx.state === "running" && second !== lastSecond) {
        lastSecond = second;
        const at = ctx.currentTime + (second + 1 - since);
        const buffer = ctx.createBuffer(1, Math.round(ctx.sampleRate * 0.005), ctx.sampleRate);
        buffer.getChannelData(0).fill(0.5);
        const source = ctx.createBufferSource();
        source.buffer = buffer;
        source.connect(ctx.destination);
        source.start(at);
      }
      frame = requestAnimationFrame(draw);
    };
    frame = requestAnimationFrame(draw);
    return () => cancelAnimationFrame(frame);
  }, [clicks]);

  useEffect(() => {
    if (!clicks) return;
    const ctx = new AudioContext();
    audio.current = ctx;
    void ctx.resume().finally(() => setNeedsClick(ctx.state !== "running"));
    return () => void ctx.close();
  }, [clicks]);

  return (
    <div className="pattern-window">
      <canvas ref={canvas} className="pattern-canvas" />
      {needsClick && (
        <button
          className="pattern-sound"
          onClick={() => void audio.current?.resume().then(() => setNeedsClick(audio.current?.state !== "running"))}
        >
          Click here to turn on the test sound
        </button>
      )}
    </div>
  );
}

function FlashBox() {
  const [on, setOn] = useState(false);
  const [presses, setPresses] = useState(0);
  useEffect(() => {
    let off = 0;
    const down = (e: KeyboardEvent) => {
      if (e.repeat || e.key !== " ") return;
      e.preventDefault();
      setPresses((p) => p + 1);
      setOn(true);
      window.clearTimeout(off);
      off = window.setTimeout(() => setOn(false), 150);
    };
    window.addEventListener("keydown", down);
    return () => window.removeEventListener("keydown", down);
  }, []);
  return (
    <div className={on ? "flash-box flash-on" : "flash-box"} tabIndex={0}>
      {!on && (
        <p className="flash-text">
          {presses === 0
            ? "Click here, then press the space bar firmly about once a second."
            : "Keep pressing the space bar about once a second."}
        </p>
      )}
    </div>
  );
}

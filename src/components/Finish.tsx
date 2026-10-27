import { useEffect, useMemo, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  ArrowDownLeft,
  ArrowDownRight,
  ArrowUpLeft,
  ArrowUpRight,
  CircleCheck,
  Pause,
  Play,
  TriangleAlert,
} from "lucide-react";
import { Stage } from "./Stage";
import { Timeline } from "./Timeline";
import { TopBar } from "./TopBar";
import { Bubble, BubbleShape, BubbleSize, FinishSettings, Layout, Loudness, Media, Session, SoundProgress, api } from "../lib/api";
import { checkSummary, formatDuration, recordingTitle } from "../lib/format";
import { usePlayer } from "../lib/player";
import { Trim, formatTime, savedEnd, snap, withEnd, withStart } from "../lib/trim";
import { Button, ICON_STROKE, IconButton, Kbd, ProgressBar, SectionLabel, Segmented, Switch, toast } from "../ui";
import "../styles/finish.css";

const LOUDNESS: { value: Loudness; label: string; hint: string }[] = [
  { value: "social", label: "Social", hint: "−14 LUFS" },
  { value: "tutorial", label: "Tutorial & podcast", hint: "−16 LUFS" },
  { value: "keep", label: "Keep", hint: "as recorded" },
];

const DEFAULT_BUBBLE: Bubble = { shape: "circle", size: "medium", x: 1, y: 1 };

const SHAPES: { value: BubbleShape; label: string }[] = [
  { value: "circle", label: "Circle" },
  { value: "rounded", label: "Rounded" },
];

const SIZES: { value: BubbleSize; label: string }[] = [
  { value: "small", label: "Small" },
  { value: "medium", label: "Medium" },
  { value: "large", label: "Large" },
];

const CORNERS = [
  { label: "Top left", icon: ArrowUpLeft, x: 0, y: 0 },
  { label: "Top right", icon: ArrowUpRight, x: 1, y: 0 },
  { label: "Bottom left", icon: ArrowDownLeft, x: 0, y: 1 },
  { label: "Bottom right", icon: ArrowDownRight, x: 1, y: 1 },
];

/** Keys typed into these don't drive the player. */
const TYPING = new Set(["INPUT", "TEXTAREA", "SELECT"]);

/**
 * Finish: preview and trim the recording, choose its layout and voice, then export it.
 * Every change is saved with the recording right away (finish.json).
 */
export function Finish({ session, onHome }: { session: Session; onHome: () => void }) {
  const [settings, setSettings] = useState<FinishSettings | null>(null);
  const [media, setMedia] = useState<Media | null>(null);
  const [peaks, setPeaks] = useState<Uint8Array | null>(null);
  const saving = useRef<Promise<void>>(Promise.resolve());
  const duration = session.check?.duration_s || session.elapsed_s;
  const fps = session.screen?.fps ?? 30;

  useEffect(() => {
    Promise.all([api.finishSettings(session.id), api.recordingMedia(session.id)]).then(
      ([s, m]) => {
        setSettings(s);
        setMedia(m);
      },
      (e) => {
        toast(String(e), "error");
        onHome();
      },
    );
    api.recordingWaveform(session.id).then(
      (bytes) => setPeaks(new Uint8Array(bytes)),
      (e) => toast(String(e), "error"),
    );
  }, [session.id]);

  /**
   * Changes the choices and saves them (in order, so the last change wins); `save` false
   * while dragging, then a last change with `save` true.
   */
  const latest = useRef<FinishSettings | null>(null);
  const change = (patch: Partial<FinishSettings>, save = true) => {
    const base = latest.current ?? settings;
    if (!base) return;
    const next = { ...base, ...patch };
    latest.current = next;
    setSettings(next);
    if (save)
      saving.current = saving.current.then(() =>
        api.saveFinishSettings(session.id, next).catch((e) => toast(String(e), "error")),
      );
  };

  // The trim follows the handles while dragging; it's saved when they're let go.
  const [trim, setTrim] = useState<Trim>({ start: 0, end: duration });
  useEffect(() => {
    if (settings) setTrim({ start: settings.start_s, end: settings.end_s ?? duration });
    // Only when the saved choices arrive.
  }, [settings === null]);
  const saveTrim = (t: Trim) => change({ start_s: t.start, end_s: savedEnd(t, duration, fps) });

  const screenRef = useRef<HTMLVideoElement>(null);
  const cameraRef = useRef<HTMLVideoElement>(null);
  const soundRef = useRef<HTMLAudioElement>(null);
  const micRef = useRef<HTMLAudioElement>(null);
  const systemRef = useRef<HTMLAudioElement>(null);
  // The processed sound leads once it's there; until then the recorded tracks.
  const refs = useMemo(() => [soundRef, micRef, systemRef, screenRef, cameraRef], []);
  const player = usePlayer(refs, trim, fps);
  // Open at the trim's start.
  useEffect(() => {
    if (settings) player.seek(settings.start_s);
  }, [settings === null]);

  const setEdge = (edge: "start" | "end", seconds: number) => {
    const next = edge === "start" ? withStart(trim, seconds, fps) : withEnd(trim, seconds, fps, duration);
    setTrim(next);
    player.seek(edge === "start" ? next.start : next.end);
    return next;
  };

  // The processed sound for the voice choices, made in the background.
  const [sound, setSound] = useState<{ path: string | null; progress: SoundProgress | null; error: string | null }>({
    path: null,
    progress: null,
    error: null,
  });
  const hasSound = !!(session.mic || session.system_audio);
  useEffect(() => {
    if (!settings || !hasSound) return;
    let live = true;
    setSound((s) => ({ ...s, progress: { id: session.id, stage: "loudness", fraction: 0 }, error: null }));
    const stop = listen<SoundProgress>("sound-progress", (e) => {
      if (live && e.payload.id === session.id) setSound((s) => ({ ...s, progress: e.payload }));
    });
    api.prepareSound(session.id, settings.clean_voice, settings.loudness).then(
      (path) => {
        if (!live) return;
        // The new sound replaces the old one where the playhead is.
        player.pause();
        setSound({ path, progress: null, error: null });
      },
      (e) => live && String(e) !== "cancelled" && setSound((s) => ({ ...s, progress: null, error: String(e) })),
    );
    return () => {
      live = false;
      void stop.then((f) => f());
    };
  }, [settings?.clean_voice, settings?.loudness, settings === null]);
  useEffect(() => {
    if (sound.path) player.seek(player.time);
  }, [sound.path]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (TYPING.has((e.target as HTMLElement).tagName) || e.metaKey || e.ctrlKey || e.altKey) return;
      const frame = 1 / fps;
      const step = e.shiftKey ? 1 : frame;
      const seekTo = (t: number) => player.seek(Math.min(Math.max(snap(t, fps), 0), duration));
      switch (e.key) {
        case " ":
          player.toggle();
          break;
        case "ArrowLeft":
          seekTo(player.time - step);
          break;
        case "ArrowRight":
          seekTo(player.time + step);
          break;
        case "i":
        case "I":
          saveTrim(setEdge("start", player.time));
          break;
        case "o":
        case "O":
          saveTrim(setEdge("end", player.time));
          break;
        default:
          return;
      }
      e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const { text, ok } = checkSummary(session);
  const layouts: { value: Layout; label: string; disabled: boolean }[] = [
    { value: "screen", label: "Screen", disabled: !session.screen },
    { value: "bubble", label: "Screen + camera", disabled: !session.screen || !session.camera },
    { value: "camera", label: "Camera", disabled: !session.camera },
  ];
  const layout = settings?.layout ?? "screen";

  return (
    <div className="screen">
      <TopBar
        title={
          <>
            <span className="app-name">KreoKit Rec</span>
            <span className="topbar-name">
              {recordingTitle(session)} · {formatDuration(session.elapsed_s)}
            </span>
          </>
        }
        onHome={onHome}
      />
      <main className="finish-main">
        <div className="finish-work">
          <div className={ok ? "finish-check" : "finish-check problem"} role="status">
            {ok ? (
              <CircleCheck size={16} strokeWidth={ICON_STROKE} aria-hidden />
            ) : (
              <TriangleAlert size={16} strokeWidth={ICON_STROKE} aria-hidden />
            )}
            {text}
          </div>
          {media && (
            <Stage
              media={media}
              layout={layout}
              bubble={settings?.bubble ?? DEFAULT_BUBBLE}
              screenSize={session.screen ? [session.screen.width, session.screen.height] : null}
              cameraSize={session.camera ? [session.camera.width, session.camera.height] : null}
              screenRef={screenRef}
              cameraRef={cameraRef}
              onToggle={player.toggle}
              onBubble={(position) => settings && change({ bubble: { ...settings.bubble, ...position } }, false)}
              onBubbleDone={() => settings && change({}, true)}
            />
          )}
          {sound.path ? (
            <audio key={sound.path} ref={soundRef} src={convertFileSrc(sound.path)} preload="auto" />
          ) : (
            <>
              {media?.mic && <audio ref={micRef} src={convertFileSrc(media.mic)} preload="auto" />}
              {media?.system && <audio ref={systemRef} src={convertFileSrc(media.system)} preload="auto" />}
            </>
          )}
          <div className="transport">
            <IconButton
              icon={player.playing ? Pause : Play}
              label={player.playing ? "Pause" : "Play"}
              shortcut="Space"
              variant="secondary"
              onClick={player.toggle}
            />
            <span className="transport-time">
              {formatTime(player.time)} <span className="transport-total">/ {formatTime(duration)}</span>
            </span>
            <span className="transport-trim">
              Keeps {formatTime(trim.start)} – {formatTime(trim.end)} ({formatTime(trim.end - trim.start)})
            </span>
            <Button size="sm" onClick={() => saveTrim(setEdge("start", player.time))}>
              Start here <Kbd>I</Kbd>
            </Button>
            <Button size="sm" onClick={() => saveTrim(setEdge("end", player.time))}>
              End here <Kbd>O</Kbd>
            </Button>
          </div>
          <Timeline
            duration={duration}
            peaks={peaks}
            time={player.time}
            playing={player.playing}
            trim={trim}
            onSeek={(t) => player.seek(snap(t, fps))}
            onTrim={(edge, t) => setEdge(edge, t)}
            onTrimDone={() => saveTrim(trim)}
          />
        </div>
        {settings && (
          <aside className="finish-inspector" aria-label="Finish choices">
            <section className="finish-section">
              <SectionLabel>Layout</SectionLabel>
              <Segmented label="Layout" value={settings.layout} options={layouts} onChange={(l) => change({ layout: l })} />
            </section>
            {settings.layout === "bubble" && (
              <section className="finish-section">
                <SectionLabel>Camera bubble</SectionLabel>
                <Segmented
                  label="Shape"
                  value={settings.bubble.shape}
                  options={SHAPES}
                  onChange={(shape) => change({ bubble: { ...settings.bubble, shape } })}
                />
                <Segmented
                  label="Size"
                  value={settings.bubble.size}
                  options={SIZES}
                  onChange={(size) => change({ bubble: { ...settings.bubble, size } })}
                />
                <div className="finish-corners" role="group" aria-label="Corner">
                  {CORNERS.map((c) => (
                    <IconButton
                      key={c.label}
                      icon={c.icon}
                      label={c.label}
                      variant="secondary"
                      pressed={settings.bubble.x === c.x && settings.bubble.y === c.y}
                      onClick={() => change({ bubble: { ...settings.bubble, x: c.x, y: c.y } })}
                    />
                  ))}
                  <span className="finish-hint">Or drag the camera in the preview.</span>
                </div>
              </section>
            )}
            {session.mic && (
              <section className="finish-section">
                <SectionLabel>Voice</SectionLabel>
                <Switch
                  label="Clean up voice"
                  checked={settings.clean_voice}
                  onChange={(clean_voice) => change({ clean_voice })}
                />
                <p className="finish-hint">
                  Removes background noise from the microphone.
                  {session.system_audio ? " The computer sound stays as it is." : ""}
                </p>
              </section>
            )}
            {(session.mic || session.system_audio) && (
              <section className="finish-section">
                <SectionLabel>Loudness</SectionLabel>
                <Segmented
                  label="Loudness"
                  value={settings.loudness}
                  options={LOUDNESS}
                  onChange={(loudness) => change({ loudness })}
                />
                {sound.progress ? (
                  <div className="finish-sound-status">
                    <span>
                      {sound.progress.stage === "clean" ? "Cleaning up the voice…" : "Setting the loudness…"}{" "}
                      {Math.round(sound.progress.fraction * 100)}%
                    </span>
                    <ProgressBar value={sound.progress.fraction * 100} label="Preparing the sound" />
                  </div>
                ) : sound.error ? (
                  <p className="finish-sound-error" role="alert">
                    {sound.error}
                  </p>
                ) : sound.path ? (
                  <p className="finish-hint">The preview plays the sound as it will be exported.</p>
                ) : null}
              </section>
            )}
          </aside>
        )}
      </main>
    </div>
  );
}

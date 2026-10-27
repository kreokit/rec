import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Circle, Ellipsis, FolderOpen, LifeBuoy, Mic, Scissors, Trash2, TriangleAlert, Video } from "lucide-react";
import { listen } from "@tauri-apps/api/event";
import { Camera, Display, Estimate, Mic as MicDevice, Options, Os, Region, Session, Window, WindowChoice, api } from "../lib/api";
import { checkSummary, clockTime, formatBytes, formatDuration, formatRoom, recordingTitle } from "../lib/format";
import { regionPixels } from "../lib/region";
import { Button, Field, ICON_STROKE, Menu, MenuEntry, Segmented, Select, Switch, toast, useContextMenu } from "../ui";
import { CameraPreview } from "./CameraPreview";
import { ConfirmDialog } from "./ConfirmDialog";
import { LevelMeter, useMonitorPeak } from "./LevelMeter";
import { TopBar } from "./TopBar";
import "../styles/home.css";

const NO_MIC = "none";
const NO_CAMERA = "none";

/** The last setup, kept while the app runs: Home comes back as it was after a recording. */
const last: {
  displayId: string;
  area: "display" | "region" | "window";
  region: Region | null;
  windowId: string;
  cameraId: string;
  quality: "1080" | "720";
  systemAudio: boolean;
} = {
  displayId: "",
  area: "display",
  region: null,
  windowId: "",
  cameraId: NO_CAMERA,
  quality: "1080",
  systemAudio: false,
};

/** "App – Title", or whichever of the two it has. */
function windowName(w: Window): string {
  return w.app && w.title ? `${w.app} – ${w.title}` : w.app || w.title;
}

function windowChoice(w: Window): WindowChoice {
  return { id: w.id, name: windowName(w), width: w.width, height: w.height, scale: w.scale };
}

/** Home: recover a cut-off recording, set up and start a new one, or find a recent one. */
export function Home({
  micAllowed,
  onStart,
  onFinish,
}: {
  micAllowed: boolean;
  onStart: (session: Session) => void;
  /** Opens a recording in Finish. */
  onFinish: (session: Session) => void;
}) {
  const [displays, setDisplays] = useState<Display[]>([]);
  const [mics, setMics] = useState<MicDevice[]>([]);
  const [displayId, setDisplayId] = useState(last.displayId);
  const [micId, setMicId] = useState<string>(NO_MIC);
  const [cameras, setCameras] = useState<Camera[]>([]);
  // Off until chosen: the camera is optional.
  const [cameraId, setCameraId] = useState(last.cameraId);
  // The whole display, or part of it (dragged in the region overlay).
  const [area, setArea] = useState(last.area);
  const [region, setRegion] = useState(last.region);
  // Or a window (ScreenCaptureKit); the list is asked for each time "Window" is chosen.
  const [windows, setWindows] = useState<Window[]>([]);
  const [windowId, setWindowId] = useState(last.windowId);
  const [windowSupport, setWindowSupport] = useState<{ ok: boolean; note: string | null }>({ ok: false, note: null });
  // The computer's sound (macOS 13+).
  const [systemSupported, setSystemSupported] = useState(false);
  const [systemNote, setSystemNote] = useState<string | null>(null);
  // Regions (not on Linux desktops whose screen dialog chooses whole displays).
  const [regionSupport, setRegionSupport] = useState<{ ok: boolean; note: string | null }>({ ok: true, note: null });
  const [os, setOs] = useState<Os>("macos");
  const [systemAudio, setSystemAudio] = useState(last.systemAudio);
  const [quality, setQuality] = useState(last.quality);
  const [estimate, setEstimate] = useState<Estimate | null>(null);
  const [recent, setRecent] = useState<Session[] | null>(null);
  const [pending, setPending] = useState<Session[]>([]);
  const [busy, setBusy] = useState(false);
  const [deleting, setDeleting] = useState<Session | null>(null);
  const [removing, setRemoving] = useState<Session | null>(null);

  const refresh = useCallback(() => {
    api.listRecordings().then(setRecent, () => setRecent([]));
    api.pendingRecoveries().then(setPending, () => setPending([]));
  }, []);
  useEffect(refresh, [refresh]);

  useEffect(() => {
    api.devices().then((devices) => {
      const { displays, mics, cameras, system_audio, windows, windows_note } = devices;
      setDisplays(displays);
      setMics(mics);
      setCameras(cameras);
      setSystemSupported(system_audio);
      setSystemNote(devices.system_audio_note);
      if (!system_audio) setSystemAudio(false);
      setWindowSupport({ ok: windows, note: windows_note });
      setRegionSupport({ ok: devices.regions, note: devices.regions_note });
      setOs(devices.os);
      if (!windows && last.area === "window") setArea("display");
      if (!devices.regions && last.area === "region") setArea("display");
      // The display and camera chosen before, if they're still there.
      if (!displays.some((d) => String(d.id) === last.displayId)) {
        const main = displays.find((d) => d.main) ?? displays[0];
        setDisplayId(main ? String(main.id) : "");
        setRegion(null);
        setArea("display");
      }
      if (!cameras.some((c) => c.id === last.cameraId)) setCameraId(NO_CAMERA);
      const mic = mics.find((m) => m.default) ?? mics[0];
      setMicId(micAllowed && mic ? mic.id : NO_MIC);
    }, (e) => toast(String(e), "error"));
  }, [micAllowed]);

  useEffect(() => {
    Object.assign(last, { displayId, area, region, windowId, cameraId, quality, systemAudio });
  }, [displayId, area, region, windowId, cameraId, quality, systemAudio]);

  const loadWindows = useCallback(() => {
    api.windows().then(
      (list) => {
        setWindows(list);
        // The window chosen before, if it's still open; else the first one.
        setWindowId((id) => (list.some((w) => String(w.id) === id) ? id : list[0] ? String(list[0].id) : ""));
      },
      (e) => toast(String(e), "error"),
    );
  }, []);
  useEffect(() => {
    if (area === "window") loadWindows();
  }, [area, loadWindows]);

  const display = displays.find((d) => String(d.id) === displayId);
  const mic = mics.find((m) => m.id === micId);
  const camera = cameras.find((c) => c.id === cameraId);
  const peak = useMonitorPeak(!!mic);
  const chosenRegion = area === "region" ? region : null;
  const chosenWindowItem = area === "window" ? windows.find((w) => String(w.id) === windowId) : undefined;
  const chosenWindow = useMemo(() => (chosenWindowItem ? windowChoice(chosenWindowItem) : null), [chosenWindowItem]);
  const regionSize = chosenRegion && display ? regionPixels(chosenRegion, display.width, display.points_width) : null;
  const regionRef = useRef(region);
  regionRef.current = region;

  useEffect(() => {
    if (!display) return;
    api
      .estimate(display, chosenRegion, Number(quality), camera?.width ?? 0, camera?.height ?? 0, !!mic, chosenWindow, systemAudio)
      .then(setEstimate, () => setEstimate(null));
  }, [display, chosenRegion, quality, mic, camera, chosenWindow, systemAudio]);

  // The overlay's answer. Cancelled without a region before: back to the whole display.
  useEffect(() => {
    const off = listen<Region | null>("region-chosen", ({ payload }) => {
      if (payload) {
        setRegion(payload);
        setArea("region");
      } else if (!regionRef.current) {
        setArea("display");
      }
    });
    return () => void off.then((stop) => stop());
  }, []);

  const chooseRegion = () => {
    if (!display) return;
    setArea("region");
    api.regionOpen(display, region).catch((e) => {
      toast(String(e), "error");
      if (!region) setArea("display");
    });
  };

  const chooseDisplay = (id: string) => {
    setDisplayId(id);
    // A region belongs to its display.
    setRegion(null);
    setArea("display");
  };

  // Choosing a camera asks the system for it the first time (macOS); if it isn't allowed,
  // the camera stays off and the system's settings open at the right place.
  const chooseCamera = async (id: string) => {
    if (id === NO_CAMERA) return setCameraId(id);
    try {
      const { camera: access } = await api.permissions();
      const answer = access === "not_asked" ? await api.requestCamera() : access;
      if (answer === "allowed") return setCameraId(id);
      toast(
        os === "windows"
          ? "Windows doesn't let apps use the camera yet. In Settings, turn on camera access for desktop apps, then choose the camera again."
          : "KreoKit Rec may not use the camera yet. In System Preferences, tick KreoKit Rec under Camera, then choose the camera again.",
        "error",
      );
      await api.openPrivacySettings("camera");
    } catch (e) {
      toast(String(e), "error");
    }
  };

  // The level meter listens while a microphone is chosen.
  useEffect(() => {
    if (!mic) return;
    let live = true;
    api.monitorStart(mic.id).catch((e) => live && toast(String(e), "error"));
    return () => {
      live = false;
      void api.monitorStop();
    };
  }, [mic]);

  const record = async () => {
    if (!display) return;
    if (area === "window" && !chosenWindow) {
      toast("Choose a window to record, or record the whole display.", "error");
      return;
    }
    const options: Options = {
      display_id: display.id,
      display_name: display.name,
      display_width: display.width,
      display_height: display.height,
      quality: Number(quality),
      mic_id: mic ? mic.id : null,
      mic_name: mic ? mic.name : null,
      camera_id: camera ? camera.id : null,
      camera_name: camera ? camera.name : null,
      camera_width: camera?.width ?? 0,
      camera_height: camera?.height ?? 0,
      display_points_width: display.points_width,
      region: chosenRegion,
      window: chosenWindow,
      system_audio: systemAudio,
    };
    setBusy(true);
    try {
      onStart(await api.startRecording(options));
    } catch (e) {
      toast(String(e), "error");
      setBusy(false);
    }
  };

  const recover = async (session: Session) => {
    setBusy(true);
    try {
      const result = await api.recoverRecording(session.id);
      const { text, ok } = checkSummary(result);
      toast(
        result.state === "recovered" ? `Recovered ${formatDuration(result.elapsed_s)}. ${text}.` : text,
        ok ? "success" : "error",
      );
    } catch (e) {
      toast(String(e), "error");
    } finally {
      setBusy(false);
      refresh();
    }
  };

  const act = (work: () => Promise<void>, done: string) =>
    void work()
      .then(() => toast(done, "success"))
      .catch((e) => toast(String(e), "error"))
      .finally(refresh);

  const micOptions = useMemo(
    () => [
      ...mics.map((m) => ({ value: m.id, label: m.name })),
      { value: NO_MIC, label: micAllowed ? "No microphone" : "No microphone (not allowed)" },
    ],
    [mics, micAllowed],
  );

  const cameraOptions = useMemo(
    () => [
      { value: NO_CAMERA, label: "No camera" },
      ...cameras.map((c) => ({ value: c.id, label: c.name, hint: `${c.height}p` })),
    ],
    [cameras],
  );

  return (
    <div className="screen">
      <TopBar title={<span className="app-name">KreoKit Rec</span>} />
      <main className="home-main">
        {pending.map((session) => (
          <section key={session.id} className="recover-banner" aria-label="Unfinished recording">
            <LifeBuoy size={20} strokeWidth={ICON_STROKE} aria-hidden />
            <div className="recover-text">
              <strong>
                Recover recording from {clockTime(session.created_at)} ({formatDuration(session.elapsed_s)} long)
              </strong>
              <span>KreoKit Rec stopped while recording. Everything up to the last second or two can be saved.</span>
            </div>
            <Button variant="secondary" disabled={busy} onClick={() => setDeleting(session)}>
              Delete…
            </Button>
            <Button variant="primary" disabled={busy} onClick={() => void recover(session)}>
              Recover
            </Button>
          </section>
        ))}

        <section className="setup" aria-labelledby="setup-title">
          <h1 id="setup-title" className="setup-title">
            Record your screen
          </h1>
          <div className="setup-grid">
            <Field
              label="Screen"
              hint={
                area === "window" ? (
                  <span className="region-chosen">
                    {windows.length === 0 ? "No window to record. Open one, then refresh." : "Shows the window even when others cover it."}
                    <Button variant="ghost" size="sm" onClick={loadWindows}>
                      Refresh
                    </Button>
                  </span>
                ) : regionSize ? (
                  <span className="region-chosen">
                    Region {regionSize.width} × {regionSize.height}
                    <Button variant="ghost" size="sm" onClick={chooseRegion}>
                      Change…
                    </Button>
                  </span>
                ) : (
                  (!windowSupport.ok && windowSupport.note) || (!regionSupport.ok && regionSupport.note)
                )
              }
            >
              <div className="screen-choice">
                <Select
                  label="Screen"
                  value={displayId}
                  onChange={chooseDisplay}
                  options={displays.map((d) => ({
                    value: String(d.id),
                    label: d.name,
                    hint: `${d.width} × ${d.height}`,
                  }))}
                />
                <Segmented
                  label="What to record"
                  value={area}
                  onChange={(value) => (value === "region" ? chooseRegion() : setArea(value))}
                  options={[
                    { value: "display", label: "Whole display" },
                    { value: "region", label: "Region…", disabled: !regionSupport.ok },
                    { value: "window", label: "Window", disabled: !windowSupport.ok },
                  ]}
                />
                {area === "window" && (
                  <Select
                    label="Window"
                    value={windowId}
                    onChange={setWindowId}
                    options={windows.map((w) => ({
                      value: String(w.id),
                      label: windowName(w),
                      hint: `${Math.round(w.width * w.scale)} × ${Math.round(w.height * w.scale)}`,
                    }))}
                  />
                )}
              </div>
            </Field>
            <Field label="Quality" hint={estimate ? `${estimate.width} × ${estimate.height}, 30 fps` : undefined}>
              <Segmented
                label="Quality"
                value={quality}
                onChange={setQuality}
                options={[
                  { value: "1080", label: "1080p" },
                  { value: "720", label: "720p" },
                ]}
              />
            </Field>
            <Field label="Microphone">
              <Select label="Microphone" value={micId} onChange={setMicId} options={micOptions} />
            </Field>
            <Field label="Level">
              <LevelMeter peak={peak} off={!mic} />
            </Field>
            <Field label="Camera" hint={cameras.length === 0 ? "No camera found" : undefined}>
              <Select
                label="Camera"
                value={cameraId}
                onChange={(id) => void chooseCamera(id)}
                options={cameraOptions}
              />
            </Field>
            <Field label="Preview">
              <CameraPreview camera={camera} />
            </Field>
            <Field label="Computer sound" hint={systemSupported ? undefined : (systemNote ?? undefined)}>
              <Switch
                label="Record the computer's sound"
                checked={systemAudio}
                disabled={!systemSupported}
                onChange={setSystemAudio}
              />
            </Field>
          </div>
          <div className="setup-footer">
            <p className="setup-size">
              {estimate &&
                `About ${formatBytes(estimate.bytes_per_minute)} per minute` +
                  (estimate.free_bytes !== null
                    ? ` · ${formatBytes(estimate.free_bytes)} free (${formatRoom(
                        estimate.free_bytes,
                        estimate.bytes_per_minute,
                      )})`
                    : "")}
            </p>
            <Button
              variant="primary"
              className="record-button"
              icon={Circle}
              disabled={busy || !display}
              onClick={() => void record()}
            >
              Record
            </Button>
          </div>
        </section>

        <section className="recent" aria-labelledby="recent-title">
          <h2 id="recent-title" className="section-title">
            Recent recordings
          </h2>
          {recent === null ? null : recent.length === 0 ? (
            <p className="recent-empty">Your recordings appear here.</p>
          ) : (
            <ul className="recent-list">
              {recent.map((session) => (
                <RecentRow
                  key={session.id}
                  session={session}
                  onShow={() => void api.showRecording(session.id).catch((e) => toast(String(e), "error"))}
                  onFinish={() => onFinish(session)}
                  onRemove={() => setRemoving(session)}
                  onDelete={() => setDeleting(session)}
                />
              ))}
            </ul>
          )}
        </section>
      </main>

      {removing && (
        <ConfirmDialog
          title="Remove from Recent?"
          text="The recording stays on this computer in KreoKit Rec's folder; it only leaves this list."
          confirmLabel="Remove"
          onClose={() => setRemoving(null)}
          onConfirm={() => act(() => api.removeFromRecent(removing.id), "Removed from Recent.")}
        />
      )}
      {deleting && (
        <ConfirmDialog
          title="Delete recording?"
          text={`${recordingTitle(deleting)} (${formatDuration(deleting.elapsed_s)}) is deleted from this computer. This can't be undone.`}
          confirmLabel="Delete"
          onClose={() => setDeleting(null)}
          onConfirm={() => act(() => api.deleteRecording(deleting.id), "Recording deleted.")}
        />
      )}
    </div>
  );
}

function RecentRow({
  session,
  onShow,
  onFinish,
  onRemove,
  onDelete,
}: {
  session: Session;
  onShow: () => void;
  onFinish: () => void;
  onRemove: () => void;
  onDelete: () => void;
}) {
  const playable = session.state !== "failed";
  const items = (): MenuEntry[] => [
    ...(playable
      ? [
          { label: "Finish & export", icon: Scissors, onSelect: onFinish },
          { label: "Show in Finder", icon: FolderOpen, onSelect: onShow },
          "separator" as const,
        ]
      : []),
    { label: "Remove from Recent…", onSelect: onRemove },
    { label: "Delete recording…", icon: Trash2, onSelect: onDelete },
  ];
  const { onContextMenu, menu } = useContextMenu(items, "Recording actions");
  const { text, ok } = checkSummary(session);
  const size = session.screen ? `${session.screen.width} × ${session.screen.height}` : "Sound only";
  return (
    <li className="recent-row" onContextMenu={onContextMenu}>
      <button type="button" className="recent-open" onClick={playable ? onFinish : undefined} title="Finish & export">
        <span className="recent-icon">
          {session.screen ? (
            <Video size={18} strokeWidth={ICON_STROKE} aria-hidden />
          ) : (
            <Mic size={18} strokeWidth={ICON_STROKE} aria-hidden />
          )}
        </span>
        <span className="recent-info">
          <span className="recent-name">{recordingTitle(session)}</span>
          <span className="recent-meta">
            {formatDuration(session.elapsed_s)} · {size}
            {session.screen?.region ? " · Region" : ""}
            {session.screen?.window ? " · Window" : ""}
            {session.camera ? " · Camera" : ""}
            {session.mic ? " · Microphone" : ""}
            {session.system_audio ? " · Computer sound" : ""}
            {session.state === "recovered" ? " · Recovered" : ""}
          </span>
        </span>
        <span className={ok ? "recent-check" : "recent-check problem"}>
          {!ok && <TriangleAlert size={12} strokeWidth={2} aria-hidden />} {text}
        </span>
      </button>
      {playable && (
        <Button variant="secondary" size="sm" onClick={onFinish}>
          Finish & export
        </Button>
      )}
      <Menu buttonLabel={`More for ${recordingTitle(session)}`} icon={Ellipsis} variant="ghost" align="end" items={items()} />
      {menu}
    </li>
  );
}

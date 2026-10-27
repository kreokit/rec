import { useEffect, useRef, useState } from "react";
import { CircleCheck, TriangleAlert } from "lucide-react";
import { TopBar } from "./TopBar";
import { FinishSettings, Layout, Loudness, Session, api } from "../lib/api";
import { checkSummary, formatDuration, recordingTitle } from "../lib/format";
import { ICON_STROKE, SectionLabel, Segmented, Switch, toast } from "../ui";
import "../styles/finish.css";

const LOUDNESS: { value: Loudness; label: string; hint: string }[] = [
  { value: "social", label: "Social", hint: "−14 LUFS" },
  { value: "tutorial", label: "Tutorial & podcast", hint: "−16 LUFS" },
  { value: "keep", label: "Keep", hint: "as recorded" },
];

/**
 * Finish: preview and trim the recording, choose its layout and voice, then export it.
 * Every change is saved with the recording right away (finish.json).
 */
export function Finish({ session, onHome }: { session: Session; onHome: () => void }) {
  const [settings, setSettings] = useState<FinishSettings | null>(null);
  const saving = useRef<Promise<void>>(Promise.resolve());

  useEffect(() => {
    api.finishSettings(session.id).then(setSettings, (e) => {
      toast(String(e), "error");
      onHome();
    });
  }, [session.id]);

  /** Changes the choices and saves them (in order, so the last change wins). */
  const change = (patch: Partial<FinishSettings>) => {
    if (!settings) return;
    const next = { ...settings, ...patch };
    setSettings(next);
    saving.current = saving.current.then(() =>
      api.saveFinishSettings(session.id, next).catch((e) => toast(String(e), "error")),
    );
  };

  const { text, ok } = checkSummary(session);
  const layouts: { value: Layout; label: string; disabled: boolean }[] = [
    { value: "screen", label: "Screen", disabled: !session.screen },
    { value: "bubble", label: "Screen + camera", disabled: !session.screen || !session.camera },
    { value: "camera", label: "Camera", disabled: !session.camera },
  ];

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
          <div className="finish-stage" aria-label="Preview" />
        </div>
        {settings && (
          <aside className="finish-inspector" aria-label="Finish choices">
            <section className="finish-section">
              <SectionLabel>Layout</SectionLabel>
              <Segmented label="Layout" value={settings.layout} options={layouts} onChange={(layout) => change({ layout })} />
            </section>
            {session.mic && (
              <section className="finish-section">
                <SectionLabel>Voice</SectionLabel>
                <Switch
                  label="Clean up voice"
                  checked={settings.clean_voice}
                  onChange={(clean_voice) => change({ clean_voice })}
                />
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
              </section>
            )}
          </aside>
        )}
      </main>
    </div>
  );
}

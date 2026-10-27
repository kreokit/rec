import { CheckCircle2, CircleAlert, FlaskConical, Loader2, Save } from "lucide-react";
import { useEffect, useState } from "react";
import { FieldTest, FieldTestInfo, FieldTestResult, api } from "../lib/api";
import { Button, ICON_STROKE, toast } from "../ui";
import { TopBar } from "./TopBar";
import "../styles/tests.css";

/** One test: what it asks of the person, and how long it records. */
interface TestItem {
  id: FieldTest;
  title: string;
  steps: string;
  seconds: number;
  /** The test window it shows. */
  window: "pattern" | "sound" | "flash" | null;
}

/** The tests this computer can run (docs/manual-checklist.md). */
export function testsFor(info: FieldTestInfo): TestItem[] {
  const portal = info.os === "linux" && info.windows;
  const choose = portal ? " Your system asks what to share: choose the window called KreoKit Rec test pattern." : "";
  const all: (TestItem | false)[] = [
    !portal && {
      id: "screen",
      title: "Whole screen",
      steps: "A small window with moving squares opens. Don't touch anything for 30 seconds.",
      seconds: 30,
      window: "pattern",
    },
    !portal && {
      id: "region",
      title: "Part of the screen",
      steps: "The same window again; only the area around it is recorded. Don't touch anything for 20 seconds.",
      seconds: 20,
      window: "pattern",
    },
    info.windows && {
      id: "window",
      title: "One window",
      steps: `The moving squares again, recorded as a window. Don't touch anything for 20 seconds.${choose}`,
      seconds: 20,
      window: "pattern",
    },
    info.mics.length > 0 && {
      id: "sync",
      title: "Sound and picture together",
      steps: `A black box opens. Click it, then press the space bar firmly about once a second for 30 seconds (each press flashes the box). Keep the room quiet.${choose}`,
      seconds: 30,
      window: "flash",
    },
    info.cameras.length > 0 &&
      info.mics.length > 0 && {
        id: "camera",
        title: "Camera and sound together",
        steps: "Sit in front of the camera and clap your hands about once a second for 25 seconds, so the camera sees your hands.",
        seconds: 25,
        window: null,
      },
    info.computer_sound && {
      id: "sound",
      title: "Computer sound",
      steps: `Turn the sound on (not too loud). The test window clicks once a second for 15 seconds; if it asks, click it once to turn the sound on.${choose}`,
      seconds: 15,
      window: "sound",
    },
  ];
  return all.filter((t): t is TestItem => !!t);
}

type State = { kind: "idle" } | { kind: "running"; left: number } | { kind: "done"; result: FieldTestResult } | { kind: "failed"; error: string };

/**
 * The test version's Tests screen: each item of the manual checklist by clicking, then one
 * results file to send. The recordings are read back and deleted; only numbers are kept.
 */
export function TestScreen({ onApp }: { onApp: () => void }) {
  const [info, setInfo] = useState<FieldTestInfo | null>(null);
  const [states, setStates] = useState<Record<string, State>>({});
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState<string | null>(null);

  useEffect(() => {
    void api.fieldtestInfo().then(setInfo, (e) => toast(String(e), "error"));
  }, []);

  const set = (id: string, state: State) => setStates((s) => ({ ...s, [id]: state }));

  const run = async (test: TestItem) => {
    setBusy(true);
    setSaved(null);
    let timer = 0;
    try {
      const place = test.window ? await api.fieldtestPattern(test.window) : null;
      let left = test.seconds;
      set(test.id, { kind: "running", left });
      timer = window.setInterval(() => {
        left = Math.max(0, left - 1);
        set(test.id, { kind: "running", left });
      }, 1000);
      const result = await api.fieldtestRun(test.id, place);
      set(test.id, { kind: "done", result });
    } catch (e) {
      set(test.id, { kind: "failed", error: String(e) });
    } finally {
      window.clearInterval(timer);
      void api.fieldtestPatternClose();
      setBusy(false);
    }
  };

  const save = async () => {
    const results: Record<string, unknown> = {};
    for (const [id, s] of Object.entries(states)) {
      if (s.kind === "done") results[id] = s.result;
      if (s.kind === "failed") results[id] = { passed: false, error: s.error };
    }
    try {
      setSaved(await api.fieldtestSave({ saved_at: new Date().toISOString(), computer: info, tests: results }));
    } catch (e) {
      toast(String(e), "error");
    }
  };

  const tests = info ? testsFor(info) : [];
  const done = tests.filter((t) => states[t.id]?.kind === "done" || states[t.id]?.kind === "failed").length;

  return (
    <div className="screen">
      <TopBar title={<span className="app-name">KreoKit Rec Test</span>} onHome={onApp} />
      <main className="tests">
        <h1 className="tests-title">
          <FlaskConical size={26} strokeWidth={ICON_STROKE} aria-hidden /> Tests
        </h1>
        <p className="tests-intro">
          Thank you for trying the test version. Run each test once: it records for a few seconds, checks the recording
          and deletes it right away. Only the numbers are kept. At the end, save the results and send the file back.
        </p>
        {!info && <p className="tests-intro">Looking at this computer…</p>}
        {info && (
          <p className="test-computer">
            {info.os_version || info.os} · {info.cpus} cores · {info.displays.length} display
            {info.displays.length === 1 ? "" : "s"} · {info.cameras.length} camera{info.cameras.length === 1 ? "" : "s"} ·{" "}
            {info.mics.length} microphone{info.mics.length === 1 ? "" : "s"}
          </p>
        )}
        {tests.map((test, i) => {
          const state = states[test.id] ?? { kind: "idle" };
          return (
            <section className="test-row" key={test.id}>
              <span className="test-number">{i + 1}</span>
              <div className="test-text">
                <h2>{test.title}</h2>
                <p>{test.steps}</p>
                {state.kind === "running" && <p className="test-running">Recording… {state.left} s</p>}
                {state.kind === "done" && (
                  <p className={state.result.passed ? "test-passed" : "test-look"}>
                    {state.result.passed ? (
                      <CheckCircle2 size={16} strokeWidth={2} aria-hidden />
                    ) : (
                      <CircleAlert size={16} strokeWidth={2} aria-hidden />
                    )}
                    {state.result.passed ? "Passed. " : "Needs a look. "}
                    {state.result.summary}
                  </p>
                )}
                {state.kind === "failed" && (
                  <p className="test-look">
                    <CircleAlert size={16} strokeWidth={2} aria-hidden />
                    {state.error}
                  </p>
                )}
              </div>
              <Button
                variant={state.kind === "idle" ? "primary" : "secondary"}
                disabled={busy}
                icon={state.kind === "running" ? Loader2 : undefined}
                onClick={() => void run(test)}
              >
                {state.kind === "idle" ? "Start" : state.kind === "running" ? "Running" : "Again"}
              </Button>
            </section>
          );
        })}
        <div className="tests-footer">
          {saved && <p className="test-saved">Saved: {saved}. Please send this file back.</p>}
          <Button variant={done === tests.length && tests.length > 0 ? "primary" : "secondary"} icon={Save} disabled={busy || done === 0} onClick={() => void save()}>
            Save results
          </Button>
        </div>
      </main>
    </div>
  );
}

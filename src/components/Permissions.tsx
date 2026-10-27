import { useEffect, useState } from "react";
import { Camera, Check, Mic, Monitor, RotateCw } from "lucide-react";
import { Permissions as PermissionState, api } from "../lib/api";
import { Button, ICON_STROKE, toast } from "../ui";
import { TopBar } from "./TopBar";
import "../styles/permissions.css";

/**
 * What Rec may record, in plain words, with the way to allow each. Shown at start while
 * screen recording isn't allowed or the microphone wasn't asked yet; `onDone` continues to
 * Home (also without the microphone).
 */
export function Permissions({ initial, onDone }: { initial: PermissionState; onDone: () => void }) {
  const [state, setState] = useState(initial);
  // Screen Recording was opened in System Preferences: the restart step is next.
  const [screenAsked, setScreenAsked] = useState(false);

  // Allowing in System Preferences happens outside the app: check again every 2 s.
  useEffect(() => {
    const timer = window.setInterval(() => void api.permissions().then(setState, () => {}), 2000);
    return () => window.clearInterval(timer);
  }, []);

  const fail = (e: unknown) => toast(String(e), "error");

  const allowScreen = async () => {
    try {
      await api.requestScreen();
      await api.openPrivacySettings("screen");
      setScreenAsked(true);
    } catch (e) {
      fail(e);
    }
  };

  // Allowing didn't take effect: macOS's entry belongs to an older copy of the app.
  const resetScreen = async () => {
    try {
      await api.resetScreenPermission();
      await api.openPrivacySettings("screen");
      setScreenAsked(true);
    } catch (e) {
      fail(e);
    }
  };

  const allowMic = async () => {
    try {
      const answer = state.mic === "not_asked" ? await api.requestMic() : state.mic;
      if (answer === "denied") await api.openPrivacySettings("mic");
      setState(await api.permissions());
    } catch (e) {
      fail(e);
    }
  };

  const allowCamera = async () => {
    try {
      const answer = state.camera === "not_asked" ? await api.requestCamera() : state.camera;
      if (answer === "denied") await api.openPrivacySettings("camera");
      setState(await api.permissions());
    } catch (e) {
      fail(e);
    }
  };

  const screenOk = state.screen === "allowed";
  const stuck = !screenOk && !screenAsked && state.screen_asked_before;
  const micOk = state.mic === "allowed";

  return (
    <div className="screen">
      <TopBar title={<span className="app-name">KreoKit Rec</span>} />
      <main className="permissions">
        <h1 className="permissions-title">Before you record</h1>
        <p className="permissions-intro">
          macOS asks you to allow screen recording, the microphone and the camera for each app. Everything stays on
          this Mac.
        </p>

        <section className="permission-row">
          <span className="permission-icon">
            <Monitor size={20} strokeWidth={ICON_STROKE} aria-hidden />
          </span>
          <div className="permission-text">
            <h2>Screen Recording</h2>
            <p>So KreoKit Rec can record what's on your screen.</p>
            {stuck && (
              <div className="permission-stuck">
                <p className="permission-hint">
                  Screen Recording isn't on for KreoKit Rec yet. If you already ticked it, macOS is probably holding an
                  entry for an older copy of the app (this can happen after an update).
                </p>
                <ol className="permission-steps">
                  <li>Click Reset and try again: KreoKit Rec removes its old entry and asks again.</li>
                  <li>In System Preferences, click the lock, tick KreoKit Rec, choose Later.</li>
                  <li>Click Restart KreoKit Rec here.</li>
                </ol>
                <p>
                  If it still doesn't work: in System Preferences, select KreoKit Rec under Screen Recording, click –,
                  then click + and choose KreoKit Rec from Applications. Then restart the app.
                </p>
              </div>
            )}
            {!screenOk && screenAsked && (
              <ol className="permission-steps">
                <li>If macOS shows a message first, choose Open System Preferences.</li>
                <li>In System Preferences, click the lock and enter your password.</li>
                <li>Tick KreoKit Rec under Screen Recording.</li>
                {state.restart_after_screen ? (
                  <li>
                    macOS offers to quit the app: choose Later, then click Restart KreoKit Rec here. The permission only
                    applies after a restart.
                  </li>
                ) : (
                  <li>Restart KreoKit Rec when macOS asks.</li>
                )}
              </ol>
            )}
          </div>
          {screenOk ? (
            <Allowed />
          ) : stuck ? (
            <Button variant="primary" onClick={() => void resetScreen()}>
              Reset and try again
            </Button>
          ) : screenAsked ? (
            <Button variant="primary" icon={RotateCw} onClick={() => void api.restartApp()}>
              Restart KreoKit Rec
            </Button>
          ) : (
            <Button variant="primary" onClick={() => void allowScreen()}>
              Allow…
            </Button>
          )}
        </section>

        <section className="permission-row">
          <span className="permission-icon">
            <Mic size={20} strokeWidth={ICON_STROKE} aria-hidden />
          </span>
          <div className="permission-text">
            <h2>Microphone</h2>
            <p>So your voice is recorded with the screen. You can also record without it.</p>
            {state.mic === "denied" && (
              <p className="permission-hint">
                Not allowed yet: in System Preferences, tick KreoKit Rec under Microphone. It works right away.
              </p>
            )}
          </div>
          {micOk ? (
            <Allowed />
          ) : (
            <Button variant={screenOk ? "primary" : "secondary"} onClick={() => void allowMic()}>
              {state.mic === "denied" ? "Open System Preferences" : "Allow…"}
            </Button>
          )}
        </section>

        <section className="permission-row">
          <span className="permission-icon">
            <Camera size={20} strokeWidth={ICON_STROKE} aria-hidden />
          </span>
          <div className="permission-text">
            <h2>Camera</h2>
            <p>Only if you want yourself in the video. You can also allow it later, when you choose a camera.</p>
            {state.camera === "denied" && (
              <p className="permission-hint">
                Not allowed yet: in System Preferences, tick KreoKit Rec under Camera. It works right away.
              </p>
            )}
          </div>
          {state.camera === "allowed" ? (
            <Allowed />
          ) : (
            <Button variant="secondary" onClick={() => void allowCamera()}>
              {state.camera === "denied" ? "Open System Preferences" : "Allow…"}
            </Button>
          )}
        </section>

        <div className="permissions-footer">
          <Button variant={screenOk && micOk ? "primary" : "secondary"} disabled={!screenOk} onClick={onDone}>
            {screenOk && !micOk ? "Continue without microphone" : "Continue"}
          </Button>
        </div>
      </main>
    </div>
  );
}

function Allowed() {
  return (
    <span className="permission-allowed">
      <Check size={16} strokeWidth={2} aria-hidden /> Allowed
    </span>
  );
}

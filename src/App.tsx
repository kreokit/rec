import { useEffect, useState } from "react";
import { Home } from "./components/Home";
import { Permissions } from "./components/Permissions";
import { Recording } from "./components/Recording";
import { Permissions as PermissionState, Session, api } from "./lib/api";
import { checkSummary, formatDuration } from "./lib/format";
import { ToastRegion, toast } from "./ui";

type Screen =
  | { name: "loading" }
  | { name: "permissions"; permissions: PermissionState }
  | { name: "home"; micAllowed: boolean }
  | { name: "recording"; session: Session };

/** The app: permissions first if needed, then Home ⇄ Recording. */
function App() {
  const [screen, setScreen] = useState<Screen>({ name: "loading" });

  const home = () =>
    void api.permissions().then(
      (p) => setScreen({ name: "home", micAllowed: p.mic === "allowed" }),
      () => setScreen({ name: "home", micAllowed: false }),
    );

  useEffect(() => {
    void (async () => {
      // A recording already running (the window was reloaded) continues on its screen.
      const live = await api.liveStatus().catch(() => null);
      if (live) return setScreen({ name: "recording", session: live.session });
      const permissions = await api.permissions();
      const ready = permissions.screen === "allowed" && permissions.mic !== "not_asked";
      setScreen(ready ? { name: "home", micAllowed: permissions.mic === "allowed" } : { name: "permissions", permissions });
    })().catch((e) => {
      toast(String(e), "error");
      home();
    });
  }, []);

  const finished = (session: Session) => {
    const { text, ok } = checkSummary(session);
    toast(
      session.state === "failed" ? text : `Recording saved (${formatDuration(session.elapsed_s)}). ${text}.`,
      ok ? "success" : "error",
    );
    home();
  };

  return (
    <>
      {screen.name === "permissions" && <Permissions initial={screen.permissions} onDone={home} />}
      {screen.name === "home" && (
        <Home micAllowed={screen.micAllowed} onStart={(session) => setScreen({ name: "recording", session })} />
      )}
      {screen.name === "recording" && <Recording session={screen.session} onDone={finished} />}
      <ToastRegion />
    </>
  );
}

export default App;

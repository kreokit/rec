import { useEffect, useState } from "react";
import { Finish } from "./components/Finish";
import { Home } from "./components/Home";
import { Permissions } from "./components/Permissions";
import { Recording } from "./components/Recording";
import { TestScreen } from "./components/TestScreen";
import { setOpenTests } from "./components/TopBar";
import { Permissions as PermissionState, Session, api } from "./lib/api";
import { formatDuration } from "./lib/format";
import { ToastRegion, toast } from "./ui";

type Screen =
  | { name: "loading" }
  | { name: "permissions"; permissions: PermissionState }
  | { name: "home"; micAllowed: boolean }
  | { name: "recording"; session: Session }
  | { name: "finish"; session: Session }
  | { name: "tests" };

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
      // The test version opens on its Tests screen (once the permissions are given).
      const tests = await api.fieldtestInfo().catch(() => null);
      if (tests) setOpenTests(() => setScreen({ name: "tests" }));
      setScreen(
        !ready
          ? { name: "permissions", permissions }
          : tests
            ? { name: "tests" }
            : { name: "home", micAllowed: permissions.mic === "allowed" },
      );
    })().catch((e) => {
      toast(String(e), "error");
      home();
    });
  }, []);

  /** After Stop: straight to Finish (it shows the check); a failed recording goes Home. */
  const finished = (session: Session) => {
    if (session.state === "failed") {
      toast(session.error ?? "Nothing was recorded.", "error");
      return home();
    }
    toast(`Recording saved (${formatDuration(session.elapsed_s)}).`, "success");
    setScreen({ name: "finish", session });
  };

  return (
    <>
      {screen.name === "permissions" && <Permissions initial={screen.permissions} onDone={home} />}
      {screen.name === "home" && (
        <Home
          micAllowed={screen.micAllowed}
          onStart={(session) => setScreen({ name: "recording", session })}
          onFinish={(session) => setScreen({ name: "finish", session })}
        />
      )}
      {screen.name === "recording" && <Recording session={screen.session} onDone={finished} onCancelled={home} />}
      {screen.name === "finish" && <Finish session={screen.session} onHome={home} />}
      {screen.name === "tests" && <TestScreen onApp={home} />}
      <ToastRegion />
    </>
  );
}

export default App;

import { TopBar } from "./components/TopBar";
import { ToastRegion } from "./ui";

/** The app shell. Record setup and recent recordings arrive in milestone 2. */
function App() {
  return (
    <>
      <div className="screen">
        <TopBar title="KreoKit Rec" />
        <main className="shell-empty">
          <p>Record your screen, camera and microphone, offline. Coming together in milestone 2.</p>
        </main>
      </div>
      <ToastRegion />
    </>
  );
}

export default App;

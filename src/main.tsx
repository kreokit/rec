import React from "react";
import ReactDOM from "react-dom/client";
import "@fontsource-variable/inter";
import "./styles/tokens.css";
import "./styles/base.css";
import "./styles/shell.css";
import App from "./App";
import { Countdown } from "./components/Countdown";
import { PatternSetup, PatternWindow } from "./components/PatternWindow";
import { RegionOverlay, RegionSetup } from "./components/RegionOverlay";
import "./styles/tests.css";

declare global {
  interface Window {
    /** Set in the region overlay's window only (region_open in lib.rs). */
    __KREOKIT_REGION__?: RegionSetup;
    /** Set in the test version's pattern window only (fieldtest.rs). */
    __KREOKIT_PATTERN__?: PatternSetup;
    /** Set in the countdown's window only (start_recording in lib.rs). */
    __KREOKIT_COUNTDOWN__?: boolean;
  }
}

// macOS draws the window buttons inside our top bar (tauri.macos.conf.json).
if (navigator.userAgent.includes("Mac")) document.documentElement.classList.add("platform-mac");

const region = window.__KREOKIT_REGION__;
if (region) document.documentElement.classList.add("region-window");
const pattern = window.__KREOKIT_PATTERN__;
if (pattern) document.documentElement.classList.add("pattern-window-root");
const countdown = window.__KREOKIT_COUNTDOWN__;
if (countdown) document.documentElement.classList.add("countdown-window");

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    {pattern ? (
      <PatternWindow setup={pattern} />
    ) : region ? (
      <RegionOverlay setup={region} />
    ) : countdown ? (
      <Countdown />
    ) : (
      <App />
    )}
  </React.StrictMode>,
);

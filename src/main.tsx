import React from "react";
import ReactDOM from "react-dom/client";
import "@fontsource-variable/inter";
import "./styles/tokens.css";
import "./styles/base.css";
import "./styles/shell.css";
import App from "./App";
import { PatternSetup, PatternWindow } from "./components/PatternWindow";
import { RegionOverlay, RegionSetup } from "./components/RegionOverlay";
import "./styles/tests.css";

declare global {
  interface Window {
    /** Set in the region overlay's window only (region_open in lib.rs). */
    __KREOKIT_REGION__?: RegionSetup;
    /** Set in the test version's pattern window only (fieldtest.rs). */
    __KREOKIT_PATTERN__?: PatternSetup;
  }
}

// macOS draws the window buttons inside our top bar (tauri.macos.conf.json).
if (navigator.userAgent.includes("Mac")) document.documentElement.classList.add("platform-mac");

const region = window.__KREOKIT_REGION__;
if (region) document.documentElement.classList.add("region-window");
const pattern = window.__KREOKIT_PATTERN__;
if (pattern) document.documentElement.classList.add("pattern-window-root");

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    {pattern ? <PatternWindow setup={pattern} /> : region ? <RegionOverlay setup={region} /> : <App />}
  </React.StrictMode>,
);

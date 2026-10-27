import React from "react";
import ReactDOM from "react-dom/client";
import "@fontsource-variable/inter";
import "./styles/tokens.css";
import "./styles/base.css";
import "./styles/shell.css";
import App from "./App";
import { RegionOverlay, RegionSetup } from "./components/RegionOverlay";

declare global {
  interface Window {
    /** Set in the region overlay's window only (region_open in lib.rs). */
    __KREOKIT_REGION__?: RegionSetup;
  }
}

// macOS draws the window buttons inside our top bar (tauri.macos.conf.json).
if (navigator.userAgent.includes("Mac")) document.documentElement.classList.add("platform-mac");

const region = window.__KREOKIT_REGION__;
if (region) document.documentElement.classList.add("region-window");

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>{region ? <RegionOverlay setup={region} /> : <App />}</React.StrictMode>,
);

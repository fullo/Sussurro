import React from "react";
import ReactDOM from "react-dom/client";
import { getCurrentWindow } from "@tauri-apps/api/window";
import App from "./App";
import Overlay from "./Overlay";

// One frontend bundle serves both windows: the settings window renders the
// full app, the "overlay" window renders only the floating recording pill.
// CSS is bundled globally, so overlay-specific page styles are scoped via
// this body class instead of bare html/body selectors.
const isOverlay = getCurrentWindow().label === "overlay";
if (isOverlay) document.body.classList.add("overlay-window");

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>{isOverlay ? <Overlay /> : <App />}</React.StrictMode>,
);

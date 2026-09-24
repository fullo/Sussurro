import React from "react";
import ReactDOM from "react-dom/client";
import { getCurrentWindow } from "@tauri-apps/api/window";
import App from "./App";
import Overlay from "./Overlay";

// One frontend bundle serves both windows: the settings window renders the
// full app, the "overlay" window renders only the floating recording pill.
// CSS is bundled globally, so overlay-specific page styles are scoped via
// this body class instead of bare html/body selectors.
function render() {
  const isOverlay = getCurrentWindow().label === "overlay";
  if (isOverlay) document.body.classList.add("overlay-window");

  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>{isOverlay ? <Overlay /> : <App />}</React.StrictMode>,
  );
}

// Browser preview (`npm run dev` opened in a normal browser, not in Tauri):
// fake the backend with sample data. `import.meta.env.DEV` is a literal
// false in production builds, so this branch — and the mock module — is
// never bundled.
if (import.meta.env.DEV && !("__TAURI_INTERNALS__" in window)) {
  import("./dev/mockTauri").then((m) => {
    m.installMockTauri();
    render();
  });
} else {
  render();
}

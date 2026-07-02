import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import "./Overlay.css";

/** Floating pill shown while recording / transcribing. The window itself is
 *  shown/hidden from Rust (pipeline::update_overlay); this component only
 *  renders the right content for the current state. */
export default function Overlay() {
  const [status, setStatus] = useState("idle");

  useEffect(() => {
    const unlisten = listen<string>("pipeline-status", (e) => setStatus(e.payload));
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  const state = status.split(":")[0];
  return (
    <div className={`overlay-pill ${state}`}>
      <span className="overlay-dot" aria-hidden="true" />
      <span>{state === "processing" ? "Transcribing…" : "Recording"}</span>
    </div>
  );
}

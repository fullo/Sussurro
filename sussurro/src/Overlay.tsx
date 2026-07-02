import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import "./Overlay.css";

/** Floating pill shown while recording / transcribing, plus the live partial
 *  transcript while you speak. The window itself is shown/hidden from Rust
 *  (pipeline::update_overlay). */
export default function Overlay() {
  const [status, setStatus] = useState("idle");
  const [partial, setPartial] = useState("");

  useEffect(() => {
    const unlistenStatus = listen<string>("pipeline-status", (e) => {
      setStatus(e.payload);
      if (e.payload === "recording") setPartial(""); // new dictation
    });
    const unlistenPartial = listen<string>("partial-transcript", (e) =>
      setPartial(e.payload),
    );
    return () => {
      unlistenStatus.then((f) => f());
      unlistenPartial.then((f) => f());
    };
  }, []);

  const state = status.split(":")[0];
  return (
    <div className="overlay-wrap">
      <div className={`overlay-pill ${state}`}>
        <span className="overlay-dot" aria-hidden="true" />
        <span>{state === "processing" ? "Transcribing…" : "Recording"}</span>
      </div>
      {partial && <p className="overlay-text">{partial}</p>}
    </div>
  );
}

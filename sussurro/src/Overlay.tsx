import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import "./Overlay.css";

/** Floating pill shown while recording / transcribing, plus the live partial
 *  transcript while you speak — and for the whole of a browser meeting
 *  (#126, "Recording (meeting)"). The window itself is shown/hidden from
 *  Rust (pipeline::update_overlay / refresh_overlay). */
export default function Overlay() {
  const [status, setStatus] = useState("idle");
  const [partial, setPartial] = useState("");
  const [meeting, setMeeting] = useState(false);

  useEffect(() => {
    const unlistenStatus = listen<string>("pipeline-status", (e) => {
      setStatus(e.payload);
      if (e.payload === "recording") setPartial(""); // new dictation
    });
    const unlistenPartial = listen<string>("partial-transcript", (e) =>
      setPartial(e.payload),
    );
    const unlistenMeeting = listen<boolean>("meeting-live", (e) => setMeeting(e.payload));
    return () => {
      unlistenStatus.then((f) => f());
      unlistenPartial.then((f) => f());
      unlistenMeeting.then((f) => f());
    };
  }, []);

  const state = status.split(":")[0];
  // A dictation's own state wins while it runs; otherwise the meeting's.
  const dictating = state === "recording" || state === "processing";
  const label = dictating
    ? state === "processing"
      ? "Transcribing…"
      : "Recording"
    : meeting
      ? "Recording (meeting)"
      : "Recording";
  return (
    <div className="overlay-wrap">
      <div className={`overlay-pill ${dictating ? state : meeting ? "recording meeting" : state}`}>
        <span className="overlay-dot" aria-hidden="true" />
        <span>{label}</span>
      </div>
      {partial && <p className="overlay-text">{partial}</p>}
    </div>
  );
}

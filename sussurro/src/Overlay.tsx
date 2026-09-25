import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { levelToPercent, splitSettled, tailView, type PreviewSplit } from "./lib/overlayText";
import "./Overlay.css";

/** How often the input-level meter polls `mic_level` while recording. */
const LEVEL_POLL_MS = 100;
/** Characters of preview kept on screen: about three lines of the pill. */
const PREVIEW_MAX_CHARS = 150;

/** Floating pill shown while recording / transcribing, plus the live partial
 *  transcript while you speak — and for the whole of a browser meeting
 *  (#126, "Recording (meeting)"). The window itself is shown/hidden from
 *  Rust (pipeline::update_overlay / refresh_overlay) and is never focusable.
 *
 *  While a dictation records, a small input-level meter shows the mic is
 *  picking up audio, and the live preview (#98) draws the words two passes
 *  agreed on solid and the still-provisional tail as ghost text, keeping
 *  only the last lines of a long dictation. */
export default function Overlay() {
  const [status, setStatus] = useState("idle");
  const [preview, setPreview] = useState<PreviewSplit>({ settled: "", ghost: "" });
  const [meeting, setMeeting] = useState(false);
  const [level, setLevel] = useState(0);
  /** The previous partial, to tell settled words from revised ones. */
  const lastPartial = useRef("");

  useEffect(() => {
    const unlistenStatus = listen<string>("pipeline-status", (e) => {
      setStatus(e.payload);
      if (e.payload === "recording") {
        // New dictation.
        lastPartial.current = "";
        setPreview({ settled: "", ghost: "" });
      }
    });
    const unlistenPartial = listen<string>("partial-transcript", (e) => {
      setPreview(splitSettled(lastPartial.current, e.payload));
      lastPartial.current = e.payload;
    });
    const unlistenMeeting = listen<boolean>("meeting-live", (e) => setMeeting(e.payload));
    return () => {
      unlistenStatus.then((f) => f());
      unlistenPartial.then((f) => f());
      unlistenMeeting.then((f) => f());
    };
  }, []);

  const state = status.split(":")[0];

  // Input-level meter: poll the recorder only while a dictation records.
  useEffect(() => {
    if (state !== "recording") {
      setLevel(0);
      return;
    }
    const id = setInterval(() => {
      invoke<number>("mic_level")
        .then((rms) => setLevel(levelToPercent(rms)))
        .catch(() => {});
    }, LEVEL_POLL_MS);
    return () => clearInterval(id);
  }, [state]);

  // A dictation's own state wins while it runs; otherwise the meeting's.
  const dictating = state === "recording" || state === "processing";
  const label = dictating
    ? state === "processing"
      ? "Transcribing…"
      : "Recording"
    : meeting
      ? "Recording (meeting)"
      : "Recording";
  const view = tailView(preview, PREVIEW_MAX_CHARS);
  const hasText = (view.settled + view.ghost).trim() !== "";
  return (
    <div className="overlay-wrap">
      <div className={`overlay-pill ${dictating ? state : meeting ? "recording meeting" : state}`}>
        <span className="overlay-dot" aria-hidden="true" />
        <span>{label}</span>
        {state === "recording" && (
          <span className="overlay-meter" aria-hidden="true" title="Microphone input level">
            <span className="overlay-meter-fill" style={{ width: `${level}%` }} />
          </span>
        )}
      </div>
      {hasText && (
        <p className={`overlay-text${state === "processing" ? " is-final-pass" : ""}`}>
          <span className="overlay-lines">
            {view.clipped && <span className="overlay-ghost">… </span>}
            {view.settled && <span className="overlay-settled">{view.settled}</span>}
            {view.ghost && <span className="overlay-ghost">{view.ghost}</span>}
          </span>
        </p>
      )}
    </div>
  );
}

import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Ctl } from "../hooks/useAppController";

/** The status pill that is also the Dictate button: hold (push-to-talk) or
 *  click (toggle), exactly like the global hotkey. */
export function DictatePill({ ctl, className = "" }: { ctl: Ctl; className?: string }) {
  const [pillHover, setPillHover] = useState(false);
  const { state, status, modelReady } = ctl;
  const pushToTalk = ctl.settings.push_to_talk;
  const pillLabel =
    state === "recording"
      ? pushToTalk ? "● Recording — release to stop" : "■ Recording — click to stop"
      : state === "processing" ? "Transcribing…"
      : state === "error" ? status
      : !modelReady ? "Model not downloaded yet"
      : pillHover ? (pushToTalk ? "● Dictate — hold" : "● Dictate") : "Ready";

  const pillDown = () => {
    if (state === "idle" || state === "recording") invoke("trigger_dictation", { pressed: true });
  };
  const pillUp = () => {
    invoke("trigger_dictation", { pressed: false });
  };

  return (
    <button
      type="button"
      className={`status status-${state} pill-btn ${className}`.trim()}
      onMouseEnter={() => setPillHover(true)}
      onMouseLeave={() => {
        setPillHover(false);
        // Dragging off the button while holding must stop push-to-talk.
        if (state === "recording" && pushToTalk) pillUp();
      }}
      onMouseDown={pillDown}
      onMouseUp={pillUp}
      aria-label="Dictate: hold (push-to-talk) or click (toggle) to record"
    >
      {pillLabel}
    </button>
  );
}

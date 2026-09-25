import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Ctl } from "../hooks/useAppController";
import {
  ENROL_PARAGRAPHS,
  canFinishEnrolment,
  enrolLanguage,
  enrolmentClock,
  enrolmentFull,
  enrolmentPercent,
  type EnrolLanguage,
} from "../lib/ownVoice";
import { levelToPercent } from "../lib/overlayText";
import type { EnrolProgress, OwnVoiceStatus } from "../lib/types";

type Phase = "ready" | "recording" | "saving" | "error";

/** Record your voice (#243, P14): read a short paragraph aloud (~30 s) from
 *  the mic you pick; Sussurro keeps a voice print on this computer (never
 *  the recording) to label your voice "You" in single-channel recordings.
 *  Offered from the speaker panel of a room recording and from Settings →
 *  Voices. */
export function OwnVoiceDialog({
  ctl,
  onClose,
  onEnrolled,
}: {
  ctl: Ctl;
  onClose: () => void;
  /** The voice was recorded (or re-recorded). */
  onEnrolled: (status: OwnVoiceStatus) => void;
}) {
  const [status, setStatus] = useState<OwnVoiceStatus | null>(null);
  const [devices, setDevices] = useState<string[]>([]);
  const [device, setDevice] = useState(ctl.settings.input_device ?? "");
  const [lang, setLang] = useState<EnrolLanguage>(() =>
    enrolLanguage(ctl.settings.language ?? "", typeof navigator === "undefined" ? "" : navigator.language),
  );
  const [phase, setPhase] = useState<Phase>("ready");
  const [progress, setProgress] = useState<EnrolProgress | null>(null);
  const [error, setError] = useState("");
  const recording = useRef(false);

  useEffect(() => {
    invoke<OwnVoiceStatus>("own_voice_status").then(setStatus).catch(() => {});
    invoke<string[]>("list_input_devices").then(setDevices).catch(() => {});
    // Closing the dialog mid-reading throws the recording away.
    return () => {
      if (recording.current) invoke("own_voice_enrol_cancel").catch(() => {});
    };
  }, []);

  const finish = async () => {
    if (!recording.current) return;
    recording.current = false;
    setPhase("saving");
    try {
      const s = await invoke<OwnVoiceStatus>("own_voice_enrol_finish");
      onEnrolled(s);
    } catch (e) {
      setError(String(e));
      setPhase("error");
    }
  };

  // Poll the time and level while reading; save by itself at the maximum.
  useEffect(() => {
    if (phase !== "recording" || !status) return;
    const t = window.setInterval(async () => {
      try {
        const p = await invoke<EnrolProgress>("own_voice_enrol_progress");
        setProgress(p);
        if (p.failed) {
          recording.current = false;
          invoke("own_voice_enrol_cancel").catch(() => {});
          setError("The microphone stopped delivering audio. Check it and try again.");
          setPhase("error");
        } else if (enrolmentFull(p.elapsed_ms, status)) {
          finish();
        }
      } catch {
        /* next tick */
      }
    }, 150);
    return () => window.clearInterval(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [phase, status]);

  const start = async () => {
    setError("");
    setProgress(null);
    try {
      await invoke("own_voice_enrol_start", { device });
      recording.current = true;
      setPhase("recording");
    } catch (e) {
      setError(String(e));
      setPhase("error");
    }
  };

  const cancel = () => {
    if (recording.current) {
      recording.current = false;
      invoke("own_voice_enrol_cancel").catch(() => {});
    }
    onClose();
  };

  const elapsed = progress?.elapsed_ms ?? 0;
  const canStop = !!status && canFinishEnrolment(elapsed, status);
  const again = status?.enrolled;

  return (
    <div className="modal-backdrop" role="presentation" onClick={phase === "recording" || phase === "saving" ? undefined : cancel}>
      <div
        className="modal ov-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="ov-title"
        onClick={(e) => e.stopPropagation()}
        onKeyDown={(e) => {
          if (e.key === "Escape" && phase !== "saving") {
            e.stopPropagation();
            cancel();
          }
        }}
      >
        <h2 id="ov-title">{again ? "Record your voice again" : "Record your voice"}</h2>
        <p className="sh-muted">
          Read the paragraph below aloud, at your normal pace (about {Math.round((status?.target_ms ?? 30_000) / 1000)}{" "}
          seconds). Sussurro then labels your voice <b>You</b> in recordings made with one microphone — a meeting in
          the room, system audio without a separate mic, a transcription with Identify voices. It keeps a voice print
          on this computer only, never the recording, and never adds you to People.
        </p>

        {phase === "ready" && (
          <label className="ov-field">
            Microphone
            <select value={device} onChange={(e) => setDevice(e.target.value)}>
              <option value="">System default</option>
              {devices.map((d) => (
                <option key={d} value={d}>
                  {d}
                </option>
              ))}
            </select>
          </label>
        )}

        <blockquote className="ov-paragraph" lang={lang}>
          {ENROL_PARAGRAPHS[lang]}
        </blockquote>
        {phase === "ready" && (
          <p className="ov-lang">
            <button type="button" className="link-btn" onClick={() => setLang(lang === "it" ? "en" : "it")}>
              {lang === "it" ? "Read it in English instead" : "Leggilo in italiano"}
            </button>
          </p>
        )}

        {phase === "recording" && status && (
          <div className="ov-progress">
            <div
              className="vu"
              role="meter"
              aria-label="Input level"
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={Math.round(levelToPercent(progress?.level ?? 0))}
            >
              <div className="vu-fill" style={{ width: `${levelToPercent(progress?.level ?? 0)}%` }} />
            </div>
            <div
              className="progress"
              role="progressbar"
              aria-label="Reading time"
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={enrolmentPercent(elapsed, status)}
            >
              <div className="progress-fill" style={{ width: `${enrolmentPercent(elapsed, status)}%` }} />
            </div>
            <small className="sh-muted">
              {enrolmentClock(elapsed, status)}
              {canStop ? " · stop when you reach the end" : " · keep reading"}
            </small>
          </div>
        )}
        {phase === "saving" && (
          <p className="sh-note" role="status">
            Measuring your voice… The speaker model is downloaded on first use.
          </p>
        )}
        {phase === "error" && (
          <p className="ctx-note warn" role="alert">
            {error}
          </p>
        )}

        <div className="row-gap end">
          <button type="button" className="btn-ghost sh-btn" onClick={cancel} disabled={phase === "saving"}>
            Cancel
          </button>
          {(phase === "ready" || phase === "error") && (
            <button type="button" className="btn-dark sh-btn" onClick={start} autoFocus>
              {phase === "error" ? "Try again" : "Start reading"}
            </button>
          )}
          {phase === "recording" && (
            <button type="button" className="btn-dark sh-btn" onClick={finish} disabled={!canStop}>
              Stop and save
            </button>
          )}
        </div>
      </div>
    </div>
  );
}

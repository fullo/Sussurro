import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Card, Switch } from "../components/ui";
import type { Ctl } from "../hooks/useAppController";
import { ownVoiceSummary } from "../lib/ownVoice";
import type { OwnVoiceStatus } from "../lib/types";
import { OwnVoiceDialog } from "../shell/OwnVoiceDialog";

/** Settings → Voices: your own voice, "You" (#243, P14). Record it (read a
 *  paragraph aloud), record it again, turn *Label my voice as You* off, or
 *  forget it. */
export function OwnVoiceCard({ ctl }: { ctl: Ctl }) {
  const [status, setStatus] = useState<OwnVoiceStatus | null>(null);
  const [recording, setRecording] = useState(false);
  const [confirmForget, setConfirmForget] = useState(false);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    invoke<OwnVoiceStatus>("own_voice_status").then(setStatus).catch((e) => ctl.setBusy(String(e)));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const run = async (f: () => Promise<void>) => {
    setBusy(true);
    try {
      await f();
    } catch (e) {
      ctl.setBusy(String(e));
    } finally {
      setBusy(false);
    }
  };

  const setLabel = (on: boolean) =>
    run(async () => setStatus(await invoke<OwnVoiceStatus>("own_voice_set_label", { enabled: on })));

  const forget = () =>
    run(async () => {
      setConfirmForget(false);
      await invoke<boolean>("own_voice_forget");
      setStatus(await invoke<OwnVoiceStatus>("own_voice_status"));
      ctl.flash("Your voice was forgotten.", 3000);
    });

  return (
    <Card title="Your voice">
      <p className="card-hint">
        Optional. Read a short paragraph aloud once and Sussurro labels your voice <b>You</b> in recordings made with
        one microphone: a meeting in the room, system audio without a separate mic, a transcription with Identify
        voices. In browser meetings and System audio + mic your microphone is already “You”. The voice print stays on
        this computer (never in the archive or an export), the recording itself is not kept, and you are never added
        to People.
      </p>
      <p className="card-hint" role="status">
        {ownVoiceSummary(status)}
      </p>
      {status?.enrolled && (
        <div className="row-gap">
          <Switch checked={status.label_as_you} onChange={setLabel} label="Label my voice as You" />
          <span>Label my voice as You</span>
        </div>
      )}
      {status?.enrolled && (
        <p className="card-hint">
          The voice that sounds most like yours becomes “You” at the end of a recording, after Identify voices and
          after Re-detect speakers. A speaker you named or linked to someone keeps its name.
        </p>
      )}
      <div className="row-gap">
        <button type="button" className="btn-dark sh-btn" onClick={() => setRecording(true)} disabled={busy || !status}>
          {status?.enrolled ? "Record again" : "Record your voice"}
        </button>
        {status?.enrolled &&
          (confirmForget ? (
            <>
              <button type="button" className="btn-dark sh-btn" onClick={forget} disabled={busy} autoFocus>
                Forget my voice
              </button>
              <button type="button" className="btn-ghost sh-btn" onClick={() => setConfirmForget(false)} disabled={busy}>
                Cancel
              </button>
            </>
          ) : (
            <button type="button" className="btn-ghost sh-btn" onClick={() => setConfirmForget(true)} disabled={busy}>
              Forget my voice…
            </button>
          ))}
      </div>
      {confirmForget && (
        <p className="card-hint">
          The voice print is deleted from this computer. Documents keep the “You” labels they already have.
        </p>
      )}
      {recording && (
        <OwnVoiceDialog
          ctl={ctl}
          onClose={() => setRecording(false)}
          onEnrolled={(s) => {
            setStatus(s);
            setRecording(false);
            ctl.flash("Your voice is recorded.", 3000);
          }}
        />
      )}
    </Card>
  );
}

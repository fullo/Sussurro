import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Card, Switch, Tip } from "../components/ui";
import type { Ctl } from "../hooks/useAppController";
import type { VoiceStatus } from "../lib/types";
import { VOICE_STORED, VOICE_TELL, VOICE_WHERE, forgetAllPrompt } from "../lib/voiceRecognition";

/** Settings → Privacy → Voices (#242, P12/P13): suggestions on/off,
 *  *Forget all voices* with a confirmation, and what the law says in
 *  plain words. Per-person recognition lives in People. */
export function VoicesCard({ ctl, onOpenPeople }: { ctl: Ctl; onOpenPeople?: () => void }) {
  const { settings, save } = ctl;
  const [profiles, setProfiles] = useState<VoiceStatus[] | null>(null);
  const [confirm, setConfirm] = useState(false);
  const [busy, setBusy] = useState(false);

  const load = useCallback(() => {
    invoke<VoiceStatus[]>("voices_status")
      .then(setProfiles)
      .catch(() => setProfiles(null));
  }, []);
  useEffect(load, [load]);

  const n = profiles?.length ?? 0;
  const ready = profiles?.filter((p) => p.ready).length ?? 0;

  const forgetAll = async () => {
    setBusy(true);
    try {
      const gone = await invoke<number>("voices_forget_all");
      ctl.flash(gone ? `${gone} voice profile${gone === 1 ? "" : "s"} deleted.` : "Nothing to forget.", 3000);
      setConfirm(false);
      load();
    } catch (e) {
      ctl.setBusy(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Card title="Voices">
      <p className="card-hint">
        {VOICE_STORED} {VOICE_WHERE}
      </p>
      <div className="field">
        <div className="field-label">
          <span>
            Suggest names from known voices{" "}
            <Tip text="In a meeting or transcription, the speaker panel shows “Sounds like Anna · Link · Not Anna” when a Voice N matches someone whose voice is recognised. Nothing is ever linked without your click. Off hides every suggestion but keeps the voice profiles." />
          </span>
          <small>{settings.voice_suggestions === false ? "off: no suggestions" : "only for people with Recognise this voice on"}</small>
        </div>
        <Switch
          checked={settings.voice_suggestions !== false}
          onChange={(v) => save({ ...settings, voice_suggestions: v })}
          label="Suggest names from known voices"
        />
      </div>
      <div className="field">
        <div className="field-label">
          <span>Voice profiles on this computer</span>
          <small>
            {profiles === null
              ? "can't be read"
              : n === 0
                ? "none — turn on Recognise this voice for a person in People"
                : `${n} (${ready} ready to suggest)`}
          </small>
        </div>
        {onOpenPeople && (
          <button type="button" className="btn-ghost" onClick={onOpenPeople}>
            Open People
          </button>
        )}
      </div>
      {confirm ? (
        <div className="row-gap prof-actions" role="alertdialog" aria-label="Confirm forget all voices">
          <span>{forgetAllPrompt(n)}</span>
          <button type="button" className="btn-danger sh-btn push" disabled={busy} onClick={forgetAll}>
            Forget all voices
          </button>
          <button type="button" className="btn-ghost" autoFocus onClick={() => setConfirm(false)}>
            Cancel
          </button>
        </div>
      ) : (
        <div className="field">
          <div className="field-label">
            <span>Forget all voices</span>
            <small>deletes every voice profile and every “Not this person” answer at once</small>
          </div>
          <button type="button" className="btn-ghost" disabled={busy} onClick={() => setConfirm(true)}>
            Forget all voices…
          </button>
        </div>
      )}
      <p className="card-hint">
        <strong>Your responsibility.</strong> {VOICE_TELL} Anyone can ask you to delete their profile: turn off{" "}
        <em>Recognise this voice</em> for them, or forget all voices here. Voice N labels inside one document
        identify no one and are not voice profiles.
      </p>
    </Card>
  );
}

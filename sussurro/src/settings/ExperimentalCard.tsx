import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Card, Switch, Tip } from "../components/ui";
import type { Ctl } from "../hooks/useAppController";
import { TTS_ABOUT, offPrompt } from "../lib/tts";
import type { TtsStatus } from "../lib/types";
import { ExperimentalBadge } from "../shell/ReadAloudCard";

/** Settings → Experimental (P24): optional modules, off by default. Read
 *  aloud (#255) is the first: turning it on downloads nothing (that happens
 *  in Models → Voices, on request); turning it off offers to delete what
 *  was downloaded. */
export function ExperimentalCard({ ctl, onOpenModels }: { ctl: Ctl; onOpenModels: () => void }) {
  const { settings, save } = ctl;
  const [offAsk, setOffAsk] = useState<string | null>(null);
  const [deleting, setDeleting] = useState(false);
  const on = !!settings.tts_enabled;

  const toggle = async (next: boolean) => {
    if (!(await save({ ...settings, tts_enabled: next }))) return;
    if (next) {
      setOffAsk(null);
      return;
    }
    try {
      const s = await invoke<TtsStatus>("tts_status");
      setOffAsk(offPrompt(s.bytes_on_disk));
    } catch {
      setOffAsk(null);
    }
  };

  const deleteAll = async () => {
    setDeleting(true);
    try {
      await invoke("tts_delete", { language: null, voice: null });
      ctl.flash("Read-aloud models deleted.", 3000);
      setOffAsk(null);
    } catch (e) {
      ctl.setBusy(String(e));
    } finally {
      setDeleting(false);
    }
  };

  return (
    <Card title={<>Experimental <span className="via">optional modules, off by default</span></>}>
      <p className="card-hint">
        These features are still being tested. They stay off until you turn them on, and the rest of Sussurro never
        depends on them.
      </p>
      <div className="field">
        <div className="field-label">
          <span>
            Read aloud <ExperimentalBadge />{" "}
            <Tip text="Text-to-speech with Pocket TTS by Kyutai (Italian and English), on this computer, to an audio file. Turning it on downloads nothing: pick the languages and voices in Models → Voices, which shows each download's size and licence first. Audio it makes is synthetic speech and is marked as such." />
          </span>
          <small>{on ? "on — models are downloaded only from Models → Voices" : "off: no voice model is downloaded or used"}</small>
        </div>
        <Switch checked={on} onChange={toggle} label="Read aloud" />
      </div>
      <p className="card-hint">{TTS_ABOUT}</p>
      {on && (
        <div className="row-gap">
          <button type="button" className="btn-ghost sh-btn" onClick={onOpenModels}>
            Open Models → Voices
          </button>
        </div>
      )}
      {offAsk && (
        <div className="row-gap prof-actions" role="alertdialog" aria-label="Delete the read-aloud models">
          <span>{offAsk}</span>
          <button type="button" className="btn-danger sh-btn push" disabled={deleting} onClick={deleteAll}>
            Delete models
          </button>
          <button type="button" className="btn-ghost sh-btn" autoFocus onClick={() => setOffAsk(null)}>
            Keep them
          </button>
        </div>
      )}
    </Card>
  );
}

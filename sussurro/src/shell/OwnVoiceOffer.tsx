import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Ctl } from "../hooks/useAppController";
import { ownVoiceOffer } from "../lib/ownVoice";
import type { Item, OwnVoiceFound, OwnVoiceStatus } from "../lib/types";
import { OwnVoiceDialog } from "./OwnVoiceDialog";

/** The speaker panel's line about your own voice (#243, P14), in
 *  single-channel recordings with voice data: record your voice (then look
 *  for it here), look for it, or say that "You" came from it. */
export function OwnVoiceOffer({ ctl, item, onItem }: { ctl: Ctl; item: Item; onItem: (item: Item) => void }) {
  const [status, setStatus] = useState<OwnVoiceStatus | null>(null);
  const [recording, setRecording] = useState(false);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let alive = true;
    invoke<OwnVoiceStatus>("own_voice_status")
      .then((s) => alive && setStatus(s))
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, []);

  const offer = ownVoiceOffer(item, status);
  if (offer === "none") return null;

  const find = async () => {
    setBusy(true);
    try {
      const r = await invoke<OwnVoiceFound>("own_voice_find", { id: item.id });
      onItem(r.item);
      ctl.flash(
        r.found ? "Your voice is labelled “You”." : "No voice here sounds enough like yours to label it “You”.",
        3500,
      );
    } catch (e) {
      ctl.setBusy(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="spk-own-voice">
      {offer === "matched" && (
        <p className="ctx-note">
          “You” was found by your recorded voice. Rename that speaker if it isn't you: it won't be labelled “You”
          again in this document.
        </p>
      )}
      {offer === "enrol" && (
        <>
          <p className="ctx-note">
            Is one of these voices yours? Read a short paragraph aloud once and Sussurro labels your voice “You” in
            recordings like this one.
          </p>
          <button type="button" className="btn-ghost sh-btn" onClick={() => setRecording(true)} disabled={busy}>
            Record your voice
          </button>
        </>
      )}
      {offer === "find" && (
        <button type="button" className="btn-ghost sh-btn" onClick={find} disabled={busy}>
          {busy ? "Looking for your voice…" : "Find my voice"}
        </button>
      )}
      {recording && (
        <OwnVoiceDialog
          ctl={ctl}
          onClose={() => setRecording(false)}
          onEnrolled={(s) => {
            setStatus(s);
            setRecording(false);
            // Enrolled from this document: look for the voice here at once.
            find();
          }}
        />
      )}
    </div>
  );
}

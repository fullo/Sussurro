import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Ctl } from "../hooks/useAppController";
import { MAX_LABEL_CHARS, isVoice, labelProblem, redetectBlocked, speakerShares, speakerSource } from "../lib/speakers";
import type { Item } from "../lib/types";

/** The context pane's *Speakers* section (#130, 0.9 preview): the
 *  document's speakers with colour and share of speech, rename for this
 *  document, and "Re-detect speakers". Moving a line is on the line itself
 *  (transcript line actions). */
export function SpeakerPanel({ ctl, item, onItem }: { ctl: Ctl; item: Item; onItem: (item: Item) => void }) {
  const [renaming, setRenaming] = useState<{ id: string; label: string } | null>(null);
  const [confirmRedetect, setConfirmRedetect] = useState(false);
  const [busy, setBusy] = useState(false);
  const shares = speakerShares(item);
  const editable = !item.recording && !item.edited_externally;
  const blocked = redetectBlocked(item);
  const id = item.id;

  const call = async (cmd: string, args: Record<string, unknown>, done?: string) => {
    setBusy(true);
    try {
      const updated = await invoke<Item>(cmd, { id, ...args });
      onItem(updated);
      if (done) ctl.flash(done, 2500);
      return true;
    } catch (e) {
      ctl.setBusy(String(e));
      return false;
    } finally {
      setBusy(false);
    }
  };

  const saveName = async () => {
    if (!renaming) return;
    if (labelProblem(renaming.id, renaming.label)) return;
    const current = item.segments.speakers.find((s) => s.id === renaming.id)?.label ?? "";
    if (renaming.label.trim() === current) {
      setRenaming(null);
      return;
    }
    if (await call("archive_rename_speaker", { speakerId: renaming.id, label: renaming.label })) setRenaming(null);
  };

  const redetect = async () => {
    setConfirmRedetect(false);
    await call("archive_redetect_speakers", {}, "Speakers re-detected.");
  };

  const problem = renaming ? labelProblem(renaming.id, renaming.label) : "";

  return (
    <section className="ctx-sect" aria-labelledby="ctx-spk-h">
      <h3 id="ctx-spk-h">
        Speakers <span className="ctx-hint">this document</span>
      </h3>
      {shares.length === 0 ? (
        <p className="ctx-note">{blocked && !item.recording ? blocked : "No speakers yet."}</p>
      ) : (
        <ul className="spk-list">
          {shares.map(({ speaker, percent, lines }) => (
            <li key={speaker.id} className="spk-row">
              {renaming?.id === speaker.id ? (
                <form
                  className="spk-rename"
                  onSubmit={(e) => {
                    e.preventDefault();
                    saveName();
                  }}
                >
                  <input
                    autoFocus
                    aria-label={`New name for ${speaker.label}`}
                    value={renaming.label}
                    maxLength={MAX_LABEL_CHARS + 20}
                    placeholder={isVoice(speaker.id) ? speaker.id.replace("voice:", "Voice ") : "Name"}
                    disabled={busy}
                    onChange={(e) => setRenaming({ id: speaker.id, label: e.target.value })}
                    onKeyDown={(e) => {
                      if (e.key === "Escape") {
                        e.preventDefault();
                        e.stopPropagation();
                        setRenaming(null);
                      }
                    }}
                  />
                  <button type="submit" className="btn-dark sh-btn" disabled={busy || !!problem}>
                    Save
                  </button>
                  <button type="button" className="btn-ghost sh-btn" onClick={() => setRenaming(null)} disabled={busy}>
                    Cancel
                  </button>
                  {problem && <p className="ctx-note warn">{problem}</p>}
                </form>
              ) : (
                <>
                  <span className="tx-chip" style={{ background: speaker.color || undefined }} title={speaker.label}>
                    <i aria-hidden="true" />
                    {speaker.label}
                  </span>
                  <span className="spk-src">{speakerSource(speaker.id)}</span>
                  <span className="spk-pct" title={`${lines} line${lines === 1 ? "" : "s"}`}>
                    {percent}%
                  </span>
                  {editable && (
                    <span className="spk-acts">
                      <button
                        type="button"
                        className="link-btn"
                        disabled={busy}
                        onClick={() => setRenaming({ id: speaker.id, label: speaker.label })}
                        aria-label={`Rename ${speaker.label} in this document`}
                      >
                        Rename
                      </button>
                    </span>
                  )}
                </>
              )}
            </li>
          ))}
        </ul>
      )}
      {editable && item.embedded_segments ? (
        confirmRedetect ? (
          <div className="spk-confirm" role="group" aria-label="Confirm re-detect">
            <p className="ctx-note">
              Every line with voice data gets its voice again from the recording's voice prints — lines you moved by
              hand included. Names you gave to voices stay where the voice stays.
            </p>
            <div className="ctx-exports">
              <button type="button" className="btn-dark sh-btn" onClick={redetect} disabled={busy} autoFocus>
                Re-detect
              </button>
              <button type="button" className="btn-ghost sh-btn" onClick={() => setConfirmRedetect(false)} disabled={busy}>
                Cancel
              </button>
            </div>
          </div>
        ) : (
          <button type="button" className="btn-ghost sh-btn spk-redetect" onClick={() => setConfirmRedetect(true)} disabled={busy}>
            {busy ? "Working…" : "Re-detect speakers"}
          </button>
        )
      ) : (
        shares.length > 0 && blocked && <p className="ctx-note">{blocked}</p>
      )}
    </section>
  );
}

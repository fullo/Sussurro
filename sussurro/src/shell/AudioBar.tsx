import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Ctl } from "../hooks/useAppController";
import { audioChannel, audioLabel } from "../lib/audio";
import { formatBytes } from "../lib/format";
import type { Item } from "../lib/types";

/** The item's saved audio (#141): which files, how big, and "Delete audio,
 *  keep transcript" (to the OS trash, like the rest of the archive). Shown
 *  only when the item folder holds audio. */
export function AudioBar({ ctl, item, onItem }: { ctl: Ctl; item: Item; onItem: (item: Item) => void }) {
  const [confirm, setConfirm] = useState(false);
  const [busy, setBusy] = useState(false);
  const files = item.audio ?? [];
  if (files.length === 0) return null;

  const del = async () => {
    setConfirm(false);
    setBusy(true);
    try {
      const updated = await invoke<Item>("archive_delete_audio", { id: item.id });
      ctl.flash("Audio moved to the trash. The transcript is kept.");
      onItem(updated);
    } catch (e) {
      ctl.setBusy(String(e));
    } finally {
      setBusy(false);
    }
  };

  const detail = files
    .map((f) => `${f.name}${audioChannel(f) ? ` (${audioChannel(f)})` : ""} · ${formatBytes(f.bytes)}`)
    .join("\n");
  return (
    <div className="audio-bar" role="group" aria-label="Saved audio">
      <span aria-hidden="true">♪</span>
      <span title={detail}>{audioLabel(item)}</span>
      {item.folder_bytes ? (
        <span className="sh-muted" title="The whole item folder on disk, audio included">
          · item {formatBytes(item.folder_bytes)} on disk
        </span>
      ) : null}
      <button
        type="button"
        className="btn-ghost sh-btn push"
        disabled={busy || !!item.recording}
        onClick={() => setConfirm(true)}
      >
        Delete audio…
      </button>

      {confirm && (
        <div className="modal-backdrop" role="presentation" onClick={() => setConfirm(false)}>
          <div
            className="modal confirm-modal"
            role="alertdialog"
            aria-modal="true"
            aria-labelledby="del-audio-title"
            aria-describedby="del-audio-desc"
            onClick={(e) => e.stopPropagation()}
            onKeyDown={(e) => e.key === "Escape" && setConfirm(false)}
          >
            <h2 id="del-audio-title">Delete the audio, keep the transcript?</h2>
            <p id="del-audio-desc" className="sh-muted">
              {files.length === 1 ? files[0].name : `${files.length} audio files`} ({formatBytes(files.reduce((s, f) => s + f.bytes, 0))})
              {files.length === 1 ? " goes to the trash — you can restore it" : " go to the trash — you can restore them"}{" "}
              from there. The transcript, its lines and everything else in the item stay.
            </p>
            <div className="row-gap end">
              <button type="button" className="btn-ghost sh-btn" autoFocus onClick={() => setConfirm(false)}>
                Cancel
              </button>
              <button type="button" className="btn-danger" onClick={del}>
                Delete audio
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

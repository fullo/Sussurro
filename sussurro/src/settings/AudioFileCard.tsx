import { useEffect, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { CollapsibleCard, Tip } from "../components/ui";
import { useEngineRuns } from "../hooks/useEngineRuns";
import { isRunning, wasCancelled } from "../lib/engineRuns";
import { progressPercent } from "../lib/format";
import type { EngineResult } from "../lib/types";
import type { CardProps } from "./DictationCard";

export const AUDIO_EXTENSIONS = ["wav", "mp3", "m4a", "aac", "flac", "ogg"];

/** Pick an audio file to transcribe; null when the dialog is dismissed. */
export async function pickAudioFile(): Promise<string | null> {
  const path = await openDialog({
    title: "Transcribe an audio file",
    multiple: false,
    directory: false,
    filters: [{ name: "Audio", extensions: AUDIO_EXTENSIONS }],
  });
  return path && typeof path === "string" ? path : null;
}

/** Classic window only: transcribe an audio file into the archive. In the
 *  workspace this lives in New → File, with the Library behind it. */
export function AudioFileCard({ ctl, collapsible }: CardProps) {
  const { setBusy } = ctl;
  const engine = useEngineRuns();
  const file = engine.runs.file;
  const running = isRunning(file);
  /** Item type for the archive (P10 — note by default, or transcription). */
  const [fileItemType, setFileItemType] = useState<"note" | "transcription">("note");
  const [fileResult, setFileResult] = useState<EngineResult | null>(null);

  // Progress of *this* file's session (events are routed by session id),
  // shown in the status line.
  const pct = file?.progress ? progressPercent(file.progress.processed_s, file.progress.total_s) : null;
  useEffect(() => {
    if (running) setBusy(`Transcribing ${file.label}…${pct !== null ? ` ${pct}%` : ""}`);
  }, [running, file?.label, pct, setBusy]);
  // Errors go to the status line, as before; a cancel says nothing was saved.
  const failed = file?.status === "error" ? (wasCancelled(file) ? "Cancelled — nothing was saved." : file.error ?? "") : null;
  useEffect(() => {
    if (failed !== null) setBusy(failed);
  }, [failed, setBusy]);

  return (
    <CollapsibleCard
      storageKey="fileOpen"
      title={<>Audio file <span className="via">transcribe a recording</span></>}
      collapsible={collapsible}
    >
      <p className="card-hint">
        Transcribe a .wav / .mp3 / .m4a file with the current engine and cleanup — no dictation needed.
        <Tip text="Something Wispr Flow doesn't do: it's dictation-only. Long files are split at pauses and cleaned part by part; the result is saved in your archive (Documents/Sussurro) as a markdown file, not injected anywhere." />
      </p>
      <div className="field">
        <div className="field-label">
          <span>Save as <Tip text="Note: your own voice. Transcription: audio recorded by others (a podcast, an interview, a lecture) — shown with timestamps." /></span>
          <small>archive item type</small>
        </div>
        <select
          value={fileItemType}
          disabled={running}
          onChange={(e) => setFileItemType(e.target.value as "note" | "transcription")}
          aria-label="Save as"
        >
          <option value="note">Note</option>
          <option value="transcription">Transcription</option>
        </select>
      </div>
      <div className="model-row">
        <button
          disabled={!engine.canStartFile}
          onClick={async () => {
            const path = await pickAudioFile();
            if (!path) return;
            setFileResult(null);
            const result = await engine.startFile(path, fileItemType);
            setFileResult(result);
            if (result) setBusy("");
          }}
        >
          {running ? `Transcribing ${file.label}…` : "Choose audio file…"}
        </button>
        {running && (
          <button
            className="btn-ghost"
            disabled={file.sessionId === null}
            title={file.sessionId === null ? "Starting…" : "Stop and discard: nothing is saved"}
            onClick={() => engine.cancel("file")}
          >
            Cancel
          </button>
        )}
      </div>
      {fileResult && (
        <div className="history">
          <p className="cleaned">{fileResult.text}</p>
          <p className="card-hint">
            Saved to the archive as a {fileResult.item_type}: <code>{fileResult.item_id}</code>
          </p>
        </div>
      )}
    </CollapsibleCard>
  );
}

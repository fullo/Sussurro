import { useEffect, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { CollapsibleCard, Tip } from "../components/ui";
import type { Ctl } from "../hooks/useAppController";
import type { EngineRuns } from "../hooks/useEngineRuns";
import { describeProgress, isRunning, wasCancelled } from "../lib/engineRuns";
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
 *  workspace this lives in New → File, with the Library behind it. `engine`
 *  is the window's one `useEngineRuns`, so a file started in the workspace
 *  before ui_v2 was switched off shows up here too (#158). */
export function AudioFileCard({ ctl, collapsible, engine }: CardProps & { engine: EngineRuns }) {
  const { setBusy } = ctl;
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
  // A run adopted from the workspace (#158) ends through its events, not
  // through the click handler below: show its result the same way.
  const adoptedDone = fileResult === null && file?.status === "done" ? file.result : null;
  useEffect(() => {
    if (adoptedDone) setBusy("");
  }, [adoptedDone, setBusy]);
  const shown = fileResult ?? adoptedDone;

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
      {shown && (
        <div className="history">
          <p className="cleaned">{shown.text}</p>
          <p className="card-hint">
            Saved to the archive as a {shown.item_type}: <code>{shown.item_id}</code>
          </p>
        </div>
      )}
    </CollapsibleCard>
  );
}

/** Classic window only (#158): a microphone session keeps running when
 *  ui_v2 is switched off mid-recording, and the classic UI has no New
 *  screen. This notice shows it and lets the user stop it (the note is
 *  saved as usual) — never discard it from here. */
export function MicSessionNotice({ ctl, engine }: { ctl: Ctl; engine: EngineRuns }) {
  const run = engine.runs.mic;
  if (!run) return null;
  if (!isRunning(run)) {
    const text =
      run.status === "done" && run.result
        ? `The microphone session was saved to the archive: ${run.result.item_id}`
        : wasCancelled(run)
          ? "The microphone session was discarded."
          : `The microphone session failed: ${run.error ?? ""}${run.itemId ? ` — what was transcribed is kept as ${run.itemId}` : ""}`;
    return (
      <section className="card" role="status">
        <p className="card-hint">{text}</p>
        <div className="model-row">
          <button className="btn-ghost" onClick={() => engine.dismiss("mic")}>
            OK
          </button>
        </div>
      </section>
    );
  }
  return (
    <section className="card" role="status" aria-label="Microphone session">
      <p className="card-hint">
        A microphone session started in the workspace is {run.status === "running" ? "still recording" : "finishing"}
        {" · "}
        {describeProgress(run)}. Stop it to save the note in your archive (Documents/Sussurro).
      </p>
      {run.status === "running" && (
        <div className="model-row">
          <button
            onClick={async () => {
              const err = await engine.stopMic();
              if (err) ctl.setBusy(err);
            }}
          >
            ■ Stop and save
          </button>
        </div>
      )}
    </section>
  );
}

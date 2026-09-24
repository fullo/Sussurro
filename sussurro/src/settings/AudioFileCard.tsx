import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { CollapsibleCard, Tip } from "../components/ui";
import { baseName, progressPercent } from "../lib/format";
import type { EngineProgress, EngineResult } from "../lib/types";
import type { CardProps } from "./DictationCard";

export const AUDIO_EXTENSIONS = ["wav", "mp3", "m4a", "aac", "flac", "ogg"];

/** Classic window only: transcribe an audio file into the archive. In the
 *  workspace this lives in New → File, with the Library behind it. */
export function AudioFileCard({ ctl, collapsible }: CardProps) {
  const { setBusy } = ctl;
  /** Audio-file card (long-form engine, #113): item type for the archive
   *  (P10 — note by default, or transcription), the running file, and the
   *  last result. */
  const [fileItemType, setFileItemType] = useState<"note" | "transcription">("note");
  const [fileRunning, setFileRunning] = useState<string | null>(null);
  const [fileResult, setFileResult] = useState<EngineResult | null>(null);

  // Progress of the file being transcribed, shown in the status line.
  useEffect(() => {
    if (!fileRunning) return;
    const unlisten = listen<EngineProgress>("engine-progress", (e) => {
      const p = e.payload;
      setBusy(`Transcribing ${fileRunning}… ${progressPercent(p.processed_s, p.total_s)}%`);
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, [fileRunning, setBusy]);

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
          disabled={fileRunning !== null}
          onChange={(e) => setFileItemType(e.target.value as "note" | "transcription")}
          aria-label="Save as"
        >
          <option value="note">Note</option>
          <option value="transcription">Transcription</option>
        </select>
      </div>
      <button
        disabled={fileRunning !== null}
        onClick={async () => {
          const path = await openDialog({
            title: "Transcribe an audio file",
            multiple: false,
            directory: false,
            filters: [{ name: "Audio", extensions: AUDIO_EXTENSIONS }],
          });
          if (!path || typeof path !== "string") return;
          const name = baseName(path);
          setFileResult(null);
          setFileRunning(name);
          setBusy(`Transcribing ${name}…`);
          try {
            const result = await invoke<EngineResult>("transcribe_file", {
              path,
              itemType: fileItemType,
            });
            setFileResult(result);
            setBusy("");
          } catch (err) {
            setBusy(String(err));
          } finally {
            setFileRunning(null);
          }
        }}
      >
        {fileRunning ? `Transcribing ${fileRunning}…` : "Choose audio file…"}
      </button>
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

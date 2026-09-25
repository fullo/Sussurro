import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Tip } from "../components/ui";
import type { Ctl } from "../hooks/useAppController";
import { audioBytes, compressPercent, compressSummaryText, wavFiles } from "../lib/audio";
import { formatBytes } from "../lib/format";
import type { CompressProgress, CompressSummary, Item, UncompressedAudio } from "../lib/types";

/* *Compress audio* (#248, P16): saved WAV files become Ogg Opus (about 10×
   smaller), each checked by decoding it before the WAV goes to the trash.
   The backend runs one job at a time (archive::compress); these are its two
   entry points: one item (the document's audio bar) and every item
   (Settings → Archive). */

/** The running job's progress while `active`, 0–100. */
function useCompressPercent(active: boolean): number {
  const [pct, setPct] = useState(0);
  useEffect(() => {
    if (!active) {
      setPct(0);
      return;
    }
    let alive = true;
    const un = listen<CompressProgress>("audio-compress-progress", (e) => {
      if (alive) setPct(compressPercent(e.payload));
    });
    return () => {
      alive = false;
      un.then((f) => f()).catch(() => {});
    };
  }, [active]);
  return pct;
}

function ProgressBar({ pct, label }: { pct: number; label: string }) {
  return (
    <div className="progress" role="progressbar" aria-label={label} aria-valuemin={0} aria-valuemax={100} aria-valuenow={pct}>
      <div className="progress-fill" style={{ width: `${pct}%` }} />
    </div>
  );
}

/** "Compress audio" for one item: shown when its folder holds WAV audio. */
export function CompressItemButton({ ctl, item, onItem }: { ctl: Ctl; item: Item; onItem: (item: Item) => void }) {
  const [running, setRunning] = useState(false);
  const [cancelling, setCancelling] = useState(false);
  const pct = useCompressPercent(running);
  const wavs = wavFiles(item);
  if (wavs.length === 0 && !running) return null;

  const run = async () => {
    setRunning(true);
    try {
      const s = await invoke<CompressSummary>("archive_compress_audio", { id: item.id });
      ctl.flash(compressSummaryText(s));
      onItem(await invoke<Item>("archive_get", { id: item.id }));
    } catch (e) {
      ctl.setBusy(String(e));
    } finally {
      setRunning(false);
      setCancelling(false);
    }
  };

  if (running) {
    return (
      <span className="compress-run" role="status" aria-live="polite">
        <ProgressBar pct={pct} label="Compressing the audio" />
        <span className="sh-muted">{cancelling ? "Cancelling…" : `Compressing… ${pct}%`}</span>
        <button
          type="button"
          className="btn-ghost sh-btn"
          disabled={cancelling}
          onClick={() => {
            setCancelling(true);
            invoke("archive_compress_cancel").catch(() => {});
          }}
        >
          Cancel
        </button>
      </span>
    );
  }
  const size = formatBytes(audioBytes({ audio: wavs }));
  return (
    <button
      type="button"
      className="btn-ghost sh-btn"
      disabled={!!item.recording}
      onClick={run}
      title={`Convert ${wavs.length === 1 ? wavs[0].name : `${wavs.length} WAV files`} (${size}) to Opus, about 10× smaller. Each copy is checked before its WAV goes to the trash.`}
    >
      Compress audio
    </button>
  );
}

/** Settings → Archive: how much saved audio is still WAV, and "Compress
 *  all…" with progress and cancel. */
export function CompressAllField({ ctl }: { ctl: Ctl }) {
  const [left, setLeft] = useState<UncompressedAudio | null>(null);
  const [confirm, setConfirm] = useState(false);
  const [running, setRunning] = useState(false);
  const [cancelling, setCancelling] = useState(false);
  const pct = useCompressPercent(running);

  const refresh = useCallback(() => {
    invoke<UncompressedAudio>("archive_uncompressed_audio")
      .then(setLeft)
      .catch(() => setLeft(null));
  }, []);
  useEffect(refresh, [refresh]);

  const run = async () => {
    setConfirm(false);
    setRunning(true);
    try {
      const s = await invoke<CompressSummary>("archive_compress_audio", { id: null });
      ctl.flash(compressSummaryText(s));
      if (s.failed.length > 0) console.warn("Compress audio: not converted", s.failed);
    } catch (e) {
      ctl.setBusy(String(e));
    } finally {
      setRunning(false);
      setCancelling(false);
      refresh();
    }
  };

  const nothing = !left || left.items === 0;
  return (
    <div className="field">
      <div className="field-label">
        <span>
          Compress audio{" "}
          <Tip text="Converts the audio already saved as WAV to Opus, about 10× smaller (11 MB instead of 115 MB per hour), with no difference for transcription or voice labels. Each Opus copy is decoded and checked before its WAV goes to the trash, so you can restore it from there. Items being recorded are skipped." />
        </span>
        <small>
          {running
            ? cancelling
              ? "cancelling…"
              : `compressing… ${pct}%`
            : nothing
              ? "no WAV audio in the archive"
              : `${left.items} item${left.items === 1 ? "" : "s"} with WAV audio · ${formatBytes(left.bytes)}`}
        </small>
      </div>
      {running ? (
        <div className="row-gap">
          <ProgressBar pct={pct} label="Compressing the archive's audio" />
          <button
            type="button"
            className="btn-ghost"
            disabled={cancelling}
            onClick={() => {
              setCancelling(true);
              invoke("archive_compress_cancel").catch(() => {});
            }}
          >
            Cancel
          </button>
        </div>
      ) : (
        <button type="button" className="btn-ghost" disabled={nothing} onClick={() => setConfirm(true)}>
          Compress all…
        </button>
      )}

      {confirm && left && (
        <div className="modal-backdrop" role="presentation" onClick={() => setConfirm(false)}>
          <div
            className="modal confirm-modal"
            role="alertdialog"
            aria-modal="true"
            aria-labelledby="compress-all-title"
            aria-describedby="compress-all-desc"
            onClick={(e) => e.stopPropagation()}
            onKeyDown={(e) => e.key === "Escape" && setConfirm(false)}
          >
            <h2 id="compress-all-title">Compress all saved audio to Opus?</h2>
            <p id="compress-all-desc" className="sh-muted">
              {left.items} item{left.items === 1 ? "" : "s"} hold {formatBytes(left.bytes)} of WAV audio; as Opus it takes
              about a tenth of that. Each file is checked after conversion, then its WAV goes to the trash — you can
              restore it from there. It takes a few seconds per hour of audio, and you can cancel.
            </p>
            <div className="row-gap end">
              <button type="button" className="btn-ghost sh-btn" autoFocus onClick={() => setConfirm(false)}>
                Cancel
              </button>
              <button type="button" className="btn-dark" onClick={run}>
                Compress all
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

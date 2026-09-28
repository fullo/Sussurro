import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Card } from "../components/ui";
import type { Ctl } from "../hooks/useAppController";
import { checkNotes, checkSummary, metadataLine, signatureDetails, signatureLine, watermarkLine } from "../lib/tts";
import type { TtsStatus, WatermarkCheck } from "../lib/types";
import { ExperimentalBadge } from "./ReadAloudCard";

/** Models → Voices, *Check a file* (#257, E17): pick an audio file (the
 *  picker opens from Rust — no path crosses IPC), and see what each marking
 *  layer says. Runs the AudioSeal detector on this computer; it is
 *  downloaded with the read-aloud models and never from here. */
export function CheckFileCard({ ctl }: { ctl: Ctl }) {
  const [ready, setReady] = useState<boolean | null>(null);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<WatermarkCheck | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(() => {
    invoke<TtsStatus>("tts_status")
      .then((s) => setReady(!!s.watermark?.detector_downloaded))
      .catch(() => setReady(false));
  }, []);
  useEffect(refresh, [refresh, ctl.settings.tts_voices]);
  // A read-aloud download ending may have brought the detector.
  useEffect(() => {
    const un = listen<unknown>("tts-download-progress", (e) => {
      if (e.payload === null) refresh();
    });
    return () => {
      un.then((f) => f()).catch(() => {});
    };
  }, [refresh]);

  const pick = async () => {
    setBusy(true);
    setError(null);
    try {
      const r = await invoke<WatermarkCheck | null>("watermark_check_file");
      if (r) setResult(r);
    } catch (e) {
      setError(String(e));
      refresh();
    } finally {
      setBusy(false);
    }
  };

  return (
    <Card
      title={
        <>
          Check a file <span className="via">synthetic speech marks</span> <ExperimentalBadge />
        </>
      }
    >
      <p className="card-hint">
        Looks for the marks Sussurro puts on every file it speaks: the inaudible watermark, the file's tags and its
        signed metadata (C2PA — inside the file, or in the <code>.c2pa</code> file with the same name next to it).
        Pick any audio file (Opus, Ogg, WAV, MP3, M4A, FLAC); it is checked on this computer and nothing is uploaded.
      </p>
      <div className="row-gap">
        <button type="button" className="btn-ghost sh-btn" disabled={busy || !ready} onClick={pick}>
          {busy && <span className="btn-spinner" aria-hidden="true" />}
          {busy ? "Checking…" : "Check a file…"}
        </button>
        {ready === false && (
          <span className="sh-muted">
            Needs the watermark detector, which comes with the first read-aloud download above.
          </span>
        )}
      </div>
      {error && (
        <p className="card-hint" role="alert">
          {error}
        </p>
      )}
      {result && !error && (
        <div className="check-result" role="status" aria-live="polite">
          <p>
            <strong>{result.file_name}</strong>{" "}
            <span className="sh-muted">
              · {result.format} · {result.seconds.toFixed(1)} s
            </span>
          </p>
          <p>
            <strong>{checkSummary(result)}</strong>
          </p>
          <ul className="check-layers">
            <li>{watermarkLine(result)}</li>
            <li>
              {metadataLine(result)}
              {result.metadata.tags.length > 0 && (
                <span className="sh-muted">
                  {" "}
                  ({result.metadata.tags.map(([k, v]) => `${k}=${v}`).join(", ")})
                </span>
              )}
            </li>
            <li>
              {signatureLine(result)}
              {signatureDetails(result) && <span className="sh-muted"> ({signatureDetails(result)})</span>}
            </li>
          </ul>
          {checkNotes(result).map((n) => (
            <p key={n} className="card-hint">
              {n}
            </p>
          ))}
        </div>
      )}
    </Card>
  );
}

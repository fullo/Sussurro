import { useCallback, useEffect, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Ctl } from "../hooks/useAppController";
import { audioSrcPath } from "../lib/replay";
import {
  LISTEN_NOTE,
  SYNTHETIC_NOTE,
  TRANSCRIPT_DOC,
  defaultLanguage,
  documentLabel,
  estimateMinutes,
  jobFraction,
  jobIsFor,
  jobLabel,
  readableDocs,
  readiness,
  speechFacts,
  speechFor,
  staleNote,
  type ReadableDoc,
} from "../lib/readAloud";
import type { CompanionDoc, Item, ReadAloudJob, ReadAloudOutcome, SpeechStatus, TtsStatus } from "../lib/types";
import { ExperimentalBadge } from "./ReadAloudCard";

/** The URL scheme of item audio and temporary files (archive::playback). */
const SCHEME = "sussurro-audio";

/* The Audio tab's "Generated speech" section (read aloud, #256, P17/P21/
   P24): the item's speech files — played, marked as synthetic, flagged when
   out of date, deleted to the trash — and, while the experimental module is
   on, Listen (a temporary file, deleted when the document closes) and Save
   (speech.opus next to the item). Nothing is ever downloaded from here: a
   missing model or voice points to Models → Voices. */
export function ReadAloudSection({
  ctl,
  item,
  onChanged,
  onOpenModels,
}: {
  ctl: Ctl;
  item: Item;
  onChanged?: () => void;
  onOpenModels?: () => void;
}) {
  const enabled = !!ctl.settings.tts_enabled;
  const [status, setStatus] = useState<TtsStatus | null>(null);
  const [files, setFiles] = useState<SpeechStatus[]>([]);
  const [docs, setDocs] = useState<ReadableDoc[]>(readableDocs([]));
  const [doc, setDoc] = useState(TRANSCRIPT_DOC);
  const [language, setLanguage] = useState("");
  const [job, setJob] = useState<ReadAloudJob | null>(null);
  const [listenSrc, setListenSrc] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);
  const [error, setError] = useState("");

  const refreshFiles = useCallback(() => {
    invoke<SpeechStatus[]>("read_aloud_files", { id: item.id })
      .then(setFiles)
      .catch(() => setFiles([]));
  }, [item.id]);

  // Reload when the item changes (its text, a new speech file).
  const speechKey = (item.speech ?? []).map((f) => `${f.name}:${f.bytes}`).join(",");
  useEffect(refreshFiles, [refreshFiles, speechKey, item.body]);

  useEffect(() => {
    if (!enabled) {
      setStatus(null);
      return;
    }
    invoke<TtsStatus>("tts_status")
      .then((s) => {
        setStatus(s);
        setLanguage((l) => l || defaultLanguage(s, item.meta.language));
      })
      .catch(() => setStatus(null));
  }, [enabled, item.meta.language, ctl.settings.tts_voices]);

  useEffect(() => {
    invoke<CompanionDoc[]>("recipe_documents", { id: item.id })
      .then((d) => setDocs(readableDocs(d)))
      .catch(() => setDocs(readableDocs([])));
  }, [item.id]);

  // The job, also one started before this pane opened.
  useEffect(() => {
    let alive = true;
    invoke<ReadAloudJob | null>("read_aloud_job")
      .then((j) => alive && setJob(j))
      .catch(() => {});
    const un = listen<ReadAloudJob | null>("read-aloud-progress", (e) => {
      if (!alive) return;
      setJob(e.payload);
      if (e.payload === null) refreshFiles();
    });
    return () => {
      alive = false;
      un.then((f) => f()).catch(() => {});
    };
  }, [refreshFiles]);

  // A temporary Listen file goes when the document closes (P17).
  useEffect(
    () => () => {
      invoke("read_aloud_discard").catch(() => {});
    },
    [item.id],
  );
  useEffect(() => setListenSrc(null), [item.id]);

  const running = job !== null;
  const mine = jobIsFor(job, item.id);
  const ready = readiness(status, language);
  const existing = speechFor(files, doc);
  const docChars = doc === TRANSCRIPT_DOC ? item.body.length : 0;

  const start = async (save: boolean) => {
    setError("");
    if (!save) setListenSrc(null);
    try {
      const out = await invoke<ReadAloudOutcome>("read_aloud_start", {
        id: item.id,
        document: doc,
        language: language || null,
        save,
      });
      if (out.save) {
        ctl.flash(`Speech saved as ${out.file} — ${Math.round(out.seconds)} s.`, 3000);
        refreshFiles();
        onChanged?.();
      } else {
        setListenSrc(convertFileSrc(out.file, SCHEME));
      }
    } catch (e) {
      const msg = String(e);
      if (/cancelled/.test(msg)) ctl.flash("Read aloud cancelled — nothing was kept.", 3000);
      else setError(msg);
    }
  };

  const remove = async (file: string) => {
    setConfirmDelete(null);
    try {
      await invoke("read_aloud_delete", { id: item.id, file });
      ctl.flash(`${file} moved to the trash.`, 3000);
      refreshFiles();
      onChanged?.();
    } catch (e) {
      setError(String(e));
    }
  };

  // Module off and nothing generated: this section doesn't exist (P24).
  if (!enabled && files.length === 0) return null;

  return (
    <section className="ra" aria-label="Generated speech">
      <div className="ra-head">
        <h3>
          Generated speech <ExperimentalBadge />
        </h3>
        <p className="sh-muted ra-note">{SYNTHETIC_NOTE}</p>
      </div>

      {files.length > 0 && (
        <ul className="ra-files" aria-label="Speech files">
          {files.map((f) => {
            const note = staleNote(f);
            return (
              <li key={f.file} className="ra-file">
                <div className="row-gap">
                  <strong>{documentLabel(f.document, docs)}</strong>
                  <span className="ra-synthetic" title={f.marked.length ? `Marked: ${f.marked.join(", ")}` : undefined}>
                    Synthetic
                  </span>
                  <span className="sh-muted mono ra-name">{f.file}</span>
                  <span className="push" />
                  {confirmDelete === f.file ? (
                    <span className="row-gap" role="alertdialog" aria-label={`Delete ${f.file}`}>
                      <span className="sh-muted">Move to the trash?</span>
                      <button type="button" className="btn-dark sh-btn" onClick={() => remove(f.file)}>
                        Delete
                      </button>
                      <button type="button" className="btn-ghost sh-btn" autoFocus onClick={() => setConfirmDelete(null)}>
                        Keep
                      </button>
                    </span>
                  ) : (
                    <button
                      type="button"
                      className="btn-ghost sh-btn"
                      onClick={() => setConfirmDelete(f.file)}
                      disabled={mine && job?.save === true && speechFor(files, job.document)?.file === f.file}
                    >
                      Delete…
                    </button>
                  )}
                </div>
                <p className="sh-muted ra-facts">{speechFacts(f, status?.languages)}</p>
                {note && (
                  <p className={f.stale || f.source_missing ? "ra-stale" : "sh-muted"} role="note">
                    {note}
                  </p>
                )}
                <audio
                  className="ra-audio"
                  controls
                  preload="metadata"
                  src={convertFileSrc(audioSrcPath(item.id, f.file), SCHEME)}
                  aria-label={`Generated speech of ${documentLabel(f.document, docs)} (synthetic voice)`}
                />
              </li>
            );
          })}
        </ul>
      )}

      {!enabled ? (
        <p className="sh-muted ra-off">
          Read aloud is off, so no new speech can be made. Turn it on in Settings → Experimental.
        </p>
      ) : item.recording ? (
        <p className="sh-muted">Read aloud opens when the session ends.</p>
      ) : (
        <div className="ra-make">
          <div className="row-gap ra-pick">
            <label className="au-opt">
              <span>Read</span>
              <select value={doc} onChange={(e) => setDoc(e.target.value)} disabled={running}>
                {docs.map((d) => (
                  <option key={d.file} value={d.file}>
                    {d.label}
                  </option>
                ))}
              </select>
            </label>
            <label className="au-opt">
              <span>Language</span>
              <select value={language} onChange={(e) => setLanguage(e.target.value)} disabled={running || !status}>
                {(status?.languages ?? []).map((l) => (
                  <option key={l.code} value={l.code}>
                    {l.label}
                  </option>
                ))}
              </select>
            </label>
            {ready.state === "ready" && <span className="sh-muted">Voice: {ready.voice}</span>}
          </div>

          {ready.state === "missing" && (
            <p className="ra-missing" role="note">
              Download {ready.what} first — nothing is downloaded from here.{" "}
              {onOpenModels && (
                <button type="button" className="link-btn" onClick={onOpenModels}>
                  Open Models → Voices
                </button>
              )}
            </p>
          )}
          {ready.state === "no-language" && (
            <p className="ra-missing" role="note">Pick a language read aloud has a voice for.</p>
          )}

          {mine && job ? (
            <div className="row-gap tts-progress" role="status" aria-live="polite">
              <div
                className="progress"
                role="progressbar"
                aria-label="Reading aloud"
                aria-valuemin={0}
                aria-valuemax={100}
                aria-valuenow={Math.round(jobFraction(job) * 100)}
              >
                <div className="progress-fill" style={{ width: `${jobFraction(job) * 100}%` }} />
              </div>
              <span className="sh-muted">{jobLabel(job)}</span>
              <button type="button" className="btn-ghost sh-btn" onClick={() => invoke("read_aloud_cancel").catch(() => {})}>
                Cancel
              </button>
            </div>
          ) : (
            <div className="row-gap">
              <button
                type="button"
                className="btn-ghost sh-btn"
                disabled={running || ready.state !== "ready"}
                onClick={() => start(false)}
                title="Make the speech in a temporary file and play it — nothing is saved"
              >
                Listen
              </button>
              <button
                type="button"
                className="btn-dark sh-btn"
                disabled={running || ready.state !== "ready"}
                onClick={() => start(true)}
                title="Save the speech next to this item, as an Opus file marked as synthetic"
              >
                {existing ? (existing.stale ? "Make it again" : "Replace speech file") : "Save as speech file"}
              </button>
              {running && !mine && <span className="sh-muted">Another document is being read aloud.</span>}
              {!running && docChars > 4000 && (
                <span className="sh-muted">This takes about {estimateMinutes(docChars, language)} min on this computer.</span>
              )}
            </div>
          )}

          {listenSrc && !running && (
            <div className="ra-listen">
              <audio className="ra-audio" controls autoPlay src={listenSrc} aria-label="Temporary read-aloud (synthetic voice)" />
              <p className="sh-muted">{LISTEN_NOTE}</p>
            </div>
          )}
        </div>
      )}

      {error && (
        <p className="au-error" role="alert">
          {error}
        </p>
      )}
    </section>
  );
}

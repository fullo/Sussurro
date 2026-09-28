import { useCallback, useEffect, useRef, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Ctl } from "../hooks/useAppController";
import { audioSrcPath, formatPlayerTime } from "../lib/replay";
import {
  SYNTHETIC_NOTE,
  TRANSCRIPT_DOC,
  createLabel,
  defaultLanguage,
  documentLabel,
  estimateMinutes,
  jobFraction,
  jobIsFor,
  jobLabel,
  listenDisabled,
  readableDocs,
  readiness,
  speechFacts,
  speechFor,
  speechSignatureNote,
  staleNote,
  type ReadableDoc,
} from "../lib/readAloud";
import type { CompanionDoc, Item, ReadAloudJob, ReadAloudOutcome, SpeechStatus, TtsStatus } from "../lib/types";
import { ExperimentalBadge } from "./ReadAloudCard";

/** The URL scheme of item audio and generated speech (archive::playback). */
const SCHEME = "sussurro-audio";

/* The Audio tab's "Generated speech" section (read aloud, #256, #327,
   P17/P21/P24): one button per document — Create when there is no speech
   for it yet, Listen (playing the saved file at once, no generation) once
   there is; Create again next to Listen when it's stale. One shared
   in-app player for the section (no native <audio controls> anywhere
   here, #327): play/pause, a seek bar with time, closed when the item
   changes or the document pane unmounts. Nothing is ever downloaded from
   here: a missing model or voice points to Models → Voices. */
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
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);
  const [error, setError] = useState("");

  // The one in-app player (#327): which file plays, and its transport.
  const [playing, setPlaying] = useState<{ file: string; label: string } | null>(null);
  const [isPlaying, setIsPlaying] = useState(false);
  const [curMs, setCurMs] = useState(0);
  const [durMs, setDurMs] = useState(0);
  const audioRef = useRef<HTMLAudioElement>(null);

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

  // The player stops when another item opens (or this pane unmounts, which
  // removes the <audio> element on its own).
  useEffect(() => {
    setPlaying(null);
  }, [item.id]);

  const running = job !== null;
  const mine = jobIsFor(job, item.id);
  const ready = readiness(status, language);
  const existing = speechFor(files, doc);
  const docChars = doc === TRANSCRIPT_DOC ? item.body.length : 0;
  const create = enabled ? createLabel(existing) : null;

  const play = (file: string, label: string) => {
    if (playing?.file === file) {
      const el = audioRef.current;
      if (el) {
        if (el.paused) el.play().catch(() => {});
        else el.pause();
      }
      return;
    }
    setCurMs(0);
    setDurMs(0);
    setPlaying({ file, label });
  };

  const start = async () => {
    setError("");
    // Create again replaces the file the player may be streaming.
    if (existing && playing?.file === existing.file) setPlaying(null);
    try {
      const out = await invoke<ReadAloudOutcome>("read_aloud_start", {
        id: item.id,
        document: doc,
        language: language || null,
      });
      ctl.flash(`Speech saved as ${out.file} — ${Math.round(out.seconds)} s.`, 3000);
      refreshFiles();
      onChanged?.();
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
      if (playing?.file === file) setPlaying(null);
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
            const disabled = listenDisabled(job, item.id, f.document);
            const nowPlaying = playing?.file === f.file;
            return (
              <li key={f.file} className="ra-file">
                <div className="row-gap">
                  <strong>{documentLabel(f.document, docs)}</strong>
                  <span className="ra-synthetic" title={f.marked.length ? `Marked: ${f.marked.join(", ")}` : undefined}>
                    Synthetic
                  </span>
                  <span className="sh-muted mono ra-name">{f.file}</span>
                  <button
                    type="button"
                    className="btn-ghost sh-btn"
                    disabled={disabled}
                    onClick={() => play(f.file, documentLabel(f.document, docs))}
                    title={disabled ? "This file is being replaced" : "Play the saved speech"}
                  >
                    {nowPlaying && isPlaying ? "Pause" : "Listen"}
                  </button>
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
                    <button type="button" className="btn-ghost sh-btn" onClick={() => setConfirmDelete(f.file)} disabled={disabled}>
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
                {speechSignatureNote(f) && (
                  <p className="sh-muted" role="note">
                    {speechSignatureNote(f)}
                  </p>
                )}
              </li>
            );
          })}
        </ul>
      )}

      {playing && (
        <div className="ra-player row-gap" role="group" aria-label={`Playing ${playing.label}`}>
          <audio
            key={playing.file}
            ref={audioRef}
            autoPlay
            src={convertFileSrc(audioSrcPath(item.id, playing.file), SCHEME)}
            onPlay={() => setIsPlaying(true)}
            onPause={() => setIsPlaying(false)}
            onEnded={() => setIsPlaying(false)}
            onLoadedMetadata={(e) => {
              const d = e.currentTarget.duration;
              if (Number.isFinite(d)) setDurMs(Math.round(d * 1000));
            }}
            onTimeUpdate={(e) => setCurMs(Math.round(e.currentTarget.currentTime * 1000))}
            onError={() => setError(`${playing.file} could not be played.`)}
            style={{ display: "none" }}
          />
          <button
            type="button"
            className="btn-dark au-play"
            onClick={() => (isPlaying ? audioRef.current?.pause() : audioRef.current?.play().catch(() => {}))}
            aria-label={isPlaying ? "Pause" : "Play"}
          >
            <span aria-hidden="true">{isPlaying ? "❚❚" : "▶"}</span>
          </button>
          <span className="ra-player-label sh-muted">{playing.label}</span>
          <div className="au-seek">
            <span className="au-time mono" aria-hidden="true">{formatPlayerTime(curMs)}</span>
            <input
              type="range"
              min={0}
              max={Math.max(1, durMs)}
              step={100}
              value={Math.min(curMs, durMs)}
              aria-label="Position"
              aria-valuetext={`${formatPlayerTime(curMs)} of ${formatPlayerTime(durMs)}`}
              onChange={(e) => {
                const t = Number(e.target.value);
                if (audioRef.current) audioRef.current.currentTime = t / 1000;
                setCurMs(t);
              }}
            />
            <span className="au-time mono" aria-hidden="true">{formatPlayerTime(durMs)}</span>
          </div>
          <button type="button" className="btn-ghost sh-btn" onClick={() => setPlaying(null)} aria-label="Close player">
            ✕
          </button>
        </div>
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
              {existing && (
                <button
                  type="button"
                  className="btn-dark sh-btn"
                  disabled={listenDisabled(job, item.id, doc)}
                  onClick={() => play(existing.file, documentLabel(doc, docs))}
                >
                  {playing?.file === existing.file && isPlaying ? "Pause" : "Listen"}
                </button>
              )}
              {create && (
                <button
                  type="button"
                  className={existing ? "btn-ghost sh-btn" : "btn-dark sh-btn"}
                  disabled={running || ready.state !== "ready"}
                  onClick={start}
                >
                  {create}
                </button>
              )}
              {running && !mine && <span className="sh-muted">Another document is being read aloud.</span>}
              {!running && !existing && docChars > 4000 && (
                <span className="sh-muted">This takes about {estimateMinutes(docChars, language)} min on this computer.</span>
              )}
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

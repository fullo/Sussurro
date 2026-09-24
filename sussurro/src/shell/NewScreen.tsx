import { useEffect, useState } from "react";
import { TranscriptView, toLines } from "@sussurro/transcript";
import type { Ctl } from "../hooks/useAppController";
import type { EngineRuns, RunArgs } from "../hooks/useEngineRuns";
import { describeProgress, isRunning, wasCancelled, type Run, type RunKind } from "../lib/engineRuns";
import { baseName, formatClock, progressPercent } from "../lib/format";
import { TYPE_LABEL } from "../lib/library";
import type { ItemType } from "../lib/types";
import { CleanupLevelPicker } from "../settings/CleanupCard";
import { AUDIO_EXTENSIONS, pickAudioFile } from "../settings/AudioFileCard";
import { LANGUAGES } from "../lib/constants";
import { differsFromDictation, effectiveRun, runArgs, type RunChoice } from "../lib/runOptions";
import { ChipEditor } from "./ChipEditor";
import { sttLabel } from "./labels";

/** New's options, shared by every source. Language and cleanup level are
 *  per run (#157): unset = the dictation settings, never saved to them. */
export interface NewDefaults extends RunChoice {
  tags: string[];
  categories: string[];
}

type Tab = "mic" | "file";

/** New: every source becomes an item in the Library. 0.7 has Microphone and
 *  File; Link, Meeting and System audio arrive in later releases. */
export function NewScreen({
  ctl,
  engine,
  defaults,
  onDefaultsChange,
  onRunStart,
  onOpenItem,
}: {
  ctl: Ctl;
  engine: EngineRuns;
  defaults: NewDefaults;
  onDefaultsChange: (d: NewDefaults) => void;
  onRunStart: (kind: RunKind) => void;
  onOpenItem: (id: string) => void;
}) {
  const [tab, setTab] = useState<Tab>(() => (isRunning(engine.runs.file) && !isRunning(engine.runs.mic) ? "file" : "mic"));
  const options = runArgs(ctl.settings, defaults);

  return (
    <div className="sh-screen">
      <header className="sh-topbar">
        <h1>New</h1>
        <span className="sh-muted">Every source becomes an item in your Library</span>
      </header>
      <div className="sh-scroll new-body">
        <div className="new-tabs" role="tablist" aria-label="Source">
          {(["mic", "file"] as const).map((t) => (
            <button
              key={t}
              type="button"
              role="tab"
              id={`new-tab-${t}`}
              aria-selected={tab === t}
              aria-controls={`new-panel-${t}`}
              className={`new-tab${tab === t ? " active" : ""}`}
              onClick={() => setTab(t)}
            >
              {t === "mic" ? "Microphone" : "File"}
              {isRunning(engine.runs[t]) && <span className={t === "mic" ? "sh-rec-dot" : "sh-busy-dot"} aria-label="running" />}
            </button>
          ))}
        </div>
        <div className="new-grid">
          <section id={`new-panel-${tab}`} role="tabpanel" aria-labelledby={`new-tab-${tab}`} className="new-source">
            {tab === "mic" ? (
              <MicPanel ctl={ctl} engine={engine} options={options} onRunStart={onRunStart} onOpenItem={onOpenItem} />
            ) : (
              <FilePanel ctl={ctl} engine={engine} options={options} onRunStart={onRunStart} onOpenItem={onOpenItem} />
            )}
          </section>
          <OptionsCard ctl={ctl} defaults={defaults} onChange={onDefaultsChange} />
        </div>
      </div>
    </div>
  );
}

/* ---------- options shared by every source ---------- */

function OptionsCard({
  ctl,
  defaults,
  onChange,
}: {
  ctl: Ctl;
  defaults: NewDefaults;
  onChange: (d: NewDefaults) => void;
}) {
  const { settings } = ctl;
  const run = effectiveRun(settings, defaults);
  return (
    <section className="card new-options" aria-labelledby="new-options-title">
      <h2 id="new-options-title" className="sh-sect">Options — same for every source</h2>
      <div className="opt-grid">
        <label className="opt-k" htmlFor="new-lang">Language</label>
        <select
          id="new-lang"
          value={run.language}
          disabled={settings.engine !== "whisper"}
          onChange={(e) => onChange({ ...defaults, language: e.target.value })}
        >
          {LANGUAGES.map(([code, label]) => (
            <option key={code} value={code}>{label}</option>
          ))}
        </select>

        <span className="opt-k" id="new-cleanup">Cleanup level</span>
        <CleanupLevelPicker
          value={run.cleanupLevel}
          onChange={(cleanupLevel) => onChange({ ...defaults, cleanupLevel })}
          label="Cleanup level"
        />

        <span className="opt-k">Default tags</span>
        <ChipEditor
          label="Default tags"
          values={defaults.tags}
          addLabel="+ tag"
          onChange={(tags) => onChange({ ...defaults, tags })}
        />

        <span className="opt-k">Category</span>
        <ChipEditor
          label="Default categories"
          values={defaults.categories}
          addLabel="+ category"
          onChange={(categories) => onChange({ ...defaults, categories })}
        />
      </div>
      <p className="sh-note">
        Language and cleanup level apply to these runs only and start from your dictation settings
        {settings.engine !== "whisper" && " (Parakeet detects the language itself)"}.
        {differsFromDictation(settings, defaults) && (
          <>
            {" "}
            <button
              type="button"
              className="link-btn"
              onClick={() => onChange({ ...defaults, language: null, cleanupLevel: null })}
            >
              Use the dictation settings
            </button>
          </>
        )}{" "}
        Tags and category are added to the item when it is saved.
      </p>
      <p className="sh-note">Engine: {sttLabel(settings)}</p>
    </section>
  );
}

/* ---------- run status (shared by mic and file) ---------- */

function RunOutcome({
  run,
  onOpenItem,
  onDismiss,
  againLabel,
}: {
  run: Run;
  onOpenItem: (id: string) => void;
  onDismiss: () => void;
  againLabel: string;
}) {
  if (run.status === "done" && run.result) {
    const r = run.result;
    return (
      <div className="run-outcome ok" role="status">
        <p>
          Saved as a <strong>{TYPE_LABEL[r.item_type] ?? r.item_type}</strong>: “{r.title}”
          {r.segments > 0 && <span className="sh-muted"> · {r.segments} lines · {formatClock(r.duration_s)}</span>}
        </p>
        <div className="row-gap">
          <button type="button" className="btn-dark" onClick={() => onOpenItem(r.item_id)}>
            Open in Library
          </button>
          <button type="button" className="btn-ghost sh-btn" onClick={onDismiss}>
            {againLabel}
          </button>
        </div>
      </div>
    );
  }
  if (run.status === "error") {
    // A failed run may keep what it transcribed as an interrupted item (#153).
    const kept = !wasCancelled(run) && run.itemId;
    return (
      <div className="run-outcome err" role="alert">
        <p>
          {wasCancelled(run) ? "Cancelled — nothing was saved." : run.error}
          {kept && " What was transcribed until then is kept in the Library, marked interrupted."}
        </p>
        <div className="row-gap">
          {kept && (
            <button type="button" className="btn-dark" onClick={() => onOpenItem(kept)}>
              Open in Library
            </button>
          )}
          <button type="button" className="btn-ghost sh-btn" onClick={onDismiss}>
            {againLabel}
          </button>
        </div>
      </div>
    );
  }
  return null;
}

/* ---------- Microphone ---------- */

function MicPanel({
  ctl,
  engine,
  options,
  onRunStart,
  onOpenItem,
}: {
  ctl: Ctl;
  engine: EngineRuns;
  options: RunArgs;
  onRunStart: (kind: RunKind) => void;
  onOpenItem: (id: string) => void;
}) {
  const run = engine.runs.mic;
  const [title, setTitle] = useState("");
  const [now, setNow] = useState(() => Date.now());
  const live = isRunning(run);
  useEffect(() => {
    if (!live) return;
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, [live]);

  if (!run) {
    return (
      <div className="stack">
        <div className="mic-start">
          <div>
            <h2 className="sh-h2">Record a note</h2>
            <p className="sh-muted">
              A long microphone session, separate from the dictation hotkey. Sussurro transcribes while you
              speak and saves a <strong>Note</strong> in your Library when you stop.
            </p>
          </div>
          <label className="field-stack">
            <span className="opt-k">Title <span className="sh-muted">(optional)</span></span>
            <input
              value={title}
              onChange={(e) => setTitle(e.target.value)}
              placeholder="From the first words if empty"
              spellCheck={false}
            />
          </label>
          <div className="row-gap">
            <span className="tb note" title="Microphone sessions are your own voice">Note</span>
            <button
              type="button"
              className="btn-rec"
              disabled={!engine.canStartMic}
              title={engine.canStartMic ? "Start recording" : "Wait for the file to start"}
              onClick={async () => {
                onRunStart("mic");
                const err = await engine.startMic(title, "note", options);
                if (err) ctl.setBusy(err);
                else setTitle("");
              }}
            >
              <span className="rec-dot" aria-hidden="true" /> {engine.micStarting ? "Starting…" : "Start recording"}
            </button>
          </div>
        </div>
      </div>
    );
  }

  const elapsed = live ? (now - run.startedAt) / 1000 : run.result?.duration_s ?? 0;
  const lines = toLines(run.segments);
  return (
    <div className="stack">
      {live && (
        <div className={`mic-live${run.status === "running" ? " rec" : ""}`}>
          <span className={`pill-rec${run.status === "running" ? "" : " off"}`}>
            {run.status === "running" ? `● Recording ${formatClock(elapsed)}` : "Stopped"}
          </span>
          <span className="sh-muted" aria-live="polite">{describeProgress(run)}</span>
          <span className="row-gap push">
            {run.status === "running" && (
              <button type="button" className="btn-stop" onClick={async () => {
                const err = await engine.stopMic();
                if (err) ctl.setBusy(err);
              }}>
                ■ Stop
              </button>
            )}
            <button
              type="button"
              className="btn-ghost sh-btn"
              disabled={run.sessionId === null}
              title="Stop and discard: nothing is saved"
              onClick={() => engine.cancel("mic")}
            >
              Discard
            </button>
          </span>
        </div>
      )}
      <RunOutcome run={run} onOpenItem={onOpenItem} onDismiss={() => engine.dismiss("mic")} againLabel="New recording" />
      <div className="live-box tx-scroll" aria-label="Live transcript">
        <TranscriptView
          lines={lines}
          follow
          label="Live transcript"
          emptyText={live ? "Speak — lines appear here as they are transcribed." : "No speech was transcribed."}
        />
      </div>
    </div>
  );
}

/* ---------- File ---------- */

function FilePanel({
  ctl,
  engine,
  options,
  onRunStart,
  onOpenItem,
}: {
  ctl: Ctl;
  engine: EngineRuns;
  options: RunArgs;
  onRunStart: (kind: RunKind) => void;
  onOpenItem: (id: string) => void;
}) {
  const run = engine.runs.file;
  const [path, setPath] = useState<string | null>(null);
  const [itemType, setItemType] = useState<Exclude<ItemType, "meeting">>("note");
  const [title, setTitle] = useState("");
  const [dragOver, setDragOver] = useState(false);
  const setBusy = ctl.setBusy;
  const live = isRunning(run);

  // Files dropped on the window (Tauri reports paths, not File objects).
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    import("@tauri-apps/api/webview")
      .then(({ getCurrentWebview }) =>
        getCurrentWebview().onDragDropEvent((e) => {
          const p = e.payload;
          if (p.type === "over" || p.type === "enter") setDragOver(true);
          else if (p.type === "leave") setDragOver(false);
          else if (p.type === "drop") {
            setDragOver(false);
            const audio = p.paths.find((f) => AUDIO_EXTENSIONS.includes(f.split(".").pop()?.toLowerCase() ?? ""));
            if (audio) setPath(audio);
            else if (p.paths.length) setBusy(`Not an audio file: ${baseName(p.paths[0])}`);
          }
        }),
      )
      .then((f) => {
        if (cancelled) f();
        else unlisten = f;
      })
      .catch(() => {
        /* no drag-and-drop outside Tauri (dev preview) */
      });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [setBusy]);

  if (run && !live) {
    return (
      <div className="stack">
        <RunOutcome
          run={run}
          onOpenItem={onOpenItem}
          onDismiss={() => {
            engine.dismiss("file");
            setPath(null);
            setTitle("");
          }}
          againLabel="Transcribe another file"
        />
        {run.segments.length > 0 && (
          <div className="live-box tx-scroll">
            <TranscriptView lines={toLines(run.segments)} label="Transcript" />
          </div>
        )}
      </div>
    );
  }

  if (live) {
    const p = run.progress;
    const pct = p ? progressPercent(p.processed_s, p.total_s) : 0;
    return (
      <div className="stack">
        <div className="file-row">
          <span aria-hidden="true">♪</span>
          <span className="file-name">{run.label}</span>
          {p && <span className="mono sh-muted">{formatClock(p.processed_s)} / {formatClock(p.total_s)}</span>}
        </div>
        <div className="progress" role="progressbar" aria-label={`Transcribing ${run.label}`} aria-valuemin={0} aria-valuemax={100} aria-valuenow={pct}>
          <div className="progress-fill" style={{ width: `${pct}%` }} />
        </div>
        <div className="row-gap">
          <span className="sh-muted" aria-live="polite">
            {pct}% · {describeProgress(run)}
            {p && p.segments_done > 0 && ` · ${p.segments_done} lines`}
          </span>
          <button
            type="button"
            className="btn-ghost sh-btn push"
            disabled={run.sessionId === null}
            title={run.sessionId === null ? "Starting…" : "Stop and discard: nothing is saved"}
            onClick={() => engine.cancel("file")}
          >
            Cancel
          </button>
        </div>
        <div className="live-box tx-scroll">
          <TranscriptView lines={toLines(run.segments)} follow label="Transcript so far" emptyText="The first lines appear after the first pause…" />
        </div>
      </div>
    );
  }

  return (
    <div className="stack">
      <div className={`drop${dragOver ? " over" : ""}`}>
        <strong>Transcribe an audio file</strong>
        <span className="sh-muted">wav, mp3, m4a, aac, flac, ogg · drop it here or</span>
        <button
          type="button"
          className="btn-ghost sh-btn"
          onClick={async () => {
            const p = await pickAudioFile();
            if (p) setPath(p);
          }}
        >
          Choose a file…
        </button>
        {path && (
          <div className="file-row">
            <span aria-hidden="true">♪</span>
            <span className="file-name" title={path}>{baseName(path)}</span>
            <button type="button" className="icon-btn" aria-label="Remove the chosen file" onClick={() => setPath(null)}>×</button>
          </div>
        )}
      </div>

      <fieldset className="types">
        <legend className="sh-sect">Item type</legend>
        {([
          ["note", "Your own voice: a voice memo, a dictated idea.", true],
          ["transcription", "Audio recorded by others: podcast, interview, lecture. Shown with timestamps.", false],
        ] as const).map(([value, hint, isDefault]) => (
          <label key={value} className={`typecard${itemType === value ? " sel" : ""}`}>
            <input
              type="radio"
              name="item-type"
              value={value}
              checked={itemType === value}
              onChange={() => setItemType(value)}
            />
            <span className="typecard-body">
              <b>
                {TYPE_LABEL[value]}
                {isDefault && <span className="sh-muted"> default</span>}
              </b>
              <span className="sh-muted">{hint}</span>
            </span>
          </label>
        ))}
      </fieldset>

      <label className="field-stack">
        <span className="opt-k">Title <span className="sh-muted">(optional — the file name if empty)</span></span>
        <input value={title} onChange={(e) => setTitle(e.target.value)} spellCheck={false} />
      </label>

      <div className="row-gap end">
        <button
          type="button"
          className="btn-dark"
          disabled={!path || !engine.canStartFile}
          onClick={async () => {
            if (!path) return;
            onRunStart("file");
            await engine.startFile(path, itemType, title, options);
          }}
        >
          Transcribe
        </button>
      </div>
    </div>
  );
}

import { useEffect, useState } from "react";
import { detectsLanguage, engineLabel } from "../lib/engines";
import { invoke } from "@tauri-apps/api/core";
import { TranscriptView, toLines } from "@sussurro/transcript";
import type { Ctl } from "../hooks/useAppController";
import type { EngineRuns, RunArgs } from "../hooks/useEngineRuns";
import {
  describeProgress,
  discardStep,
  isRunning,
  showDiscard,
  wasCancelled,
  type DiscardAction,
  type DiscardStep,
  type Run,
  type RunKind,
} from "../lib/engineRuns";
import { baseName, formatClock, progressPercent } from "../lib/format";
import { TYPE_LABEL } from "../lib/library";
import type { ItemType, LinkInfo, SystemAudioDevices, YtDlpStatus } from "../lib/types";
import {
  ECHO_NOTE,
  NATIVE,
  SETUP_HELP,
  choiceStillValid,
  deviceLabel,
  devicesProblem,
  initialSystemDevice,
  loadSystemDevice,
  nativeAvailable,
  nativeFallbackNote,
  nativeNote,
  nativeOptionLabel,
  osOf,
  saveSystemDevice,
  systemTabVisible,
} from "../lib/systemAudio";
import {
  canTranscribeLink,
  describeDownload,
  downloadPercent,
  kindLabel,
  linkPhase,
  linkProblem,
  looksLikeLink,
  viaLabel,
} from "../lib/links";
import { CleanupLevelPicker } from "../settings/CleanupCard";
import { AUDIO_EXTENSIONS, pickAudioFile } from "./audioFile";
import { LANGUAGES } from "../lib/constants";
import { differsFromDictation, effectiveRun, runArgs, saveAudioChoice, type RunChoice } from "../lib/runOptions";
import { identifyVoicesArg } from "../lib/speakers";
import { recordsOthers } from "../lib/meetingNotice";
import { ChipEditor } from "./ChipEditor";
import { RecordingReminder, useRecordingNotice } from "./RecordingNotice";
import { sttLabel } from "./labels";

/** New's options, shared by every source. Language and cleanup level are
 *  per run (#157): unset = the dictation settings, never saved to them. */
export interface NewDefaults extends RunChoice {
  tags: string[];
  categories: string[];
}

type Tab = "mic" | "file" | "link" | "system";

const TAB_LABEL: Record<Tab, string> = { mic: "Microphone", file: "File", link: "Link", system: "System audio + mic" };

/** The tab New opens on: a running session, file or link, else the
 *  microphone. */
function initialTab(runs: EngineRuns["runs"]): Tab {
  if (isRunning(runs.mic)) return "mic";
  if (isRunning(runs.system)) return "system";
  if (isRunning(runs.file)) return "file";
  if (isRunning(runs.link)) return "link";
  return "mic";
}

/** New: every source becomes an item in the Library. Microphone and File
 *  (0.7), Link (0.8, #123), System audio + mic (0.10, #139 — behind the
 *  meetings preview, since it records other people). */
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
  const [tab, setTab] = useState<Tab>(() => initialTab(engine.runs));
  const options: RunArgs = { ...runArgs(ctl.settings, defaults), saveAudio: saveAudioChoice(ctl.settings, defaults) };
  const showSystem = systemTabVisible(!!ctl.settings.meetings_enabled, isRunning(engine.runs.system));
  const tabs: Tab[] = showSystem ? ["mic", "file", "link", "system"] : ["mic", "file", "link"];
  // The preview was switched off while this tab was open.
  const shown: Tab = tabs.includes(tab) ? tab : "mic";

  return (
    <div className="sh-screen">
      <header className="sh-topbar">
        <h1>New</h1>
        <span className="sh-muted">Every source becomes an item in your Library</span>
      </header>
      <div className="sh-scroll new-body">
        <div className="new-tabs" role="tablist" aria-label="Source">
          {tabs.map((t) => (
            <button
              key={t}
              type="button"
              role="tab"
              id={`new-tab-${t}`}
              aria-selected={shown === t}
              aria-controls={`new-panel-${t}`}
              className={`new-tab${shown === t ? " active" : ""}`}
              onClick={() => setTab(t)}
            >
              {TAB_LABEL[t]}
              {isRunning(engine.runs[t]) && (
                <span className={t === "mic" || t === "system" ? "sh-rec-dot" : "sh-busy-dot"} aria-label="running" />
              )}
            </button>
          ))}
        </div>
        <div className="new-grid">
          <section id={`new-panel-${shown}`} role="tabpanel" aria-labelledby={`new-tab-${shown}`} className="new-source">
            {shown === "mic" && (
              <MicPanel ctl={ctl} engine={engine} options={options} onRunStart={onRunStart} onOpenItem={onOpenItem} />
            )}
            {shown === "file" && (
              <FilePanel ctl={ctl} engine={engine} options={options} onRunStart={onRunStart} onOpenItem={onOpenItem} />
            )}
            {shown === "link" && <LinkPanel engine={engine} options={options} onRunStart={onRunStart} onOpenItem={onOpenItem} />}
            {shown === "system" && (
              <SystemPanel ctl={ctl} engine={engine} options={options} onRunStart={onRunStart} onOpenItem={onOpenItem} />
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
        {detectsLanguage(settings.engine) && ` (${engineLabel(settings.engine)} detects the language itself)`}.
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
      <SaveAudio
        checked={saveAudioChoice(settings, defaults)}
        onChange={(saveAudio) => onChange({ ...defaults, saveAudio })}
      />
      <p className="sh-note">Engine: {sttLabel(settings)}</p>
    </section>
  );
}

/* ---------- "Save audio" (P9, #141) ---------- */

/** Off unless the user asks (or turned the default on in Settings →
 *  Archive). The note names the folder: Documents is often synced to
 *  iCloud Drive or OneDrive, and the audio would go with it. */
function SaveAudio({ checked, onChange }: { checked: boolean; onChange: (on: boolean) => void }) {
  const [dir, setDir] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    invoke<string>("archive_dir")
      .then((d) => alive && setDir(d))
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, []);
  return (
    <div className="save-audio">
      <label className="check-row">
        <input type="checkbox" checked={checked} onChange={(e) => onChange(e.target.checked)} />
        <span>
          Save audio <span className="sh-muted">— keep the recording next to the transcript</span>
        </span>
      </label>
      <p className="sh-note" role="note">
        {checked ? "Saved" : "When ticked, saved"} as <code>audio.wav</code> (one file per channel for a meeting) in
        the item's folder
        {dir ? (
          <>
            {" "}under <code>{dir}</code>
          </>
        ) : (
          " in the archive"
        )}
        . If that folder syncs to iCloud Drive, OneDrive or another cloud, the audio is uploaded too. About 115 MB per
        hour; you can delete it later from the document and keep the transcript.
      </p>
    </div>
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
          {wasCancelled(run)
            ? run.kind === "link" && run.itemId === null
              ? "Cancelled — the download was deleted and nothing was saved."
              : "Discarded — nothing was saved to the Library (anything transcribed is in the trash)."
            : run.error}
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
  /** 0.9 preview (#130): an in-room meeting, one mic for everyone —
   *  saved as a Meeting with voices labelled "Voice 1, Voice 2…". */
  const [inRoom, setInRoom] = useState(false);
  const meetingOn = !!ctl.settings.meetings_enabled && inRoom;
  const { confirmNotice, dialog } = useRecordingNotice(ctl);

  if (!run) {
    return (
      <div className="stack">
        <div className="mic-start">
          <div>
            <h2 className="sh-h2">{meetingOn ? "Record a meeting in the room" : "Record a note"}</h2>
            <p className="sh-muted">
              A long microphone session, separate from the dictation hotkey. Sussurro transcribes while you
              speak and saves a <strong>{meetingOn ? "Meeting" : "Note"}</strong> in your Library when you stop.
              {meetingOn && " Voices are told apart as Voice 1, Voice 2… — rename them in the document."}
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
          {ctl.settings.meetings_enabled && (
            <label className="check-row">
              <input type="checkbox" checked={inRoom} onChange={(e) => setInRoom(e.target.checked)} />
              <span>
                Meeting in the room <span className="sh-muted">— several people on this mic, labelled by voice (preview)</span>
              </span>
            </label>
          )}
          <div className="row-gap">
            {meetingOn ? (
              <span className="tb meeting" title="Several people on one microphone">Meeting</span>
            ) : (
              <span className="tb note" title="Microphone sessions are your own voice">Note</span>
            )}
            <button
              type="button"
              className="btn-rec"
              disabled={!engine.canStartMic}
              title={
                engine.canStartMic
                  ? "Start recording"
                  : isRunning(engine.runs.system)
                    ? "A System audio + mic session is using the microphone"
                    : "Wait for the file to start"
              }
              onClick={async () => {
                // A meeting in the room records other people (#136).
                if (meetingOn && !(await confirmNotice())) return;
                onRunStart("mic");
                const err = await engine.startMic(title, meetingOn ? "meeting" : "note", options);
                if (err) ctl.setBusy(err);
                else setTitle("");
              }}
            >
              <span className="rec-dot" aria-hidden="true" /> {engine.micStarting ? "Starting…" : "Start recording"}
            </button>
          </div>
          {meetingOn && <RecordingReminder />}
        </div>
        {dialog}
      </div>
    );
  }

  return (
    <CaptureLive
      run={run}
      onStop={engine.stopMic}
      onCancel={() => engine.cancel("mic")}
      onDismiss={() => engine.dismiss("mic")}
      onOpenItem={onOpenItem}
      setBusy={ctl.setBusy}
      againLabel="New recording"
    />
  );
}

/* ---------- a live capture session (mic, system audio + mic) ---------- */

/** A running (or finished) capture session: clock, Stop, Discard with a
 *  confirmation (#158), the warnings the engine sent (#139), the outcome
 *  and the live transcript. */
function CaptureLive({
  run,
  onStop,
  onCancel,
  onDismiss,
  onOpenItem,
  setBusy,
  againLabel,
}: {
  run: Run;
  onStop: () => Promise<string | null>;
  onCancel: () => void;
  onDismiss: () => void;
  onOpenItem: (id: string) => void;
  setBusy: (msg: string) => void;
  againLabel: string;
}) {
  const [now, setNow] = useState(() => Date.now());
  const live = isRunning(run);
  const [discard, setDiscard] = useState<DiscardStep>("idle");
  const confirming = discard === "confirming" && showDiscard(run);
  const sessionId = run.sessionId;
  // A new session starts with the question closed.
  useEffect(() => setDiscard("idle"), [sessionId]);
  const act = (action: DiscardAction) => {
    const next = discardStep(discard, action);
    setDiscard(next.step);
    if (next.cancel) onCancel();
  };
  useEffect(() => {
    if (!live) return;
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, [live]);

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
            {run.status === "running" && !confirming && (
              <button type="button" className="btn-stop" onClick={async () => {
                const err = await onStop();
                if (err) setBusy(err);
              }}>
                ■ Stop
              </button>
            )}
            {/* Discard only while recording, and only after a confirmation
                (#158): the item already holds everything said so far. */}
            {showDiscard(run) &&
              (confirming ? (
                <span className="row-gap" role="group" aria-label="Discard this recording?">
                  <span className="sh-muted">Discard this recording? What was transcribed goes to the trash.</span>
                  <button type="button" className="btn-ghost sh-btn" autoFocus onClick={() => act("keep")}>
                    Keep recording
                  </button>
                  <button type="button" className="btn-stop" onClick={() => act("confirm")}>
                    Discard
                  </button>
                </span>
              ) : (
                <button
                  type="button"
                  className="btn-ghost sh-btn"
                  disabled={run.sessionId === null}
                  title="Stop and throw this recording away (asks first)"
                  onClick={() => act("ask")}
                >
                  Discard…
                </button>
              ))}
          </span>
        </div>
      )}
      {live && recordsOthers(run) && <RecordingReminder />}
      {run.warnings.length > 0 && (
        <div className="link-notice" role="status" aria-live="polite">
          {run.warnings.map((w, i) => (
            <p key={i} className="warn-line">{w}</p>
          ))}
        </div>
      )}
      <RunOutcome run={run} onOpenItem={onOpenItem} onDismiss={onDismiss} againLabel={againLabel} />
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

/* ---------- System audio + mic (#139) ---------- */

function SystemPanel({
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
  const run = engine.runs.system;
  const [title, setTitle] = useState("");
  const [list, setList] = useState<SystemAudioDevices | null>(null);
  const [listError, setListError] = useState<string | null>(null);
  /** "" = the dictation's input device (Settings). */
  const [mic, setMic] = useState(ctl.settings.input_device ?? "");
  const [system, setSystem] = useState("");
  const [refresh, setRefresh] = useState(0);
  const os = osOf();
  const help = SETUP_HELP[os];
  const { confirmNotice, dialog } = useRecordingNotice(ctl);

  useEffect(() => {
    let alive = true;
    invoke<SystemAudioDevices>("list_system_audio_devices")
      .then((l) => {
        if (!alive) return;
        setList(l);
        setListError(null);
        setSystem((current) => (choiceStillValid(l, current) ? current : initialSystemDevice(l, loadSystemDevice(), mic)));
      })
      .catch((e) => alive && setListError(String(e)));
    return () => {
      alive = false;
    };
    // `mic` only seeds the first choice; a later mic change doesn't reset it.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [refresh]);

  if (run) {
    return (
      <CaptureLive
        run={run}
        onStop={engine.stopSystem}
        onCancel={() => engine.cancel("system")}
        onDismiss={() => engine.dismiss("system")}
        onOpenItem={onOpenItem}
        setBusy={ctl.setBusy}
        againLabel="New recording"
      />
    );
  }

  const defaultInput = list?.default_input ?? null;
  const settingsMic = ctl.settings.input_device || defaultInput || "system default";
  const native = list?.native ?? null;
  const hasNative = nativeAvailable(list);
  const isNative = system === NATIVE;
  const problem = list ? devicesProblem(mic || ctl.settings.input_device || "", system, defaultInput, native) : null;
  const noLoopback = !!list && !hasNative && !list.devices.some((d) => d.loopback);
  const fallback = nativeFallbackNote(list);
  return (
    <div className="stack">
      <div className="mic-start">
        <div>
          <h2 className="sh-h2">Record a call from a desktop app</h2>
          <p className="sh-muted">
            Zoom, Teams or any app that plays through the computer: your microphone and the computer's sound are
            recorded as two channels and saved as a <strong>Meeting</strong>. You are “You”; the others are told apart
            as Voice 1, Voice 2… — rename them in the document.
          </p>
        </div>

        <label className="field-stack">
          <span className="opt-k">Your microphone</span>
          <select value={mic} onChange={(e) => setMic(e.target.value)}>
            <option value="">Dictation input ({settingsMic})</option>
            {list?.devices.map((d) => (
              <option key={d.name} value={d.name}>{deviceLabel(d, defaultInput)}</option>
            ))}
          </select>
        </label>

        <label className="field-stack">
          <span className="opt-k">
            Computer's sound{" "}
            <button type="button" className="link-btn" onClick={() => setRefresh((n) => n + 1)}>
              refresh
            </button>
          </span>
          <select value={system} onChange={(e) => setSystem(e.target.value)} aria-invalid={!!problem && !!system}>
            <option value="">Choose what carries the computer's sound…</option>
            {hasNative && native && <option value={NATIVE}>{nativeOptionLabel(native)}</option>}
            {list?.devices.length ? (
              <optgroup label={hasNative ? "Or an input device" : "Input devices"}>
                {list.devices.map((d) => (
                  <option key={d.name} value={d.name}>{deviceLabel(d, defaultInput)}</option>
                ))}
              </optgroup>
            ) : null}
          </select>
        </label>
        {listError && <p className="link-notice">Could not list the audio devices: {listError}</p>}
        {problem && system && <p className="link-notice">{problem}</p>}
        {isNative && native && <p className="sh-muted">{nativeNote(native)}</p>}
        {fallback && <p className="sh-muted">{fallback}</p>}
        <p className="sh-muted">{ECHO_NOTE}</p>
        {noLoopback && (
          <p className="link-notice">
            No loopback device found. Install {help.devices} (see below), then press refresh — or pick any input that
            carries the call's sound.
          </p>
        )}

        <details className="sys-help" open={noLoopback}>
          <summary>
            {hasNative ? "Or route the computer's sound through a virtual device" : "How to route the computer's sound into an input device"} ({help.devices})
          </summary>
          <ol>
            {help.steps.map((s) => (
              <li key={s}>{s}</li>
            ))}
          </ol>
          <p className="sh-muted">Sussurro only reads what you choose here — nothing leaves the computer.</p>
        </details>

        <label className="field-stack">
          <span className="opt-k">Title <span className="sh-muted">(optional)</span></span>
          <input value={title} onChange={(e) => setTitle(e.target.value)} placeholder="From the first words if empty" spellCheck={false} />
        </label>
        <div className="row-gap">
          <span className="tb meeting" title="Other people's voices: a meeting">Meeting</span>
          <button
            type="button"
            className="btn-rec"
            disabled={!engine.canStartSystem || !list || !!problem}
            title={
              !engine.canStartSystem
                ? isRunning(engine.runs.mic)
                  ? "A microphone session is using the microphone"
                  : "Wait for the file to start"
                : problem ?? "Start recording"
            }
            onClick={async () => {
              // It records other people (#136).
              if (!(await confirmNotice())) return;
              onRunStart("system");
              const err = await engine.startSystem({ mic: mic || null, system, native: isNative }, title, options);
              if (err) ctl.setBusy(err);
              else {
                saveSystemDevice(system);
                setTitle("");
              }
            }}
          >
            <span className="rec-dot" aria-hidden="true" /> {engine.systemStarting ? "Starting…" : "Start recording"}
          </button>
        </div>
        <RecordingReminder />
      </div>
      {dialog}
    </div>
  );
}

/* ---------- "Identify voices" (P11, #134) ---------- */

/** Per-run toggle for transcriptions (off by default): label the voices
 *  "Voice 1, Voice 2…" with the same clustering as meetings. */
function IdentifyVoices({ checked, onChange }: { checked: boolean; onChange: (on: boolean) => void }) {
  return (
    <label className="check-row">
      <input type="checkbox" checked={checked} onChange={(e) => onChange(e.target.checked)} />
      <span>
        Identify voices{" "}
        <span className="sh-muted">
          — label who speaks as Voice 1, Voice 2… (rename them in the document; the speaker model is downloaded on
          first use)
        </span>
      </span>
    </label>
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
  const [identify, setIdentify] = useState(false);
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
            setIdentify(false);
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

      {/* Notes are the user's own voice: never speakers (P10). */}
      {itemType === "transcription" && <IdentifyVoices checked={identify} onChange={setIdentify} />}

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
            await engine.startFile(path, itemType, title, {
              ...options,
              identifyVoices: identifyVoicesArg(itemType, identify),
            });
          }}
        >
          Transcribe
        </button>
      </div>
    </div>
  );
}

/* ---------- Link (#123) ---------- */

/** Text with `code` spans (the install instructions). */
function WithCode({ text }: { text: string }) {
  return (
    <>
      {text.split("`").map((part, i) => (i % 2 === 1 ? <code key={i}>{part}</code> : <span key={i}>{part}</span>))}
    </>
  );
}

function LinkPanel({
  engine,
  options,
  onRunStart,
  onOpenItem,
}: {
  engine: EngineRuns;
  options: RunArgs;
  onRunStart: (kind: RunKind) => void;
  onOpenItem: (id: string) => void;
}) {
  const run = engine.runs.link;
  const [url, setUrl] = useState("");
  const [title, setTitle] = useState("");
  const [allowLocal, setAllowLocal] = useState(false);
  const [identify, setIdentify] = useState(false);
  const [info, setInfo] = useState<LinkInfo | null>(null);
  const [ytDlp, setYtDlp] = useState<YtDlpStatus | null>(null);
  const [startError, setStartError] = useState<string | null>(null);
  const live = isRunning(run);

  // Is yt-dlp installed? Asked once per visit (it may be installed meanwhile).
  useEffect(() => {
    let alive = true;
    invoke<YtDlpStatus>("yt_dlp_status")
      .then((s) => alive && setYtDlp(s))
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, []);

  // What the link is, as the backend sees it (no network), while typing.
  useEffect(() => {
    const u = url.trim();
    if (!u) {
      setInfo(null);
      return;
    }
    if (!looksLikeLink(u)) {
      setInfo({ kind: null, error: "Not a link yet — it should start with https://", local: false, label: "" });
      return;
    }
    let alive = true;
    const t = setTimeout(() => {
      invoke<LinkInfo>("link_inspect", { url: u })
        .then((i) => alive && setInfo(i))
        .catch(() => {});
    }, 200);
    return () => {
      alive = false;
      clearTimeout(t);
    };
  }, [url]);

  if (run && !live) {
    return (
      <div className="stack">
        <RunOutcome
          run={run}
          onOpenItem={onOpenItem}
          onDismiss={() => {
            engine.dismiss("link");
            setUrl("");
            setTitle("");
            setAllowLocal(false);
            setIdentify(false);
          }}
          againLabel="Transcribe another link"
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
    const phase = linkPhase(run);
    const d = run.download;
    const p = run.progress;
    const dlPct = downloadPercent(d);
    const pct = phase === "download" ? dlPct ?? 0 : p ? progressPercent(p.processed_s, p.total_s) : 0;
    const name = d?.title || run.label;
    return (
      <div className="stack">
        <div className="file-row">
          <span aria-hidden="true">🔗</span>
          <span className="file-name" title={run.label}>{name}</span>
          {phase === "transcribe" && p && (
            <span className="mono sh-muted">{formatClock(p.processed_s)} / {formatClock(p.total_s)}</span>
          )}
        </div>
        <ol className="link-steps" aria-label="Steps">
          <li className={phase === "download" ? "active" : "done"} aria-current={phase === "download" ? "step" : undefined}>
            1 · Download{d ? ` (${viaLabel(d.via)})` : ""}
          </li>
          <li className={phase === "transcribe" ? "active" : ""} aria-current={phase === "transcribe" ? "step" : undefined}>
            2 · Transcribe
          </li>
        </ol>
        <div
          className={`progress${phase === "download" && dlPct === null ? " indeterminate" : ""}`}
          role="progressbar"
          aria-label={phase === "download" ? `Downloading ${name}` : `Transcribing ${name}`}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={phase === "download" && dlPct === null ? undefined : pct}
        >
          <div className="progress-fill" style={{ width: `${phase === "download" && dlPct === null ? 100 : pct}%` }} />
        </div>
        <div className="row-gap">
          <span className="sh-muted" aria-live="polite">
            {phase === "download" ? describeDownload(d) : `${pct}% · ${describeProgress(run)}`}
            {phase === "transcribe" && p && p.segments_done > 0 && ` · ${p.segments_done} lines`}
          </span>
          <button
            type="button"
            className="btn-ghost sh-btn push"
            disabled={run.sessionId === null}
            title="Stop and discard: nothing is saved, the download is deleted"
            onClick={() => engine.cancel("link")}
          >
            Cancel
          </button>
        </div>
        {phase === "transcribe" && (
          <div className="live-box tx-scroll">
            <TranscriptView lines={toLines(run.segments)} follow label="Transcript so far" emptyText="The first lines appear after the first pause…" />
          </div>
        )}
      </div>
    );
  }

  const problem = linkProblem(info, ytDlp, allowLocal);
  const needsYtDlp = info?.kind === "platform" && ytDlp !== null && !ytDlp.found;
  return (
    <div className="stack">
      <div className="mic-start">
        <div>
          <h2 className="sh-h2">Transcribe from a link</h2>
          <p className="sh-muted">
            A direct link to an audio or video file, or a video page (YouTube, Vimeo, SoundCloud…) through yt-dlp.
            Saved as a <strong>Transcription</strong>.
          </p>
        </div>
        <label className="field-stack">
          <span className="opt-k">Link</span>
          <input
            type="url"
            value={url}
            onChange={(e) => {
              setUrl(e.target.value);
              setStartError(null);
            }}
            placeholder="https://…"
            spellCheck={false}
            autoComplete="off"
            aria-invalid={!!info?.error}
            aria-describedby="link-status"
          />
        </label>
        <div id="link-status" className="row-gap" aria-live="polite">
          {info?.kind && <span className={`tb ${info.kind === "platform" ? "meeting" : "note"}`}>{kindLabel(info.kind)}</span>}
          {info?.kind === "platform" && ytDlp?.found && (
            <span className="sh-muted">yt-dlp {ytDlp.version ?? ""} found</span>
          )}
          {problem && <span className="field-err">{problem}</span>}
        </div>
        {needsYtDlp && ytDlp && (
          <p className="link-notice">
            <WithCode text={ytDlp.install_help} />
          </p>
        )}
        <label className="check-row">
          <input type="checkbox" checked={allowLocal} onChange={(e) => setAllowLocal(e.target.checked)} />
          <span>
            Allow local network addresses{" "}
            <span className="sh-muted">(this computer, your router, a NAS — off unless you need it)</span>
          </span>
        </label>
        <IdentifyVoices checked={identify} onChange={setIdentify} />
        {identify && (
          <p className="sh-note">
            Decide now: the download is deleted after the transcription, so voices can't be identified later without
            transcribing the link again.
          </p>
        )}
        <label className="field-stack">
          <span className="opt-k">Title <span className="sh-muted">(optional — the video's title, or the file name, if empty)</span></span>
          <input value={title} onChange={(e) => setTitle(e.target.value)} spellCheck={false} />
        </label>
        {startError && <p className="run-outcome err" role="alert">{startError}</p>}
        <div className="row-gap">
          <span className="tb transcription" title="Audio recorded by others">Transcription</span>
          <button
            type="button"
            className="btn-dark push"
            disabled={!canTranscribeLink(info, ytDlp, allowLocal) || !engine.canStartLink}
            onClick={async () => {
              if (!info?.label) return;
              onRunStart("link");
              const err = await engine.startLink(url, title, info.label, allowLocal, {
                ...options,
                identifyVoices: identifyVoicesArg("transcription", identify),
              });
              setStartError(err);
            }}
          >
            {engine.linkStarting ? "Starting…" : "Transcribe"}
          </button>
        </div>
      </div>
      <p className="sh-note" role="note">
        Downloading from video platforms is subject to their terms of service and to copyright. Transcribe only
        media you have the right to use — you are responsible for what you download. The downloaded audio is a
        temporary file, deleted when the transcription ends.
      </p>
    </div>
  );
}

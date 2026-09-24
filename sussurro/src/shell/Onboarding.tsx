import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import { HotkeyRecorder } from "../components/HotkeyRecorder";
import { Switch } from "../components/ui";
import type { Ctl } from "../hooks/useAppController";
import { cleanupProfile, profileSummary } from "../lib/llmProfiles";
import {
  LOCAL_SERVERS,
  SETUP_STEPS,
  STEP_TITLES,
  WHATS_NEW,
  isMac,
  permissionRows,
  probeProfile,
  profileForServer,
  stepAfter,
  stepBefore,
  syncedFolder,
  withCleanupServer,
  type LocalServer,
  type OnboardingMode,
  type SetupStep,
} from "../lib/onboarding";
import { CleanupLevelControl } from "../settings/CleanupCard";
import { EngineField, ModelField } from "../settings/SpeechCard";

/** First-run onboarding (#115), over the workspace: the guided setup on a
 *  fresh install, one "What's new" screen for a user upgrading from 0.6.x.
 *  Every step can be skipped; closing it (finished or skipped) stores
 *  `onboarding: "done"`, and Settings → About → "Run the setup again"
 *  opens the setup again. */
export function Onboarding({
  ctl,
  mode,
  onModeChange,
  onClose,
}: {
  ctl: Ctl;
  mode: OnboardingMode;
  onModeChange: (m: OnboardingMode) => void;
  onClose: () => void;
}) {
  const archive = useArchiveFolder(ctl);
  const finish = async () => {
    if (ctl.micTest) await ctl.toggleMicTest();
    if (ctl.settings.onboarding !== "done") await ctl.save({ ...ctl.settings, onboarding: "done" });
    onClose();
  };
  return (
    <div className="modal-backdrop onb-backdrop" role="presentation">
      <div className="modal onb" role="dialog" aria-modal="true" aria-labelledby="onb-title">
        {mode === "whats_new" ? (
          <WhatsNew ctl={ctl} archive={archive} onSetup={() => onModeChange("welcome")} onDone={finish} />
        ) : (
          <Setup ctl={ctl} archive={archive} onDone={finish} />
        )}
      </div>
    </div>
  );
}

/* ---------- The archive folder (the macOS Documents prompt) ---------- */

type ArchiveFolder = ReturnType<typeof useArchiveFolder>;

/** The archive path, and `prepare()`: creating the folder now is what makes
 *  macOS ask for the Documents folder here, never when a recording starts. */
function useArchiveFolder(ctl: Ctl) {
  const [path, setPath] = useState("");
  const [status, setStatus] = useState<"idle" | "working" | "ready" | "error">("idle");
  const [error, setError] = useState("");

  // Only the path: `archive_dir` touches nothing on disk.
  useEffect(() => {
    setStatus("idle");
    invoke<string>("archive_dir")
      .then(setPath)
      .catch((e) => setError(String(e)));
  }, [ctl.settings.archive_dir]);

  const prepare = useCallback(async (): Promise<boolean> => {
    setStatus("working");
    setError("");
    try {
      setPath(await invoke<string>("archive_prepare"));
      setStatus("ready");
      return true;
    } catch (e) {
      setError(String(e));
      setStatus("error");
      return false;
    }
  }, []);

  const change = async () => {
    const picked = await openDialog({ title: "Choose the archive folder", directory: true, multiple: false });
    if (!picked || typeof picked !== "string") return;
    if (await ctl.save({ ...ctl.settings, archive_dir: picked })) await prepare();
  };

  const useDefault = async () => {
    if (await ctl.save({ ...ctl.settings, archive_dir: "" })) await prepare();
  };

  return { path, status, error, prepare, change, useDefault, custom: ctl.settings.archive_dir.trim() !== "" };
}

function ArchiveBox({ ctl, archive, compact = false }: { ctl: Ctl; archive: ArchiveFolder; compact?: boolean }) {
  const synced = syncedFolder(archive.path);
  const mac = isMac();
  return (
    <div className="onb-archive">
      <div className="model-row">
        <span className="path" title={archive.path}>{archive.path || "…"}</span>
      </div>
      <div className="list-actions start">
        {archive.status !== "ready" && (
          <button type="button" className="btn-dark sh-btn" disabled={archive.status === "working"} onClick={archive.prepare}>
            {archive.status === "working" ? "Creating…" : mac && !archive.custom ? "Allow access and create the folder" : "Create the folder"}
          </button>
        )}
        <button type="button" className="btn-ghost sh-btn" disabled={archive.status === "working"} onClick={archive.change}>
          Change…
        </button>
        {archive.custom && (
          <button type="button" className="btn-ghost sh-btn" disabled={archive.status === "working"} onClick={archive.useDefault}>
            Use the default
          </button>
        )}
      </div>
      <p className="onb-status" role="status">
        {archive.status === "ready" && <span className="onb-ok">✓ The archive folder is ready.</span>}
        {archive.status === "error" && (
          <>
            <span className="onb-bad">✗ {archive.error}</span>
            {mac && (
              <button type="button" className="link-btn" onClick={() => invoke("open_settings", { target: "files" }).catch((e) => ctl.setBusy(String(e)))}>
                {" "}Open Files and Folders
              </button>
            )}
          </>
        )}
      </p>
      {!compact && mac && !archive.custom && (
        <p className="card-hint">
          macOS asks once whether Sussurro may use your Documents folder. It asks now, so the question never
          interrupts a recording — choose <b>Allow</b>.
        </p>
      )}
      <p className="card-hint">
        {synced
          ? `This folder is in ${synced}: your notes and transcriptions will sync there too.`
          : "If Documents is synced — iCloud Drive (“Desktop & Documents”) or OneDrive — your notes and transcriptions sync with it."}{" "}
        Prefer another place, like an Obsidian vault? Use Change…
      </p>
    </div>
  );
}

/* ---------- What's new (upgrade from 0.6.x) ---------- */

function WhatsNew({
  ctl,
  archive,
  onSetup,
  onDone,
}: {
  ctl: Ctl;
  archive: ArchiveFolder;
  onSetup: () => void;
  onDone: () => void;
}) {
  const [failedOnce, setFailedOnce] = useState(false);
  const start = async () => {
    // Ask for the folder now (the macOS Documents prompt), unless the user
    // already saw it fail and wants to go on.
    if (archive.status !== "ready" && !failedOnce && !(await archive.prepare())) {
      setFailedOnce(true);
      return;
    }
    onDone();
  };
  return (
    <>
      <header className="onb-head">
        <span className="daruma" aria-hidden="true" />
        <h2 id="onb-title">What's new in Sussurro {ctl.version}</h2>
      </header>
      <div className="onb-body">
        <ul className="onb-list">
          {WHATS_NEW.map((e) => (
            <li key={e.title}>
              <b>{e.title}.</b> {e.text}
            </li>
          ))}
        </ul>
        <h3 className="onb-h3">Your archive</h3>
        <p className="card-hint">Notes and transcriptions are saved here, one folder of markdown files per item.</p>
        <ArchiveBox ctl={ctl} archive={archive} />
      </div>
      <footer className="onb-foot">
        {ctl.busy && <p className="onb-busy" role="status">{ctl.busy}</p>}
        <button type="button" className="btn-ghost sh-btn" onClick={onSetup}>
          Run the full setup
        </button>
        <button type="button" className="btn-dark sh-btn push" disabled={archive.status === "working"} onClick={start}>
          {failedOnce && archive.status === "error" ? "Continue anyway" : "Get started"}
        </button>
      </footer>
    </>
  );
}

/* ---------- The guided setup (fresh install) ---------- */

function Setup({ ctl, archive, onDone }: { ctl: Ctl; archive: ArchiveFolder; onDone: () => void }) {
  const [step, setStep] = useState<SetupStep>("welcome");
  const bodyRef = useRef<HTMLDivElement>(null);
  const index = SETUP_STEPS.indexOf(step);

  const go = async (next: SetupStep | null) => {
    // Never leave the microphone open behind the next step.
    if (ctl.micTest) await ctl.toggleMicTest();
    if (next === null) {
      onDone();
      return;
    }
    setStep(next);
    bodyRef.current?.scrollTo?.({ top: 0 });
  };

  const onNext = async () => {
    // The archive step's Next is the deliberate moment for the macOS
    // Documents prompt; on failure the error shows and the user can Skip.
    if (step === "archive" && archive.status !== "ready" && !(await archive.prepare())) return;
    go(stepAfter(step));
  };

  const last = stepAfter(step) === null;
  return (
    <>
      <header className="onb-head">
        <span className="daruma" aria-hidden="true" />
        <h2 id="onb-title">{step === "welcome" ? "Welcome to Sussurro" : STEP_TITLES[step]}</h2>
        <button type="button" className="btn-ghost sh-btn push" onClick={onDone}>
          Skip setup
        </button>
      </header>
      <ol className="onb-steps" aria-label={`Step ${index + 1} of ${SETUP_STEPS.length}`}>
        {SETUP_STEPS.map((s, i) => (
          <li key={s} className={i === index ? "on" : i < index ? "done" : ""} aria-current={i === index ? "step" : undefined}>
            {STEP_TITLES[s]}
          </li>
        ))}
      </ol>
      <div className="onb-body" ref={bodyRef}>
        {step === "welcome" && <WelcomeStep />}
        {step === "permissions" && <PermissionsStep ctl={ctl} />}
        {step === "archive" && <ArchiveStep ctl={ctl} archive={archive} />}
        {step === "models" && <ModelsStep ctl={ctl} />}
        {step === "cleanup" && <CleanupStep ctl={ctl} />}
        {step === "hotkey" && <HotkeyStep ctl={ctl} />}
      </div>
      <footer className="onb-foot">
        {ctl.busy && <p className="onb-busy" role="status">{ctl.busy}</p>}
        {stepBefore(step) && (
          <button type="button" className="btn-ghost sh-btn" onClick={() => go(stepBefore(step))}>
            Back
          </button>
        )}
        {step !== "welcome" && (
          <button type="button" className="btn-ghost sh-btn push" onClick={() => go(stepAfter(step))}>
            {last ? "Skip and finish" : "Skip this step"}
          </button>
        )}
        <button
          type="button"
          className={`btn-dark sh-btn${step === "welcome" ? " push" : ""}`}
          disabled={step === "archive" && archive.status === "working"}
          onClick={onNext}
        >
          {step === "welcome" ? "Set up Sussurro" : last ? "Finish" : "Next"}
        </button>
      </footer>
    </>
  );
}

function WelcomeStep() {
  return (
    <>
      <p className="onb-lead">
        Sussurro turns your voice into text on this computer. What you say is transcribed here and never leaves it —
        unless you choose to use an external LLM for cleanup.
      </p>
      <ul className="onb-list">
        <li>
          <b>Dictation.</b> Press the shortcut in any app, speak, and the text is pasted where your cursor is. This
          window can stay closed: Sussurro lives in the tray.
        </li>
        <li>
          <b>Notes and transcriptions.</b> Record long notes from the microphone, or transcribe audio files and links,
          into an archive of markdown files.
        </li>
        <li>
          <b>Cleanup and recipes.</b> A local LLM removes filler words and fixes punctuation, and turns transcripts into
          summaries, action items or minutes.
        </li>
      </ul>
      <p className="card-hint">
        Five short steps: permissions, the archive folder, the speech model, cleanup and your shortcut. Each can be
        skipped; Settings → About → Run the setup again brings them back.
      </p>
    </>
  );
}

function StatusIcon({ ok }: { ok: boolean }) {
  return ok ? <span className="onb-ok" aria-label="Done">✓</span> : <span className="onb-bad" aria-label="To do">✗</span>;
}

function PermissionsStep({ ctl }: { ctl: Ctl }) {
  const { checkPermissions } = ctl;
  // Fresh on entry and when a microphone test ends; during a test every 2 s,
  // so the answer to the OS prompt shows up while it runs.
  useEffect(() => {
    checkPermissions();
    if (!ctl.micTest) return;
    const id = setInterval(checkPermissions, 2000);
    return () => clearInterval(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ctl.micTest]);
  const rows = permissionRows(ctl.permissions);
  return (
    <>
      <p className="onb-lead">Sussurro needs to hear you, and to paste what you said.</p>
      {rows.length === 0 && <p className="card-hint">Checking…</p>}
      <ul className="onb-checks">
        {rows.map((r) => (
          <li key={r.id}>
            <StatusIcon ok={r.ok} />
            <div className="onb-check-text">
              <b>{r.label}</b>
              <span className="sh-muted">{r.detail}</span>
              {r.id === "microphone" && (ctl.micTest || ctl.recordingNow) && (
                <div className="vu" role="meter" aria-label="Input level" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(ctl.vuPct)}>
                  <div className="vu-fill" style={{ width: `${ctl.vuPct}%` }} />
                </div>
              )}
            </div>
            {r.id === "microphone" && r.action !== "settings" && (
              <button type="button" className="btn-ghost sh-btn" onClick={ctl.toggleMicTest}>
                {ctl.micTest ? "Stop the test" : r.action === "ask" ? "Test the microphone" : "Test"}
              </button>
            )}
            {r.action === "settings" && (
              <button
                type="button"
                className="btn-ghost sh-btn"
                onClick={() => invoke("open_settings", { target: r.id }).catch((e) => ctl.setBusy(String(e)))}
              >
                Open System Settings
              </button>
            )}
          </li>
        ))}
      </ul>
      <div className="list-actions start">
        <button type="button" className="btn-ghost sh-btn" onClick={checkPermissions}>
          Check again
        </button>
      </div>
      {isMac() && (
        <p className="card-hint">
          After allowing Accessibility, macOS may ask to reopen Sussurro. The Documents folder comes next.
        </p>
      )}
    </>
  );
}

function ArchiveStep({ ctl, archive }: { ctl: Ctl; archive: ArchiveFolder }) {
  return (
    <>
      <p className="onb-lead">
        Notes, meetings and transcriptions are saved in one folder, as markdown files you can open with any editor.
        Dictation doesn't use it: it pastes where your cursor is.
      </p>
      <ArchiveBox ctl={ctl} archive={archive} />
    </>
  );
}

function ModelsStep({ ctl }: { ctl: Ctl }) {
  const { modelReady, sidecarAvailable } = ctl;
  return (
    <>
      <p className="onb-lead">Speech becomes text with a model that runs on this computer. Pick one and download it once.</p>
      <EngineField ctl={ctl} />
      <ModelField ctl={ctl} />
      <p className="onb-status" role="status">
        {modelReady ? (
          <span className="onb-ok">✓ The selected model is downloaded and ready.</span>
        ) : (
          "Not downloaded yet: use the download button next to the model. It can take a few minutes."
        )}
      </p>
      <ul className="onb-list small">
        <li>
          <b>Whisper</b> works in any language and uses the graphics card. Large-v3-turbo is the most accurate; the
          smaller ones are quicker to download and run.
        </li>
        <li>
          <b>Parakeet</b> is much faster on computers without a strong graphics card and recognises 25 European
          languages on its own (one 456 MB model).
        </li>
        {sidecarAvailable && (
          <li>
            <b>Qwen3-ASR</b> is optional (about 2.5 GB): better on English, while Whisper stays more accurate on
            Italian. Details under Models.
          </li>
        )}
      </ul>
    </>
  );
}

/** What the cleanup step found at one local server. */
type Probe = { state: "checking" } | { state: "found"; models: string[] } | { state: "absent" };

function CleanupStep({ ctl }: { ctl: Ctl }) {
  const { settings, save } = ctl;
  const [probes, setProbes] = useState<Record<string, Probe>>({});
  const [round, setRound] = useState(0);

  // Local servers only, at their default ports (llm_list_models, nothing saved).
  useEffect(() => {
    let live = true;
    setProbes(Object.fromEntries(LOCAL_SERVERS.map((s) => [s.name, { state: "checking" } as Probe])));
    for (const server of LOCAL_SERVERS) {
      invoke<string[]>("llm_list_models", { profile: probeProfile(server) })
        .then((models) => live && setProbes((p) => ({ ...p, [server.name]: { state: "found", models } })))
        .catch(() => live && setProbes((p) => ({ ...p, [server.name]: { state: "absent" } })));
    }
    return () => {
      live = false;
    };
  }, [round]);

  const current = cleanupProfile(settings);
  const use = async (server: LocalServer, models: string[]) => {
    if (await save(withCleanupServer(settings, server, models))) ctl.checkOllama();
  };
  const row = (server: LocalServer): ReactNode => {
    const probe = probes[server.name] ?? { state: "checking" };
    const inUse = profileForServer(settings, server)?.id === current?.id;
    return (
      <li key={server.name}>
        <StatusIcon ok={probe.state === "found"} />
        <div className="onb-check-text">
          <b>{server.name}</b>
          <span className="sh-muted">
            {probe.state === "checking" && "Looking…"}
            {probe.state === "absent" && `Not running on this computer (${server.base_url}).`}
            {probe.state === "found" &&
              (probe.models.length > 0
                ? `Running, with ${probe.models.length} model${probe.models.length === 1 ? "" : "s"}.`
                : "Running, but it has no models yet.")}
          </span>
        </div>
        {probe.state === "found" && (inUse ? (
          <span className="onb-ok">In use</span>
        ) : (
          <button type="button" className="btn-ghost sh-btn" disabled={probe.models.length === 0} onClick={() => use(server, probe.models)}>
            Use for cleanup
          </button>
        ))}
        {probe.state === "found" && inUse && server.api === "ollama" && probe.models.length === 0 && (
          <button type="button" className="btn-ghost sh-btn" disabled={ctl.pullingModel} onClick={ctl.pullOllamaModel}>
            {ctl.pullingModel ? "Pulling…" : `Pull ${current?.model ?? "a model"}`}
          </button>
        )}
        {probe.state === "absent" && server.api === "ollama" && (
          <button type="button" className="btn-ghost sh-btn" onClick={() => openUrl("https://ollama.com/download")}>
            Get Ollama
          </button>
        )}
      </li>
    );
  };
  const anyFound = Object.values(probes).some((p) => p.state === "found");
  return (
    <>
      <p className="onb-lead">
        Cleanup removes filler words and fixes punctuation with a language model. It is optional: without it you get
        exactly what you said.
      </p>
      <ul className="onb-checks">{LOCAL_SERVERS.map(row)}</ul>
      <div className="list-actions start">
        <button type="button" className="btn-ghost sh-btn" onClick={() => setRound((r) => r + 1)}>
          Check again
        </button>
      </div>
      {current && <p className="card-hint">Cleanup uses: <b>{current.name}</b> · {profileSummary(current)}</p>}
      <div className="field">
        <div className="field-label">
          <span>Cleanup level</span>
          <small>{anyFound ? "Light is a good start" : "None until a server is running"}</small>
        </div>
        <CleanupLevelControl ctl={ctl} />
      </div>
      <p className="card-hint">
        Another server, a remote one or an API key: add it later as an LLM profile in Recipes.
      </p>
    </>
  );
}

function HotkeyStep({ ctl }: { ctl: Ctl }) {
  const { settings, save } = ctl;
  return (
    <>
      <p className="onb-lead">
        {settings.push_to_talk
          ? "Hold the shortcut in any app, speak, and let go: the text is pasted where your cursor is."
          : "Press the shortcut in any app, speak, and press it again: the text is pasted where your cursor is."}
      </p>
      <div className="field">
        <div className="field-label">
          <span>Shortcut</span>
          <small>click, then press the keys (Esc cancels)</small>
        </div>
        <HotkeyRecorder value={settings.hotkey} onChange={(combo) => save({ ...settings, hotkey: combo })} />
      </div>
      <div className="field">
        <div className="field-label">
          <span>Push-to-talk</span>
          <small>{settings.push_to_talk ? "hold to record" : "off: tap to start, tap to stop"}</small>
        </div>
        <Switch checked={settings.push_to_talk} onChange={(v) => save({ ...settings, push_to_talk: v })} label="Push-to-talk" />
      </div>
      <p className="card-hint">
        The microphone, the dictionary and snippets are in Settings. New, in the sidebar, records notes and transcribes
        files.
      </p>
    </>
  );
}

import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { Ctl } from "../hooks/useAppController";
import { bundledProblem, cleanupProfile, offerBundled } from "../lib/llmProfiles";
import { BundledOffer, BundledProblem } from "./BundledLlm";

/** First-run checklist: permissions, Ollama (or the bundled model, #118),
 *  speech model. Hidden once everything is in place or when dismissed for
 *  this window session. */
export function SetupBanner({ ctl }: { ctl: Ctl }) {
  const [setupDismissed, setSetupDismissed] = useState(false);
  const { settings, ollamaStatus, ollamaModels, permissions, modelReady, bundledLlm } = ctl;
  // The Ollama checks only concern an Ollama cleanup profile (#119).
  const onOllama = cleanupProfile(settings)?.api === "ollama";

  const needed =
    (ollamaStatus !== null &&
      (!ollamaStatus.running || !ollamaStatus.has_model || !modelReady)) ||
    offerBundled(settings, bundledLlm, ollamaStatus) ||
    bundledProblem(settings, bundledLlm) !== null ||
    permissions?.microphone === "denied" ||
    permissions?.accessibility === "denied";
  if (setupDismissed || !needed) return null;

  return (
    <div className="setup-banner" role="alert">
      <div className="setup-head">
        <strong>Setup</strong>
        <span className="summary-right">
          <button
            className="btn-ghost"
            onClick={() => {
              ctl.checkOllama();
              ctl.checkPermissions();
              ctl.loadBundledLlm();
            }}
          >
            Re-check
          </button>
          <button
            className="btn-ghost"
            onClick={() => setSetupDismissed(true)}
            aria-label="Dismiss setup banner"
          >
            ×
          </button>
        </span>
      </div>
      <ul>
        {permissions?.accessibility === "denied" && (
          <li>
            <span className="setup-bad">✗</span> Accessibility permission not
            granted — Sussurro needs it to paste dictated text into other apps.
            <button
              className="btn-ghost"
              onClick={() => invoke("open_settings", { target: "accessibility" })}
            >
              Open Settings
            </button>
          </li>
        )}
        {permissions?.microphone === "denied" && (
          <li>
            <span className="setup-bad">✗</span> Microphone access denied —
            dictation can't record until you allow it.
            <button
              className="btn-ghost"
              onClick={() => invoke("open_settings", { target: "microphone" })}
            >
              Open Settings
            </button>
          </li>
        )}
        {onOllama && ollamaStatus && !ollamaStatus.installed && (
          <li>
            <span className="setup-bad">✗</span> Ollama is not installed — cleanup
            and translation need it (dictation still works, raw only).
            <button
              className="btn-ghost"
              onClick={() => openUrl("https://ollama.com/download")}
            >
              Get Ollama
            </button>
          </li>
        )}
        {onOllama && ollamaStatus?.installed && !ollamaStatus.running && (
          <li>
            <span className="setup-bad">✗</span> Ollama is installed but not
            running — start the Ollama app (or run <code>ollama serve</code>),
            then Re-check.
          </li>
        )}
        {onOllama && ollamaStatus?.running && !ollamaStatus.has_model && (!ollamaModels || ollamaModels.length === 0) && (
          <li>
            <span className="setup-bad">✗</span> No models on your Ollama
            server yet — pull one to enable cleanup.
            <button
              className="btn-ghost"
              disabled={ctl.pullingModel}
              onClick={ctl.pullOllamaModel}
            >
              {ctl.pullingModel && <span className="btn-spinner" aria-hidden="true" />}
              {ctl.pullingModel ? "Pulling…" : "Pull it"}
            </button>
          </li>
        )}
        <BundledOffer ctl={ctl} inline />
        <BundledProblem ctl={ctl} inline />
        {!modelReady && (
          <li>
            <span className="setup-bad">✗</span> Speech model not downloaded yet.
            <button className="btn-ghost" onClick={ctl.downloadModel} disabled={ctl.downloadingModel}>
              {ctl.downloadingModel && <span className="btn-spinner" aria-hidden="true" />}
              {ctl.downloadingModel ? "Downloading…" : "Download"}
            </button>
          </li>
        )}
      </ul>
    </div>
  );
}

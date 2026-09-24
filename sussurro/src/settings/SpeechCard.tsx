import type { ReactNode } from "react";
import { AdvancedGroup, CollapsibleCard, Switch, Tip } from "../components/ui";
import { LANGUAGES, MODELS } from "../lib/constants";
import { ENGINES, detectsLanguage, engineDetail, engineLabel, engineSelectable } from "../lib/engines";
import type { Ctl } from "../hooks/useAppController";
import type { CardProps } from "./DictationCard";

/* The speech-recognition fields are split so the workspace can show the
   engine and model under Models and the rest under Settings → Speech, while
   the classic window keeps them together in one card. */

export function EngineField({ ctl }: { ctl: Ctl }) {
  const { settings, save, sidecarAvailable } = ctl;
  return (
    <div className="field">
      <div className="field-label">
        <span>Engine <Tip text="Whisper: GPU-accelerated, any language, choose the model size below. Parakeet: NVIDIA's CPU-optimized model — roughly 10x faster than Whisper without a GPU, auto-detects 25 European languages, one fixed 456 MB model. Qwen3-ASR: optional, runs in a bundled llama-server; see its card under Models." /></span>
        <small>{engineDetail(settings.engine)}</small>
      </div>
      <div className="segmented" role="radiogroup" aria-label="STT engine">
        {ENGINES.map(({ value: e, label }) => {
          const selectable = engineSelectable(e, settings.engine, sidecarAvailable);
          return (
            <button
              key={e}
              role="radio"
              aria-checked={settings.engine === e}
              className={settings.engine === e ? "on" : ""}
              disabled={!selectable}
              title={selectable ? undefined : "Not in this build: it needs the bundled llama-server"}
              onClick={() => save({ ...settings, engine: e })}
            >
              {label}
            </button>
          );
        })}
      </div>
    </div>
  );
}

export function LanguageField({ ctl }: { ctl: Ctl }) {
  const { settings, save } = ctl;
  if (settings.engine !== "whisper") return null;
  return (
    <div className="field">
      <div className="field-label">
        <span>Language <Tip text="Tell Whisper which language you dictate in. A fixed language is more accurate and slightly faster than auto-detect — especially on smaller models. Note: the English-only models ignore this." /></span>
        <small>hint for the transcriber</small>
      </div>
      <select
        value={settings.language}
        onChange={(e) => save({ ...settings, language: e.target.value })}
        aria-label="Language"
      >
        {LANGUAGES.map(([code, label]) => (
          <option key={code} value={code}>{label}</option>
        ))}
      </select>
    </div>
  );
}

export function ModelField({ ctl }: { ctl: Ctl }) {
  const { settings, save, installedWhisper, modelReady, downloadingModel } = ctl;
  return (
    <div className="field">
      <div className="field-label">
        <span>Model <Tip text="Whisper: bigger = more accurate but slower; 'English' variants are faster for English-only dictation. Parakeet has a single fixed model (int8, 456 MB); Qwen3-ASR too (1.7B Q8 plus its audio encoder, about 2.5 GB)." /></span>
        <small>speech-to-text, fully offline</small>
      </div>
      <div className="model-row">
        {settings.engine === "whisper" ? (
          <select
            value={settings.whisper_model}
            onChange={(e) => save({ ...settings, whisper_model: e.target.value })}
            aria-label="Whisper model"
          >
            {MODELS.map((m) => (
              <option key={m.file} value={m.file}>
                {m.label}{installedWhisper.includes(m.file) ? " · installed" : ""}
              </option>
            ))}
            {installedWhisper
              .filter((f) => !MODELS.some((m) => m.file === f))
              .map((f) => (
                <option key={f} value={f}>{f} · installed</option>
              ))}
          </select>
        ) : settings.engine === "parakeet" ? (
          <span className="fixed-model">Parakeet TDT 0.6B v3 · int8 · 456 MB</span>
        ) : (
          <span className="fixed-model">Qwen3-ASR 1.7B · Q8 · 2.5 GB</span>
        )}
        {!modelReady && (
          <button
            className="btn-primary btn-icon"
            onClick={ctl.downloadModel}
            disabled={downloadingModel}
            title={downloadingModel ? "Downloading — this can take a while…" : "Download the selected model"}
            aria-label={downloadingModel ? "Downloading model" : "Download the selected model"}
          >
            {downloadingModel ? (
              <span className="btn-spinner" aria-hidden="true" />
            ) : (
              <svg
                width="18"
                height="18"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth="2.5"
                strokeLinecap="round"
                strokeLinejoin="round"
                aria-hidden="true"
              >
                <path d="M12 3v12" />
                <path d="m7 10 5 5 5-5" />
                <path d="M5 21h14" />
              </svg>
            )}
          </button>
        )}
      </div>
    </div>
  );
}

export function ModelsFolderField({ ctl }: { ctl: Ctl }) {
  const { settings, setSettings, save } = ctl;
  return (
    <div className="field">
      <div className="field-label">
        <span>Models folder <Tip text="Where downloaded STT models are stored (up to a few GB). Leave empty for the default app-data folder, or point it at a roomier disk. Already-downloaded models must be moved there manually." /></span>
        <small>empty = app data default</small>
      </div>
      <input
        value={settings.models_dir}
        placeholder="F:\claude\models"
        onChange={(e) => setSettings({ ...settings, models_dir: e.target.value })}
        onBlur={() => save(settings)}
        spellCheck={false}
        aria-label="Models folder"
      />
    </div>
  );
}

export function WhisperModeField({ ctl }: { ctl: Ctl }) {
  const { settings, save } = ctl;
  return (
    <div className="field">
      <div className="field-label">
        <span>Whisper mode <Tip text="For dictating quietly (open office, late night): boosts microphone gain 3x and lowers the silence gate so soft speech still registers." /></span>
        <small>for quiet speech</small>
      </div>
      <Switch
        checked={settings.whisper_mode}
        onChange={(v) => save({ ...settings, whisper_mode: v })}
      />
    </div>
  );
}

/** Classic window: engine, language, model and the advanced speech options. */
export function SpeechCard({ ctl, collapsible }: CardProps) {
  return (
    <CollapsibleCard
      storageKey="speechOpen"
      title={<>Speech recognition <span className="via">engine & models</span></>}
      collapsible={collapsible}
    >
      <EngineField ctl={ctl} />
      <LanguageField ctl={ctl} />
      <ModelField ctl={ctl} />
      <AdvancedGroup>
        <ModelsFolderField ctl={ctl} />
        <WhisperModeField ctl={ctl} />
      </AdvancedGroup>
    </CollapsibleCard>
  );
}

/** Workspace Settings → Speech: what is left once engine and model moved to Models. */
export function SpeechOptionsCard({ ctl, footer }: { ctl: Ctl; footer?: ReactNode }) {
  return (
    <CollapsibleCard storageKey="speechOpen" title="Speech" collapsible={false}>
      <LanguageField ctl={ctl} />
      {detectsLanguage(ctl.settings.engine) && (
        <p className="card-hint">{engineLabel(ctl.settings.engine)} detects the language on its own.</p>
      )}
      <WhisperModeField ctl={ctl} />
      {footer}
    </CollapsibleCard>
  );
}

import { AdvancedGroup, CollapsibleCard } from "../components/ui";
import type { Ctl } from "../hooks/useAppController";
import { EngineField, ModelField, ModelsFolderField } from "../settings/SpeechCard";
import { cleanupLabel } from "./labels";
import type { SectionId } from "./SettingsScreen";

/** Models: the same engine / model choice and download as the classic
 *  Speech card (shared components, not a copy). */
export function ModelsScreen({ ctl, onOpenSettings }: { ctl: Ctl; onOpenSettings: (s: SectionId) => void }) {
  const { settings, installedWhisper, modelReady } = ctl;
  return (
    <div className="sh-screen">
      <header className="sh-topbar">
        <h1>Models</h1>
        <span className="sh-muted">Everything runs on this machine</span>
      </header>
      <div className="sh-scroll cards-col">
        <CollapsibleCard storageKey="modelsStt" title={<>Speech-to-text <span className="via">engine & model</span></>} collapsible={false}>
          <EngineField ctl={ctl} />
          <ModelField ctl={ctl} />
          <p className="card-hint" role="status">
            {modelReady
              ? "The selected model is downloaded and ready."
              : "The selected model is not downloaded yet — use the download button next to it."}
            {settings.engine === "whisper" && installedWhisper.length > 0 &&
              ` Installed in the models folder: ${installedWhisper.length}.`}
          </p>
          <AdvancedGroup>
            <ModelsFolderField ctl={ctl} />
          </AdvancedGroup>
        </CollapsibleCard>

        <CollapsibleCard storageKey="modelsLlm" title={<>Cleanup <span className="via">local LLM</span></>} collapsible={false}>
          <p className="card-hint">
            Currently: <strong>{cleanupLabel(settings)}</strong> via{" "}
            {settings.cleanup_api === "ollama" ? "Ollama" : "an OpenAI-compatible server"}. The server, model and
            prompts are set in Settings → Cleanup.
          </p>
          <button type="button" className="btn-ghost sh-btn" onClick={() => onOpenSettings("cleanup")}>
            Open cleanup settings
          </button>
        </CollapsibleCard>
      </div>
    </div>
  );
}

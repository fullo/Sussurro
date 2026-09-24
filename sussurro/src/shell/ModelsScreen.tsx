import { AdvancedGroup, Card } from "../components/ui";
import type { Ctl } from "../hooks/useAppController";
import { EngineField, ModelField, ModelsFolderField } from "../settings/SpeechCard";
import { QWEN3_ASR_NOTE } from "../lib/engines";
import { cleanupProfile, profileHost } from "../lib/llmProfiles";
import { cleanupGate } from "../lib/privacy";
import { cleanupLabel } from "./labels";
import type { SectionId } from "./SettingsScreen";

/** Models: engine and model choice and download (the same fields as the
 *  onboarding's model step — shared components, not a copy). */
export function ModelsScreen({
  ctl,
  onOpenSettings,
  onOpenRecipes,
}: {
  ctl: Ctl;
  onOpenSettings: (s: SectionId) => void;
  onOpenRecipes: () => void;
}) {
  const { settings, save, installedWhisper, modelReady, sidecarAvailable } = ctl;
  const qwenInUse = settings.engine === "qwen3_asr";
  const profile = cleanupProfile(settings);
  return (
    <div className="sh-screen">
      <header className="sh-topbar">
        <h1>Models</h1>
        <span className="sh-muted">
          {!profile?.external
            ? "Everything runs on this machine"
            : cleanupGate(settings).state === "allowed"
              ? "Speech runs on this machine; cleanup uses an external profile"
              : "Everything runs on this machine; cleanup is held back until you allow its external profile"}
        </span>
      </header>
      <div className="sh-scroll cards-col">
        <Card title={<>Speech-to-text <span className="via">engine & model</span></>}>
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
        </Card>

        <Card title={<>Qwen3-ASR <span className="via">optional engine</span></>}>
          <p className="card-hint">{QWEN3_ASR_NOTE}</p>
          {!sidecarAvailable && !qwenInUse ? (
            <p className="card-hint" role="status">
              Not available in this build: it has no bundled llama-server.
            </p>
          ) : (
            <div className="row-gap">
              {qwenInUse ? (
                <>
                  <span className="sh-muted" role="status">
                    In use{modelReady ? "" : " — download it with the button next to the model above"}.
                  </span>
                  <button type="button" className="btn-ghost sh-btn" onClick={() => save({ ...settings, engine: "whisper" })}>
                    Back to Whisper
                  </button>
                </>
              ) : (
                <button type="button" className="btn-ghost sh-btn" onClick={() => save({ ...settings, engine: "qwen3_asr" })}>
                  Use Qwen3-ASR
                </button>
              )}
            </div>
          )}
        </Card>

        <Card title={<>Cleanup <span className="via">LLM profile</span></>}>
          <p className="card-hint">
            Currently: <strong>{cleanupLabel(settings)}</strong>
            {profile && (
              <>
                {" "}on the <strong>{profile.name}</strong> profile ({profile.api === "ollama" ? "Ollama" : "OpenAI-compatible"}
                {profile.external ? `, external: ${profileHost(profile)}` : ", on this machine"})
              </>
            )}
            . Servers and models are LLM profiles, edited in Recipes; the level and prompts are in Settings → Cleanup.
          </p>
          <div className="row-gap">
            <button type="button" className="btn-ghost sh-btn" onClick={onOpenRecipes}>
              Edit LLM profiles
            </button>
            <button type="button" className="btn-ghost sh-btn" onClick={() => onOpenSettings("cleanup")}>
              Open cleanup settings
            </button>
          </div>
        </Card>
      </div>
    </div>
  );
}

import { AdvancedGroup, CollapsibleCard, EndpointNote, Tip } from "../components/ui";
import { CLEANUP_LEVELS, LANGUAGES } from "../lib/constants";
import type { Ctl } from "../hooks/useAppController";
import { API_LABELS, cleanupProfile, patchCleanupProfile, profileSummary } from "../lib/llmProfiles";
import type { CleanupLevel, LlmApi, LlmProfile } from "../lib/types";
import type { CardProps } from "./DictationCard";

/** A segmented cleanup level picker. The workspace's New screen uses it for
 *  a per-run level (#157) that never touches the settings. */
export function CleanupLevelPicker({
  value,
  onChange,
  label = "Cleanup level",
}: {
  value: CleanupLevel;
  onChange: (level: CleanupLevel) => void;
  label?: string;
}) {
  return (
    <div className="segmented" role="radiogroup" aria-label={label}>
      {CLEANUP_LEVELS.map((l) => (
        <button
          key={l.value}
          type="button"
          role="radio"
          aria-checked={value === l.value}
          className={value === l.value ? "on" : ""}
          onClick={() => onChange(l.value)}
        >
          {l.label}
        </button>
      ))}
    </div>
  );
}

/** The dictation's cleanup level (saved in the settings). */
export function CleanupLevelControl({ ctl, label = "Cleanup level" }: { ctl: Ctl; label?: string }) {
  const { settings, save } = ctl;
  return (
    <CleanupLevelPicker
      value={settings.cleanup_level}
      onChange={(cleanup_level) => save({ ...settings, cleanup_level })}
      label={label}
    />
  );
}

/** Which LLM profile cleans dictations and long-form runs (#119). */
export function CleanupProfileField({ ctl }: { ctl: Ctl }) {
  const { settings, save } = ctl;
  const current = cleanupProfile(settings);
  return (
    <div className="field">
      <div className="field-label">
        <span>Profile <Tip text="The LLM profile that cleans your dictations, microphone sessions and files, and the local API's /clean. A profile is a server (Ollama or any OpenAI-compatible /v1 server), a model and an optional API key." /></span>
        <small>{current?.external ? "external: text leaves this machine" : "runs on this machine"}</small>
      </div>
      <select
        value={current?.id ?? ""}
        onChange={(e) => save({ ...settings, cleanup_profile: e.target.value })}
        aria-label="Cleanup profile"
      >
        {settings.llm_profiles.map((p) => (
          <option key={p.id} value={p.id}>
            {p.name}
            {p.external ? " (external)" : ""}
          </option>
        ))}
      </select>
    </div>
  );
}

/** The classic window's backend / server / key / model fields, mapped onto
 *  the cleanup profile: today's fields, now editing a profile. */
function ClassicProfileFields({ ctl, profile }: { ctl: Ctl; profile: LlmProfile }) {
  const { settings, setSettings, save, ollamaModels } = ctl;
  const patch = (p: Partial<LlmProfile>) => patchCleanupProfile(settings, p);
  return (
    <>
      <div className="field">
        <div className="field-label"><span>Cleanup backend <Tip text="Which chat API drives cleanup. Ollama (native) is the default. OpenAI-compatible works with any /v1 server — llama.cpp-server, LM Studio, or antirez's DS4 — reusing the Server and model fields below." /></span></div>
        <select
          value={profile.api}
          onChange={(e) => save(patch({ api: e.target.value as LlmApi }))}
          aria-label="Cleanup backend"
        >
          <option value="ollama">{API_LABELS.ollama}</option>
          <option value="openai">{API_LABELS.openai}</option>
        </select>
      </div>

      <div className="field">
        <div className="field-label"><span>Server <Tip text="Address of the cleanup server. Ollama: http://localhost:11434 (default). OpenAI-compatible: the server base URL, e.g. http://localhost:8080 — with or without a trailing /v1." /></span></div>
        <input
          value={profile.base_url}
          onChange={(e) => setSettings(patch({ base_url: e.target.value }))}
          onBlur={() => save(settings)}
          spellCheck={false}
          aria-label="Cleanup server"
        />
        <EndpointNote url={profile.base_url} external={profile.external} />
      </div>

      {profile.api === "openai" && (
        <div className="field">
          <div className="field-label"><span>API key <Tip text="Optional bearer token for the OpenAI-compatible server. Most local servers (llama.cpp, LM Studio, DS4) ignore it — leave empty unless yours requires one." /></span></div>
          <input
            type="password"
            value={profile.api_key}
            onChange={(e) => setSettings(patch({ api_key: e.target.value }))}
            onBlur={() => save(settings)}
            spellCheck={false}
            autoComplete="off"
            aria-label="API key"
          />
        </div>
      )}

      <div className="field">
        <div className="field-label">
          <span>LLM model <Tip text="The model that cleans up the transcript (filler removal, grammar, rewriting). Any small instruct model works — llama3.2:3b is a good default. The list shows what is installed on the server; with Ollama, add more with 'ollama pull'." /></span>
          {ollamaModels === null && <small>server unreachable — type the name</small>}
        </div>
        {ollamaModels ? (
          <select
            value={profile.model}
            onChange={(e) => save(patch({ model: e.target.value }))}
            aria-label="LLM model"
          >
            {!ollamaModels.includes(profile.model) && (
              <option value={profile.model}>
                {profile.model} (not installed)
              </option>
            )}
            {ollamaModels.map((m) => (
              <option key={m} value={m}>{m}</option>
            ))}
          </select>
        ) : (
          <input
            value={profile.model}
            onChange={(e) => setSettings(patch({ model: e.target.value }))}
            onBlur={() => save(settings)}
            spellCheck={false}
            aria-label="LLM model"
          />
        )}
      </div>
    </>
  );
}

export function CleanupCard({
  ctl,
  collapsible,
  onEditProfiles,
}: CardProps & {
  /** Workspace: profiles are edited in Recipes, and this opens it. Without
   *  it (classic window) the cleanup profile's fields are edited inline. */
  onEditProfiles?: () => void;
}) {
  const { settings, setSettings, save, modelAdoptNote, defaultPrompts } = ctl;
  const profile = cleanupProfile(settings);
  return (
    <CollapsibleCard
      storageKey="cleanupOpen"
      title={<>Cleanup <span className="via">via {profile?.name ?? "LLM"}</span></>}
      collapsible={collapsible}
    >
      <div className="field">
        <div className="field-label">
          <span>Level <Tip text="How much the LLM edits your transcript. None: exactly what you said, mistakes included. Light: removes 'um/uh' and fixes grammar. Medium: also tightens for clarity and conciseness. High: rewrites for brevity and polish. If the cleanup server is unreachable you always get the raw transcript." /></span>
          <small>{CLEANUP_LEVELS.find((l) => l.value === settings.cleanup_level)?.hint}</small>
        </div>
        <CleanupLevelControl ctl={ctl} />
      </div>

      <div className="field">
        <div className="field-label">
          <span>Translate to <Tip text="Dictate in one language and get the cleaned text in another — the LLM translates while cleaning. 'Keep language' disables translation. Works even with Cleanup None (translate-only). Something Wispr Flow can't do locally." /></span>
          <small>output language</small>
        </div>
        <select
          value={settings.output_language}
          onChange={(e) => save({ ...settings, output_language: e.target.value })}
          aria-label="Translate to"
        >
          <option value="">Keep language</option>
          {LANGUAGES.filter(([code]) => code !== "auto").map(([code, label]) => (
            <option key={code} value={code}>{label}</option>
          ))}
        </select>
      </div>

      <CleanupProfileField ctl={ctl} />

      {profile && onEditProfiles && (
        <div className="field field-col">
          <div className="row-gap">
            <span className="profile-line">{profileSummary(profile)}</span>
            <button type="button" className="btn-ghost push" onClick={onEditProfiles}>
              Edit profiles in Recipes
            </button>
          </div>
          <EndpointNote url={profile.base_url} external={profile.external} />
        </div>
      )}
      {profile && !onEditProfiles && <ClassicProfileFields ctl={ctl} profile={profile} />}
      {modelAdoptNote && <small className="model-note">✓ {modelAdoptNote}</small>}

      <AdvancedGroup>
        <div className="field field-col">
          <div className="field-label">
            <span>Cleanup prompts <Tip text="Rewrite the per-level instructions sent to the LLM. Leave a box empty to use the built-in default (shown as placeholder). Power-user territory: a bad prompt makes every dictation worse." /></span>
            <small>empty = built-in default</small>
          </div>
          {(["light", "medium", "high"] as const).map((lvl, i) => (
            <div key={lvl} className="prompt-override">
              <span className="prompt-level">{lvl}</span>
              <textarea
                rows={2}
                value={settings.prompt_overrides[lvl]}
                placeholder={defaultPrompts[i]}
                aria-label={`Cleanup prompt, ${lvl}`}
                onChange={(e) =>
                  setSettings({
                    ...settings,
                    prompt_overrides: { ...settings.prompt_overrides, [lvl]: e.target.value },
                  })
                }
                onBlur={() => save(settings)}
                spellCheck={false}
              />
            </div>
          ))}
        </div>
      </AdvancedGroup>
    </CollapsibleCard>
  );
}

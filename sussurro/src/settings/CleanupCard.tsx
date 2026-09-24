import { AdvancedGroup, CollapsibleCard, EndpointNote, Tip } from "../components/ui";
import { CLEANUP_LEVELS, LANGUAGES } from "../lib/constants";
import type { Ctl } from "../hooks/useAppController";
import type { CardProps } from "./DictationCard";

/** The cleanup level control, shared by the Cleanup card and the workspace's
 *  New screen (the level applies to dictation and to long-form runs). */
export function CleanupLevelControl({ ctl, label = "Cleanup level" }: { ctl: Ctl; label?: string }) {
  const { settings, save } = ctl;
  return (
    <div className="segmented" role="radiogroup" aria-label={label}>
      {CLEANUP_LEVELS.map((l) => (
        <button
          key={l.value}
          role="radio"
          aria-checked={settings.cleanup_level === l.value}
          className={settings.cleanup_level === l.value ? "on" : ""}
          onClick={() => save({ ...settings, cleanup_level: l.value })}
        >
          {l.label}
        </button>
      ))}
    </div>
  );
}

export function CleanupCard({ ctl, collapsible }: CardProps) {
  const { settings, setSettings, save, ollamaModels, modelAdoptNote, defaultPrompts } = ctl;
  return (
    <CollapsibleCard
      storageKey="cleanupOpen"
      title={<>Cleanup <span className="via">via Ollama</span></>}
      collapsible={collapsible}
    >
      <div className="field">
        <div className="field-label">
          <span>Level <Tip text="How much the local LLM edits your transcript. None: exactly what you said, mistakes included. Light: removes 'um/uh' and fixes grammar. Medium: also tightens for clarity and conciseness. High: rewrites for brevity and polish. If Ollama is unreachable you always get the raw transcript." /></span>
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

      <div className="field">
        <div className="field-label"><span>Cleanup backend <Tip text="Which chat API drives cleanup. Ollama (native) is the default. OpenAI-compatible works with any /v1 server — llama.cpp-server, LM Studio, or antirez's DS4 — reusing the Server and model fields below." /></span></div>
        <select
          value={settings.cleanup_api}
          onChange={(e) => save({ ...settings, cleanup_api: e.target.value as "ollama" | "openai" })}
          aria-label="Cleanup backend"
        >
          <option value="ollama">Ollama (native)</option>
          <option value="openai">OpenAI-compatible</option>
        </select>
      </div>

      <div className="field">
        <div className="field-label"><span>Server <Tip text="Address of the cleanup server. Ollama: http://localhost:11434 (default). OpenAI-compatible: the server base URL, e.g. http://localhost:8080 — with or without a trailing /v1." /></span></div>
        <input
          value={settings.ollama_url}
          onChange={(e) => setSettings({ ...settings, ollama_url: e.target.value })}
          onBlur={() => save(settings)}
          spellCheck={false}
          aria-label="Cleanup server"
        />
        <EndpointNote url={settings.ollama_url} />
      </div>

      {settings.cleanup_api === "openai" && (
        <div className="field">
          <div className="field-label"><span>API key <Tip text="Optional bearer token for the OpenAI-compatible server. Most local servers (llama.cpp, LM Studio, DS4) ignore it — leave empty unless yours requires one." /></span></div>
          <input
            type="password"
            value={settings.api_key}
            onChange={(e) => setSettings({ ...settings, api_key: e.target.value })}
            onBlur={() => save(settings)}
            spellCheck={false}
            autoComplete="off"
            aria-label="API key"
          />
        </div>
      )}

      <div className="field">
        <div className="field-label">
          <span>LLM model <Tip text="The Ollama model that cleans up the transcript (filler removal, grammar, rewriting). Any small instruct model works — llama3.2:3b is a good default. The list shows what is installed on your Ollama server; add more with 'ollama pull'." /></span>
          {ollamaModels === null && <small>server unreachable — type the name</small>}
        </div>
        {ollamaModels ? (
          <select
            value={settings.ollama_model}
            onChange={(e) => save({ ...settings, ollama_model: e.target.value })}
            aria-label="LLM model"
          >
            {!ollamaModels.includes(settings.ollama_model) && (
              <option value={settings.ollama_model}>
                {settings.ollama_model} (not installed)
              </option>
            )}
            {ollamaModels.map((m) => (
              <option key={m} value={m}>{m}</option>
            ))}
          </select>
        ) : (
          <input
            value={settings.ollama_model}
            onChange={(e) => setSettings({ ...settings, ollama_model: e.target.value })}
            onBlur={() => save(settings)}
            spellCheck={false}
            aria-label="LLM model"
          />
        )}
        {modelAdoptNote && (
          <small className="model-note">✓ {modelAdoptNote}</small>
        )}
      </div>

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

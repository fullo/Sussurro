import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Card, EndpointNote, Tip } from "../components/ui";
import type { Ctl } from "../hooks/useAppController";
import {
  API_LABELS,
  cleanupProfile,
  commitProfile,
  isBundled,
  keyStorageBadge,
  keyStorageWarning,
  newProfile,
  profileProblems,
  profileSummary,
  removeProfile,
  withApi,
  withBaseUrl,
} from "../lib/llmProfiles";
import type { CredentialStoreStatus, LlmApi, LlmProfile } from "../lib/types";
import { BundledProfilePanel } from "../settings/BundledLlm";
import { RecipesCard } from "./RecipesCard";

/** Recipes (proposal A rail): the recipes — named prompts that write a
 *  companion document next to a transcript or an answer (#120) — above the
 *  LLM profiles they run on (#119). */
export function RecipesScreen({ ctl }: { ctl: Ctl }) {
  const { settings } = ctl;
  /** The profile open in the editor: a saved one, or a new draft (id ""). */
  const [editing, setEditing] = useState<LlmProfile | null>(null);
  const cleanupId = cleanupProfile(settings)?.id;

  return (
    <div className="sh-screen">
      <header className="sh-topbar">
        <h1>Recipes</h1>
        <span className="sh-muted">Prompts, and the LLM profiles they run on</span>
      </header>
      <div className="sh-scroll cards-col">
        <RecipesCard ctl={ctl} />
        <Card
          title={<>LLM profiles <span className="via">Ollama or OpenAI-compatible</span></>}
          headerExtra={
            <button
              type="button"
              className="btn-ghost"
              onClick={() => setEditing(newProfile(settings.llm_profiles))}
              disabled={editing?.id === ""}
            >
              Add profile
            </button>
          }
        >
          <p className="card-hint">
            A profile is a server, a model and an optional API key: Ollama, or any server with an OpenAI-compatible{" "}
            <code>/v1</code> API (llama.cpp's llama-server, LM Studio, DS4, a hosted service). Cleanup uses the
            profile marked <em>Cleanup</em>; change it here or in Settings → Cleanup.
          </p>
          <ul className="prof-list" aria-label="LLM profiles">
            {settings.llm_profiles.map((p) => (
              <li key={p.id}>
                <button
                  type="button"
                  className={`prof-row${editing?.id === p.id ? " active" : ""}`}
                  aria-expanded={editing?.id === p.id}
                  onClick={() => setEditing(editing?.id === p.id ? null : p)}
                >
                  <span className="prof-name">{p.name}</span>
                  <span className="prof-sub">{profileSummary(p)}</span>
                  {p.id === cleanupId && <span className="tb note">Cleanup</span>}
                  {isBundled(p) && (
                    <span className="tb note" title="Built into Sussurro: nothing to install or configure">Built in</span>
                  )}
                  {keyStorageBadge(p) && (
                    <span className="ext" title="Open the profile for details">{keyStorageBadge(p)}</span>
                  )}
                  {p.external ? (
                    <span className="ext" title="Text sent to this profile leaves this machine">↗ External</span>
                  ) : (
                    <span className="badge-local" title="Runs on this machine">Local</span>
                  )}
                </button>
              </li>
            ))}
          </ul>
          {editing && isBundled(editing) && <BundledProfilePanel ctl={ctl} onDone={() => setEditing(null)} />}
          {editing && !isBundled(editing) && (
            <ProfileEditor
              key={editing.id || "new"}
              ctl={ctl}
              initial={editing}
              onDone={() => setEditing(null)}
            />
          )}
        </Card>
      </div>
    </div>
  );
}

type TestState = { state: "idle" | "testing" | "ok" | "error"; message: string };

function ProfileEditor({ ctl, initial, onDone }: { ctl: Ctl; initial: LlmProfile; onDone: () => void }) {
  const { settings, save, flash } = ctl;
  const [draft, setDraft] = useState(initial);
  /** Models the server listed at the last test; null = not tested / failed. */
  const [models, setModels] = useState<string[] | null>(null);
  const [test, setTest] = useState<TestState>({ state: "idle", message: "" });
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [saving, setSaving] = useState(false);
  /** The OS credential store, for the API key warning (#159); null = unknown. */
  const [store, setStore] = useState<CredentialStoreStatus | null>(null);

  const isNew = initial.id === "";
  const isCleanup = !isNew && cleanupProfile(settings)?.id === initial.id;
  const problems = profileProblems(draft, settings.llm_profiles);
  const dirty = JSON.stringify(draft) !== JSON.stringify(initial);
  const next = isNew ? null : removeProfile(settings, initial.id);
  const successor = next ? cleanupProfile(next) : null;

  const edit = (p: LlmProfile) => {
    // A different server or API makes the last test meaningless.
    if (p.base_url !== draft.base_url || p.api !== draft.api || p.api_key !== draft.api_key) {
      setModels(null);
      setTest({ state: "idle", message: "" });
    }
    setDraft(p);
  };

  const testConnection = async (profile: LlmProfile, quiet = false) => {
    setTest({ state: "testing", message: "Connecting…" });
    try {
      const listed = await invoke<string[]>("llm_list_models", { profile });
      setModels(listed);
      setTest({
        state: "ok",
        message: listed.length
          ? `Connected: ${listed.length} model${listed.length === 1 ? "" : "s"} available.`
          : "Connected, but the server lists no models.",
      });
      if (!profile.model.trim() && listed.length) setDraft((d) => ({ ...d, model: listed[0] }));
    } catch (e) {
      setModels(null);
      setTest(quiet ? { state: "idle", message: "" } : { state: "error", message: `Could not connect: ${e}` });
    }
  };

  // A saved profile shows its models right away when the server answers.
  useEffect(() => {
    invoke<CredentialStoreStatus>("credential_store_status").then(setStore).catch(() => {});
    if (!isNew) testConnection(initial, true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const commit = async (useForCleanup: boolean) => {
    setSaving(true);
    const { settings: saved, profile } = commitProfile(settings, draft);
    const ok = await save(useForCleanup ? { ...saved, cleanup_profile: profile.id } : saved);
    setSaving(false);
    if (ok) {
      flash(useForCleanup ? `Cleanup now uses “${profile.name}”.` : `Profile “${profile.name}” saved.`);
      onDone();
    }
  };

  const remove = async () => {
    if (!next) return;
    if (await save(next)) {
      flash(`Profile “${initial.name}” deleted.`);
      onDone();
    }
  };

  const keyWarning = keyStorageWarning(draft, isNew ? null : initial, store);
  const modelMissing = models !== null && draft.model !== "" && !models.includes(draft.model);

  return (
    <div className="prof-editor" role="group" aria-label={isNew ? "New profile" : `Edit ${initial.name}`}>
      <div className="field">
        <div className="field-label"><span>Name</span></div>
        <input
          value={draft.name}
          onChange={(e) => edit({ ...draft, name: e.target.value })}
          aria-label="Profile name"
          autoFocus={isNew}
        />
      </div>

      <div className="field">
        <div className="field-label">
          <span>API <Tip text="Ollama (native) talks to Ollama's own API. OpenAI-compatible works with any /v1 server: llama.cpp's llama-server, LM Studio, DS4, Ollama's /v1, or a hosted service." /></span>
        </div>
        <select
          value={draft.api}
          onChange={(e) => edit(withApi(draft, e.target.value as LlmApi))}
          aria-label="API"
        >
          <option value="ollama">{API_LABELS.ollama}</option>
          <option value="openai">{API_LABELS.openai}</option>
        </select>
      </div>

      <div className="field field-col">
        <div className="field-label">
          <span>Server <Tip text="Ollama: http://localhost:11434. OpenAI-compatible: the server's base URL, with or without a trailing /v1 (e.g. http://localhost:8080/v1)." /></span>
        </div>
        <input
          value={draft.base_url}
          onChange={(e) => edit(withBaseUrl(draft, e.target.value))}
          spellCheck={false}
          aria-label="Server"
        />
        <label className="check-row">
          <input
            type="checkbox"
            checked={draft.external}
            onChange={(e) => edit({ ...draft, external: e.target.checked })}
          />
          <span>
            External: text sent to this profile leaves this machine
            <small> Set from the server address; change it if you know better (a box on your own network, say).</small>
          </span>
        </label>
        <EndpointNote url={draft.base_url} external={draft.external} />
      </div>

      {draft.api === "openai" && (
        <>
        <div className="field">
          <div className="field-label">
            <span>API key <Tip text="Optional bearer token. Local servers usually ignore it; hosted services need one. Kept in the system keychain (macOS Keychain, Windows Credential Manager, or the Secret Service on Linux), not in Sussurro's settings file." /></span>
            <small>optional</small>
          </div>
          <input
            type="password"
            value={draft.api_key}
            onChange={(e) => edit({ ...draft, api_key: e.target.value })}
            spellCheck={false}
            autoComplete="off"
            aria-label="API key"
          />
        </div>
        {keyWarning && <small className="endpoint-note" role="note">⚠ {keyWarning}</small>}
        </>
      )}

      <div className="field">
        <div className="field-label">
          <span>Model</span>
          {modelMissing && <small>not on the server</small>}
        </div>
        {models && models.length > 0 ? (
          <select value={draft.model} onChange={(e) => edit({ ...draft, model: e.target.value })} aria-label="Model">
            {!models.includes(draft.model) && <option value={draft.model}>{draft.model || "Choose a model"}</option>}
            {models.map((m) => (
              <option key={m} value={m}>{m}</option>
            ))}
          </select>
        ) : (
          <input
            value={draft.model}
            onChange={(e) => edit({ ...draft, model: e.target.value })}
            placeholder={draft.api === "ollama" ? "llama3.2:3b" : "model id"}
            spellCheck={false}
            aria-label="Model"
          />
        )}
        <button
          type="button"
          className="btn-ghost"
          onClick={() => testConnection(draft)}
          disabled={test.state === "testing" || !draft.base_url.trim()}
        >
          {test.state === "testing" && <span className="btn-spinner" aria-hidden="true" />}
          Test connection
        </button>
      </div>
      <p className={`prof-test ${test.state}`} role="status">{test.message}</p>

      <div className="field">
        <div className="field-label">
          <span>Context window <Tip text="How many tokens the model can read at once. Recipes split long transcripts into parts that fit, then combine them. Leave empty if unsure: Sussurro assumes 4096, safe for small local models. On Ollama the model is loaded with this window." /></span>
          <small>tokens · optional</small>
        </div>
        <input
          type="number"
          min={0}
          step={1024}
          inputMode="numeric"
          value={draft.context_tokens ? String(draft.context_tokens) : ""}
          placeholder="4096"
          onChange={(e) => edit({ ...draft, context_tokens: Math.max(0, Math.floor(Number(e.target.value) || 0)) })}
          aria-label="Context window in tokens"
        />
      </div>

      {problems.length > 0 && (dirty || isNew) && (
        <ul className="prof-problems">
          {problems.map((p) => <li key={p}>{p}</li>)}
        </ul>
      )}

      {confirmDelete ? (
        <div className="row-gap prof-actions" role="alertdialog" aria-label="Confirm delete">
          <span>
            Delete “{initial.name}”?
            {isCleanup && successor ? ` Cleanup will use “${successor.name}”.` : ""}
          </span>
          <button type="button" className="btn-danger sh-btn push" onClick={remove}>Delete</button>
          <button type="button" className="btn-ghost" onClick={() => setConfirmDelete(false)}>Keep</button>
        </div>
      ) : (
        <div className="row-gap prof-actions">
          <button
            type="button"
            className="btn-dark sh-btn"
            disabled={saving || problems.length > 0 || (!dirty && !isNew)}
            onClick={() => commit(false)}
          >
            {isNew ? "Add profile" : "Save"}
          </button>
          {!isCleanup && (
            <button
              type="button"
              className="btn-ghost"
              disabled={saving || problems.length > 0}
              onClick={() => commit(true)}
            >
              {dirty || isNew ? "Save and use for cleanup" : "Use for cleanup"}
            </button>
          )}
          <button type="button" className="btn-ghost" onClick={onDone}>
            {dirty || isNew ? "Cancel" : "Close"}
          </button>
          {!isNew && (
            <button
              type="button"
              className="btn-ghost push"
              disabled={!next}
              title={next ? undefined : "The last profile can't be deleted"}
              onClick={() => setConfirmDelete(true)}
            >
              Delete…
            </button>
          )}
        </div>
      )}
    </div>
  );
}

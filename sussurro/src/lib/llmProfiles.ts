/* LLM profiles (#119): pure helpers shared by the profile editor (Recipes),
   Settings → Cleanup and the app controller. The Rust side (llm/profile.rs,
   Settings::normalize) repairs whatever reaches it; these keep the UI's
   edits well-formed in the first place. */

import { isLocalEndpoint, parseEndpoint } from "./endpoint";
import type { LlmApi, LlmProfile, Settings } from "./types";

/** Suggested server per API: Ollama's port, llama.cpp-server's default. */
export const DEFAULT_URLS: Record<LlmApi, string> = {
  ollama: "http://localhost:11434",
  openai: "http://localhost:8080/v1",
};

export const API_LABELS: Record<LlmApi, string> = {
  ollama: "Ollama (native)",
  openai: "OpenAI-compatible",
};

/** Whether text sent to `url` leaves this machine (mirrors llm::infer_external). */
export function inferExternal(url: string): boolean {
  return !isLocalEndpoint(url);
}

type ProfileSettings = Pick<Settings, "llm_profiles" | "cleanup_profile">;

/** The profile cleanup runs on: the selected one, else the first (as in
 *  Settings::cleanup_llm), else null when there are none. */
export function cleanupProfile(s: ProfileSettings): LlmProfile | null {
  return s.llm_profiles.find((p) => p.id === s.cleanup_profile) ?? s.llm_profiles[0] ?? null;
}

/** A new server address: `external` is inferred again, replacing a manual override. */
export function withBaseUrl(p: LlmProfile, base_url: string): LlmProfile {
  return { ...p, base_url, external: inferExternal(base_url) };
}

/** Switch API. A server still at the other API's suggested address (or
 *  empty) follows to this API's suggestion; a custom address is kept. */
export function withApi(p: LlmProfile, api: LlmApi): LlmProfile {
  if (p.api === api) return p;
  const url = p.base_url.trim();
  const suggested = !url || Object.values(DEFAULT_URLS).includes(url);
  const next = { ...p, api };
  return suggested ? withBaseUrl(next, DEFAULT_URLS[api]) : next;
}

/** Patch a profile; a `base_url` change re-infers `external` unless the
 *  patch sets `external` itself. */
export function patchProfile(p: LlmProfile, patch: Partial<LlmProfile>): LlmProfile {
  let next = { ...p, ...patch };
  if (patch.base_url !== undefined && patch.base_url !== p.base_url && patch.external === undefined) {
    next = withBaseUrl(next, patch.base_url);
  }
  return next;
}

function slug(name: string): string {
  return name
    .toLowerCase()
    .normalize("NFKD")
    .replace(/[̀-ͯ]/g, "")
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 32);
}

/** An id derived from `name`, unique among `existing`. */
export function newProfileId(existing: LlmProfile[], name: string): string {
  const ids = new Set(existing.map((p) => p.id));
  const base = slug(name) || "profile";
  if (!ids.has(base)) return base;
  for (let n = 2; ; n++) {
    const id = `${base}-${n}`;
    if (!ids.has(id)) return id;
  }
}

/** `name`, or `name 2`, `name 3`… — the first not used by another profile. */
export function uniqueName(existing: LlmProfile[], name: string): string {
  const names = new Set(existing.map((p) => p.name.trim().toLowerCase()));
  if (!names.has(name.toLowerCase())) return name;
  for (let n = 2; ; n++) {
    const candidate = `${name} ${n}`;
    if (!names.has(candidate.toLowerCase())) return candidate;
  }
}

/** A blank profile for the editor ("Add profile"). Not saved until the user
 *  saves it; its id is fixed from the name at save time (`commitProfile`). */
export function newProfile(existing: LlmProfile[], api: LlmApi = "openai"): LlmProfile {
  const name = uniqueName(existing, "New profile");
  return {
    id: "",
    name,
    api,
    base_url: DEFAULT_URLS[api],
    api_key: "",
    model: "",
    external: inferExternal(DEFAULT_URLS[api]),
  };
}

/** Why a profile can't be saved yet, in display order; empty when it can. */
export function profileProblems(p: LlmProfile, others: LlmProfile[]): string[] {
  const out: string[] = [];
  const name = p.name.trim();
  if (!name) out.push("Give the profile a name.");
  else if (others.some((o) => o.id !== p.id && o.name.trim().toLowerCase() === name.toLowerCase()))
    out.push(`Another profile is already called “${name}”.`);
  if (!p.base_url.trim()) out.push("Enter the server address.");
  if (!p.model.trim()) out.push("Choose a model.");
  return out;
}

/** Save `p` into the settings: replace the profile with its id, or append
 *  a new one (an empty id gets one from its name). Returns the settings and
 *  the saved profile. Names and fields are trimmed. */
export function commitProfile(s: Settings, p: LlmProfile): { settings: Settings; profile: LlmProfile } {
  const clean: LlmProfile = {
    ...p,
    name: p.name.trim(),
    base_url: p.base_url.trim(),
    api_key: p.api_key.trim(),
    model: p.model.trim(),
  };
  const exists = clean.id !== "" && s.llm_profiles.some((o) => o.id === clean.id);
  if (exists) {
    return {
      settings: { ...s, llm_profiles: s.llm_profiles.map((o) => (o.id === clean.id ? clean : o)) },
      profile: clean,
    };
  }
  const added = { ...clean, id: newProfileId(s.llm_profiles, clean.name) };
  return { settings: { ...s, llm_profiles: [...s.llm_profiles, added] }, profile: added };
}

/** Delete a profile. The last one can't go (null). Deleting the cleanup
 *  profile moves cleanup to the first remaining one. */
export function removeProfile(s: Settings, id: string): Settings | null {
  const rest = s.llm_profiles.filter((p) => p.id !== id);
  if (rest.length === 0 || rest.length === s.llm_profiles.length) return null;
  const cleanup_profile = s.cleanup_profile === id ? rest[0].id : s.cleanup_profile;
  return { ...s, llm_profiles: rest, cleanup_profile };
}

/** Patch the cleanup profile in place (the classic window's Server / model /
 *  API key fields edit it this way). */
export function patchCleanupProfile(s: Settings, patch: Partial<LlmProfile>): Settings {
  const current = cleanupProfile(s);
  if (!current) return s;
  return {
    ...s,
    llm_profiles: s.llm_profiles.map((p) => (p.id === current.id ? patchProfile(p, patch) : p)),
  };
}

/** Whether cleanup would talk to a different server (reload its model list). */
export function cleanupServerChanged(prev: ProfileSettings | null, next: ProfileSettings): boolean {
  const a = prev ? cleanupProfile(prev) : null;
  const b = cleanupProfile(next);
  if (!a || !b) return a !== b;
  return a.id !== b.id || a.api !== b.api || a.base_url !== b.base_url || a.api_key !== b.api_key;
}

/** "localhost:11434" style host for lists; the raw URL when it won't parse. */
export function profileHost(p: LlmProfile): string {
  return parseEndpoint(p.base_url)?.host ?? p.base_url;
}

/** "Ollama · llama3.2:3b" / "api.example.com · gpt-4o-mini" */
export function profileSummary(p: LlmProfile): string {
  const where = p.external ? profileHost(p) : p.api === "ollama" ? "Ollama" : "OpenAI-compatible";
  return p.model ? `${where} · ${p.model}` : where;
}

/** Whether `model` is among the server's models, Ollama-style
 *  (`llama3.2` matches `llama3.2:latest`). */
export function modelListed(models: string[], model: string): boolean {
  return models.some((m) => m === model || m.startsWith(`${model}:`));
}

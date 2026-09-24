/* Privacy gate for external LLM profiles (#122): pure helpers behind the
   per-run confirmation dialog, the cleanup opt-in in Settings → Cleanup
   and the "sent externally" marker in the Library. The backend enforces
   all of it (consent tokens, the opt-in check in cleanup); these only
   decide what the UI shows and asks. */

import { parseEndpoint } from "./endpoint";
import { cleanupProfile } from "./llmProfiles";
import type { ExternalRunConsent, ExternalRunPreview, LlmProfile, Settings } from "./types";

/** Host of a profile's server, lowercased (mirrors LlmProfile::host). */
export function profileHostOf(p: Pick<LlmProfile, "base_url">): string {
  return parseEndpoint(p.base_url)?.host ?? "";
}

/** Whether a run on `p` needs the per-run confirmation. */
export function needsConsent(p: Pick<LlmProfile, "external"> | null | undefined): boolean {
  return !!p?.external;
}

/** 12345 → "12,345". */
export function formatCount(n: number): string {
  return Math.max(0, Math.round(n)).toLocaleString("en-US");
}

/** "≈ 12,345 characters (≈ 3,087 tokens)" */
export function sizeLabel(chars: number, tokens: number): string {
  return `≈ ${formatCount(chars)} characters (≈ ${formatCount(tokens)} tokens)`;
}

/** Tooltip of the "sent externally" marker: the hosts, or "" when none. */
export function externalHostsTitle(hosts: string[] | undefined): string {
  const list = (hosts ?? []).filter(Boolean);
  if (!list.length) return "";
  return `Sent to an external LLM: ${list.join(", ")}`;
}

/** Whether an item's text ever went to an external host. */
export function sentExternally(item: { external_hosts?: string[] }): boolean {
  return (item.external_hosts ?? []).length > 0;
}

/** Cleanup calls an LLM at all (mirrors Settings::cleanup_active): a level
 *  other than None, or a translation. */
export function cleanupActive(s: Pick<Settings, "cleanup_level" | "output_language">): boolean {
  const lang = (s.output_language ?? "").trim();
  return s.cleanup_level !== "none" || (lang !== "" && lang !== "same");
}

/** Whether cleanup may send to `p` (mirrors LlmProfile::cleanup_allowed):
 *  always when local; when external, only with the opt-in for its host. */
export function cleanupAllowed(p: LlmProfile): boolean {
  if (!p.external) return true;
  const host = profileHostOf(p);
  return host !== "" && (p.cleanup_opt_in ?? "").trim().toLowerCase() === host;
}

export type CleanupGate =
  /** Cleanup runs on this machine (or no profile at all). */
  | { state: "local" }
  /** External profile, opted in: dictations go to `host` for cleanup. */
  | { state: "allowed"; host: string; profile: LlmProfile }
  /** External profile without the opt-in: cleanup keeps the raw text. */
  | { state: "blocked"; host: string; profile: LlmProfile };

/** Where cleanup stands for the privacy notice in Settings → Cleanup. */
export function cleanupGate(s: Settings): CleanupGate {
  const p = cleanupProfile(s);
  if (!p || !p.external) return { state: "local" };
  const host = profileHostOf(p) || p.base_url.trim();
  return cleanupAllowed(p) ? { state: "allowed", host, profile: p } : { state: "blocked", host, profile: p };
}

/** Give or withdraw the cleanup opt-in of profile `id` (bound to its
 *  current host). */
export function withCleanupOptIn(s: Settings, id: string, on: boolean): Settings {
  return {
    ...s,
    llm_profiles: s.llm_profiles.map((p) => {
      if (p.id !== id) return p;
      const next = { ...p };
      if (on && profileHostOf(p)) next.cleanup_opt_in = profileHostOf(p);
      else delete next.cleanup_opt_in;
      return next;
    }),
  };
}

type Call = (cmd: string, args: Record<string, unknown>) => Promise<unknown>;

/** Get what a run on `profile` needs before it starts (#122):
 *  - local profile → `{ consent: null }`, nothing asked, nothing called;
 *  - external profile → the preview is fetched and shown (`confirm`); on
 *    OK a fresh one-time token is issued and returned; on Cancel → null
 *    (the run must not start). Asked every time — nothing is remembered.
 *  Errors (item gone, profile removed…) propagate. */
export async function obtainConsent(
  call: Call,
  confirm: (preview: ExternalRunPreview) => Promise<boolean>,
  profile: LlmProfile,
  req: ConsentRequest,
): Promise<{ consent: string | null } | null> {
  if (!needsConsent(profile)) return { consent: null };
  const args = consentArgs(req);
  const preview = (await call("external_run_preview", args)) as ExternalRunPreview;
  if (!(await confirm(preview))) return null;
  const granted = (await call("prepare_external_run", args)) as ExternalRunConsent;
  return { consent: granted.token };
}

/** Arguments of `external_run_preview` / `prepare_external_run`. */
export interface ConsentRequest {
  id: string;
  recipeId: string | null;
  question: string | null;
  profileId: string;
}

/** The invoke() arguments for a consent request (Tauri maps camelCase to
 *  the command's snake_case parameters). */
export function consentArgs(r: ConsentRequest): Record<string, unknown> {
  return r.question !== null
    ? { id: r.id, recipeId: null, question: r.question, profileId: r.profileId }
    : { id: r.id, recipeId: r.recipeId, question: null, profileId: r.profileId };
}

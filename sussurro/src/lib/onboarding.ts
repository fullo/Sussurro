/* First-run onboarding (#115): pure helpers for the guided setup and the
   "What's new" screen (shell/Onboarding.tsx). */

import { commitProfile, uniqueName } from "./llmProfiles";
import type { LlmApi, LlmProfile, Permissions, PermState, Settings } from "./types";

/** What the window opens with: the full setup on a fresh install, one
 *  "What's new" screen for a user upgrading from 0.6.x, nothing once done. */
export type OnboardingMode = "welcome" | "whats_new";

export function onboardingMode(s: Pick<Settings, "onboarding">): OnboardingMode | null {
  return s.onboarding === "welcome" || s.onboarding === "whats_new" ? s.onboarding : null;
}

/** The guided setup, in order. Every step can be skipped. */
export const SETUP_STEPS = ["welcome", "permissions", "archive", "models", "cleanup", "hotkey"] as const;
export type SetupStep = (typeof SETUP_STEPS)[number];

export const STEP_TITLES: Record<SetupStep, string> = {
  welcome: "Welcome",
  permissions: "Permissions",
  archive: "Archive folder",
  models: "Speech model",
  cleanup: "Cleanup",
  hotkey: "Dictation shortcut",
};

/** The step after `step`; null after the last one (the setup is finished). */
export function stepAfter(step: SetupStep): SetupStep | null {
  const i = SETUP_STEPS.indexOf(step);
  return i >= 0 && i < SETUP_STEPS.length - 1 ? SETUP_STEPS[i + 1] : null;
}

/** The step before `step`; null on the first one. */
export function stepBefore(step: SetupStep): SetupStep | null {
  const i = SETUP_STEPS.indexOf(step);
  return i > 0 ? SETUP_STEPS[i - 1] : null;
}

/* ---------- Permissions ---------- */

export interface PermissionRow {
  id: "microphone" | "accessibility";
  label: string;
  /** Why Sussurro needs it, or where it stands. */
  detail: string;
  ok: boolean;
  /** "ask": a short microphone test makes the OS ask; "settings": only the
   *  OS privacy pane can change it; null: nothing to do. */
  action: "ask" | "settings" | null;
}

/** The permission step's rows. Accessibility only where the OS gates paste
 *  injection behind it (macOS); a check that failed shows nothing. */
export function permissionRows(p: Permissions | null): PermissionRow[] {
  if (!p) return [];
  const rows: PermissionRow[] = [];
  const mic: Record<PermState, Omit<PermissionRow, "id" | "label">> = {
    granted: { detail: "Allowed.", ok: true, action: null },
    not_applicable: { detail: "This system doesn't ask: nothing to allow.", ok: true, action: null },
    unknown: {
      detail: "Not asked yet. A short microphone test makes the system ask now, instead of at your first dictation.",
      ok: false,
      action: "ask",
    },
    denied: {
      detail: "Denied: dictation and recordings can't hear you until you allow it in the system settings.",
      ok: false,
      action: "settings",
    },
  };
  rows.push({ id: "microphone", label: "Microphone", ...mic[p.microphone] });
  if (p.accessibility !== "not_applicable") {
    const granted = p.accessibility === "granted";
    rows.push({
      id: "accessibility",
      label: "Accessibility",
      detail: granted
        ? "Allowed: dictated text is pasted where your cursor is."
        : "Needed to paste dictated text into other apps. Allow Sussurro in the system settings, then check again.",
      ok: granted,
      action: granted ? null : "settings",
    });
  }
  return rows;
}

/** Whether this is macOS, where the first access to Documents shows a
 *  system prompt (the archive step asks for it on purpose). */
export function isMac(platform: string = typeof navigator !== "undefined" ? navigator.userAgent : ""): boolean {
  return /Mac/i.test(platform);
}

/** The sync service a folder visibly belongs to, from its path. macOS
 *  "Desktop & Documents in iCloud" keeps `~/Documents` as the path, so the
 *  archive step always carries the general note as well. */
export function syncedFolder(path: string): string | null {
  if (/[\\/]Library[\\/]Mobile Documents[\\/]|iCloud ?Drive/i.test(path)) return "iCloud Drive";
  if (/OneDrive/i.test(path)) return "OneDrive";
  if (/Dropbox/i.test(path)) return "Dropbox";
  if (/Google ?Drive|GoogleDrive|My Drive/i.test(path)) return "Google Drive";
  return null;
}

/* ---------- Cleanup servers ---------- */

/** LLM servers on this computer the cleanup step looks for, at their
 *  default addresses: Ollama (what the default "Local" profile points at)
 *  and two common OpenAI-compatible ones. Nothing remote is ever probed. */
export const LOCAL_SERVERS: { name: string; api: LlmApi; base_url: string }[] = [
  { name: "Ollama", api: "ollama", base_url: "http://localhost:11434" },
  { name: "LM Studio", api: "openai", base_url: "http://localhost:1234/v1" },
  { name: "llama.cpp server", api: "openai", base_url: "http://localhost:8080/v1" },
];

export type LocalServer = (typeof LOCAL_SERVERS)[number];

/** A throwaway profile for `llm_list_models` (nothing is saved). */
export function probeProfile(server: LocalServer): LlmProfile {
  return { id: "", name: server.name, api: server.api, base_url: server.base_url, api_key: "", model: "", external: false };
}

/** `http://localhost:1234/v1/` and `http://127.0.0.1:1234` are the same server. */
function serverKey(url: string): string {
  return url
    .trim()
    .toLowerCase()
    .replace(/\/+$/, "")
    .replace(/\/v1$/, "")
    .replace("://127.0.0.1", "://localhost");
}

/** The profile already pointing at this server, if any. */
export function profileForServer(s: Pick<Settings, "llm_profiles">, server: LocalServer): LlmProfile | null {
  const key = serverKey(server.base_url);
  return s.llm_profiles.find((p) => p.api === server.api && serverKey(p.base_url) === key) ?? null;
}

/** A small instruct model if there is one (the controller's own pick for
 *  Ollama), else the first. */
export function pickCleanupModel(models: string[]): string {
  return models.find((m) => /llama3\.2|qwen2\.5|gemma|phi3|mistral|instruct/i.test(m)) ?? models[0] ?? "";
}

/** Settings with `server` cleaning dictations: its existing profile (given a
 *  model when it has none), else a new local profile named after it. */
export function withCleanupServer(s: Settings, server: LocalServer, models: string[]): Settings {
  const existing = profileForServer(s, server);
  if (existing) {
    const model = existing.model.trim() ? existing.model : pickCleanupModel(models);
    const { settings } = commitProfile(s, { ...existing, model });
    return { ...settings, cleanup_profile: existing.id };
  }
  const draft: LlmProfile = { ...probeProfile(server), name: uniqueName(s.llm_profiles, server.name), model: pickCleanupModel(models) };
  const { settings, profile } = commitProfile(s, draft);
  return { ...settings, cleanup_profile: profile.id };
}

/* ---------- What's new (upgrade from 0.6.x) ---------- */

export const WHATS_NEW: { title: string; text: string }[] = [
  {
    title: "A workspace",
    text: "This window now has a sidebar: New, Library, People, Recipes, Models and Settings. Dictation works exactly as before — hotkey, speak, pasted — and all your settings are kept.",
  },
  {
    title: "Notes and transcriptions",
    text: "New records long notes from the microphone and transcribes audio files and links. Everything is saved as markdown in an archive folder you can open with any editor.",
  },
  {
    title: "Recipes and LLM profiles",
    text: "Turn a transcript into a summary, action items or meeting minutes, or ask it a question. Your cleanup server is now an LLM profile, under Recipes.",
  },
  {
    title: "Meetings (preview)",
    text: "With the browser extension or this computer's sound, record meetings with speaker labels and subtitles. Off until you turn it on.",
  },
  {
    title: "Command mode is gone",
    text: "The second hotkey is removed; your system's voice control covers it. Spoken editing commands inside dictation (“new line”, “scratch that”) still work.",
  },
];

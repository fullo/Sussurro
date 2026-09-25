/* The meeting's language (#288): the side panel's selector next to Start.
 *
 * - The choices come from the app, `GET /app/languages` (token route, like
 *   `/app/version`): the active engine's languages with their native names
 *   and the dictation's language. An app without the route (404, older than
 *   #288) gets no selector and no `start.language` — it transcribes with
 *   its dictation setting, as before.
 * - The choice is remembered per meeting platform in `storage.local` under
 *   one key (`meetingLanguage`, `{meet: "en", teams: "it", …}`; pages that
 *   are no known platform share `other`). Not a secret: content scripts can
 *   read it (#217), and that's fine.
 * - The first time on a platform the app's dictation language is picked;
 *   a remembered code the engine no longer offers falls back the same way,
 *   and else to Auto-detect.
 *
 * The background sends the chosen code as `start {language}` (`auto` to
 * detect) and re-sends it on a reconnect. */
import browser from "webextension-polyfill";
import { appUrl, authHeaders, type Pairing } from "./pairing";
import type { Platform } from "./platform";

/** Detect the language: always offered, never listed by the app. */
export const AUTO = "auto";
export const AUTO_LABEL = "Auto-detect";

/** `storage.local` key of the per-platform choice. */
export const LANGUAGE_KEY = "meetingLanguage";

export interface AppLanguage {
  code: string;
  /** In the language itself ("Italiano"). */
  name: string;
}

export interface AppLanguages {
  /** `whisper` | `parakeet` | `qwen3_asr`. */
  engine: string;
  /** The dictation's language (`auto` or a code). */
  default: string;
  languages: AppLanguage[];
}

/** A code the app could accept: `auto`, or 2–3 lowercase letters. Pure. */
export function isLanguageCode(v: unknown): v is string {
  return typeof v === "string" && (v === AUTO || /^[a-z]{2,3}$/.test(v));
}

const MAX_LANGUAGES = 200;
const MAX_NAME = 64;

/** The app's answer, checked (it only feeds a `<select>`, but stays
 *  bounded). `null` when it isn't the expected shape. Pure. */
export function parseLanguages(body: unknown): AppLanguages | null {
  if (!body || typeof body !== "object") return null;
  const b = body as Record<string, unknown>;
  if (!Array.isArray(b.languages)) return null;
  const seen = new Set<string>();
  const languages: AppLanguage[] = [];
  for (const l of b.languages.slice(0, MAX_LANGUAGES)) {
    const { code, name } = (l ?? {}) as Record<string, unknown>;
    if (!isLanguageCode(code) || code === AUTO || seen.has(code)) continue;
    seen.add(code);
    languages.push({ code, name: typeof name === "string" && name.trim() ? name.trim().slice(0, MAX_NAME) : code });
  }
  return {
    engine: typeof b.engine === "string" ? b.engine.slice(0, 32) : "",
    default: isLanguageCode(b.default) ? b.default : AUTO,
    languages,
  };
}

type Fetch = (input: string, init?: RequestInit) => Promise<Response>;

/** `GET /app/languages`. `null`: an app without it, or no usable answer —
 *  the panel then shows no selector. Never throws. */
export async function fetchLanguages(p: Pairing, fetchImpl: Fetch = fetch, timeoutMs = 4000): Promise<AppLanguages | null> {
  try {
    const res = await fetchImpl(appUrl(p.port, "/app/languages"), {
      headers: authHeaders(p),
      cache: "no-store",
      credentials: "omit",
      signal: AbortSignal.timeout(timeoutMs),
    });
    if (!res.ok) return null;
    return parseLanguages(await res.json());
  } catch {
    return null;
  }
}

/** The storage key of a platform's choice. Pure. */
export function platformKey(platform: Platform | null | undefined): string {
  return platform ?? "other";
}

/** What the selector shows: the platform's remembered choice when the
 *  engine still offers it, else the app's dictation language, else
 *  Auto-detect. Pure. */
export function chooseLanguage(remembered: unknown, list: AppLanguages): string {
  const offered = (c: unknown): c is string => isLanguageCode(c) && (c === AUTO || list.languages.some((l) => l.code === c));
  if (offered(remembered)) return remembered;
  if (offered(list.default)) return list.default;
  return AUTO;
}

/** The selector's options: Auto-detect first, then the languages by
 *  native name. Pure. */
export function languageOptions(list: AppLanguages): AppLanguage[] {
  const sorted = [...list.languages].sort((a, b) => a.name.localeCompare(b.name));
  return [{ code: AUTO, name: AUTO_LABEL }, ...sorted];
}

/** A code as the panel names it ("English", "Auto-detect"); the code when
 *  the list doesn't know it. Pure. */
export function languageLabel(code: string, list: AppLanguages | null | undefined): string {
  if (code === AUTO) return AUTO_LABEL;
  return list?.languages.find((l) => l.code === code)?.name ?? code;
}

/** The subset of `storage.local` used here (injectable for tests). */
export interface LanguageStorage {
  get(keys: string[]): Promise<Record<string, unknown>>;
  set(items: Record<string, unknown>): Promise<void>;
}

const local = (): LanguageStorage => browser.storage.local;

async function readAll(storage: LanguageStorage): Promise<Record<string, string>> {
  const raw = (await storage.get([LANGUAGE_KEY]))[LANGUAGE_KEY];
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return {};
  return Object.fromEntries(Object.entries(raw as Record<string, unknown>).filter((e): e is [string, string] => isLanguageCode(e[1])));
}

/** The platform's remembered code, or null (first time, or unreadable). */
export async function rememberedLanguage(platform: Platform | null | undefined, storage: LanguageStorage = local()): Promise<string | null> {
  try {
    return (await readAll(storage))[platformKey(platform)] ?? null;
  } catch {
    return null;
  }
}

/** Remember `code` for the platform (the other platforms keep theirs). */
export async function rememberLanguage(platform: Platform | null | undefined, code: string, storage: LanguageStorage = local()): Promise<void> {
  if (!isLanguageCode(code)) return;
  const all = await readAll(storage).catch(() => ({}));
  await storage.set({ [LANGUAGE_KEY]: { ...all, [platformKey(platform)]: code } });
}

/** What the selector allows: shown when the app listed its languages,
 *  changeable only while nothing records (the language is fixed for the
 *  meeting). Pure. */
export function languageSelectView(list: AppLanguages | null | undefined, canStart: boolean): { visible: boolean; enabled: boolean } {
  return { visible: !!list, enabled: !!list && canStart };
}

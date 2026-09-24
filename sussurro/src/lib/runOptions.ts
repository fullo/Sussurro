import type { CleanupLevel, Settings } from "./types";

/** New's per-run choices (#157). `null`/absent = follow the dictation
 *  setting. Kept in the workspace only: choosing never saves the settings. */
export interface RunChoice {
  language?: string | null;
  cleanupLevel?: CleanupLevel | null;
}

/** What a run will use: the choice, or the dictation setting. Parakeet
 *  detects the language itself, so its language is always "auto". */
export function effectiveRun(settings: Settings, choice: RunChoice): { language: string; cleanupLevel: CleanupLevel } {
  return {
    language: settings.engine === "whisper" ? choice.language || settings.language : "auto",
    cleanupLevel: choice.cleanupLevel ?? settings.cleanup_level,
  };
}

/** The per-run arguments of `transcribe_file` / `engine_start_mic`. The
 *  language is only a Whisper hint: with Parakeet it is left to the backend
 *  (null = the settings), which records what it detects. */
export function runArgs(settings: Settings, choice: RunChoice): { language: string | null; cleanupLevel: CleanupLevel } {
  const e = effectiveRun(settings, choice);
  return { language: settings.engine === "whisper" ? e.language : null, cleanupLevel: e.cleanupLevel };
}

/** True when the run differs from the dictation settings. */
export function differsFromDictation(settings: Settings, choice: RunChoice): boolean {
  const e = effectiveRun(settings, choice);
  const d = effectiveRun(settings, {});
  return e.language !== d.language || e.cleanupLevel !== d.cleanupLevel;
}

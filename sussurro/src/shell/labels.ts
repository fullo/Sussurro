import { CLEANUP_LEVELS, LANGUAGES } from "../lib/constants";
import { cleanupProfile } from "../lib/llmProfiles";
import type { Settings } from "../lib/types";

/** "Whisper large-v3-turbo" / "Parakeet TDT v3" / "Qwen3-ASR 1.7B" */
export function sttLabel(s: Pick<Settings, "engine" | "whisper_model">): string {
  if (s.engine === "parakeet") return "Parakeet TDT v3";
  if (s.engine === "qwen3_asr") return "Qwen3-ASR 1.7B";
  const m = s.whisper_model.replace(/^ggml-/, "").replace(/\.bin$/, "").replace(/-q\d+_\d+$/, "");
  return `Whisper ${m}`;
}

/** "Light · llama3.2:3b" / "Off" — the model of the cleanup profile. */
export function cleanupLabel(
  s: Pick<Settings, "cleanup_level" | "llm_profiles" | "cleanup_profile">,
): string {
  if (s.cleanup_level === "none") return "Off";
  const level = CLEANUP_LEVELS.find((l) => l.value === s.cleanup_level)?.label ?? s.cleanup_level;
  const model = cleanupProfile(s)?.model;
  return model ? `${level} · ${model}` : level;
}

export function languageLabel(code: string): string {
  return LANGUAGES.find(([c]) => c === code)?.[1] ?? code;
}

import type { CleanupLevel } from "./types";

export const LANGUAGES: [string, string][] = [
  ["auto", "Auto-detect"],
  ["it", "Italiano"],
  ["en", "English"],
  ["es", "Español"],
  ["fr", "Français"],
  ["de", "Deutsch"],
  ["pt", "Português"],
  ["nl", "Nederlands"],
  ["ja", "日本語"],
  ["zh", "中文"],
];

export const MODELS = [
  { file: "ggml-base.en.bin", label: "Base · English · 148 MB · fastest" },
  { file: "ggml-small.bin", label: "Small · multilingual · 488 MB" },
  { file: "ggml-medium.bin", label: "Medium · multilingual · 1.5 GB" },
  { file: "ggml-large-v3-turbo-q5_0.bin", label: "Large v3 Turbo · multilingual · 574 MB · best" },
];

export const CLEANUP_LEVELS: { value: CleanupLevel; label: string; hint: string }[] = [
  { value: "none", label: "None", hint: "raw transcript" },
  { value: "light", label: "Light", hint: "fillers + grammar" },
  { value: "medium", label: "Medium", hint: "clarity" },
  { value: "high", label: "High", hint: "rewrite" },
];

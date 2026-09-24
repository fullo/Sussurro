import type { Settings } from "./types";

export type SttEngine = Settings["engine"];

/** The STT engines, in picker order (settings.rs `SttEngine`). */
export const ENGINES: { value: SttEngine; label: string; detail: string }[] = [
  { value: "whisper", label: "Whisper", detail: "whisper.cpp · GPU" },
  { value: "parakeet", label: "Parakeet", detail: "Parakeet TDT v3 · CPU" },
  { value: "qwen3_asr", label: "Qwen3-ASR", detail: "Qwen3-ASR 1.7B · optional" },
];

export function engineLabel(engine: SttEngine): string {
  return ENGINES.find((e) => e.value === engine)?.label ?? engine;
}

export function engineDetail(engine: SttEngine): string {
  return ENGINES.find((e) => e.value === engine)?.detail ?? "";
}

/** Engines that detect the language themselves and take no language hint
 *  or dictionary prompt. */
export function detectsLanguage(engine: SttEngine): boolean {
  return engine !== "whisper";
}

/** Whether the engine can be picked in this build: Qwen3-ASR needs the
 *  bundled llama-server sidecar (#116/#117). The current engine always
 *  stays selectable, so the picker never hides what is in use. */
export function engineSelectable(engine: SttEngine, current: SttEngine, sidecarAvailable: boolean): boolean {
  return engine !== "qwen3_asr" || sidecarAvailable || current === engine;
}

/** The Models card's note (#117, figures from the #109 benchmark on an
 *  M1 Pro, FLEURS word error rate). */
export const QWEN3_ASR_NOTE =
  "Optional engine, never the default. Qwen3-ASR 1.7B runs on this machine in a separate process " +
  "(llama.cpp's llama-server, bundled with Sussurro) and detects the language itself. It takes no " +
  "dictionary prompt, so your dictionary words only reach the cleanup step. Whisper large-v3-turbo " +
  "is more accurate on Italian (1.8% vs 2.5% word errors in our benchmark) at similar speed with " +
  "about a quarter of the memory; Qwen3-ASR does better on English (3.0% vs 4.5%). " +
  "Download: about 2.5 GB.";

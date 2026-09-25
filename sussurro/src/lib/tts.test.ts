import { describe, expect, it } from "vitest";
import {
  canPreview,
  downloadBytes,
  downloadPrompt,
  languageStatus,
  offPrompt,
  progressFraction,
  progressLabel,
  selectedVoice,
  withVoice,
} from "./tts";
import type { TtsLanguage, TtsStatus, TtsVoice } from "./types";

const voice = (over: Partial<TtsVoice> = {}): TtsVoice => ({
  id: "giovanni",
  label: "Giovanni",
  source: "Common Voice Italian (CC0)",
  bytes: 18_486_272,
  downloaded: false,
  selected: true,
  ...over,
});

const lang = (over: Partial<TtsLanguage> = {}): TtsLanguage => ({
  code: "it",
  label: "Italian",
  variant: "24 layers",
  model_bytes: 1_307_501_592,
  model_downloaded: false,
  voices: [voice(), voice({ id: "alba", label: "Alba", selected: false, bytes: 24_777_760 })],
  ...over,
});

const status = (languages: TtsLanguage[] = [lang()]): TtsStatus => ({
  enabled: true,
  engine: "Pocket TTS",
  licence: "CC-BY-4.0",
  attribution: "Pocket TTS by Kyutai, ONNX export by KevinAHM",
  languages,
  bytes_on_disk: 0,
  downloading: null,
  loaded: null,
});

describe("download sizes and the confirmation", () => {
  it("counts only what is missing", () => {
    expect(downloadBytes(lang())).toBe(1_307_501_592 + 18_486_272);
    expect(downloadBytes(lang({ model_downloaded: true }))).toBe(18_486_272);
    const l = lang({ model_downloaded: true, voices: [voice({ downloaded: true })] });
    expect(downloadBytes(l)).toBe(0);
    expect(downloadBytes(lang({ model_downloaded: true }), lang().voices[1])).toBe(24_777_760);
  });

  it("names the size and the licence before anything is fetched (P24)", () => {
    const p = downloadPrompt(status(), lang());
    expect(p).toContain("Italian model (24 layers, 1.3 GB)");
    expect(p).toContain("voice Giovanni (18.5 MB)");
    expect(p).toContain("Total 1.3 GB");
    expect(p).toContain("Licence: CC-BY-4.0 (Pocket TTS by Kyutai");
    const voiceOnly = downloadPrompt(status(), lang({ model_downloaded: true }), lang().voices[1]);
    expect(voiceOnly).toBe(
      "Download the voice Alba (24.8 MB) from huggingface.co? Total 24.8 MB. Licence: CC-BY-4.0 (Pocket TTS by Kyutai, ONNX export by KevinAHM).",
    );
  });
});

describe("status lines", () => {
  it("says what is on disk", () => {
    expect(languageStatus(lang())).toBe("Not downloaded — 1.3 GB for the model");
    expect(languageStatus(lang({ model_downloaded: true }))).toMatch(/download a voice/);
    const two = lang({ model_downloaded: true, voices: [voice({ downloaded: true }), voice({ id: "x", downloaded: true })] });
    expect(languageStatus(two)).toBe("Downloaded, 2 voices");
  });

  it("previews only a downloaded voice of a downloaded model", () => {
    expect(canPreview(lang(), voice({ downloaded: true }))).toBe(false);
    expect(canPreview(lang({ model_downloaded: true }), voice())).toBe(false);
    expect(canPreview(lang({ model_downloaded: true }), voice({ downloaded: true }))).toBe(true);
  });

  it("picks the selected voice, else the first", () => {
    expect(selectedVoice(lang())?.id).toBe("giovanni");
    const none = lang({ voices: [voice({ selected: false, id: "a" }), voice({ selected: false, id: "b" })] });
    expect(selectedVoice(none)?.id).toBe("a");
    expect(selectedVoice(lang({ voices: [] }))).toBeUndefined();
  });
});

describe("progress", () => {
  const p = { language: "it", voice: null, file: "flow_lm_main.onnx", done_bytes: 500_000_000, total_bytes: 1_000_000_000 };
  it("reads as a percentage with the file", () => {
    expect(progressLabel(p)).toBe("50% · 500.0 MB of 1.0 GB · flow_lm_main.onnx");
    expect(progressFraction(p)).toBe(0.5);
    expect(progressFraction(null)).toBe(0);
    expect(progressFraction({ ...p, total_bytes: 0 })).toBe(0);
    expect(progressLabel({ ...p, total_bytes: 0, file: "" })).toBe("0% · 500.0 MB of 0 B");
  });
});

describe("turning the module off", () => {
  it("offers to delete the models only when there are some", () => {
    expect(offPrompt(0)).toBeNull();
    expect(offPrompt(1_326_000_000)).toMatch(/Delete its downloaded models and voices too \(1\.3 GB\)/);
  });
});

describe("voice per language", () => {
  it("sets one language and keeps the others", () => {
    expect(withVoice(undefined, "it", "alba")).toEqual({ it: "alba" });
    expect(withVoice({ en: "alba" }, "it", "marius")).toEqual({ en: "alba", it: "marius" });
  });
});

import { describe, expect, it } from "vitest";
import {
  canPreview,
  checkNotes,
  checkSummary,
  metadataLine,
  signatureLine,
  watermarkLine,
  watermarkMissing,
  watermarkPrompt,
  downloadBytes,
  downloadPrompt,
  languageStatus,
  offPrompt,
  progressFraction,
  progressLabel,
  selectedVoice,
  withVoice,
} from "./tts";
import type { TtsLanguage, TtsStatus, TtsVoice, TtsWatermark, WatermarkCheck } from "./types";

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

const wm = (over: Partial<TtsWatermark> = {}): TtsWatermark => ({
  bytes: 93_481_274,
  downloaded: false,
  detector_downloaded: false,
  licence: "MIT",
  attribution: "AudioSeal by Meta, ONNX export by DarumaHQ",
  ...over,
});

describe("the watermark models (#257)", () => {
  it("come with the first download and are named in the confirmation", () => {
    expect(downloadBytes(lang(), undefined, wm())).toBe(1_307_501_592 + 18_486_272 + 93_481_274);
    expect(downloadBytes(lang(), undefined, wm({ downloaded: true }))).toBe(1_307_501_592 + 18_486_272);
    const p = downloadPrompt({ ...status(), watermark: wm() }, lang());
    expect(p).toContain("the Italian model (24 layers, 1.3 GB), the voice Giovanni (18.5 MB) and the watermark models");
    expect(p).toContain("(93.5 MB, MIT, AudioSeal by Meta, ONNX export by DarumaHQ)");
    expect(p).toContain("Total 1.4 GB");
    // Nothing else to fetch: no watermark-only surprise in a language prompt.
    const done = lang({ model_downloaded: true, voices: [voice({ downloaded: true })] });
    expect(downloadPrompt({ ...status([done]), watermark: wm() }, done)).toContain("nothing new");
    expect(watermarkPrompt(wm())).toBe(
      "Download the watermark models that mark every file Sussurro speaks (93.5 MB, MIT, AudioSeal by Meta, ONNX export by DarumaHQ) from huggingface.co? Total 93.5 MB.",
    );
  });

  it("block previews and are explained only when a model is there", () => {
    const l = lang({ model_downloaded: true });
    const v = voice({ downloaded: true });
    expect(canPreview(l, v, wm())).toBe(false);
    expect(canPreview(l, v, wm({ downloaded: true }))).toBe(true);
    expect(canPreview(l, v)).toBe(true);
    expect(watermarkMissing({ ...status([lang()]), watermark: wm() })).toBeNull();
    expect(watermarkMissing({ ...status([l]), watermark: wm() })).toMatch(/watermark/);
    expect(watermarkMissing({ ...status([l]), watermark: wm({ downloaded: true }) })).toBeNull();
    expect(watermarkMissing(status([l]))).toBeNull();
  });
});

const check = (over: Partial<WatermarkCheck> = {}): WatermarkCheck => ({
  file_name: "speech.opus",
  format: "Ogg Opus",
  seconds: 12.3,
  truncated: false,
  short: false,
  summary: "made_by_sussurro",
  watermark: { verdict: "found", frames_marked: 0.954, bit_errors: 0 },
  metadata: { status: "sussurro", tags: [["SYNTHETIC", "1"]] },
  signature: { status: "not_checked" },
  ...over,
});

describe("Check a file wording (E17)", () => {
  it("says made by Sussurro only with the code", () => {
    expect(checkSummary(check())).toMatch(/^Made by Sussurro/);
    expect(watermarkLine(check())).toBe("Watermark: found — 95% of the audio marked, with Sussurro's code (0 of 16 bits off).");
  });

  it("never names another tool nor a human", () => {
    const inc = check({ summary: "inconclusive", watermark: { verdict: "inconclusive", frames_marked: 0.89, bit_errors: 9 } });
    expect(checkSummary(inc)).toMatch(/^Inconclusive/);
    expect(watermarkLine(inc)).toContain("9 of the 16 code bits differ");
    const none = check({ summary: "no_mark", watermark: { verdict: "not_found", frames_marked: 0.01, bit_errors: 8 }, metadata: { status: "none", tags: [] } });
    expect(checkSummary(none)).toBe("No Sussurro mark found. That doesn't say who or what made the audio.");
    for (const r of [inc, none]) {
      const all = [checkSummary(r), watermarkLine(r), metadataLine(r)].join(" ").toLowerCase();
      expect(all).not.toMatch(/human|person|real recording|another tool|other tool's/);
    }
    expect(watermarkLine(none)).toContain("not found");
  });

  it("describes tags as unsigned and the C2PA slot as not checked", () => {
    expect(checkSummary(check({ summary: "tags_only" }))).toMatch(/tags can be copied/);
    expect(metadataLine(check())).toMatch(/not signed/);
    expect(metadataLine(check({ metadata: { status: "synthetic", tags: [] } }))).toMatch(/other software/);
    expect(metadataLine(check({ format: "MP3", metadata: { status: "not_read", tags: [] } }))).toBe("Tags: not read for MP3 files.");
    expect(signatureLine(check())).toBe("Signed metadata (C2PA): not checked yet.");
  });

  it("notes short and long files, and that nothing left the computer", () => {
    expect(checkNotes(check())).toEqual(["Checked on this computer: nothing was uploaded."]);
    const notes = checkNotes(check({ short: true, truncated: true }));
    expect(notes).toHaveLength(3);
    expect(notes[0]).toMatch(/under 3 seconds/);
  });
});

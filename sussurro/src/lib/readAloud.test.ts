import { describe, expect, it } from "vitest";
import {
  TRANSCRIPT_DOC,
  baseCode,
  createLabel,
  defaultLanguage,
  documentLabel,
  estimateMinutes,
  jobFraction,
  jobIsFor,
  jobLabel,
  listenDisabled,
  readableDocs,
  readiness,
  speechFacts,
  speechFor,
  speechSignatureNote,
  staleNote,
} from "./readAloud";
import type { ReadAloudJob, SpeechStatus, TtsLanguage, TtsStatus } from "./types";

const lang = (code: string, over: Partial<TtsLanguage> = {}): TtsLanguage => ({
  code,
  label: code === "it" ? "Italian" : "English",
  variant: "",
  model_bytes: 1000,
  model_downloaded: true,
  voices: [{ id: "v", label: "Vera", source: "", bytes: 10, downloaded: true, selected: true }],
  ...over,
});

const status = (over: Partial<TtsStatus> = {}): TtsStatus => ({
  enabled: true,
  engine: "Pocket TTS",
  licence: "CC-BY-4.0",
  attribution: "",
  languages: [lang("it"), lang("en", { model_downloaded: false })],
  bytes_on_disk: 0,
  downloading: null,
  loaded: null,
  ...over,
});

const speech = (over: Partial<SpeechStatus> = {}): SpeechStatus => ({
  file: "speech.opus",
  bytes: 2048,
  document: TRANSCRIPT_DOC,
  voice: "giovanni",
  language: "it",
  engine: "Pocket TTS",
  date: "2026-09-26T10:00:00+02:00",
  marked: ["metadata"],
  signed: false,
  unsigned: "",
  recorded: true,
  stale: false,
  source_missing: false,
  ...over,
});

const job = (over: Partial<ReadAloudJob> = {}): ReadAloudJob => ({
  item_id: "2026/09/x",
  document: TRANSCRIPT_DOC,
  language: "it",
  voice: "giovanni",
  done: 3,
  total: 12,
  ...over,
});

describe("read aloud", () => {
  it("lists the transcript first, then companions by title", () => {
    const docs = readableDocs([{ file: "document.md", meta: { title: "Minutes" } }, { file: "x.md" }]);
    expect(docs.map((d) => d.label)).toEqual(["Transcript", "Minutes", "x.md"]);
    expect(documentLabel("document.md", docs)).toBe("Minutes");
    expect(documentLabel("gone.md", docs)).toBe("gone.md");
    expect(documentLabel("", docs)).toBe("Unknown document");
  });

  it("is ready only with the module on and the model and voice downloaded", () => {
    expect(readiness(null, "it").state).toBe("off");
    expect(readiness(status({ enabled: false }), "it").state).toBe("off");
    expect(readiness(status(), "fr").state).toBe("no-language");
    expect(readiness(status(), "it-IT")).toMatchObject({ state: "ready", voice: "Vera" });
    const missing = readiness(status(), "en");
    expect(missing).toMatchObject({ state: "missing", what: "the English model" });
    const noVoice = readiness(
      status({ languages: [lang("it", { voices: [{ id: "v", label: "Vera", source: "", bytes: 1, downloaded: false, selected: true }] })] }),
      "it",
    );
    expect(noVoice).toMatchObject({ state: "missing", what: "the voice Vera" });
  });

  it("offers the item's language first", () => {
    expect(defaultLanguage(status(), "en")).toBe("en");
    expect(defaultLanguage(status(), "auto")).toBe("it");
    expect(defaultLanguage(status(), "")).toBe("it");
    expect(defaultLanguage(null, "it")).toBe("");
    expect(baseCode(" EN_gb ")).toBe("en");
  });

  it("describes the job", () => {
    expect(jobLabel(job())).toBe("Making the speech file… 3 of 12 passages");
    expect(jobLabel(job({ total: 0 }))).toBe("Making the speech file…");
    expect(jobFraction(job())).toBe(0.25);
    expect(jobFraction(null)).toBe(0);
    expect(jobIsFor(job(), "2026/09/x")).toBe(true);
    expect(jobIsFor(job(), "other")).toBe(false);
  });

  it("shows one button per document (#327): none / exists / stale / missing source / module off / job running", () => {
    // none: no speech yet -> Create.
    expect(createLabel(undefined)).toBe("Create");
    // exists, current: Listen alone, no Create.
    expect(createLabel(speech())).toBeNull();
    // stale: Listen stays, plus the secondary Create again.
    expect(createLabel(speech({ stale: true }))).toBe("Create again");
    // missing source: the create label is unaffected (only the note differs).
    expect(createLabel(speech({ source_missing: true }))).toBeNull();
    expect(createLabel(speech({ stale: true, source_missing: true }))).toBe("Create again");
    // module off: the section itself decides not to show Create at all —
    // createLabel doesn't need `enabled`, the caller gates it. Listen is
    // unaffected either way (it plays a file already on disk).

    // job running: Listen for the file being replaced is disabled...
    expect(listenDisabled(job({ document: TRANSCRIPT_DOC }), "2026/09/x", TRANSCRIPT_DOC)).toBe(true);
    // ...but not another document of the same item...
    expect(listenDisabled(job({ document: TRANSCRIPT_DOC }), "2026/09/x", "document.md")).toBe(false);
    // ...nor while no job runs, nor for another item.
    expect(listenDisabled(null, "2026/09/x", TRANSCRIPT_DOC)).toBe(false);
    expect(listenDisabled(job({ document: TRANSCRIPT_DOC }), "other", TRANSCRIPT_DOC)).toBe(false);
  });

  it("says why a speech file is unsigned, only when the app recorded a reason", () => {
    expect(speechSignatureNote(speech({ signed: true, marked: ["metadata", "watermark", "signature"] }))).toBeNull();
    expect(speechSignatureNote(speech())).toBeNull(); // made before signing existed
    const note = speechSignatureNote(speech({ unsigned: "the macOS Keychain is not available" }));
    expect(note).toMatch(/^Not signed: the macOS Keychain is not available\./);
    expect(note).toMatch(/watermark/);
  });

  it("explains speech that no longer matches its document", () => {
    expect(staleNote(speech())).toBeNull();
    expect(staleNote(speech({ stale: true }))).toMatch(/Out of date/);
    expect(staleNote(speech({ source_missing: true }))).toMatch(/deleted/);
    expect(staleNote(speech({ recorded: false }))).toMatch(/no record/);
    expect(speechFacts(speech(), [lang("it")])).toBe("voice giovanni · Italian · Pocket TTS · 2 KB · 2026-09-26");
    // #259: a two-voice podcast script names both hosts' voices.
    expect(speechFacts(speech({ voice_b: "alba" }), [lang("it")])).toBe(
      "voices giovanni (Host A), alba (Host B) · Italian · Pocket TTS · 2 KB · 2026-09-26",
    );
    expect(speechFor([speech(), speech({ file: "speech-document.opus", document: "document.md" })], "document.md")?.file).toBe(
      "speech-document.opus",
    );
    expect(speechFor([speech()], "x.md")).toBeUndefined();
  });

  it("estimates how long a document takes", () => {
    expect(estimateMinutes(100, "it")).toBe(1);
    expect(estimateMinutes(60_000, "it")).toBeGreaterThan(estimateMinutes(60_000, "en"));
  });
});

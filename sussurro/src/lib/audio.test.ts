import { describe, expect, it } from "vitest";
import { audioBadge, audioBytes, audioChannel, audioLabel, compressPercent, compressSummaryText, savedAudioFormat, wavFiles } from "./audio";
import { formatBytes } from "./format";
import { saveAudioChoice } from "./runOptions";
import type { Settings } from "./types";

describe("saved audio (#141)", () => {
  it("sizes and labels an item's audio", () => {
    expect(audioBytes({})).toBe(0);
    expect(audioLabel({ audio: [] })).toBe("");
    const one = { audio: [{ name: "audio.wav", bytes: 115_200_044 }] };
    expect(audioBytes(one)).toBe(115_200_044);
    expect(audioLabel(one)).toBe("Audio · 115.2 MB");
    const two = {
      audio: [
        { name: "audio-mic.wav", bytes: 1_000_000 },
        { name: "audio-remote.wav", bytes: 2_500_000 },
      ],
    };
    expect(audioLabel(two)).toBe("Audio · 2 files · 3.5 MB");
  });

  it("names the channel of a per-channel file", () => {
    expect(audioChannel({ name: "audio.wav", bytes: 0 })).toBe("");
    expect(audioChannel({ name: "audio-mic.wav", bytes: 0 })).toBe("You (microphone)");
    expect(audioChannel({ name: "audio-remote.wav", bytes: 0 })).toBe("Others (remote)");
    expect(audioChannel({ name: "audio-system.wav", bytes: 0 })).toBe("System audio");
    expect(audioChannel({ name: "audio.opus", bytes: 0 })).toBe("");
    expect(audioChannel({ name: "audio-remote.opus", bytes: 0 })).toBe("Others (remote)");
  });

  it("reads the saved audio format, Opus by default (#247, #248)", () => {
    expect(savedAudioFormat({})).toBe("opus");
    expect(savedAudioFormat({ saved_audio_format: "wav" })).toBe("wav");
    expect(savedAudioFormat({ saved_audio_format: "opus" })).toBe("opus");
  });

  it("finds what Compress audio converts and reports it (#248)", () => {
    const item = {
      audio: [
        { name: "audio-mic.wav", bytes: 10 },
        { name: "audio-remote.opus", bytes: 2 },
        { name: "audio-system.wav", bytes: 5 },
      ],
    };
    expect(wavFiles(item).map((f) => f.name)).toEqual(["audio-mic.wav", "audio-system.wav"]);
    expect(wavFiles({})).toEqual([]);

    expect(compressPercent({ done_bytes: 0, total_bytes: 0 })).toBe(0);
    expect(compressPercent({ done_bytes: 50, total_bytes: 200 })).toBe(25);
    expect(compressPercent({ done_bytes: 300, total_bytes: 200 })).toBe(100);

    const base = { items: 0, files: 0, bytes_before: 0, bytes_after: 0, cancelled: false, failed: [] };
    const mb = 1024 * 1024;
    const one = { ...base, items: 1, files: 1, bytes_before: 115 * mb, bytes_after: 11 * mb };
    expect(compressSummaryText(one)).toBe(
      `Audio compressed: ${formatBytes(115 * mb)} → ${formatBytes(11 * mb)}. The WAV file is in the trash.`,
    );
    const many = { ...one, items: 3, files: 4 };
    expect(compressSummaryText(many)).toContain("in 3 items");
    expect(compressSummaryText(many)).toContain("files are in the trash");
    expect(compressSummaryText({ ...many, cancelled: true })).toContain("the rest stays WAV");
    expect(compressSummaryText({ ...base, cancelled: true })).toBe("Compression cancelled: the WAV audio is kept.");
    expect(compressSummaryText(base)).toBe("No WAV audio to compress.");
    const failed = { ...base, failed: [{ id: "a", error: "x" }] };
    expect(compressSummaryText(failed)).toBe("1 item could not be compressed and keeps the WAV.");
    expect(compressSummaryText({ ...one, failed: [{ id: "a", error: "x" }, { id: "b", error: "y" }] })).toContain(
      "2 items could not be compressed and keep the WAV.",
    );
  });

  it("marks Library rows with audio only", () => {
    expect(audioBadge({})).toBe("");
    expect(audioBadge({ audio_bytes: 0 })).toBe("");
    expect(audioBadge({ audio_bytes: 64_044 })).toBe("♪ 64 KB");
  });

  it("formats byte sizes", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(999)).toBe("999 B");
    expect(formatBytes(1_500)).toBe("2 KB");
    expect(formatBytes(115_200_044)).toBe("115.2 MB");
    expect(formatBytes(4_294_967_258)).toBe("4.3 GB");
  });

  it("is off unless asked (P9)", () => {
    const s = (save_audio?: boolean) => ({ save_audio }) as Settings;
    expect(saveAudioChoice(s(), {})).toBe(false);
    expect(saveAudioChoice(s(false), { saveAudio: null })).toBe(false);
    expect(saveAudioChoice(s(true), {})).toBe(true);
    expect(saveAudioChoice(s(true), { saveAudio: false })).toBe(false);
    expect(saveAudioChoice(s(false), { saveAudio: true })).toBe(true);
  });
});

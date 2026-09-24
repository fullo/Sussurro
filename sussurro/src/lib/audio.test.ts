import { describe, expect, it } from "vitest";
import { audioBadge, audioBytes, audioChannel, audioLabel } from "./audio";
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

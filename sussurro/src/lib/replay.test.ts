import { describe, expect, it } from "vitest";
import {
  activeSegment,
  activeWord,
  adjacentLine,
  buildPlaylist,
  channelFile,
  formatPlayerTime,
  fromVirtual,
  locate,
  sortLines,
  speakerTalkTime,
  toVirtual,
  wordSpans,
  type ReplaySegment,
} from "./replay";

const seg = (id: number, start: number, end: number, over: Partial<ReplaySegment> = {}): ReplaySegment => ({
  id,
  start_ms: start,
  end_ms: end,
  text: `line ${id}`,
  ...over,
});

describe("channelFile", () => {
  it("maps every channel to audio.wav on a single-channel item", () => {
    expect(channelFile("mic", ["audio.wav"])).toBe("audio.wav");
    expect(channelFile("file", ["audio.wav"])).toBe("audio.wav");
    expect(channelFile(undefined, ["audio.wav"])).toBe("audio.wav");
  });

  it("picks the channel's own file on a two-channel item", () => {
    const files = ["audio-mic.wav", "audio-remote.wav"];
    expect(channelFile("mic", files)).toBe("audio-mic.wav");
    expect(channelFile("remote", files)).toBe("audio-remote.wav");
    // A missing channel: nothing to play.
    expect(channelFile("system", files)).toBeNull();
    // Channel defaults to mic (segments.json's default).
    expect(channelFile(undefined, files)).toBe("audio-mic.wav");
  });

  it("uses a lone channel file for everything, and nothing without files", () => {
    expect(channelFile("remote", ["audio-mic.wav"])).toBe("audio-mic.wav");
    expect(channelFile("mic", [])).toBeNull();
  });
});

describe("buildPlaylist", () => {
  const two = ["audio-remote.wav", "audio-mic.wav"];

  it("plays the whole recording, all files, for all speakers", () => {
    const pl = buildPlaylist([seg(0, 1000, 5000)], two, 60_000, null);
    expect(pl.entries).toEqual([{ start_ms: 0, end_ms: 60_000, files: ["audio-mic.wav", "audio-remote.wav"], offset_ms: 0 }]);
    expect(pl.duration_ms).toBe(60_000);
    // The last line counts when the known length is shorter or unknown.
    expect(buildPlaylist([seg(0, 1000, 75_000)], two, 0, null).duration_ms).toBe(75_000);
  });

  it("is empty without audio files", () => {
    expect(buildPlaylist([seg(0, 0, 1000, { speaker_id: "you" })], [], 5000, "you").entries).toEqual([]);
    expect(buildPlaylist([seg(0, 0, 1000)], [], 5000, null).entries).toEqual([]);
  });

  it("keeps only the speaker's lines, skipping the gaps between them", () => {
    const segments = [
      seg(0, 0, 4000, { speaker_id: "you", channel: "mic" }),
      seg(1, 4500, 9000, { speaker_id: "voice:1", channel: "remote" }),
      seg(2, 9500, 12_000, { speaker_id: "you", channel: "mic" }),
      seg(3, 20_000, 21_000, { speaker_id: "you", channel: "mic" }),
    ];
    const pl = buildPlaylist(segments, two, 30_000, "you");
    expect(pl.entries).toEqual([
      { start_ms: 0, end_ms: 4000, files: ["audio-mic.wav"], offset_ms: 0 },
      { start_ms: 9500, end_ms: 12_000, files: ["audio-mic.wav"], offset_ms: 4000 },
      { start_ms: 20_000, end_ms: 21_000, files: ["audio-mic.wav"], offset_ms: 6500 },
    ]);
    expect(pl.duration_ms).toBe(7500);
    // The other side plays from its own file.
    expect(buildPlaylist(segments, two, 30_000, "voice:1").entries).toEqual([
      { start_ms: 4500, end_ms: 9000, files: ["audio-remote.wav"], offset_ms: 0 },
    ]);
  });

  it("joins lines that touch or nearly touch, not ones a gap apart", () => {
    const segments = [
      seg(0, 0, 1000, { speaker_id: "a" }),
      seg(1, 1200, 2000, { speaker_id: "a" }), // 200 ms after: joined
      seg(2, 2500, 3000, { speaker_id: "a" }), // 500 ms after: separate
    ];
    const pl = buildPlaylist(segments, ["audio.wav"], 0, "a");
    expect(pl.entries.map((e) => [e.start_ms, e.end_ms])).toEqual([
      [0, 2000],
      [2500, 3000],
    ]);
  });

  it("joins overlapping lines across channels and plays both files", () => {
    const segments = [
      seg(0, 0, 5000, { speaker_id: "a", channel: "mic" }),
      seg(1, 4000, 8000, { speaker_id: "a", channel: "remote" }),
      // Contained in the first: no new span.
      seg(2, 1000, 2000, { speaker_id: "a", channel: "mic" }),
    ];
    const pl = buildPlaylist(segments, ["audio-mic.wav", "audio-remote.wav"], 0, "a");
    expect(pl.entries).toEqual([{ start_ms: 0, end_ms: 8000, files: ["audio-mic.wav", "audio-remote.wav"], offset_ms: 0 }]);
  });

  it("orders lines by time whatever their order in the file", () => {
    const segments = [seg(1, 9000, 10_000, { speaker_id: "a" }), seg(0, 1000, 2000, { speaker_id: "a" })];
    expect(buildPlaylist(segments, ["audio.wav"], 0, "a").entries.map((e) => e.start_ms)).toEqual([1000, 9000]);
  });

  it("leaves out blank lines, zero-length lines and unsaved channels", () => {
    const segments = [
      seg(0, 0, 1000, { speaker_id: "a", text: "  " }),
      seg(1, 2000, 2000, { speaker_id: "a" }),
      seg(2, 3000, 4000, { speaker_id: "a", channel: "system" }),
      // Not transcribed, but it has audio: kept.
      seg(3, 5000, 6000, { speaker_id: "a", channel: "mic", text: "", stt_error: "boom" }),
    ];
    const pl = buildPlaylist(segments, ["audio-mic.wav", "audio-remote.wav"], 0, "a");
    expect(pl.entries.map((e) => e.start_ms)).toEqual([5000]);
  });
});

describe("seek mapping", () => {
  // Spans [1000, 3000) and [5000, 6000): virtual 0–2000 and 2000–3000.
  const pl = buildPlaylist(
    [seg(0, 1000, 3000, { speaker_id: "a" }), seg(1, 5000, 6000, { speaker_id: "a" })],
    ["audio.wav"],
    0,
    "a",
  );

  it("maps real time onto the playlist's timeline", () => {
    expect(toVirtual(pl, 0)).toBe(0); // before the first span
    expect(toVirtual(pl, 1500)).toBe(500);
    expect(toVirtual(pl, 4000)).toBe(2000); // in the gap: where the next span starts
    expect(toVirtual(pl, 5500)).toBe(2500);
    expect(toVirtual(pl, 9000)).toBe(3000); // past the end
  });

  it("maps the timeline back to real time", () => {
    expect(fromVirtual(pl, 0)).toEqual({ index: 0, ms: 1000 });
    expect(fromVirtual(pl, 1999)).toEqual({ index: 0, ms: 2999 });
    expect(fromVirtual(pl, 2000)).toEqual({ index: 1, ms: 5000 });
    expect(fromVirtual(pl, 3000)).toEqual({ index: 1, ms: 6000 });
    // Clamped.
    expect(fromVirtual(pl, -5)).toEqual({ index: 0, ms: 1000 });
    expect(fromVirtual(pl, 99_999)).toEqual({ index: 1, ms: 6000 });
    expect(fromVirtual(buildPlaylist([], [], 0, "a"), 10)).toBeNull();
  });

  it("round-trips inside spans", () => {
    for (const v of [0, 700, 1999, 2000, 2400, 2999]) {
      const at = fromVirtual(pl, v)!;
      expect(toVirtual(pl, at.ms)).toBe(v);
    }
  });

  it("locates a real time in a span or skips to the next one", () => {
    expect(locate(pl, 0)).toEqual({ index: 0, ms: 1000 });
    expect(locate(pl, 2000)).toEqual({ index: 0, ms: 2000 });
    expect(locate(pl, 3000)).toEqual({ index: 1, ms: 5000 }); // end is exclusive
    expect(locate(pl, 4000)).toEqual({ index: 1, ms: 5000 });
    expect(locate(pl, 6000)).toBeNull();
  });

  it("is the identity for all speakers", () => {
    const all = buildPlaylist([], ["audio.wav"], 10_000, null);
    expect(toVirtual(all, 4321)).toBe(4321);
    expect(fromVirtual(all, 4321)).toEqual({ index: 0, ms: 4321 });
  });
});

describe("highlighting", () => {
  const lines = sortLines([
    seg(2, 10_000, 14_000, { speaker_id: "b", channel: "remote" }),
    seg(0, 0, 4000, { speaker_id: "a" }),
    seg(1, 5000, 9000, { speaker_id: "b" }),
    // Overlaps line 2 from the other channel.
    seg(3, 12_000, 13_000, { speaker_id: "a", channel: "mic" }),
    seg(4, 15_000, 16_000, { speaker_id: "a", text: "" }), // blank: not a line
  ]);

  it("sorts lines by time and drops blank ones", () => {
    expect(lines.map((l) => l.id)).toEqual([0, 1, 2, 3]);
  });

  it("finds the line being played, or none in a gap", () => {
    expect(activeSegment(lines, 0)?.id).toBe(0);
    expect(activeSegment(lines, 3999)?.id).toBe(0);
    expect(activeSegment(lines, 4000)).toBeNull();
    expect(activeSegment(lines, 7000)?.id).toBe(1);
    expect(activeSegment(lines, 11_000)?.id).toBe(2);
    expect(activeSegment(lines, 20_000)).toBeNull();
    expect(activeSegment([], 5)).toBeNull();
  });

  it("prefers the later of two overlapping lines, and falls back to the longer one", () => {
    expect(activeSegment(lines, 12_500)?.id).toBe(3);
    // Line 3 is over, line 2 (started earlier) still covers the time.
    expect(activeSegment(lines, 13_500)?.id).toBe(2);
  });

  it("only lights the filtered speaker's lines", () => {
    expect(activeSegment(lines, 12_500, "b")?.id).toBe(2);
    expect(activeSegment(lines, 7000, "a")).toBeNull();
    expect(activeSegment(lines, 12_500, "a")?.id).toBe(3);
  });

  it("goes to the next and previous line", () => {
    expect(adjacentLine(lines, 1000, 1)?.id).toBe(1);
    expect(adjacentLine(lines, 12_000, 1)).toBeNull();
    // 3 s into line 1: back to its start; right at its start: the previous line.
    expect(adjacentLine(lines, 8000, -1)?.id).toBe(1);
    expect(adjacentLine(lines, 5500, -1)?.id).toBe(0);
    expect(adjacentLine(lines, 500, -1)).toBeNull();
    // Filtered.
    expect(adjacentLine(lines, 1000, 1, "a")?.id).toBe(3);
    expect(adjacentLine(lines, 14_000, -1, "b")?.id).toBe(2);
  });
});

describe("word highlighting", () => {
  const words = [
    { w: "ciao", start_ms: 1000, end_ms: 1300 },
    { w: "a", start_ms: 1400, end_ms: 1500 },
    { w: "tutti", start_ms: 1700, end_ms: 2100 },
  ];

  it("aligns the words when the displayed text has the same words", () => {
    expect(wordSpans("Ciao a tutti.", words)).toEqual([
      { text: "Ciao", start_ms: 1000, end_ms: 1300 },
      { text: "a", start_ms: 1400, end_ms: 1500 },
      { text: "tutti.", start_ms: 1700, end_ms: 2100 },
    ]);
  });

  it("gives up when cleanup or an edit changed the words", () => {
    expect(wordSpans("Ciao a tutti voi.", words)).toBeNull();
    expect(wordSpans("Ciao", undefined)).toBeNull();
    expect(wordSpans("Ciao", [])).toBeNull();
  });

  it("lights the last word started, through the pause after it", () => {
    expect(activeWord(words, 900)).toBe(-1);
    expect(activeWord(words, 1000)).toBe(0);
    expect(activeWord(words, 1350)).toBe(0);
    expect(activeWord(words, 1600)).toBe(1);
    expect(activeWord(words, 5000)).toBe(2);
    expect(activeWord([], 5000)).toBe(-1);
  });
});

describe("speakerTalkTime", () => {
  it("lists speakers with saved lines, in the document's order", () => {
    const segments = [
      seg(0, 0, 3000, { speaker_id: "voice:1", channel: "remote" }),
      seg(1, 3000, 4000, { speaker_id: "you", channel: "mic" }),
      seg(2, 5000, 7000, { speaker_id: "voice:1", channel: "remote" }),
      seg(3, 8000, 9000, { speaker_id: "voice:2", channel: "system" }), // unsaved channel
    ];
    const speakers = [
      { id: "you", label: "You" },
      { id: "voice:1", label: "Anna" },
      { id: "voice:2", label: "Voice 2" },
      { id: "voice:3", label: "Voice 3" },
    ];
    expect(speakerTalkTime(segments, ["audio-mic.wav", "audio-remote.wav"], speakers)).toEqual([
      { id: "you", label: "You", ms: 1000 },
      { id: "voice:1", label: "Anna", ms: 5000 },
    ]);
  });
});

it("formats the player clock", () => {
  expect(formatPlayerTime(0)).toBe("0:00");
  expect(formatPlayerTime(65_400)).toBe("1:05");
  expect(formatPlayerTime(3_725_000)).toBe("1:02:05");
  expect(formatPlayerTime(-5)).toBe("0:00");
});

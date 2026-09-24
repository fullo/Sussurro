import { describe, expect, it } from "vitest";
import { buildPlaylist, type ReplaySegment } from "./replay";
import { ReplayPlayer, type MediaLike } from "./replayPlayer";

/** A media element on a fake clock: advance(ms) moves the playing ones. */
class FakeMedia implements MediaLike {
  currentTime = 0;
  paused = true;
  playbackRate = 1;
  plays = 0;
  constructor(public duration: number) {}
  get ended() {
    return this.currentTime >= this.duration;
  }
  play() {
    this.plays++;
    this.paused = false;
  }
  pause() {
    this.paused = true;
  }
  advance(ms: number) {
    if (this.paused) return;
    this.currentTime = Math.min(this.duration, this.currentTime + (ms / 1000) * this.playbackRate);
    if (this.ended) this.paused = true;
  }
}

const seg = (id: number, start: number, end: number, over: Partial<ReplaySegment> = {}): ReplaySegment => ({
  id,
  start_ms: start,
  end_ms: end,
  text: `line ${id}`,
  ...over,
});

/** Run the clock in 10 ms frames, calling tick() like the component. */
function run(player: ReplayPlayer, media: FakeMedia[], ms: number) {
  for (let t = 0; t < ms; t += 10) {
    for (const m of media) m.advance(10);
    player.tick();
  }
}

describe("ReplayPlayer", () => {
  const segments = [
    seg(0, 1000, 3000, { speaker_id: "you", channel: "mic" }),
    seg(1, 3000, 6000, { speaker_id: "voice:1", channel: "remote" }),
    seg(2, 8000, 9000, { speaker_id: "you", channel: "mic" }),
  ];
  const files = ["audio-mic.wav", "audio-remote.wav"];

  function setup(speaker: string | null) {
    const mic = new FakeMedia(20);
    const remote = new FakeMedia(20);
    const media = new Map<string, FakeMedia>([
      ["audio-mic.wav", mic],
      ["audio-remote.wav", remote],
    ]);
    const player = new ReplayPlayer(media, buildPlaylist(segments, files, 20_000, speaker));
    return { mic, remote, player, all: [mic, remote] };
  }

  it("plays only the speaker's spans from their channel, skipping the rest", () => {
    const { mic, remote, player, all } = setup("you");
    player.play();
    expect(mic.currentTime).toBe(1);
    expect(mic.paused).toBe(false);
    expect(remote.paused).toBe(true);
    run(player, all, 2100);
    // Past the first span's end: jumped over voice:1 to the next span.
    expect(player.entryIndex).toBe(1);
    expect(mic.currentTime).toBeGreaterThanOrEqual(8);
    expect(mic.currentTime).toBeLessThan(8.2);
    expect(remote.paused).toBe(true);
    run(player, all, 1200);
    // The end of the playlist: stopped at its end.
    expect(player.playing).toBe(false);
    expect(mic.paused).toBe(true);
    expect(player.now()).toBe(9000);
    // Play again starts over.
    player.play();
    expect(player.entryIndex).toBe(0);
    expect(mic.currentTime).toBe(1);
  });

  it("plays both channel files together, in sync, for all speakers", () => {
    const { mic, remote, player, all } = setup(null);
    player.play();
    expect(mic.paused || remote.paused).toBe(false);
    // The remote element lags (a stall): it's pulled back in sync.
    run(player, all, 500);
    remote.currentTime -= 0.5;
    player.tick();
    expect(Math.abs(remote.currentTime - mic.currentTime)).toBeLessThan(0.01);
    // A small drift is left alone (no audible re-seeks).
    remote.currentTime -= 0.05;
    player.tick();
    expect(mic.currentTime - remote.currentTime).toBeCloseTo(0.05, 5);
  });

  it("keeps the longest file as the clock when one channel ends early", () => {
    const mic = new FakeMedia(5);
    const remote = new FakeMedia(20);
    const media = new Map([
      ["audio-mic.wav", mic],
      ["audio-remote.wav", remote],
    ]);
    const player = new ReplayPlayer(media, buildPlaylist(segments, files, 20_000, null));
    player.play();
    run(player, [mic, remote], 6000);
    expect(mic.ended).toBe(true);
    expect(player.playing).toBe(true);
    expect(player.now()).toBeCloseTo(6000, -2);
  });

  it("seeks on the speaker's timeline and skips ±10 s without landing in gaps", () => {
    const { mic, player } = setup("you");
    // Virtual 2500 = 500 ms into the second span (8000 + 500).
    player.seekVirtual(2500);
    expect(player.entryIndex).toBe(1);
    expect(mic.currentTime).toBe(8.5);
    expect(player.virtualNow()).toBe(2500);
    player.skip(-10_000);
    expect(player.now()).toBe(1000);
    player.skip(10_000);
    // Past the end: stays at the end, not playing.
    expect(player.now()).toBe(9000);
    expect(player.playing).toBe(false);
  });

  it("seeks a real time in a gap to the next span", () => {
    const { mic, player } = setup("you");
    player.seek(4000);
    expect(player.entryIndex).toBe(1);
    expect(mic.currentTime).toBe(8);
    player.seek(50_000);
    expect(player.now()).toBe(9000);
  });

  it("keeps the position when the speaker filter changes", () => {
    const { mic, remote, player, all } = setup(null);
    player.seek(4000);
    player.play();
    run(player, all, 100);
    // Only "you": 4.1 s is in a gap → the next of their spans.
    player.setPlaylist(buildPlaylist(segments, files, 20_000, "you"));
    expect(player.playing).toBe(true);
    expect(mic.currentTime).toBe(8);
    expect(remote.paused).toBe(true);
    // Back to everyone: stays where it is, both files play.
    run(player, all, 200);
    player.setPlaylist(buildPlaylist(segments, files, 20_000, null));
    expect(mic.currentTime).toBeCloseTo(8.2, 5);
    expect(remote.currentTime).toBeCloseTo(8.2, 5);
    expect(remote.paused).toBe(false);
  });

  it("applies the speed to every element, and pauses them all", () => {
    const { mic, remote, player, all } = setup(null);
    player.setRate(1.5);
    player.play();
    run(player, all, 1000);
    expect(mic.currentTime).toBeCloseTo(1.5, 5);
    expect(remote.playbackRate).toBe(1.5);
    player.toggle();
    expect(mic.paused && remote.paused).toBe(true);
    expect(player.now()).toBeCloseTo(1500, 5);
  });

  it("does nothing with an empty playlist", () => {
    const player = new ReplayPlayer(new Map(), buildPlaylist(segments, [], 0, "you"));
    player.play();
    expect(player.playing).toBe(false);
    expect(player.tick()).toBe(false);
  });

  it("reports a refused play()", async () => {
    const errors: unknown[] = [];
    const m: MediaLike = {
      currentTime: 0,
      duration: 10,
      paused: true,
      ended: false,
      playbackRate: 1,
      play: () => Promise.reject(new Error("NotAllowedError")),
      pause: () => {},
    };
    const player = new ReplayPlayer(new Map([["audio.wav", m]]), buildPlaylist([], ["audio.wav"], 10_000, null), (e) => errors.push(e));
    player.play();
    await Promise.resolve();
    await Promise.resolve();
    expect(errors).toHaveLength(1);
  });
});

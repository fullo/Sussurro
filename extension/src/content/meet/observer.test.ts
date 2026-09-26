// @vitest-environment happy-dom
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import type { ObserverMsg } from "../names/messages";
import { tileKey } from "../names/guard";
import { NameObserver, TICK_MS } from "../names/observer";
import { MEET_PROFILE } from "./profile";

const HERE = dirname(fileURLToPath(import.meta.url));
const grid = () => new DOMParser().parseFromString(readFileSync(join(HERE, "fixtures", "grid-3.html"), "utf8"), "text/html");

/** An observer on the grid fixture with a fake clock and fake receivers. */
function rig() {
  const doc = grid();
  let now = 1_000;
  let speaking: number[] = [];
  let level = 0;
  const out: ObserverMsg[] = [];
  const obs = new NameObserver({
    profile: MEET_PROFILE,
    doc,
    receivers: () => [{ getContributingSources: () => speaking.map((source) => ({ source, timestamp: now - 20, audioLevel: 0.4 })) }],
    now: () => now,
    timeOrigin: 0,
    remoteLevel: () => (speaking.length ? 0.2 : level),
    emit: (m) => out.push(m),
  });
  const run = (ms: number) => {
    for (let t = 0; t < ms; t += TICK_MS) {
      now += TICK_MS;
      obs.tick();
    }
  };
  return { doc, obs, out, run, speak: (s: number[]) => (speaking = s), setLevel: (l: number) => (level = l) };
}

const of = (out: ObserverMsg[], type: ObserverMsg["type"]) => out.filter((m) => m.type === type);

describe("Meet observer", () => {
  it("sends the CSRC timeline, binds the lit tile's name, lists the others, reports health", () => {
    const r = rig();
    r.speak([111]);
    r.run(1_000);
    const active = of(r.out, "speaker_active");
    expect(active).toEqual([{ type: "speaker_active", id: "csrc:111", source: "rtp", at: 1_080 }]);
    // Five agreeing votes (one per tick with one CSRC and one lit tile).
    expect(of(r.out, "speaker_name")).toEqual([{ type: "speaker_name", id: "csrc:111", name: "Bruno Esempio" }]);
    // The user's own tile is not a participant.
    expect(of(r.out, "participants")).toEqual([{ type: "participants", names: ["Bruno Esempio", "Chiara Prova"] }]);
    const health = of(r.out, "observer_health");
    expect(health).toHaveLength(1);
    expect(health[0]).toMatchObject({ state: "ok", set: "meet-2026-09a", hooks: { tile: "ok", tileName: "ok", speaking: "ok", csrc: "ok" } });
    // No tile timeline while CSRCs are there.
    expect(active.every((m) => m.type === "speaker_active" && m.source === "rtp")).toBe(true);

    // Silence: idle at the last packet; once bound, a new turn carries the name.
    r.speak([]);
    r.run(600);
    expect(of(r.out, "speaker_idle")).toHaveLength(1);
    r.speak([111]);
    r.run(200);
    expect(of(r.out, "speaker_active").at(-1)).toMatchObject({ id: "csrc:111", name: "Bruno Esempio" });
    r.obs.stop();
    expect(r.out.at(-1)).toMatchObject({ type: "speaker_idle", id: "csrc:111" });
  });

  it("without CSRCs, sends the lit tiles as a named timeline once the audio proved it has none", () => {
    const r = rig();
    // Remote audio, but no contributing sources (another browser or transport).
    r.setLevel(0.2);
    r.run(1_500);
    expect(of(r.out, "speaker_active")).toEqual([]);
    r.run(1_000);
    const bruno = tileKey("spaces/abc/devices/2");
    expect(of(r.out, "speaker_active")).toEqual([{ type: "speaker_active", id: bruno, name: "Bruno Esempio", source: "dom", at: 3_100 }]);
    // Bruno stops, Chiara lights up: seen at the next read of the page.
    r.doc.querySelectorAll("[data-audio-level]")[1].setAttribute("data-audio-level", "0");
    r.doc.querySelectorAll("[data-audio-level]")[2].setAttribute("data-audio-level", "0.5");
    r.run(1_000);
    const changes = r.out.filter((m) => m.type === "speaker_active" || m.type === "speaker_idle").map((m) => [m.type, m.id]);
    expect(changes).toEqual([
      ["speaker_active", bruno],
      ["speaker_idle", bruno],
      ["speaker_active", tileKey("spaces/abc/devices/3")],
    ]);
    expect(of(r.out, "speaker_name")).toEqual([]);
  });

  it("stops sending names when the page's speaking marker is broken", () => {
    const r = rig();
    // Nobody ever lit: remove the marker from the page.
    for (const el of r.doc.querySelectorAll("[data-audio-level]")) el.remove();
    r.speak([111]);
    r.run(21_000);
    const last = of(r.out, "observer_health").at(-1);
    expect(last).toMatchObject({ state: "names_unavailable", hooks: { speaking: "broken", csrc: "ok" } });
    expect(of(r.out, "speaker_name")).toEqual([]);
  });

  it("never throws into the page", () => {
    const r = rig();
    const bad = new NameObserver({
      profile: MEET_PROFILE,
      doc: r.doc,
      receivers: () => {
        throw new Error("boom");
      },
      now: () => 0,
      timeOrigin: 0,
      remoteLevel: () => 0,
      emit: () => {},
      note: (e) => r.out.push({ type: "participants", names: [e] }),
    });
    expect(() => bad.tick()).not.toThrow();
    expect(of(r.out, "participants")[0]).toMatchObject({ names: [expect.stringContaining("boom")] });
  });
});

// @vitest-environment happy-dom
/* Teams web names (#245) on synthetic fixtures: the provisional selector
 * set, the CSRC timeline on the mix plus its redundant track, lag-aware
 * binding, the variant without an outline, and the user's tile from the
 * mic. */
import { dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { probe } from "../names/dom";
import { tileKey } from "../names/guard";
import { fixture, of, rig } from "../names/testing";
import { TEAMS_PROFILE } from "./profile";
import { TEAMS_SETS } from "./selectors";

const HERE = dirname(fileURLToPath(import.meta.url));
const stage = () => fixture(HERE, "stage-3.html");
const [SET] = TEAMS_SETS;

/** Light one wrapper's outline (by participant id suffix), or none. */
function light(doc: Document, who: "ada" | "bruno" | "chiara" | null) {
  for (const w of doc.querySelectorAll('[data-tid="participant-stream"]')) {
    const on = !!who && (w.getAttribute("data-participant-id") ?? "").endsWith(who);
    w.querySelector('[data-tid="voice-level-outline"]')?.setAttribute("data-speaking", on ? "true" : "false");
  }
}

/**
 * A call: turns of 2.5 s with 1 s of silence between, Bruno (CSRC 111)
 * then Chiara (222), twice; the outline lights 800 ms after a turn starts
 * and goes out 800 ms after it ends (Teams trails the audio, #239).
 */
const TURNS: [number, "bruno" | "chiara"][] = [
  [111, "bruno"],
  [222, "chiara"],
  [111, "bruno"],
  [222, "chiara"],
];
const LAG = 800;
function scene(r: ReturnType<typeof rig>) {
  return (t: number) => {
    const turn = TURNS[Math.floor(t / 3_500)];
    const into = t % 3_500;
    r.s.sources = turn && into < 2_500 ? [turn[0]] : [];
    const litTurn = TURNS[Math.floor((t - LAG) / 3_500)];
    const litInto = (t - LAG) % 3_500;
    light(r.doc, t >= LAG && litTurn && litInto < 2_500 ? litTurn[1] : null);
  };
}

describe("Teams selector set", () => {
  it("reads wrappers, names without qualifiers, the user's own and the lit one", () => {
    const doc = stage();
    light(doc, "bruno");
    const p = probe(doc, SET);
    expect(p.tiles).toEqual([
      { key: tileKey("8:orgid:0000-ada"), name: "Ada Test", self: true, speaking: false },
      { key: tileKey("8:orgid:0000-bruno"), name: "Bruno Esempio", self: false, speaking: true },
      { key: tileKey("8:orgid:0000-chiara"), name: "Chiara Prova", self: false, speaking: false },
    ]);
    expect(p.selfName).toBe("Ada Test");
    expect(p.indicated).toBe(3);
    expect(p.paused).toBe(false);
    expect(p.matched).toEqual({
      tile: "stream wrapper test id",
      tileId: "participant-id attribute",
      tileName: "display-name test id",
      selfMarker: "local-stream attribute",
      speaking: "voice-level outline, speaking flag",
    });
  });

  it("falls back to the stream id and to the outline's level", () => {
    const doc = stage();
    for (const w of doc.querySelectorAll("[data-participant-id]")) {
      w.setAttribute("data-stream-id", `s-${w.getAttribute("data-participant-id")}`);
      w.removeAttribute("data-participant-id");
    }
    const outline = doc.querySelectorAll('[data-tid="voice-level-outline"]')[2];
    outline.removeAttribute("data-speaking");
    outline.setAttribute("data-voice-level", "0.4");
    const p = probe(doc, SET);
    expect(p.matched.tileId).toBe("stream-id attribute");
    expect(p.tiles.map((t) => t.speaking)).toEqual([false, false, true]);
    expect(p.tiles[2].key).toBe(tileKey("s-8:orgid:0000-chiara"));
  });
});

describe("Teams observer", () => {
  it("binds CSRCs on the mix and its redundant track to the late outline, after two turns", () => {
    const r = rig(stage(), TEAMS_PROFILE);
    r.s.copies = 2; // the mix and the redundant track carry the same CSRCs
    r.run(14_000, scene(r));
    // The redundant track does not make every speaker a mirror.
    const active = of(r.out, "speaker_active");
    expect(active.map((m) => m.id)).toEqual(["csrc:111", "csrc:222", "csrc:111", "csrc:222"]);
    expect(active.every((m) => m.source === "rtp")).toBe(true);
    // One binding each, the right way round, only once each had two turns.
    const names = of(r.out, "speaker_name");
    expect(names).toEqual([
      { type: "speaker_name", id: "csrc:111", name: "Bruno Esempio" },
      { type: "speaker_name", id: "csrc:222", name: "Chiara Prova" },
    ]);
    // Bruno's second turn starts at 7 s: his name is the one that turn
    // carries (after its lock), not before.
    expect(active[2].name).toBeUndefined();
    expect(of(r.out, "participants")).toEqual([{ type: "participants", names: ["Bruno Esempio", "Chiara Prova"] }]);
    const health = of(r.out, "observer_health").at(-1);
    expect(health).toMatchObject({ state: "ok", set: "teams-2026-09a", hooks: { tile: "ok", tileName: "ok", speaking: "ok", csrc: "ok" } });
  });

  it("names nobody while two speak at once (overlap on the mix)", () => {
    const r = rig(stage(), TEAMS_PROFILE);
    r.run(12_000, (t) => {
      r.s.sources = [111, 222];
      light(r.doc, t > 1_000 ? "bruno" : null);
    });
    expect(of(r.out, "speaker_name")).toEqual([]);
  });

  it("reports names unavailable on the variant without an outline", () => {
    const r = rig(fixture(HERE, "no-outline.html"), TEAMS_PROFILE);
    r.run(12_000, (t) => {
      r.s.sources = t % 3_000 < 2_000 ? [111] : [];
    });
    const last = of(r.out, "observer_health").at(-1);
    expect(last).toMatchObject({ state: "names_unavailable", set: "teams-2026-09a", hooks: { speaking: "broken", csrc: "ok" } });
    expect(of(r.out, "speaker_name")).toEqual([]);
    // The audio timeline still goes out: the app keeps "Voice N".
    expect(of(r.out, "speaker_active").length).toBeGreaterThan(0);
  });

  it("learns the user's tile from the mic when the local marker is missing", () => {
    const doc = stage();
    doc.querySelector("[data-is-local]")?.removeAttribute("data-is-local");
    const r = rig(doc, TEAMS_PROFILE);
    r.run(1_000);
    // Before: Ada counts as a remote participant.
    expect(of(r.out, "participants").at(-1)?.names).toContain("Ada Test");
    // The user speaks alone; their outline lights 800 ms late.
    r.run(4_000, (t) => {
      r.s.mic = 0.3;
      light(r.doc, t >= LAG ? "ada" : null);
    });
    r.s.mic = 0;
    // Later: Ada's lit tile never votes, and she is no longer a participant.
    r.run(12_000, (t) => {
      r.s.sources = t % 3_500 < 2_500 ? [111] : [];
      light(r.doc, t % 3_500 >= LAG && t % 3_500 < 2_500 + LAG ? "ada" : null);
    });
    expect(of(r.out, "speaker_name")).toEqual([]);
    expect(of(r.out, "participants").at(-1)?.names).toEqual(["Bruno Esempio", "Chiara Prova"]);
  });
});

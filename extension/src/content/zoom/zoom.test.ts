// @vitest-environment happy-dom
/* Zoom web client names (#246) on synthetic fixtures: the provisional
 * selector set, per-participant SSRCs (WebRTC mode), the screen-share
 * pause, and the tile timeline when there are no RTP receivers (WASM
 * mode). */
import { dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { probe } from "../names/dom";
import { tileKey } from "../names/guard";
import { fixture, of, rig } from "../names/testing";
import { ZOOM_PROFILE } from "./profile";
import { ZOOM_SETS } from "./selectors";

const HERE = dirname(fileURLToPath(import.meta.url));
const gallery = () => fixture(HERE, "gallery-3.html");
const [SET] = ZOOM_SETS;
const BRUNO = "16778240";
const CHIARA = "16780288";

/** Mark one participant (every tile of theirs) as the active speaker. */
function light(doc: Document, userId: string | null) {
  for (const t of doc.querySelectorAll("[data-user-id]")) t.setAttribute("data-active-speaker", String(t.getAttribute("data-user-id") === userId));
}

function share(doc: Document, on: boolean) {
  doc.querySelector("[data-share-view]")?.remove();
  if (on) {
    const el = doc.createElement("div");
    el.setAttribute("data-share-view", "true");
    doc.body.appendChild(el);
  }
}

/** Turns of 2.5 s with 1 s of silence: Bruno (SSRC 2001), Chiara (2002),
 *  twice; the active-speaker marker follows 600 ms late. */
const TURNS: [number, string][] = [
  [2001, BRUNO],
  [2002, CHIARA],
  [2001, BRUNO],
  [2002, CHIARA],
];
const LAG = 600;
function scene(r: ReturnType<typeof rig>) {
  return (t: number) => {
    const turn = TURNS[Math.floor(t / 3_500)];
    r.s.sources = turn && t % 3_500 < 2_500 ? [turn[0]] : [];
    const lit = TURNS[Math.floor((t - LAG) / 3_500)];
    light(r.doc, t >= LAG && lit && (t - LAG) % 3_500 < 2_500 ? lit[1] : null);
  };
}

describe("Zoom selector set", () => {
  it("reads tiles once per participant, names without Zoom's qualifiers, the user's own and the active one", () => {
    const doc = gallery();
    light(doc, BRUNO);
    const p = probe(doc, SET);
    expect(p.tiles).toEqual([
      { key: tileKey(BRUNO), name: "Bruno Esempio", self: false, speaking: true },
      { key: tileKey("16779264"), name: "Ada Test", self: true, speaking: false },
      { key: tileKey(CHIARA), name: "Chiara Prova", self: false, speaking: false },
    ]);
    expect(p.selfName).toBe("Ada Test");
    expect(p.matched).toEqual({
      tile: "user-id attribute",
      tileId: "user-id attribute",
      tileName: "footer name test id",
      selfMarker: "self attribute",
      speaking: "active-speaker attribute",
    });
    expect(p.paused).toBe(false);
    share(doc, true);
    expect(probe(doc, SET).paused).toBe(true);
  });

  it("reports cut names as rejected instead of guessing", () => {
    const doc = gallery();
    for (const n of doc.querySelectorAll('[data-testid="footer-name"]')) n.textContent = `${(n.textContent ?? "").slice(0, 6)}…`;
    const p = probe(doc, SET);
    expect(p.tiles.every((t) => t.name === null)).toBe(true);
    expect(p.rejectedNames).toBe(3);
  });
});

describe("Zoom observer", () => {
  it("WebRTC mode: binds each participant's SSRC to the late active-speaker marker", () => {
    const r = rig(gallery(), ZOOM_PROFILE);
    r.run(14_000, scene(r));
    const active = of(r.out, "speaker_active");
    expect(active.map((m) => m.id)).toEqual(["ssrc:2001", "ssrc:2002", "ssrc:2001", "ssrc:2002"]);
    expect(active.every((m) => m.source === "rtp")).toBe(true);
    expect(of(r.out, "speaker_name")).toEqual([
      { type: "speaker_name", id: "ssrc:2001", name: "Bruno Esempio" },
      { type: "speaker_name", id: "ssrc:2002", name: "Chiara Prova" },
    ]);
    expect(of(r.out, "participants")).toEqual([{ type: "participants", names: ["Bruno Esempio", "Chiara Prova"] }]);
    const health = of(r.out, "observer_health").at(-1);
    expect(health).toMatchObject({ state: "ok", set: "zoom-2026-09a", hooks: { tile: "ok", tileName: "ok", speaking: "ok", ssrc: "ok" } });
    expect(health?.hooks.csrc).toBeUndefined();
  });

  it("does not vote while a screen share holds the spotlight", () => {
    const r = rig(gallery(), ZOOM_PROFILE);
    share(r.doc, true);
    r.run(14_000, scene(r));
    expect(of(r.out, "speaker_active")).toHaveLength(4);
    expect(of(r.out, "speaker_name")).toEqual([]);
    // The share ends: the next turns bind.
    share(r.doc, false);
    r.run(14_000, scene(r));
    expect(of(r.out, "speaker_name").map((m) => m.id)).toEqual(["ssrc:2001", "ssrc:2002"]);
  });

  it("WASM mode (no RTP receivers): the active tile becomes a named timeline, and the health says so", () => {
    const r = rig(gallery(), ZOOM_PROFILE);
    r.s.noReceivers = true;
    r.s.level = 0.2; // remote speech arrives, but through no receiver
    r.run(1_900, (t) => light(r.doc, t >= 500 ? BRUNO : null));
    expect(of(r.out, "speaker_active")).toEqual([]);
    r.run(2_000, (t) => light(r.doc, t < 1_000 ? BRUNO : CHIARA));
    const changes = r.out.filter((m) => m.type === "speaker_active" || m.type === "speaker_idle").map((m) => [m.type, m.id, m.type === "speaker_active" ? m.name : undefined, m.source]);
    expect(changes).toEqual([
      ["speaker_active", tileKey(BRUNO), "Bruno Esempio", "dom"],
      ["speaker_idle", tileKey(BRUNO), undefined, "dom"],
      ["speaker_active", tileKey(CHIARA), "Chiara Prova", "dom"],
    ]);
    expect(of(r.out, "speaker_name")).toEqual([]);
    // A screen share closes the timeline: the spotlight is not the speaker.
    r.run(500, () => share(r.doc, true));
    expect(r.out.at(-1)).toMatchObject({ type: "speaker_idle", id: tileKey(CHIARA), source: "dom" });
    share(r.doc, false);
    r.run(20_000, (t) => light(r.doc, t % 2_000 < 1_000 ? BRUNO : CHIARA));
    expect(of(r.out, "observer_health").at(-1)).toMatchObject({ state: "ok", hooks: { ssrc: "missing", speaking: "ok" } });
  });
});

/* Test rig for the name observer (#131, #245, #246): an observer on a
 * fixture document with a fake clock, fake receivers and fake channel
 * levels. Test-only — no entry point imports it. */
import { readFileSync } from "node:fs";
import { join } from "node:path";
import type { ObserverMsg } from "./messages";
import { NameObserver, TICK_MS } from "./observer";
import type { PlatformProfile } from "./profile";

export const fixture = (dir: string, name: string): Document =>
  new DOMParser().parseFromString(readFileSync(join(dir, "fixtures", name), "utf8"), "text/html");

export interface RigState {
  /** RTP sources speaking now. */
  sources: number[];
  /** CSRC profiles: how many receivers carry the mix (Teams: the mix and
   *  its redundant track = 2). SSRC profiles get one receiver per source. */
  copies: number;
  /** Remote level without sources (a transport without RTP receivers). */
  level: number;
  /** Our mic's level. */
  mic: number;
  /** No RTP receivers at all (Zoom's WASM mode). */
  noReceivers: boolean;
}

export function rig(doc: Document, profile: PlatformProfile) {
  let now = 1_000;
  const s: RigState = { sources: [], copies: 1, level: 0, mic: 0, noReceivers: false };
  const out: ObserverMsg[] = [];
  const entries = (list: number[]) => () => list.map((source) => ({ source, timestamp: now - 20, audioLevel: 0.4 }));
  const receivers = () => {
    if (s.noReceivers) return [];
    if (profile.sources.kind === "ssrc") return s.sources.map((src) => ({ getSynchronizationSources: entries([src]) }));
    return Array.from({ length: s.copies }, () => ({ getContributingSources: entries(s.sources) }));
  };
  const obs = new NameObserver({
    doc,
    profile,
    receivers,
    now: () => now,
    timeOrigin: 0,
    remoteLevel: () => (s.sources.length ? 0.2 : s.level),
    micLevel: () => s.mic,
    emit: (m) => out.push(m),
  });
  /** Run `ms` of polls; `step(t)` (t = ms since the run began) sets the
   *  scene first, and the page counts as changed (what the
   *  MutationObserver reports in a browser). */
  const run = (ms: number, step?: (t: number) => void) => {
    for (let t = 0; t < ms; t += TICK_MS) {
      if (step) {
        step(t);
        obs.pageChanged();
      }
      now += TICK_MS;
      obs.tick();
    }
  };
  return { doc, obs, out, run, s, now: () => now };
}

export const of = <T extends ObserverMsg["type"]>(out: ObserverMsg[], type: T) => out.filter((m): m is Extract<ObserverMsg, { type: T }> => m.type === type);

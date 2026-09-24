import { describe, expect, it } from "vitest";
import { TrackRegistry, diffTracks, type TrackLike } from "./registry";

class T implements TrackLike {
  readyState = "live";
  readonly id: string;
  readonly kind: string;
  constructor(id: string, kind = "audio") {
    this.id = id;
    this.kind = kind;
  }
  stop() {
    this.readyState = "ended";
  }
}

const ids = (ts: TrackLike[]) => ts.map((t) => t.id).sort();
const pc = () => ({});
const sender = () => ({});

describe("TrackRegistry: peer connections", () => {
  it("separates the sent mic from the received remote tracks", () => {
    const r = new TrackRegistry<T>();
    const a = pc();
    r.setSender(a, sender(), new T("mic"));
    r.addRemote(a, new T("r1"));
    const s = r.select();
    expect([ids(s.mic), s.micVia]).toEqual([["mic"], "sender"]);
    expect([ids(s.remote), s.remoteVia]).toEqual([["r1"], "peer-connection"]);
    expect(s.wantOwnMic).toBe(false);
  });

  it("mixes several connections and tracks added mid-call into one remote channel", () => {
    const r = new TrackRegistry<T>();
    const a = pc();
    const b = pc();
    r.addRemote(a, new T("r1"));
    r.addRemote(b, new T("r2"));
    r.addRemote(b, new T("r3"));
    r.addRemote(b, new T("v", "video"));
    expect(ids(r.select().remote)).toEqual(["r1", "r2", "r3"]);
  });

  it("follows replaceTrack (device switch) without doubling the mic", () => {
    const r = new TrackRegistry<T>();
    const a = pc();
    const s1 = sender();
    const preview = new T("preview");
    r.addPageGum(preview);
    r.setSender(a, s1, null); // addTransceiver('audio'): nothing sent yet
    expect(r.select().micVia).toBe("page-getUserMedia");
    r.setSender(a, s1, new T("mic1"));
    expect([ids(r.select().mic), r.select().micVia]).toEqual([["mic1"], "sender"]);
    r.setSender(a, s1, new T("mic2"));
    expect(ids(r.select().mic)).toEqual(["mic2"]);
    r.removeSender(s1);
    // Back to what the page got from getUserMedia.
    expect(ids(r.select().mic)).toEqual(["preview"]);
  });

  it("drops ended tracks and closed connections", () => {
    const r = new TrackRegistry<T>();
    const a = pc();
    const mic = new T("mic");
    const r1 = new T("r1");
    r.setSender(a, sender(), mic);
    r.addRemote(a, r1);
    r1.stop();
    r.prune();
    expect(r.select().remote).toEqual([]);
    expect(r.select().remoteVia).toBe("none");
    r.closePc(a);
    expect(r.select().mic).toEqual([]);
    // A connection was seen: no media-element fallback.
    expect(r.pcSeen).toBe(true);
  });

  it("de-duplicates a track seen through several paths", () => {
    const r = new TrackRegistry<T>();
    const a = pc();
    const t = new T("r1");
    r.addRemote(a, t);
    r.addRemote(a, t);
    const mic = new T("mic");
    r.setSender(a, sender(), mic);
    r.setSender(a, sender(), mic);
    expect(r.select().remote).toHaveLength(1);
    expect(r.select().mic).toHaveLength(1);
  });
});

describe("TrackRegistry: fallbacks", () => {
  it("uses media elements only when the page made no peer connection", () => {
    const r = new TrackRegistry<T>();
    const mic = new T("mic");
    r.addPageGum(mic);
    // The self-view element carries the mic: never counted as remote.
    r.setElementTracks([new T("el1"), mic, new T("vid", "video")]);
    const s = r.select();
    expect([ids(s.remote), s.remoteVia]).toEqual([["el1"], "media-element"]);
    expect([ids(s.mic), s.micVia]).toEqual([["mic"], "page-getUserMedia"]);

    r.addPc(pc());
    expect(r.select().remoteVia).toBe("none");
  });

  it("asks for its own mic only in the element fallback, and uses it", () => {
    const r = new TrackRegistry<T>();
    r.setElementTracks([new T("el1")]);
    expect(r.select().wantOwnMic).toBe(true);
    r.addOwnGum(new T("own"));
    const s = r.select();
    expect([ids(s.mic), s.micVia, s.wantOwnMic]).toEqual([["own"], "own-getUserMedia", false]);

    // With peer connections the hook never asks.
    const p = new TrackRegistry<T>();
    p.addRemote(pc(), new T("r1"));
    expect(p.select().wantOwnMic).toBe(false);
  });

  it("reports nothing found (the background may then try tab capture)", () => {
    const s = new TrackRegistry<T>().select();
    expect([s.micVia, s.remoteVia, s.wantOwnMic]).toEqual(["none", "none", false]);
  });
});

it("diffTracks says what to connect and disconnect", () => {
  const a = new T("a");
  const b = new T("b");
  const c = new T("c");
  const d = diffTracks([a, b], [b, c]);
  expect(ids(d.add)).toEqual(["c"]);
  expect(ids(d.remove)).toEqual(["a"]);
});

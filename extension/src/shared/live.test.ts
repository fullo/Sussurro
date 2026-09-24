import { describe, expect, it } from "vitest";
import {
  announceText,
  applyLive,
  backlogView,
  currentItemId,
  emptyMirror,
  formatSeconds,
  initialTranscript,
  lineCount,
  mirrorReceive,
  mirrorSnapshot,
  parseAppMessage,
  type AppMessage,
  type LiveAction,
  type LiveTranscript,
} from "./live";
import { partLines } from "../sidepanel/lines";

const seg = (id: number, text: string, extra: Record<string, unknown> = {}, kind: "new" | "updated" = "new") => ({
  type: "segment",
  kind,
  segment: { id, channel: "remote", start_ms: id * 1000, end_ms: id * 1000 + 900, text, ...extra },
});

const act = (raw: unknown): LiveAction => {
  const msg = parseAppMessage(raw);
  if (!msg) throw new Error(`not an app message: ${JSON.stringify(raw)}`);
  return { kind: "app", msg, at: 1000 };
};

const feed = (msgs: unknown[], from: LiveTranscript = initialTranscript("e1")) => msgs.reduce<LiveTranscript>((t, m) => applyLive(t, act(m)), from);

describe("parseAppMessage", () => {
  it("accepts the app's messages as protocol.rs sends them", () => {
    expect(parseAppMessage(seg(3, "Ciao.", { speaker_id: "voice:1" }))).toEqual({
      type: "segment",
      kind: "new",
      segment: { id: 3, channel: "remote", start_ms: 3000, end_ms: 3900, text: "Ciao.", speaker_id: "voice:1" },
    });
    expect(parseAppMessage({ type: "speaker", id: "meet:Anna", label: "Anna" })).toEqual({ type: "speaker", id: "meet:Anna", label: "Anna" });
    expect(parseAppMessage({ type: "status", state: "recording", session_id: 4, backlog_s: 2.5, processed_s: 10, queue_len: 1 })).toEqual({
      type: "status",
      state: "recording",
      backlog_s: 2.5,
      processed_s: 10,
      queue_len: 1,
    });
    expect(parseAppMessage(seg(1, "", { channel: "mic", failed: true }))).toMatchObject({ segment: { channel: "mic", failed: true } });
  });

  it("ignores anything else", () => {
    for (const bad of [null, "x", [], {}, { type: "dance" }, { type: "segment", kind: "gone", segment: {} }, { type: "segment", kind: "new", segment: { id: "1", start_ms: 0, text: "" } }, { type: "speaker", id: "" }, { type: "status" }]) {
      expect(parseAppMessage(bad)).toBeNull();
    }
  });
});

describe("the live transcript reducer", () => {
  it("merges new and updated segments by id, in time order", () => {
    const t = feed([
      { type: "status", state: "ready", protocol: 1 },
      { type: "status", state: "started", item_id: "2026/09/2026-09-24-sync" },
      seg(0, "Hello."),
      seg(2, "Third."),
      seg(1, "Second."),
      seg(0, "Hello, all.", { speaker_id: "voice:1" }, "updated"),
      seg(7, "Missed as new, arrives as updated.", {}, "updated"),
    ]);
    expect(t.rev).toBe(7);
    expect(t.parts).toHaveLength(1);
    expect(t.parts[0].itemId).toBe("2026/09/2026-09-24-sync");
    expect(t.parts[0].lines.map((l) => [l.id, l.text])).toEqual([
      [0, "Hello, all."],
      [1, "Second."],
      [2, "Third."],
      [7, "Missed as new, arrives as updated."],
    ]);
    expect(t.parts[0].lines[0].speaker_id).toBe("voice:1");
    expect(lineCount(t)).toBe(4);
  });

  it("keeps order when an update moves a line", () => {
    const t = feed([seg(0, "a"), seg(1, "b"), { type: "segment", kind: "updated", segment: { id: 0, channel: "remote", start_ms: 5000, end_ms: 6000, text: "a" } }]);
    expect(t.parts[0].lines.map((l) => l.id)).toEqual([1, 0]);
  });

  it("records speakers and uses them for the chips", () => {
    const t = feed([
      { type: "status", state: "started", item_id: "i1" },
      seg(0, "Hi everyone.", { speaker_id: "meet:Anna" }),
      seg(1, "Hi Anna.", { channel: "mic" }),
      seg(2, "Morning.", { speaker_id: "voice:2" }),
      { type: "speaker", id: "meet:Anna", label: "Anna Rossi" },
    ]);
    const lines = partLines(t.parts[0]);
    expect(lines.map((l) => l.speaker?.label)).toEqual(["Anna Rossi", "You", "Voice 2"]);
    expect(lines[1].speaker?.color).toBe("#1a1a1a");
    expect(lines[2].speaker?.color).toBe("#7e22ce");
    // A later rename wins.
    const renamed = applyLive(t, act({ type: "speaker", id: "voice:2", label: "Bo" }));
    expect(partLines(renamed.parts[0])[2].speaker).toMatchObject({ label: "Bo", color: "#7e22ce" });
    expect(announceText(renamed.parts[0], renamed.parts[0].lines[2])).toBe("Bo: Morning.");
  });

  it("drops blank lines but keeps the ones the engine failed on", () => {
    const t = feed([seg(0, "  "), seg(1, "", { failed: true }), seg(2, "ok")]);
    const lines = partLines(t.parts[0]);
    expect(lines.map((l) => l.id)).toEqual([1, 2]);
    expect(lines[0].sttError).toBeTruthy();
    expect(announceText(t.parts[0], t.parts[0].lines[1])).toBe("not transcribed");
  });

  it("puts a reconnect's new item in a new part and acts on it", () => {
    let t = feed([{ type: "status", state: "started", item_id: "a" }, seg(0, "first connection")]);
    // The socket dropped; the background reconnected: a new `start` = a new item.
    t = feed([{ type: "status", state: "ready" }, { type: "status", state: "started", item_id: "b" }, seg(0, "second connection")], t);
    expect(t.parts.map((p) => [p.itemId, p.lines.map((l) => l.text)])).toEqual([
      ["a", ["first connection"]],
      ["b", ["second connection"]],
    ]);
    expect(currentItemId(t)).toBe("b");
    expect(lineCount(t)).toBe(2);
  });

  it("follows the item's final name on done (an untitled meeting is renamed)", () => {
    const t = feed([{ type: "status", state: "started", item_id: "2026/09/2026-09-24-untitled" }, seg(0, "x"), { type: "status", state: "done", item_id: "2026/09/2026-09-24-weekly-sync" }]);
    expect(t.parts).toHaveLength(1);
    expect(currentItemId(t)).toBe("2026/09/2026-09-24-weekly-sync");
  });

  it("tracks progress and warnings, and a reset starts over", () => {
    let t = feed([{ type: "status", state: "started", item_id: "a" }, { type: "status", state: "recording", backlog_s: 12 }, { type: "status", state: "warning", message: "lost 2 s of audio" }]);
    expect(t.progress).toMatchObject({ state: "recording", backlogS: 12 });
    expect(t.warning).toBe("lost 2 s of audio");
    t = applyLive(t, act({ type: "status", state: "done", item_id: "a" }));
    expect(t.progress).toBeNull();
    const reset = applyLive(t, { kind: "reset" });
    expect(reset).toEqual({ ...initialTranscript("e1"), rev: t.rev + 1 });
    expect(currentItemId(reset)).toBeNull();
  });
});

describe("backlog indicator", () => {
  const at = (state: string, backlog_s: number) => feed([{ type: "status", state, backlog_s }]);
  it("says whether the transcript keeps up", () => {
    expect(backlogView(initialTranscript())).toBeNull();
    expect(backlogView(at("recording", 1.2))).toEqual({ text: "Transcript up to date.", tone: "ok" });
    expect(backlogView(at("recording", 8.4))).toEqual({ text: "Transcript 8 s behind.", tone: "busy" });
    expect(backlogView(at("recording", 95))).toMatchObject({ tone: "lag" });
    expect(backlogView(at("recording", 95))?.text).toMatch(/^Transcript 1 min 35 s behind: .*Nothing is lost/);
    expect(backlogView(at("finishing", 40))?.text).toBe("Finishing: about 40 s of audio left to transcribe.");
    expect(backlogView(at("finishing", 0))?.text).toBe("Finishing the transcript…");
    expect(formatSeconds(120)).toBe("2 min");
  });
});

describe("the panel's mirror of the background's transcript", () => {
  // The background: a transcript and the broadcasts it sends.
  const background = (msgs: AppMessage[], epoch = "e1") => {
    let t = initialTranscript(epoch);
    const sent: { epoch: string; rev: number; action: LiveAction }[] = [];
    for (const msg of msgs) {
      const action: LiveAction = { kind: "app", msg, at: 0 };
      t = applyLive(t, action);
      sent.push({ epoch, rev: t.rev, action });
    }
    return { t, sent };
  };
  const msgs = [seg(0, "a"), seg(1, "b"), seg(2, "c"), seg(1, "B", {}, "updated")].map((m) => parseAppMessage(m)!);

  it("applies broadcasts after a snapshot, in order, ignoring old ones", () => {
    const bg = background(msgs);
    let m = mirrorSnapshot(emptyMirror(), background(msgs.slice(0, 2)).t).m;
    // A broadcast already in the snapshot is ignored.
    let r = mirrorReceive(m, "e1", bg.sent[1].rev, bg.sent[1].action);
    expect([r.applied, r.refetch]).toEqual([false, false]);
    for (const b of bg.sent.slice(2)) {
      r = mirrorReceive(m, b.epoch, b.rev, b.action);
      expect(r.applied).toBe(true);
      m = r.m;
    }
    expect(m.t).toEqual(bg.t);
  });

  it("keeps what arrives before the snapshot and replays the part it lacks", () => {
    const bg = background(msgs);
    let m = emptyMirror();
    for (const b of bg.sent.slice(1)) {
      const r = mirrorReceive(m, b.epoch, b.rev, b.action);
      expect(r.refetch).toBe(false);
      m = r.m;
    }
    const r = mirrorSnapshot(m, background(msgs.slice(0, 2)).t);
    expect(r.refetch).toBe(false);
    expect(r.m.t).toEqual(bg.t);
    expect(r.m.pending).toEqual([]);
  });

  it("asks for a new snapshot on a gap or another epoch (a restarted background)", () => {
    const bg = background(msgs);
    const m = mirrorSnapshot(emptyMirror(), background(msgs.slice(0, 1)).t).m;
    expect(mirrorReceive(m, "e1", bg.sent[2].rev, bg.sent[2].action).refetch).toBe(true);
    const restarted = background(msgs.slice(0, 1), "e2");
    const r = mirrorReceive(m, "e2", restarted.sent[0].rev, restarted.sent[0].action);
    expect([r.applied, r.refetch]).toEqual([false, true]);
    expect(mirrorSnapshot(r.m, restarted.t).m.t).toEqual(restarted.t);
  });

  it("sees a new Start's reset", () => {
    const bg = background(msgs);
    const m = mirrorSnapshot(emptyMirror(), bg.t).m;
    const reset = applyLive(bg.t, { kind: "reset" });
    const r = mirrorReceive(m, "e1", reset.rev, { kind: "reset" });
    expect(r.applied).toBe(true);
    expect(lineCount(r.m.t!)).toBe(0);
  });
});

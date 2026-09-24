import { describe, expect, it } from "vitest";
import { actionsView, isRecording, type ActionsInput } from "./actions";

const input = (p: Partial<ActionsInput>): ActionsInput => ({ itemId: "2026/09/sync", lines: 3, phase: "live", subtitles: "on_request", busy: null, ...p });
const shown = (i: ActionsInput) => {
  const v = actionsView(i);
  return Object.fromEntries(Object.entries(v).map(([k, s]) => [k, s.visible ? (s.enabled ? "on" : "off") : "hidden"]));
};

describe("item buttons", () => {
  it("appear once the app created the meeting's item", () => {
    expect(shown(input({ itemId: null, lines: 0, phase: "connecting" }))).toEqual({ open: "hidden", copy: "hidden", srt: "hidden" });
    expect(shown(input({ lines: 0 }))).toEqual({ open: "on", copy: "off", srt: "off" });
    expect(actionsView(input({ lines: 0 })).copy.hint).toBe("Nothing transcribed yet.");
  });

  it("offer Open and Copy during the meeting, Create .srt after it", () => {
    expect(shown(input({}))).toEqual({ open: "on", copy: "on", srt: "off" });
    expect(actionsView(input({})).srt.hint).toMatch(/when the recording ends/);
    expect(shown(input({ phase: "stopping" }))).toMatchObject({ srt: "off" });
    for (const phase of ["done", "error", "idle"] as const) expect(shown(input({ phase }))).toEqual({ open: "on", copy: "on", srt: "on" });
  });

  it("show Create .srt only when the app writes subtitles on request", () => {
    expect(shown(input({ phase: "done", subtitles: "always" })).srt).toBe("hidden");
    expect(shown(input({ phase: "done", subtitles: undefined })).srt).toBe("hidden");
  });

  it("run one action at a time", () => {
    expect(shown(input({ phase: "done", busy: "copy" }))).toEqual({ open: "off", copy: "off", srt: "off" });
  });

  it("know when a recording is running", () => {
    expect(["checking", "arming", "connecting", "live", "reconnecting", "stopping"].every((p) => isRecording(p as never))).toBe(true);
    expect([null, "idle", "done", "error"].some((p) => isRecording(p as never))).toBe(false);
  });
});

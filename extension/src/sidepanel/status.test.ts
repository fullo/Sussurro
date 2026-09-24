import { describe, expect, it } from "vitest";
import type { PanelState } from "../shared/messages";
import { panelView, viaText } from "./status";

const state = (p: Partial<PanelState>): PanelState => ({
  tabId: 1,
  meetingPage: true,
  platform: "meet",
  paired: true,
  app: { ok: true, version: "0.9.0" },
  phase: "idle",
  attempt: 0,
  capture: null,
  tabCapture: "off",
  transport: null,
  ...p,
});

describe("panelView", () => {
  it("offers Start only on a paired meeting page, and says nothing is captured yet", () => {
    const v = panelView(state({}));
    expect(v).toMatchObject({ canStart: true, canStop: false, tone: "neutral" });
    expect(v.line).toMatch(/Google Meet.*Nothing is captured/);
    expect(panelView(state({ paired: false }))).toMatchObject({ canStart: false, tone: "problem" });
    expect(panelView(state({ meetingPage: false, platform: null }))).toMatchObject({ canStart: false });
    expect(panelView(null)).toMatchObject({ canStart: false, canStop: false });
  });

  it("offers Stop while capturing or reconnecting", () => {
    for (const phase of ["checking", "arming", "connecting", "live", "reconnecting"] as const) {
      expect(panelView(state({ phase }))).toMatchObject({ canStart: false, canStop: true });
    }
    expect(panelView(state({ phase: "live", platform: "zoom" })).line).toMatch(/Recording Zoom/);
    expect(panelView(state({ phase: "reconnecting", attempt: 3, problem: "not_running" })).line).toMatch(/attempt 3.*not reachable/);
  });

  it("explains problems and lets the user try again", () => {
    expect(panelView(state({ app: { ok: false, problem: "not_running" } }))).toMatchObject({ tone: "problem", canStart: true });
    const err = panelView(state({ phase: "error", problem: "meetings_disabled" }));
    expect(err.line).toMatch(/Meetings are off/);
    expect(err.canStart).toBe(true);
    expect(panelView(state({ phase: "error", message: "boom" })).line).toBe("boom");
  });

  it("words the channel sources", () => {
    expect(viaText("peer-connection")).toBe("call audio");
    expect(viaText("own-getUserMedia")).toMatch(/extra permission/);
    expect(viaText("none")).toBe("not found yet");
  });
});

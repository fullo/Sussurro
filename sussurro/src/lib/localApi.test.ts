import { describe, expect, it } from "vitest";
import { apiNotice } from "./localApi";

describe("apiNotice", () => {
  it("is ok only when the API listens on the configured port", () => {
    expect(apiNotice(true, 4525, { state: "listening", port: 4525 })).toEqual({
      tone: "ok",
      text: "Local API listening on 127.0.0.1:4525.",
      offerEnable: false,
    });
  });

  it("offers to switch the API on when it is off in the settings", () => {
    for (const status of [null, { state: "off" } as const, { state: "listening", port: 4525 } as const]) {
      const n = apiNotice(false, 4525, status);
      expect(n.tone).toBe("warn");
      expect(n.offerEnable).toBe(true);
    }
    expect(apiNotice(false, 4525, { state: "listening", port: 4525 }).text).toMatch(/stops when Sussurro restarts/);
    expect(apiNotice(false, 4525, null).text).toMatch(/restart/);
  });

  it("asks for a restart when the settings changed since startup", () => {
    const moved = apiNotice(true, 5000, { state: "listening", port: 4525 });
    expect(moved.tone).toBe("warn");
    expect(moved.text).toMatch(/listens on port 4525; port 5000 applies when Sussurro restarts/);
    expect(apiNotice(true, 4525, { state: "off" }).text).toMatch(/starts when Sussurro restarts/);
    expect(apiNotice(true, 4525, null).tone).toBe("warn");
  });

  it("explains a port that could not be bound", () => {
    expect(apiNotice(true, 4525, { state: "failed", port: 4525 }).text).toMatch(/another program may be using it/);
    expect(apiNotice(true, 5000, { state: "failed", port: 4525 }).text).toMatch(/Restart Sussurro to try port 5000/);
  });
});

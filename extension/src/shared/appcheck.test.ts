import { describe, expect, it } from "vitest";
import { backoffDelay, RECONNECT } from "./backoff";
import { isFatal, problemText, toAppCheck, type AppProblem } from "./appcheck";

describe("toAppCheck", () => {
  it("passes a compatible app", () => {
    expect(toAppCheck({ kind: "ok", app: "0.9.0", protocol: 1 })).toEqual({ ok: true, app: "0.9.0" });
    expect(toAppCheck({ kind: "ok", app: "0.9.0", protocol: 1, subtitles: "on_request" })).toEqual({ ok: true, app: "0.9.0", subtitles: "on_request" });
  });

  it("names every failure, not paired included", () => {
    expect(toAppCheck(null)).toEqual({ ok: false, reason: "not_paired" });
    expect(toAppCheck({ kind: "not_running" })).toEqual({ ok: false, reason: "not_running" });
    expect(toAppCheck({ kind: "bad_token" })).toEqual({ ok: false, reason: "bad_token" });
    expect(toAppCheck({ kind: "protocol_mismatch", app: "1.0.0", protocol: 2 })).toMatchObject({ reason: "protocol_mismatch" });
    expect(toAppCheck({ kind: "unexpected", status: 500 })).toEqual({ ok: false, reason: "unexpected", detail: "HTTP 500" });
  });

  it("retries only what may fix itself, and words everything", () => {
    const all: AppProblem[] = ["not_paired", "not_running", "timeout", "blocked", "bad_token", "forbidden", "app_outdated", "protocol_mismatch", "unexpected"];
    expect(all.filter((p) => !isFatal(p))).toEqual(["not_running", "timeout", "unexpected"]);
    for (const p of all) expect(problemText(p)).toBeTruthy();
  });
});

describe("backoffDelay", () => {
  it("grows exponentially between half and all of the ceiling", () => {
    expect(backoffDelay(1, RECONNECT, () => 0)).toBe(250);
    expect(backoffDelay(1, RECONNECT, () => 1)).toBe(500);
    expect(backoffDelay(2, RECONNECT, () => 1)).toBe(1000);
    expect(backoffDelay(4, RECONNECT, () => 0.5)).toBe(3000);
  });

  it("is capped and tolerates odd input", () => {
    expect(backoffDelay(50, RECONNECT, () => 1)).toBe(RECONNECT.maxMs);
    expect(backoffDelay(0, RECONNECT, () => 1)).toBe(500);
    expect(backoffDelay(3, RECONNECT, () => 7)).toBe(2000);
  });
});

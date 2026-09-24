import { describe, expect, it } from "vitest";
import { backoffDelay, RECONNECT } from "./backoff";
import { DEFAULT_PORT, classifyVersion, isFatal, liveUrl, parsePairing, problemText, versionUrl } from "./pairing";

const TOKEN = "ab".repeat(32);

describe("parsePairing", () => {
  it("reads {port, token}, defaulting the port to the app's", () => {
    expect(parsePairing({ port: 5000, token: ` ${TOKEN} ` })).toEqual({ port: 5000, token: TOKEN });
    expect(parsePairing({ token: TOKEN })).toEqual({ port: DEFAULT_PORT, token: TOKEN });
    expect(parsePairing({ port: "4600", token: TOKEN })).toEqual({ port: 4600, token: TOKEN });
  });

  it("treats anything else as not paired", () => {
    for (const bad of [undefined, null, "x", {}, { token: "" }, { token: "a b" }, { token: TOKEN, port: 0 }, { token: TOKEN, port: 70000 }, { token: TOKEN, port: 1.5 }]) {
      expect(parsePairing(bad)).toBeNull();
    }
  });
});

it("builds loopback URLs, the token only in the WebSocket query", () => {
  const p = { port: 4525, token: "a+b/c" };
  expect(versionUrl(p)).toBe("http://127.0.0.1:4525/app/version");
  expect(liveUrl(p)).toBe("ws://127.0.0.1:4525/live?token=a%2Bb%2Fc");
});

describe("classifyVersion", () => {
  it("accepts the same protocol", () => {
    expect(classifyVersion(200, { app: "0.9.0", protocol: 1 })).toEqual({ ok: true, app: "0.9.0" });
  });

  it("tells the failures apart", () => {
    const reason = (s: number | null, b: unknown = null) => {
      const r = classifyVersion(s, b);
      return r.ok ? "ok" : r.reason;
    };
    expect(reason(null)).toBe("not-running");
    expect(reason(401)).toBe("bad-token");
    expect(reason(403)).toBe("forbidden");
    expect(reason(404)).toBe("meetings-disabled");
    expect(reason(500)).toBe("error");
    expect(reason(200, { app: "1.0.0", protocol: 2 })).toBe("protocol-mismatch");
    expect(reason(200, "garbage")).toBe("protocol-mismatch");
  });

  it("retries only what may fix itself", () => {
    expect(isFatal("not-running")).toBe(false);
    expect(isFatal("error")).toBe(false);
    for (const p of ["not-paired", "bad-token", "forbidden", "meetings-disabled", "protocol-mismatch"] as const) {
      expect(isFatal(p)).toBe(true);
      expect(problemText(p)).toBeTruthy();
    }
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

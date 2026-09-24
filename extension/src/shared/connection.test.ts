import { describe, expect, it, vi } from "vitest";

vi.mock("webextension-polyfill", () => ({ default: {} }));

const { classifyResponse, describeResult, testConnection } = await import("./connection");
const { PROTOCOL_VERSION } = await import("./pairing");
type ConnectionResult = import("./connection").ConnectionResult;

const TOKEN = "0123456789abcdef".repeat(4);
const PAIRING = { port: 4525, token: TOKEN };

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });

describe("classifyResponse", () => {
  it("accepts the app's handshake with our protocol", () => {
    expect(classifyResponse(200, { app: "0.9.0", protocol: PROTOCOL_VERSION })).toEqual({
      kind: "ok",
      app: "0.9.0",
      protocol: PROTOCOL_VERSION,
    });
  });

  it("reads the app's subtitles setting (#129), ignoring unknown values", () => {
    expect(classifyResponse(200, { app: "0.9.0", protocol: PROTOCOL_VERSION, subtitles: "on_request" })).toMatchObject({ kind: "ok", subtitles: "on_request" });
    expect(classifyResponse(200, { app: "0.9.0", protocol: PROTOCOL_VERSION, subtitles: "always" })).toMatchObject({ kind: "ok", subtitles: "always" });
    expect(classifyResponse(200, { app: "0.9.0", protocol: PROTOCOL_VERSION, subtitles: "sometimes" })).not.toHaveProperty("subtitles");
  });

  it("refuses another protocol", () => {
    expect(classifyResponse(200, { app: "1.2.0", protocol: PROTOCOL_VERSION + 1 })).toEqual({
      kind: "protocol_mismatch",
      app: "1.2.0",
      protocol: PROTOCOL_VERSION + 1,
    });
  });

  it("maps the app's refusals", () => {
    expect(classifyResponse(401, { error: "missing or wrong extension token" })).toEqual({ kind: "bad_token" });
    expect(classifyResponse(403, { error: "origin not allowed" })).toEqual({ kind: "forbidden" });
    expect(classifyResponse(404, { error: "unknown endpoint" })).toEqual({ kind: "meetings_disabled" });
  });

  it("flags anything else as unexpected", () => {
    expect(classifyResponse(200, null)).toEqual({ kind: "unexpected", status: 200 });
    expect(classifyResponse(200, { app: 1, protocol: "1" })).toEqual({ kind: "unexpected", status: 200 });
    expect(classifyResponse(500, { error: "x" })).toEqual({ kind: "unexpected", status: 500 });
  });
});

describe("testConnection", () => {
  it("sends the token as a bearer header to the loopback port", async () => {
    const fetchImpl = vi.fn(async () => json(200, { app: "0.9.0", protocol: PROTOCOL_VERSION }));
    expect(await testConnection(PAIRING, { fetchImpl })).toEqual({ kind: "ok", app: "0.9.0", protocol: PROTOCOL_VERSION });
    const [url, init] = fetchImpl.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe("http://127.0.0.1:4525/app/version");
    expect(url).not.toContain(TOKEN);
    expect(init.headers).toEqual({ Authorization: `Bearer ${TOKEN}` });
    expect(init.credentials).toBe("omit");
  });

  it("tells the app not running from a blocked reply", async () => {
    const refused = vi.fn(async () => {
      throw new TypeError("Failed to fetch");
    });
    expect(await testConnection(PAIRING, { fetchImpl: refused })).toEqual({ kind: "not_running" });
    expect(refused).toHaveBeenCalledTimes(2);

    // The authenticated request fails (CORS), the opaque probe gets through.
    const blocked = vi.fn(async (_url: string, init?: RequestInit) => {
      if (init?.mode === "no-cors") return new Response(null, { status: 200 });
      throw new TypeError("Failed to fetch");
    });
    expect(await testConnection(PAIRING, { fetchImpl: blocked })).toEqual({ kind: "blocked" });
    const probe = blocked.mock.calls[1][1] as RequestInit;
    expect(probe.headers).toBeUndefined();
  });

  it("reports a timeout", async () => {
    const slow = vi.fn(async () => {
      throw new DOMException("The operation timed out.", "TimeoutError");
    });
    expect(await testConnection(PAIRING, { fetchImpl: slow })).toEqual({ kind: "timeout" });
  });

  it("maps HTTP answers, including non-JSON ones", async () => {
    const answer = (r: Response) => testConnection(PAIRING, { fetchImpl: async () => r });
    expect(await answer(json(401, { error: "x" }))).toEqual({ kind: "bad_token" });
    expect(await answer(json(404, { error: "unknown endpoint" }))).toEqual({ kind: "meetings_disabled" });
    expect(await answer(new Response("<html>hello</html>", { status: 200 }))).toEqual({ kind: "unexpected", status: 200 });
  });
});

describe("describeResult", () => {
  const all: ConnectionResult[] = [
    { kind: "ok", app: "0.9.0", protocol: PROTOCOL_VERSION },
    { kind: "protocol_mismatch", app: "1.0.0", protocol: PROTOCOL_VERSION + 1 },
    { kind: "protocol_mismatch", app: "0.8.0", protocol: PROTOCOL_VERSION - 1 },
    { kind: "not_running" },
    { kind: "timeout" },
    { kind: "blocked" },
    { kind: "bad_token" },
    { kind: "forbidden" },
    { kind: "meetings_disabled" },
    { kind: "unexpected", status: 500 },
  ];

  it("gives every outcome a distinct title, ok only for ok", () => {
    const msgs = all.map((r) => describeResult(r, 4525));
    expect(msgs.map((m) => m.ok)).toEqual(all.map((r) => r.kind === "ok"));
    const titles = new Set(all.filter((r) => r.kind !== "protocol_mismatch").map((r) => describeResult(r, 4525).title));
    expect(titles.size).toBe(all.length - 2);
  });

  it("says what to do", () => {
    expect(describeResult({ kind: "not_running" }, 5000).detail).toMatch(/127\.0\.0\.1:5000.*Start Sussurro/);
    expect(describeResult({ kind: "bad_token" }, 4525).detail).toMatch(/Copy the pairing code again/);
    expect(describeResult({ kind: "meetings_disabled" }, 4525).detail).toMatch(/Turn on Meetings/);
    expect(describeResult(all[1], 4525).detail).toMatch(/update the extension/);
    expect(describeResult(all[2], 4525).detail).toMatch(/update Sussurro/);
    expect(describeResult(all[0], 4525).title).toBe("Connected to Sussurro 0.9.0");
  });

  it("never mentions the token", () => {
    for (const r of all) expect(JSON.stringify(describeResult(r, 4525))).not.toContain(TOKEN);
  });
});

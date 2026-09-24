import { describe, expect, it, vi } from "vitest";

vi.mock("webextension-polyfill", () => ({ default: {} }));

const { describeFailure, exportFilename, exportItem, itemPath, openItem } = await import("./items");

const TOKEN = "0123456789abcdef".repeat(4);
const PAIRING = { port: 4525, token: TOKEN };
const ID = "2026/09/2026-09-24-weekly sync";

type Call = { url: string; init?: RequestInit };
const fakeFetch = (res: () => Response | Promise<Response>) => {
  const calls: Call[] = [];
  const f = async (url: string, init?: RequestInit) => {
    calls.push({ url, init });
    return res();
  };
  return { f, calls };
};

describe("item routes", () => {
  it("build the app's paths, keeping the id's slashes", () => {
    expect(itemPath(ID, "open")).toBe("/items/2026/09/2026-09-24-weekly%20sync/open");
    expect(itemPath("a?b#c", "export")).toBe("/items/a%3Fb%23c/export");
    expect(exportFilename(ID, "srt")).toBe("2026-09-24-weekly sync.srt");
    expect(exportFilename("x/a:b", "txt")).toBe("a_b.txt");
  });

  it("open the item with the token in the header only", async () => {
    const { f, calls } = fakeFetch(() => new Response('{"ok":true}', { status: 200 }));
    expect(await openItem(PAIRING, ID, f)).toEqual({ ok: true, value: true });
    expect(calls[0].url).toBe("http://127.0.0.1:4525/items/2026/09/2026-09-24-weekly%20sync/open");
    expect(calls[0].init?.method).toBe("POST");
    expect(calls[0].init?.headers).toEqual({ Authorization: `Bearer ${TOKEN}` });
    expect(calls[0].url).not.toContain(TOKEN);
  });

  it("export as text or subtitles", async () => {
    const { f, calls } = fakeFetch(() => new Response("[00:00:01] Ciao.\n", { status: 200 }));
    expect(await exportItem(PAIRING, ID, "txt", f)).toEqual({ ok: true, value: "[00:00:01] Ciao.\n" });
    await exportItem(PAIRING, ID, "srt", f);
    expect(calls.map((c) => [c.init?.method, new URL(c.url).search])).toEqual([
      ["GET", "?format=txt"],
      ["GET", "?format=srt"],
    ]);
  });

  it("explain failures without the token", async () => {
    const down = await openItem(PAIRING, ID, async () => {
      throw new TypeError("Failed to fetch");
    });
    expect(down).toEqual({ ok: false, error: "The Sussurro app is not reachable." });
    const refused = fakeFetch(() => new Response('{"error":"notes have no subtitles"}', { status: 422 }));
    const r = await exportItem(PAIRING, ID, "srt", refused.f);
    expect(r).toEqual({ ok: false, error: "Sussurro can't export it: notes have no subtitles." });
    expect(describeFailure(401, null)).toMatch(/Pair the extension again/);
    expect(describeFailure(404, { error: "unknown endpoint" })).toMatch(/Meetings are off/);
    expect(describeFailure(404, { error: "no item 2026/09/x" })).toMatch(/deleted or moved/);
    expect(describeFailure(500, null)).toBe("Sussurro answered with HTTP 500.");
    for (const s of [401, 403, 404, 500]) expect(describeFailure(s, { error: TOKEN })).not.toContain(TOKEN);
  });
});

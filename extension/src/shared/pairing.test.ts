import { readFileSync, readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it, vi } from "vitest";
import { restrictToTrustedContexts } from "./secureStore";

// The polyfill refuses to load outside an extension. The storage area is
// injected in most tests; the default one (for onPairingChanged) is fixed.
const { listeners } = vi.hoisted(() => ({
  listeners: [] as ((changes: Record<string, unknown>, area: string) => void)[],
}));
vi.mock("webextension-polyfill", () => ({
  default: {
    storage: {
      local: { get: async () => ({ port: 4525, token: "0123456789abcdef".repeat(4) }) },
      onChanged: {
        addListener: (l: (typeof listeners)[number]) => listeners.push(l),
        removeListener: (l: (typeof listeners)[number]) => listeners.splice(listeners.indexOf(l), 1),
      },
    },
  },
}));

const {
  PAIRING_KEYS,
  authHeaders,
  appUrl,
  clearPairing,
  encodePairingCode,
  getPairing,
  liveAuth,
  liveUrl,
  pairingFromItems,
  parsePairingCode,
  setPairing,
} = await import("./pairing");
type PairingStorage = import("./pairing").PairingStorage;

const TOKEN = "0123456789abcdef".repeat(4);

/** An in-memory `storage.local`. */
function memoryStorage(initial: Record<string, unknown> = {}): PairingStorage & { items: Record<string, unknown> } {
  const items = { ...initial };
  return {
    items,
    async get(keys) {
      return Object.fromEntries(keys.filter((k) => k in items).map((k) => [k, items[k]]));
    },
    async set(next) {
      Object.assign(items, next);
    },
    async remove(keys) {
      for (const k of keys) delete items[k];
    },
  };
}

/** The secure store and `storage.local`, both in memory. */
function memoryStores(legacy: Record<string, unknown> = {}) {
  return { secure: memoryStorage(), legacy: memoryStorage(legacy) };
}

describe("storage", () => {
  it("round-trips a pairing under the shared keys, never in storage.local", async () => {
    const s = memoryStores();
    expect(await getPairing(s)).toBeNull();
    await setPairing({ port: 4525, token: TOKEN }, s);
    expect(s.secure.items).toEqual({ [PAIRING_KEYS.port]: 4525, [PAIRING_KEYS.token]: TOKEN });
    expect(s.legacy.items).toEqual({});
    expect(PAIRING_KEYS).toEqual({ port: "port", token: "token" });
    expect(await getPairing(s)).toEqual({ port: 4525, token: TOKEN });
    await clearPairing(s);
    expect(s.secure.items).toEqual({});
    expect(await getPairing(s)).toBeNull();
  });

  it("moves a pairing an older version left in storage.local (#217)", async () => {
    const s = memoryStores({ port: 4600, token: TOKEN, recordingNoticeSeen: true });
    expect(await getPairing(s)).toEqual({ port: 4600, token: TOKEN });
    expect(s.secure.items).toEqual({ port: 4600, token: TOKEN });
    expect(s.legacy.items).toEqual({ recordingNoticeSeen: true }); // only the pairing leaves
    expect(await getPairing(s)).toEqual({ port: 4600, token: TOKEN });
    // A newer write there (a downgrade and back) wins over the stored one.
    s.legacy.items.port = 4700;
    s.legacy.items.token = "f".repeat(64);
    expect(await getPairing(s)).toEqual({ port: 4700, token: "f".repeat(64) });
    expect(s.legacy.items).toEqual({ recordingNoticeSeen: true });
    // Junk there is removed and ignored.
    s.legacy.items.token = "junk";
    expect(await getPairing(s)).toEqual({ port: 4700, token: "f".repeat(64) });
    expect(s.legacy.items).toEqual({ recordingNoticeSeen: true });
    // Save and forget clear what an old version left too.
    s.legacy.items.token = TOKEN;
    s.legacy.items.port = 1;
    await clearPairing(s);
    expect([s.secure.items, s.legacy.items]).toEqual([{}, { recordingNoticeSeen: true }]);
  });

  it("keeps using storage.local when the secure store can't be written", async () => {
    const s = memoryStores({ port: 4600, token: TOKEN });
    s.secure.set = async () => {
      throw new Error("no IndexedDB");
    };
    expect(await getPairing(s)).toEqual({ port: 4600, token: TOKEN });
    expect(s.legacy.items).toEqual({ port: 4600, token: TOKEN }); // not lost
  });

  it("normalizes on save and refuses an invalid pairing", async () => {
    const s = memoryStores();
    expect(await setPairing({ port: 4525, token: ` ${TOKEN.toUpperCase()} ` }, s)).toEqual({ port: 4525, token: TOKEN });
    expect(s.secure.items.token).toBe(TOKEN);
    const before = { ...s.secure.items };
    await expect(setPairing({ port: 0, token: TOKEN }, s)).rejects.toThrow(/Invalid pairing/);
    await expect(setPairing({ port: 4525, token: "short" }, s)).rejects.toThrow(/Invalid pairing/);
    expect(s.secure.items).toEqual(before);
  });

  it("treats damaged or partial storage as not paired", () => {
    expect(pairingFromItems({})).toBeNull();
    expect(pairingFromItems({ port: 4525 })).toBeNull();
    expect(pairingFromItems({ token: TOKEN })).toBeNull();
    expect(pairingFromItems({ port: 4525, token: "x" })).toBeNull();
    expect(pairingFromItems({ port: 99999, token: TOKEN })).toBeNull();
    expect(pairingFromItems({ port: { n: 1 }, token: TOKEN })).toBeNull();
    expect(pairingFromItems({ port: "4525", token: TOKEN })).toEqual({ port: 4525, token: TOKEN });
  });

  it("notifies pairing changes announced by other pages or written to storage.local", async () => {
    const { onPairingChanged } = await import("./pairing");
    const s = memoryStores();
    const seen: unknown[] = [];
    const off = onPairingChanged((p) => seen.push(p), s);
    expect(listeners).toHaveLength(1);
    listeners[0]({ other: {} }, "local");
    listeners[0]({ token: {} }, "sync");
    expect(seen).toEqual([]);
    // The old way: a write to storage.local, moved on the way.
    s.legacy.items.port = 4525;
    s.legacy.items.token = TOKEN;
    listeners[0]({ token: {} }, "local");
    await vi.waitFor(() => expect(seen).toContainEqual({ port: 4525, token: TOKEN }));
    expect(s.legacy.items).toEqual({});
    // Another page saved (announced on the extension's BroadcastChannel).
    seen.length = 0;
    await setPairing({ port: 5000, token: TOKEN }, memoryStores());
    s.secure.items.port = 5000;
    await vi.waitFor(() => expect(seen).toContainEqual({ port: 5000, token: TOKEN }));
    off();
    expect(listeners).toHaveLength(0);
  });
});

describe("storage access (#217)", () => {
  it("restricts storage.local to trusted contexts where the browser can", async () => {
    const calls: unknown[] = [];
    expect(await restrictToTrustedContexts({ setAccessLevel: async (a) => void calls.push(a) })).toBe(true);
    expect(calls).toEqual([{ accessLevel: "TRUSTED_CONTEXTS" }]);
    // Chrome 116–139 refuses it for storage.local; Firefox has no such call.
    const old = {
      setAccessLevel: async () => {
        throw new Error("This StorageArea does not support setting access level");
      },
    };
    expect(await restrictToTrustedContexts(old)).toBe(false);
    expect(await restrictToTrustedContexts({})).toBe(false);
    expect(await restrictToTrustedContexts(undefined)).toBe(false);
  });

  it("content scripts never import the pairing or the secure store", () => {
    // They share storage.local with the web page's renderer: nothing there
    // may reach for the token (the build bundles each entry on its own).
    const dir = fileURLToPath(new URL("../content/", import.meta.url));
    const files = readdirSync(dir, { recursive: true, encoding: "utf8" }).filter((f) => f.endsWith(".ts") && !f.endsWith(".test.ts"));
    expect(files.length).toBeGreaterThan(3);
    for (const f of files) {
      const src = readFileSync(`${dir}/${f}`, "utf8");
      expect(src, f).not.toMatch(/shared\/(pairing|secureStore)|storage\.(local|session|sync)|indexedDB/);
    }
  });
});

describe("the pairing code the app copies", () => {
  it("parses into what setPairing stores", async () => {
    const parsed = parsePairingCode(encodePairingCode({ port: 5000, token: TOKEN }));
    expect(parsed.ok).toBe(true);
    const s = memoryStores();
    if (parsed.ok) await setPairing(parsed.value, s);
    expect(await getPairing(s)).toEqual({ port: 5000, token: TOKEN });
  });
});

describe("URLs and headers", () => {
  it("target the loopback address only", () => {
    expect(appUrl(4525, "/app/version")).toBe("http://127.0.0.1:4525/app/version");
    expect(liveUrl({ port: 4525, token: TOKEN })).toBe(`ws://127.0.0.1:4525/live?token=${TOKEN}`);
    // An app that takes the token as the first message (#217): not in the URL.
    expect(liveUrl({ port: 4525, token: TOKEN }, true)).toBe("ws://127.0.0.1:4525/live");
    expect(JSON.parse(liveAuth({ port: 4525, token: TOKEN }))).toEqual({ type: "auth", token: TOKEN });
    expect(authHeaders({ port: 4525, token: TOKEN })).toEqual({ Authorization: `Bearer ${TOKEN}` });
  });
});

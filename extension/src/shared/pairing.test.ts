import { describe, expect, it, vi } from "vitest";

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

describe("storage", () => {
  it("round-trips a pairing under the shared keys", async () => {
    const s = memoryStorage();
    expect(await getPairing(s)).toBeNull();
    await setPairing({ port: 4525, token: TOKEN }, s);
    expect(s.items).toEqual({ [PAIRING_KEYS.port]: 4525, [PAIRING_KEYS.token]: TOKEN });
    expect(PAIRING_KEYS).toEqual({ port: "port", token: "token" });
    expect(await getPairing(s)).toEqual({ port: 4525, token: TOKEN });
    await clearPairing(s);
    expect(s.items).toEqual({});
    expect(await getPairing(s)).toBeNull();
  });

  it("normalizes on save and refuses an invalid pairing", async () => {
    const s = memoryStorage();
    expect(await setPairing({ port: 4525, token: ` ${TOKEN.toUpperCase()} ` }, s)).toEqual({ port: 4525, token: TOKEN });
    expect(s.items.token).toBe(TOKEN);
    const before = { ...s.items };
    await expect(setPairing({ port: 0, token: TOKEN }, s)).rejects.toThrow(/Invalid pairing/);
    await expect(setPairing({ port: 4525, token: "short" }, s)).rejects.toThrow(/Invalid pairing/);
    expect(s.items).toEqual(before);
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

  it("notifies pairing changes in storage.local only", async () => {
    const { onPairingChanged } = await import("./pairing");
    const seen: unknown[] = [];
    const off = onPairingChanged((p) => seen.push(p));
    expect(listeners).toHaveLength(1);
    listeners[0]({ other: {} }, "local");
    listeners[0]({ token: {} }, "sync");
    expect(seen).toEqual([]);
    listeners[0]({ token: {} }, "local");
    await vi.waitFor(() => expect(seen).toEqual([{ port: 4525, token: TOKEN }]));
    off();
    expect(listeners).toHaveLength(0);
  });
});

describe("the pairing code the app copies", () => {
  it("parses into what setPairing stores", async () => {
    const parsed = parsePairingCode(encodePairingCode({ port: 5000, token: TOKEN }));
    expect(parsed.ok).toBe(true);
    const s = memoryStorage();
    if (parsed.ok) await setPairing(parsed.value, s);
    expect(await getPairing(s)).toEqual({ port: 5000, token: TOKEN });
  });
});

describe("URLs and headers", () => {
  it("target the loopback address only", () => {
    expect(appUrl(4525, "/app/version")).toBe("http://127.0.0.1:4525/app/version");
    expect(liveUrl({ port: 4525, token: TOKEN })).toBe(`ws://127.0.0.1:4525/live?token=${TOKEN}`);
    expect(authHeaders({ port: 4525, token: TOKEN })).toEqual({ Authorization: `Bearer ${TOKEN}` });
  });
});

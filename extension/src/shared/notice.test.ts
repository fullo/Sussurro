import { describe, expect, it, vi } from "vitest";

// The polyfill refuses to load outside an extension: a fake storage.onChanged.
const { listeners } = vi.hoisted(() => ({
  listeners: [] as ((changes: Record<string, { newValue?: unknown }>, area: string) => void)[],
}));
vi.mock("webextension-polyfill", () => ({
  default: {
    storage: {
      local: { get: async () => ({}) },
      onChanged: {
        addListener: (l: (typeof listeners)[number]) => listeners.push(l),
        removeListener: (l: (typeof listeners)[number]) => listeners.splice(listeners.indexOf(l), 1),
      },
    },
  },
}));

const { NOTICE_KEY, RECORDING_NOTICE, RECORDING_PRIVACY_URL, DONT_SHOW_AGAIN_DEFAULT, answerNotice, isNoticeNeeded, onNoticeChanged, resetNotice, startStep } =
  await import("./notice");
const { PAIRING_KEYS, clearPairing, setPairing } = await import("./pairing");
type NoticeStorage = import("./notice").NoticeStorage;

/** An in-memory `storage.local`, with `clear()` like the real one. */
function memoryStorage(initial: Record<string, unknown> = {}): NoticeStorage & { items: Record<string, unknown>; clear(): void } {
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
    clear() {
      for (const k of Object.keys(items)) delete items[k];
    },
  };
}

describe("recording notice before the first Start (#136)", () => {
  it("is needed on a fresh install", async () => {
    expect(await isNoticeNeeded(memoryStorage())).toBe(true);
  });

  it("is remembered only when going ahead with “Don't show this again”", async () => {
    const s = memoryStorage();
    expect(await answerNotice(false, true, s)).toBe(false); // Cancel
    expect(await answerNotice(true, false, s)).toBe(false); // unticked
    expect(s.items).toEqual({});
    expect(await isNoticeNeeded(s)).toBe(true);
    expect(await answerNotice(true, true, s)).toBe(true);
    expect(s.items).toEqual({ [NOTICE_KEY]: true });
    expect(await isNoticeNeeded(s)).toBe(false);
  });

  it("shows again when storage.local is cleared or the options reset it", async () => {
    const s = memoryStorage({ [NOTICE_KEY]: true });
    s.clear();
    expect(await isNoticeNeeded(s)).toBe(true);
    await answerNotice(true, true, s);
    await resetNotice(s);
    expect(await isNoticeNeeded(s)).toBe(true);
  });

  it("survives Forget / Pair again: its key is not the pairing's", async () => {
    const s = memoryStorage();
    await answerNotice(true, true, s);
    // storage.local is the pairing's old home (#217): it is cleared there too.
    const stores = { secure: memoryStorage(), legacy: s };
    await setPairing({ port: 4525, token: "0123456789abcdef".repeat(4) }, stores);
    await clearPairing(stores);
    expect(Object.values(PAIRING_KEYS)).not.toContain(NOTICE_KEY);
    expect(await isNoticeNeeded(s)).toBe(false);
  });

  it("only a stored true counts; an unreadable storage asks", async () => {
    for (const v of [false, "true", 1, null]) expect(await isNoticeNeeded(memoryStorage({ [NOTICE_KEY]: v }))).toBe(true);
    const broken: NoticeStorage = { get: () => Promise.reject(new Error("no")), set: async () => {}, remove: async () => {} };
    expect(await isNoticeNeeded(broken)).toBe(true);
  });

  it("Start asks first unless known to be acknowledged", () => {
    expect(startStep(true)).toBe("ask");
    expect(startStep(undefined)).toBe("ask");
    expect(startStep(false)).toBe("start");
  });

  it("follows changes from the options page", () => {
    const seen: boolean[] = [];
    const off = onNoticeChanged((n) => seen.push(n));
    for (const l of [...listeners]) l({ [NOTICE_KEY]: { newValue: true } }, "local");
    for (const l of [...listeners]) l({ [NOTICE_KEY]: {} }, "local"); // removed
    for (const l of [...listeners]) l({ [NOTICE_KEY]: { newValue: true } }, "sync"); // other area
    for (const l of [...listeners]) l({ token: { newValue: "x" } }, "local"); // other key
    off();
    expect(seen).toEqual([false, true]);
    expect(listeners).toHaveLength(0);
  });

  it("uses the app's wording and README link", () => {
    expect(RECORDING_NOTICE.dontShowAgain).toBe("Don't show this again");
    expect(RECORDING_PRIVACY_URL).toMatch(/^https:\/\/github\.com\/fullo\/Sussurro#/);
    expect(DONT_SHOW_AGAIN_DEFAULT).toBe(true);
  });
});

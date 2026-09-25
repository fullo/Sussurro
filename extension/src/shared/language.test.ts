import { describe, expect, it, vi } from "vitest";

// The polyfill refuses to load outside an extension; every call here
// passes its own storage.
vi.mock("webextension-polyfill", () => ({ default: { storage: { local: {} } } }));

const {
  AUTO,
  LANGUAGE_KEY,
  chooseLanguage,
  fetchLanguages,
  isLanguageCode,
  languageLabel,
  languageOptions,
  languageSelectView,
  parseLanguages,
  platformKey,
  rememberLanguage,
  rememberedLanguage,
} = await import("./language");
type LanguageStorage = import("./language").LanguageStorage;
type AppLanguages = import("./language").AppLanguages;

function memoryStorage(initial: Record<string, unknown> = {}): LanguageStorage & { items: Record<string, unknown> } {
  const items = { ...initial };
  return {
    items,
    async get(keys) {
      return Object.fromEntries(keys.filter((k) => k in items).map((k) => [k, items[k]]));
    },
    async set(next) {
      Object.assign(items, next);
    },
  };
}

const LIST: AppLanguages = {
  engine: "whisper",
  default: "it",
  languages: [
    { code: "it", name: "Italiano" },
    { code: "en", name: "English" },
    { code: "de", name: "Deutsch" },
  ],
};

describe("the app's language list", () => {
  it("is parsed and bounded", () => {
    const parsed = parseLanguages({
      engine: "parakeet",
      default: "en",
      languages: [
        { code: "en", name: " English " },
        { code: "en", name: "dup" },
        { code: "auto", name: "Auto" },
        { code: "EN-us", name: "bad code" },
        { code: "fr" },
        { code: "x".repeat(4), name: "long" },
        null,
        { code: "de", name: "D".repeat(500) },
      ],
    });
    expect(parsed?.engine).toBe("parakeet");
    expect(parsed?.default).toBe("en");
    expect(parsed?.languages.map((l) => l.code)).toEqual(["en", "fr", "de"]);
    expect(parsed?.languages[0].name).toBe("English");
    expect(parsed?.languages[1].name).toBe("fr");
    expect(parsed?.languages[2].name.length).toBe(64);
    expect(parseLanguages({ languages: [] })?.default).toBe(AUTO);
    expect(parseLanguages({ default: "../x", languages: [] })?.default).toBe(AUTO);
    expect(parseLanguages(null)).toBeNull();
    expect(parseLanguages({ languages: "en" })).toBeNull();
  });

  it("comes from GET /app/languages with the token; an older app has none", async () => {
    const calls: [string, RequestInit | undefined][] = [];
    const ok = async (url: string, init?: RequestInit) => {
      calls.push([url, init]);
      return new Response(JSON.stringify(LIST), { status: 200 });
    };
    const p = { port: 4525, token: "a".repeat(64) };
    expect(await fetchLanguages(p, ok)).toEqual(LIST);
    expect(calls[0][0]).toBe("http://127.0.0.1:4525/app/languages");
    expect(new Headers(calls[0][1]?.headers).get("Authorization")).toBe(`Bearer ${"a".repeat(64)}`);
    expect(await fetchLanguages(p, async () => new Response("", { status: 404 }))).toBeNull();
    expect(await fetchLanguages(p, async () => new Response("not json", { status: 200 }))).toBeNull();
    expect(
      await fetchLanguages(p, async () => {
        throw new TypeError("refused");
      }),
    ).toBeNull();
  });

  it("offers Auto-detect first, then the languages by native name", () => {
    expect(languageOptions(LIST).map((l) => l.name)).toEqual(["Auto-detect", "Deutsch", "English", "Italiano"]);
    expect(languageLabel("en", LIST)).toBe("English");
    expect(languageLabel(AUTO, null)).toBe("Auto-detect");
    expect(languageLabel("ja", LIST)).toBe("ja");
  });
});

describe("the selector", () => {
  it("takes the remembered choice, else the dictation language, else Auto-detect", () => {
    expect(chooseLanguage("en", LIST)).toBe("en");
    expect(chooseLanguage(AUTO, LIST)).toBe(AUTO);
    // First time on this platform: the app's dictation language.
    expect(chooseLanguage(null, LIST)).toBe("it");
    // The engine no longer offers the remembered one (engine changed).
    expect(chooseLanguage("ja", LIST)).toBe("it");
    expect(chooseLanguage({ bad: 1 }, LIST)).toBe("it");
    expect(chooseLanguage(null, { ...LIST, default: "ja" })).toBe(AUTO);
    expect(chooseLanguage(null, { ...LIST, default: AUTO })).toBe(AUTO);
  });

  it("is shown with a list and changeable only when a Start would start", () => {
    expect(languageSelectView(LIST, true)).toEqual({ visible: true, enabled: true });
    expect(languageSelectView(LIST, false)).toEqual({ visible: true, enabled: false });
    expect(languageSelectView(null, true)).toEqual({ visible: false, enabled: false });
    expect(languageSelectView(undefined, true)).toEqual({ visible: false, enabled: false });
  });

  it("checks codes", () => {
    for (const ok of ["auto", "en", "haw"]) expect(isLanguageCode(ok)).toBe(true);
    for (const bad of ["", "EN", "en-US", "engl", 3, null, "e"]) expect(isLanguageCode(bad)).toBe(false);
  });
});

describe("the choice is remembered per platform", () => {
  it("keeps each platform's own and leaves the others alone", async () => {
    const s = memoryStorage();
    expect(await rememberedLanguage("meet", s)).toBeNull();
    await rememberLanguage("meet", "en", s);
    await rememberLanguage("teams", "it", s);
    await rememberLanguage(null, "de", s);
    expect(s.items[LANGUAGE_KEY]).toEqual({ meet: "en", teams: "it", other: "de" });
    expect(await rememberedLanguage("meet", s)).toBe("en");
    expect(await rememberedLanguage("teams", s)).toBe("it");
    expect(await rememberedLanguage("zoom", s)).toBeNull();
    expect(await rememberedLanguage(null, s)).toBe("de");
    await rememberLanguage("meet", AUTO, s);
    expect(await rememberedLanguage("meet", s)).toBe(AUTO);
    await rememberLanguage("meet", "not a code", s);
    expect(await rememberedLanguage("meet", s)).toBe(AUTO);
    expect(platformKey(undefined)).toBe("other");
  });

  it("reads garbage or a failing storage as nothing remembered", async () => {
    expect(await rememberedLanguage("meet", memoryStorage({ [LANGUAGE_KEY]: "en" }))).toBeNull();
    expect(await rememberedLanguage("meet", memoryStorage({ [LANGUAGE_KEY]: { meet: "<b>" } }))).toBeNull();
    const broken: LanguageStorage = {
      get: async () => {
        throw new Error("no storage");
      },
      set: async () => {},
    };
    expect(await rememberedLanguage("meet", broken)).toBeNull();
    // A bad stored value is dropped on the next write.
    const s = memoryStorage({ [LANGUAGE_KEY]: { meet: 42, zoom: "fr" } });
    await rememberLanguage("teams", "en", s);
    expect(s.items[LANGUAGE_KEY]).toEqual({ zoom: "fr", teams: "en" });
  });
});

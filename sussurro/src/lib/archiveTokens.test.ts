import { describe, expect, it } from "vitest";
import {
  archiveApiStatus,
  curlExample,
  MAX_NAME_CHARS,
  MAX_TOKENS,
  normalizeScopes,
  scopeSummary,
  toggleScope,
  tokenNameError,
  type ArchiveTokenInfo,
} from "./archiveTokens";

const info = (name: string, id = name): ArchiveTokenInfo => ({
  id,
  name,
  scopes: ["read"],
  created: "2026-09-25T10:00:00Z",
  last_used: null,
});

describe("tokenNameError", () => {
  it("accepts a new, trimmed, printable name", () => {
    expect(tokenNameError("  backup script ", [info("other")])).toBeNull();
    expect(tokenNameError("é".repeat(MAX_NAME_CHARS), [])).toBeNull();
  });

  it("refuses empty, long, control characters and duplicates", () => {
    expect(tokenNameError("   ", [])).toMatch(/name/);
    expect(tokenNameError("x".repeat(MAX_NAME_CHARS + 1), [])).toMatch(/At most/);
    expect(tokenNameError("tab\there", [])).toMatch(/control/);
    expect(tokenNameError("Backup", [info("backup")])).toMatch(/already exists/);
  });

  it("refuses a token over the limit", () => {
    const all = Array.from({ length: MAX_TOKENS }, (_, i) => info(`t${i}`));
    expect(tokenNameError("one more", all)).toMatch(/revoke/);
  });
});

describe("scopes", () => {
  it("keep a fixed order without duplicates", () => {
    expect(normalizeScopes(["write", "read", "write"])).toEqual(["read", "write"]);
    expect(scopeSummary(["write", "people", "read"])).toBe("read · people · write");
    expect(scopeSummary([])).toBe("");
  });

  it("ticking people ticks read too; unticking leaves the rest", () => {
    expect(toggleScope([], "people", true)).toEqual(["read", "people"]);
    expect(toggleScope(["read", "people"], "read", false)).toEqual(["people"]);
    expect(toggleScope(["read"], "write", true)).toEqual(["read", "write"]);
    expect(toggleScope(["read", "write"], "write", false)).toEqual(["read"]);
  });
});

describe("texts", () => {
  it("the curl example keeps the token out of the command line", () => {
    const c = curlExample(4525);
    expect(c).toContain("$SUSSURRO_TOKEN");
    expect(c).toContain("http://127.0.0.1:4525/archive/items");
    expect(c).not.toMatch(/sua_[0-9a-f]/);
  });

  it("the status follows the switches and the token count", () => {
    expect(archiveApiStatus(true, false, 3)).toMatch(/^off/);
    expect(archiveApiStatus(false, true, 1)).toMatch(/local API is off/);
    expect(archiveApiStatus(true, true, 0)).toMatch(/create a token/);
    expect(archiveApiStatus(true, true, 1)).toBe("on · 1 token");
    expect(archiveApiStatus(true, true, 2)).toBe("on · 2 tokens");
  });
});

import { describe, expect, it } from "vitest";
import {
  encodePairingCode,
  maskToken,
  maskedPairingCode,
  normalizeToken,
  parsePairingCode,
  parsePort,
  validatePairing,
} from "./pairingCode";

const TOKEN = "5c1e0f2a".repeat(8);

describe("pairing code", () => {
  it("round-trips port and token", () => {
    const code = encodePairingCode({ port: 4525, token: TOKEN });
    expect(code).toBe(`sussurro:4525:${TOKEN}`);
    expect(parsePairingCode(code)).toEqual({ ok: true, value: { port: 4525, token: TOKEN } });
  });

  it("tolerates whitespace, line breaks, quotes and case", () => {
    const messy = `  "SUSSURRO:4525:${TOKEN.slice(0, 30)}\n${TOKEN.slice(30).toUpperCase()}"\n`;
    expect(parsePairingCode(messy)).toEqual({ ok: true, value: { port: 4525, token: TOKEN } });
  });

  it("explains what is wrong, without echoing the token", () => {
    const err = (s: string) => {
      const r = parsePairingCode(s);
      expect(r.ok).toBe(false);
      return r.ok ? "" : r.error;
    };
    expect(err("")).toMatch(/Paste the pairing code/);
    expect(err(TOKEN)).toMatch(/not a Sussurro pairing code/);
    expect(err(`other:4525:${TOKEN}`)).toMatch(/not a Sussurro pairing code/);
    expect(err(`sussurro:4525`)).toMatch(/not a Sussurro pairing code/);
    expect(err(`sussurro:4525:${TOKEN}:x`)).toMatch(/not a Sussurro pairing code/);
    expect(err(`sussurro:0:${TOKEN}`)).toMatch(/port/);
    expect(err(`sussurro:70000:${TOKEN}`)).toMatch(/port/);
    expect(err(`sussurro:45a5:${TOKEN}`)).toMatch(/port/);
    const truncated = err(`sussurro:4525:${TOKEN.slice(0, 40)}`);
    expect(truncated).toMatch(/incomplete/);
    expect(truncated).not.toContain(TOKEN.slice(0, 40));
    expect(err(`sussurro:4525:${TOKEN.slice(0, 63)}g`)).toMatch(/incomplete/);
  });
});

describe("parts", () => {
  it("parsePort accepts 1–65535 only", () => {
    expect(parsePort(4525)).toBe(4525);
    expect(parsePort(" 80 ")).toBe(80);
    expect(parsePort("65535")).toBe(65535);
    for (const bad of ["0", "65536", "-1", "1.5", "", "abc", 1e6]) expect(parsePort(bad)).toBeNull();
  });

  it("normalizeToken lowercases and checks the length", () => {
    expect(normalizeToken(` ${TOKEN.toUpperCase()} `)).toBe(TOKEN);
    expect(normalizeToken(TOKEN + "0")).toBeNull();
    expect(normalizeToken("")).toBeNull();
  });

  it("validatePairing checks both fields", () => {
    expect(validatePairing("4525", TOKEN)).toEqual({ ok: true, value: { port: 4525, token: TOKEN } });
    expect(validatePairing("x", TOKEN).ok).toBe(false);
    expect(validatePairing(4525, "abc").ok).toBe(false);
  });

  it("masks all but the last four characters", () => {
    expect(maskToken(TOKEN)).toBe("••••••••0f2a");
    expect(maskToken(TOKEN)).not.toContain(TOKEN.slice(0, 8));
    expect(maskToken("abc")).toBe("••••••••");
    expect(maskToken("")).toBe("");
    expect(maskedPairingCode(4525)).toBe("sussurro:4525:••••••••");
  });
});

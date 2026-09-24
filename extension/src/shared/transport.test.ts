import { describe, expect, it } from "vitest";
import { decodePayload, detectMode, encodePayload, fromBase64, makeProbe, ownBuffer, toBase64 } from "./transport";

const bytes = (b: ArrayBuffer | null) => (b ? [...new Uint8Array(b)] : null);

describe("mode negotiation", () => {
  it("is binary when the probe survives as bytes (structured clone)", () => {
    expect(detectMode(structuredClone(makeProbe()))).toBe("binary");
  });

  it("falls back to base64 when the port serialised it as JSON", () => {
    expect(detectMode(JSON.parse(JSON.stringify({ p: makeProbe() })).p)).toBe("base64");
    expect(detectMode(undefined)).toBe("base64");
    expect(detectMode(Uint8Array.of(9, 9, 9).buffer)).toBe("base64");
  });
});

describe("payloads", () => {
  const pcm = Uint8Array.of(0, 1, 2, 250, 255).buffer;

  it("round-trip in both modes", () => {
    expect(bytes(decodePayload(encodePayload("binary", pcm)))).toEqual([0, 1, 2, 250, 255]);
    const b64 = encodePayload("base64", pcm);
    expect(typeof b64).toBe("string");
    expect(bytes(decodePayload(JSON.parse(JSON.stringify(b64))))).toEqual([0, 1, 2, 250, 255]);
  });

  it("accepts a typed-array view and rejects garbage", () => {
    const big = Uint8Array.of(9, 1, 2, 9);
    expect(bytes(decodePayload(big.subarray(1, 3)))).toEqual([1, 2]);
    expect(decodePayload({})).toBeNull();
    expect(decodePayload(42)).toBeNull();
    expect(decodePayload("%%%")).toBeNull();
  });

  it("base64 handles buffers larger than one apply() chunk", () => {
    const u = new Uint8Array(100_000).map((_, i) => i * 7);
    expect(fromBase64(toBase64(u))).toEqual(u);
  });
});

describe("ownBuffer (Firefox Xray wrappers)", () => {
  it("keeps a usable buffer as is", () => {
    const b = new ArrayBuffer(4);
    expect(ownBuffer(b)).toBe(b);
  });

  it("copies with structuredClone when the buffer can't be viewed", () => {
    // Stand-in for an Xray-wrapped page buffer: looks like an ArrayBuffer,
    // but `new Uint8Array(it)` throws.
    const wrapped = new Proxy(new ArrayBuffer(2), {
      get() {
        throw new Error("Permission denied to access property constructor");
      },
    });
    const copy = new ArrayBuffer(2);
    expect(ownBuffer(wrapped, () => copy)).toBe(copy);
    expect(ownBuffer(wrapped, () => ({}))).toBeNull();
    expect(
      ownBuffer(wrapped, () => {
        throw new Error("nope");
      }),
    ).toBeNull();
  });
});

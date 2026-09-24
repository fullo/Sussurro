import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { CHANNEL, FRAME_HEADER, SeqCounter, decodeFrame, encodeFrame, floatToPcm16, rms16, toPcm16 } from "./frame";

const protocolRs = readFileSync(fileURLToPath(new URL("../../../sussurro/src-tauri/src/api/protocol.rs", import.meta.url)), "utf8");

describe("the app's /live protocol (sussurro/src-tauri/src/api/protocol.rs)", () => {
  it("maps channel bytes the same way: 0 = mic, 1 = remote", () => {
    expect(CHANNEL).toEqual({ mic: 0, remote: 1 });
    expect(protocolRs).toMatch(/0 => Some\(Channel::Mic\)/);
    expect(protocolRs).toMatch(/1 => Some\(Channel::Remote\)/);
  });

  it("uses the same header size", () => {
    expect(protocolRs).toContain(`pub const FRAME_HEADER: usize = ${FRAME_HEADER};`);
  });
});

describe("encodeFrame / decodeFrame", () => {
  it("writes [u8 channel][u32 seq LE][i16 LE…] (the app's own test vector)", () => {
    const buf = encodeFrame(1, 0x0102_0304, Int16Array.of(0, 16_384, -32_768, 32_767));
    expect([...new Uint8Array(buf, 0, 5)]).toEqual([1, 4, 3, 2, 1]);
    expect([...new Uint8Array(buf, 5)]).toEqual([0, 0, 0, 0x40, 0, 0x80, 0xff, 0x7f]);
    const f = decodeFrame(buf);
    expect(f.channel).toBe(1);
    expect(f.seq).toBe(0x0102_0304);
    expect([...f.pcm]).toEqual([0, 16_384, -32_768, 32_767]);
  });

  it("accepts the PCM as raw bytes and an empty frame", () => {
    const pcm = Int16Array.of(-2, 3);
    expect([...decodeFrame(encodeFrame(0, 7, pcm.buffer)).pcm]).toEqual([-2, 3]);
    const empty = decodeFrame(encodeFrame(0, 0, new Int16Array(0)));
    expect(empty.pcm.length).toBe(0);
  });

  it("refuses what the app would refuse", () => {
    expect(() => encodeFrame(0, -1, new Int16Array(1))).toThrow(/u32/);
    expect(() => encodeFrame(0, 2 ** 32, new Int16Array(1))).toThrow(/u32/);
    expect(() => encodeFrame(0, 0, new Int16Array(300_000))).toThrow(/limit/);
    expect(() => decodeFrame(new Uint8Array(4))).toThrow();
    expect(() => decodeFrame(new Uint8Array(6))).toThrow(/odd/);
  });
});

describe("PCM conversion", () => {
  it("clamps, rounds and maps silence to 0", () => {
    expect([0, 1, -1, 2, -2, 0.5, -0.5, NaN].map(floatToPcm16)).toEqual([0, 32767, -32768, 32767, -32768, 16384, -16384, 0]);
    expect([...toPcm16(Float32Array.of(0.25, -0.25))]).toEqual([8192, -8192]);
  });

  it("measures levels", () => {
    expect(rms16(new Int16Array(0))).toBe(0);
    expect(rms16(new Int16Array(10))).toBe(0);
    expect(rms16(Int16Array.of(16384, -16384))).toBeCloseTo(0.5);
  });
});

describe("SeqCounter", () => {
  it("numbers each channel from 0, whatever the producer's own counter", () => {
    const c = new SeqCounter();
    expect([c.take(0, "page", 40), c.take(1, "page", 40), c.take(0, "page", 41), c.take(1, "page", 41)]).toEqual([0, 0, 1, 1]);
  });

  it("turns a producer's skipped frames and dropped frames into gaps", () => {
    const c = new SeqCounter();
    c.take(0, "page", 5);
    expect(c.take(0, "page", 8)).toBe(3); // 6 and 7 never arrived
    c.skip(0, 2);
    expect(c.take(0, "page", 9)).toBe(6);
  });

  it("keeps counting when the remote channel moves to another producer", () => {
    const c = new SeqCounter();
    c.take(1, "page", 100);
    c.take(1, "page", 101);
    // Tab capture takes over and starts its own counter at 0: no reset, no gap.
    expect(c.take(1, "tab", 0)).toBe(2);
    expect(c.take(1, "tab", 1)).toBe(3);
  });
});

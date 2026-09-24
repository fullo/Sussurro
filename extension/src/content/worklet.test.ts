import { describe, expect, it } from "vitest";
import { PROCESSOR_NAME, WORKLET_SOURCE } from "./worklet";

interface Processor {
  process(inputs: Float32Array[][], outputs: Float32Array[][]): boolean;
  port: { posted: { mic: ArrayBuffer; remote: ArrayBuffer }[]; onmessage: ((e: { data: unknown }) => void) | null };
}

/** Evaluate the worklet source with a fake AudioWorkletGlobalScope. */
function load(block: number): Processor {
  let Ctor: (new (o: unknown) => Processor) | null = null;
  class AudioWorkletProcessor {
    port = {
      posted: [] as unknown[],
      onmessage: null,
      postMessage(m: unknown) {
        this.posted.push(m);
      },
    };
  }
  const registerProcessor = (name: string, c: new (o: unknown) => Processor) => {
    expect(name).toBe(PROCESSOR_NAME);
    Ctor = c;
  };
  new Function("AudioWorkletProcessor", "registerProcessor", WORKLET_SOURCE)(AudioWorkletProcessor, registerProcessor);
  return new Ctor!({ processorOptions: { block } });
}

const quantum = (v: number) => new Float32Array(128).fill(v);

describe("capture worklet", () => {
  it("posts one i16 block per channel every `block` samples", () => {
    const p = load(256);
    const out = [[new Float32Array(128)]];
    expect(p.process([[quantum(0.5), quantum(-0.25)]], out)).toBe(true);
    expect(p.port.posted).toHaveLength(0);
    p.process([[quantum(0.5), quantum(-0.25)]], out);
    expect(p.port.posted).toHaveLength(1);
    const { mic, remote } = p.port.posted[0];
    expect(new Int16Array(mic)).toEqual(new Int16Array(256).fill(16384));
    expect(new Int16Array(remote)).toEqual(new Int16Array(256).fill(-8192));
  });

  it("keeps both channels advancing with silence when an input is missing", () => {
    const p = load(128);
    const out = [[new Float32Array(128)]];
    p.process([[quantum(1)]], out); // remote bus inactive
    p.process([[]], out); // no input at all: length from the output
    p.process([], out);
    expect(p.port.posted).toHaveLength(3);
    expect(new Int16Array(p.port.posted[0].mic)[0]).toBe(32767);
    expect(new Int16Array(p.port.posted[0].remote)).toEqual(new Int16Array(128));
    expect(new Int16Array(p.port.posted[2].mic)).toEqual(new Int16Array(128));
  });

  it("stops on request", () => {
    const p = load(128);
    p.port.onmessage!({ data: "stop" });
    expect(p.process([[quantum(1), quantum(1)]], [[new Float32Array(128)]])).toBe(false);
    expect(p.port.posted).toHaveLength(0);
  });
});

/* Audio frames for the app's `/live` WebSocket (#126, protocol v1):
 *
 *   [u8 channel][u32 seq LE][i16 pcm LE…]
 *
 * `channel` is 0 = `mic` (the page's own microphone) or 1 = `remote`
 * (everyone else), matching `channel_from_byte` in
 * sussurro/src-tauri/src/api/protocol.rs. The PCM is mono at the rate sent
 * in `start`. `seq` counts one channel's frames on one connection from 0; a
 * gap is filled with silence by the app, an older or repeated `seq` is
 * dropped. Pure — unit tested. */

/** Logical channels, by wire value. */
export const CHANNEL = { mic: 0, remote: 1 } as const;
export type ChannelName = keyof typeof CHANNEL;
export type ChannelByte = (typeof CHANNEL)[ChannelName];
export const CHANNEL_NAMES: readonly ChannelName[] = ["mic", "remote"];

/** Bytes before the PCM. */
export const FRAME_HEADER = 5;
/** Samples per frame at the page's rate (≈ 43 ms at 48 kHz). */
export const FRAME_SAMPLES = 2048;
/** The app refuses messages above 512 KiB (`MAX_MESSAGE_BYTES`). */
export const MAX_FRAME_BYTES = 512 * 1024;

/** Float sample in [-1, 1] → i16, clamped and rounded. Also inlined into the
 *  AudioWorklet (its `toString()` is), so keep it self-contained. */
export function floatToPcm16(x: number): number {
  const v = x !== x ? 0 : x < -1 ? -1 : x > 1 ? 1 : x;
  return Math.round(v < 0 ? v * 32768 : v * 32767);
}

/** Float32 samples → i16 PCM. */
export function toPcm16(samples: ArrayLike<number>): Int16Array {
  const out = new Int16Array(samples.length);
  for (let i = 0; i < samples.length; i++) out[i] = floatToPcm16(samples[i]);
  return out;
}

/** Build one wire frame. `pcm` is little-endian i16 (an `Int16Array` or its
 *  bytes); the output is always little endian, whatever the host. */
export function encodeFrame(channel: ChannelByte, seq: number, pcm: Int16Array | ArrayBuffer): ArrayBuffer {
  const samples = pcm instanceof Int16Array ? pcm : new Int16Array(pcm);
  if (!Number.isInteger(seq) || seq < 0 || seq > 0xffffffff) throw new RangeError(`seq ${seq} is not a u32`);
  const buf = new ArrayBuffer(FRAME_HEADER + samples.length * 2);
  if (buf.byteLength > MAX_FRAME_BYTES) throw new RangeError(`frame of ${buf.byteLength} bytes exceeds the app's limit`);
  const dv = new DataView(buf);
  dv.setUint8(0, channel);
  dv.setUint32(1, seq, true);
  for (let i = 0; i < samples.length; i++) dv.setInt16(FRAME_HEADER + 2 * i, samples[i], true);
  return buf;
}

export interface DecodedFrame {
  channel: number;
  seq: number;
  pcm: Int16Array;
}

/** Parse a wire frame (tests and the local harness). */
export function decodeFrame(buf: ArrayBuffer | Uint8Array): DecodedFrame {
  const u8 = buf instanceof Uint8Array ? buf : new Uint8Array(buf);
  if (u8.length < FRAME_HEADER) throw new RangeError("frame shorter than its header");
  if ((u8.length - FRAME_HEADER) % 2) throw new RangeError("odd number of PCM bytes");
  const dv = new DataView(u8.buffer, u8.byteOffset, u8.byteLength);
  const n = (u8.length - FRAME_HEADER) / 2;
  const pcm = new Int16Array(n);
  for (let i = 0; i < n; i++) pcm[i] = dv.getInt16(FRAME_HEADER + 2 * i, true);
  return { channel: dv.getUint8(0), seq: dv.getUint32(1, true), pcm };
}

/** Root mean square of i16 PCM, in [0, 1] (a level meter). */
export function rms16(pcm: Int16Array): number {
  if (!pcm.length) return 0;
  let ss = 0;
  for (let i = 0; i < pcm.length; i++) ss += pcm[i] * pcm[i];
  return Math.sqrt(ss / pcm.length) / 32768;
}

/**
 * Per-connection `seq` numbering in the background. Frames reach it from
 * different producers over a meeting (the page's worklet, the Chrome
 * tab-capture document) whose own counters restart independently, and a
 * reconnect starts a new meeting on the app, where `seq` must start again.
 * So the background numbers the frames itself: +1 per frame, plus the
 * frames a producer reports as skipped (a jump in its own counter) or that
 * the background dropped, so the app fills those with silence.
 */
export class SeqCounter {
  private next: number[] = [0, 0];
  private last = new Map<string, number>();

  /** The wire `seq` for a frame of `channel` that `producer` numbered
   *  `producerSeq`. */
  take(channel: ChannelByte, producer: string, producerSeq: number): number {
    const key = `${channel}:${producer}`;
    const prev = this.last.get(key);
    this.last.set(key, producerSeq);
    // A forward jump in the producer's own counter = frames it never sent.
    if (prev !== undefined && producerSeq > prev + 1) this.skip(channel, producerSeq - prev - 1);
    const seq = this.next[channel];
    this.next[channel] = (seq + 1) >>> 0;
    return seq;
  }

  /** `n` frames of `channel` were dropped on purpose (a full queue). */
  skip(channel: ChannelByte, n: number): void {
    this.next[channel] = (this.next[channel] + n) >>> 0;
  }
}

/* Binary audio between the content script (or Chrome's offscreen document)
 * and the background, over a `runtime.connect` port. Pure — unit tested.
 *
 * Firefox serialises extension messages with the structured clone
 * algorithm, so an `ArrayBuffer` arrives as an `ArrayBuffer`. Chrome used
 * JSON (an ArrayBuffer turns into `{}`) until Chrome 148 added the
 * `"message_serialization": "structured_clone"` manifest key, which our
 * Chrome manifest sets. The two ends negotiate instead of trusting the
 * browser version: the background's `hello` carries a 3-byte probe buffer;
 * if it arrives as bytes the port is binary-safe, otherwise (older Chrome,
 * Edge/Brave on an older engine) the audio is sent as base64 (+33 %). */

export type TransportMode = "binary" | "base64";

/** What the background puts in its `hello`. */
export function makeProbe(): ArrayBuffer {
  return Uint8Array.of(1, 2, 3).buffer;
}

function isArrayBuffer(x: unknown): x is ArrayBuffer {
  // Not `instanceof`: the value may come from another realm (Firefox's
  // content-script sandbox, a MessagePort from the page).
  return Object.prototype.toString.call(x) === "[object ArrayBuffer]";
}

/** Did the probe survive the port as bytes? */
export function detectMode(probe: unknown): TransportMode {
  if (isArrayBuffer(probe) && probe.byteLength === 3) {
    const u = new Uint8Array(probe);
    if (u[0] === 1 && u[1] === 2 && u[2] === 3) return "binary";
  }
  return "base64";
}

/** Payload for the port. */
export function encodePayload(mode: TransportMode, buf: ArrayBuffer): ArrayBuffer | string {
  return mode === "binary" ? buf : toBase64(new Uint8Array(buf));
}

/** A payload off the port → bytes, whichever way it was sent (null if it
 *  is neither bytes nor base64). */
export function decodePayload(x: unknown): ArrayBuffer | null {
  if (isArrayBuffer(x)) return x;
  if (ArrayBuffer.isView(x)) {
    const v = x as ArrayBufferView;
    return new Uint8Array(v.buffer, v.byteOffset, v.byteLength).slice().buffer;
  }
  if (typeof x === "string") {
    try {
      return fromBase64(x).buffer as ArrayBuffer;
    } catch {
      return null;
    }
  }
  return null;
}

export function toBase64(u8: Uint8Array): string {
  let s = "";
  for (let i = 0; i < u8.length; i += 0x8000) s += String.fromCharCode(...u8.subarray(i, i + 0x8000));
  return btoa(s);
}

export function fromBase64(b64: string): Uint8Array {
  const s = atob(b64);
  const u = new Uint8Array(s.length);
  for (let i = 0; i < s.length; i++) u[i] = s.charCodeAt(i);
  return u;
}

/**
 * A buffer received from the page (through the handshake's MessagePort)
 * as one this realm owns. In Firefox the content script sees the page's
 * objects through Xray wrappers: `new Uint8Array(pageBuffer)` throws
 * "Permission denied to access property constructor", `structuredClone`
 * works (spike #104). Chrome needs no copy.
 */
export function ownBuffer(x: unknown, clone: (v: unknown) => unknown = structuredClone): ArrayBuffer | null {
  try {
    if (isArrayBuffer(x)) {
      new Uint8Array(x); // throws on a Firefox Xray wrapper
      return x;
    }
  } catch {
    /* fall through to the copy */
  }
  try {
    const c = clone(x);
    return isArrayBuffer(c) ? c : null;
  } catch {
    return null;
  }
}

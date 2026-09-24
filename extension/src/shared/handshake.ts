/* MAIN ↔ ISOLATED handshake: a private MessageChannel instead of
 * `window.postMessage` traffic. Pure (the window is injected) — unit tested.
 *
 * Why: `window.postMessage` is seen by every script in the page, and the
 * two content scripts are injected in no guaranteed order — Firefox may run
 * the ISOLATED script after the MAIN one, so the MAIN script's first message
 * was lost in spike #104. So:
 *
 * - MAIN, on install, listens for a port offer and posts `main-ready`.
 * - ISOLATED posts an offer (a fresh MessageChannel's port2) at start and
 *   again on every `main-ready` it sees. Whichever order the scripts ran in,
 *   one offer reaches a listening MAIN.
 * - MAIN takes the first offer only, answers `ack` on that port and hides
 *   the offer from the page's later listeners. ISOLATED keeps the port that
 *   was acknowledged and closes the others.
 *
 * Everything after that (audio, state, commands) goes over the port. The
 * page shares the MAIN world, so this is hygiene rather than a security
 * boundary: it keeps audio off the window's message bus and away from other
 * frames, and never lets a late offer replace the live port. */

export const HANDSHAKE_TAG = "__sussurro_capture_v1";

export type HandshakeMessage = { [HANDSHAKE_TAG]: "main-ready" | "offer" };

/** The bits of `window` the handshake uses (a fake in the tests). */
export interface WindowLike {
  postMessage(message: unknown, targetOrigin: string, transfer?: Transferable[]): void;
  addEventListener(type: "message", listener: (e: MessageEvent) => void, capture?: boolean): void;
  removeEventListener(type: "message", listener: (e: MessageEvent) => void, capture?: boolean): void;
}

function kind(data: unknown): string | null {
  if (!data || typeof data !== "object") return null;
  try {
    const v = (data as Record<string, unknown>)[HANDSHAKE_TAG];
    return typeof v === "string" ? v : null;
  } catch {
    return null;
  }
}

/** The acknowledgement MAIN sends as the port's first message. */
export const ACK = { t: "ack" } as const;

export function isAck(data: unknown): boolean {
  try {
    return !!data && typeof data === "object" && (data as { t?: unknown }).t === "ack";
  } catch {
    return false;
  }
}

/**
 * MAIN world: wait for ISOLATED's port. Calls `onPort` once, with the first
 * offer's port (already acknowledged). Returns a function that stops
 * listening.
 */
export function acceptPort(win: WindowLike, targetOrigin: string, onPort: (port: MessagePort) => void): () => void {
  let done = false;
  const listener = (e: MessageEvent) => {
    if (e.source !== (win as unknown) || kind(e.data) !== "offer") return;
    // Ours: the page's own listeners don't need to see it.
    e.stopImmediatePropagation();
    const port = e.ports && e.ports[0];
    if (done || !port) return;
    done = true;
    win.removeEventListener("message", listener, true);
    port.postMessage(ACK);
    onPort(port);
  };
  win.addEventListener("message", listener, true);
  win.postMessage({ [HANDSHAKE_TAG]: "main-ready" }, targetOrigin);
  return () => {
    done = true;
    win.removeEventListener("message", listener, true);
  };
}

/**
 * ISOLATED world: offer ports until MAIN acknowledges one. Resolves with
 * that port; its `onmessage` is free for the caller to set (MAIN only
 * speaks after the caller's first message, apart from repeated state).
 */
export function offerPort(
  win: WindowLike,
  targetOrigin: string,
  makeChannel: () => MessageChannel = () => new MessageChannel(),
): Promise<MessagePort> {
  return new Promise((resolve) => {
    const pending: MessagePort[] = [];
    let settled = false;
    const offer = () => {
      if (settled) return;
      const ch = makeChannel();
      pending.push(ch.port1);
      ch.port1.onmessage = (e: MessageEvent) => {
        if (settled || !isAck(e.data)) return;
        settled = true;
        win.removeEventListener("message", onReady, true);
        for (const p of pending) if (p !== ch.port1) p.close();
        ch.port1.onmessage = null;
        resolve(ch.port1);
      };
      win.postMessage({ [HANDSHAKE_TAG]: "offer" }, targetOrigin, [ch.port2]);
    };
    const onReady = (e: MessageEvent) => {
      if (e.source !== (win as unknown) || kind(e.data) !== "main-ready") return;
      e.stopImmediatePropagation();
      offer();
    };
    win.addEventListener("message", onReady, true);
    offer();
  });
}

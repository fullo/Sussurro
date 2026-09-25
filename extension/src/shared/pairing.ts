/* Pairing with the Sussurro app (#127, E6): where the extension keeps the
   local API port and the extension token, and how it reaches the app.

   Every entry point reads the pairing through this module — the options page
   writes it, the background worker (#128) and the side panel read it — so the
   keys are defined only here.

   Where (#217): in the extension origin's IndexedDB (secureStore.ts), which
   content scripts in meeting pages can't open — not in `storage.local`,
   which they can. A pairing an older version left in `storage.local` is
   moved over on the next read and removed from there.

   The token is a secret: never log it, never put it in an error message.
   It travels in the `Authorization` header (HTTP) and in the first message
   on `/live` (`auth`, #217); only with an app too old for that, in the
   WebSocket URL (see `liveUrl`), which must not be logged either. */
import browser from "webextension-polyfill";
import { normalizeToken, parsePort, type Pairing } from "@sussurro/pairing";
import { announcePairingChange, idbStorage, onPairingAnnounced } from "./secureStore";

export {
  encodePairingCode,
  maskToken,
  parsePairingCode,
  validatePairing,
} from "@sussurro/pairing";
export type { Pairing, Parsed } from "@sussurro/pairing";

/** Keys of the pairing (in the secure store; in `storage.local` before #217). */
export const PAIRING_KEYS = { port: "port", token: "token" } as const;

/** The app's default local API port (`Settings::api_port`). */
export const DEFAULT_PORT = 4525;

/** Protocol spoken with the app (`api::protocol::PROTOCOL_VERSION`). The app
 *  reports its own protocol and the oldest it still accepts
 *  (`GET /app/version` → `protocol`, `protocol_min`); the extension runs
 *  when its version is in that range (`protocolCompatible`). 2 (#131):
 *  speaker ids, `speaker_idle`, `speaker_name`, `observer_health`. */
export const PROTOCOL_VERSION = 2;

/** Whether this extension can talk to an app speaking `protocol`, which
 *  accepts clients from `protocolMin` (absent: only its own). Pure. */
export function protocolCompatible(protocol: number, protocolMin?: number): boolean {
  const min = typeof protocolMin === "number" && protocolMin <= protocol ? protocolMin : protocol;
  return min <= PROTOCOL_VERSION && PROTOCOL_VERSION <= protocol;
}

/** A key/value area with the `storage.local` shape (injectable for tests). */
export interface PairingStorage {
  get(keys: string[]): Promise<Record<string, unknown>>;
  set(items: Record<string, unknown>): Promise<void>;
  remove(keys: string[]): Promise<void>;
}

/** Where the pairing is kept. */
export interface PairingStores {
  /** Readable by the extension's own pages and background only. */
  secure: PairingStorage;
  /** `storage.local`, where versions before #217 kept it. */
  legacy: PairingStorage;
}

let secure: PairingStorage | null = null;
const stores = (): PairingStores => ({ secure: (secure ??= idbStorage()), legacy: browser.storage.local });
const KEYS = [PAIRING_KEYS.port, PAIRING_KEYS.token];

/** The pairing in stored items, or null when missing or invalid. Pure. */
export function pairingFromItems(items: Record<string, unknown>): Pairing | null {
  const rawPort = items[PAIRING_KEYS.port];
  const rawToken = items[PAIRING_KEYS.token];
  const port = typeof rawPort === "number" || typeof rawPort === "string" ? parsePort(rawPort) : null;
  const token = typeof rawToken === "string" ? normalizeToken(rawToken) : null;
  return port !== null && token !== null ? { port, token } : null;
}

/** The stored pairing. One left in `storage.local` (an older version, or a
 *  newer write there) is moved to the secure store first; if that store
 *  can't be written, it stays where it is and is still used. */
export async function getPairing(s: PairingStores = stores()): Promise<Pairing | null> {
  const old = await s.legacy.get(KEYS);
  if (Object.keys(old).length) {
    const moved = pairingFromItems(old);
    if (moved) {
      try {
        await s.secure.set({ [PAIRING_KEYS.port]: moved.port, [PAIRING_KEYS.token]: moved.token });
      } catch {
        return moved;
      }
    }
    await s.legacy.remove(KEYS);
    if (moved) {
      announcePairingChange();
      return moved;
    }
  }
  return pairingFromItems(await s.secure.get(KEYS));
}

/** Save a pairing (validated again: a bad one is refused, never stored). */
export async function setPairing(p: Pairing, s: PairingStores = stores()): Promise<Pairing> {
  const pairing = pairingFromItems({ [PAIRING_KEYS.port]: p.port, [PAIRING_KEYS.token]: p.token });
  if (!pairing) throw new Error("Invalid pairing: check the port and the token.");
  await s.secure.set({ [PAIRING_KEYS.port]: pairing.port, [PAIRING_KEYS.token]: pairing.token });
  await s.legacy.remove(KEYS);
  announcePairingChange();
  return pairing;
}

export async function clearPairing(s: PairingStores = stores()): Promise<void> {
  await s.secure.remove(KEYS);
  await s.legacy.remove(KEYS);
  announcePairingChange();
}

/** Call `cb` with the new pairing (or null) whenever it changes: announced
 *  by another extension page, or written to `storage.local` the old way
 *  (then moved). Returns the unsubscribe function. */
export function onPairingChanged(cb: (p: Pairing | null) => void, s?: PairingStores): () => void {
  const reread = () => void getPairing(s).then(cb, () => cb(null));
  const listener = (changes: Record<string, unknown>, area: string) => {
    if (area !== "local") return;
    if (!(PAIRING_KEYS.port in changes) && !(PAIRING_KEYS.token in changes)) return;
    reread();
  };
  browser.storage.onChanged.addListener(listener);
  const off = onPairingAnnounced(reread);
  return () => {
    browser.storage.onChanged.removeListener(listener);
    off();
  };
}

/** `http://127.0.0.1:<port><path>` — loopback only, like the app's bind. */
export function appUrl(port: number, path: string): string {
  return `http://127.0.0.1:${port}${path}`;
}

/** The `/live` WebSocket URL (#128). With `firstMessage` (an app that
 *  reports `live_auth: "message"`, #217) it has no token: `liveAuth` goes
 *  first on the socket. Otherwise it carries the token: never log it. */
export function liveUrl(p: Pairing, firstMessage = false): string {
  const base = `ws://127.0.0.1:${p.port}/live`;
  return firstMessage ? base : `${base}?token=${encodeURIComponent(p.token)}`;
}

/** The first message on a `/live` opened without a token (#217). */
export function liveAuth(p: Pairing): string {
  return JSON.stringify({ type: "auth", token: p.token });
}

/** `Authorization` header of the token routes. */
export function authHeaders(p: Pairing): Record<string, string> {
  return { Authorization: `Bearer ${p.token}` };
}

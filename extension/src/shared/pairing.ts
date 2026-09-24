/* Pairing with the Sussurro app (#127, E6): where the extension keeps the
   local API port and the extension token, and how it reaches the app.

   Every entry point reads the pairing through this module — the options page
   writes it, the background worker (#128) and the side panel read it — so the
   `storage.local` keys are defined only here.

   The token is a secret: never log it, never put it in an error message.
   It travels only in the `Authorization` header (HTTP) and in the `/live`
   WebSocket URL (see `liveUrl`), which must not be logged either. */
import browser from "webextension-polyfill";
import { normalizeToken, parsePort, type Pairing } from "@sussurro/pairing";

export {
  encodePairingCode,
  maskToken,
  parsePairingCode,
  validatePairing,
} from "@sussurro/pairing";
export type { Pairing, Parsed } from "@sussurro/pairing";

/** `storage.local` keys of the pairing. */
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

/** The subset of `storage.local` used here (injectable for tests). */
export interface PairingStorage {
  get(keys: string[]): Promise<Record<string, unknown>>;
  set(items: Record<string, unknown>): Promise<void>;
  remove(keys: string[]): Promise<void>;
}

const local = (): PairingStorage => browser.storage.local;

/** The pairing in stored items, or null when missing or invalid. Pure. */
export function pairingFromItems(items: Record<string, unknown>): Pairing | null {
  const rawPort = items[PAIRING_KEYS.port];
  const rawToken = items[PAIRING_KEYS.token];
  const port = typeof rawPort === "number" || typeof rawPort === "string" ? parsePort(rawPort) : null;
  const token = typeof rawToken === "string" ? normalizeToken(rawToken) : null;
  return port !== null && token !== null ? { port, token } : null;
}

export async function getPairing(storage: PairingStorage = local()): Promise<Pairing | null> {
  return pairingFromItems(await storage.get([PAIRING_KEYS.port, PAIRING_KEYS.token]));
}

/** Save a pairing (validated again: a bad one is refused, never stored). */
export async function setPairing(p: Pairing, storage: PairingStorage = local()): Promise<Pairing> {
  const pairing = pairingFromItems({ [PAIRING_KEYS.port]: p.port, [PAIRING_KEYS.token]: p.token });
  if (!pairing) throw new Error("Invalid pairing: check the port and the token.");
  await storage.set({ [PAIRING_KEYS.port]: pairing.port, [PAIRING_KEYS.token]: pairing.token });
  return pairing;
}

export async function clearPairing(storage: PairingStorage = local()): Promise<void> {
  await storage.remove([PAIRING_KEYS.port, PAIRING_KEYS.token]);
}

/** Call `cb` with the new pairing (or null) whenever it changes in
 *  `storage.local`. Returns the unsubscribe function. */
export function onPairingChanged(cb: (p: Pairing | null) => void): () => void {
  const listener = (changes: Record<string, unknown>, area: string) => {
    if (area !== "local") return;
    if (!(PAIRING_KEYS.port in changes) && !(PAIRING_KEYS.token in changes)) return;
    getPairing().then(cb, () => cb(null));
  };
  browser.storage.onChanged.addListener(listener);
  return () => browser.storage.onChanged.removeListener(listener);
}

/** `http://127.0.0.1:<port><path>` — loopback only, like the app's bind. */
export function appUrl(port: number, path: string): string {
  return `http://127.0.0.1:${port}${path}`;
}

/** The `/live` WebSocket URL (#128). Carries the token: never log it. */
export function liveUrl(p: Pairing): string {
  return `ws://127.0.0.1:${p.port}/live?token=${encodeURIComponent(p.token)}`;
}

/** `Authorization` header of the token routes. */
export function authHeaders(p: Pairing): Record<string, string> {
  return { Authorization: `Bearer ${p.token}` };
}

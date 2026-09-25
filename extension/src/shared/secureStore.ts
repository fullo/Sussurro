/* Storage content scripts can't reach (#217), for the pairing's token.
 *
 * `storage.local` is shared with the content scripts that run in meeting
 * pages: Firefox always exposes it to them (no `setAccessLevel` there,
 * bugzil.la/1724754), Chrome unless it is restricted with
 * `storage.local.setAccessLevel` (Chrome ≥ 140 only, and only once the
 * background has run after a browser start). A renderer compromised by the
 * meeting page could then ask for the token. IndexedDB belongs to the
 * extension's own origin instead: the background, the options page and
 * the side panel share it, while a content script's `indexedDB` is the web
 * page's. So the pairing lives here, and `storage.local` keeps only the
 * non-secret recording-notice flag (restricted too where Chrome allows).
 *
 * Changes are announced on a BroadcastChannel, which is per origin as
 * well: content scripts don't hear them. */
import type { PairingStorage } from "./pairing";

const DB_NAME = "sussurro";
const STORE = "pairing";

function request<T>(r: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    r.onsuccess = () => resolve(r.result);
    r.onerror = () => reject(r.error ?? new Error("IndexedDB request failed"));
  });
}

function done(tx: IDBTransaction): Promise<void> {
  return new Promise((resolve, reject) => {
    tx.oncomplete = () => resolve();
    tx.onabort = tx.onerror = () => reject(tx.error ?? new Error("IndexedDB transaction failed"));
  });
}

/** A key/value area over one IndexedDB object store of the extension's
 *  origin, with the `storage.local` shape the pairing code uses. */
export function idbStorage(factory: IDBFactory = indexedDB, name = DB_NAME): PairingStorage {
  let db: Promise<IDBDatabase> | null = null;
  const open = (): Promise<IDBDatabase> => {
    if (!db) {
      const r = factory.open(name, 1);
      r.onupgradeneeded = () => {
        if (!r.result.objectStoreNames.contains(STORE)) r.result.createObjectStore(STORE);
      };
      db = request(r).catch((e: unknown) => {
        db = null;
        throw e;
      });
    }
    return db;
  };
  return {
    async get(keys) {
      const tx = (await open()).transaction(STORE, "readonly");
      const store = tx.objectStore(STORE);
      const values = await Promise.all(keys.map((k) => request(store.get(k))));
      const out: Record<string, unknown> = {};
      keys.forEach((k, i) => {
        if (values[i] !== undefined) out[k] = values[i];
      });
      return out;
    },
    async set(items) {
      const tx = (await open()).transaction(STORE, "readwrite");
      const store = tx.objectStore(STORE);
      for (const [k, v] of Object.entries(items)) store.put(v, k);
      await done(tx);
    },
    async remove(keys) {
      const tx = (await open()).transaction(STORE, "readwrite");
      const store = tx.objectStore(STORE);
      for (const k of keys) store.delete(k);
      await done(tx);
    },
  };
}

/** The BroadcastChannel that announces pairing changes. */
export const PAIRING_CHANNEL = "sussurro-pairing";

/** Tell the extension's other pages (and the background) the pairing
 *  changed. */
export function announcePairingChange(): void {
  if (typeof BroadcastChannel === "undefined") return;
  const ch = new BroadcastChannel(PAIRING_CHANNEL);
  ch.postMessage("changed");
  ch.close();
}

/** Call `cb` on every announced change; returns the unsubscribe. */
export function onPairingAnnounced(cb: () => void): () => void {
  if (typeof BroadcastChannel === "undefined") return () => {};
  const ch = new BroadcastChannel(PAIRING_CHANNEL);
  ch.onmessage = () => cb();
  return () => ch.close();
}

/** The subset of Chrome's `StorageArea` used to restrict it. */
export interface RestrictableArea {
  setAccessLevel?(access: { accessLevel: "TRUSTED_CONTEXTS" | "TRUSTED_AND_UNTRUSTED_CONTEXTS" }): Promise<void> | void;
}

/** Keep `area` (Chrome's `storage.local`) away from content scripts where
 *  the browser supports it (Chrome ≥ 140). The setting doesn't outlive the
 *  browser session: call it every time the background starts. `true` when
 *  applied. */
export async function restrictToTrustedContexts(area: RestrictableArea | undefined): Promise<boolean> {
  if (!area || typeof area.setAccessLevel !== "function") return false;
  try {
    await area.setAccessLevel({ accessLevel: "TRUSTED_CONTEXTS" });
    return true;
  } catch {
    // Chrome 116–139: only `storage.session` accepts an access level.
    return false;
  }
}

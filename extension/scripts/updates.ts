/* Pure helpers for Firefox's self-hosted update manifest (#228): the JSON
   at the manifest's `browser_specific_settings.gecko.update_url`
   (docs/extension/updates.json, served by GitHub Pages), which tells
   installed copies of the signed .xpi where each newer version lives.
   Format: https://extensionworkshop.com/documentation/manage/updating-your-extension/
   No I/O here; scripts/update-manifest.ts does it. Unit-tested in
   updates.test.ts. */
import { createHash } from "node:crypto";
import { unzipSync, strFromU8 } from "fflate";

/** The add-on's id on addons.mozilla.org. Never change it: AMO ties the
 *  add-on (and every signed version) to it. */
export const GECKO_ID = "sussurro@darumahq.it";

/** Where GitHub Pages serves `docs/extension/updates.json` from. */
export const UPDATE_URL = "https://fullo.github.io/Sussurro/extension/updates.json";

export const RELEASES = "https://github.com/fullo/Sussurro/releases/download";

export type UpdateEntry = {
  version: string;
  update_link: string;
  update_hash: string;
  browser_specific_settings?: { gecko: { strict_min_version?: string } };
};

export type UpdateManifest = { addons: Record<string, { updates: UpdateEntry[] }> };

/** Release asset name of the signed add-on, e.g. `sussurro-extension-firefox-0.10.1.xpi`. */
export function xpiName(version: string): string {
  return `sussurro-extension-firefox-${version}.xpi`;
}

/** The signed .xpi attached to the GitHub release `v<version>`. */
export function updateLink(version: string): string {
  return `${RELEASES}/v${version}/${xpiName(version)}`;
}

/** `sha256:<hex>`, the form Firefox checks a downloaded update against. */
export function updateHash(bytes: Uint8Array): string {
  return `sha256:${createHash("sha256").update(bytes).digest("hex")}`;
}

/** A Firefox manifest version as the release ships it: X.Y.Z (the build
 *  drops any pre-release suffix, see manifest.ts). */
export function checkVersion(version: string): string {
  const v = version.trim().replace(/^v/, "");
  if (!/^(0|[1-9]\d{0,4})\.(0|[1-9]\d{0,4})\.(0|[1-9]\d{0,4})$/.test(v)) {
    throw new Error(`"${version}" is not an X.Y.Z extension version`);
  }
  return v;
}

/** Orders dotted-integer versions numerically (0.10.0 after 0.9.9). */
export function compareVersions(a: string, b: string): number {
  const pa = a.split(".").map(Number);
  const pb = b.split(".").map(Number);
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const d = (pa[i] ?? 0) - (pb[i] ?? 0);
    if (d) return d;
  }
  return 0;
}

/** An update manifest listing no version yet. */
export function emptyManifest(id: string = GECKO_ID): UpdateManifest {
  return { addons: { [id]: { updates: [] } } };
}

/** Checks the shape Firefox expects, so a hand edit can't silently break
 *  every installed copy's update check. Returns the manifest unchanged. */
export function validateManifest(value: unknown, id: string = GECKO_ID): UpdateManifest {
  const addons = (value as UpdateManifest | null)?.addons;
  const updates = addons?.[id]?.updates;
  if (!addons || typeof addons !== "object" || !Array.isArray(updates)) {
    throw new Error(`update manifest must be { "addons": { "${id}": { "updates": [...] } } }`);
  }
  const seen = new Set<string>();
  for (const u of updates) {
    checkVersion(u?.version ?? "");
    if (seen.has(u.version)) throw new Error(`version ${u.version} is listed twice`);
    seen.add(u.version);
    if (typeof u.update_link !== "string" || !u.update_link.startsWith("https://")) {
      throw new Error(`version ${u.version}: update_link must be an https URL`);
    }
    if (!/^sha256:[0-9a-f]{64}$/.test(u.update_hash ?? "")) {
      throw new Error(`version ${u.version}: update_hash must be sha256:<64 hex>`);
    }
  }
  return value as UpdateManifest;
}

/** The manifest with `entry` added (replacing an entry for the same
 *  version) and versions in ascending order. The input is not modified. */
export function addUpdate(manifest: UpdateManifest, entry: UpdateEntry, id: string = GECKO_ID): UpdateManifest {
  checkVersion(entry.version);
  const current = validateManifest(manifest, id).addons[id].updates;
  const updates = [...current.filter((u) => u.version !== entry.version), entry].sort((a, b) =>
    compareVersions(a.version, b.version),
  );
  return validateManifest({ ...manifest, addons: { ...manifest.addons, [id]: { ...manifest.addons[id], updates } } }, id);
}

/** What a signed .xpi says about itself: its manifest's version, gecko id
 *  and minimum Firefox version. Throws on anything that is not a zip with
 *  a manifest.json. */
export function readXpi(bytes: Uint8Array): { version: string; id?: string; strict_min_version?: string } {
  let files: Record<string, Uint8Array>;
  try {
    files = unzipSync(bytes, { filter: (f) => f.name === "manifest.json" });
  } catch (e) {
    throw new Error(`not a zip/xpi: ${(e as Error).message}`);
  }
  if (!files["manifest.json"]) throw new Error("the .xpi has no manifest.json");
  const m = JSON.parse(strFromU8(files["manifest.json"]));
  const gecko = m?.browser_specific_settings?.gecko ?? {};
  return { version: checkVersion(String(m?.version ?? "")), id: gecko.id, strict_min_version: gecko.strict_min_version };
}

/** The update entry for a signed .xpi's bytes, cross-checked against the
 *  version it is published as. The entry's `strict_min_version` is the
 *  one the .xpi itself declares, so Firefox never offers an update the
 *  installed browser would then refuse to install (0.10.0 required 128;
 *  later versions require 140, #234). When `minVersion` is given (the
 *  current `manifest.firefox.json`'s), the .xpi must declare exactly that:
 *  a mismatch means a stale or foreign build. Entries already in the
 *  manifest are never rewritten (see addUpdate). */
export function entryForXpi(bytes: Uint8Array, version: string, id: string = GECKO_ID, minVersion?: string): UpdateEntry {
  const v = checkVersion(version);
  const xpi = readXpi(bytes);
  if (xpi.version !== v) throw new Error(`the .xpi is version ${xpi.version}, not ${v}`);
  if (xpi.id !== id) throw new Error(`the .xpi's gecko id is ${xpi.id ?? "missing"}, not ${id}`);
  if (minVersion !== undefined && xpi.strict_min_version !== minVersion) {
    throw new Error(
      `the .xpi requires Firefox ${xpi.strict_min_version ?? "(no minimum)"}, but manifest.firefox.json says ${minVersion}: is it a stale build?`,
    );
  }
  const entry: UpdateEntry = { version: v, update_link: updateLink(v), update_hash: updateHash(bytes) };
  if (xpi.strict_min_version) entry.browser_specific_settings = { gecko: { strict_min_version: xpi.strict_min_version } };
  return entry;
}

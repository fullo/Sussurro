/* Adds a released version to Firefox's self-hosted update manifest (#228):
 *
 *   node scripts/update-manifest.ts 0.10.1            (npm run update-manifest -- 0.10.1)
 *   node scripts/update-manifest.ts 0.10.1 path/to/sussurro-extension-firefox-0.10.1.xpi
 *
 * Run it AFTER the GitHub release v<version> is published: with no path it
 * downloads the signed .xpi from the release (a draft's assets are not
 * public, so this also proves the link Firefox will follow works), checks
 * its version, gecko id and minimum Firefox (it must match
 * manifest.firefox.json's strict_min_version, #234), and writes the entry —
 * update_link, the SHA-256 update_hash and that strict_min_version — into
 * docs/extension/updates.json, leaving the other versions' entries as
 * published. Commit that file
 * on a branch and merge it: GitHub Pages then serves it at the manifest's
 * update_url, and installed copies update on their next check.
 * Runs directly under Node's type stripping (Node >= 24).
 */
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import { addUpdate, checkVersion, entryForXpi, updateLink, validateManifest } from "./updates.ts";

const EXT = dirname(dirname(fileURLToPath(import.meta.url)));
const FILE = join(EXT, "..", "docs", "extension", "updates.json");
const TEMPLATE = join(EXT, "manifest.firefox.json");

const [versionArg, xpiPath] = process.argv.slice(2);
if (!versionArg) {
  console.error("usage: node scripts/update-manifest.ts <version> [signed .xpi]");
  process.exit(2);
}

try {
  const version = checkVersion(versionArg);
  let bytes: Uint8Array;
  if (xpiPath) {
    bytes = readFileSync(xpiPath);
  } else {
    const url = updateLink(version);
    const res = await fetch(url, { redirect: "follow" });
    if (!res.ok) throw new Error(`${url}: HTTP ${res.status} (is release v${version} published, with its .xpi?)`);
    bytes = new Uint8Array(await res.arrayBuffer());
  }
  // The new entry carries the minimum Firefox the .xpi declares, which must
  // be the current manifest's (#234): published entries keep their own.
  const minVersion: string = JSON.parse(readFileSync(TEMPLATE, "utf8")).browser_specific_settings.gecko.strict_min_version;
  const entry = entryForXpi(bytes, version, undefined, minVersion);
  const manifest = addUpdate(validateManifest(JSON.parse(readFileSync(FILE, "utf8"))), entry);
  writeFileSync(FILE, JSON.stringify(manifest, null, 2) + "\n");
  console.log(`${relative(process.cwd(), FILE)}: v${version} → ${entry.update_link} (${entry.update_hash})`);
} catch (e) {
  console.error(`update-manifest: ${(e as Error).message}`);
  process.exit(1);
}

/* Writes the AMO submission metadata (reviewer notes + desktop-only
 * compatibility, #234) for `web-ext sign --amo-metadata`:
 *
 *   node scripts/amo-metadata.ts <out.json>
 *
 * The notes are extension/AMO-REVIEWER-NOTES.md. Used by the release
 * workflow's signing step. Runs directly under Node's type stripping.
 */
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { amoMetadata } from "./amo.ts";

const EXT = dirname(dirname(fileURLToPath(import.meta.url)));

const out = process.argv[2];
if (!out) {
  console.error("usage: node scripts/amo-metadata.ts <out.json>");
  process.exit(2);
}
try {
  const meta = amoMetadata(readFileSync(join(EXT, "AMO-REVIEWER-NOTES.md"), "utf8"));
  writeFileSync(out, JSON.stringify(meta, null, 2) + "\n");
  console.log(`${out}: reviewer notes (${meta.version.approval_notes.length} chars), compatibility ${meta.version.compatibility.join(", ")}`);
} catch (e) {
  console.error(`amo-metadata: ${(e as Error).message}`);
  process.exit(1);
}

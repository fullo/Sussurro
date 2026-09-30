/* `web-ext lint` on a Firefox build, failing on any error and on any
 * warning not accepted in lint-policy.ts (#234):
 *
 *   node scripts/lint.ts                (npm run lint, after npm run build:firefox)
 *   node scripts/lint.ts listed         (npm run lint:listed, after npm run build:firefox:listed)
 *
 * `--self-hosted` (the default target): the add-on is distributed outside
 * AMO's listing with its own update_url, which listed-mode lint rejects
 * (MANIFEST_UPDATE_URL). The `listed` argument instead lints dist/firefox-listed
 * (no update_url, #268/#269) in AMO's normal, listed mode — the check the
 * AMO listed-channel submission must pass. Runs directly under Node's type
 * stripping (Node >= 24).
 */
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import webExt from "web-ext";
import { unacceptedFindings, type LintResult } from "./lint-policy.ts";

const EXT = dirname(dirname(fileURLToPath(import.meta.url)));
const arg = process.argv[2];
if (arg !== undefined && arg !== "listed") {
  console.error("usage: node scripts/lint.ts [listed]");
  process.exit(2);
}
const listed = arg === "listed";
const sourceDir = join(EXT, "dist", listed ? "firefox-listed" : "firefox");

const result = (await webExt.cmd.lint(
  { sourceDir, selfHosted: !listed, output: "text" },
  { shouldExitProgram: false },
)) as LintResult;

const lines = new Map<string, string[]>();
const lineOf = (file: string, line: number): string => {
  if (!lines.has(file)) lines.set(file, readFileSync(join(sourceDir, file), "utf8").split("\n"));
  return lines.get(file)![line - 1] ?? "";
};

const bad = unacceptedFindings(result, lineOf);
if (bad.length) {
  console.error(`\nweb-ext lint: ${bad.length} finding(s) not accepted in scripts/lint-policy.ts:`);
  for (const b of bad) console.error(`  - ${b}`);
  console.error("Fix them, or accept them there and explain why in AMO-REVIEWER-NOTES.md and the README.");
  process.exit(1);
}
console.log(
  `\nweb-ext lint (${listed ? "listed" : "self-hosted"}): ${result.errors.length} errors; ${result.warnings.length} warnings, all accepted (AMO-REVIEWER-NOTES.md).`,
);

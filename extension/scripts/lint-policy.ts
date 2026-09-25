/* Which `web-ext lint` findings the Firefox build may ship with (#234).
 * Pure; scripts/lint.ts runs the linter and applies it, and
 * lint-policy.test.ts pins it. Every accepted warning is explained in
 * extension/AMO-REVIEWER-NOTES.md (sent to AMO's reviewers with each
 * signed version) and in the extension README; anything else fails the
 * lint step, so a new warning can't slip into a signed release unnoticed.
 */

/** One finding as addons-linter reports it (line/column are 1-based). */
export type Finding = { code: string; file?: string; line?: number; column?: number; message?: string };

export type LintResult = { errors: Finding[]; warnings: Finding[]; notices: Finding[] };

/** React DOM's own write for `dangerouslySetInnerHTML`, as it appears
 *  minified in react-dom-client.production.js (`setProp` and
 *  `setPropOnCustomElement`):  `n?.__html!==u&&(l.innerHTML=u)`. */
const REACT_INNER_HTML = /__html\)?!==([\w$]+)&&\(([\w$]+)\.innerHTML=\1\)/g;

/** How many of those React DOM has: one per prop setter. */
export const REACT_INNER_HTML_SITES = 2;

const PAGE_CHUNK = /^assets\/page-[\w-]+\.js$/;

/** Is the flagged column inside React DOM's dangerouslySetInnerHTML write? */
export function isReactInnerHtml(lineText: string, column: number): boolean {
  const at = column - 1;
  for (const m of lineText.matchAll(REACT_INNER_HTML)) {
    const start = m.index + m[0].indexOf(`${m[2]}.innerHTML`);
    if (at >= start && at < start + m[2].length + ".innerHTML".length) return true;
  }
  return false;
}

/**
 * The findings the build must not ship with, as messages (empty = pass).
 * `lineOf(file, line)` returns that 1-based line of a packaged file.
 */
export function unacceptedFindings(result: LintResult, lineOf: (file: string, line: number) => string): string[] {
  const where = (f: Finding) => `${f.code} ${f.file ?? ""}${f.line ? `:${f.line}:${f.column ?? 0}` : ""}`.trim();
  const bad: string[] = result.errors.map((f) => `error: ${where(f)} ${f.message ?? ""}`.trim());
  let reactSites = 0;
  for (const w of result.warnings) {
    if (w.code === "KEY_FIREFOX_ANDROID_UNSUPPORTED_BY_MIN_VERSION" && w.file === "manifest.json") {
      // The add-on is desktop-only (no gecko_android): the linter still
      // checks Android against gecko.strict_min_version (140 < 142).
      continue;
    }
    if (
      w.code === "UNSAFE_VAR_ASSIGNMENT" &&
      w.file !== undefined &&
      PAGE_CHUNK.test(w.file) &&
      w.line !== undefined &&
      w.column !== undefined &&
      isReactInnerHtml(lineOf(w.file, w.line), w.column)
    ) {
      reactSites++;
      continue;
    }
    bad.push(`warning: ${where(w)} ${w.message ?? ""}`.trim());
  }
  if (reactSites > REACT_INNER_HTML_SITES) {
    bad.push(`warning: ${reactSites} React DOM innerHTML sites, expected at most ${REACT_INNER_HTML_SITES}`);
  }
  return bad;
}

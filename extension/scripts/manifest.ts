/* Pure helpers for the extension build (no I/O): version mapping, manifest
   assembly and the list of files a manifest points at. Unit-tested in
   manifest.test.ts; scripts/build.ts does the I/O around them. */

export type Target = "chrome" | "firefox";

export const TARGETS: readonly Target[] = ["chrome", "firefox"];

export function isTarget(s: string | undefined): s is Target {
  return s === "chrome" || s === "firefox";
}

/** A WebExtension manifest, loosely typed: only the keys the build touches. */
export type Manifest = Record<string, unknown> & { version?: string; version_name?: string };

/**
 * The app's semver → the manifest `version` (1–4 dot-separated integers,
 * 0–65535, no leading zeros: the stricter Chrome rule, which Firefox also
 * accepts) plus, for a pre-release or build suffix, the full string as
 * Chrome's `version_name`. The extension ships in lockstep with the app.
 */
export function toManifestVersion(appVersion: string): { version: string; version_name?: string } {
  const m = /^(\d+)\.(\d+)\.(\d+)([-+].*)?$/.exec(appVersion.trim());
  if (!m) throw new Error(`app version "${appVersion}" is not semver (X.Y.Z[-pre][+build])`);
  const parts = [m[1], m[2], m[3]];
  for (const p of parts) {
    if (p.length > 1 && p.startsWith("0")) throw new Error(`app version "${appVersion}": leading zero in "${p}"`);
    if (Number(p) > 65535) throw new Error(`app version "${appVersion}": "${p}" exceeds 65535`);
  }
  const version = parts.join(".");
  return m[4] ? { version, version_name: appVersion.trim() } : { version };
}

/** The template manifest with the app's version stamped in. `version_name`
 *  is Chrome-only (Firefox's linter rejects it). */
export function buildManifest(template: Manifest, target: Target, appVersion: string): Manifest {
  const { version, version_name } = toManifestVersion(appVersion);
  const out: Manifest = { ...template, version };
  delete out.version_name;
  if (version_name && target === "chrome") out.version_name = version_name;
  return out;
}

/** Build variants of the Firefox manifest (#268): `self-hosted` is today's
 *  release build, with its own `update_url` (AMO's unlisted/self-distribution
 *  channel, #228). `listed` is the build submitted to AMO's *listed* channel
 *  (#269): AMO refuses `update_url` there (it manages updates itself once an
 *  add-on is listed — see extension/README.md, "Releases, signing and
 *  updates"), so it must come out of the manifest before packaging. Chrome
 *  has no such variant: the same build serves both the Chrome Web Store and
 *  Edge Add-ons. */
export type FirefoxVariant = "self-hosted" | "listed";

/** Strips `browser_specific_settings.gecko.update_url` for the `listed`
 *  Firefox build. Leaves everything else, and any other target, untouched. */
export function forVariant(manifest: Manifest, variant: FirefoxVariant): Manifest {
  if (variant !== "listed") return manifest;
  const bss = manifest.browser_specific_settings as { gecko?: Record<string, unknown> } | undefined;
  if (!bss?.gecko || !("update_url" in bss.gecko)) return manifest;
  const { update_url: _update_url, ...gecko } = bss.gecko;
  return { ...manifest, browser_specific_settings: { ...bss, gecko } };
}

/** Every packaged file the manifest references (scripts, pages, icons), so the
 *  build can fail fast on a typo instead of shipping a broken zip. */
export function referencedFiles(manifest: Manifest): string[] {
  const files = new Set<string>();
  const add = (v: unknown) => {
    if (typeof v === "string" && v) files.add(v);
  };
  const addIcons = (v: unknown) => {
    if (typeof v === "string") add(v);
    else if (v && typeof v === "object") Object.values(v).forEach(add);
  };
  const obj = (v: unknown): Record<string, unknown> =>
    v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : {};

  addIcons(manifest.icons);
  addIcons(obj(manifest.action).default_icon);
  add(obj(manifest.action).default_popup);
  const bg = obj(manifest.background);
  add(bg.service_worker);
  if (Array.isArray(bg.scripts)) bg.scripts.forEach(add);
  add(obj(manifest.side_panel).default_path);
  const sidebar = obj(manifest.sidebar_action);
  add(sidebar.default_panel);
  addIcons(sidebar.default_icon);
  add(obj(manifest.options_ui).page);
  if (Array.isArray(manifest.content_scripts)) {
    for (const cs of manifest.content_scripts) {
      const c = obj(cs);
      if (Array.isArray(c.js)) c.js.forEach(add);
      if (Array.isArray(c.css)) c.css.forEach(add);
    }
  }
  return [...files].sort();
}

/** Release asset name, e.g. `sussurro-extension-firefox-0.9.0.zip`, or, for
 *  the `listed` Firefox variant, `sussurro-extension-firefox-listed-0.9.0.zip`
 *  (kept out of the GitHub release: it is the file submitted to AMO's
 *  listed channel, not something end users install directly, #268). */
export function zipName(target: Target, appVersion: string, variant?: FirefoxVariant): string {
  const suffix = variant === "listed" ? "-listed" : "";
  return `sussurro-extension-${target}${suffix}-${appVersion.trim()}.zip`;
}

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

/** Release asset name, e.g. `sussurro-extension-firefox-0.9.0.zip`. */
export function zipName(target: Target, appVersion: string): string {
  return `sussurro-extension-${target}-${appVersion.trim()}.zip`;
}

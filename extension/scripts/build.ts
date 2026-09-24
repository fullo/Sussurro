/* Builds one browser target into dist/<target>/ and zips it:
 *
 *   node scripts/build.ts chrome|firefox      (npm run build:chrome / build:firefox)
 *
 * Runs directly under Node's TypeScript type stripping (Node >= 24).
 * Three kinds of output, all in the same folder:
 *   - the HTML pages (side panel / sidebar, options) — one Vite build with
 *     React and the shared `@sussurro/transcript` components;
 *   - the background worker and the two content scripts — one Vite build
 *     each as a single self-contained IIFE (content scripts cannot be ES
 *     modules, and the MAIN-world one must not leak globals into the page);
 *   - manifest.json (from manifest.<target>.json, version = the app's) and
 *     the icons, copied from the app's icons.
 */
import { build, mergeConfig, type InlineConfig } from "vite";
import { zipSync } from "fflate";
import { copyFileSync, existsSync, mkdirSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, relative, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { baseConfig } from "../vite.config.ts";
import { buildManifest, isTarget, referencedFiles, zipName, TARGETS, type Manifest } from "./manifest.ts";

const EXT = dirname(dirname(fileURLToPath(import.meta.url)));
const APP = join(EXT, "..", "sussurro");

const target = process.argv[2];
if (!isTarget(target)) {
  console.error(`usage: node scripts/build.ts <${TARGETS.join("|")}>`);
  process.exit(2);
}

const appVersion: string = JSON.parse(readFileSync(join(APP, "package.json"), "utf8")).version;
const outDir = join(EXT, "dist", target);

const common: InlineConfig = {
  configFile: false,
  root: EXT,
  base: "./",
  publicDir: false,
  logLevel: "warn",
  build: { outDir, emptyOutDir: false, modulePreload: false, reportCompressedSize: false },
};
const config = (extra: InlineConfig): InlineConfig => mergeConfig(mergeConfig(baseConfig(target), common), extra);

rmSync(outDir, { recursive: true, force: true });
mkdirSync(outDir, { recursive: true });

// 1. Pages.
await build(
  config({
    build: {
      rollupOptions: {
        input: {
          sidepanel: join(EXT, "sidepanel.html"),
          options: join(EXT, "options.html"),
          // Chrome only: the tab-capture fallback (Firefox has no tabCapture).
          ...(target === "chrome" ? { offscreen: join(EXT, "offscreen.html") } : {}),
        },
      },
    },
  }),
);

// 2. Background worker + content scripts, one self-contained file each.
const scripts: [name: string, entry: string][] = [
  ["background", "src/background/index.ts"],
  ["content-main", "src/content/main-world.ts"],
  ["content-isolated", "src/content/isolated-world.ts"],
];
for (const [name, entry] of scripts) {
  await build(
    config({
      build: {
        rollupOptions: {
          input: join(EXT, entry),
          output: { format: "iife", entryFileNames: `${name}.js`, inlineDynamicImports: true },
        },
      },
    }),
  );
}

// Chrome runs the background as a service worker: no DOM. A stray import
// of the page code (React, the transcript components' CSS) would bring
// `document` in and kill the worker at load — refuse to ship that.
if (/\bdocument\./.test(readFileSync(join(outDir, "background.js"), "utf8"))) {
  console.error("background.js uses `document`, which Chrome's service worker lacks: keep page-only code out of src/background/.");
  process.exit(1);
}

// 3. Icons (the app's own) and the manifest.
mkdirSync(join(outDir, "icons"), { recursive: true });
for (const size of [32, 64, 128]) {
  copyFileSync(join(APP, "src-tauri", "icons", `${size}x${size}.png`), join(outDir, "icons", `${size}.png`));
}
const template: Manifest = JSON.parse(readFileSync(join(EXT, `manifest.${target}.json`), "utf8"));
const manifest = buildManifest(template, target, appVersion);
writeFileSync(join(outDir, "manifest.json"), JSON.stringify(manifest, null, 2) + "\n");

const missing = referencedFiles(manifest).filter((f) => !existsSync(join(outDir, f)));
if (missing.length) {
  console.error(`manifest.json references files the build did not produce: ${missing.join(", ")}`);
  process.exit(1);
}

// 4. Zip (sorted entries, fixed timestamps: same input → same bytes).
const files: Record<string, [Uint8Array, { mtime: Date }]> = {};
const walk = (dir: string) => {
  for (const name of readdirSync(dir).sort()) {
    const full = join(dir, name);
    if (statSync(full).isDirectory()) walk(full);
    else files[relative(outDir, full).split(sep).join("/")] = [readFileSync(full), { mtime: new Date(1980, 0, 1) }];
  }
};
walk(outDir);
const zipPath = join(EXT, "dist", zipName(target, appVersion));
writeFileSync(zipPath, zipSync(files, { level: 9 }));

console.log(`${target}: ${relative(EXT, outDir)}/ (${Object.keys(files).length} files) → ${relative(EXT, zipPath)} [v${manifest.version}]`);

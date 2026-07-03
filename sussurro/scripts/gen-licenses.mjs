// Generates src/licenses.json — the third-party license list shown in the
// About dialog. Reads the *actual* resolved dependencies (Rust crates via
// `cargo metadata`, npm production deps via `npm ls`) and bundles each
// package's license id + full license text, so the About page works offline.
//
// Regenerate after changing dependencies:  npm run licenses
//
// Not run at build time on purpose — keeps the release/CI pipeline unchanged
// (see CLAUDE.md "keep CI simple"). The generated file is committed.

import { execFileSync } from "node:child_process";
import { readdirSync, readFileSync, writeFileSync, existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const LICENSE_FILE_RE = /^(LICEN[CS]E|COPYING|NOTICE|UNLICENSE)/i;

/** Read and concatenate the license files found directly in a package dir. */
function readLicenseText(pkgDir) {
  if (!pkgDir || !existsSync(pkgDir)) return "";
  let out = [];
  for (const name of readdirSync(pkgDir).sort()) {
    if (!LICENSE_FILE_RE.test(name)) continue;
    try {
      const text = readFileSync(join(pkgDir, name), "utf8").trim();
      if (text) out.push(out.length ? `\n--- ${name} ---\n${text}` : text);
    } catch {
      /* directory entry, unreadable file — skip */
    }
  }
  return out.join("\n");
}

function rustCrates() {
  const meta = JSON.parse(
    execFileSync("cargo", ["metadata", "--format-version", "1"], {
      cwd: join(ROOT, "src-tauri"),
      maxBuffer: 64 * 1024 * 1024,
      encoding: "utf8",
    }),
  );
  const workspace = new Set(meta.workspace_members);
  return meta.packages
    .filter((p) => !workspace.has(p.id)) // drop our own crate(s)
    .map((p) => ({
      name: p.name,
      version: p.version,
      license: p.license || "(unspecified)",
      repository: p.repository || "",
      text: readLicenseText(dirname(p.manifest_path)),
      ecosystem: "rust",
    }));
}

function npmPackages() {
  // --parseable prints the install path of every (production) dependency.
  let paths;
  try {
    paths = execFileSync(
      "npm",
      ["ls", "--all", "--omit=dev", "--parseable"],
      { cwd: ROOT, maxBuffer: 64 * 1024 * 1024, encoding: "utf8" },
    );
  } catch (e) {
    // `npm ls` exits non-zero on peer-dep warnings but still prints paths.
    paths = e.stdout ? e.stdout.toString() : "";
  }
  const seen = new Set();
  const out = [];
  for (const dir of paths.split("\n").map((s) => s.trim()).filter(Boolean)) {
    const manifest = join(dir, "package.json");
    if (!existsSync(manifest)) continue;
    let pkg;
    try {
      pkg = JSON.parse(readFileSync(manifest, "utf8"));
    } catch {
      continue;
    }
    if (!pkg.name || dir === ROOT) continue; // skip the app itself
    const key = `${pkg.name}@${pkg.version}`;
    if (seen.has(key)) continue;
    seen.add(key);
    const license =
      pkg.license ||
      (Array.isArray(pkg.licenses)
        ? pkg.licenses.map((l) => l.type).join(" OR ")
        : "") ||
      "(unspecified)";
    out.push({
      name: pkg.name,
      version: pkg.version || "",
      license,
      repository:
        (pkg.repository && (pkg.repository.url || pkg.repository)) || "",
      text: readLicenseText(dir),
      ecosystem: "npm",
    });
  }
  return out;
}

const byName = (a, b) =>
  a.name.localeCompare(b.name) || a.version.localeCompare(b.version);
const collected = [...rustCrates().sort(byName), ...npmPackages().sort(byName)];

// Deduplicate license texts by content: Apache-2.0 (identical everywhere)
// collapses to one entry, while MIT texts — which embed each project's own
// copyright line — stay distinct. Cuts the file from ~5.6 MB to a fraction.
const texts = [];
const textIndex = new Map();
const packages = collected.map((p) => {
  let textId = -1;
  if (p.text) {
    if (!textIndex.has(p.text)) {
      textIndex.set(p.text, texts.length);
      texts.push(p.text);
    }
    textId = textIndex.get(p.text);
  }
  return {
    name: p.name,
    version: p.version,
    license: p.license,
    repository: p.repository,
    ecosystem: p.ecosystem,
    textId,
  };
});

const rust = packages.filter((p) => p.ecosystem === "rust").length;
const npm = packages.length - rust;
// public/ (not src/) so Vite serves it as a static asset the About dialog
// fetches on demand — it never enters the main JS bundle.
writeFileSync(
  join(ROOT, "public", "licenses.json"),
  JSON.stringify({ packages, texts }) + "\n",
);
console.log(
  `Wrote public/licenses.json — ${rust} Rust crates, ${npm} npm packages, ` +
    `${texts.length} unique license texts`,
);

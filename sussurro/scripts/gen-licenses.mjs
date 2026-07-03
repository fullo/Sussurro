// Generates src/licenses.json — the third-party license list shown in the
// About dialog. Reads the *actual* resolved dependencies (Rust crates via
// `cargo metadata`, npm production deps via `npm ls`) and bundles each
// package's license + full license text, so the About page works offline.
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

// --- SPDX expression handling -------------------------------------------------
// A crate declares an SPDX expression like "MIT OR Apache-2.0". `OR` means the
// redistributor *chooses* one; `AND` means all apply. For an attribution page
// we resolve each `OR` to a single license (so we don't show a misleading
// "MIT OR Apache-2.0") while keeping every `AND` term. Preference order below
// picks the conventional, most-permissive option — MIT first.
const PRIORITY = {
  "MIT": 0, "MIT-0": 1, "Apache-2.0": 2, "ISC": 3, "BSD-2-Clause": 4,
  "BSD-3-Clause": 5, "0BSD": 6, "Zlib": 7, "BSL-1.0": 8, "Unlicense": 9,
  "CC0-1.0": 10, "Unicode-3.0": 11, "Unicode-DFS-2016": 11, "MPL-2.0": 20,
  "CDLA-Permissive-2.0": 21, "LGPL-2.1-or-later": 30, "GPL-3.0-or-later": 31,
};
const priorityOf = (id) => {
  const base = id.replace(/\s+WITH\s+.*$/i, "").trim();
  const p = PRIORITY[base];
  return (p === undefined ? 100 : p) + (/\sWITH\s/i.test(id) ? 0.5 : 0);
};

function tokenizeSpdx(expr) {
  return expr
    .replace(/\s*\/\s*/g, " OR ") // legacy dual-license notation: `A / B`
    .replace(/\(/g, " ( ")
    .replace(/\)/g, " ) ")
    .split(/\s+/)
    .filter(Boolean);
}

/** Parse an SPDX expression and return the chosen license id(s) as an array. */
function chooseLicenses(expr) {
  const tokens = tokenizeSpdx(expr);
  let pos = 0;
  const parseOr = () => {
    const branches = [parseAnd()];
    while (tokens[pos] === "OR") {
      pos++;
      branches.push(parseAnd());
    }
    return branches.length === 1 ? branches[0] : { op: "OR", branches };
  };
  const parseAnd = () => {
    const parts = [parsePrimary()];
    while (tokens[pos] === "AND") {
      pos++;
      parts.push(parsePrimary());
    }
    return parts.length === 1 ? parts[0] : { op: "AND", parts };
  };
  const parsePrimary = () => {
    if (tokens[pos] === "(") {
      pos++;
      const e = parseOr();
      if (tokens[pos] === ")") pos++;
      return e;
    }
    let id = tokens[pos++];
    if (tokens[pos] === "WITH") id += ` WITH ${tokens[(pos += 2) - 1]}`;
    return { op: "LEAF", id };
  };
  const evaluate = (node) => {
    if (node.op === "LEAF") return [node.id];
    if (node.op === "AND") return node.parts.flatMap(evaluate);
    // OR: keep the branch with the best (lowest) preference score.
    const best = (ids) => Math.min(...ids.map(priorityOf));
    return node.branches
      .map(evaluate)
      .sort((a, b) => best(a) - best(b) || a.length - b.length)[0];
  };
  return [...new Set(evaluate(parseOr()))];
}

function resolveLicense(raw) {
  if (!raw || raw === "(unspecified)") {
    return { license: "(unspecified)", spdx: "", ids: [] };
  }
  let ids;
  try {
    ids = chooseLicenses(raw);
  } catch {
    ids = []; // unparseable — fall back to showing the raw expression
  }
  const license = ids.length ? [...ids].sort().join(" AND ") : raw;
  // Record the original expression only when we actually made a choice.
  const spdx = license !== raw ? raw : "";
  return { license, spdx, ids };
}

// --- License text extraction -------------------------------------------------
const LICENSE_KEYWORDS = {
  "MIT": ["MIT"], "MIT-0": ["MIT"], "Apache-2.0": ["APACHE"], "ISC": ["ISC"],
  "BSD-2-Clause": ["BSD"], "BSD-3-Clause": ["BSD"], "0BSD": ["0BSD", "BSD"],
  "Zlib": ["ZLIB"], "BSL-1.0": ["BSL", "BOOST"], "Unlicense": ["UNLICEN"],
  "CC0-1.0": ["CC0"], "Unicode-3.0": ["UNICODE"], "MPL-2.0": ["MPL"],
  "GPL-3.0-or-later": ["GPL"], "LGPL-2.1-or-later": ["LGPL", "GPL"],
};

/**
 * Read license text from a package dir, preferring the file(s) matching the
 * chosen license(s) — so a crate resolved to "MIT" shows LICENSE-MIT, not the
 * Apache text it also ships. Falls back to every license file when nothing
 * matches (single generic LICENSE, unusual naming, …).
 */
function readLicenseText(pkgDir, ids) {
  if (!pkgDir || !existsSync(pkgDir)) return "";
  const files = readdirSync(pkgDir).filter((n) => LICENSE_FILE_RE.test(n)).sort();
  const keywords = ids.flatMap((id) => {
    const base = id.replace(/\s+WITH\s+.*$/i, "").trim();
    return LICENSE_KEYWORDS[base] || [];
  });
  const matched = keywords.length
    ? files.filter((n) => keywords.some((k) => n.toUpperCase().includes(k)))
    : [];
  const chosen = matched.length ? matched : files;
  const out = [];
  for (const name of chosen) {
    try {
      const text = readFileSync(join(pkgDir, name), "utf8").trim();
      if (text) out.push(out.length ? `\n--- ${name} ---\n${text}` : text);
    } catch {
      /* directory entry / unreadable — skip */
    }
  }
  return out.join("\n");
}

function pkgEntry(name, version, rawLicense, repository, dir, ecosystem) {
  const { license, spdx, ids } = resolveLicense(rawLicense);
  return {
    name,
    version,
    license,
    spdx,
    repository,
    text: readLicenseText(dir, ids),
    ecosystem,
  };
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
    .map((p) =>
      pkgEntry(
        p.name,
        p.version,
        p.license || "(unspecified)",
        p.repository || "",
        dirname(p.manifest_path),
        "rust",
      ),
    );
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
    if (!existsSync(manifest) || dir === ROOT) continue;
    let pkg;
    try {
      pkg = JSON.parse(readFileSync(manifest, "utf8"));
    } catch {
      continue;
    }
    if (!pkg.name) continue;
    const key = `${pkg.name}@${pkg.version}`;
    if (seen.has(key)) continue;
    seen.add(key);
    const license =
      pkg.license ||
      (Array.isArray(pkg.licenses)
        ? pkg.licenses.map((l) => l.type).join(" OR ")
        : "") ||
      "(unspecified)";
    out.push(
      pkgEntry(
        pkg.name,
        pkg.version || "",
        license,
        (pkg.repository && (pkg.repository.url || pkg.repository)) || "",
        dir,
        "npm",
      ),
    );
  }
  return out;
}

const byName = (a, b) =>
  a.name.localeCompare(b.name) || a.version.localeCompare(b.version);
const collected = [...rustCrates().sort(byName), ...npmPackages().sort(byName)];

// Deduplicate license texts by content: Apache-2.0 (identical everywhere)
// collapses to one entry, while MIT texts — which embed each project's own
// copyright line — stay distinct. Cuts the file size to a fraction.
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
    spdx: p.spdx,
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

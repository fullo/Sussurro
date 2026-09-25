// Generates public/licenses.json — the third-party license list shown in the
// About dialog. Reads the *actual* resolved dependencies (Rust crates via
// `cargo metadata`, npm production deps via `npm ls`) and bundles each
// package's license + full license text, so the About page works offline.
//
// Regenerate after changing dependencies:  npm run licenses
//
// With `--extension` it writes the browser extension's list instead
// (extension/src/options/licenses.json, shown on the extension's options
// page, #138): the npm production deps of extension/ only — the extension
// has no Rust, models or binaries. Run it from extension/ as
// `npm run licenses` (needs `npm ci` there first).
//
// Dev dependencies (build tools, test runners, type packages) are never
// listed: they are not shipped. Both modes check it and fail otherwise.
//
// Not run at build time on purpose — keeps the release/CI pipeline unchanged
// (see CLAUDE.md "keep CI simple"). The generated file is committed.

import { execFileSync } from "node:child_process";
import { readdirSync, readFileSync, writeFileSync, existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const EXTENSION = resolve(ROOT, "..", "extension");
const EXTENSION_MODE = process.argv.includes("--extension");
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

function npmPackages(root = ROOT) {
  // --parseable prints the install path of every (production) dependency.
  let paths;
  try {
    paths = execFileSync(
      "npm",
      ["ls", "--all", "--omit=dev", "--parseable"],
      { cwd: root, maxBuffer: 64 * 1024 * 1024, encoding: "utf8" },
    );
  } catch (e) {
    // `npm ls` exits non-zero on peer-dep warnings but still prints paths.
    paths = e.stdout ? e.stdout.toString() : "";
  }
  const seen = new Set();
  const out = [];
  for (const dir of paths.split("\n").map((s) => s.trim()).filter(Boolean)) {
    const manifest = join(dir, "package.json");
    if (!existsSync(manifest) || dir === root) continue;
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

// Models Sussurro downloads on first use and runs locally. They are not
// packages, so no manifest lists them: their licences and attribution
// lines live here. CC-BY-4.0 requires the attribution below (#130).
function downloadedModels() {
  return [
    {
      name: "WeSpeaker ResNet34-LM (speaker embeddings)",
      version: "voxceleb_resnet34_LM.onnx",
      license: "CC-BY-4.0",
      spdx: "",
      repository: "https://huggingface.co/Wespeaker/wespeaker-voxceleb-resnet34-LM",
      text:
        "Speaker labels (\"Voice 1, Voice 2…\") use the WeSpeaker ResNet34-LM speaker " +
        "embedding model (file voxceleb_resnet34_LM.onnx, trained on VoxCeleb2) by the " +
        "WeSpeaker project, https://github.com/wenet-e2e/wespeaker — Wang et al., " +
        "\"Wespeaker: A research and production oriented speaker embedding learning " +
        "toolkit\", ICASSP 2023. Downloaded unmodified from " +
        "https://huggingface.co/Wespeaker/wespeaker-voxceleb-resnet34-LM on first use. " +
        "Licensed under the Creative Commons Attribution 4.0 International licence " +
        "(CC BY 4.0): https://creativecommons.org/licenses/by/4.0/",
      ecosystem: "model",
    },
  ];
}

// Prebuilt upstream binaries shipped inside the installers (the llama-server
// sidecar, #116). Read from the pinned lock so the version and licence texts
// always match what `npm run sidecar` bundles.
function bundledBinaries() {
  const sidecarDir = join(ROOT, "src-tauri", "sidecar");
  const lock = JSON.parse(readFileSync(join(sidecarDir, "llama-server.lock.json"), "utf8"));
  const out = [
    {
      name: "llama.cpp (llama-server sidecar)",
      version: lock.release,
      license: lock.license,
      spdx: "",
      repository: lock.upstream,
      text: readFileSync(join(sidecarDir, lock.licenseText), "utf8").trim(),
      ecosystem: "binary",
    },
  ];
  const seen = new Set();
  for (const t of Object.values(lock.targets)) {
    for (const x of t.extraLicenses) {
      if (seen.has(x.component)) continue;
      seen.add(x.component);
      out.push({
        name: x.component,
        version: lock.release,
        license: x.license,
        spdx: "",
        repository: x.repository || "",
        text: readFileSync(join(sidecarDir, x.text), "utf8").trim(),
        ecosystem: "binary",
      });
    }
  }
  return out;
}

/** Fails when a direct dev dependency of `root` made it into the list:
 *  `npm ls --omit=dev` should never return one, and none of them ships.
 *  (A dev dependency that is also a production one isn't dev-only.) */
function assertNoDevOnly(root, npm) {
  const pkg = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
  const prod = new Set(Object.keys(pkg.dependencies || {}));
  const devOnly = new Set(Object.keys(pkg.devDependencies || {}).filter((d) => !prod.has(d)));
  const leaked = npm.filter((p) => devOnly.has(p.name)).map((p) => p.name);
  if (leaked.length) {
    throw new Error(`dev dependencies in the licence list of ${root}: ${leaked.join(", ")}`);
  }
  if (prod.size && !npm.length) {
    throw new Error(`no npm packages found in ${root}: run \`npm ci\` there first`);
  }
}

const byName = (a, b) =>
  a.name.localeCompare(b.name) || a.version.localeCompare(b.version);

/** Deduplicates license texts by content: Apache-2.0 (identical
 *  everywhere) collapses to one entry, while MIT texts — which embed each
 *  project's own copyright line — stay distinct. */
function withSharedTexts(collected) {
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
  return { packages, texts };
}

if (EXTENSION_MODE) {
  const npm = npmPackages(EXTENSION).sort(byName);
  assertNoDevOnly(EXTENSION, npm);
  const { packages, texts } = withSharedTexts(npm);
  writeFileSync(
    join(EXTENSION, "src", "options", "licenses.json"),
    JSON.stringify({ packages, texts }, null, 1) + "\n",
  );
  console.log(
    `Wrote extension/src/options/licenses.json — ${packages.length} npm packages, ` +
      `${texts.length} unique license texts`,
  );
  process.exit(0);
}

const appNpm = npmPackages().sort(byName);
assertNoDevOnly(ROOT, appNpm);
const collected = [
  ...rustCrates().sort(byName),
  ...appNpm,
  ...downloadedModels(),
  ...bundledBinaries(),
];

// Shared texts cut the file size to a fraction.
const { packages, texts } = withSharedTexts(collected);

const rust = packages.filter((p) => p.ecosystem === "rust").length;
const models = packages.filter((p) => p.ecosystem === "model").length;
const binaries = packages.filter((p) => p.ecosystem === "binary").length;
const npm = packages.length - rust - models - binaries;
// public/ (not src/) so Vite serves it as a static asset the About dialog
// fetches on demand — it never enters the main JS bundle.
writeFileSync(
  join(ROOT, "public", "licenses.json"),
  JSON.stringify({ packages, texts }) + "\n",
);
console.log(
  `Wrote public/licenses.json — ${rust} Rust crates, ${npm} npm packages, ` +
    `${models} downloaded models, ${binaries} bundled binaries, ` +
    `${texts.length} unique license texts`,
);

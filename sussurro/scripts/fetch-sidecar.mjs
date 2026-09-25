#!/usr/bin/env node
// Downloads the pinned llama.cpp release for one target, verifies its SHA-256
// against src-tauri/sidecar/llama-server.lock.json (fail-closed: a mismatch
// aborts and writes nothing) and lays out what Tauri bundles (plan E9, #116):
//
//   src-tauri/binaries/sussurro-llama-server-<target-triple>[.exe]   externalBin
//   src-tauri/binaries/llama-server-libs/                            resources
//
// The bundle picks these up only when the build merges
// src-tauri/tauri.sidecar.conf.json (`tauri build --config …`), so `cargo
// test`, `cargo clippy` and `tauri dev` keep working without the sidecar.
//
// Usage (from sussurro/):
//   npm run sidecar                          # host target (rustc -vV)
//   npm run sidecar -- --target aarch64-apple-darwin
//   npm run sidecar -- --archive <file>      # use a pre-downloaded asset (still verified)
//   npm run sidecar -- --force               # re-extract even if up to date
//
// Pure Node (no npm deps, no tar/unzip binaries): the archive reader below
// handles the two formats the pinned assets use (.tar.gz and .zip).

import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  readFileSync,
  renameSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { gunzipSync, inflateRawSync } from "node:zlib";

const HERE = dirname(fileURLToPath(import.meta.url));
export const TAURI_DIR = resolve(HERE, "..", "src-tauri");
export const LOCK_PATH = join(TAURI_DIR, "sidecar", "llama-server.lock.json");
export const OUT_DIR = join(TAURI_DIR, "binaries");
const STAMP = "llama-server.stamp.json";

export class ChecksumError extends Error {
  constructor(label, expected, actual) {
    super(
      `SHA-256 mismatch for ${label}: expected ${expected}, got ${actual}. ` +
        "Refusing to use it (fail-closed). If upstream re-published the asset, " +
        "re-pin it deliberately in src-tauri/sidecar/llama-server.lock.json.",
    );
    this.name = "ChecksumError";
  }
}

// --- manifest ---------------------------------------------------------------

const HEX64 = /^[0-9a-f]{64}$/;
const SAFE_NAME = /^[A-Za-z0-9._+-]+$/;
const TRIPLE = /^[a-z0-9_]+-[a-z0-9_]+-[a-z0-9_]+(-[a-z0-9_]+)?$/;

/**
 * Validates the lock manifest and returns it; throws with every problem found.
 * File names must be bare names (no path separators) so nothing from the lock
 * can write outside the output directory.
 */
export function validateLock(lock) {
  const errors = [];
  const need = (cond, msg) => cond || errors.push(msg);
  const str = (v) => typeof v === "string" && v.length > 0;

  need(lock && typeof lock === "object", "lock is not an object");
  if (errors.length) throw new Error(`invalid sidecar lock: ${errors.join("; ")}`);
  need(str(lock.name) && SAFE_NAME.test(lock.name), "name must be a bare file name");
  need(str(lock.upstream) && lock.upstream.startsWith("https://github.com/"), "upstream must be a GitHub https URL");
  need(str(lock.release), "release is required");
  need(str(lock.license), "license (SPDX) is required");
  need(str(lock.licenseFile), "licenseFile is required");
  need(str(lock.licenseText), "licenseText is required");
  need(str(lock.libDir) && SAFE_NAME.test(lock.libDir), "libDir must be a bare directory name");
  const targets = lock.targets && typeof lock.targets === "object" ? lock.targets : {};
  need(Object.keys(targets).length > 0, "targets must list at least one target");

  for (const [triple, t] of Object.entries(targets)) {
    const at = (msg) => `${triple}: ${msg}`;
    need(TRIPLE.test(triple), at("not a target triple"));
    need(t && typeof t === "object", at("entry is not an object"));
    if (!t || typeof t !== "object") continue;
    need(str(t.asset) && SAFE_NAME.test(t.asset), at("asset must be a bare file name"));
    need(/\.(tar\.gz|tgz|zip)$/.test(t.asset ?? ""), at("asset must be .tar.gz or .zip"));
    need(
      t.url === `${lock.upstream}/releases/download/${lock.release}/${t.asset}`,
      at("url must be <upstream>/releases/download/<release>/<asset>"),
    );
    need(str(t.sha256) && HEX64.test(t.sha256), at("sha256 must be 64 lowercase hex chars"));
    need(Number.isInteger(t.size) && t.size > 0, at("size must be a positive integer"));
    need(typeof t.root === "string" && (t.root === "" || t.root.endsWith("/")), at("root must be '' or end with '/'"));
    need(str(t.binary) && SAFE_NAME.test(t.binary), at("binary must be a bare file name"));
    need(
      triple.includes("windows") ? t.binary.endsWith(".exe") : !t.binary.endsWith(".exe"),
      at("binary must end in .exe exactly on Windows targets"),
    );
    need(Array.isArray(t.libs) && t.libs.length > 0, at("libs must be a non-empty array"));
    for (const lib of t.libs ?? []) {
      need(str(lib) && SAFE_NAME.test(lib), at(`lib ${JSON.stringify(lib)} must be a bare file name`));
    }
    need(new Set(t.libs ?? []).size === (t.libs ?? []).length, at("libs has duplicates"));
    need(
      t.licenseInArchive === undefined || typeof t.licenseInArchive === "boolean",
      at("licenseInArchive must be a boolean"),
    );
    need(Array.isArray(t.extraLicenses), at("extraLicenses must be an array"));
    for (const x of t.extraLicenses ?? []) {
      need(str(x.file) && SAFE_NAME.test(x.file), at("extraLicenses[].file must be a bare file name"));
      need(str(x.text) && str(x.component) && str(x.license), at("extraLicenses[] needs text, component, license"));
    }
  }
  if (errors.length) throw new Error(`invalid sidecar lock: ${errors.join("; ")}`);
  return lock;
}

export function readLock(path = LOCK_PATH) {
  return validateLock(JSON.parse(readFileSync(path, "utf8")));
}

/** Tauri's externalBin naming: `<name>-<target-triple>[.exe]`. */
export function sidecarFileName(lock, triple) {
  return `${lock.name}-${triple}${triple.includes("windows") ? ".exe" : ""}`;
}

// --- checksums --------------------------------------------------------------

export const sha256 = (buf) => createHash("sha256").update(buf).digest("hex");

export function verifySha256(buf, expected, label) {
  const actual = sha256(buf);
  if (actual !== expected) throw new ChecksumError(label, expected, actual);
}

// --- archives ---------------------------------------------------------------

const cstr = (buf, start, len) => {
  const end = buf.indexOf(0, start);
  return buf.toString("utf8", start, end === -1 || end > start + len ? start + len : end);
};
const octal = (buf, start, len) => {
  if (buf[start] & 0x80) throw new Error("tar: base-256 sizes are not supported");
  const s = cstr(buf, start, len).trim();
  return s ? parseInt(s, 8) : 0;
};

/**
 * Reads a (gzipped) ustar/GNU/pax tar into Map<name, {type, data?, link?}>.
 * Only regular files and symlinks are kept.
 */
export function readTar(buf) {
  const entries = new Map();
  let off = 0;
  let longName = null;
  let paxPath = null;
  let paxLink = null;
  while (off + 512 <= buf.length) {
    const header = buf.subarray(off, off + 512);
    if (header.every((b) => b === 0)) break;
    const size = octal(header, 124, 12);
    const type = String.fromCharCode(header[156] || 48); // NUL = regular file
    const dataStart = off + 512;
    const data = buf.subarray(dataStart, dataStart + size);
    off = dataStart + Math.ceil(size / 512) * 512;

    if (type === "L") {
      longName = cstr(data, 0, data.length);
      continue;
    }
    if (type === "x") {
      for (const rec of data.toString("utf8").split("\n")) {
        const m = /^\d+ ([^=]+)=(.*)$/.exec(rec);
        if (m && m[1] === "path") paxPath = m[2];
        if (m && m[1] === "linkpath") paxLink = m[2];
      }
      continue;
    }
    if (type === "g") continue;

    let name = cstr(header, 0, 100);
    const prefix = cstr(header, 257, 6) === "ustar" ? cstr(header, 345, 155) : "";
    if (prefix) name = `${prefix}/${name}`;
    name = paxPath ?? longName ?? name;
    const link = paxLink ?? cstr(header, 157, 100);
    longName = paxPath = paxLink = null;

    if (type === "0" || type === "7") entries.set(name, { type: "file", data });
    else if (type === "2") entries.set(name, { type: "symlink", link });
  }
  return entries;
}

/** Reads a zip (stored or deflated, no zip64/encryption) into the same map. */
export function readZip(buf) {
  let eocd = -1;
  for (let i = buf.length - 22; i >= Math.max(0, buf.length - 22 - 0xffff); i--) {
    if (buf.readUInt32LE(i) === 0x06054b50) {
      eocd = i;
      break;
    }
  }
  if (eocd < 0) throw new Error("zip: end of central directory not found");
  const count = buf.readUInt16LE(eocd + 10);
  let p = buf.readUInt32LE(eocd + 16);
  const entries = new Map();
  for (let i = 0; i < count; i++) {
    if (buf.readUInt32LE(p) !== 0x02014b50) throw new Error("zip: bad central directory entry");
    const flags = buf.readUInt16LE(p + 8);
    const method = buf.readUInt16LE(p + 10);
    const compSize = buf.readUInt32LE(p + 20);
    const size = buf.readUInt32LE(p + 24);
    const nameLen = buf.readUInt16LE(p + 28);
    const extraLen = buf.readUInt16LE(p + 30);
    const commentLen = buf.readUInt16LE(p + 32);
    const local = buf.readUInt32LE(p + 42);
    const name = buf.toString("utf8", p + 46, p + 46 + nameLen);
    p += 46 + nameLen + extraLen + commentLen;
    if (name.endsWith("/")) continue;
    if (flags & 1) throw new Error(`zip: ${name} is encrypted`);
    if (compSize === 0xffffffff || size === 0xffffffff || local === 0xffffffff) {
      throw new Error("zip: zip64 archives are not supported");
    }
    if (buf.readUInt32LE(local) !== 0x04034b50) throw new Error(`zip: bad local header for ${name}`);
    const start = local + 30 + buf.readUInt16LE(local + 26) + buf.readUInt16LE(local + 28);
    const raw = buf.subarray(start, start + compSize);
    let data;
    if (method === 0) data = raw;
    else if (method === 8) data = inflateRawSync(raw);
    else throw new Error(`zip: ${name} uses unsupported compression method ${method}`);
    if (data.length !== size) throw new Error(`zip: ${name} has the wrong size`);
    entries.set(name, { type: "file", data });
  }
  return entries;
}

export function readArchive(buf, assetName) {
  if (/\.zip$/i.test(assetName)) return readZip(buf);
  if (/\.(tar\.gz|tgz)$/i.test(assetName)) return readTar(gunzipSync(buf));
  throw new Error(`unsupported archive type: ${assetName}`);
}

/** Returns the bytes of `path`, following symlinks inside the archive. */
export function pickFile(entries, path) {
  let current = path;
  for (let hops = 0; hops < 8; hops++) {
    const e = entries.get(current);
    if (!e) throw new Error(`archive has no ${current}${current !== path ? ` (via ${path})` : ""}`);
    if (e.type === "file") return e.data;
    const base = current.includes("/") ? current.slice(0, current.lastIndexOf("/") + 1) : "";
    if (e.link.startsWith("/") || e.link.split("/").includes("..")) {
      throw new Error(`archive symlink ${current} -> ${e.link} leaves its directory`);
    }
    current = base + e.link;
  }
  throw new Error(`archive symlink loop at ${path}`);
}

// --- fetch + layout ---------------------------------------------------------

/** Host target triple as Tauri sees it (rustc), with a Node fallback. */
export function hostTriple() {
  try {
    const out = execFileSync("rustc", ["-vV"], { encoding: "utf8" });
    const m = /^host: (\S+)$/m.exec(out);
    if (m) return m[1];
  } catch {
    /* no rustc on PATH: fall through */
  }
  const map = {
    "darwin-arm64": "aarch64-apple-darwin",
    "win32-x64": "x86_64-pc-windows-msvc",
    "linux-x64": "x86_64-unknown-linux-gnu",
  };
  const key = `${process.platform}-${process.arch}`;
  if (map[key]) return map[key];
  throw new Error(`cannot determine the target triple for ${key}; pass --target`);
}

async function download(url, { attempts = 3, timeoutMs = 5 * 60_000 } = {}) {
  let lastErr;
  for (let i = 1; i <= attempts; i++) {
    try {
      const res = await fetch(url, { redirect: "follow", signal: AbortSignal.timeout(timeoutMs) });
      if (!res.ok) throw new Error(`HTTP ${res.status} for ${url}`);
      return Buffer.from(await res.arrayBuffer());
    } catch (e) {
      lastErr = e;
      if (i < attempts) await new Promise((r) => setTimeout(r, 5_000 * i));
    }
  }
  throw lastErr;
}

/**
 * Fetches, verifies and lays out the sidecar for `target`. Everything is
 * staged in a temp dir and moved into place only after every check passed,
 * so a failure never leaves a half-written or unverified sidecar behind.
 *
 * `fetchAsset(url)` and `archivePath` are injectable for tests / offline use.
 */
export async function fetchSidecar({
  lock = readLock(),
  lockDir = dirname(LOCK_PATH),
  target = hostTriple(),
  outDir = OUT_DIR,
  archivePath,
  fetchAsset = download,
  force = false,
  log = console.log,
} = {}) {
  const t = lock.targets[target];
  if (!t) {
    throw new Error(
      `no llama-server pinned for ${target} (pinned: ${Object.keys(lock.targets).join(", ")})`,
    );
  }
  const binName = sidecarFileName(lock, target);
  const binPath = join(outDir, binName);
  const libDir = join(outDir, lock.libDir);
  const stampPath = join(outDir, STAMP);
  const stamp = { name: lock.name, release: lock.release, target, sha256: t.sha256 };

  if (!force && existsSync(stampPath) && existsSync(binPath)) {
    try {
      const prev = JSON.parse(readFileSync(stampPath, "utf8"));
      const complete = t.libs.every((l) => existsSync(join(libDir, l)));
      if (complete && JSON.stringify(prev) === JSON.stringify(stamp)) {
        log(`sidecar up to date: ${binName} (llama.cpp ${lock.release})`);
        return { binPath, libDir, skipped: true };
      }
    } catch {
      /* unreadable stamp: redo */
    }
  }

  mkdirSync(outDir, { recursive: true });
  const cacheDir = join(outDir, ".cache");
  const cached = join(cacheDir, t.asset);
  let buf;
  if (archivePath) {
    buf = readFileSync(archivePath);
    verifySha256(buf, t.sha256, archivePath);
  } else if (existsSync(cached) && sha256(readFileSync(cached)) === t.sha256) {
    buf = readFileSync(cached);
    log(`using cached ${t.asset}`);
  } else {
    log(`downloading ${t.url}`);
    buf = await fetchAsset(t.url);
    verifySha256(buf, t.sha256, t.url); // throws before anything is written
    mkdirSync(cacheDir, { recursive: true });
    writeFileSync(cached, buf);
  }
  if (buf.length !== t.size) {
    // Unreachable with a matching SHA-256; kept as a cheap lock sanity check.
    throw new Error(`${t.asset}: size ${buf.length} != pinned ${t.size}`);
  }

  const entries = readArchive(buf, t.asset);
  const files = [[binName, pickFile(entries, t.root + t.binary), true]];
  for (const lib of t.libs) files.push([join(lock.libDir, lib), pickFile(entries, t.root + lib), true]);

  // Licence texts ship next to the libs and must match the committed copies
  // the About dialog shows; a drift means upstream changed its terms. Some
  // assets (Windows) carry no llama.cpp LICENSE: the committed copy ships.
  const licences = [
    [t.licenseInArchive === false ? null : lock.licenseFile, lock.licenseText, "LICENSE-llama.cpp"],
  ];
  for (const x of t.extraLicenses) licences.push([x.file, x.text, x.file]);
  for (const [inArchive, committed, outName] of licences) {
    const expected = readFileSync(join(lockDir, committed));
    const data = inArchive ? pickFile(entries, t.root + inArchive) : expected;
    // Compare text, not bytes: a Windows checkout with core.autocrlf turns the
    // committed copy's LF into CRLF (the .gitattributes rule prevents that, this
    // keeps the check correct on clones made before it).
    const eol = (b) => b.toString("utf8").replace(/\r\n/g, "\n");
    if (eol(data) !== eol(expected)) {
      throw new Error(
        `${inArchive} in ${t.asset} differs from sidecar/${committed}: review the licence ` +
          "change, update the committed text and run `npm run licenses`",
      );
    }
    files.push([join(lock.libDir, outName), data, false]);
  }

  const staging = join(outDir, `.staging-${process.pid}`);
  rmSync(staging, { recursive: true, force: true });
  try {
    mkdirSync(join(staging, lock.libDir), { recursive: true });
    for (const [rel, data, exec] of files) {
      const p = join(staging, rel);
      writeFileSync(p, data);
      if (exec && process.platform !== "win32") chmodSync(p, 0o755);
    }
    // Swap in: drop the previous sidecar (any target) and its libs.
    rmSync(libDir, { recursive: true, force: true });
    for (const triple of Object.keys(lock.targets)) {
      rmSync(join(outDir, sidecarFileName(lock, triple)), { force: true });
    }
    renameSync(join(staging, lock.libDir), libDir);
    renameSync(join(staging, binName), binPath);
    writeFileSync(stampPath, JSON.stringify(stamp, null, 2) + "\n");
  } finally {
    rmSync(staging, { recursive: true, force: true });
  }
  log(
    `sidecar ready: binaries/${binName} + binaries/${lock.libDir}/ ` +
      `(${t.libs.length} libs, llama.cpp ${lock.release}, ${t.backend}, sha256 verified)`,
  );
  return { binPath, libDir, skipped: false };
}

// --- CLI --------------------------------------------------------------------

function parseArgs(argv) {
  const opts = {};
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--target") opts.target = argv[++i];
    else if (a === "--archive") opts.archivePath = resolve(argv[++i]);
    else if (a === "--force") opts.force = true;
    else throw new Error(`unknown argument ${a}`);
  }
  return opts;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  fetchSidecar(parseArgs(process.argv.slice(2))).catch((e) => {
    console.error(`fetch-sidecar: ${e.message}`);
    process.exit(1);
  });
}

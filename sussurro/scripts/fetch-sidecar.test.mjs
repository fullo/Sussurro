// Tests for the sidecar fetch script (#116): manifest schema, checksum
// verification (fail-closed) and the layout Tauri's externalBin expects —
// with fake assets built here, never the network.
import { describe, expect, it, beforeEach, afterEach } from "vitest";
import { existsSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync, mkdirSync, readdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { deflateRawSync, gzipSync } from "node:zlib";
import {
  ChecksumError,
  LOCK_PATH,
  TAURI_DIR,
  fetchSidecar,
  pickFile,
  readArchive,
  readLock,
  sha256,
  sidecarFileName,
  validateLock,
  verifySha256,
} from "./fetch-sidecar.mjs";

// --- fake archives ----------------------------------------------------------

function tarHeader(name, { size = 0, type = "0", link = "" } = {}) {
  const h = Buffer.alloc(512);
  h.write(name, 0, 100);
  h.write("0000755\0", 100);
  h.write("0000000\0", 108);
  h.write("0000000\0", 116);
  h.write(size.toString(8).padStart(11, "0") + "\0", 124);
  h.write("00000000000\0", 136);
  h.write(type, 156);
  if (link) h.write(link, 157, 100);
  h.write("ustar\0", 257);
  h.write("00", 263);
  h.fill(0x20, 148, 156); // checksum field counts as spaces
  let sum = 0;
  for (const b of h) sum += b;
  h.write(sum.toString(8).padStart(6, "0") + "\0 ", 148);
  return h;
}

/** entries: [name, string|Buffer] for files, [name, {link}] for symlinks. */
function makeTarGz(entries) {
  const parts = [];
  for (const [name, content] of entries) {
    // (not `content.link`: String.prototype.link exists)
    if (typeof content === "object" && !Buffer.isBuffer(content)) {
      parts.push(tarHeader(name, { type: "2", link: content.link }));
      continue;
    }
    const data = Buffer.from(content);
    parts.push(tarHeader(name, { size: data.length }));
    parts.push(data, Buffer.alloc((512 - (data.length % 512)) % 512));
  }
  parts.push(Buffer.alloc(1024));
  return gzipSync(Buffer.concat(parts));
}

function makeZip(entries) {
  const locals = [];
  const centrals = [];
  let offset = 0;
  for (const [name, content] of entries) {
    const data = Buffer.from(content);
    const comp = deflateRawSync(data);
    const nameBuf = Buffer.from(name);
    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x04034b50, 0);
    local.writeUInt16LE(20, 4);
    local.writeUInt16LE(8, 8);
    local.writeUInt32LE(comp.length, 18);
    local.writeUInt32LE(data.length, 22);
    local.writeUInt16LE(nameBuf.length, 26);
    const central = Buffer.alloc(46);
    central.writeUInt32LE(0x02014b50, 0);
    central.writeUInt16LE(20, 4);
    central.writeUInt16LE(20, 6);
    central.writeUInt16LE(8, 10);
    central.writeUInt32LE(comp.length, 20);
    central.writeUInt32LE(data.length, 24);
    central.writeUInt16LE(nameBuf.length, 28);
    central.writeUInt32LE(offset, 42);
    locals.push(local, nameBuf, comp);
    centrals.push(central, nameBuf);
    offset += local.length + nameBuf.length + comp.length;
  }
  const cd = Buffer.concat(centrals);
  const eocd = Buffer.alloc(22);
  eocd.writeUInt32LE(0x06054b50, 0);
  eocd.writeUInt16LE(entries.length, 8);
  eocd.writeUInt16LE(entries.length, 10);
  eocd.writeUInt32LE(cd.length, 12);
  eocd.writeUInt32LE(offset, 16);
  return Buffer.concat([...locals, cd, eocd]);
}

const LICENSE = "MIT License\n\nCopyright (c) ggml authors\n";

function fakeLock(dir, asset, buf, extra = {}) {
  writeFileSync(join(dir, "llama.txt"), LICENSE);
  const zip = asset.endsWith(".zip");
  return validateLock({
    name: "sussurro-llama-server",
    upstream: "https://github.com/ggml-org/llama.cpp",
    release: "b1",
    license: "MIT",
    licenseFile: "LICENSE",
    licenseText: "llama.txt",
    libDir: "llama-server-libs",
    targets: {
      [zip ? "x86_64-pc-windows-msvc" : "x86_64-unknown-linux-gnu"]: {
        backend: "CPU",
        asset,
        url: `https://github.com/ggml-org/llama.cpp/releases/download/b1/${asset}`,
        sha256: sha256(buf),
        size: buf.length,
        root: zip ? "" : "llama-b1/",
        binary: zip ? "llama-server.exe" : "llama-server",
        libs: zip ? ["ggml.dll"] : ["libllama.so.0", "libggml-cpu-x64.so"],
        extraLicenses: [],
        ...extra,
      },
    },
  });
}

const linuxTar = () =>
  makeTarGz([
    ["llama-b1/llama-server", "ELF-server"],
    ["llama-b1/llama-cli", "not shipped"],
    ["llama-b1/libllama.so.0.5.0", "ELF-libllama"],
    ["llama-b1/libllama.so.0", { link: "libllama.so.0.5.0" }],
    ["llama-b1/libggml-cpu-x64.so", "ELF-cpu"],
    ["llama-b1/LICENSE", LICENSE],
  ]);

let dir;
beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "sidecar-test-"));
});
afterEach(() => rmSync(dir, { recursive: true, force: true }));

const quiet = () => {};

// --- the real manifest ------------------------------------------------------

describe("llama-server.lock.json", () => {
  const lock = readLock();

  it("pins the three release targets from one llama.cpp release", () => {
    expect(Object.keys(lock.targets).sort()).toEqual([
      "aarch64-apple-darwin",
      "x86_64-pc-windows-msvc",
      "x86_64-unknown-linux-gnu",
    ]);
    for (const t of Object.values(lock.targets)) {
      expect(t.url).toContain(`/releases/download/${lock.release}/`);
      expect(t.asset).toContain(`llama-${lock.release}-bin-`);
    }
    expect(lock.targets["aarch64-apple-darwin"].backend).toBe("Metal");
    expect(lock.targets["x86_64-pc-windows-msvc"].backend).toBe("Vulkan");
    expect(lock.targets["x86_64-unknown-linux-gnu"].backend).toBe("CPU");
  });

  it("carries the SHA-256s verified in the #109 benchmark", () => {
    expect(lock.release).toBe("b11146");
    expect(lock.targets["aarch64-apple-darwin"].sha256).toBe(
      "1ad3f9eff80edb9dbef4259ad564d1720612ef7eea48fa4afed0e54f5f3d5711",
    );
    expect(lock.targets["x86_64-pc-windows-msvc"].sha256).toBe(
      "55a378aa095b466979d85075234f66d7655c7a7483222af0c006c0e55b4d7bd6",
    );
    expect(lock.targets["x86_64-unknown-linux-gnu"].sha256).toBe(
      "c150306eb16b5ab696f76a8bdf810c35fd98a24e82158742e6fa28f420ff8410",
    );
  });

  it("has the committed licence texts it points at", () => {
    const base = join(TAURI_DIR, "sidecar");
    expect(readFileSync(join(base, lock.licenseText), "utf8")).toMatch(/^MIT License/);
    for (const t of Object.values(lock.targets)) {
      for (const x of t.extraLicenses) expect(existsSync(join(base, x.text))).toBe(true);
    }
  });

  it("matches the Tauri merge config (externalBin name + libs dir)", () => {
    const conf = JSON.parse(readFileSync(join(TAURI_DIR, "tauri.sidecar.conf.json"), "utf8"));
    expect(conf.bundle.externalBin).toEqual([`binaries/${lock.name}`]);
    expect(conf.bundle.resources).toEqual({
      [`binaries/${lock.libDir}/`]: `${lock.libDir}/`,
    });
    // The base config must stay sidecar-free so cargo test/clippy/dev
    // don't need the download (tauri-build checks externalBin at compile time).
    const base = JSON.parse(readFileSync(join(TAURI_DIR, "tauri.conf.json"), "utf8"));
    expect(base.bundle.externalBin).toBeUndefined();
  });

  it("names the binary the way Tauri's externalBin expects", () => {
    expect(sidecarFileName(lock, "aarch64-apple-darwin")).toBe(
      "sussurro-llama-server-aarch64-apple-darwin",
    );
    expect(sidecarFileName(lock, "x86_64-pc-windows-msvc")).toBe(
      "sussurro-llama-server-x86_64-pc-windows-msvc.exe",
    );
  });

  it("is the file the script reads by default", () => {
    expect(LOCK_PATH.endsWith(join("sidecar", "llama-server.lock.json"))).toBe(true);
  });
});

describe("validateLock", () => {
  const good = () => JSON.parse(readFileSync(LOCK_PATH, "utf8"));

  it("rejects a malformed checksum", () => {
    const lock = good();
    lock.targets["aarch64-apple-darwin"].sha256 = "ABC";
    expect(() => validateLock(lock)).toThrow(/sha256/);
  });

  it("rejects a URL that is not the pinned release asset", () => {
    const lock = good();
    lock.targets["x86_64-unknown-linux-gnu"].url = "https://example.com/llama.tar.gz";
    expect(() => validateLock(lock)).toThrow(/url must be/);
  });

  it("rejects file names that could escape the output directory", () => {
    const lock = good();
    lock.targets["x86_64-unknown-linux-gnu"].libs.push("../../evil.so");
    expect(() => validateLock(lock)).toThrow(/bare file name/);
    const lock2 = good();
    lock2.name = "../x";
    expect(() => validateLock(lock2)).toThrow(/name/);
  });

  it("requires .exe exactly on Windows", () => {
    const lock = good();
    lock.targets["x86_64-pc-windows-msvc"].binary = "llama-server";
    expect(() => validateLock(lock)).toThrow(/\.exe/);
  });
});

// --- checksums --------------------------------------------------------------

describe("verifySha256", () => {
  it("accepts the matching digest and rejects any other", () => {
    const buf = Buffer.from("hello");
    expect(() => verifySha256(buf, sha256(buf), "x")).not.toThrow();
    expect(() => verifySha256(buf, sha256(Buffer.from("hellO")), "x")).toThrow(ChecksumError);
  });
});

// --- archives ---------------------------------------------------------------

describe("archive reader", () => {
  it("reads tar.gz files and follows symlinks inside the archive", () => {
    const entries = readArchive(linuxTar(), "a.tar.gz");
    expect(pickFile(entries, "llama-b1/libllama.so.0").toString()).toBe("ELF-libllama");
    expect(() => pickFile(entries, "llama-b1/missing.so")).toThrow(/no llama-b1\/missing.so/);
  });

  it("refuses symlinks that leave their directory", () => {
    const entries = readArchive(
      makeTarGz([["d/evil", { link: "../../etc/passwd" }]]),
      "a.tar.gz",
    );
    expect(() => pickFile(entries, "d/evil")).toThrow(/leaves its directory/);
  });

  it("reads deflated zip entries", () => {
    const entries = readArchive(makeZip([["llama-server.exe", "MZ-server"]]), "a.zip");
    expect(pickFile(entries, "llama-server.exe").toString()).toBe("MZ-server");
  });

  it("handles names longer than the ustar field (pax path)", () => {
    const long = `${"d/".repeat(60)}file`;
    // a pax record's length counts its own digits
    const tail = ` path=${long}\n`;
    let len = tail.length + 1;
    while (String(len).length + tail.length !== len) len = String(len).length + tail.length;
    const body = Buffer.from(`${len}${tail}`);
    const tar = gzipSync(
      Buffer.concat([
        tarHeader("PaxHeader", { size: body.length, type: "x" }),
        body,
        Buffer.alloc((512 - (body.length % 512)) % 512),
        tarHeader("short", { size: 2 }),
        Buffer.from("ok"),
        Buffer.alloc(510),
        Buffer.alloc(1024),
      ]),
    );
    expect(pickFile(readArchive(tar, "a.tar.gz"), long).toString()).toBe("ok");
  });
});

// --- fetch + layout ---------------------------------------------------------

describe("fetchSidecar", () => {
  it("lays out the verified binary with the target-triple suffix and the libs", async () => {
    const buf = linuxTar();
    const lock = fakeLock(dir, "llama-b1-bin-ubuntu-x64.tar.gz", buf);
    const out = join(dir, "binaries");
    const res = await fetchSidecar({
      lock,
      lockDir: dir,
      target: "x86_64-unknown-linux-gnu",
      outDir: out,
      fetchAsset: async () => buf,
      log: quiet,
    });
    const bin = join(out, "sussurro-llama-server-x86_64-unknown-linux-gnu");
    expect(res.binPath).toBe(bin);
    expect(readFileSync(bin, "utf8")).toBe("ELF-server");
    if (process.platform !== "win32") expect(statSync(bin).mode & 0o111).not.toBe(0);
    expect(readdirSync(join(out, "llama-server-libs")).sort()).toEqual([
      "LICENSE-llama.cpp",
      "libggml-cpu-x64.so",
      "libllama.so.0",
    ]);
    expect(readFileSync(join(out, "llama-server-libs", "libllama.so.0"), "utf8")).toBe("ELF-libllama");
    // nothing that isn't allowlisted (llama-cli) is extracted
    expect(existsSync(join(out, "llama-cli"))).toBe(false);
  });

  it("fails closed on a checksum mismatch and writes nothing", async () => {
    const buf = linuxTar();
    const lock = fakeLock(dir, "llama-b1-bin-ubuntu-x64.tar.gz", buf);
    const tampered = Buffer.from(buf);
    tampered[tampered.length - 30] ^= 0xff;
    const out = join(dir, "binaries");
    await expect(
      fetchSidecar({
        lock,
        lockDir: dir,
        target: "x86_64-unknown-linux-gnu",
        outDir: out,
        fetchAsset: async () => tampered,
        log: quiet,
      }),
    ).rejects.toBeInstanceOf(ChecksumError);
    expect(existsSync(join(out, "sussurro-llama-server-x86_64-unknown-linux-gnu"))).toBe(false);
    expect(existsSync(join(out, "llama-server-libs"))).toBe(false);
    expect(existsSync(join(out, ".cache", "llama-b1-bin-ubuntu-x64.tar.gz"))).toBe(false);
  });

  it("verifies a local --archive too", async () => {
    const buf = linuxTar();
    const lock = fakeLock(dir, "llama-b1-bin-ubuntu-x64.tar.gz", buf);
    const archive = join(dir, "local.tar.gz");
    writeFileSync(archive, Buffer.concat([buf, Buffer.from("x")]));
    await expect(
      fetchSidecar({
        lock,
        lockDir: dir,
        target: "x86_64-unknown-linux-gnu",
        outDir: join(dir, "binaries"),
        archivePath: archive,
        fetchAsset: async () => {
          throw new Error("must not download");
        },
        log: quiet,
      }),
    ).rejects.toBeInstanceOf(ChecksumError);
  });

  it("keeps a previous good sidecar when a new fetch fails", async () => {
    const buf = linuxTar();
    const lock = fakeLock(dir, "llama-b1-bin-ubuntu-x64.tar.gz", buf);
    const out = join(dir, "binaries");
    const opts = { lock, lockDir: dir, target: "x86_64-unknown-linux-gnu", outDir: out, log: quiet };
    await fetchSidecar({ ...opts, fetchAsset: async () => buf });
    rmSync(join(out, ".cache"), { recursive: true });
    await expect(
      fetchSidecar({ ...opts, force: true, fetchAsset: async () => Buffer.from("nope") }),
    ).rejects.toBeInstanceOf(ChecksumError);
    expect(readFileSync(join(out, "sussurro-llama-server-x86_64-unknown-linux-gnu"), "utf8")).toBe(
      "ELF-server",
    );
  });

  it("is a no-op when already up to date, and uses the verified cache", async () => {
    const buf = linuxTar();
    const lock = fakeLock(dir, "llama-b1-bin-ubuntu-x64.tar.gz", buf);
    const out = join(dir, "binaries");
    let downloads = 0;
    const opts = {
      lock,
      lockDir: dir,
      target: "x86_64-unknown-linux-gnu",
      outDir: out,
      log: quiet,
      fetchAsset: async () => {
        downloads++;
        return buf;
      },
    };
    await fetchSidecar(opts);
    expect((await fetchSidecar(opts)).skipped).toBe(true);
    expect((await fetchSidecar({ ...opts, force: true })).skipped).toBe(false);
    expect(downloads).toBe(1);
  });

  it("refuses an archive whose licence differs from the committed text", async () => {
    const buf = makeTarGz([
      ["llama-b1/llama-server", "ELF"],
      ["llama-b1/libllama.so.0", "ELF"],
      ["llama-b1/libggml-cpu-x64.so", "ELF"],
      ["llama-b1/LICENSE", "Some other licence\n"],
    ]);
    const lock = fakeLock(dir, "llama-b1-bin-ubuntu-x64.tar.gz", buf);
    await expect(
      fetchSidecar({
        lock,
        lockDir: dir,
        target: "x86_64-unknown-linux-gnu",
        outDir: join(dir, "binaries"),
        fetchAsset: async () => buf,
        log: quiet,
      }),
    ).rejects.toThrow(/differs from sidecar/);
  });

  it("accepts a committed licence checked out with CRLF line endings (Windows autocrlf)", async () => {
    const buf = makeTarGz([
      ["llama-b1/llama-server", "ELF"],
      ["llama-b1/libllama.so.0", "ELF"],
      ["llama-b1/libggml-cpu-x64.so", "ELF"],
      ["llama-b1/LICENSE", LICENSE],
    ]);
    const lock = fakeLock(dir, "llama-b1-bin-ubuntu-x64.tar.gz", buf);
    writeFileSync(join(dir, "llama.txt"), LICENSE.replace(/\n/g, "\r\n"));
    const res = await fetchSidecar({
      lock,
      lockDir: dir,
      target: "x86_64-unknown-linux-gnu",
      outDir: join(dir, "binaries"),
      fetchAsset: async () => buf,
      log: quiet,
    });
    expect(res.skipped).toBe(false);
  });

  it("handles the Windows zip layout (.exe suffix, files at the root)", async () => {
    // like the real Windows asset: no llama.cpp LICENSE inside, the
    // committed copy ships instead
    const buf = makeZip([
      ["llama-server.exe", "MZ"],
      ["ggml.dll", "MZ-ggml"],
    ]);
    const lock = fakeLock(dir, "llama-b1-bin-win-vulkan-x64.zip", buf, { licenseInArchive: false });
    const out = join(dir, "binaries");
    await fetchSidecar({
      lock,
      lockDir: dir,
      target: "x86_64-pc-windows-msvc",
      outDir: out,
      fetchAsset: async () => buf,
      log: quiet,
    });
    expect(existsSync(join(out, "sussurro-llama-server-x86_64-pc-windows-msvc.exe"))).toBe(true);
    expect(readFileSync(join(out, "llama-server-libs", "ggml.dll"), "utf8")).toBe("MZ-ggml");
    expect(readFileSync(join(out, "llama-server-libs", "LICENSE-llama.cpp"), "utf8")).toBe(LICENSE);
  });

  it("errors clearly for a target that is not pinned", async () => {
    const buf = linuxTar();
    const lock = fakeLock(dir, "llama-b1-bin-ubuntu-x64.tar.gz", buf);
    mkdirSync(join(dir, "binaries"));
    await expect(
      fetchSidecar({ lock, lockDir: dir, target: "x86_64-apple-darwin", outDir: join(dir, "binaries"), log: quiet }),
    ).rejects.toThrow(/no llama-server pinned for x86_64-apple-darwin/);
  });
});

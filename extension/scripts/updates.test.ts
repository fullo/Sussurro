import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { strToU8, zipSync } from "fflate";
import {
  GECKO_ID,
  UPDATE_URL,
  addUpdate,
  checkVersion,
  compareVersions,
  emptyManifest,
  entryForXpi,
  readXpi,
  updateHash,
  updateLink,
  validateManifest,
  type UpdateEntry,
} from "./updates";

const repoFile = (p: string) => fileURLToPath(new URL(`../../${p}`, import.meta.url));

/** A minimal .xpi: a zip with a manifest.json. */
const xpi = (manifest: Record<string, unknown>) => zipSync({ "manifest.json": strToU8(JSON.stringify(manifest)), "background.js": strToU8("0") });
const firefoxXpi = (version: string, id = GECKO_ID) =>
  xpi({ manifest_version: 3, version, browser_specific_settings: { gecko: { id, strict_min_version: "128.0" } } });

const entry = (version: string): UpdateEntry => ({ version, update_link: updateLink(version), update_hash: `sha256:${"a".repeat(64)}` });

describe("the published files", () => {
  it("docs/extension/updates.json is a valid update manifest for the add-on", () => {
    const m = validateManifest(JSON.parse(readFileSync(repoFile("docs/extension/updates.json"), "utf8")));
    expect(Object.keys(m.addons)).toEqual([GECKO_ID]);
  });

  it("the Firefox manifest points at it, under the stable gecko id", () => {
    const gecko = JSON.parse(readFileSync(repoFile("extension/manifest.firefox.json"), "utf8")).browser_specific_settings.gecko;
    expect(gecko.id).toBe(GECKO_ID);
    expect(gecko.update_url).toBe(UPDATE_URL);
    // GitHub Pages serves docs/ from main at https://fullo.github.io/Sussurro/.
    expect(UPDATE_URL).toBe("https://fullo.github.io/Sussurro/extension/updates.json");
  });
});

describe("links, hashes, versions", () => {
  it("links to the signed .xpi of the GitHub release", () => {
    expect(updateLink("0.10.1")).toBe("https://github.com/fullo/Sussurro/releases/download/v0.10.1/sussurro-extension-firefox-0.10.1.xpi");
  });

  it("hashes with SHA-256 in Firefox's format", () => {
    expect(updateHash(strToU8("abc"))).toBe("sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
  });

  it("accepts X.Y.Z (and a leading v) only", () => {
    expect(checkVersion(" v0.10.1 ")).toBe("0.10.1");
    for (const bad of ["0.10", "0.10.1-rc.1", "0.010.1", "1.2.3.4", ""]) expect(() => checkVersion(bad)).toThrow(/X\.Y\.Z/);
  });

  it("orders versions numerically", () => {
    expect(compareVersions("0.10.0", "0.9.9")).toBeGreaterThan(0);
    expect(compareVersions("1.0.0", "1.0.0")).toBe(0);
    expect(compareVersions("0.9.0", "0.9.1")).toBeLessThan(0);
  });
});

describe("validateManifest", () => {
  it("accepts the empty manifest", () => {
    expect(validateManifest(emptyManifest())).toEqual({ addons: { [GECKO_ID]: { updates: [] } } });
  });

  it("rejects a wrong shape, another id, duplicates and bad entries", () => {
    expect(() => validateManifest({})).toThrow(/addons/);
    expect(() => validateManifest(emptyManifest("other@example.com"))).toThrow(/addons/);
    const withUpdates = (updates: unknown[]) => ({ addons: { [GECKO_ID]: { updates } } });
    expect(() => validateManifest(withUpdates([entry("0.10.0"), entry("0.10.0")]))).toThrow(/twice/);
    expect(() => validateManifest(withUpdates([{ ...entry("0.10.0"), update_link: "http://x/y.xpi" }]))).toThrow(/https/);
    expect(() => validateManifest(withUpdates([{ ...entry("0.10.0"), update_hash: "sha1:00" }]))).toThrow(/sha256/);
    expect(() => validateManifest(withUpdates([{ ...entry("0.10.0"), version: "latest" }]))).toThrow(/X\.Y\.Z/);
  });
});

describe("addUpdate", () => {
  it("adds entries in version order without touching the input", () => {
    const empty = emptyManifest();
    const one = addUpdate(empty, entry("0.10.0"));
    const two = addUpdate(one, entry("0.9.0"));
    const three = addUpdate(two, entry("0.10.1"));
    expect(empty.addons[GECKO_ID].updates).toEqual([]);
    expect(three.addons[GECKO_ID].updates.map((u) => u.version)).toEqual(["0.9.0", "0.10.0", "0.10.1"]);
  });

  it("replaces an entry for the same version (re-running is idempotent)", () => {
    const first = addUpdate(emptyManifest(), entry("0.10.1"));
    const redo = addUpdate(first, { ...entry("0.10.1"), update_hash: `sha256:${"b".repeat(64)}` });
    expect(redo.addons[GECKO_ID].updates).toHaveLength(1);
    expect(redo.addons[GECKO_ID].updates[0].update_hash).toBe(`sha256:${"b".repeat(64)}`);
    expect(addUpdate(redo, redo.addons[GECKO_ID].updates[0])).toEqual(redo);
  });

  it("keeps other add-ons' entries", () => {
    const m = { addons: { ...emptyManifest().addons, "other@example.com": { updates: [] } } };
    expect(Object.keys(addUpdate(m, entry("0.10.1")).addons).sort()).toEqual([GECKO_ID, "other@example.com"].sort());
  });
});

describe("readXpi / entryForXpi", () => {
  it("reads the version, id and minimum Firefox from the packaged manifest", () => {
    expect(readXpi(firefoxXpi("0.10.1"))).toEqual({ version: "0.10.1", id: GECKO_ID, strict_min_version: "128.0" });
  });

  it("builds the entry with the hash of the exact bytes", () => {
    const bytes = firefoxXpi("0.10.1");
    expect(entryForXpi(bytes, "v0.10.1")).toEqual({
      version: "0.10.1",
      update_link: updateLink("0.10.1"),
      update_hash: updateHash(bytes),
      browser_specific_settings: { gecko: { strict_min_version: "128.0" } },
    });
  });

  it("refuses a mismatched version or id, and non-xpi files", () => {
    expect(() => entryForXpi(firefoxXpi("0.10.0"), "0.10.1")).toThrow(/version 0\.10\.0, not 0\.10\.1/);
    expect(() => entryForXpi(firefoxXpi("0.10.1", "other@example.com"), "0.10.1")).toThrow(/gecko id/);
    expect(() => readXpi(strToU8("<html>404</html>"))).toThrow(/not a zip/);
    expect(() => readXpi(zipSync({ "a.txt": strToU8("x") }))).toThrow(/no manifest/);
  });
});

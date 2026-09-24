import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import generated from "./licenses.json";
import { licenseEntries, repositoryUrl } from "./licenses";

const read = (p: string) => JSON.parse(readFileSync(new URL(p, import.meta.url), "utf8"));
const lock = read("../../package-lock.json") as {
  packages: Record<string, { version?: string; dev?: boolean; devOptional?: boolean; link?: boolean }>;
};
const pkg = read("../../package.json") as { dependencies: Record<string, string>; devDependencies: Record<string, string> };

/** What the extension ships: every locked package that is not dev-only. */
function shippedFromLock(): string[] {
  return Object.entries(lock.packages)
    .filter(([path, p]) => path && !p.dev && !p.devOptional && !p.link)
    .map(([path, p]) => `${path.slice(path.lastIndexOf("node_modules/") + "node_modules/".length)}@${p.version}`)
    .sort();
}

describe("third-party licences (#138)", () => {
  it("list exactly the production dependencies in package-lock.json — run `npm run licenses` after changing them", () => {
    const listed = generated.packages.map((p) => `${p.name}@${p.version}`).sort();
    expect(listed).toEqual(shippedFromLock());
  });

  it("never list a dev dependency", () => {
    const names = new Set(generated.packages.map((p) => p.name));
    for (const dev of Object.keys(pkg.devDependencies)) {
      if (!(dev in pkg.dependencies)) expect(names.has(dev), dev).toBe(false);
    }
    for (const dep of Object.keys(pkg.dependencies)) expect(names.has(dep), dep).toBe(true);
  });

  it("carry a licence and its text for every package", () => {
    const entries = licenseEntries();
    expect(entries.length).toBeGreaterThan(0);
    for (const e of entries) {
      expect(e.license, e.name).not.toBe("(unspecified)");
      expect(e.text.length, e.name).toBeGreaterThan(100);
    }
    expect(entries.map((e) => e.name)).toEqual([...entries.map((e) => e.name)].sort((a, b) => a.localeCompare(b)));
  });

  it("resolve shared texts and missing ones", () => {
    const entries = licenseEntries({
      packages: [
        { name: "b", version: "1", license: "MIT", repository: "", textId: 0 },
        { name: "a", version: "2", license: "MIT", repository: "", textId: -1 },
      ],
      texts: ["MIT text"],
    });
    expect(entries.map((e) => [e.name, e.text])).toEqual([
      ["a", ""],
      ["b", "MIT text"],
    ]);
  });

  it("link only to http(s) repositories", () => {
    expect(repositoryUrl("git+https://github.com/mozilla/webextension-polyfill.git")).toBe("https://github.com/mozilla/webextension-polyfill");
    expect(repositoryUrl("https://github.com/react/react.git")).toBe("https://github.com/react/react");
    expect(repositoryUrl("github:foo/bar")).toBe("");
    expect(repositoryUrl("javascript:alert(1)")).toBe("");
    expect(repositoryUrl("")).toBe("");
  });
});

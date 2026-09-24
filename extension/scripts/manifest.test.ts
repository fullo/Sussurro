import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { buildManifest, referencedFiles, toManifestVersion, zipName, type Manifest, type Target } from "./manifest";
import { APP_MATCH, MEETING_MATCHES, detectPlatform } from "../src/shared/platform";

const template = (t: Target): Manifest =>
  JSON.parse(readFileSync(fileURLToPath(new URL(`../manifest.${t}.json`, import.meta.url)), "utf8"));

describe("toManifestVersion", () => {
  it("keeps a plain X.Y.Z", () => {
    expect(toManifestVersion("0.9.0")).toEqual({ version: "0.9.0" });
    expect(toManifestVersion(" 1.2.30 ")).toEqual({ version: "1.2.30" });
  });

  it("moves a pre-release or build suffix into version_name", () => {
    expect(toManifestVersion("0.9.0-beta.1")).toEqual({ version: "0.9.0", version_name: "0.9.0-beta.1" });
    expect(toManifestVersion("0.9.0+abc")).toEqual({ version: "0.9.0", version_name: "0.9.0+abc" });
  });

  it("rejects what browsers would refuse", () => {
    expect(() => toManifestVersion("0.9")).toThrow(/semver/);
    expect(() => toManifestVersion("v0.9.0")).toThrow(/semver/);
    expect(() => toManifestVersion("0.09.0")).toThrow(/leading zero/);
    expect(() => toManifestVersion("0.70000.0")).toThrow(/65535/);
  });
});

describe("buildManifest", () => {
  it("stamps the app version and leaves the template untouched", () => {
    const t = template("firefox");
    const m = buildManifest(t, "firefox", "0.9.1");
    expect(m.version).toBe("0.9.1");
    expect(t.version).toBe("0.0.0");
    expect(m.browser_specific_settings).toEqual(t.browser_specific_settings);
  });

  it("adds version_name for pre-releases on Chrome only", () => {
    expect(buildManifest(template("chrome"), "chrome", "0.9.0-rc.2").version_name).toBe("0.9.0-rc.2");
    expect(buildManifest(template("firefox"), "firefox", "0.9.0-rc.2")).not.toHaveProperty("version_name");
    expect(buildManifest({ version_name: "stale" }, "chrome", "0.9.0")).not.toHaveProperty("version_name");
  });
});

describe("referencedFiles", () => {
  it("lists scripts, pages and icons of each manifest", () => {
    const common = ["content-isolated.js", "content-main.js", "icons/128.png", "icons/32.png", "icons/64.png", "options.html", "sidepanel.html", "background.js"];
    expect(referencedFiles(template("chrome"))).toEqual([...common].sort());
    expect(referencedFiles(template("firefox"))).toEqual([...common].sort());
  });

  it("tolerates missing or odd keys", () => {
    expect(referencedFiles({})).toEqual([]);
    expect(referencedFiles({ icons: "a.png", background: { scripts: ["b.js", 3] }, content_scripts: [null] })).toEqual(["a.png", "b.js"]);
  });
});

it("zipName carries the target and the app version", () => {
  expect(zipName("chrome", "0.9.0")).toBe("sussurro-extension-chrome-0.9.0.zip");
});

describe.each(["chrome", "firefox"] as const)("manifest.%s.json", (t) => {
  const m = template(t);

  it("is MV3 with minimal permissions and no broad host access", () => {
    expect(m.manifest_version).toBe(3);
    // Chrome adds only what the tab-capture fallback needs (#128): the
    // `tabCapture` stream id and the offscreen document that opens it.
    expect(m.permissions).toEqual(t === "chrome" ? ["storage", "sidePanel", "tabCapture", "offscreen"] : ["storage"]);
    expect(m.host_permissions).toEqual([...MEETING_MATCHES, APP_MATCH]);
    expect(JSON.stringify(m)).not.toMatch(/<all_urls>|\*:\/\/\*\/|https?:\/\/\*\//);
  });

  it("injects a MAIN-world and an ISOLATED-world script on the meeting pages only", () => {
    const cs = m.content_scripts as { matches: string[]; world?: string; js: string[] }[];
    expect(cs.map((c) => c.world ?? "ISOLATED")).toEqual(["MAIN", "ISOLATED"]);
    for (const c of cs) expect(c.matches).toEqual(MEETING_MATCHES);
  });

  it("matches what detectPlatform recognises", () => {
    // One sample per match pattern: the manifest and the runtime check agree.
    const samples = ["https://meet.google.com/a", "https://teams.microsoft.com/a", "https://teams.live.com/a", "https://app.zoom.us/wc/1"];
    expect(samples.map(detectPlatform)).toEqual(["meet", "teams", "teams", "zoom"]);
    expect(MEETING_MATCHES).toHaveLength(samples.length);
  });

  if (t === "firefox") {
    it("pins a Gecko id and Firefox >= 128 (MAIN-world content scripts)", () => {
      const gecko = (m.browser_specific_settings as { gecko: { id: string; strict_min_version: string } }).gecko;
      expect(gecko.id).toBe("sussurro@darumahq.it");
      expect(parseInt(gecko.strict_min_version, 10)).toBeGreaterThanOrEqual(128);
      expect(m.background).toEqual({ scripts: ["background.js"] });
    });

    it("sets its own extension CSP: Firefox's MV3 default upgrades ws://127.0.0.1 (spike #104)", () => {
      const csp = (m.content_security_policy as { extension_pages: string }).extension_pages;
      expect(csp).toBe("script-src 'self'");
      expect(csp).not.toMatch(/upgrade-insecure-requests/);
    });
  } else {
    it("needs Chrome >= 116 (side panel on the toolbar button) and uses a service worker", () => {
      expect(parseInt(m.minimum_chrome_version as string, 10)).toBeGreaterThanOrEqual(116);
      expect(m.background).toEqual({ service_worker: "background.js" });
    });

    it("asks for structured-clone messaging (binary audio to the background, Chrome >= 148)", () => {
      expect(m.message_serialization).toBe("structured_clone");
    });
  }
});

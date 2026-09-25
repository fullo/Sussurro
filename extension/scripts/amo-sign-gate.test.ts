/* Dry runs of the release workflow's signing condition (#228): the gate
   script is run exactly as the workflow step runs it, with the ref and
   secrets a tag push, a build-only dispatch or a fork would have. */
import { describe, expect, it } from "vitest";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const SCRIPT = fileURLToPath(new URL("./amo-sign-gate.sh", import.meta.url));

function gate(env: Record<string, string>): { sign?: string; version?: string; stdout: string } {
  const dir = mkdtempSync(join(tmpdir(), "amo-gate-"));
  const out = join(dir, "output");
  try {
    const clean = { ...process.env };
    delete clean.AMO_JWT_ISSUER;
    delete clean.AMO_JWT_SECRET;
    delete clean.APP_VERSION;
    const r = spawnSync("bash", [SCRIPT], { env: { ...clean, GITHUB_OUTPUT: out, ...env }, encoding: "utf8" });
    expect(r.status, r.stderr).toBe(0);
    const outputs = Object.fromEntries(
      readFileSync(out, "utf8")
        .trim()
        .split("\n")
        .map((l) => l.split("=", 2) as [string, string]),
    );
    return { ...outputs, stdout: r.stdout };
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

const secrets = { AMO_JWT_ISSUER: "user:1:2", AMO_JWT_SECRET: "s3cret" };
const tag = { GITHUB_REF: "refs/tags/v0.10.1", APP_VERSION: "0.10.1" };

describe.skipIf(process.platform === "win32")("amo-sign-gate.sh", () => {
  it("signs a tag run with both secrets", () => {
    const r = gate({ ...tag, ...secrets });
    expect(r.sign).toBe("true");
    expect(r.version).toBe("0.10.1");
    expect(r.stdout).not.toContain("::notice");
  });

  it("never signs a build-only (branch) run, even with the secrets", () => {
    const r = gate({ GITHUB_REF: "refs/heads/main", APP_VERSION: "0.10.1", ...secrets });
    expect(r.sign).toBe("false");
    expect(r.stdout).toContain("::notice title=Firefox signing skipped::Not a tag run (refs/heads/main)");
  });

  it("skips a tag run with no or partial secrets, with a notice and exit 0", () => {
    const partial: Record<string, string>[] = [{}, { AMO_JWT_ISSUER: "user:1:2" }, { AMO_JWT_SECRET: "s3cret" }, { AMO_JWT_ISSUER: "", AMO_JWT_SECRET: "" }];
    for (const env of partial) {
      const r = gate({ ...tag, ...env });
      expect(r.sign).toBe("false");
      expect(r.stdout).toContain("AMO_JWT_ISSUER / AMO_JWT_SECRET secrets are not set");
    }
  });

  it("skips pre-release versions", () => {
    const r = gate({ GITHUB_REF: "refs/tags/v0.10.1-rc.1", APP_VERSION: "0.10.1-rc.1", ...secrets });
    expect(r.sign).toBe("false");
    expect(r.stdout).toContain("not a plain X.Y.Z");
  });

  it("reads the app version from sussurro/package.json by default", () => {
    const app = JSON.parse(readFileSync(fileURLToPath(new URL("../../sussurro/package.json", import.meta.url)), "utf8"));
    expect(gate({ GITHUB_REF: "refs/heads/main" }).version).toBe(app.version);
  });
});

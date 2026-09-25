import { describe, expect, it } from "vitest";
import {
  LOCAL_SERVERS,
  SETUP_STEPS,
  WHATS_NEW,
  bundledSetupState,
  isMac,
  onboardingMode,
  permissionRows,
  pickCleanupModel,
  probeProfile,
  profileForServer,
  stepAfter,
  stepBefore,
  syncedFolder,
  withCleanupServer,
} from "./onboarding";
import type { LlmProfile, Settings } from "./types";

const local: LlmProfile = {
  id: "local",
  name: "Local",
  api: "ollama",
  base_url: "http://localhost:11434",
  api_key: "",
  model: "llama3.2:3b",
  external: false,
};
const lmStudio = LOCAL_SERVERS.find((s) => s.name === "LM Studio")!;
const settings = (profiles: LlmProfile[] = [local]) =>
  ({ llm_profiles: profiles, cleanup_profile: profiles[0]?.id ?? "" }) as unknown as Settings;

describe("onboardingMode", () => {
  it("opens the setup on a fresh install and What's new on an upgrade", () => {
    expect(onboardingMode({ onboarding: "welcome" })).toBe("welcome");
    expect(onboardingMode({ onboarding: "whats_new" })).toBe("whats_new");
    expect(onboardingMode({ onboarding: "done" })).toBeNull();
  });
});

describe("steps", () => {
  it("walks the setup in order and ends after the hotkey", () => {
    expect(SETUP_STEPS[0]).toBe("welcome");
    expect(stepAfter("welcome")).toBe("permissions");
    expect(stepAfter("permissions")).toBe("archive");
    expect(stepAfter("hotkey")).toBeNull();
    expect(stepBefore("welcome")).toBeNull();
    expect(stepBefore("archive")).toBe("permissions");
    // Every step reachable forward from the first.
    const seen: string[] = [];
    for (let s: (typeof SETUP_STEPS)[number] | null = "welcome"; s; s = stepAfter(s)) seen.push(s);
    expect(seen).toEqual([...SETUP_STEPS]);
  });
});

describe("permissionRows", () => {
  it("asks for the microphone with a test when it was never asked (macOS)", () => {
    const rows = permissionRows({ microphone: "unknown", accessibility: "denied" });
    expect(rows.map((r) => [r.id, r.ok, r.action])).toEqual([
      ["microphone", false, "ask"],
      ["accessibility", false, "settings"],
    ]);
  });

  it("sends a denied microphone to the system settings", () => {
    expect(permissionRows({ microphone: "denied", accessibility: "granted" })[0].action).toBe("settings");
  });

  it("shows no Accessibility row where the OS doesn't gate paste", () => {
    const rows = permissionRows({ microphone: "not_applicable", accessibility: "not_applicable" });
    expect(rows).toHaveLength(1);
    expect(rows[0]).toMatchObject({ id: "microphone", ok: true, action: null });
  });

  it("is empty when the check failed", () => {
    expect(permissionRows(null)).toEqual([]);
  });
});

describe("isMac / syncedFolder", () => {
  it("detects macOS from the user agent", () => {
    expect(isMac("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)")).toBe(true);
    expect(isMac("Mozilla/5.0 (Windows NT 10.0; Win64; x64)")).toBe(false);
  });

  it("names the sync service a path visibly belongs to", () => {
    expect(syncedFolder("C:\\Users\\anna\\OneDrive\\Documents\\Sussurro")).toBe("OneDrive");
    expect(syncedFolder("/Users/anna/Library/Mobile Documents/com~apple~CloudDocs/Sussurro")).toBe("iCloud Drive");
    expect(syncedFolder("/Users/anna/Dropbox/Sussurro")).toBe("Dropbox");
    expect(syncedFolder("/Users/anna/Documents/Sussurro")).toBeNull();
    expect(syncedFolder("/home/anna/Documents/Sussurro")).toBeNull();
  });
});

describe("cleanup servers", () => {
  it("probes with an unsaved, local profile", () => {
    const p = probeProfile(lmStudio);
    expect(p).toMatchObject({ id: "", api: "openai", base_url: "http://localhost:1234/v1", external: false, api_key: "" });
  });

  it("finds a profile already on the server, whatever its URL spelling", () => {
    const mine = { ...local, id: "lms", name: "Mine", api: "openai" as const, base_url: "http://127.0.0.1:1234/" };
    expect(profileForServer(settings([local, mine]), lmStudio)?.id).toBe("lms");
    expect(profileForServer(settings([local]), lmStudio)).toBeNull();
  });

  it("prefers a small instruct model", () => {
    expect(pickCleanupModel(["text-embedding-nomic", "qwen2.5-7b-instruct"])).toBe("qwen2.5-7b-instruct");
    expect(pickCleanupModel(["some-model"])).toBe("some-model");
    expect(pickCleanupModel([])).toBe("");
  });

  it("adds a new profile for a server found and selects it for cleanup", () => {
    const next = withCleanupServer(settings(), lmStudio, ["text-embedding-nomic", "gemma-3-4b-it"]);
    expect(next.llm_profiles).toHaveLength(2);
    const added = next.llm_profiles[1];
    expect(added).toMatchObject({ name: "LM Studio", api: "openai", base_url: "http://localhost:1234/v1", model: "gemma-3-4b-it", external: false });
    expect(added.id).not.toBe("");
    expect(next.cleanup_profile).toBe(added.id);
    // The Local profile is untouched.
    expect(next.llm_profiles[0]).toEqual(local);
  });

  it("finds Ollama on the default Local profile, and only local servers are probed", () => {
    const ollama = LOCAL_SERVERS.find((s) => s.api === "ollama")!;
    expect(profileForServer(settings(), ollama)?.id).toBe("local");
    const next = withCleanupServer(settings(), ollama, ["qwen2.5:3b"]);
    expect(next.llm_profiles).toEqual([local]);
    expect(next.cleanup_profile).toBe("local");
    for (const s of LOCAL_SERVERS) expect(s.base_url).toMatch(/^http:\/\/localhost:\d+/);
  });

  it("reuses the existing profile, keeping its model", () => {
    const mine = { ...local, id: "lms", name: "LM Studio", api: "openai" as const, base_url: "http://localhost:1234/v1", model: "my-model" };
    const next = withCleanupServer(settings([local, mine]), lmStudio, ["qwen2.5-7b-instruct"]);
    expect(next.llm_profiles).toHaveLength(2);
    expect(next.cleanup_profile).toBe("lms");
    expect(next.llm_profiles[1].model).toBe("my-model");
  });

  it("gives an existing profile without a model the one found", () => {
    const mine = { ...local, id: "lms", name: "LM Studio", api: "openai" as const, base_url: "http://localhost:1234/v1", model: "" };
    const next = withCleanupServer(settings([local, mine]), lmStudio, ["qwen2.5-7b-instruct"]);
    expect(next.llm_profiles[1].model).toBe("qwen2.5-7b-instruct");
  });

  it("names a new profile apart from an existing one with the same name", () => {
    const other = { ...local, id: "lm-studio", name: "LM Studio", base_url: "http://10.0.0.5:11434" };
    const next = withCleanupServer(settings([local, other]), lmStudio, ["m"]);
    expect(next.llm_profiles[2].name).toBe("LM Studio 2");
    expect(new Set(next.llm_profiles.map((p) => p.id)).size).toBe(3);
  });
});

describe("bundledSetupState (#118)", () => {
  const bundled = { ...local, id: "bundled", name: "Local (bundled)", api: "openai" as const, base_url: "http://127.0.0.1", model: "qwen3-1.7b", bundled: true };
  const on = { available: true };

  it("offers the bundled model only once every probe came back empty", () => {
    expect(bundledSetupState(["absent", "absent", "absent"], on, local)).toBe("offer");
    expect(bundledSetupState(["absent", "checking", "absent"], on, local)).toBeNull();
    expect(bundledSetupState(["found", "absent", "absent"], on, local)).toBeNull();
    expect(bundledSetupState([], on, local)).toBeNull();
  });

  it("needs the sidecar in this build", () => {
    expect(bundledSetupState(["absent", "absent"], { available: false }, local)).toBeNull();
    expect(bundledSetupState(["absent", "absent"], null, local)).toBeNull();
  });

  it("shows it in use when cleanup is already on it, servers or not", () => {
    expect(bundledSetupState(["found", "absent"], on, bundled)).toBe("in_use");
    expect(bundledSetupState(["checking"], null, bundled)).toBe("in_use");
  });
});

describe("WHATS_NEW", () => {
  it("has a few short entries", () => {
    expect(WHATS_NEW.length).toBeGreaterThanOrEqual(3);
    for (const e of WHATS_NEW) expect(e.title && e.text).toBeTruthy();
  });
});

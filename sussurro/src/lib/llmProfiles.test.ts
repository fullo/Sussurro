import { describe, expect, it } from "vitest";
import {
  BUNDLED_ID,
  bundledNote,
  bundledProblem,
  cleanupProfile,
  cleanupServerChanged,
  commitProfile,
  formatGb,
  inferExternal,
  isBundled,
  offerBundled,
  keyStorageBadge,
  keyStorageWarning,
  mergeKeyStorage,
  modelListed,
  newProfile,
  newProfileId,
  patchCleanupProfile,
  patchProfile,
  profileProblems,
  profileSummary,
  removeProfile,
  uniqueName,
  withApi,
  withBaseUrl,
} from "./llmProfiles";
import type { BundledLlmStatus, CredentialStoreStatus, LlmProfile, OllamaStatus, Settings } from "./types";

const local: LlmProfile = {
  id: "local",
  name: "Local",
  api: "ollama",
  base_url: "http://localhost:11434",
  api_key: "",
  model: "llama3.2:3b",
  external: false,
};
const work: LlmProfile = {
  id: "work",
  name: "Work",
  api: "openai",
  base_url: "https://api.example.com/v1",
  api_key: "sk",
  model: "gpt-4o-mini",
  external: true,
};

const settings = (over: Partial<Settings> = {}): Settings =>
  ({ llm_profiles: [local, work], cleanup_profile: "local", ...over }) as Settings;

describe("external inference", () => {
  it("marks off-machine URLs external", () => {
    expect(inferExternal("http://localhost:11434")).toBe(false);
    expect(inferExternal("http://127.0.0.1:8080/v1")).toBe(false);
    expect(inferExternal("http://studio.local:1234")).toBe(false);
    expect(inferExternal("https://api.openai.com/v1")).toBe(true);
    expect(inferExternal("http://192.168.1.5:11434")).toBe(true);
    expect(inferExternal("")).toBe(true);
  });

  it("re-infers on a URL change, replacing a manual override", () => {
    const overridden = { ...work, base_url: "http://192.168.1.5:11434", external: false };
    expect(withBaseUrl(overridden, "http://10.0.0.2:8080").external).toBe(true);
    expect(withBaseUrl(overridden, "http://localhost:8080").external).toBe(false);
  });

  it("patchProfile keeps an override unless the URL changes", () => {
    const overridden = { ...work, external: false };
    expect(patchProfile(overridden, { model: "x" }).external).toBe(false);
    expect(patchProfile(overridden, { base_url: work.base_url }).external).toBe(false);
    expect(patchProfile(overridden, { base_url: "https://other.example.com" }).external).toBe(true);
    expect(patchProfile(overridden, { base_url: "https://other.example.com", external: false }).external).toBe(false);
  });
});

describe("cleanupProfile", () => {
  it("returns the selected profile, else the first, else null", () => {
    expect(cleanupProfile(settings({ cleanup_profile: "work" }))).toBe(work);
    expect(cleanupProfile(settings({ cleanup_profile: "gone" }))).toBe(local);
    expect(cleanupProfile(settings({ llm_profiles: [] }))).toBeNull();
  });

  it("detects a change of cleanup server", () => {
    const s = settings();
    expect(cleanupServerChanged(s, s)).toBe(false);
    expect(cleanupServerChanged(null, s)).toBe(true);
    expect(cleanupServerChanged(s, settings({ cleanup_profile: "work" }))).toBe(true);
    expect(cleanupServerChanged(s, patchCleanupProfile(s, { model: "qwen" }))).toBe(false);
    expect(cleanupServerChanged(s, patchCleanupProfile(s, { base_url: "http://localhost:1" }))).toBe(true);
    expect(cleanupServerChanged(s, patchCleanupProfile(s, { api_key: "k" }))).toBe(true);
  });
});

describe("withApi", () => {
  it("moves a suggested URL to the other API's suggestion, keeps a custom one", () => {
    const p = newProfile([], "openai");
    expect(withApi(p, "ollama").base_url).toBe("http://localhost:11434");
    expect(withApi({ ...p, base_url: "" }, "ollama").base_url).toBe("http://localhost:11434");
    expect(withApi(work, "ollama").base_url).toBe(work.base_url);
    expect(withApi(work, "ollama").api).toBe("ollama");
    expect(withApi(work, "openai")).toBe(work);
  });
});

describe("new profiles", () => {
  it("makes unique ids and names", () => {
    expect(newProfileId([local, work], "Work")).toBe("work-2");
    expect(newProfileId([local], "LM Studio (Mac)")).toBe("lm-studio-mac");
    expect(newProfileId([], "Città")).toBe("citta");
    expect(newProfileId([], "!!!")).toBe("profile");
    expect(uniqueName([local], "local")).toBe("local 2");
    expect(uniqueName([local], "Other")).toBe("Other");
  });

  it("starts blank on a local OpenAI-compatible server", () => {
    const p = newProfile([local]);
    expect(p).toMatchObject({ id: "", name: "New profile", api: "openai", model: "", external: false });
  });

  it("lists what blocks a save", () => {
    expect(profileProblems(local, [local, work])).toEqual([]);
    expect(profileProblems({ ...newProfile([]), name: " work " }, [local, work])).toEqual([
      "Another profile is already called “work”.",
      "Choose a model.",
    ]);
    expect(profileProblems({ ...local, name: "", base_url: " " }, [local])).toEqual([
      "Give the profile a name.",
      "Enter the server address.",
    ]);
  });
});

describe("commit / remove", () => {
  it("appends a new profile with an id from its name", () => {
    const { settings: s, profile } = commitProfile(settings(), { ...newProfile([]), name: " LM Studio ", model: " m " });
    expect(profile.id).toBe("lm-studio");
    expect(profile.name).toBe("LM Studio");
    expect(profile.model).toBe("m");
    expect(s.llm_profiles.map((p) => p.id)).toEqual(["local", "work", "lm-studio"]);
  });

  it("replaces an existing profile in place", () => {
    const { settings: s } = commitProfile(settings(), { ...work, model: "gpt-5" });
    expect(s.llm_profiles.map((p) => p.id)).toEqual(["local", "work"]);
    expect(s.llm_profiles[1].model).toBe("gpt-5");
  });

  it("never removes the last profile and moves cleanup off a deleted one", () => {
    expect(removeProfile(settings({ llm_profiles: [local] }), "local")).toBeNull();
    expect(removeProfile(settings(), "missing")).toBeNull();
    const s = removeProfile(settings({ cleanup_profile: "work" }), "work")!;
    expect(s.llm_profiles).toEqual([local]);
    expect(s.cleanup_profile).toBe("local");
    expect(removeProfile(settings(), "work")!.cleanup_profile).toBe("local");
  });

  it("patches only the cleanup profile", () => {
    const s = patchCleanupProfile(settings({ cleanup_profile: "work" }), { base_url: "http://localhost:8080" });
    expect(s.llm_profiles[0]).toBe(local);
    expect(s.llm_profiles[1]).toMatchObject({ base_url: "http://localhost:8080", external: false });
  });
});

describe("labels", () => {
  it("summarises local and external profiles", () => {
    expect(profileSummary(local)).toBe("Ollama · llama3.2:3b");
    expect(profileSummary(work)).toBe("api.example.com · gpt-4o-mini");
    expect(profileSummary({ ...local, api: "openai", model: "" })).toBe("OpenAI-compatible");
  });

  it("matches Ollama model tags", () => {
    expect(modelListed(["llama3.2:latest"], "llama3.2")).toBe(true);
    expect(modelListed(["llama3.2:3b"], "llama3.2:3b")).toBe(true);
    expect(modelListed(["qwen2.5:3b"], "llama3.2")).toBe(false);
  });
});

describe("API keys in the credential store (#159)", () => {
  const ok: CredentialStoreStatus = { available: true, name: "the macOS Keychain", error: "" };
  const none: CredentialStoreStatus = { available: false, name: "the Secret Service keyring", error: "no D-Bus session" };
  const inStore: LlmProfile = { ...work, api_key_storage: "keychain" };
  const inFile: LlmProfile = { ...work, api_key_storage: "file" };

  it("says nothing when the key is, or will be, in the store", () => {
    expect(keyStorageWarning(inStore, inStore, ok)).toBeNull();
    expect(keyStorageWarning(inStore, inStore, none)).toBeNull();
    expect(keyStorageWarning({ ...inStore, api_key: "sk-new" }, inStore, ok)).toBeNull();
    expect(keyStorageWarning(newProfile([]), null, none)).toBeNull(); // no key typed
    expect(keyStorageWarning({ ...work, api_key: "sk" }, null, null)).toBeNull(); // status unknown yet
  });

  it("warns when a new or changed key would land in the settings file", () => {
    const w = keyStorageWarning({ ...inStore, api_key: "sk-new" }, inStore, none);
    expect(w).toContain("clear text");
    expect(w).toContain("no D-Bus session");
    expect(keyStorageWarning({ ...newProfile([]), api_key: "sk" }, null, none)).toContain("clear text");
  });

  it("warns about a saved fallback key and an unreadable one", () => {
    expect(keyStorageWarning(inFile, inFile, ok)).toContain("clear text");
    const locked: LlmProfile = { ...work, api_key: "", api_key_storage: "unreadable" };
    expect(keyStorageWarning(locked, locked, ok)).toContain("the macOS Keychain");
    // Typing a new key replaces the unreadable one: no stale warning.
    expect(keyStorageWarning({ ...locked, api_key: "sk-new" }, locked, ok)).toBeNull();
  });

  it("badges the profile list", () => {
    expect(keyStorageBadge(inStore)).toBeNull();
    expect(keyStorageBadge(local)).toBeNull();
    expect(keyStorageBadge(inFile)).toBe("Key in settings file");
    expect(keyStorageBadge({ ...work, api_key: "", api_key_storage: "unreadable" })).toBe("Key unreadable");
  });

  it("merges only the storage the backend reports, for unchanged keys", () => {
    const current = settings({ hotkey: "Alt+Space" });
    const saved = settings({ llm_profiles: [local, inStore] });
    const merged = mergeKeyStorage(current, saved);
    expect(merged.llm_profiles[1].api_key_storage).toBe("keychain");
    expect(merged.hotkey).toBe("Alt+Space");
    // The user typed another key meanwhile: that profile is left alone.
    const retyped = settings({ llm_profiles: [local, { ...work, api_key: "sk-typing" }] });
    expect(mergeKeyStorage(retyped, saved)).toBe(retyped);
    // Nothing to change: same object.
    expect(mergeKeyStorage(merged, saved)).toBe(merged);
  });
});

describe("the bundled profile (#118)", () => {
  const bundled: LlmProfile = {
    id: BUNDLED_ID,
    name: "Local (bundled)",
    api: "openai",
    base_url: "http://127.0.0.1",
    api_key: "",
    model: "qwen3-1.7b",
    external: false,
    context_tokens: 8192,
    bundled: true,
  };
  const status = (over: Partial<BundledLlmStatus> = {}): BundledLlmStatus => ({
    available: true,
    downloaded: false,
    running: false,
    model: "Qwen3 1.7B",
    download_bytes: 2_165_039_200,
    ...over,
  });
  const down: OllamaStatus = { installed: false, running: false, has_model: false };
  const up: OllamaStatus = { installed: true, running: true, has_model: true };

  it("is recognised by its flag only", () => {
    expect(isBundled(bundled)).toBe(true);
    expect(isBundled({ ...local, id: BUNDLED_ID })).toBe(false);
    expect(isBundled(null)).toBe(false);
  });

  it("reads as built in and local", () => {
    expect(profileSummary(bundled)).toBe("Built into Sussurro · qwen3-1.7b · no setup needed");
    expect(inferExternal(bundled.base_url)).toBe(false);
    expect(formatGb(2_165_039_200)).toBe("2.2 GB");
    expect(bundledNote(status())).toContain("No setup needed");
    expect(bundledNote(status())).toContain("2.2 GB");
  });

  it("can't be deleted, and its id is never given to a new profile", () => {
    const s = settings({ llm_profiles: [local, bundled] });
    expect(removeProfile(s, BUNDLED_ID)).toBeNull();
    // Deleting the other one leaves the built-in.
    expect(removeProfile(s, "local")?.llm_profiles).toEqual([bundled]);
    expect(newProfileId([local], "Bundled")).toBe("bundled-2");
    expect(commitProfile(settings(), { ...newProfile([]), name: "Bundled", model: "m" }).profile.id).toBe("bundled-2");
  });

  it("is offered only when the cleanup server is unreachable and the build has it", () => {
    const s = settings({ llm_profiles: [local, bundled] });
    expect(offerBundled(s, status(), down)).toBe(true);
    expect(offerBundled(s, status(), up)).toBe(false);
    expect(offerBundled(s, status({ available: false }), down)).toBe(false);
    expect(offerBundled(s, null, down)).toBe(false);
    expect(offerBundled(s, status(), null)).toBe(false);
    // Already on it: nothing to offer (the download problem shows instead).
    expect(offerBundled({ ...s, cleanup_profile: BUNDLED_ID }, status(), down)).toBe(false);
  });

  it("explains why it can't run yet", () => {
    const on = settings({ llm_profiles: [local, bundled], cleanup_profile: BUNDLED_ID });
    expect(bundledProblem(on, status())).toContain("not downloaded");
    expect(bundledProblem(on, status({ available: false }))).toContain("no bundled llama-server");
    expect(bundledProblem(on, status({ downloaded: true }))).toBeNull();
    expect(bundledProblem(settings(), status())).toBeNull();
    expect(bundledProblem(on, null)).toBeNull();
  });
});

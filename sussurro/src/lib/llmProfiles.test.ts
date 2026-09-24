import { describe, expect, it } from "vitest";
import {
  cleanupProfile,
  cleanupServerChanged,
  commitProfile,
  inferExternal,
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

import { describe, expect, it, vi } from "vitest";
import {
  cleanupActive,
  cleanupAllowed,
  cleanupGate,
  consentArgs,
  externalHostsTitle,
  formatCount,
  needsConsent,
  obtainConsent,
  peopleLabel,
  profileHostOf,
  sentExternally,
  sizeLabel,
  withCleanupOptIn,
} from "./privacy";
import type { ExternalRunPreview, LlmProfile, Settings } from "./types";

const local: LlmProfile = { id: "local", name: "Local", api: "ollama", base_url: "http://localhost:11434", api_key: "", model: "llama3.2:3b", external: false };
const work: LlmProfile = { id: "work", name: "Work", api: "openai", base_url: "https://API.example.com:443/v1", api_key: "k", model: "gpt-4o-mini", external: true };

function settings(over: Partial<Settings> = {}): Settings {
  return {
    llm_profiles: [local, work],
    cleanup_profile: "local",
    cleanup_level: "light",
    output_language: "",
    ...over,
  } as Settings;
}

const preview: ExternalRunPreview = {
  item_id: "2026/09/a",
  item_title: "Weekly sync",
  recipe_id: "summary",
  recipe_name: "Summary",
  question: null,
  profile_id: "work",
  profile_name: "Work",
  host: "api.example.com",
  base_url: "https://api.example.com/v1",
  model: "gpt-4o-mini",
  chars: 12345,
  approx_tokens: 3087,
  external: true,
};

describe("per-run consent", () => {
  it("is needed only on external profiles", () => {
    expect(needsConsent(work)).toBe(true);
    expect(needsConsent(local)).toBe(false);
    expect(needsConsent(null)).toBe(false);
  });

  it("never asks or calls anything for a local profile", async () => {
    const call = vi.fn();
    const confirm = vi.fn();
    const got = await obtainConsent(call, confirm, local, { id: "a", recipeId: "summary", question: null, profileId: "local" });
    expect(got).toEqual({ consent: null });
    expect(call).not.toHaveBeenCalled();
    expect(confirm).not.toHaveBeenCalled();
  });

  it("shows the preview and issues a token only after OK", async () => {
    const call = vi.fn(async (cmd: string, _args: Record<string, unknown>) => (cmd === "external_run_preview" ? preview : { token: "t1", expires_in_secs: 120 }));
    const confirm = vi.fn(async () => true);
    const req = { id: "2026/09/a", recipeId: "summary", question: null, profileId: "work" };
    const got = await obtainConsent(call, confirm, work, req);
    expect(got).toEqual({ consent: "t1" });
    expect(confirm).toHaveBeenCalledWith(preview);
    expect(call.mock.calls.map((c) => c[0])).toEqual(["external_run_preview", "prepare_external_run"]);
    expect(call.mock.calls[1][1]).toEqual({ id: "2026/09/a", recipeId: "summary", question: null, profileId: "work", includeEmails: false });
  });

  it("sends nothing and issues no token when the user cancels", async () => {
    const call = vi.fn(async (_cmd: string, _args: Record<string, unknown>) => preview);
    const got = await obtainConsent(call, async () => false, work, { id: "a", recipeId: null, question: "Who?", profileId: "work" });
    expect(got).toBeNull();
    expect(call).toHaveBeenCalledTimes(1);
    expect(call.mock.calls[0][0]).toBe("external_run_preview");
  });

  it("asks every time", async () => {
    let n = 0;
    const call = vi.fn(async (cmd: string) => (cmd === "external_run_preview" ? preview : { token: `t${++n}`, expires_in_secs: 120 }));
    const confirm = vi.fn(async () => true);
    const req = { id: "a", recipeId: "summary", question: null, profileId: "work" };
    const a = await obtainConsent(call, confirm, work, req);
    const b = await obtainConsent(call, confirm, work, req);
    expect(confirm).toHaveBeenCalledTimes(2);
    expect(a?.consent).not.toBe(b?.consent);
  });

  it("propagates backend refusals", async () => {
    const call = vi.fn(async () => {
      throw "no archive item 'x'";
    });
    await expect(obtainConsent(call, async () => true, work, { id: "x", recipeId: "summary", question: null, profileId: "work" })).rejects.toBe(
      "no archive item 'x'",
    );
  });

  it("builds a question or a recipe request, never both", () => {
    expect(consentArgs({ id: "a", recipeId: "summary", question: "Who?", profileId: "w" })).toEqual({ id: "a", recipeId: null, question: "Who?", profileId: "w", includeEmails: false });
    expect(consentArgs({ id: "a", recipeId: "summary", question: null, profileId: "w" })).toEqual({ id: "a", recipeId: "summary", question: null, profileId: "w", includeEmails: false });
  });

  // #143: the email choice is part of what is confirmed.
  it("sends the participant-email choice with the request", async () => {
    expect(consentArgs({ id: "a", recipeId: "meeting-minutes", question: null, profileId: "w", includeEmails: true })).toEqual({
      id: "a",
      recipeId: "meeting-minutes",
      question: null,
      profileId: "w",
      includeEmails: true,
    });
    const calls: [string, Record<string, unknown>][] = [];
    const call = vi.fn(async (cmd: string, args: Record<string, unknown>) => {
      calls.push([cmd, args]);
      return cmd === "prepare_external_run" ? { token: "t", expires_in_secs: 120 } : preview;
    });
    await obtainConsent(call, async () => true, work, { id: "a", recipeId: "summary", question: null, profileId: "work", includeEmails: true });
    expect(calls.map(([c, a]) => [c, a.includeEmails])).toEqual([
      ["external_run_preview", true],
      ["prepare_external_run", true],
    ]);
  });

  it("says which names and emails go along", () => {
    const base = { speakers: ["Anna", "Voice 2"], participants: 3, emails_available: 2, emails_sent: 0 };
    expect(peopleLabel(base)).toBe("2 speaker names (Anna, Voice 2) and 3 participant names — emails are not sent");
    expect(peopleLabel({ ...base, emails_sent: 2 })).toBe("2 speaker names (Anna, Voice 2) and 3 participant names and 2 emails");
    expect(peopleLabel({ speakers: ["Voice 1"], participants: 1, emails_available: 1, emails_sent: 0 })).toBe(
      "1 speaker name (Voice 1) and 1 participant name — the email is not sent",
    );
    expect(peopleLabel({ speakers: [], participants: 0 })).toBe("");
    expect(peopleLabel({})).toBe("");
  });

  it("describes the size", () => {
    expect(formatCount(12345)).toBe("12,345");
    expect(formatCount(-3)).toBe("0");
    expect(sizeLabel(12345, 3087)).toBe("≈ 12,345 characters (≈ 3,087 tokens)");
  });
});

describe("cleanup opt-in", () => {
  it("reads the host like the backend", () => {
    expect(profileHostOf(work)).toBe("api.example.com");
    expect(profileHostOf({ base_url: "" })).toBe("");
  });

  it("is always allowed locally, and bound to the host when external", () => {
    expect(cleanupAllowed(local)).toBe(true);
    expect(cleanupAllowed(work)).toBe(false);
    expect(cleanupAllowed({ ...work, cleanup_opt_in: "api.example.com" })).toBe(true);
    expect(cleanupAllowed({ ...work, cleanup_opt_in: "API.EXAMPLE.COM " })).toBe(true);
    expect(cleanupAllowed({ ...work, base_url: "https://llm.other.example/v1", cleanup_opt_in: "api.example.com" })).toBe(false);
  });

  it("says where cleanup stands", () => {
    expect(cleanupGate(settings())).toEqual({ state: "local" });
    const blocked = cleanupGate(settings({ cleanup_profile: "work" }));
    expect(blocked.state).toBe("blocked");
    expect(blocked.state !== "local" && blocked.host).toBe("api.example.com");
    const s = withCleanupOptIn(settings({ cleanup_profile: "work" }), "work", true);
    expect(s.llm_profiles[1].cleanup_opt_in).toBe("api.example.com");
    expect(s.llm_profiles[0]).toBe(local);
    expect(cleanupGate(s).state).toBe("allowed");
    const off = withCleanupOptIn(s, "work", false);
    expect("cleanup_opt_in" in off.llm_profiles[1]).toBe(false);
    expect(cleanupGate(off).state).toBe("blocked");
  });

  it("knows when cleanup sends anything at all", () => {
    expect(cleanupActive(settings())).toBe(true);
    expect(cleanupActive(settings({ cleanup_level: "none" }))).toBe(false);
    expect(cleanupActive(settings({ cleanup_level: "none", output_language: "en" }))).toBe(true);
    expect(cleanupActive(settings({ cleanup_level: "none", output_language: "same" }))).toBe(false);
  });
});

describe("sent-externally marker", () => {
  it("is derived from the item's hosts", () => {
    expect(sentExternally({})).toBe(false);
    expect(sentExternally({ external_hosts: [] })).toBe(false);
    expect(sentExternally({ external_hosts: ["api.example.com"] })).toBe(true);
    expect(externalHostsTitle(["api.example.com", "llm.other.example"])).toBe("Sent to an external LLM: api.example.com, llm.other.example");
    expect(externalHostsTitle(undefined)).toBe("");
  });
});

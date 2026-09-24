import { describe, expect, it } from "vitest";
import {
  commitRecipe,
  companionFileName,
  companionLabel,
  defaultRecipeProfile,
  duplicateRecipe,
  itemHasSpeakers,
  newRecipe,
  newRecipeId,
  progressFraction,
  participantEmails,
  progressLabel,
  provenance,
  recipeProblems,
  recipesFor,
  removeRecipe,
} from "./recipes";
import type { CompanionDoc, Item, ItemType, LlmProfile, Recipe, Settings } from "./types";

const builtin = (id: string, name: string): Recipe => ({ id, name, prompt: "p", target: "companion_document", builtin: true });
const BUILTINS = [
  builtin("formatted-document", "Formatted document"),
  builtin("summary", "Summary"),
  builtin("action-items", "Action items"),
  builtin("decisions", "Decisions"),
];
const mine: Recipe = { id: "open-questions", name: "Open questions", prompt: "List them.", target: "companion_document", builtin: false };

const profile = (id: string, external: boolean): LlmProfile => ({
  id,
  name: id,
  api: "ollama",
  base_url: external ? "https://x.example.com" : "http://localhost:11434",
  api_key: "",
  model: "m",
  external,
});

const settings = (over: Partial<Settings> = {}): Settings =>
  ({ llm_profiles: [profile("work", true), profile("local", false), profile("lan", false)], cleanup_profile: "work", recipes: [mine], ...over }) as Settings;

describe("recipe editing", () => {
  it("new ids never clash with built-ins or user recipes", () => {
    const all = [...BUILTINS, mine];
    expect(newRecipeId(all, "Summary")).toBe("summary-2");
    expect(newRecipeId(all, "Domande è aperte")).toBe("domande-e-aperte");
    expect(newRecipeId(all, "???")).toBe("recipe");
  });

  it("new and duplicated recipes get unused names and are never built-in", () => {
    expect(newRecipe(BUILTINS).name).toBe("New recipe");
    expect(newRecipe([...BUILTINS, { ...mine, name: "New recipe" }]).name).toBe("New recipe 2");
    const copy = duplicateRecipe(BUILTINS, BUILTINS[1]);
    expect(copy).toMatchObject({ id: "", name: "Summary (copy)", builtin: false, prompt: "p" });
  });

  it("problems: name, duplicate name, prompt", () => {
    const all = [...BUILTINS, mine];
    expect(recipeProblems({ ...mine, id: "" , name: " summary " }, all)).toEqual(["Another recipe is already called “summary”."]);
    expect(recipeProblems({ ...mine, name: "", prompt: " " }, all)).toHaveLength(2);
    expect(recipeProblems(mine, all)).toEqual([]);
  });

  it("commit appends with a fresh id or replaces in place; remove drops", () => {
    const s = settings();
    const all = [...BUILTINS, ...s.recipes];
    const added = commitRecipe(s, all, { ...newRecipe(all), name: " Risks ", prompt: " List risks. ", builtin: true });
    expect(added.recipe).toMatchObject({ id: "risks", name: "Risks", prompt: "List risks.", builtin: false });
    expect(added.settings.recipes.map((r) => r.id)).toEqual(["open-questions", "risks"]);
    const edited = commitRecipe(added.settings, all, { ...mine, prompt: "New." });
    expect(edited.settings.recipes[0].prompt).toBe("New.");
    expect(edited.settings.recipes).toHaveLength(2);
    expect(removeRecipe(edited.settings, "risks").recipes.map((r) => r.id)).toEqual(["open-questions"]);
  });
});

describe("companion files", () => {
  it("mirror the backend naming", () => {
    expect(BUILTINS.map(companionFileName)).toEqual(["document.md", "summary.md", "action-items.md", "decisions.md"]);
    expect(companionFileName({ ...mine, name: "Transcript" })).toBe("recipe-transcript.md");
    expect(companionFileName({ ...mine, name: "Document" })).toBe("recipe-document.md");
    expect(companionFileName({ ...mine, name: "!!" })).toBe("untitled.md");
  });

  const doc = (file: string, recipe: string, generated_by = ""): CompanionDoc => ({
    file,
    meta: { title: "", generated_by, recipe, profile: "Local", model: "m", external: false, date: "", transcript: "transcript.md" },
    body: "",
    edited_externally: false,
  });

  it("labels by recipe name, numbering kept copies", () => {
    expect(companionLabel(doc("document.md", "formatted-document"), BUILTINS)).toBe("Formatted document");
    expect(companionLabel(doc("document-2.md", "formatted-document"), BUILTINS)).toBe("Formatted document (2)");
    expect(companionLabel(doc("notes.md", ""), BUILTINS)).toBe("notes");
    // A saved Ask answer (#121) is labelled by its question.
    const answer = (question: string) => {
      const d = doc("who-sends-the-file.md", "question");
      return { ...d, meta: { ...d.meta, kind: "answer", question } };
    };
    expect(companionLabel(answer("Who sends the file?"), BUILTINS)).toBe("Who sends the file?");
    expect(companionLabel(answer("x".repeat(40)), BUILTINS)).toBe(`${"x".repeat(32)}…`);
  });

  it("provenance from generated_by, else the separate keys", () => {
    expect(provenance(doc("a.md", "summary", "Summary / Local / qwen3"))).toEqual(["Summary", "Local", "qwen3"]);
    expect(provenance(doc("a.md", "summary"))).toEqual(["summary", "Local", "m"]);
  });
});

describe("profile choice and progress", () => {
  it("prefers a remembered local profile, then the cleanup one if local, then any local", () => {
    expect(defaultRecipeProfile(settings(), "lan")?.id).toBe("lan");
    expect(defaultRecipeProfile(settings(), "work")?.id).toBe("local"); // external never preselected
    expect(defaultRecipeProfile(settings({ cleanup_profile: "lan" }), null)?.id).toBe("lan");
    expect(defaultRecipeProfile(settings({ llm_profiles: [profile("work", true)] }), null)).toBeNull();
  });

  it("describes and measures each phase", () => {
    expect(progressLabel("Summary", null)).toBe("Summary · starting…");
    expect(progressLabel("Summary", { phase: "map", done: 1, total: 5 })).toBe("Summary · reading part 2 of 5");
    expect(progressLabel("Summary", { phase: "map", done: 5, total: 5 })).toBe("Summary · reading part 5 of 5");
    expect(progressLabel("Summary", { phase: "reduce", done: 0, total: 1 })).toBe("Summary · writing the result…");
    expect(progressFraction({ phase: "map", done: 5, total: 10 })).toBeCloseTo(0.4);
    expect(progressFraction({ phase: "reduce", done: 1, total: 1 })).toBe(1);
    expect(progressFraction(null)).toBe(0);
  });
});

// #143: meeting recipes only where the transcript names its speakers.
describe("speakers-only recipes", () => {
  const minutes: Recipe = { ...builtin("meeting-minutes", "Meeting minutes"), speakers_only: true };
  const who: Recipe = { ...builtin("who-said-what", "Who said what"), speakers_only: true };
  const ALL = [...BUILTINS, minutes, who, mine];

  const item = (type: ItemType, speakers: boolean, extra: Partial<Item> = {}): Item => ({
    id: "2026/09/x",
    meta: { type, title: "T", date: "", source: "", language: "it", engine: "", tags: [], categories: [], participants: [] },
    segments: {
      version: 1,
      speakers: speakers ? [{ id: "meet:anna", label: "Anna", color: "#000" }, { id: "voice:1", label: "Voice 1", color: "#111" }] : [],
      segments: [
        { id: 0, start_ms: 0, end_ms: 900, raw: "Ciao.", text: "Ciao.", speaker_id: speakers ? "meet:anna" : undefined },
        { id: 1, start_ms: 1000, end_ms: 1900, raw: "Ok.", text: "Ok.", speaker_id: speakers ? "voice:1" : undefined },
      ],
    },
    body: "",
    edited_externally: false,
    ...extra,
  });
  const ids = (rs: Recipe[]) => rs.map((r) => r.id);

  it("shows them on meetings and transcriptions with speakers", () => {
    expect(ids(recipesFor(ALL, item("meeting", true)))).toEqual(ids(ALL));
    expect(ids(recipesFor(ALL, item("transcription", true)))).toEqual(ids(ALL));
  });

  it("hides them on items without speakers", () => {
    const general = ids([...BUILTINS, mine]);
    expect(ids(recipesFor(ALL, item("meeting", false)))).toEqual(general);
    expect(ids(recipesFor(ALL, item("transcription", false)))).toEqual(general);
    // Notes never, even with a labelled voice.
    expect(ids(recipesFor(ALL, item("note", true)))).toEqual(general);
    // A speaker with no label, or only on an empty line, doesn't count.
    const blank = item("meeting", true);
    blank.segments.speakers = blank.segments.speakers.map((s) => ({ ...s, label: " " }));
    expect(itemHasSpeakers(blank)).toBe(false);
    const silent = item("meeting", true);
    silent.segments.segments = silent.segments.segments.map((s) => ({ ...s, text: " " }));
    expect(itemHasSpeakers(silent)).toBe(false);
  });

  it("reads speakers from a transcript edited outside Sussurro", () => {
    const edited = (body: string) => item("meeting", false, { edited_externally: true, body });
    expect(itemHasSpeakers(edited("# T\n\n**[00:00:01] Anna:** ciao\n"))).toBe(true);
    expect(itemHasSpeakers(edited("# T\n\n**[00:00:01]** ciao\n[00:00:02] hi\n"))).toBe(false);
    expect(itemHasSpeakers(edited("Anna: ciao"))).toBe(false);
  });

  it("counts the participant emails a run could include", () => {
    const i = item("meeting", true);
    expect(participantEmails(i)).toBe(0);
    i.meta.participants = [{ name: "Anna", email: "anna@example.com" }, { name: "Ben" }, { name: " ", email: "x@example.com" }, { name: "Carla", email: " " }];
    expect(participantEmails(i)).toBe(1);
  });
});

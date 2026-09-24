import { describe, expect, it } from "vitest";
import {
  INITIAL_ASK,
  MAX_QUESTION_CHARS,
  answerFileName,
  askBlocked,
  askReducer,
  defaultAskProfile,
  externalNote,
  isAnswerRun,
  plainText,
  profileChoices,
  questionProblem,
  runName,
  type AskAction,
  type AskState,
} from "./ask";
import type { Item, LlmProfile, Recipe, RecipeFinished, Settings } from "./types";

const profile = (id: string, external: boolean, base = external ? "https://llm.example.com/v1" : "http://localhost:11434"): LlmProfile => ({
  id,
  name: id[0].toUpperCase() + id.slice(1),
  api: "openai",
  base_url: base,
  api_key: "",
  model: `${id}-model`,
  external,
});

const settings = (over: Partial<Settings> = {}): Settings =>
  ({ llm_profiles: [profile("work", true), profile("local", false)], cleanup_profile: "work", recipes: [], ...over }) as Settings;

const finished = (over: Partial<RecipeFinished> = {}): RecipeFinished => ({
  item_id: "a",
  recipe_id: "question",
  recipe_name: "Question",
  file: null,
  answer: null,
  answer_id: null,
  question: "Who sends the file?",
  profile: "Local",
  model: "llama3.2:3b",
  external: false,
  error: null,
  cancelled: false,
  ...over,
});

const run = (s: AskState, ...actions: AskAction[]) => actions.reduce(askReducer, s);

describe("run lifecycle", () => {
  const ask: AskAction = { type: "start", recipeId: "question", recipeName: "Question", question: "Who sends the file?", answerRun: true };

  it("start → progress → answer", () => {
    let s = run(INITIAL_ASK, ask);
    expect(s.run).toMatchObject({ question: "Who sends the file?", step: null, cancelling: false, fromAsk: true });
    s = askReducer(s, {
      type: "progress",
      event: { item_id: "a", recipe_id: "question", recipe_name: "Question", question: "Who sends the file?", phase: "map", done: 1, total: 4 },
    });
    expect(s.run?.step).toEqual({ phase: "map", done: 1, total: 4 });
    expect(s.run?.fromAsk).toBe(true);
    s = askReducer(s, { type: "finished", event: finished({ answer: "Anna, on Friday.", answer_id: 7 }), answerRun: true });
    expect(s.run).toBeNull();
    expect(s.answer).toMatchObject({ answerId: 7, text: "Anna, on Friday.", question: "Who sends the file?", profile: "Local", saved: null });
  });

  it("a new question clears the previous answer and messages", () => {
    const s = run(
      INITIAL_ASK,
      ask,
      { type: "finished", event: finished({ answer: "x", answer_id: 1 }), answerRun: true },
      { type: "refused", error: "boom" },
      ask,
    );
    expect(s.answer).toBeNull();
    expect(s.error).toBe("");
    expect(s.run?.fromAsk).toBe(true);
  });

  it("a refusal (external profile, run in progress…) ends the run with its message", () => {
    const s = run(INITIAL_ASK, ask, { type: "refused", error: "“Work” is an external profile" });
    expect(s.run).toBeNull();
    expect(s.error).toContain("external profile");
  });

  it("cancel marks the run until it ends, then says so", () => {
    let s = run(INITIAL_ASK, ask, { type: "cancel" });
    expect(s.run?.cancelling).toBe(true);
    // Progress that arrives while stopping keeps the flag.
    s = askReducer(s, {
      type: "progress",
      event: { item_id: "a", recipe_id: "question", recipe_name: "Question", phase: "map", done: 2, total: 4 },
    });
    expect(s.run?.cancelling).toBe(true);
    expect(s.run?.question).toBe("Who sends the file?");
    s = askReducer(s, { type: "finished", event: finished({ cancelled: true }), answerRun: true });
    expect(s.run).toBeNull();
    expect(s.answer).toBeNull();
    expect(s.notice).toBe("“Who sends the file?” cancelled.");
    expect(askReducer(INITIAL_ASK, { type: "cancel" })).toBe(INITIAL_ASK);
  });

  it("errors name the run", () => {
    const s = run(INITIAL_ASK, ask, { type: "finished", event: finished({ error: "timeout" }), answerRun: true });
    expect(s.error).toBe("“Who sends the file?” failed: timeout");
  });

  it("a document recipe started here reports the file written and keeps an unsaved answer", () => {
    const s = run(
      INITIAL_ASK,
      ask,
      { type: "finished", event: finished({ answer: "Anna.", answer_id: 4 }), answerRun: true },
      { type: "start", recipeId: "summary", recipeName: "Summary", question: null, answerRun: false },
    );
    expect(s.answer?.text).toBe("Anna.");
    expect(s.run?.recipeName).toBe("Summary");
    const done = askReducer(s, {
      type: "finished",
      event: finished({ recipe_id: "summary", recipe_name: "Summary", question: null, file: "summary.md" }),
      answerRun: false,
    });
    expect(done.written).toEqual({ file: "summary.md", recipeName: "Summary" });
    expect(done.answer?.answerId).toBe(4);
  });

  it("a Document tab run shows progress here but its outcome is left to the tab", () => {
    let s = askReducer(INITIAL_ASK, {
      type: "progress",
      event: { item_id: "a", recipe_id: "summary", recipe_name: "Summary", phase: "single", done: 0, total: 1 },
    });
    expect(s.run).toMatchObject({ recipeName: "Summary", fromAsk: false });
    s = askReducer(s, { type: "finished", event: finished({ recipe_id: "summary", recipe_name: "Summary", question: null, error: "down" }), answerRun: false });
    expect(s).toEqual(INITIAL_ASK);
  });

  it("an adopted answer run (UI reloaded mid-run) still shows its answer", () => {
    let s = askReducer(INITIAL_ASK, {
      type: "adopt",
      status: { item_id: "a", recipe_id: "question", recipe_name: "Question", question: "Why?", progress: null },
    });
    expect(s.run).toMatchObject({ question: "Why?", fromAsk: false });
    // Adopting never replaces a run already known.
    expect(askReducer(s, { type: "adopt", status: { item_id: "a", recipe_id: "x", recipe_name: "X", progress: null } })).toBe(s);
    s = askReducer(s, { type: "finished", event: finished({ question: "Why?", answer: "Because.", answer_id: 3 }), answerRun: true });
    expect(s.answer?.text).toBe("Because.");
  });

  it("save as document: saving, saved, failure", () => {
    let s = run(INITIAL_ASK, ask, { type: "finished", event: finished({ answer: "A", answer_id: 2 }), answerRun: true }, { type: "saving" });
    expect(s.answer?.saving).toBe(true);
    const failed = askReducer(s, { type: "save_failed", error: "no longer available" });
    expect(failed.answer?.saving).toBe(false);
    expect(failed.error).toBe("no longer available");
    s = askReducer(s, { type: "saved", file: "who-sends-the-file.md" });
    expect(s.answer).toMatchObject({ saving: false, saved: "who-sends-the-file.md" });
    expect(askReducer(s, { type: "dismiss_answer" }).answer).toBeNull();
    expect(askReducer(INITIAL_ASK, { type: "saving" })).toBe(INITIAL_ASK);
  });

  it("dismissing messages clears error, notice and written", () => {
    const s: AskState = { ...INITIAL_ASK, error: "e", notice: "n", written: { file: "f.md", recipeName: "F" } };
    expect(askReducer(s, { type: "dismiss_message" })).toEqual(INITIAL_ASK);
  });
});

describe("profiles", () => {
  it("lists every profile and marks the external ones", () => {
    const c = profileChoices(settings());
    expect(c.map((p) => [p.id, p.external])).toEqual([
      ["work", true],
      ["local", false],
    ]);
    expect(c[0].detail).toBe("work-model · llm.example.com");
    expect(c[1].detail).toBe("local-model · localhost:11434");
    expect(profileChoices(settings({ llm_profiles: [profile("odd", false, "not a url")] }))[0].detail).toBe("odd-model · not a url");
  });

  it("starts on the remembered profile, else a local one", () => {
    expect(defaultAskProfile(settings(), "work")?.id).toBe("work");
    expect(defaultAskProfile(settings(), null)?.id).toBe("local");
    expect(defaultAskProfile(settings(), "gone")?.id).toBe("local");
    expect(defaultAskProfile(settings({ llm_profiles: [profile("work", true)] }), null)?.id).toBe("work");
    expect(defaultAskProfile(settings({ llm_profiles: [] }), null)).toBeNull();
  });

  it("explains an external choice", () => {
    expect(externalNote(profile("local", false))).toBe("");
    expect(externalNote(null)).toBe("");
    const note = externalNote(profile("work", true));
    expect(note).toContain("llm.example.com");
    expect(note).toContain("confirmation");
  });
});

describe("helpers", () => {
  const recipes: Recipe[] = [
    { id: "summary", name: "Summary", prompt: "p", target: "companion_document", builtin: true },
    { id: "who", name: "Who said it", prompt: "p", target: "answer", builtin: false },
  ];

  it("tells answer runs from document runs", () => {
    expect(isAnswerRun("question", recipes)).toBe(true);
    expect(isAnswerRun("who", recipes)).toBe(true);
    expect(isAnswerRun("summary", recipes)).toBe(false);
    expect(isAnswerRun("unknown", recipes)).toBe(false);
  });

  it("validates questions", () => {
    expect(questionProblem("  ")).toBe("Write a question first.");
    expect(questionProblem("Why?")).toBe("");
    expect(questionProblem("x".repeat(MAX_QUESTION_CHARS))).toBe("");
    expect(questionProblem("x".repeat(MAX_QUESTION_CHARS + 1))).toContain("too long");
  });

  it("blocks a recording item or no profiles", () => {
    expect(askBlocked({ recording: true }, [profile("local", false)])).toContain("recorded");
    expect(askBlocked({}, [])).toContain("LLM profile");
    expect(askBlocked({ recording: false }, [profile("local", false)])).toBe("");
  });

  it("names saved answers like the backend", () => {
    expect(answerFileName({ question: "Who sends the file?", recipeName: "Question" })).toBe("who-sends-the-file.md");
    expect(answerFileName({ question: "Perché è così?", recipeName: "Question" })).toBe("perche-e-cosi.md");
    expect(answerFileName({ question: "Document?", recipeName: "Question" })).toBe("question-document.md");
    expect(answerFileName({ question: "???", recipeName: "Question" })).toBe("untitled.md");
    expect(answerFileName({ question: null, recipeName: "Who said it" })).toBe("who-said-it.md");
    expect(answerFileName({ question: null, recipeName: "Transcript" })).toBe("recipe-transcript.md");
    expect(answerFileName({ question: "word ".repeat(40), recipeName: "Question" }).length).toBeLessThanOrEqual(63);
  });

  it("names runs by their question", () => {
    expect(runName("Summary", null)).toBe("Summary");
    expect(runName("Question", "Why?")).toBe("“Why?”");
    expect(runName("Question", "y".repeat(60))).toBe(`“${"y".repeat(48)}…”`);
  });

  it("copies the transcript as plain text", () => {
    const item = (edited: boolean) =>
      ({
        segments: { version: 1, speakers: [], segments: [{ id: 0, start_ms: 0, end_ms: 1, raw: "", text: " One. " }, { id: 1, start_ms: 1, end_ms: 2, raw: "", text: "" }, { id: 2, start_ms: 2, end_ms: 3, raw: "", text: "Two." }] },
        body: "# Title\n\nEdited.\n",
        edited_externally: edited,
      }) as Pick<Item, "segments" | "body" | "edited_externally">;
    expect(plainText(item(false))).toBe("One.\n\nTwo.");
    expect(plainText(item(true))).toBe("# Title\n\nEdited.");
  });
});

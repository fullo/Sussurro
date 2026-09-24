/* Ask panel (#121): the pure state behind the context pane's Ask section —
   run lifecycle (start, progress, cancel, finish), the answer and its
   "Save as document", profile choices with external ones marked. The pane
   (shell/ContextPane.tsx) only wires these to invoke() and events. */

import { defaultRecipeProfile, slug } from "./recipes";
import type {
  Item,
  LlmProfile,
  Recipe,
  RecipeFinished,
  RecipeProgress,
  RecipeRunStatus,
  RecipeStep,
  Settings,
} from "./types";

/** Id of the transient free-question recipe (recipes/answer.rs). */
export const QUESTION_RECIPE_ID = "question";
/** Longest question the backend accepts, in characters. */
export const MAX_QUESTION_CHARS = 2000;

export interface AskRun {
  recipeId: string;
  recipeName: string;
  /** The question, for a free question. */
  question: string | null;
  step: RecipeStep | null;
  cancelling: boolean;
  /** Started from this pane (vs. the Document tab, or adopted after a reload). */
  fromAsk: boolean;
}

export interface AskAnswer {
  /** Backend handle for saving; null when the backend kept none. */
  answerId: number | null;
  recipeName: string;
  question: string | null;
  text: string;
  profile: string;
  model: string;
  external: boolean;
  /** Server the transcript went to, on an external profile (#122). */
  host: string;
  saving: boolean;
  /** File it was saved as. */
  saved: string | null;
}

export interface AskState {
  run: AskRun | null;
  answer: AskAnswer | null;
  /** A document recipe started here wrote this file. */
  written: { file: string; recipeName: string } | null;
  error: string;
  notice: string;
}

export const INITIAL_ASK: AskState = { run: null, answer: null, written: null, error: "", notice: "" };

export type AskAction =
  /** `answerRun`: the run answers here (a new answer replaces the shown
   *  one); a document run leaves an unsaved answer where it is. */
  | { type: "start"; recipeId: string; recipeName: string; question: string | null; answerRun: boolean }
  | { type: "refused"; error: string }
  /** The user cancelled the external-profile confirmation (#122). */
  | { type: "declined"; notice: string }
  | { type: "adopt"; status: RecipeRunStatus }
  | { type: "progress"; event: RecipeProgress }
  | { type: "cancel" }
  | { type: "finished"; event: RecipeFinished; answerRun: boolean }
  | { type: "saving" }
  | { type: "saved"; file: string }
  | { type: "save_failed"; error: string }
  | { type: "dismiss_answer" }
  | { type: "dismiss_message" };

/** `s` cut to `max` characters with an ellipsis. */
export function shorten(s: string, max: number): string {
  const chars = Array.from(s);
  return chars.length > max ? `${chars.slice(0, max).join("").trimEnd()}…` : s;
}

/** What a run is called in progress lines and messages. */
export function runName(recipeName: string, question: string | null | undefined): string {
  return question ? `“${shorten(question, 48)}”` : recipeName;
}

export function askReducer(s: AskState, a: AskAction): AskState {
  switch (a.type) {
    case "start":
      return {
        run: { recipeId: a.recipeId, recipeName: a.recipeName, question: a.question, step: null, cancelling: false, fromAsk: true },
        answer: a.answerRun ? null : s.answer,
        written: null,
        error: "",
        notice: "",
      };
    case "refused":
      return { ...s, run: null, error: a.error };
    case "declined":
      return { ...s, run: null, error: "", notice: a.notice, written: null };
    case "adopt":
      if (s.run) return s;
      return {
        ...s,
        run: {
          recipeId: a.status.recipe_id,
          recipeName: a.status.recipe_name,
          question: a.status.question ?? null,
          step: a.status.progress,
          cancelling: false,
          fromAsk: false,
        },
      };
    case "progress": {
      const e = a.event;
      return {
        ...s,
        run: {
          recipeId: e.recipe_id,
          recipeName: e.recipe_name,
          question: e.question ?? s.run?.question ?? null,
          step: { phase: e.phase, done: e.done, total: e.total },
          cancelling: s.run?.cancelling ?? false,
          fromAsk: s.run?.fromAsk ?? false,
        },
      };
    }
    case "cancel":
      return s.run ? { ...s, run: { ...s.run, cancelling: true } } : s;
    case "finished": {
      const e = a.event;
      const mine = s.run?.fromAsk ?? false;
      const next: AskState = { ...s, run: null };
      // The Document tab reports its own document runs.
      if (!mine && !a.answerRun) return next;
      const name = runName(e.recipe_name, e.question);
      if (e.cancelled) return { ...next, notice: `${name} cancelled.` };
      if (e.error) return { ...next, error: `${name} failed: ${e.error}` };
      if (e.answer != null) {
        return {
          ...next,
          answer: {
            answerId: e.answer_id ?? null,
            recipeName: e.recipe_name,
            question: e.question ?? null,
            text: e.answer,
            profile: e.profile ?? "",
            model: e.model ?? "",
            external: !!e.external,
            host: e.host ?? "",
            saving: false,
            saved: null,
          },
        };
      }
      if (e.file && mine) return { ...next, written: { file: e.file, recipeName: e.recipe_name } };
      return next;
    }
    case "saving":
      return s.answer ? { ...s, error: "", answer: { ...s.answer, saving: true } } : s;
    case "saved":
      return s.answer ? { ...s, answer: { ...s.answer, saving: false, saved: a.file } } : s;
    case "save_failed":
      return { ...s, error: a.error, answer: s.answer ? { ...s.answer, saving: false } : null };
    case "dismiss_answer":
      return { ...s, answer: null };
    case "dismiss_message":
      return { ...s, error: "", notice: "", written: null };
  }
}

/** Whether a run's result goes to the Ask panel (a free question or an
 *  answer recipe) rather than to a document. */
export function isAnswerRun(recipeId: string, recipes: Recipe[]): boolean {
  return recipeId === QUESTION_RECIPE_ID || recipes.find((r) => r.id === recipeId)?.target === "answer";
}

/** Why a question can't be asked yet; "" when it can. */
export function questionProblem(q: string): string {
  const t = q.trim();
  if (!t) return "Write a question first.";
  if (Array.from(t).length > MAX_QUESTION_CHARS) return `The question is too long (at most ${MAX_QUESTION_CHARS} characters).`;
  return "";
}

/** Why nothing can run on this item right now; "" when something can. */
export function askBlocked(item: Pick<Item, "recording">, profiles: LlmProfile[]): string {
  if (item.recording) return "This item is being recorded — ask when the session ends.";
  if (!profiles.length) return "Add an LLM profile in Recipes first.";
  return "";
}

export interface ProfileChoice {
  id: string;
  name: string;
  /** "llama3.2:3b · localhost:11434" */
  detail: string;
  external: boolean;
}

function host(url: string): string {
  try {
    return new URL(url).host || url;
  } catch {
    return url;
  }
}

/** Every profile as the Ask panel lists it, external ones marked. */
export function profileChoices(s: Settings): ProfileChoice[] {
  return s.llm_profiles.map((p) => ({
    id: p.id,
    name: p.name,
    detail: [p.model, host(p.base_url)].filter(Boolean).join(" · "),
    external: p.external,
  }));
}

/** The profile the Ask panel starts on: the remembered one if it still
 *  exists (external included — it is marked), else the local default the
 *  Document tab would pick, else the first profile. */
export function defaultAskProfile(s: Settings, remembered: string | null): LlmProfile | null {
  return s.llm_profiles.find((p) => p.id === remembered) ?? defaultRecipeProfile(s, remembered) ?? s.llm_profiles[0] ?? null;
}

/** The notice under the profile list when `p` is external. */
export function externalNote(p: LlmProfile | null): string {
  if (!p?.external) return "";
  return `“${p.name}” is external: each run asks for your confirmation before the transcript goes to ${host(p.base_url)}.`;
}

/** File "Save as document" asks for (mirrors recipes::answer::answer_file_name;
 *  the backend adds -2, -3… when it is taken). */
export function answerFileName(a: Pick<AskAnswer, "question" | "recipeName">): string {
  const prefix = a.question ? "question" : "recipe";
  const s = slug(a.question ?? a.recipeName, 60) || "untitled";
  return s === "transcript" || s === "document" ? `${prefix}-${s}.md` : `${s}.md`;
}

/** The transcript as plain text for "Copy text": one paragraph per line
 *  from the segments, or the markdown body when it was edited outside. */
export function plainText(item: Pick<Item, "segments" | "body" | "edited_externally">): string {
  const lines = item.segments.segments.map((s) => s.text.trim()).filter(Boolean);
  if (!item.edited_externally && lines.length) return lines.join("\n\n");
  return item.body.trim();
}

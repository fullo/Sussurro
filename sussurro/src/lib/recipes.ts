/* Recipes (#120): pure helpers for the Recipes screen's editor and the
   document pane's Document tab. The Rust side (recipes/mod.rs,
   Settings::normalize) repairs whatever reaches it; these keep the UI's
   edits well-formed in the first place. */

import type { CompanionDoc, LlmProfile, Recipe, RecipeStep, RecipeTarget, Settings } from "./types";

export const TARGET_LABELS: Record<RecipeTarget, string> = {
  companion_document: "Document",
  answer: "Answer",
};

export const FORMATTED_DOCUMENT_ID = "formatted-document";

/** Lowercase ASCII slug, at most `max` characters, cut on a `-` when that
 *  keeps at least half (as archive::paths::slugify). */
function slug(name: string, max = 40): string {
  const s = name
    .toLowerCase()
    .normalize("NFKD")
    .replace(/[̀-ͯ]/g, "")
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
  if (s.length <= max) return s;
  const cut = s.slice(0, max);
  const dash = cut.lastIndexOf("-");
  return (dash >= max / 2 ? cut.slice(0, dash) : cut).replace(/-+$/, "");
}

/** An id from `name`, unique among every recipe (built-ins included). */
export function newRecipeId(all: Recipe[], name: string): string {
  const ids = new Set(all.map((r) => r.id));
  const base = slug(name) || "recipe";
  if (!ids.has(base)) return base;
  for (let n = 2; ; n++) if (!ids.has(`${base}-${n}`)) return `${base}-${n}`;
}

/** A blank user recipe for the editor; its id is set when saved. */
export function newRecipe(all: Recipe[]): Recipe {
  const names = new Set(all.map((r) => r.name.toLowerCase()));
  let name = "New recipe";
  for (let n = 2; names.has(name.toLowerCase()); n++) name = `New recipe ${n}`;
  return { id: "", name, prompt: "", target: "companion_document", builtin: false };
}

/** A user copy of `r` (e.g. a built-in to adapt). */
export function duplicateRecipe(all: Recipe[], r: Recipe): Recipe {
  const names = new Set(all.map((x) => x.name.toLowerCase()));
  let name = `${r.name} (copy)`;
  for (let n = 2; names.has(name.toLowerCase()); n++) name = `${r.name} (copy ${n})`;
  return { ...r, id: "", name, builtin: false };
}

/** Why a recipe can't be saved yet; empty when it can. */
export function recipeProblems(r: Recipe, all: Recipe[]): string[] {
  const out: string[] = [];
  const name = r.name.trim();
  if (!name) out.push("Give the recipe a name.");
  else if (all.some((o) => o.id !== r.id && o.name.trim().toLowerCase() === name.toLowerCase()))
    out.push(`Another recipe is already called “${name}”.`);
  if (!r.prompt.trim()) out.push("Write the prompt: what the model should do with the transcript.");
  return out;
}

/** Save a user recipe into the settings (replace by id, or append with a
 *  fresh id). `all` = built-ins + user recipes, for id uniqueness. */
export function commitRecipe(s: Settings, all: Recipe[], r: Recipe): { settings: Settings; recipe: Recipe } {
  const clean: Recipe = { ...r, name: r.name.trim(), prompt: r.prompt.trim(), builtin: false };
  const recipes = s.recipes ?? [];
  if (clean.id && recipes.some((o) => o.id === clean.id)) {
    return { settings: { ...s, recipes: recipes.map((o) => (o.id === clean.id ? clean : o)) }, recipe: clean };
  }
  const added = { ...clean, id: newRecipeId(all, clean.name) };
  return { settings: { ...s, recipes: [...recipes, added] }, recipe: added };
}

export function removeRecipe(s: Settings, id: string): Settings {
  return { ...s, recipes: (s.recipes ?? []).filter((r) => r.id !== id) };
}

/** File a document recipe writes (mirrors recipes::companion_file_name). */
export function companionFileName(r: Recipe): string {
  if (r.id === FORMATTED_DOCUMENT_ID) return "document.md";
  const s = slug(r.name, 60) || "untitled";
  return s === "transcript" || s === "document" ? `recipe-${s}.md` : `${s}.md`;
}

/** The profile the Document tab offers first: the remembered one if it is
 *  still there and local, else the cleanup profile when local, else the
 *  first local one, else null (only external profiles). */
export function defaultRecipeProfile(s: Settings, remembered: string | null): LlmProfile | null {
  const local = s.llm_profiles.filter((p) => !p.external);
  return (
    local.find((p) => p.id === remembered) ??
    local.find((p) => p.id === s.cleanup_profile) ??
    local[0] ??
    null
  );
}

/** "Formatted document · part 2 of 5" style progress line. */
export function progressLabel(name: string, step: RecipeStep | null): string {
  if (!step) return `${name} · starting…`;
  switch (step.phase) {
    case "single":
      return `${name} · writing…`;
    case "map":
      return `${name} · reading part ${Math.min(step.done + 1, step.total)} of ${step.total}`;
    case "merge":
      return `${name} · condensing notes (${Math.min(step.done + 1, step.total)} of ${step.total})`;
    case "reduce":
      return `${name} · writing the result…`;
  }
}

/** Share of the run done, 0–1, for a progress bar (map is most of it). */
export function progressFraction(step: RecipeStep | null): number {
  if (!step) return 0;
  const f = step.total ? step.done / step.total : 0;
  switch (step.phase) {
    case "single":
      return 0.1 + 0.8 * f;
    case "map":
      return 0.8 * f;
    case "merge":
      return 0.8 + 0.1 * f;
    case "reduce":
      return 0.9 + 0.1 * f;
  }
}

/** Tab label of a companion: its recipe's name when known, else the file. */
export function companionLabel(doc: CompanionDoc, recipes: Recipe[]): string {
  const r = recipes.find((x) => x.id === doc.meta.recipe);
  const base = r?.name ?? doc.file.replace(/\.md$/i, "");
  const variant = /-(\d+)\.md$/i.exec(doc.file);
  return r && variant && companionFileName(r) !== doc.file ? `${base} (${variant[1]})` : base;
}

/** Provenance line pieces: recipe · profile · model (whatever is known). */
export function provenance(doc: CompanionDoc): string[] {
  const m = doc.meta;
  if (m.generated_by) return m.generated_by.split(" / ").map((x) => x.trim()).filter(Boolean);
  return [m.recipe, m.profile, m.model].filter(Boolean);
}

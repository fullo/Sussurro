import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { CollapsibleCard, Tip } from "../components/ui";
import type { Ctl } from "../hooks/useAppController";
import {
  TARGET_LABELS,
  commitRecipe,
  companionFileName,
  duplicateRecipe,
  newRecipe,
  recipeProblems,
  removeRecipe,
} from "../lib/recipes";
import type { Recipe, RecipeTarget } from "../lib/types";

/** Built-in and user recipes (#120), on the Recipes screen. Built-ins are
 *  read-only (duplicate one to adapt it); user recipes live in the settings. */
export function RecipesCard({ ctl }: { ctl: Ctl }) {
  const { settings } = ctl;
  const [all, setAll] = useState<Recipe[]>([]);
  /** The recipe open in the editor: a saved one, or a new draft (id ""). */
  const [editing, setEditing] = useState<Recipe | null>(null);

  useEffect(() => {
    invoke<Recipe[]>("recipes_list")
      .then(setAll)
      .catch((e) => ctl.setBusy(String(e)));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [settings.recipes]);

  return (
    <CollapsibleCard
      storageKey="recipesList"
      title={<>Recipes <span className="via">prompts that write documents</span></>}
      collapsible={false}
      headerExtra={
        <button type="button" className="btn-ghost" onClick={() => setEditing(newRecipe(all))} disabled={editing?.id === ""}>
          Add recipe
        </button>
      }
    >
      <p className="card-hint">
        Run a recipe from the <em>Document</em> tab of a note, meeting or transcription: it reads the transcript (with
        speakers and timestamps when there are any) and writes a markdown file next to it. Long transcripts are read in
        parts that fit the profile's context window, then combined.
      </p>
      <ul className="prof-list" aria-label="Recipes">
        {all.map((r) => (
          <li key={r.id}>
            <button
              type="button"
              className={`prof-row${editing?.id === r.id ? " active" : ""}`}
              aria-expanded={editing?.id === r.id}
              onClick={() => setEditing(editing?.id === r.id ? null : r)}
            >
              <span className="prof-name">{r.name}</span>
              <span className="prof-sub">
                {r.target === "companion_document" ? companionFileName(r) : "answer in the Ask panel"}
              </span>
              {r.builtin && <span className="tb note">Built-in</span>}
            </button>
          </li>
        ))}
      </ul>
      {editing && (
        <RecipeEditor
          key={editing.id || `new-${editing.name}`}
          ctl={ctl}
          all={all}
          initial={editing}
          onDuplicate={(r) => setEditing(duplicateRecipe(all, r))}
          onDone={() => setEditing(null)}
        />
      )}
    </CollapsibleCard>
  );
}

function RecipeEditor({
  ctl,
  all,
  initial,
  onDuplicate,
  onDone,
}: {
  ctl: Ctl;
  all: Recipe[];
  initial: Recipe;
  onDuplicate: (r: Recipe) => void;
  onDone: () => void;
}) {
  const { settings, save, flash } = ctl;
  const [draft, setDraft] = useState(initial);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [saving, setSaving] = useState(false);
  const readOnly = initial.builtin;
  const isNew = initial.id === "";
  const problems = recipeProblems(draft, all);
  const dirty = JSON.stringify(draft) !== JSON.stringify(initial);

  const commit = async () => {
    setSaving(true);
    const { settings: next, recipe } = commitRecipe(settings, all, draft);
    const ok = await save(next);
    setSaving(false);
    if (ok) {
      flash(`Recipe “${recipe.name}” saved.`);
      onDone();
    }
  };

  const remove = async () => {
    if (await save(removeRecipe(settings, initial.id))) {
      flash(`Recipe “${initial.name}” deleted. Documents it wrote stay in their folders.`);
      onDone();
    }
  };

  return (
    <div className="prof-editor" role="group" aria-label={readOnly ? initial.name : isNew ? "New recipe" : `Edit ${initial.name}`}>
      {readOnly && (
        <p className="card-hint rc-builtin-note">Built-in recipe: it can't be changed. Duplicate it to make your own version.</p>
      )}
      <div className="field">
        <div className="field-label"><span>Name</span></div>
        <input
          value={draft.name}
          readOnly={readOnly}
          onChange={(e) => setDraft({ ...draft, name: e.target.value })}
          aria-label="Recipe name"
          autoFocus={isNew}
        />
      </div>
      <div className="field">
        <div className="field-label">
          <span>Output <Tip text="Document: a markdown file next to the transcript, named after the recipe. Answer: shown in the Ask panel of the open document (arriving in a later update)." /></span>
          {draft.target === "companion_document" && draft.name.trim() && <small className="mono">{companionFileName(draft)}</small>}
        </div>
        <select
          value={draft.target}
          disabled={readOnly}
          onChange={(e) => setDraft({ ...draft, target: e.target.value as RecipeTarget })}
          aria-label="Output"
        >
          {(Object.keys(TARGET_LABELS) as RecipeTarget[]).map((t) => (
            <option key={t} value={t}>{TARGET_LABELS[t]}</option>
          ))}
        </select>
      </div>
      <div className="field field-col">
        <div className="field-label">
          <span>Prompt <Tip text="What the model should do with the transcript, in plain words. Sussurro adds the transcript, its title, date and participants, and the rules (same language, no invented facts, markdown only)." /></span>
        </div>
        <textarea
          className="rc-prompt"
          rows={7}
          value={draft.prompt}
          readOnly={readOnly}
          onChange={(e) => setDraft({ ...draft, prompt: e.target.value })}
          placeholder="List the open questions, with who raised each one."
          aria-label="Prompt"
        />
      </div>
      {!readOnly && problems.length > 0 && (dirty || isNew) && (
        <ul className="prof-problems">
          {problems.map((p) => <li key={p}>{p}</li>)}
        </ul>
      )}
      {confirmDelete ? (
        <div className="row-gap prof-actions" role="alertdialog" aria-label="Confirm delete">
          <span>Delete “{initial.name}”? Documents it already wrote are kept.</span>
          <button type="button" className="btn-danger sh-btn push" onClick={remove}>Delete</button>
          <button type="button" className="btn-ghost" onClick={() => setConfirmDelete(false)}>Keep</button>
        </div>
      ) : (
        <div className="row-gap prof-actions">
          {!readOnly && (
            <button
              type="button"
              className="btn-dark sh-btn"
              disabled={saving || problems.length > 0 || (!dirty && !isNew)}
              onClick={commit}
            >
              {isNew ? "Add recipe" : "Save"}
            </button>
          )}
          {!isNew && (
            <button type="button" className="btn-ghost" onClick={() => onDuplicate(initial)}>Duplicate</button>
          )}
          <button type="button" className="btn-ghost" onClick={onDone}>
            {!readOnly && (dirty || isNew) ? "Cancel" : "Close"}
          </button>
          {!readOnly && !isNew && (
            <button type="button" className="btn-ghost push" onClick={() => setConfirmDelete(true)}>Delete…</button>
          )}
        </div>
      )}
    </div>
  );
}

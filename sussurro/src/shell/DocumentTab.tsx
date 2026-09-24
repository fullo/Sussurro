import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Ctl } from "../hooks/useAppController";
import { fileManagerName, formatLongDate } from "../lib/format";
import {
  companionLabel,
  defaultRecipeProfile,
  participantEmails,
  progressFraction,
  progressLabel,
  provenance,
  recipesFor,
} from "../lib/recipes";
import { isAnswerRun, runName } from "../lib/ask";
import { profileHostOf } from "../lib/privacy";
import { useExternalConsent } from "./ConsentDialog";
import { EmailOptIn } from "./EmailOptIn";
import type {
  CompanionDoc,
  Item,
  Recipe,
  RecipeFinished,
  RecipeProgress,
  RecipeRunStatus,
  RecipeStep,
} from "../lib/types";
import { Markdown } from "./Markdown";

const PROFILE_KEY = "recipeProfile";
const RECIPE_KEY = "recipeLast";

function remembered(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function remember(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    /* storage unavailable: the choice just isn't kept */
  }
}

type Run = { recipeName: string; step: RecipeStep | null; cancelling: boolean };

/** The document pane's *Document* tab (#120): the companion documents
 *  recipes wrote next to the transcript, rendered, and the bar that runs a
 *  recipe on this item. */
export function DocumentTab({
  ctl,
  item,
  onCount,
  onChanged,
  select = null,
  version = 0,
}: {
  ctl: Ctl;
  item: Item;
  /** How many documents the item has (for the tab label). */
  onCount: (n: number) => void;
  onChanged: () => void;
  /** Show this document (a new `n` asks again for the same file). */
  select?: { file: string; n: number } | null;
  /** Bumped when a document was written elsewhere (the Ask panel). */
  version?: number;
}) {
  const { settings } = ctl;
  const id = item.id;
  const [recipes, setRecipes] = useState<Recipe[]>([]);
  const [docs, setDocs] = useState<CompanionDoc[] | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [recipeId, setRecipeId] = useState<string>(() => remembered(RECIPE_KEY) ?? "formatted-document");
  const [profileId, setProfileId] = useState<string | null>(
    () => defaultRecipeProfile(settings, remembered(PROFILE_KEY))?.id ?? null,
  );
  const [run, setRun] = useState<Run | null>(null);
  const [error, setError] = useState("");
  /** Send participant emails with the next run (#143); never remembered. */
  const [emails, setEmails] = useState(false);
  const idRef = useRef(id);
  idRef.current = id;
  const recipesRef = useRef<Recipe[]>([]);
  recipesRef.current = recipes;
  const mounted = useRef({ select: false, version: false });
  const { consentFor, dialog, asking } = useExternalConsent();

  const loadDocs = async (select?: string | null) => {
    try {
      const list = await invoke<CompanionDoc[]>("recipe_documents", { id });
      if (idRef.current !== id) return;
      setDocs(list);
      onCount(list.length);
      setSelected((cur) => {
        const want = select ?? cur;
        return want && list.some((d) => d.file === want) ? want : (list[0]?.file ?? null);
      });
    } catch (e) {
      setDocs([]);
      onCount(0);
      setError(String(e));
    }
  };

  // Recipes change when the user edits them in Recipes (settings.recipes).
  useEffect(() => {
    invoke<Recipe[]>("recipes_list")
      .then(setRecipes)
      .catch((e) => setError(String(e)));
  }, [settings.recipes]);

  useEffect(() => {
    setDocs(null);
    setRun(null);
    setError("");
    setEmails(false);
    loadDocs(select?.file ?? null);
    // A run on this item may already be going (started before a reload).
    invoke<RecipeRunStatus[]>("recipe_status")
      .then((all) => {
        const mine = all.find((r) => r.item_id === id);
        if (mine && idRef.current === id) setRun({ recipeName: runName(mine.recipe_name, mine.question), step: mine.progress, cancelling: false });
      })
      .catch(() => {});
    const subs = [
      listen<RecipeProgress>("recipe-progress", (e) => {
        const p = e.payload;
        if (p.item_id !== idRef.current) return;
        setRun((r) => ({
          recipeName: runName(p.recipe_name, p.question),
          step: { phase: p.phase, done: p.done, total: p.total },
          cancelling: r?.cancelling ?? false,
        }));
      }),
      listen<RecipeFinished>("recipe-finished", (e) => {
        const f = e.payload;
        if (f.item_id !== idRef.current) return;
        setRun(null);
        // The Ask panel reports its own answers, errors and cancels.
        const answerRun = isAnswerRun(f.recipe_id, recipesRef.current);
        if (f.error && !answerRun) setError(`${f.recipe_name} failed: ${f.error}`);
        else if (f.cancelled && !answerRun) ctl.flash(`${f.recipe_name} cancelled — nothing was written.`);
        if (f.file) {
          loadDocs(f.file);
          onChanged();
        }
      }),
    ];
    return () => {
      subs.forEach((p) => p.then((f) => f()));
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id]);

  // The Ask panel opened a document, or wrote one: reload (the mount
  // already loaded, so the first run of each is skipped).
  useEffect(() => {
    if (!mounted.current.select) {
      mounted.current.select = true;
      return;
    }
    if (select) loadDocs(select.file);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [select?.n]);
  useEffect(() => {
    if (!mounted.current.version) {
      mounted.current.version = true;
      return;
    }
    loadDocs(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [version]);

  // Meeting recipes only where the transcript names its speakers (#143).
  const docRecipes = useMemo(
    () => recipesFor(recipes, settings, item).filter((r) => r.target === "companion_document"),
    [recipes, settings, item],
  );
  const emailCount = participantEmails(item);
  const recipe = docRecipes.find((r) => r.id === recipeId) ?? docRecipes[0] ?? null;
  // An external profile is used only when the user picked it here; the
  // default is always a local one (no silent fallback, #122).
  const profile = settings.llm_profiles.find((p) => p.id === profileId) ?? defaultRecipeProfile(settings, profileId);
  const doc = docs?.find((d) => d.file === selected) ?? null;
  const regenerate = !!recipe && !!docs?.some((d) => d.meta.recipe === recipe.id && !d.edited_externally);

  const start = async (r: Recipe | null = recipe) => {
    if (!r || !profile || asking) return;
    setError("");
    // Participant emails go along only when ticked for this run (#143).
    const includeEmails = emails && emailCount > 0;
    // An external profile asks first, every time (#122); Cancel sends nothing.
    let consent: string | null = null;
    try {
      const got = await consentFor(profile, { id, recipeId: r.id, question: null, profileId: profile.id, includeEmails });
      if (!got) {
        ctl.flash(`${r.name} not run — nothing was sent to ${profileHostOf(profile) || profile.name}.`);
        return;
      }
      consent = got.consent;
    } catch (e) {
      setError(String(e));
      return;
    }
    setRun({ recipeName: r.name, step: null, cancelling: false });
    setEmails(false);
    try {
      // Progress and the end arrive as events; the reply only matters for
      // refusals, which happen before anything is sent.
      await invoke<RecipeFinished>("recipe_run", { id, recipeId: r.id, profileId: profile.id, consent, includeEmails });
    } catch (e) {
      setRun(null);
      setError(String(e));
    }
  };

  const cancel = async () => {
    setRun((r) => (r ? { ...r, cancelling: true } : r));
    await invoke<boolean>("recipe_cancel", { id }).catch(() => false);
  };

  const reveal = (file: string) =>
    invoke("recipe_reveal_document", { id, file }).catch((e) => ctl.setBusy(String(e)));

  const blocked = item.recording ? "This item is being recorded — recipes run when the session ends." : "";
  const extNote = profile?.external
    ? `“${profile.name}” is external: each run asks for your confirmation before the transcript goes to ${profileHostOf(profile) || profile.base_url}.`
    : "";

  return (
    <div className="rc-tab">
      <div className="rc-bar" role="group" aria-label="Run a recipe">
        {run ? (
          <>
            <div className="rc-progress" role="status" aria-live="polite">
              <span>{run.cancelling ? `${run.recipeName} · stopping after this step…` : progressLabel(run.recipeName, run.step)}</span>
              <span className="rc-meter" aria-hidden="true">
                <i style={{ width: `${Math.round(progressFraction(run.step) * 100)}%` }} />
              </span>
            </div>
            <button type="button" className="btn-ghost sh-btn" onClick={cancel} disabled={run.cancelling}>
              Cancel
            </button>
          </>
        ) : (
          <>
            <select
              aria-label="Recipe"
              value={recipe?.id ?? ""}
              onChange={(e) => {
                setRecipeId(e.target.value);
                remember(RECIPE_KEY, e.target.value);
              }}
              disabled={!docRecipes.length}
            >
              {docRecipes.map((r) => (
                <option key={r.id} value={r.id}>{r.name}</option>
              ))}
            </select>
            <span className="sh-muted">on</span>
            <select
              aria-label="LLM profile"
              value={profile?.id ?? ""}
              onChange={(e) => {
                setProfileId(e.target.value);
                remember(PROFILE_KEY, e.target.value);
              }}
            >
              {!profile && <option value="">Choose a profile</option>}
              {settings.llm_profiles.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name}
                  {p.model ? ` · ${p.model}` : ""}
                  {p.external ? ` · ↗ external (${profileHostOf(p) || "asks each run"})` : ""}
                </option>
              ))}
            </select>
            <button
              type="button"
              className="btn-dark sh-btn"
              onClick={() => start()}
              disabled={!recipe || !profile || !!blocked}
            >
              {regenerate ? "Regenerate" : "Run"}
            </button>
          </>
        )}
      </div>
      {!run && !blocked && <EmailOptIn count={emailCount} checked={emails} onChange={setEmails} />}
      {blocked && !run && <p className="rc-note" role="note">{blocked}</p>}
      {extNote && !blocked && !run && <p className="rc-note warn" role="note">{extNote}</p>}
      {dialog}
      {error && (
        <div className="notice-warn" role="alert">
          {error}{" "}
          <button type="button" className="link-btn" onClick={() => setError("")}>Dismiss</button>
        </div>
      )}

      {docs && docs.length > 1 && (
        <div className="rc-docs" role="tablist" aria-label="Documents">
          {docs.map((d) => (
            <button
              key={d.file}
              type="button"
              role="tab"
              aria-selected={d.file === selected}
              className={`fchip${d.file === selected ? " on" : ""}`}
              onClick={() => setSelected(d.file)}
              title={d.file}
            >
              {companionLabel(d, recipes)}
            </button>
          ))}
        </div>
      )}

      <div className="doc-scroll rc-scroll">
        {docs === null ? (
          <p className="sh-muted rc-empty">Loading…</p>
        ) : doc ? (
          <>
            <div className="rc-prov">
              {provenance(doc).length > 0 ? (
                <span>
                  Generated by <b>{provenance(doc)[0]}</b>
                  {provenance(doc).slice(1).map((p) => ` · ${p}`).join("")}
                  {doc.meta.date ? ` · ${formatLongDate(doc.meta.date)}` : ""}
                </span>
              ) : (
                <span>Written outside Sussurro</span>
              )}
              {doc.meta.external && (
                <span className="ext" title={doc.meta.host ? `The transcript was sent to ${doc.meta.host}` : "The transcript was sent to an external LLM"}>
                  ↗ External{doc.meta.host ? ` · ${doc.meta.host}` : ""}
                </span>
              )}
              <span className="mono">{doc.file}</span>
              <button type="button" className="link-btn push" onClick={() => reveal(doc.file)} title={`Show the file in ${fileManagerName()}`}>
                Show file
              </button>
            </div>
            {doc.edited_externally && (
              <div className="notice-warn" role="note">
                <strong>Edited outside Sussurro.</strong> Your changes are kept: running the recipe again writes a new
                copy next to this file.
              </div>
            )}
            <Markdown source={doc.body} label={companionLabel(doc, recipes)} />
          </>
        ) : (
          <div className="empty-state quiet rc-empty">
            <p>No documents yet.</p>
            <p className="sh-muted">
              A recipe writes a markdown file next to the transcript. <b>Formatted document</b> turns it into a
              readable document with a tl;dr, headings and tables; <b>Summary</b>, <b>Action items</b> and{" "}
              <b>Decisions</b> pull out just that. Recipes run on a local LLM profile unless you pick an external one,
              which asks before every run; you can add your own recipes in Recipes.
            </p>
            {!run && !blocked && docRecipes.some((r) => r.id === "formatted-document") && profile && (
              <button
                type="button"
                className="btn-dark sh-btn"
                onClick={() => start(docRecipes.find((r) => r.id === "formatted-document")!)}
              >
                Write the formatted document
              </button>
            )}
          </div>
        )}
      </div>
    </div>
  );
}

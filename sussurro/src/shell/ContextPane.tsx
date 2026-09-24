import { useEffect, useReducer, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { save as saveDialog } from "@tauri-apps/plugin-dialog";
import type { Ctl } from "../hooks/useAppController";
import { fileManagerName } from "../lib/format";
import {
  INITIAL_ASK,
  QUESTION_RECIPE_ID,
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
} from "../lib/ask";
import { type ExportChoice, exportChoices, exportFileName, hasSubtitles, subtitlesInfo } from "../lib/export";
import { profileHostOf } from "../lib/privacy";
import { companionFileName, participantEmails, progressFraction, progressLabel, recipesFor } from "../lib/recipes";
import { EmailOptIn } from "./EmailOptIn";
import type { Item, Person, Recipe, RecipeFinished, RecipeProgress, RecipeRunStatus, SubtitlesStatus } from "../lib/types";
import { useExternalConsent } from "./ConsentDialog";
import { Markdown } from "./Markdown";
import { SpeakerPanel } from "./SpeakerPanel";
import { speakersEnabled } from "../lib/speakers";

const PROFILE_KEY = "askProfile";

function remembered(): string | null {
  try {
    return localStorage.getItem(PROFILE_KEY);
  } catch {
    return null;
  }
}

function remember(value: string) {
  try {
    localStorage.setItem(PROFILE_KEY, value);
  } catch {
    /* storage unavailable: the choice just isn't kept */
  }
}

/** Window width below which the context pane is a drawer (plan §9; keep
 *  in step with the `max-width: 1000px` rules in shell.css). */
const DRAWER_QUERY = "(max-width: 1000px)";

/** Whether the context pane is laid out as a drawer. */
export function useDrawerLayout(): boolean {
  const query = () => typeof window !== "undefined" && !!window.matchMedia?.(DRAWER_QUERY).matches;
  const [narrow, setNarrow] = useState(query);
  useEffect(() => {
    const mq = window.matchMedia?.(DRAWER_QUERY);
    if (!mq) return;
    const on = () => setNarrow(mq.matches);
    on();
    mq.addEventListener("change", on);
    return () => mq.removeEventListener("change", on);
  }, []);
  return narrow;
}

/** What the drawer toggle shows while the drawer is closed. */
export type ContextStatus = "idle" | "running" | "answer";

const KIND_NOUN: Record<string, string> = { note: "note", meeting: "meeting", transcription: "transcription" };

/** The document pane's context pane (plan §9): *Speakers* (#130, not on
 *  notes), *Ask* (#121) and *Export*. Below ~1000 px it is a
 *  drawer. */
export function ContextPane({
  ctl,
  item,
  drawer,
  open,
  onClose,
  onStatus,
  onDocsChanged,
  onOpenDocument,
  onItem,
  people,
  onPeopleChanged,
  pickedLine,
  onPickLine,
}: {
  ctl: Ctl;
  item: Item;
  /** The People registry (#132), for linking speakers. */
  people?: Person[];
  onPeopleChanged?: () => void;
  /** Voice map (#144): the line picked on the map, and how to pick one. */
  pickedLine?: number | null;
  onPickLine?: (segmentId: number) => void;
  /** A speaker edit returned the updated item (#130). */
  onItem: (item: Item) => void;
  /** Laid out as a drawer (narrow window). */
  drawer: boolean;
  /** The drawer is open (ignored when not a drawer). */
  open: boolean;
  onClose: () => void;
  onStatus: (s: ContextStatus) => void;
  /** A companion document was written (recipe run or saved answer). */
  onDocsChanged: () => void;
  /** Show a companion document in the Document tab. */
  onOpenDocument: (file: string) => void;
}) {
  const { settings } = ctl;
  const id = item.id;
  const [recipes, setRecipes] = useState<Recipe[]>([]);
  const [state, dispatch] = useReducer(askReducer, INITIAL_ASK);
  const [question, setQuestion] = useState("");
  /** Send participant emails with the next run (#143); never remembered. */
  const [emails, setEmails] = useState(false);
  const [profileId, setProfileId] = useState<string | null>(() => defaultAskProfile(settings, remembered())?.id ?? null);
  const recipesRef = useRef<Recipe[]>([]);
  recipesRef.current = recipes;
  const idRef = useRef(id);
  idRef.current = id;
  const paneRef = useRef<HTMLElement>(null);
  const { consentFor, dialog, asking } = useExternalConsent();
  const [exporting, setExporting] = useState(false);
  const [srtBusy, setSrtBusy] = useState(false);
  const [srtStatus, setSrtStatus] = useState<SubtitlesStatus | null>(null);

  // Where transcript.srt stands; again after every save of the item (the
  // Always setting rewrites it then).
  useEffect(() => {
    if (!hasSubtitles(item.meta.type)) {
      setSrtStatus(null);
      return;
    }
    let live = true;
    invoke<SubtitlesStatus>("archive_subtitles_status", { id })
      .then((st) => live && setSrtStatus(st))
      .catch(() => live && setSrtStatus(null));
    return () => {
      live = false;
    };
  }, [id, item]);

  useEffect(() => {
    invoke<Recipe[]>("recipes_list")
      .then(setRecipes)
      .catch(() => setRecipes([]));
  }, [settings.recipes]);

  useEffect(() => {
    setEmails(false);
    // A run on this item may already be going (started before a reload).
    invoke<RecipeRunStatus[]>("recipe_status")
      .then((all) => {
        const mine = all.find((r) => r.item_id === id);
        if (mine && idRef.current === id) dispatch({ type: "adopt", status: mine });
      })
      .catch(() => {});
    const subs = [
      listen<RecipeProgress>("recipe-progress", (e) => {
        if (e.payload.item_id === idRef.current) dispatch({ type: "progress", event: e.payload });
      }),
      listen<RecipeFinished>("recipe-finished", (e) => {
        const f = e.payload;
        if (f.item_id !== idRef.current) return;
        dispatch({ type: "finished", event: f, answerRun: isAnswerRun(f.recipe_id, recipesRef.current) });
        // An external run also changes the item's "sent externally" marker.
        if (f.file || f.external) onDocsChanged();
      }),
    ];
    return () => {
      subs.forEach((p) => p.then((f) => f()));
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id]);

  const status: ContextStatus = state.run ? "running" : state.answer && !state.answer.saved ? "answer" : "idle";
  useEffect(() => onStatus(status), [status, onStatus]);

  // The drawer takes the focus when it opens.
  useEffect(() => {
    if (drawer && open) paneRef.current?.focus();
  }, [drawer, open]);

  const profile = settings.llm_profiles.find((p) => p.id === profileId) ?? defaultAskProfile(settings, null);
  const choices = profileChoices(settings);
  const blocked = askBlocked(item, settings.llm_profiles);
  const busy = !!state.run;
  const noun = KIND_NOUN[item.meta.type] ?? "document";
  const showSpeakers = speakersEnabled(item);
  // Meeting recipes only where the transcript names its speakers (#143).
  const shown = recipesFor(recipes, item);
  const emailCount = participantEmails(item);

  const dropAnswer = () => {
    const a = state.answer;
    if (a?.answerId != null && !a.saved) invoke("recipe_dismiss_answer", { id, answerId: a.answerId }).catch(() => {});
  };

  const start = async (r: Recipe | null, q: string | null) => {
    if (!profile || busy || blocked || asking) return;
    const answerRun = q !== null || r?.target === "answer";
    // Participant emails go along only when ticked for this run (#143).
    const includeEmails = emails && emailCount > 0;
    // An external profile asks first, every time (#122); Cancel sends nothing.
    let consent: string | null = null;
    try {
      const got = await consentFor(profile, { id, recipeId: r?.id ?? null, question: q, profileId: profile.id, includeEmails });
      if (!got) {
        dispatch({ type: "declined", notice: `Nothing was sent to ${profileHostOf(profile) || profile.name}.` });
        if (q !== null) setQuestion((cur) => cur || q);
        return;
      }
      consent = got.consent;
    } catch (e) {
      dispatch({ type: "refused", error: String(e) });
      if (q !== null) setQuestion((cur) => cur || q);
      return;
    }
    // A new answer replaces the one shown; a document run leaves it.
    if (answerRun) dropAnswer();
    dispatch({ type: "start", recipeId: r?.id ?? QUESTION_RECIPE_ID, recipeName: r?.name ?? "Question", question: q, answerRun });
    setEmails(false);
    try {
      // Progress and the end arrive as events; the reply matters only for
      // refusals, which happen before anything is sent.
      if (q !== null) await invoke<RecipeFinished>("recipe_ask", { id, question: q, profileId: profile.id, consent, includeEmails });
      else if (r) await invoke<RecipeFinished>("recipe_run", { id, recipeId: r.id, profileId: profile.id, consent, includeEmails });
    } catch (e) {
      dispatch({ type: "refused", error: String(e) });
      // Give a refused question back to edit or re-ask.
      if (q !== null) setQuestion((cur) => cur || q);
    }
  };

  const ask = () => {
    const problem = questionProblem(question);
    if (problem) {
      dispatch({ type: "refused", error: problem });
      return;
    }
    // The answer card repeats the question; the box is ready for the next.
    setQuestion("");
    start(null, question.trim());
  };

  const cancel = async () => {
    dispatch({ type: "cancel" });
    await invoke<boolean>("recipe_cancel", { id }).catch(() => false);
  };

  const save = async () => {
    const a = state.answer;
    if (!a || a.answerId == null) return;
    dispatch({ type: "saving" });
    try {
      const file = await invoke<string>("recipe_save_answer", { id, answerId: a.answerId });
      dispatch({ type: "saved", file });
      onDocsChanged();
    } catch (e) {
      dispatch({ type: "save_failed", error: String(e) });
    }
  };

  const copy = async (text: string, what: string) => {
    try {
      await invoke("copy_text", { text });
      ctl.flash(`${what} copied.`, 2500);
    } catch (e) {
      ctl.setBusy(String(e));
    }
  };

  const exportAs = async (c: ExportChoice) => {
    let path: string | null;
    try {
      path = await saveDialog({
        title: `Export as ${c.label}`,
        defaultPath: exportFileName(id, c.format),
        filters: [{ name: c.filter, extensions: [c.format] }],
      });
    } catch (e) {
      ctl.setBusy(String(e));
      return;
    }
    if (!path) return;
    setExporting(true);
    try {
      const written = await invoke<string>("archive_export", { id, format: c.format, path });
      ctl.flash(`Exported to ${written}`);
    } catch (e) {
      ctl.setBusy(String(e));
    } finally {
      setExporting(false);
    }
  };

  const createSrt = async () => {
    setSrtBusy(true);
    try {
      const st = await invoke<SubtitlesStatus>("archive_create_subtitles", { id });
      if (idRef.current === id) setSrtStatus(st);
      ctl.flash(`${st.file} written next to the transcript.`);
    } catch (e) {
      ctl.setBusy(String(e));
    } finally {
      setSrtBusy(false);
    }
  };

  const subtitles = subtitlesInfo(item.meta.type, settings.subtitles ?? "on_request", srtStatus, !!item.recording);

  const questionIssue = question.trim() ? questionProblem(question) : "";
  const note = externalNote(profile);
  const run = state.run;
  const answer = state.answer;

  return (
    <aside
      ref={paneRef}
      className={`ctx${drawer ? " drawer" : ""}${drawer && open ? " open" : ""}`}
      id="ctx-pane"
      aria-label={showSpeakers ? "Speakers, ask and export" : "Ask and export"}
      tabIndex={-1}
      onKeyDown={(e) => {
        if (drawer && e.key === "Escape") {
          e.stopPropagation();
          onClose();
        }
      }}
    >
      {drawer && (
        <div className="ctx-drawer-head">
          <span>{showSpeakers ? "Speakers · Ask · Export" : "Ask · Export"}</span>
          <button type="button" className="icon-btn" onClick={onClose} aria-label="Close the panel" title="Close (Esc)">
            ✕
          </button>
        </div>
      )}

      {showSpeakers && (
        <SpeakerPanel
          ctl={ctl}
          item={item}
          onItem={onItem}
          people={people}
          onPeopleChanged={onPeopleChanged}
          pickedLine={pickedLine}
          onPickLine={onPickLine}
        />
      )}

      <section className="ctx-sect" aria-labelledby="ctx-ask-h">
        <h3 id="ctx-ask-h">Ask</h3>
        <div className="ctx-recipes" role="group" aria-label="Recipes">
          {shown.map((r) => (
            <button
              key={r.id}
              type="button"
              className="sh-btn btn-ghost"
              disabled={busy || !!blocked || !profile}
              onClick={() => start(r, null)}
              title={r.target === "answer" ? `${r.name}: the answer appears here` : `${r.name}: writes ${companionFileName(r)} next to the transcript`}
            >
              {r.name}
              {r.target === "answer" && <span className="ctx-ans-mark" aria-label="(answer)">↳</span>}
            </button>
          ))}
        </div>
        <form
          className="ctx-ask"
          onSubmit={(e) => {
            e.preventDefault();
            ask();
          }}
        >
          <textarea
            aria-label={`Ask about this ${noun}`}
            placeholder={`Ask about this ${noun}…`}
            rows={2}
            value={question}
            disabled={!!blocked}
            onChange={(e) => setQuestion(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
                e.preventDefault();
                ask();
              }
            }}
          />
          <button type="submit" className="btn-dark sh-btn" disabled={busy || !!blocked || !profile || !question.trim() || !!questionIssue}>
            Ask
          </button>
        </form>
        {questionIssue && <p className="ctx-note warn">{questionIssue}</p>}
        {!blocked && <EmailOptIn count={emailCount} checked={emails} onChange={setEmails} disabled={busy} />}

        {choices.length > 0 && (
          <fieldset className="ctx-profiles" disabled={busy}>
            <legend>LLM profile</legend>
            {choices.map((c) => (
              <label key={c.id} className={`ctx-prof${c.id === profile?.id ? " on" : ""}`}>
                <input
                  type="radio"
                  name={`ask-profile-${id}`}
                  value={c.id}
                  checked={c.id === profile?.id}
                  onChange={() => {
                    setProfileId(c.id);
                    remember(c.id);
                  }}
                />
                <span className="ctx-prof-text">
                  <b>{c.name}</b>
                  <small>{c.detail}</small>
                </span>
                {c.external ? (
                  <span className="ext" title="Text sent to this profile leaves this machine">↗ External</span>
                ) : (
                  <span className="badge-local">Local</span>
                )}
              </label>
            ))}
          </fieldset>
        )}
        {note && <p className="ctx-note warn" role="note">{note}</p>}
        {blocked && <p className="ctx-note" role="note">{blocked}</p>}

        {run && (
          <div className="ctx-run" role="status" aria-live="polite">
            <div className="rc-progress">
              <span>
                {run.cancelling
                  ? `${runName(run.recipeName, run.question)} · stopping after this step…`
                  : progressLabel(runName(run.recipeName, run.question), run.step)}
              </span>
              <span className="rc-meter" aria-hidden="true">
                <i style={{ width: `${Math.round(progressFraction(run.step) * 100)}%` }} />
              </span>
            </div>
            <button type="button" className="btn-ghost sh-btn" onClick={cancel} disabled={run.cancelling}>
              Cancel
            </button>
          </div>
        )}

        {(state.error || state.notice || state.written) && (
          <div className={state.error ? "ctx-msg warn" : "ctx-msg"} role={state.error ? "alert" : "status"}>
            {state.error || state.notice}
            {state.written && (
              <>
                {state.written.recipeName} written to <span className="mono">{state.written.file}</span>.{" "}
                <button type="button" className="link-btn" onClick={() => onOpenDocument(state.written!.file)}>
                  Open
                </button>
              </>
            )}{" "}
            <button type="button" className="link-btn" onClick={() => dispatch({ type: "dismiss_message" })}>
              Dismiss
            </button>
          </div>
        )}

        {answer && (
          <div className="ctx-answer" aria-label="Answer">
            <p className="ctx-answer-q">{answer.question ?? answer.recipeName}</p>
            <div className="ctx-answer-body">
              <Markdown source={answer.text} label={answer.question ? `Answer to ${answer.question}` : answer.recipeName} />
            </div>
            <p className="ctx-answer-prov">
              {[answer.profile, answer.model].filter(Boolean).join(" · ")}
              {answer.external && (
                <span className="ext" title={answer.host ? `The transcript was sent to ${answer.host}` : "The transcript was sent to an external LLM"}>
                  ↗ External
                </span>
              )}
              {!answer.saved && <span className="sh-muted"> · not saved</span>}
            </p>
            <div className="ctx-answer-actions">
              {answer.saved ? (
                <span className="ctx-saved">
                  Saved as <span className="mono">{answer.saved}</span>{" "}
                  <button type="button" className="link-btn" onClick={() => onOpenDocument(answer.saved!)}>
                    Open
                  </button>
                </span>
              ) : (
                <button
                  type="button"
                  className="btn-dark sh-btn"
                  onClick={save}
                  disabled={answer.saving || answer.answerId == null || !!item.recording}
                  title={`Writes ${answerFileName(answer)} next to the transcript`}
                >
                  {answer.saving ? "Saving…" : "Save as document"}
                </button>
              )}
              <button type="button" className="btn-ghost sh-btn" onClick={() => copy(answer.text, "Answer")}>
                Copy
              </button>
              <button
                type="button"
                className="btn-ghost sh-btn"
                onClick={() => {
                  dropAnswer();
                  dispatch({ type: "dismiss_answer" });
                }}
              >
                {answer.saved ? "Close" : "Discard"}
              </button>
            </div>
          </div>
        )}
      </section>

      <section className="ctx-sect" aria-labelledby="ctx-export-h">
        <h3 id="ctx-export-h">Export</h3>
        <div className="ctx-exports" role="group" aria-label="Save as a file">
          {exportChoices(item.meta.type).map((c) => (
            <button
              key={c.format}
              type="button"
              className="btn-ghost sh-btn"
              onClick={() => exportAs(c)}
              disabled={exporting}
              title={`${c.title} — choose where to save it`}
            >
              {c.label}
            </button>
          ))}
        </div>
        <div className="ctx-exports ctx-exports-more">
          <button type="button" className="btn-ghost sh-btn" onClick={() => copy(plainText(item), "Text")} disabled={!plainText(item)}>
            Copy text
          </button>
          <button type="button" className="btn-ghost sh-btn" onClick={() => copy(item.body.trim(), "Markdown")} disabled={!item.body.trim()}>
            Copy markdown
          </button>
          <button
            type="button"
            className="btn-ghost sh-btn"
            onClick={() => invoke("archive_reveal", { id }).catch((e) => ctl.setBusy(String(e)))}
            title={`Show the item folder in ${fileManagerName()}`}
          >
            Show folder
          </button>
        </div>
        {subtitles && (
          <div className="ctx-subs">
            <p className="ctx-note">{subtitles.note}</p>
            {subtitles.action && (
              <button
                type="button"
                className="btn-ghost sh-btn"
                onClick={createSrt}
                disabled={srtBusy}
                title="Writes transcript.srt next to the transcript"
              >
                {srtBusy ? "Writing…" : subtitles.action}
              </button>
            )}
          </div>
        )}
      </section>
      {dialog}
    </aside>
  );
}

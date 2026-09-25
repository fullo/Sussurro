import { useDeferredValue, useEffect, useMemo, useState, type KeyboardEvent } from "react";
import { Tip } from "../components/ui";
import type { Ctl } from "../hooks/useAppController";
import { fmtCount } from "../lib/format";
import { duplicateCueCount, pageOf, removeIndices, snippetDraftIssues, snippetRows, type ListSort } from "../lib/personalization";
import type { SnippetEntry } from "../utils";
import { exportSnippets, importSnippets } from "./listImport";
import { Pager, SortSelect, UndoBar } from "./listControls";

/** First line of a snippet's text, for the table. */
const preview = (text: string) => text.trim().split(/\r?\n/)[0];

/** Snippets manager (#99): a searchable, sortable, paged table of cue →
 *  text with add, edit in place, delete, duplicate-cue warnings, import and
 *  export. */
export function SnippetManager({ ctl }: { ctl: Ctl }) {
  const { settings, save } = ctl;
  const snippets = settings.snippets;

  const [query, setQuery] = useState("");
  const deferredQuery = useDeferredValue(query);
  const [sort, setSort] = useState<ListSort>("added");
  const [page, setPage] = useState(0);
  /** The snippet being edited (-1 = a new one), with its draft. */
  const [editing, setEditing] = useState<{ index: number; draft: SnippetEntry } | null>(null);

  const rows = useMemo(() => snippetRows(snippets, deferredQuery, sort), [snippets, deferredQuery, sort]);
  const dupCues = useMemo(() => duplicateCueCount(snippets), [snippets]);
  const shown = pageOf(rows, page);
  useEffect(() => setPage(0), [deferredQuery, sort]);

  /** The last removal, undoable until the list changes again. */
  const [undo, setUndo] = useState<{ message: string; previous: SnippetEntry[]; after: string } | null>(null);
  const listKey = (list: SnippetEntry[]) => JSON.stringify(list);

  /** Save the new list; `removed` names a removal that Undo can revert. */
  const commit = async (next: SnippetEntry[], removed?: string) => {
    const previous = snippets;
    const ok = await save({ ...settings, snippets: next });
    if (ok) setUndo(removed ? { message: removed, previous, after: listKey(next) } : null);
    return ok;
  };

  const saveEdit = async () => {
    if (!editing) return;
    const draft = { cue: editing.draft.cue.trim(), text: editing.draft.text.trim() };
    if (!draft.cue || !draft.text) return;
    const next = snippets.slice();
    if (editing.index === -1) next.push(draft);
    else next[editing.index] = draft;
    if (await commit(next)) setEditing(null);
  };

  const remove = async (index: number) => {
    const cue = snippets[index].cue;
    if (editing?.index === index) setEditing(null);
    await commit(removeIndices(snippets, [index]), `Removed the snippet “${cue}”.`);
  };

  const editor = (index: number) =>
    editing?.index === index ? (
      <SnippetEditor
        snippets={snippets}
        index={index}
        draft={editing.draft}
        onChange={(draft) => setEditing({ index, draft })}
        onSave={saveEdit}
        onCancel={() => setEditing(null)}
      />
    ) : null;

  return (
    <div className="field field-col lm">
      <div className="field-label">
        <span>
          Snippets{" "}
          <Tip text="Example: cue 'firma email' → pastes your full signature. Matching ignores case and punctuation, and skips the AI cleanup entirely. When two snippets share a cue, the first one in the list is used." />
        </span>
        <small>say a cue exactly — Sussurro pastes the full text instead of transcribing</small>
      </div>

      <div className="lm-bar">
        <span className="lm-count" aria-live="polite">
          {fmtCount(snippets.length)} {snippets.length === 1 ? "snippet" : "snippets"}
          {deferredQuery.trim() && ` · ${fmtCount(rows.length)} shown`}
          {dupCues > 0 && ` · ${fmtCount(dupCues)} with a repeated cue`}
        </span>
        <span className="lm-bar-actions">
          <button
            type="button"
            className="btn-ghost"
            onClick={() => setEditing({ index: -1, draft: { cue: "", text: "" } })}
            disabled={editing?.index === -1}
          >
            + Add snippet
          </button>
          <button
            type="button"
            className="btn-ghost"
            onClick={() => importSnippets(ctl)}
            title='Add snippets from a .csv file (one "cue,text" per line; quote text with commas or line breaks); existing snippets are kept'
          >
            Import .csv
          </button>
          <button
            type="button"
            className="btn-ghost"
            onClick={() => exportSnippets(ctl)}
            title="Save the snippets as a .csv file (the format Import reads)"
          >
            Export .csv
          </button>
        </span>
      </div>

      {editor(-1)}

      {undo && undo.after === listKey(snippets) && (
        <UndoBar message={undo.message} onUndo={() => commit(undo.previous)} />
      )}

      {snippets.length > 0 && (
        <div className="lm-controls">
          <input
            type="search"
            className="lm-search"
            placeholder="Search cues and texts"
            aria-label="Search snippets"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            spellCheck={false}
          />
          <SortSelect value={sort} onChange={setSort} label="Sort snippets by cue" />
        </div>
      )}

      {snippets.length === 0 ? (
        editing ? null : <p className="sh-muted lm-empty">No snippets yet. Add one, or import a .csv file.</p>
      ) : rows.length === 0 ? (
        <p className="sh-muted lm-empty">No snippet matches “{deferredQuery.trim()}”.</p>
      ) : (
        <table className="lm-table">
          <thead>
            <tr>
              <th scope="col">Cue</th>
              <th scope="col">Text</th>
              <th scope="col" aria-label="Actions" />
            </tr>
          </thead>
          <tbody>
            {shown.rows.map((r) =>
              editing?.index === r.index ? (
                <tr key={r.index} className="lm-editing">
                  <td colSpan={3}>{editor(r.index)}</td>
                </tr>
              ) : (
                <tr key={r.index}>
                  <td className="lm-cue">
                    {r.snippet.cue || <em className="sh-muted">no cue</em>}
                    {r.shadowedBy !== null && (
                      <small className="lm-warn">
                        Same cue as “{snippets[r.shadowedBy].cue}” above: this one is never used.
                      </small>
                    )}
                    {r.shadowedBy === null && r.sameCue > 0 && (
                      <small className="lm-warn">Cue repeated below: this one is used.</small>
                    )}
                    {r.incomplete && <small className="lm-warn">Incomplete: needs a cue and a text.</small>}
                  </td>
                  <td className="lm-text" title={r.snippet.text}>
                    {preview(r.snippet.text) || <em className="sh-muted">no text</em>}
                  </td>
                  <td className="lm-row-actions">
                    <button
                      type="button"
                      className="link-btn"
                      onClick={() => setEditing({ index: r.index, draft: { ...r.snippet } })}
                      aria-label={`Edit the snippet “${r.snippet.cue}”`}
                    >
                      Edit
                    </button>
                    <button
                      type="button"
                      className="link-btn lm-danger"
                      onClick={() => remove(r.index)}
                      aria-label={`Delete the snippet “${r.snippet.cue}”`}
                    >
                      Delete
                    </button>
                  </td>
                </tr>
              ),
            )}
          </tbody>
        </table>
      )}
      <Pager page={shown.page} pages={shown.pages} total={rows.length} onPage={setPage} label="Snippet pages" />
    </div>
  );
}

/** Cue + text editor for a new (index -1) or existing snippet. Enter in the
 *  cue or Ctrl/Cmd+Enter in the text saves, Escape cancels. */
function SnippetEditor({
  snippets,
  index,
  draft,
  onChange,
  onSave,
  onCancel,
}: {
  snippets: SnippetEntry[];
  index: number;
  draft: SnippetEntry;
  onChange: (d: SnippetEntry) => void;
  onSave: () => void;
  onCancel: () => void;
}) {
  const issues = snippetDraftIssues(snippets, draft, index);
  const canSave = !issues.emptyCue && !issues.emptyText;
  const id = `lm-snip-${index}`;
  const keys = (e: KeyboardEvent, enterSaves: boolean) => {
    if (e.key === "Escape") {
      e.preventDefault();
      onCancel();
    } else if (e.key === "Enter" && (enterSaves || e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      if (canSave) onSave();
    }
  };
  return (
    <div className="lm-editor" role="group" aria-label={index === -1 ? "New snippet" : `Edit the snippet “${snippets[index].cue}”`}>
      <label className="lm-editor-field">
        <span>Cue</span>
        <input
          autoFocus
          placeholder="what you say"
          value={draft.cue}
          onChange={(e) => onChange({ ...draft, cue: e.target.value })}
          onKeyDown={(e) => keys(e, true)}
          aria-describedby={issues.cueUsedBy !== null ? `${id}-dup` : undefined}
          spellCheck={false}
        />
      </label>
      <label className="lm-editor-field">
        <span>Text</span>
        <textarea
          rows={3}
          placeholder="text to paste"
          value={draft.text}
          onChange={(e) => onChange({ ...draft, text: e.target.value })}
          onKeyDown={(e) => keys(e, false)}
          spellCheck={false}
        />
      </label>
      {issues.cueUsedBy !== null && (
        <small id={`${id}-dup`} className="lm-warn" role="status">
          {issues.cueUsedBy < index || index === -1
            ? `The snippet “${snippets[issues.cueUsedBy].cue}” already uses this cue and comes first, so this one would never be used.`
            : `The snippet “${snippets[issues.cueUsedBy].cue}” further down uses the same cue; this one comes first and wins.`}
        </small>
      )}
      <div className="lm-editor-actions">
        <button type="button" className="btn-ghost" onClick={onSave} disabled={!canSave}>
          {index === -1 ? "Add snippet" : "Save"}
        </button>
        <button type="button" className="link-btn" onClick={onCancel}>
          Cancel
        </button>
        <small className="sh-muted">Ctrl/⌘+Enter saves · Esc cancels</small>
      </div>
    </div>
  );
}

import { useDeferredValue, useEffect, useMemo, useRef, useState } from "react";
import { Tip } from "../components/ui";
import type { Ctl } from "../hooks/useAppController";
import { fmtCount } from "../lib/format";
import {
  addDictionaryWords,
  dedupeDictionary,
  dictionaryDuplicateCount,
  dictionaryRows,
  editDictionaryWord,
  pageOf,
  removeIndices,
  type ListSort,
  type WordEditError,
} from "../lib/personalization";
import { describeDictionaryMerge } from "../utils";
import { exportDictionary, importDictionary } from "./listImport";
import { Pager, SortSelect, UndoBar } from "./listControls";

const EDIT_ERRORS: Record<WordEditError, string> = {
  empty: "An entry can't be empty. Use Delete to remove it.",
  multiline: "One word or phrase per entry: no line breaks.",
  duplicate: "That word is already in the dictionary.",
};

/** Personal dictionary manager (#99): a searchable, sortable, paged list
 *  with add, in-place edit, delete, bulk delete, dedupe, import and export.
 *  "Edit as text" keeps the one-per-line textarea for quick bulk edits. */
export function DictionaryManager({ ctl }: { ctl: Ctl }) {
  const { settings, save } = ctl;
  const words = settings.dictionary;

  const [query, setQuery] = useState("");
  const deferredQuery = useDeferredValue(query);
  const [sort, setSort] = useState<ListSort>("added");
  const [page, setPage] = useState(0);
  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [adding, setAdding] = useState("");
  const [editing, setEditing] = useState<{ index: number; value: string; error?: string } | null>(null);
  const [asText, setAsText] = useState(false);

  const rows = useMemo(() => dictionaryRows(words, deferredQuery, sort), [words, deferredQuery, sort]);
  const duplicates = useMemo(() => dictionaryDuplicateCount(words), [words]);
  const shown = pageOf(rows, page);

  // A new search or sort starts from the first page; any change to the list
  // drops the selection (indices move when entries are deleted).
  useEffect(() => setPage(0), [deferredQuery, sort]);
  useEffect(() => setSelected(new Set()), [words]);

  /** The last removal, until the next change: deletes have no confirmation,
   *  so they can be undone instead. */
  const [undo, setUndo] = useState<{ message: string; previous: string[]; after: string } | null>(null);

  /** Save the new list; `removed` names a removal that Undo can revert. */
  const commit = async (dictionary: string[], removed?: string) => {
    const previous = words;
    const ok = await save({ ...settings, dictionary });
    if (ok) setUndo(removed ? { message: removed, previous, after: dictionary.join("\n") } : null);
    return ok;
  };

  const add = async (text: string) => {
    const r = addDictionaryWords(words, text);
    if (r.added === 0 && r.present === 0) return;
    if (r.added > 0 && !(await commit(r.merged))) return;
    setAdding("");
    ctl.flash(describeDictionaryMerge(r), 3000);
  };

  const saveEdit = async () => {
    if (!editing) return;
    if (editing.value.trim() === words[editing.index]) {
      setEditing(null);
      return;
    }
    const r = editDictionaryWord(words, editing.index, editing.value);
    if ("error" in r) {
      setEditing({ ...editing, error: EDIT_ERRORS[r.error] });
      return;
    }
    if (await commit(r.words)) setEditing(null);
  };

  const remove = async (indices: Iterable<number>) => {
    const list = [...indices];
    if (list.length === 0) return;
    await commit(removeIndices(words, list), `Removed ${list.length === 1 ? `“${words[list[0]]}”` : `${fmtCount(list.length)} words`}.`);
  };

  const dedupe = async () => {
    const r = dedupeDictionary(words);
    if (r.removed > 0) await commit(r.words, `Removed ${fmtCount(r.removed)} duplicate or blank ${r.removed === 1 ? "entry" : "entries"}.`);
  };

  const pageIndices = shown.rows.map((r) => r.index);
  const allOnPage = pageIndices.length > 0 && pageIndices.every((i) => selected.has(i));
  const toggle = (i: number) => {
    const next = new Set(selected);
    if (next.has(i)) next.delete(i);
    else next.add(i);
    setSelected(next);
  };
  const togglePage = () => {
    const next = new Set(selected);
    for (const i of pageIndices) {
      if (allOnPage) next.delete(i);
      else next.add(i);
    }
    setSelected(next);
  };

  return (
    <div className="field field-col lm">
      <div className="field-label">
        <span>
          Personal dictionary{" "}
          <Tip text="Names, brands and jargon the models tend to misspell (e.g. Sussurro, Tauri). One word or phrase per entry. They are fed to Whisper as recognition hints and to the LLM as preferred spellings." />
        </span>
        <small>names & jargon, one per entry — biases both Whisper and the LLM</small>
      </div>

      <div className="lm-bar">
        <span className="lm-count" aria-live="polite">
          {fmtCount(words.length)} {words.length === 1 ? "word" : "words"}
          {deferredQuery.trim() && ` · ${fmtCount(rows.length)} shown`}
          {duplicates > 0 && ` · ${fmtCount(duplicates)} duplicate${duplicates === 1 ? "" : "s"}`}
        </span>
        <span className="lm-bar-actions">
          {duplicates > 0 && !asText && (
            <button type="button" className="btn-ghost" onClick={dedupe} title="Keep the first of each word (case-insensitive) and drop blank entries">
              Remove duplicates
            </button>
          )}
          <button type="button" className="btn-ghost" onClick={() => importDictionary(ctl)} title="Add words from a .txt file (one per line); existing words are kept">
            Import .txt
          </button>
          <button type="button" className="btn-ghost" onClick={() => exportDictionary(ctl)} title="Save the dictionary as a .txt file, one entry per line (the format Import reads)">
            Export .txt
          </button>
          <button type="button" className="btn-ghost" aria-pressed={asText} onClick={() => setAsText(!asText)}>
            {asText ? "Show list" : "Edit as text"}
          </button>
        </span>
      </div>

      {asText ? (
        <DictionaryText ctl={ctl} />
      ) : (
        <>
          <div className="lm-controls">
            <input
              type="search"
              className="lm-search"
              placeholder="Search the dictionary"
              aria-label="Search the dictionary"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              spellCheck={false}
            />
            <SortSelect value={sort} onChange={setSort} label="Sort the dictionary" />
          </div>

          <form
            className="lm-add"
            onSubmit={(e) => {
              e.preventDefault();
              add(adding);
            }}
          >
            <input
              placeholder="Add a word or phrase (paste several lines to add many)"
              aria-label="New dictionary word"
              value={adding}
              onChange={(e) => setAdding(e.target.value)}
              onPaste={(e) => {
                const text = e.clipboardData.getData("text");
                if (/[\r\n]/.test(text.trim())) {
                  e.preventDefault();
                  add(text);
                }
              }}
              spellCheck={false}
            />
            <button type="submit" className="btn-ghost" disabled={!adding.trim()}>
              Add
            </button>
          </form>

          {undo && undo.after === words.join("\n") && <UndoBar message={undo.message} onUndo={() => commit(undo.previous)} />}

          {selected.size > 0 && (
            <div className="lm-selection" role="region" aria-label="Selection">
              <span>{fmtCount(selected.size)} selected</span>
              <button type="button" className="btn-ghost lm-danger" onClick={() => remove(selected)}>
                Delete selected
              </button>
              <button type="button" className="link-btn" onClick={() => setSelected(new Set())}>
                Clear selection
              </button>
            </div>
          )}

          {words.length === 0 ? (
            <p className="sh-muted lm-empty">No words yet. Add one above, or import a .txt file.</p>
          ) : rows.length === 0 ? (
            <p className="sh-muted lm-empty">No word matches “{deferredQuery.trim()}”.</p>
          ) : (
            <ul className="lm-list" aria-label="Dictionary words">
              <li className="lm-row lm-head">
                <input type="checkbox" checked={allOnPage} onChange={togglePage} aria-label="Select all words on this page" />
                <span className="lm-head-label">Word</span>
              </li>
              {shown.rows.map((r) => (
                <li key={r.index} className="lm-row">
                  <input
                    type="checkbox"
                    checked={selected.has(r.index)}
                    onChange={() => toggle(r.index)}
                    aria-label={`Select “${r.word}”`}
                  />
                  {editing?.index === r.index ? (
                    <span className="lm-edit">
                      <input
                        autoFocus
                        aria-label={`Edit “${r.word}”`}
                        aria-invalid={editing.error ? true : undefined}
                        aria-describedby={editing.error ? `lm-dict-err-${r.index}` : undefined}
                        value={editing.value}
                        onChange={(e) => setEditing({ index: r.index, value: e.target.value })}
                        onKeyDown={(e) => {
                          if (e.key === "Enter") {
                            e.preventDefault();
                            saveEdit();
                          } else if (e.key === "Escape") {
                            e.preventDefault();
                            setEditing(null);
                          }
                        }}
                        spellCheck={false}
                      />
                      <button type="button" className="btn-ghost" onClick={saveEdit}>Save</button>
                      <button type="button" className="link-btn" onClick={() => setEditing(null)}>Cancel</button>
                      {editing.error && (
                        <small id={`lm-dict-err-${r.index}`} className="lm-error" role="alert">
                          {editing.error}
                        </small>
                      )}
                    </span>
                  ) : (
                    <>
                      <span className="lm-word">
                        {r.word}
                        {r.duplicate && <span className="lm-badge" title="Repeats an earlier entry">duplicate</span>}
                      </span>
                      <span className="lm-row-actions">
                        <button type="button" className="link-btn" onClick={() => setEditing({ index: r.index, value: r.word })} aria-label={`Edit “${r.word}”`}>
                          Edit
                        </button>
                        <button type="button" className="link-btn lm-danger" onClick={() => remove([r.index])} aria-label={`Delete “${r.word}”`}>
                          Delete
                        </button>
                      </span>
                    </>
                  )}
                </li>
              ))}
            </ul>
          )}
          <Pager page={shown.page} pages={shown.pages} total={rows.length} onPage={setPage} label="Dictionary pages" />
        </>
      )}
    </div>
  );
}

/** The dictionary as one-per-line text (the original editor). The parsed
 *  list round-trips losslessly only when every line is non-empty —
 *  deriving the displayed text from split/trim/join swallowed typed
 *  newlines (issue #88) — so the textarea keeps its own raw text and
 *  resyncs from settings when the dictionary changes elsewhere. */
function DictionaryText({ ctl }: { ctl: Ctl }) {
  const { settings, setSettings, save } = ctl;
  const dictionaryKey = settings.dictionary.join("\n");
  const [text, setText] = useState(dictionaryKey);
  const ref = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    // Resync only when not focused, so external updates show up without
    // fighting the user's cursor.
    if (document.activeElement !== ref.current) setText(dictionaryKey);
  }, [dictionaryKey]);

  return (
    <textarea
      ref={ref}
      rows={Math.min(14, Math.max(4, settings.dictionary.length + 1))}
      value={text}
      aria-label="Personal dictionary, one entry per line"
      onChange={(e) => {
        setText(e.target.value);
        setSettings({
          ...settings,
          dictionary: e.target.value.split("\n").map((w) => w.trim()).filter(Boolean),
        });
      }}
      onBlur={() => save(settings)}
      spellCheck={false}
      placeholder={"Sussurro\nTauri"}
    />
  );
}

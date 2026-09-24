import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { save as saveDialog } from "@tauri-apps/plugin-dialog";
import { CollapsibleCard, Tip } from "../components/ui";
import { LANGUAGES } from "../lib/constants";
import { fmtCount } from "../lib/format";
import type { HistoryEntry } from "../lib/types";
import type { CardProps } from "./DictationCard";

/** Dictation history (JSONL): search, retention, copy / re-clean / translate /
 *  correct an entry. */
export function HistoryCard({ ctl, collapsible }: CardProps) {
  const { settings, save, history, setBusy, stats } = ctl;
  const [confirmClear, setConfirmClear] = useState(false);
  /** timestamp of the history entry being edited, and its draft text */
  const [editing, setEditing] = useState<{ ts: string; draft: string } | null>(null);
  const [historyQuery, setHistoryQuery] = useState("");
  const [searchResults, setSearchResults] = useState<HistoryEntry[] | null>(null);

  const runHistorySearch = async (query: string) => {
    setHistoryQuery(query);
    if (!query.trim()) {
      setSearchResults(null);
      return;
    }
    try {
      setSearchResults(await invoke<HistoryEntry[]>("search_history", { query, n: 50 }));
    } catch {
      setSearchResults(null);
    }
  };

  const clearHistory = async () => {
    if (!confirmClear) {
      setConfirmClear(true);
      setTimeout(() => setConfirmClear(false), 3000);
      return;
    }
    setConfirmClear(false);
    try {
      await invoke("clear_history");
      ctl.setHistory([]);
    } catch (e) {
      setBusy(String(e));
    }
  };

  return (
    <CollapsibleCard
      storageKey="historyOpen"
      className="card history"
      title="History"
      collapsible={collapsible}
      headerExtra={
        history.length > 0 ? (
          <>
            <button
              className="btn-ghost"
              title="Export the whole history to Markdown or JSON"
              onClick={async (e) => {
                // Inside <summary>: don't let the click toggle the accordion.
                e.preventDefault();
                e.stopPropagation();
                const path = await saveDialog({
                  defaultPath: "sussurro-history.md",
                  filters: [
                    { name: "Markdown", extensions: ["md"] },
                    { name: "JSON", extensions: ["json"] },
                  ],
                });
                if (!path) return;
                try {
                  const msg = await invoke<string>("export_history", { path });
                  ctl.flash(msg, 3000);
                } catch (err) {
                  setBusy(String(err));
                }
              }}
            >
              Export
            </button>
            <button
              className={`btn-ghost${confirmClear ? " danger" : ""}`}
              onClick={(e) => {
                e.preventDefault();
                e.stopPropagation();
                clearHistory();
              }}
            >
              {confirmClear ? "Click again to delete" : "Clear"}
            </button>
          </>
        ) : undefined
      }
    >
      {stats && stats.total_dictations > 0 && (
        <p
          className="stats-row"
          title="Counted since install — pruning or clearing the history doesn't reset these numbers"
        >
          <strong>{fmtCount(stats.total_dictations)}</strong> dictations ·{" "}
          <strong>{fmtCount(stats.total_words)}</strong> words
          {" — today "}
          <strong>{fmtCount(stats.today_words)}</strong>
          {" · last 7 days "}
          <strong>{fmtCount(stats.week_words)}</strong>
        </p>
      )}
      <div className="field">
        <div className="field-label">
          <span>Search & retention <Tip text="Search the WHOLE history (raw and cleaned text, case-insensitive). Retention auto-deletes entries older than the chosen window — a privacy tool: what you dictated last month doesn't need to live on disk forever." /></span>
          <small>{searchResults !== null ? `${searchResults.length} matches` : "full-text, whole history"}</small>
        </div>
        <div className="model-row">
          <input
            type="search"
            placeholder="search dictations…"
            aria-label="Search dictations"
            value={historyQuery}
            onChange={(e) => runHistorySearch(e.target.value)}
            spellCheck={false}
          />
          <select
            value={settings.history_retention_days}
            onChange={(e) =>
              save({ ...settings, history_retention_days: Number(e.target.value) })
            }
            title="Auto-delete entries older than"
            aria-label="Keep history for"
          >
            <option value={0}>Keep forever</option>
            <option value={7}>7 days</option>
            <option value={30}>30 days</option>
            <option value={90}>90 days</option>
          </select>
        </div>
      </div>
      {history.length === 0 && searchResults === null && (
        <p className="empty">Nothing yet. Hold the shortcut and speak.</p>
      )}
      {searchResults !== null && searchResults.length === 0 && (
        <p className="empty">No matches for “{historyQuery}”.</p>
      )}
      <ol>
        {(searchResults ?? history).map((h) => (
          <li key={h.timestamp}>
            <div className="entry-head">
              <time>{new Date(h.timestamp).toLocaleTimeString()}</time>
              <span className="entry-actions">
                <button
                  className="btn-ghost"
                  onClick={async () => {
                    await invoke("copy_text", { text: h.cleaned });
                  }}
                >
                  Copy
                </button>
                <button
                  className="btn-ghost"
                  title="Clean the raw transcript again with the current level"
                  onClick={async () => {
                    setBusy("Re-cleaning…");
                    try {
                      await invoke("reclean", { raw: h.raw });
                      setBusy("");
                      ctl.refresh();
                    } catch (e) {
                      setBusy(String(e));
                    }
                  }}
                >
                  Re-clean
                </button>
                <select
                  className="btn-ghost translate-select"
                  value=""
                  title="Translate this entry into another language (adds a new entry)"
                  aria-label="Translate this entry"
                  onChange={async (e) => {
                    const lang = e.target.value;
                    e.target.value = "";
                    if (!lang) return;
                    setBusy("Translating…");
                    try {
                      await invoke("translate_entry", { raw: h.raw, lang });
                      setBusy("");
                      ctl.refresh();
                    } catch (err) {
                      setBusy(String(err));
                    }
                  }}
                >
                  <option value="" disabled>
                    Translate…
                  </option>
                  {LANGUAGES.filter(([code]) => code !== "auto").map(([code, label]) => (
                    <option key={code} value={code}>{label}</option>
                  ))}
                </select>
                <button
                  className="btn-ghost"
                  title="Correct the text — new words are added to your dictionary"
                  onClick={() => setEditing({ ts: h.timestamp, draft: h.cleaned })}
                >
                  Edit
                </button>
              </span>
            </div>
            {editing?.ts === h.timestamp ? (
              <div className="edit-box">
                <textarea
                  rows={3}
                  value={editing.draft}
                  onChange={(e) => setEditing({ ts: h.timestamp, draft: e.target.value })}
                  autoFocus
                />
                <div className="edit-actions">
                  <button
                    className="btn-primary"
                    onClick={async () => {
                      try {
                        const learned = await invoke<string[]>("learn_correction", {
                          raw: h.raw,
                          original: h.cleaned,
                          corrected: editing.draft,
                        });
                        setEditing(null);
                        setBusy(
                          learned.length > 0
                            ? `Learned: ${learned.join(", ")} → added to your dictionary`
                            : ""
                        );
                        ctl.refresh();
                      } catch (e) {
                        setBusy(String(e));
                      }
                    }}
                  >
                    Save correction
                  </button>
                  <button className="btn-ghost" onClick={() => setEditing(null)}>
                    Cancel
                  </button>
                </div>
              </div>
            ) : (
              <p className="cleaned">{h.cleaned}</p>
            )}
            {h.cleaned !== h.raw && <p className="raw">{h.raw}</p>}
          </li>
        ))}
      </ol>
    </CollapsibleCard>
  );
}

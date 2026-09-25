import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Ctl } from "../hooks/useAppController";
import { fileManagerName } from "../lib/format";
import {
  activeCount,
  clearAll,
  clearTypes,
  loadFacetState,
  localToday,
  saveFacetState,
  toFilters,
  toggleType,
  type Facets,
  type FacetedSearch,
  type FacetState,
} from "../lib/facets";
import { itemSubtitle, matchesQuery, splitHighlights, TYPE_FILTERS, TYPE_LABEL } from "../lib/library";
import { externalHostsTitle, sentExternally } from "../lib/privacy";
import { audioBadge } from "../lib/audio";
import type { ItemSummary } from "../lib/types";
import { DocumentPane } from "./DocumentPane";
import { LibraryFacets } from "./LibraryFacets";
import type { SectionId } from "./SettingsScreen";

/** Debounced copy of a value (search as you type without a query per key). */
function useDebounced<T>(value: T, ms: number): T {
  const [v, setV] = useState(value);
  useEffect(() => {
    const t = setTimeout(() => setV(value), ms);
    return () => clearTimeout(t);
  }, [value, ms]);
  return v;
}

export function LibraryScreen({
  ctl,
  selectedId,
  onSelect,
  version,
  onChanged,
  onCount,
  onNew,
  onOpenSettings,
}: {
  ctl: Ctl;
  selectedId: string | null;
  onSelect: (id: string | null) => void;
  /** Bumped when items change elsewhere (a run finished, an edit). */
  version: number;
  onChanged: () => void;
  onCount: (n: number) => void;
  onNew: () => void;
  /** Open a Settings section (the calendar panel links to its settings). */
  onOpenSettings?: (s: SectionId) => void;
}) {
  const [query, setQuery] = useState("");
  /** Type chips + facets (#135), remembered across screens and restarts. */
  const [sel, setSelState] = useState<FacetState>(loadFacetState);
  const setSel = (s: FacetState) => {
    setSelState(s);
    saveFacetState(s);
  };
  const [facets, setFacets] = useState<Facets | null>(null);
  const [items, setItems] = useState<ItemSummary[] | null>(null);
  /** Items in the whole archive, to tell "empty archive" from "no match". */
  const [total, setTotal] = useState<number | null>(null);
  const [archiveDir, setArchiveDir] = useState("");
  const [indexDown, setIndexDown] = useState(false);
  const q = useDebounced(query, 200);

  useEffect(() => {
    invoke<string>("archive_dir").then(setArchiveDir).catch(() => setArchiveDir(""));
  }, [ctl.settings.archive_dir]);

  useEffect(() => {
    let stale = false;
    (async () => {
      const all = await invoke<ItemSummary[]>("archive_list").catch(() => null);
      if (stale) return;
      if (all) {
        setTotal(all.length);
        onCount(all.length);
      }
      try {
        const found = await invoke<FacetedSearch>("archive_facets", { query: q, filters: toFilters(sel, localToday()) });
        if (stale) return;
        setIndexDown(false);
        setItems(found.items);
        setFacets(found.facets);
      } catch {
        // Index unavailable: fall back to the folder scan, filtered here by
        // type and text only (the facets need the index).
        if (stale) return;
        setIndexDown(true);
        setFacets(null);
        setItems((all ?? []).filter((i) => (!sel.types.length || sel.types.includes(i.meta.type)) && matchesQuery(i, q)));
      }
    })();
    return () => {
      stale = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [q, sel, version, ctl.settings.archive_dir]);

  const revealArchive = () =>
    invoke("archive_reveal", { id: null }).catch((e) => ctl.setBusy(String(e)));

  const archiveEmpty = total === 0;
  const selected = selectedId;

  return (
    <div className="sh-screen">
      <header className="sh-topbar">
        <h1>Library</h1>
        <span className="sh-muted sh-ellipsis" title={archiveDir}>{archiveDir}</span>
        <div className="sh-topbar-right">
          <button type="button" className="btn-ghost sh-btn" onClick={revealArchive}>
            Show folder
          </button>
        </div>
      </header>
      <div className="lib-body">
        <div className="lib" aria-label="Items">
          <div className="lib-filters">
            <input
              type="search"
              className="lib-search"
              placeholder="Search title, text, tags…"
              aria-label="Search the library"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              spellCheck={false}
            />
            <div className="fchips" role="group" aria-label="Item type">
              {TYPE_FILTERS.map((f) => {
                const on = f.value === "all" ? sel.types.length === 0 : sel.types.includes(f.value);
                const n = f.value === "all" ? null : facets?.types.find((t) => t.key === f.value)?.count;
                return (
                  <button
                    key={f.value}
                    type="button"
                    aria-pressed={on}
                    className={`fchip${on ? " on" : ""}`}
                    onClick={() => setSel(f.value === "all" ? clearTypes(sel) : toggleType(sel, f.value))}
                  >
                    {f.label}
                    {n != null && <span className="fchip-n">{n}</span>}
                  </button>
                );
              })}
            </div>
            <LibraryFacets state={sel} facets={facets} onChange={setSel} disabled={indexDown} />
            {indexDown && (
              <p className="sh-note">
                Search index unavailable — matching titles and tags only{activeCount(sel) > sel.types.length ? "; the other filters are off" : ""}.
              </p>
            )}
          </div>
          <div className="lib-list" role="list">
            {items === null && <p className="sh-note pad">Loading…</p>}
            {items !== null && items.length === 0 && !archiveEmpty && (
              <div className="sh-note pad">
                <p>
                  No items match{query.trim() ? ` “${query.trim()}”` : ""}
                  {activeCount(sel) ? " with these filters" : ""}.
                </p>
                {activeCount(sel) > 0 && (
                  <button type="button" className="btn-ghost sh-btn" onClick={() => setSel(clearAll())}>
                    Clear filters
                  </button>
                )}
              </div>
            )}
            {archiveEmpty && <p className="sh-note pad">Nothing here yet.</p>}
            {items?.map((it) => (
              <div role="listitem" key={it.id}>
                <button
                  type="button"
                  className={`lib-item${it.id === selected ? " active" : ""}`}
                  aria-current={it.id === selected ? "true" : undefined}
                  onClick={() => onSelect(it.id)}
                >
                  <b>{it.meta.title || "Untitled"}</b>
                  <span className="lib-sub">
                    <span className={`tb ${it.meta.type}`}>{TYPE_LABEL[it.meta.type] ?? it.meta.type}</span>
                    <span>{it.recording ? "recording now" : itemSubtitle(it.meta)}</span>
                    {it.recording && <span className="live-badge">● Recording</span>}
                    {it.interrupted && (
                      <span className="int-badge" title="The app stopped before this session was finished: it holds what was saved until then">
                        Interrupted
                      </span>
                    )}
                    {it.edited_externally && (
                      <span className="ext" title="transcript.md was changed outside Sussurro">✎ Edited outside</span>
                    )}
                    {sentExternally(it) && (
                      <span
                        className="ext sent-ext"
                        title={externalHostsTitle(it.external_hosts)}
                        aria-label={externalHostsTitle(it.external_hosts)}
                      >
                        ↗ Sent externally
                      </span>
                    )}
                    {audioBadge(it) && (
                      <span className="audio-badge" title="Saved audio in the item folder (size on disk)">
                        {audioBadge(it)}
                      </span>
                    )}
                  </span>
                  {it.snippet && (
                    <span className="lib-snippet">
                      {splitHighlights(it.snippet).map((r, i) => (r.hit ? <mark key={i}>{r.text}</mark> : <span key={i}>{r.text}</span>))}
                    </span>
                  )}
                </button>
              </div>
            ))}
          </div>
          <div className="lib-count">
            {items !== null && total !== null && (
              <>
                {items.length === total ? `${total} item${total === 1 ? "" : "s"}` : `${items.length} of ${total} items`}
              </>
            )}
          </div>
        </div>

        <div className="doc-wrap">
          {archiveEmpty ? (
            <div className="empty-state">
              <h2>Your archive is empty</h2>
              <p>
                Notes and transcriptions are saved as plain markdown files, one folder each, in
              </p>
              <p className="mono path-line" title={archiveDir}>{archiveDir || "Documents/Sussurro"}</p>
              <p className="sh-muted">
                They are yours: open them with any editor, sync them, back them up. Record a note or transcribe a
                file from <strong>New</strong>; hotkey dictations stay in the dictation history.
              </p>
              <div className="row-gap">
                <button type="button" className="btn-dark" onClick={onNew}>New…</button>
                <button type="button" className="btn-ghost sh-btn" onClick={revealArchive}>
                  Reveal in {fileManagerName()}
                </button>
              </div>
            </div>
          ) : selected ? (
            <DocumentPane
              key={selected}
              ctl={ctl}
              id={selected}
              version={version}
              onChanged={onChanged}
              onOpenSettings={onOpenSettings}
              onDeleted={() => {
                onSelect(null);
                onChanged();
              }}
            />
          ) : (
            <div className="empty-state quiet">
              <p className="sh-muted">Select an item to read and edit its transcript.</p>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

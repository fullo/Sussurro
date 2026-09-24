import { useEffect, useId, useRef, useState } from "react";
import {
  activeCount,
  clearAll,
  clearFacet,
  DATE_BUCKETS,
  facetCount,
  facetSummary,
  filterValues,
  LIST_FACETS,
  setDate,
  toggleValue,
  validRange,
  withSelected,
  type DateBucket,
  type Facets,
  type FacetState,
  type FacetValue,
  type ListFacet,
} from "../lib/facets";

type FacetName = ListFacet | "date";

/** Window width below which the facets collapse into a "Filters" drawer:
 *  the list column is 224 px there (the `max-width: 1000px` rules in
 *  shell.css), too narrow for four chips. */
const COLLAPSE_QUERY = "(max-width: 1000px)";

function useCollapsed(): boolean {
  const query = () => typeof window !== "undefined" && !!window.matchMedia?.(COLLAPSE_QUERY).matches;
  const [narrow, setNarrow] = useState(query);
  useEffect(() => {
    const mq = window.matchMedia?.(COLLAPSE_QUERY);
    if (!mq) return;
    const on = () => setNarrow(mq.matches);
    on();
    mq.addEventListener("change", on);
    return () => mq.removeEventListener("change", on);
  }, []);
  return narrow;
}

/** Show a value filter above lists longer than this. */
const FILTERABLE = 8;

/** Checkbox list of one multi-select facet. */
function ValueList({
  facet,
  state,
  values,
  onChange,
  autoFocus,
}: {
  facet: ListFacet;
  state: FacetState;
  values: FacetValue[];
  onChange: (s: FacetState) => void;
  autoFocus?: boolean;
}) {
  const [filter, setFilter] = useState("");
  const all = withSelected(values, state[facet], state.labels, facet);
  const shown = filterValues(all, filter);
  const noun = LIST_FACETS.find((f) => f.value === facet)?.noun ?? facet;
  const first = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (autoFocus) first.current?.focus();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  if (!all.length) return <p className="facet-empty">No {noun === "category" ? "categories" : `${noun}s`} in these items.</p>;
  return (
    <>
      {all.length > FILTERABLE && (
        <input
          ref={first}
          type="search"
          className="facet-find"
          placeholder={`Find a ${noun}…`}
          aria-label={`Find a ${noun}`}
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          spellCheck={false}
        />
      )}
      <ul className="facet-values" aria-label={`${noun} values`}>
        {shown.map((v, i) => {
          const on = state[facet].includes(v.key);
          return (
            <li key={v.key}>
              <label className={`facet-opt${on ? " on" : ""}${v.count === 0 && !on ? " zero" : ""}`}>
                <input
                  ref={i === 0 && all.length <= FILTERABLE ? first : undefined}
                  type="checkbox"
                  checked={on}
                  onChange={() => onChange(toggleValue(state, facet, v.key, v.label))}
                />
                <span className="facet-label">
                  {v.label}
                  {v.key.startsWith("person:") && <span className="facet-person" title="In People"> ●</span>}
                </span>
                <span className="facet-n" aria-label={`${v.count} item${v.count === 1 ? "" : "s"}`}>{v.count}</span>
              </label>
            </li>
          );
        })}
        {!shown.length && <li className="facet-empty">No match.</li>}
      </ul>
    </>
  );
}

/** Date buckets (one at a time) and a custom range. */
function DatePicker({
  state,
  values,
  onChange,
  autoFocus,
}: {
  state: FacetState;
  values: FacetValue[];
  onChange: (s: FacetState) => void;
  autoFocus?: boolean;
}) {
  const name = useId();
  const current = state.date;
  const custom = !!current && !("bucket" in current);
  const [from, setFrom] = useState(custom ? (current as { from: string }).from : "");
  const [to, setTo] = useState(custom ? (current as { to: string }).to : "");
  const [showCustom, setShowCustom] = useState(custom);
  const first = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (autoFocus) first.current?.focus();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  const count = (b: DateBucket) => values.find((v) => v.key === b)?.count ?? 0;
  const applyRange = (f: string, t: string) => {
    setFrom(f);
    setTo(t);
    if (validRange(f, t)) onChange(setDate(state, { from: f, to: t }));
  };
  const rangeBad = showCustom && (from !== "" || to !== "") && !validRange(from, to);
  return (
    <div role="radiogroup" aria-label="Date" className="facet-values">
      <label className={`facet-opt${!current && !showCustom ? " on" : ""}`}>
        <input
          ref={first}
          type="radio"
          name={name}
          checked={!current && !showCustom}
          onChange={() => {
            setShowCustom(false);
            onChange(setDate(state, null));
          }}
        />
        <span className="facet-label">Any date</span>
      </label>
      {DATE_BUCKETS.map((b) => {
        const on = !!current && "bucket" in current && current.bucket === b.value;
        return (
          <label key={b.value} className={`facet-opt${on ? " on" : ""}${count(b.value) === 0 && !on ? " zero" : ""}`}>
            <input
              type="radio"
              name={name}
              checked={on}
              onChange={() => {
                setShowCustom(false);
                onChange(setDate(state, { bucket: b.value }));
              }}
            />
            <span className="facet-label">{b.label}</span>
            <span className="facet-n" aria-label={`${count(b.value)} items`}>{count(b.value)}</span>
          </label>
        );
      })}
      <label className={`facet-opt${showCustom ? " on" : ""}`}>
        <input
          type="radio"
          name={name}
          checked={showCustom}
          onChange={() => {
            setShowCustom(true);
            if (validRange(from, to)) onChange(setDate(state, { from, to }));
          }}
        />
        <span className="facet-label">Custom range…</span>
      </label>
      {showCustom && (
        <div className="facet-range">
          <label>
            <span>From</span>
            <input type="date" value={from} max={to || undefined} onChange={(e) => applyRange(e.target.value, to)} />
          </label>
          <label>
            <span>To</span>
            <input type="date" value={to} min={from || undefined} onChange={(e) => applyRange(from, e.target.value)} />
          </label>
          {rangeBad && <p className="facet-empty" role="alert">The start must come before the end.</p>}
        </div>
      )}
    </div>
  );
}

function FacetBody({
  facet,
  state,
  facets,
  onChange,
  autoFocus,
}: {
  facet: FacetName;
  state: FacetState;
  facets: Facets | null;
  onChange: (s: FacetState) => void;
  autoFocus?: boolean;
}) {
  if (facet === "date") return <DatePicker state={state} values={facets?.dates ?? []} onChange={onChange} autoFocus={autoFocus} />;
  return <ValueList facet={facet} state={state} values={facets?.[facet] ?? []} onChange={onChange} autoFocus={autoFocus} />;
}

const FACET_NAMES: FacetName[] = ["tags", "categories", "participants", "date"];
const TITLES: Record<FacetName, string> = { tags: "Tags", categories: "Categories", participants: "Participants", date: "Date" };
const isSet = (s: FacetState, f: FacetName) => (f === "date" ? !!s.date : s[f].length > 0);

/** The Library's facet filters (#135): Tag · Category · Participant · Date
 *  as popovers (approved mock), collapsed into a "Filters" drawer on
 *  narrow windows. */
export function LibraryFacets({
  state,
  facets,
  onChange,
  disabled,
}: {
  state: FacetState;
  /** Counts for the current query; null while loading or when the index is down. */
  facets: Facets | null;
  onChange: (s: FacetState) => void;
  disabled?: boolean;
}) {
  const collapsed = useCollapsed();
  const [open, setOpen] = useState<FacetName | null>(null);
  const [drawer, setDrawer] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const triggers = useRef<Partial<Record<FacetName | "drawer", HTMLButtonElement | null>>>({});
  const popId = useId();

  const close = (focusBack = true) => {
    const which = drawer ? "drawer" : open;
    setOpen(null);
    setDrawer(false);
    if (focusBack && which) triggers.current[which]?.focus();
  };

  // Click outside closes the popover / drawer.
  useEffect(() => {
    if (!open && !drawer) return;
    const onDown = (e: MouseEvent) => {
      if (root.current && !root.current.contains(e.target as Node)) {
        setOpen(null);
        setDrawer(false);
      }
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [open, drawer]);

  // Switching layout closes whatever was open.
  useEffect(() => {
    setOpen(null);
    setDrawer(false);
  }, [collapsed]);

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape" && (open || drawer)) {
      e.stopPropagation();
      close();
    }
  };

  const n = facetCount(state);
  const any = activeCount(state) > 0;

  return (
    <div className="facets" ref={root} onKeyDown={onKeyDown}>
      {collapsed ? (
        <div className="fchips">
          <button
            type="button"
            ref={(el) => {
              triggers.current.drawer = el;
            }}
            className={`fchip facet${n ? " set" : ""}`}
            aria-haspopup="dialog"
            aria-expanded={drawer}
            aria-controls={drawer ? popId : undefined}
            disabled={disabled}
            onClick={() => setDrawer((d) => !d)}
          >
            Filters{n ? ` (${n})` : ""} ▾
          </button>
          {any && (
            <button type="button" className="fchip facet-clear" onClick={() => onChange(clearAll())}>
              Clear filters
            </button>
          )}
        </div>
      ) : (
        <div className="fchips" role="group" aria-label="Filters">
          {FACET_NAMES.map((f) => {
            const set = isSet(state, f);
            const values = f === "date" ? [] : (facets?.[f] ?? []);
            return (
              <span key={f} className="facet-trigger">
                <button
                  type="button"
                  ref={(el) => {
                    triggers.current[f] = el;
                  }}
                  className={`fchip facet${set ? " set" : ""}${open === f ? " open" : ""}`}
                  aria-haspopup="dialog"
                  aria-expanded={open === f}
                  aria-controls={open === f ? popId : undefined}
                  disabled={disabled}
                  onClick={() => setOpen((o) => (o === f ? null : f))}
                >
                  {facetSummary(state, f, values)} ▾
                </button>
                {set && (
                  <button
                    type="button"
                    className="fchip facet-x"
                    aria-label={`Clear the ${TITLES[f].toLowerCase()} filter`}
                    title="Clear"
                    onClick={() => onChange(clearFacet(state, f))}
                  >
                    ×
                  </button>
                )}
              </span>
            );
          })}
          {any && (
            <button type="button" className="fchip facet-clear" onClick={() => onChange(clearAll())}>
              Clear filters
            </button>
          )}
        </div>
      )}

      {open && !collapsed && (
        <div className="facet-pop" id={popId} role="dialog" aria-label={`Filter by ${TITLES[open].toLowerCase()}`}>
          <div className="facet-pop-head">
            <b>{TITLES[open]}</b>
            {isSet(state, open) && (
              <button type="button" className="facet-link" onClick={() => onChange(clearFacet(state, open))}>
                Clear
              </button>
            )}
          </div>
          <FacetBody key={open} facet={open} state={state} facets={facets} onChange={onChange} autoFocus />
          <p className="facet-hint">
            {open === "date" ? "One period at a time." : "Items with any of the checked values."} Esc closes.
          </p>
        </div>
      )}

      {drawer && collapsed && (
        <>
          <div className="facet-scrim" aria-hidden="true" onClick={() => close(false)} />
          <div className="facet-drawer" id={popId} role="dialog" aria-label="Filters">
            <div className="facet-pop-head">
              <b>Filters</b>
              <button type="button" className="facet-link" onClick={() => close()}>
                Done
              </button>
            </div>
            {FACET_NAMES.map((f, i) => (
              <section key={f} className="facet-sect" aria-label={TITLES[f]}>
                <div className="facet-pop-head">
                  <h3>{TITLES[f]}</h3>
                  {isSet(state, f) && (
                    <button type="button" className="facet-link" onClick={() => onChange(clearFacet(state, f))}>
                      Clear
                    </button>
                  )}
                </div>
                <FacetBody facet={f} state={state} facets={facets} onChange={onChange} autoFocus={i === 0} />
              </section>
            ))}
            {any && (
              <button type="button" className="btn-ghost sh-btn" onClick={() => onChange(clearAll())}>
                Clear filters
              </button>
            )}
          </div>
        </>
      )}
    </div>
  );
}

import { useState } from "react";
import { addChips, parseChipInput, removeChip } from "../lib/library";

/** Editable chips (tags, categories): × removes one; "+ tag" opens an input
 *  where Enter or a comma adds, Esc cancels. */
export function ChipEditor({
  label,
  values,
  onChange,
  addLabel = "+ add",
  disabled = false,
}: {
  label: string;
  values: string[];
  onChange: (values: string[]) => void;
  addLabel?: string;
  disabled?: boolean;
}) {
  const [draft, setDraft] = useState<string | null>(null);

  const commit = () => {
    if (draft === null) return;
    const next = addChips(values, parseChipInput(draft));
    setDraft(null);
    if (next.length !== values.length) onChange(next);
  };

  return (
    <div className="chips" role="group" aria-label={label}>
      {values.map((v) => (
        <span key={v} className="mtag">
          {v}
          {!disabled && (
            <button type="button" className="mtag-x" aria-label={`Remove ${v}`} onClick={() => onChange(removeChip(values, v))}>
              ×
            </button>
          )}
        </span>
      ))}
      {!disabled &&
        (draft === null ? (
          <button type="button" className="mtag add" onClick={() => setDraft("")}>
            {addLabel}
          </button>
        ) : (
          <input
            className="mtag-input"
            autoFocus
            value={draft}
            aria-label={`Add to ${label}`}
            placeholder="type, then Enter"
            onChange={(e) => {
              const v = e.target.value;
              if (v.endsWith(",")) {
                const next = addChips(values, parseChipInput(v));
                if (next.length !== values.length) onChange(next);
                setDraft("");
              } else setDraft(v);
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                commit();
              } else if (e.key === "Escape") {
                e.preventDefault();
                setDraft(null);
              }
            }}
            onBlur={commit}
          />
        ))}
    </div>
  );
}

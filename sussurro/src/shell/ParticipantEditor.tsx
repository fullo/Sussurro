import { useId, useState } from "react";
import {
  addParticipants,
  formatParticipant,
  parseParticipant,
  removeParticipant,
  splitParticipantInput,
  updateParticipant,
} from "../lib/participants";
import type { Participant } from "../lib/types";

/** Participant chips (#124): name plus email when known. "+ participant"
 *  opens an input ("Anna Rossi <anna@example.com>", a name, or an email;
 *  several separated by commas), clicking a name edits it, × removes it.
 *  Enter saves, Esc cancels.
 *
 *  `suggestions` is where the People registry (#132) plugs in: they are
 *  offered as the input's autocomplete list, and a picked one is parsed like
 *  typed text. */
export function ParticipantEditor({
  label,
  values,
  onChange,
  disabled = false,
  suggestions = [],
}: {
  label: string;
  values: Participant[];
  onChange: (values: Participant[]) => void;
  disabled?: boolean;
  suggestions?: Participant[];
}) {
  /** index null = adding a new one. */
  const [draft, setDraft] = useState<{ index: number | null; text: string } | null>(null);
  const [error, setError] = useState("");
  const listId = useId();
  const errId = useId();

  const close = () => {
    setDraft(null);
    setError("");
  };

  const commit = () => {
    if (!draft) return;
    const text = draft.text.trim();
    if (!text) return close();
    if (draft.index === null) {
      const parsed = splitParticipantInput(text).map(parseParticipant);
      const bad = parsed.find((r) => !r.ok);
      if (bad && !bad.ok) return setError(bad.error);
      const next = addParticipants(values, parsed.flatMap((r) => (r.ok ? [r.participant] : [])));
      close();
      if (JSON.stringify(next) !== JSON.stringify(values)) onChange(next);
      return;
    }
    const r = parseParticipant(text);
    if (!r.ok) return setError(r.error);
    const u = updateParticipant(values, draft.index, r.participant);
    if (!u.ok) return setError(u.error);
    close();
    if (JSON.stringify(u.list) !== JSON.stringify(values)) onChange(u.list);
  };

  const input = (index: number | null) => (
    <span className="pchip-edit">
      <input
        className="mtag-input pchip-input"
        autoFocus
        value={draft?.text ?? ""}
        list={suggestions.length ? listId : undefined}
        aria-label={index === null ? `Add to ${label}` : `Edit ${values[index]?.name ?? "participant"}`}
        aria-invalid={!!error}
        aria-describedby={error ? errId : undefined}
        placeholder="Name <email>, then Enter"
        onChange={(e) => {
          setDraft({ index, text: e.target.value });
          setError("");
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            commit();
          } else if (e.key === "Escape") {
            e.preventDefault();
            close();
          }
        }}
        onBlur={() => {
          // A typo stays open with its message instead of being lost.
          if (!error) commit();
        }}
      />
      {error && (
        <span id={errId} className="pchip-error" role="alert">
          {error}
        </span>
      )}
    </span>
  );

  return (
    <div className="chips" role="group" aria-label={label}>
      {values.map((p, i) =>
        draft?.index === i ? (
          <span key={`edit-${i}`}>{input(i)}</span>
        ) : (
          <span key={`${p.name}|${p.email ?? ""}|${i}`} className="mtag pchip">
            {disabled ? (
              <span className="pchip-name">{p.name}</span>
            ) : (
              <button
                type="button"
                className="pchip-name"
                title="Edit"
                aria-label={`Edit ${p.name}`}
                onClick={() => {
                  setError("");
                  setDraft({ index: i, text: formatParticipant(p) });
                }}
              >
                {p.name}
              </button>
            )}
            {p.email && <span className="pchip-em">· {p.email}</span>}
            {!disabled && (
              <button type="button" className="mtag-x" aria-label={`Remove ${p.name}`} onClick={() => onChange(removeParticipant(values, i))}>
                ×
              </button>
            )}
          </span>
        ),
      )}
      {!disabled &&
        (draft?.index === null ? (
          input(null)
        ) : (
          <button
            type="button"
            className="mtag add"
            onClick={() => {
              setError("");
              setDraft({ index: null, text: "" });
            }}
          >
            + participant
          </button>
        ))}
      {suggestions.length > 0 && (
        <datalist id={listId}>
          {suggestions.map((s) => (
            <option key={formatParticipant(s)} value={formatParticipant(s)} />
          ))}
        </datalist>
      )}
    </div>
  );
}

import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { CollapsibleCard } from "../components/ui";
import type { Ctl } from "../hooks/useAppController";
import {
  filterPeople,
  findDuplicates,
  mergePreview,
  parseAliases,
  personProblems,
  usageLabel,
} from "../lib/people";
import type { Person } from "../lib/types";

const NEW_PERSON: Person = { id: "", name: "", aliases: [] };

/** People (proposal A rail, #132): the registry that gives participants
 *  their email. Stored in the archive (`.sussurro/people.json`), so it
 *  travels with it; never sent anywhere. Not behind `meetings_enabled`:
 *  transcriptions have participants too (#124). */
export function PeopleScreen({ ctl }: { ctl: Ctl }) {
  const [people, setPeople] = useState<Person[] | null>(null);
  const [error, setError] = useState("");
  const [usage, setUsage] = useState<Record<string, number>>({});
  const [query, setQuery] = useState("");
  /** The person open in the editor: a saved one, or a new draft (id ""). */
  const [editing, setEditing] = useState<{ person: Person; mergeWith?: string } | null>(null);

  const load = useCallback(async () => {
    try {
      setPeople(await invoke<Person[]>("people_list"));
      setError("");
    } catch (e) {
      setError(String(e));
      setPeople(null);
      return;
    }
    // Counts come from the search index; a failure only hides them.
    invoke<Record<string, number>>("people_usage")
      .then(setUsage)
      .catch(() => setUsage({}));
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const shown = useMemo(() => filterPeople(people ?? [], query), [people, query]);
  const duplicates = useMemo(() => findDuplicates(people ?? []), [people]);

  const done = (changed: boolean) => {
    setEditing(null);
    if (changed) load();
  };

  return (
    <div className="sh-screen">
      <header className="sh-topbar">
        <h1>People</h1>
        <span className="sh-muted">Names, emails and aliases for participants</span>
      </header>
      <div className="sh-scroll cards-col">
        <CollapsibleCard
          storageKey="peopleList"
          title={<>People {people && people.length > 0 && <span className="via">{people.length}</span>}</>}
          collapsible={false}
          headerExtra={
            <button
              type="button"
              className="btn-ghost"
              disabled={!people || editing?.person.id === ""}
              onClick={() => setEditing({ person: NEW_PERSON })}
            >
              Add person
            </button>
          }
        >
          <p className="card-hint">
            When a participant's name matches a person's name or one of their aliases (ignoring case and accents),
            the participant gets that person's email. Add the spellings you see in meetings as aliases — a Meet
            display name, a nickname, a renamed voice. The list lives in your archive folder
            (<code>.sussurro/people.json</code>) on this computer and is never sent anywhere; deleting a person
            doesn't change items that already have their email.
          </p>

          {error && (
            <div className="notice-warn" role="alert">
              <strong>The People list can't be read.</strong> <span className="mono">{error}</span>
            </div>
          )}

          {people && people.length > 0 && (
            <input
              type="search"
              className="lib-search people-search"
              placeholder="Search names, aliases, emails"
              aria-label="Search people"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
            />
          )}

          {duplicates.length > 0 && !editing && (
            <div className="notice-warn people-dups" role="note">
              <strong>Possible duplicates.</strong>
              <ul>
                {duplicates.map((group) => {
                  // Keep the entry used most (then the one with most aliases).
                  const g = group.slice().sort((a, b) => (usage[b.id] ?? 0) - (usage[a.id] ?? 0) || b.aliases.length - a.aliases.length);
                  return (
                  <li key={g.map((p) => p.id).join("|")}>
                    {g.map((p) => p.name).join(" · ")}{" "}
                    <button
                      type="button"
                      className="link-btn"
                      onClick={() => setEditing({ person: g[0], mergeWith: g[1].id })}
                    >
                      Review and merge
                    </button>
                  </li>
                  );
                })}
              </ul>
            </div>
          )}

          {editing?.person.id === "" && (
            <PersonEditor ctl={ctl} initial={editing.person} people={people ?? []} usage={usage} onDone={done} />
          )}

          {people && people.length === 0 && !editing && (
            <p className="sh-muted">
              No one yet. Add people here, or use <em>+ People</em> on a participant chip in the Library.
            </p>
          )}
          {people && people.length > 0 && shown.length === 0 && <p className="sh-muted">No one matches “{query}”.</p>}

          {shown.length > 0 && (
            <ul className="prof-list" aria-label="People">
              {shown.map((p) => (
                <li key={p.id}>
                  <button
                    type="button"
                    className={`prof-row${editing?.person.id === p.id ? " active" : ""}`}
                    aria-expanded={editing?.person.id === p.id}
                    onClick={() => setEditing(editing?.person.id === p.id ? null : { person: p })}
                  >
                    <span className="prof-name">{p.name}</span>
                    <span className="prof-sub">
                      {[p.email ?? "no email", p.aliases.length ? `aka ${p.aliases.join(", ")}` : ""].filter(Boolean).join(" · ")}
                    </span>
                    <span className="people-usage">{usageLabel(usage[p.id])}</span>
                  </button>
                  {editing?.person.id === p.id && (
                    <PersonEditor
                      key={p.id}
                      ctl={ctl}
                      initial={p}
                      mergeWith={editing.mergeWith}
                      people={people ?? []}
                      usage={usage}
                      onDone={done}
                    />
                  )}
                </li>
              ))}
            </ul>
          )}
        </CollapsibleCard>
      </div>
    </div>
  );
}

function PersonEditor({
  ctl,
  initial,
  mergeWith,
  people,
  usage,
  onDone,
}: {
  ctl: Ctl;
  initial: Person;
  mergeWith?: string;
  people: Person[];
  usage: Record<string, number>;
  onDone: (changed: boolean) => void;
}) {
  const [name, setName] = useState(initial.name);
  const [email, setEmail] = useState(initial.email ?? "");
  const [aliases, setAliases] = useState(initial.aliases.join("\n"));
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [mergeId, setMergeId] = useState(mergeWith ?? "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  const isNew = initial.id === "";
  const draft: Person = { id: initial.id, name: name.trim(), email: email.trim() || undefined, aliases: parseAliases(aliases, name) };
  const problems = personProblems(draft, people);
  const dirty = JSON.stringify(draft) !== JSON.stringify({ ...initial, email: initial.email || undefined });
  const others = people.filter((p) => p.id !== initial.id);
  const mergeTarget = others.find((p) => p.id === mergeId) ?? null;
  const n = usage[initial.id];

  const run = async (f: () => Promise<unknown>, message: string) => {
    setBusy(true);
    setError("");
    try {
      await f();
      ctl.flash(message, 3000);
      onDone(true);
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };

  const save = () =>
    run(
      () => invoke(isNew ? "people_add" : "people_update", { person: draft }),
      isNew ? `${draft.name} added to People.` : "Saved.",
    );
  const remove = () => run(() => invoke("people_delete", { id: initial.id }), `${initial.name} removed from People.`);
  const merge = () =>
    mergeTarget &&
    run(
      () => invoke("people_merge", { into: initial.id, from: [mergeTarget.id] }),
      `${mergeTarget.name} merged into ${initial.name}.`,
    );

  return (
    <div className="prof-editor people-editor" role="group" aria-label={isNew ? "New person" : `Edit ${initial.name}`}>
      <div className="field">
        <div className="field-label"><span>Name</span></div>
        <input value={name} onChange={(e) => setName(e.target.value)} aria-label="Name" autoFocus={isNew} />
      </div>
      <div className="field">
        <div className="field-label">
          <span>Email</span>
          <small>optional</small>
        </div>
        <input
          type="email"
          value={email}
          onChange={(e) => setEmail(e.target.value)}
          aria-label="Email"
          spellCheck={false}
          autoComplete="off"
          placeholder="name@example.com"
        />
      </div>
      <div className="field field-col">
        <div className="field-label">
          <span>Aliases</span>
          <small>other spellings, one per line or separated by commas</small>
        </div>
        <textarea
          rows={3}
          value={aliases}
          onChange={(e) => setAliases(e.target.value)}
          aria-label="Aliases"
          placeholder={"Anna R.\nAnnie"}
        />
      </div>
      {!isNew && n !== undefined && <p className="sh-muted people-usage-line">{usageLabel(n)}</p>}

      {problems.length > 0 && (name.trim() || email.trim()) && (
        <ul className="prof-problems">
          {problems.map((p) => <li key={p}>{p}</li>)}
        </ul>
      )}
      {error && <p className="prof-test error" role="alert">{error}</p>}

      {confirmDelete ? (
        <div className="row-gap prof-actions" role="alertdialog" aria-label="Confirm delete">
          <span>
            Remove {initial.name} from People? Items that already list them keep their email; only future linking
            stops.
          </span>
          <button type="button" className="btn-danger sh-btn push" disabled={busy} onClick={remove}>Delete</button>
          <button type="button" className="btn-ghost" onClick={() => setConfirmDelete(false)}>Keep</button>
        </div>
      ) : (
        <div className="row-gap prof-actions">
          <button type="button" className="btn-dark sh-btn" disabled={busy || problems.length > 0 || (!isNew && !dirty)} onClick={save}>
            {isNew ? "Add" : "Save"}
          </button>
          <button type="button" className="btn-ghost" onClick={() => onDone(false)}>Cancel</button>
          {!isNew && (
            <button type="button" className="btn-ghost push" onClick={() => setConfirmDelete(true)}>Delete</button>
          )}
        </div>
      )}

      {!isNew && others.length > 0 && !confirmDelete && (
        <div className="people-merge">
          <div className="field">
            <div className="field-label">
              <span>Merge a duplicate</span>
              <small>into {initial.name}</small>
            </div>
            <select value={mergeId} onChange={(e) => setMergeId(e.target.value)} aria-label={`Merge into ${initial.name}`}>
              <option value="">Choose a person…</option>
              {others.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.email ? `${p.name} <${p.email}>` : p.name}
                </option>
              ))}
            </select>
          </div>
          {mergeTarget && (
            <div className="row-gap">
              <small className="sh-muted">
                {(() => {
                  const m = mergePreview(initial, [mergeTarget]);
                  return `Result: ${m.name}${m.email ? ` <${m.email}>` : ""}${m.aliases.length ? `, aka ${m.aliases.join(", ")}` : ""}. ${mergeTarget.name} is removed; items are not changed.`;
                })()}
              </small>
              <button type="button" className="btn-ghost push" disabled={busy || dirty} title={dirty ? "Save or cancel your edits first" : undefined} onClick={merge}>
                Merge
              </button>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

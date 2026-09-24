import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Ctl } from "../hooks/useAppController";
import {
  MAX_LABEL_CHARS,
  canAddSpeakerToPeople,
  identifyOffer,
  isVoice,
  labelProblem,
  linkChoices,
  linkSuggestion,
  linkedPerson,
  redetectBlocked,
  speakerShares,
  speakerSource,
} from "../lib/speakers";
import type { DocSpeaker, Item, Person, VoiceSource } from "../lib/types";

/** The context pane's *Speakers* section (#130): the document's speakers
 *  with colour and share of speech, rename for this document, link to a
 *  person, and "Re-detect speakers". Moving a line is on the line itself
 *  (transcript line actions). On a transcription without voice data it
 *  offers "Identify voices" from the original file, or says why it can't
 *  (#134). */
export function SpeakerPanel({
  ctl,
  item,
  onItem,
  people = [],
  onPeopleChanged,
}: {
  ctl: Ctl;
  item: Item;
  onItem: (item: Item) => void;
  /** The People registry (#132): link a speaker to a person. */
  people?: Person[];
  onPeopleChanged?: () => void;
}) {
  const [renaming, setRenaming] = useState<{ id: string; label: string } | null>(null);
  /** Speaker whose "Link to person…" picker is open. */
  const [picking, setPicking] = useState<string | null>(null);
  const [confirmRedetect, setConfirmRedetect] = useState(false);
  const [busy, setBusy] = useState(false);
  const shares = speakerShares(item);
  const editable = !item.recording && !item.edited_externally;
  const blocked = redetectBlocked(item);
  const id = item.id;
  const isTranscription = item.meta.type === "transcription";
  // "Identify voices" (#134): can the original file give the voices back?
  const [source, setSource] = useState<VoiceSource | null>(null);
  const wantsSource = isTranscription && !item.embedded_segments && !item.recording;
  useEffect(() => {
    setSource(null);
    if (!wantsSource) return;
    let alive = true;
    invoke<VoiceSource>("archive_voice_source", { id })
      .then((s) => alive && setSource(s))
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [id, wantsSource, item.edited_externally, item.meta.source]);
  const offer = identifyOffer(item, source);

  const call = async (cmd: string, args: Record<string, unknown>, done?: string) => {
    setBusy(true);
    try {
      const updated = await invoke<Item>(cmd, { id, ...args });
      onItem(updated);
      if (done) ctl.flash(done, 2500);
      return true;
    } catch (e) {
      ctl.setBusy(String(e));
      return false;
    } finally {
      setBusy(false);
    }
  };

  const saveName = async () => {
    if (!renaming) return;
    if (labelProblem(renaming.id, renaming.label)) return;
    const current = item.segments.speakers.find((s) => s.id === renaming.id)?.label ?? "";
    if (renaming.label.trim() === current) {
      setRenaming(null);
      return;
    }
    if (await call("archive_rename_speaker", { speakerId: renaming.id, label: renaming.label })) setRenaming(null);
  };

  const link = async (sp: DocSpeaker, person: Person) => {
    setPicking(null);
    await call("archive_link_speaker", { speakerId: sp.id, personId: person.id }, `${sp.label} linked to ${person.name}.`);
  };

  const unlink = (sp: DocSpeaker) => call("archive_unlink_speaker", { speakerId: sp.id });

  const addToPeople = async (sp: DocSpeaker) => {
    setBusy(true);
    let added: Person;
    try {
      added = await invoke<Person>("people_add", { person: { id: "", name: sp.label.trim(), aliases: [] } });
    } catch (e) {
      ctl.setBusy(String(e));
      setBusy(false);
      return;
    }
    setBusy(false);
    onPeopleChanged?.();
    await call("archive_link_speaker", { speakerId: sp.id, personId: added.id }, `${added.name} added to People.`);
  };

  const redetect = async () => {
    setConfirmRedetect(false);
    await call("archive_redetect_speakers", {}, "Speakers re-detected.");
  };

  const identify = () => call("archive_identify_voices", {}, "Voices identified.");

  const problem = renaming ? labelProblem(renaming.id, renaming.label) : "";

  return (
    <section className="ctx-sect" aria-labelledby="ctx-spk-h">
      <h3 id="ctx-spk-h">
        Speakers <span className="ctx-hint">this document</span>
      </h3>
      {shares.length === 0 ? (
        <p className="ctx-note">
          {isTranscription
            ? "No speakers yet: this transcription was made without Identify voices."
            : blocked && !item.recording
              ? blocked
              : "No speakers yet."}
        </p>
      ) : (
        <ul className="spk-list">
          {shares.map(({ speaker, percent, lines }) => (
            <li key={speaker.id} className="spk-row">
              {renaming?.id === speaker.id ? (
                <form
                  className="spk-rename"
                  onSubmit={(e) => {
                    e.preventDefault();
                    saveName();
                  }}
                >
                  <input
                    autoFocus
                    aria-label={`New name for ${speaker.label}`}
                    value={renaming.label}
                    maxLength={MAX_LABEL_CHARS + 20}
                    placeholder={isVoice(speaker.id) ? speaker.id.replace("voice:", "Voice ") : "Name"}
                    disabled={busy}
                    onChange={(e) => setRenaming({ id: speaker.id, label: e.target.value })}
                    onKeyDown={(e) => {
                      if (e.key === "Escape") {
                        e.preventDefault();
                        e.stopPropagation();
                        setRenaming(null);
                      }
                    }}
                  />
                  <button type="submit" className="btn-dark sh-btn" disabled={busy || !!problem}>
                    Save
                  </button>
                  <button type="button" className="btn-ghost sh-btn" onClick={() => setRenaming(null)} disabled={busy}>
                    Cancel
                  </button>
                  {problem && <p className="ctx-note warn">{problem}</p>}
                </form>
              ) : (
                <>
                  <span className="tx-chip" style={{ background: speaker.color || undefined }} title={speaker.label}>
                    <i aria-hidden="true" />
                    {speaker.label}
                  </span>
                  <span className="spk-src">{speakerSource(speaker.id)}</span>
                  <span className="spk-pct" title={`${lines} line${lines === 1 ? "" : "s"}`}>
                    {percent}%
                  </span>
                  {speaker.person_id && <LinkedTo speaker={speaker} people={people} />}
                  {editable && (
                    <SpeakerActions
                      speaker={speaker}
                      people={people}
                      busy={busy}
                      picking={picking === speaker.id}
                      onRename={() => setRenaming({ id: speaker.id, label: speaker.label })}
                      onPick={(open) => setPicking(open ? speaker.id : null)}
                      onLink={(p) => link(speaker, p)}
                      onUnlink={() => unlink(speaker)}
                      onAdd={() => addToPeople(speaker)}
                    />
                  )}
                </>
              )}
            </li>
          ))}
        </ul>
      )}
      {editable && item.embedded_segments ? (
        confirmRedetect ? (
          <div className="spk-confirm" role="group" aria-label="Confirm re-detect">
            <p className="ctx-note">
              Every line with voice data gets its voice again from the recording's voice prints — lines you moved by
              hand included. Names you gave to voices stay where the voice stays.
            </p>
            <div className="ctx-exports">
              <button type="button" className="btn-dark sh-btn" onClick={redetect} disabled={busy} autoFocus>
                Re-detect
              </button>
              <button type="button" className="btn-ghost sh-btn" onClick={() => setConfirmRedetect(false)} disabled={busy}>
                Cancel
              </button>
            </div>
          </div>
        ) : (
          <button type="button" className="btn-ghost sh-btn spk-redetect" onClick={() => setConfirmRedetect(true)} disabled={busy}>
            {busy ? "Working…" : "Re-detect speakers"}
          </button>
        )
      ) : (
        shares.length > 0 && blocked && <p className="ctx-note">{blocked}</p>
      )}
      {offer === "identify" && editable && (
        <div className="spk-identify">
          <p className="ctx-note">
            Tell the voices apart as Voice 1, Voice 2… from the original file
            {source?.file_name ? ` “${source.file_name}”` : ""}. The speaker model is downloaded on first use; a long
            file takes a while. Nothing leaves this computer.
          </p>
          <button type="button" className="btn-ghost sh-btn" onClick={identify} disabled={busy}>
            {busy ? "Identifying voices…" : "Identify voices"}
          </button>
        </div>
      )}
      {offer === "explain" && source && <p className="ctx-note">{source.reason}</p>}
    </section>
  );
}

/** "↔ Anna Rossi · anna@example.com" under a linked speaker (the name only
 *  when the label differs from it). */
function LinkedTo({ speaker, people }: { speaker: DocSpeaker; people: Person[] }) {
  const p = linkedPerson(speaker, people);
  if (!p) return <span className="spk-person">↔ a person no longer in People</span>;
  const parts = [p.name.trim() === speaker.label.trim() ? "" : p.name, p.email ?? ""].filter(Boolean);
  return <span className="spk-person">↔ {parts.length ? parts.join(" · ") : "in People"}</span>;
}

/** A speaker row's actions: rename, link to a person (a suggestion first),
 *  unlink, Add to People. */
function SpeakerActions({
  speaker,
  people,
  busy,
  picking,
  onRename,
  onPick,
  onLink,
  onUnlink,
  onAdd,
}: {
  speaker: DocSpeaker;
  people: Person[];
  busy: boolean;
  picking: boolean;
  onRename: () => void;
  onPick: (open: boolean) => void;
  onLink: (p: Person) => void;
  onUnlink: () => void;
  onAdd: () => void;
}) {
  const suggestion = linkSuggestion(speaker, people);
  const choices = linkChoices(speaker, people);
  return (
    <span className="spk-acts">
      <button type="button" className="link-btn" disabled={busy} onClick={onRename} aria-label={`Rename ${speaker.label} in this document`}>
        Rename
      </button>
      {speaker.person_id ? (
        <button type="button" className="link-btn" disabled={busy} onClick={onUnlink} aria-label={`Unlink ${speaker.label} from People`}>
          Unlink
        </button>
      ) : (
        <>
          {suggestion && (
            <button
              type="button"
              className="link-btn spk-suggest"
              disabled={busy}
              onClick={() => onLink(suggestion)}
              title={suggestion.email ? `${suggestion.name} <${suggestion.email}>` : suggestion.name}
            >
              Link to {suggestion.name}?
            </button>
          )}
          {picking ? (
            <select
              autoFocus
              aria-label={`Link ${speaker.label} to a person`}
              value=""
              disabled={busy}
              onChange={(e) => {
                const p = choices.find((c) => c.id === e.target.value);
                if (p) onLink(p);
              }}
              onBlur={() => onPick(false)}
              onKeyDown={(e) => {
                if (e.key === "Escape") {
                  e.stopPropagation();
                  onPick(false);
                }
              }}
            >
              <option value="">Choose a person…</option>
              {choices.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.email ? `${p.name} · ${p.email}` : p.name}
                </option>
              ))}
            </select>
          ) : (
            choices.length > 0 && (
              <button type="button" className="link-btn" disabled={busy} onClick={() => onPick(true)}>
                Link to person…
              </button>
            )
          )}
          {canAddSpeakerToPeople(speaker, people) && (
            <button type="button" className="link-btn" disabled={busy} onClick={onAdd} title={`Add ${speaker.label} to People and link this voice`}>
              Add to People
            </button>
          )}
        </>
      )}
    </span>
  );
}

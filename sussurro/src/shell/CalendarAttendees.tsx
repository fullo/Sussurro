import { useEffect, useId, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Ctl } from "../hooks/useAppController";
import {
  actionLabel,
  addedMessage,
  defaultPicks,
  matchMessage,
  noAttendeesMessage,
  overlapHint,
  pickable,
  pickedAttendees,
  timeRange,
  type AttendeesAdded,
  type CalendarLinkStatus,
  type CalendarMatch,
} from "../lib/calendar";
import type { Item } from "../lib/types";

/** "Add attendees from calendar…" (#252, P22): read an .ics file or the
 *  private ICS link saved in Settings → Calendar, pick the event (the one
 *  overlapping the recording comes first), tick the attendees, add them as
 *  participants. New attendees are ticked; completing the email of someone
 *  already listed without one is offered, never ticked by default; an
 *  existing email is never replaced. People entries are only suggested (the
 *  chips' "+ People"). */
export function CalendarAttendees({
  ctl,
  item,
  onItem,
  onClose,
  onOpenSettings,
}: {
  ctl: Ctl;
  item: Item;
  onItem: (item: Item) => void;
  onClose: () => void;
  /** Settings → Calendar, to save a link. */
  onOpenSettings?: () => void;
}) {
  const [link, setLink] = useState<CalendarLinkStatus | null>(null);
  const [busy, setBusy] = useState<"" | "file" | "link" | "add">("");
  const [error, setError] = useState("");
  const [match, setMatch] = useState<CalendarMatch | null>(null);
  const [chosen, setChosen] = useState(0);
  const [picks, setPicks] = useState<boolean[]>([]);
  const titleId = useId();

  useEffect(() => {
    invoke<CalendarLinkStatus>("calendar_link_status")
      .then(setLink)
      .catch(() => setLink(null));
  }, []);

  const show = (m: CalendarMatch) => {
    setMatch(m);
    setChosen(0);
    setPicks(defaultPicks(m.candidates[0]?.plan ?? []));
  };

  const load = async (from: "file" | "link") => {
    setBusy(from);
    setError("");
    try {
      if (from === "file") {
        const m = await invoke<CalendarMatch | null>("calendar_events_from_file", { id: item.id });
        if (m) show(m);
      } else {
        show(await invoke<CalendarMatch>("calendar_events_from_link", { id: item.id }));
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy("");
    }
  };

  const choose = (i: number) => {
    setChosen(i);
    setPicks(defaultPicks(match?.candidates[i]?.plan ?? []));
  };

  const candidate = match?.candidates[chosen];
  const plan = candidate?.plan ?? [];
  const selected = pickedAttendees(plan, picks);

  const add = async () => {
    setBusy("add");
    setError("");
    try {
      const r = await invoke<AttendeesAdded>("calendar_add_attendees", { id: item.id, attendees: selected });
      onItem(r.item);
      ctl.flash(addedMessage(r.added, r.completed), 4000);
      onClose();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy("");
    }
  };

  const message = match ? matchMessage(match) : "";

  return (
    <section className="cal-panel" aria-labelledby={titleId}>
      <div className="cal-head">
        <h3 id={titleId}>Attendees from calendar</h3>
        <button type="button" className="link-btn" onClick={onClose}>
          Close
        </button>
      </div>
      <div className="cal-sources">
        <button type="button" className="btn-ghost sh-btn" disabled={!!busy} onClick={() => load("file")}>
          {busy === "file" ? "Reading…" : "Choose .ics file…"}
        </button>
        <button
          type="button"
          className="btn-ghost sh-btn"
          disabled={!!busy || !link?.saved}
          title={link?.saved ? `Fetch the calendar from ${link.host}` : "Save a private calendar link in Settings → Calendar"}
          onClick={() => load("link")}
        >
          {busy === "link" ? "Fetching…" : link?.saved ? `Use saved link (${link.host})` : "Use saved link"}
        </button>
        {link && !link.saved && (
          <small className="sh-muted">
            No link saved
            {onOpenSettings ? (
              <>
                {" "}
                —{" "}
                <button type="button" className="link-btn" onClick={onOpenSettings}>
                  add one in Settings → Calendar
                </button>
              </>
            ) : (
              " — add one in Settings → Calendar"
            )}
            .
          </small>
        )}
      </div>

      {error && (
        <p className="cal-error" role="alert">
          {error}
        </p>
      )}

      {match && (
        <div className="cal-result" aria-live="polite">
          <p className="sh-muted">
            {match.source}: {match.events_read} event{match.events_read === 1 ? "" : "s"} read.
            {message && <> {message}</>}
          </p>
          {match.notes.map((n) => (
            <p key={n} className="cal-note">
              {n}
            </p>
          ))}
          {match.candidates.length > 0 && (
            <fieldset className="cal-events">
              <legend>Event</legend>
              {match.candidates.map((c, i) => (
                <label key={`${c.uid}|${c.start}`} className="check-row">
                  <input type="radio" name={`${titleId}-event`} checked={chosen === i} onChange={() => choose(i)} />
                  <span>
                    <strong>{c.title || "(no title)"}</strong> · {timeRange(c)}
                    <small>
                      {overlapHint(c)} · {c.attendees.length} attendee{c.attendees.length === 1 ? "" : "s"}
                    </small>
                  </span>
                </label>
              ))}
            </fieldset>
          )}
          {candidate && (
            <fieldset className="cal-attendees">
              <legend>Attendees</legend>
              {noAttendeesMessage(candidate) && <p className="sh-muted">{noAttendeesMessage(candidate)}</p>}
              {plan.map((p, i) => (
                <label key={`${p.name}|${p.email ?? ""}`} className="check-row">
                  <input
                    type="checkbox"
                    checked={pickable(p) && !!picks[i]}
                    disabled={!pickable(p)}
                    onChange={(e) => setPicks((prev) => prev.map((v, j) => (j === i ? e.target.checked : v)))}
                  />
                  <span>
                    {p.name}
                    <small>
                      {actionLabel(p)}
                      {pickable(p) && !p.in_people && p.email ? " · not in People yet (use + People on its chip)" : ""}
                    </small>
                  </span>
                </label>
              ))}
            </fieldset>
          )}
          {candidate && plan.length > 0 && (
            <div className="list-actions start">
              <button type="button" className="btn-ghost sh-btn" disabled={!!busy || !selected.length} onClick={add}>
                {busy === "add" ? "Adding…" : `Add ${selected.length} to participants`}
              </button>
            </div>
          )}
        </div>
      )}
    </section>
  );
}

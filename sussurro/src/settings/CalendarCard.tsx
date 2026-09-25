import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Card, Tip } from "../components/ui";
import { linkInputError, type CalendarLinkStatus } from "../lib/calendar";
import type { CardProps } from "./DictationCard";

/** Settings → Calendar (#252, P22): the private ICS link used by a meeting's
 *  "Add attendees from calendar…". The link is a secret — anyone who has it
 *  reads the calendar — so it lives only in the OS credential store: it is
 *  never shown again after saving (only its host), never in settings.json,
 *  exports or diagnostics. No account, no OAuth. */
export function CalendarCard({ ctl }: CardProps) {
  const { setBusy, flash } = ctl;
  const [status, setStatus] = useState<CalendarLinkStatus | null>(null);
  const [draft, setDraft] = useState("");
  const [working, setWorking] = useState(false);
  const [confirmRemove, setConfirmRemove] = useState(false);

  useEffect(() => {
    invoke<CalendarLinkStatus>("calendar_link_status")
      .then(setStatus)
      .catch((e) => setBusy(String(e)));
  }, [setBusy]);

  const inputError = linkInputError(draft);
  const canSave = !working && !!draft.trim() && !inputError && status?.store_available !== false;

  const save = async () => {
    setWorking(true);
    try {
      const st = await invoke<CalendarLinkStatus>("calendar_link_save", { link: draft.trim() });
      setStatus(st);
      setDraft("");
      flash(`Calendar link saved in ${st.store_name}.`, 4000);
    } catch (e) {
      setBusy(String(e));
    } finally {
      setWorking(false);
    }
  };

  const remove = async () => {
    setWorking(true);
    try {
      setStatus(await invoke<CalendarLinkStatus>("calendar_link_remove"));
      setConfirmRemove(false);
      flash("Calendar link removed.", 3000);
    } catch (e) {
      setBusy(String(e));
    } finally {
      setWorking(false);
    }
  };

  return (
    <Card title={<>Calendar <span className="via">meeting attendees, no account</span></>}>
      <p className="card-hint">
        A meeting's <strong>Add attendees from calendar…</strong> reads the event that overlaps the recording and offers
        its attendees as participants. Import an <code>.ics</code> file there, or save your calendar's private link here
        to skip the export each time. Nothing is synced in the background: the link is fetched only when you ask.
      </p>

      <div className="field field-col">
        <div className="field-label">
          <span>
            Private calendar link{" "}
            <Tip text="Google Calendar: Settings → your calendar → Integrate calendar → Secret address in iCal format. Outlook: Settings → Calendar → Shared calendars → Publish a calendar (with details) → ICS link. Apple Calendar: share the calendar as a public calendar (webcal:// link). Some organisations turn these links off. Whether attendees are included depends on the provider and the detail level you publish." />
          </span>
          <small>
            {status === null
              ? "checking…"
              : status.saved
                ? `saved · ${status.host}`
                : status.error
                  ? status.error
                  : "none saved"}
          </small>
        </div>
        {status?.saved && (
          <div className="model-row">
            <span className="path">
              A link to <strong>{status.host}</strong> is kept in {status.store_name}.
            </span>
            {confirmRemove ? (
              <>
                <button type="button" className="btn-ghost" disabled={working} onClick={remove}>
                  Remove now
                </button>
                <button type="button" className="btn-ghost" disabled={working} onClick={() => setConfirmRemove(false)}>
                  Cancel
                </button>
              </>
            ) : (
              <button type="button" className="btn-ghost" disabled={working} onClick={() => setConfirmRemove(true)}>
                Remove…
              </button>
            )}
          </div>
        )}
        <div className="field-stack">
          <input
            type="password"
            value={draft}
            autoComplete="off"
            spellCheck={false}
            placeholder={status?.saved ? "Paste a new link to replace it" : "https://… or webcal://…"}
            aria-label="Private calendar link"
            aria-invalid={!!inputError}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && canSave) save();
            }}
          />
          {inputError && <small className="endpoint-note">{inputError}</small>}
          {status && !status.store_available && (
            <small className="endpoint-note">
              No credential store is available ({status.error || "unknown reason"}), so the link can't be saved
              safely. Import the calendar as an .ics file from the meeting instead.
            </small>
          )}
          <div className="list-actions start">
            <button type="button" className="btn-ghost" disabled={!canSave} onClick={save}>
              {status?.saved ? "Replace link" : "Save link"}
            </button>
          </div>
          <p className="card-hint">
            Treat this link like a password: anyone who has it can read your calendar. Sussurro keeps it only in{" "}
            {status?.store_name ?? "the OS credential store"} and fetches it with the same network rules as links on New
            (no addresses on this computer or your local network).
          </p>
        </div>
      </div>
    </Card>
  );
}

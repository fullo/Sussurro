import { useCallback, useRef, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { Ctl } from "../hooks/useAppController";
import { DONT_SHOW_AGAIN_DEFAULT, RECORDING_NOTICE as T, RECORDING_PRIVACY_URL } from "../lib/recordingNotice";
import { needsMeetingNotice, settingsAfterNotice } from "../lib/meetingNotice";

/** A link to the README's section on recording meetings, opened in the
 *  system browser. */
export function RecordingPrivacyLink({ children = T.readMore }: { children?: string }) {
  return (
    <a
      href={RECORDING_PRIVACY_URL}
      onClick={(e) => {
        e.preventDefault();
        void openUrl(RECORDING_PRIVACY_URL);
      }}
    >
      {children}
    </a>
  );
}

/** The notice before the first recording of other people (#136). Never a
 *  dead end: "Start recording" always proceeds. */
function RecordingNoticeDialog({ onAnswer }: { onAnswer: (proceed: boolean, dontShowAgain: boolean) => void }) {
  const [dontShow, setDontShow] = useState(DONT_SHOW_AGAIN_DEFAULT);
  return (
    <div className="modal-backdrop" role="presentation" onClick={() => onAnswer(false, dontShow)}>
      <div
        className="modal confirm-modal consent-modal"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="rec-notice-title"
        aria-describedby="rec-notice-desc"
        onClick={(e) => e.stopPropagation()}
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            e.stopPropagation();
            onAnswer(false, dontShow);
          }
        }}
      >
        <h2 id="rec-notice-title">{T.title}</h2>
        <div id="rec-notice-desc">
          {T.body.map((p) => (
            <p key={p}>{p}</p>
          ))}
        </div>
        <p className="sh-note">
          {T.local} <RecordingPrivacyLink />
        </p>
        <label className="check-row">
          <input type="checkbox" checked={dontShow} onChange={(e) => setDontShow(e.target.checked)} />
          <span>{T.dontShowAgain}</span>
        </label>
        <div className="row-gap end">
          <button type="button" className="btn-ghost sh-btn" onClick={() => onAnswer(false, dontShow)}>
            {T.cancel}
          </button>
          <button type="button" className="btn-dark sh-btn" autoFocus onClick={() => onAnswer(true, dontShow)}>
            {T.proceed}
          </button>
        </div>
      </div>
    </div>
  );
}

/** Ask before a recording of other people starts. `confirmNotice()`
 *  resolves true at once when the notice was acknowledged before, else
 *  after the user answers (false = cancelled: don't start); `dialog` must
 *  be rendered. "Don't show this again" is saved in the settings. */
export function useRecordingNotice(ctl: Ctl) {
  const [open, setOpen] = useState(false);
  const answer = useRef<((proceed: boolean) => void) | null>(null);
  // The latest settings when the user answers (a save may land meanwhile).
  const ctlRef = useRef(ctl);
  ctlRef.current = ctl;

  const confirmNotice = useCallback((): Promise<boolean> => {
    if (!needsMeetingNotice(ctlRef.current.settings)) return Promise.resolve(true);
    return new Promise<boolean>((resolve) => {
      answer.current = resolve;
      setOpen(true);
    });
  }, []);

  const dialog = open ? (
    <RecordingNoticeDialog
      onAnswer={(proceed, dontShowAgain) => {
        setOpen(false);
        const { settings, save } = ctlRef.current;
        const next = settingsAfterNotice(settings, proceed, dontShowAgain);
        // Remembering is best effort: a failed save only means the notice
        // shows again next time; the recording goes ahead either way.
        if (next) void save(next);
        answer.current?.(proceed);
        answer.current = null;
      }}
    />
  ) : null;

  return { confirmNotice, dialog };
}

/** The subtle line on the meeting tabs of New and while recording. */
export function RecordingReminder() {
  return (
    <p className="sh-note rec-reminder" role="note">
      {T.reminder} <RecordingPrivacyLink>{T.reminderMore}</RecordingPrivacyLink>
    </p>
  );
}

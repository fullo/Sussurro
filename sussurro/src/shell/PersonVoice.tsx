import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Switch } from "../components/ui";
import type { Ctl } from "../hooks/useAppController";
import type { Person, VoiceStatus } from "../lib/types";
import {
  VOICE_DELETE,
  VOICE_STORED,
  VOICE_TELL,
  VOICE_USE,
  VOICE_WHERE,
  toggleAction,
  voiceStatusLabel,
} from "../lib/voiceRecognition";

/** People → a person → *Recognise this voice* (#242, P12/P13): the opt-in
 *  toggle, its status (confirmed speech, documents, ready or not) and
 *  *Forget this voice*. Turning it on first shows what is stored, where,
 *  how to delete it, and that the person should be told. */
export function PersonVoice({
  ctl,
  person,
  onChanged,
}: {
  ctl: Ctl;
  person: Person;
  /** The status changed (the People list shows a marker). */
  onChanged?: (s: VoiceStatus) => void;
}) {
  const [status, setStatus] = useState<VoiceStatus | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [sheet, setSheet] = useState(false);
  const [confirmForget, setConfirmForget] = useState(false);

  useEffect(() => {
    let alive = true;
    invoke<VoiceStatus>("voice_status", { personId: person.id })
      .then((s) => alive && setStatus(s))
      .catch((e) => alive && setError(String(e)));
    return () => {
      alive = false;
    };
  }, [person.id]);

  const apply = async (f: () => Promise<VoiceStatus>, message: string) => {
    setBusy(true);
    setError("");
    try {
      const s = await f();
      setStatus(s);
      onChanged?.(s);
      ctl.flash(message, 3000);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const setEnabled = (enabled: boolean) =>
    apply(
      () => invoke<VoiceStatus>("voice_set_enabled", { personId: person.id, enabled }),
      enabled ? `Recognising ${person.name}'s voice.` : `${person.name}'s voice profile deleted.`,
    );

  const forget = () =>
    apply(async () => {
      await invoke<boolean>("voice_forget", { personId: person.id });
      return invoke<VoiceStatus>("voice_status", { personId: person.id });
    }, `${person.name}'s voice profile deleted.`);

  const enabled = !!status?.enabled;
  const onToggle = (next: boolean) => {
    const act = toggleAction(enabled, next);
    if (act === "confirm") setSheet(true);
    else if (act === "off") setEnabled(false);
  };

  return (
    <div className="people-voice" role="group" aria-label={`Voice recognition for ${person.name}`}>
      <div className="field">
        <div className="field-label">
          <span>Recognise this voice</span>
          <small>suggests {person.name} for matching voices; never links by itself</small>
        </div>
        <Switch checked={enabled} onChange={(v) => !busy && status && onToggle(v)} label="Recognise this voice" />
      </div>
      <p className="sh-muted people-voice-status" aria-live="polite">
        {status ? voiceStatusLabel(status) : error ? "" : "Reading…"}
      </p>
      {enabled &&
        (confirmForget ? (
          <div className="row-gap" role="alertdialog" aria-label="Confirm forget voice">
            <span className="sh-muted">
              Delete {person.name}'s voice profile from this computer? Recognition turns off; your links stay.
            </span>
            <button
              type="button"
              className="btn-danger sh-btn push"
              disabled={busy}
              onClick={() => {
                setConfirmForget(false);
                forget();
              }}
            >
              Forget
            </button>
            <button type="button" className="btn-ghost" onClick={() => setConfirmForget(false)}>
              Keep
            </button>
          </div>
        ) : (
          <button type="button" className="link-btn" disabled={busy} onClick={() => setConfirmForget(true)}>
            Forget this voice
          </button>
        ))}
      {error && <p className="prof-test error" role="alert">{error}</p>}
      {sheet && (
        <VoiceSheet
          person={person}
          onAnswer={(ok) => {
            setSheet(false);
            if (ok) setEnabled(true);
          }}
        />
      )}
    </div>
  );
}

/** The first-use sheet (P13): what a voice profile is, where it lives, how
 *  to delete it — and tell the person. Cancel is the default. */
function VoiceSheet({ person, onAnswer }: { person: Person; onAnswer: (ok: boolean) => void }) {
  return (
    <div className="modal-backdrop" role="presentation" onClick={() => onAnswer(false)}>
      <div
        className="modal confirm-modal voice-sheet"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="voice-sheet-title"
        aria-describedby="voice-sheet-desc"
        onClick={(e) => e.stopPropagation()}
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            e.stopPropagation();
            onAnswer(false);
          }
        }}
      >
        <h2 id="voice-sheet-title">Recognise {person.name}'s voice?</h2>
        <p id="voice-sheet-desc" className="sh-muted">{VOICE_STORED}</p>
        <dl className="consent-facts">
          <dt>Where</dt>
          <dd>{VOICE_WHERE}</dd>
          <dt>What for</dt>
          <dd>{VOICE_USE}</dd>
          <dt>Delete</dt>
          <dd>{VOICE_DELETE}</dd>
        </dl>
        <p className="sh-note">
          <strong>Tell {person.name}.</strong> {VOICE_TELL}
        </p>
        <div className="row-gap end">
          <button type="button" className="btn-ghost sh-btn" autoFocus onClick={() => onAnswer(false)}>
            Cancel
          </button>
          <button type="button" className="btn-dark sh-btn" onClick={() => onAnswer(true)}>
            Turn on
          </button>
        </div>
      </div>
    </div>
  );
}

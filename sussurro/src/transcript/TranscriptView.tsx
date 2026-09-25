import { useEffect, useRef, useState } from "react";
import "./transcript.css";

/* Transcript components. Deliberately free of Tauri imports and app state —
   they take lines and callbacks — so the 0.9 browser-extension side panel
   can import them through the `@sussurro/transcript` Vite alias (E11). */

/** A speaker as a line shows it (#130): chip label and colour. */
export interface TranscriptSpeaker {
  id: string;
  label: string;
  color: string;
}

export interface TranscriptLineData {
  id: number;
  start_ms: number;
  text: string;
  /** Edited by hand in the app. */
  edited?: boolean;
  /** Speech-to-text failed on this stretch: why (shown as "[not transcribed]"). */
  sttError?: string;
  /** Who said it, when the document has speakers (#130). */
  speaker?: TranscriptSpeaker;
  /** Overlapping speech in the line (#244): the other speakers heard at
   *  the same time (empty when the app could not tell who). The line
   *  keeps its own speaker. */
  alsoSpeaking?: TranscriptSpeaker[];
}

/** A stretch of overlapping speech in a segment (#244). */
export interface OverlapSpanData {
  start_ms: number;
  end_ms: number;
  speaker_id?: string;
}

/** Segment-shaped data → lines: blank segments are dropped, except the ones
 *  STT failed on, which stay as "[not transcribed]" rows. With `speakers`,
 *  each line carries its listed speaker (chip) and, when it has overlapping
 *  speech (#244), the other listed speakers heard in it. */
export function toLines(
  segments: {
    id: number;
    start_ms: number;
    text: string;
    edited?: boolean;
    stt_error?: string;
    speaker_id?: string;
    overlap?: OverlapSpanData[];
  }[],
  speakers?: TranscriptSpeaker[],
): TranscriptLineData[] {
  const byId = new Map((speakers ?? []).map((s) => [s.id, s]));
  const chip = (sp: TranscriptSpeaker): TranscriptSpeaker => ({ id: sp.id, label: sp.label, color: sp.color });
  return segments
    .filter((s) => s.text.trim() || s.stt_error)
    .map((s) => {
      const sp = s.speaker_id ? byId.get(s.speaker_id) : undefined;
      let also: TranscriptSpeaker[] | undefined;
      if (speakers && s.overlap?.length) {
        also = [];
        for (const o of s.overlap) {
          const other = o.speaker_id && o.speaker_id !== s.speaker_id ? byId.get(o.speaker_id) : undefined;
          if (other && !also.some((a) => a.id === other.id)) also.push(chip(other));
        }
      }
      return {
        id: s.id,
        start_ms: s.start_ms,
        text: s.text,
        ...(s.edited ? { edited: true } : {}),
        ...(s.stt_error && !s.text.trim() ? { sttError: s.stt_error } : {}),
        ...(sp ? { speaker: chip(sp) } : {}),
        ...(also ? { alsoSpeaking: also } : {}),
      };
    });
}

/** The words next to a line's chip when it has overlapping speech (#244). */
export function alsoSpeakingText(also: TranscriptSpeaker[]): string {
  if (also.length === 0) return "overlapping speech";
  const names = also.map((a) => a.label);
  const list = names.length === 1 ? names[0] : `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
  return `+ ${list} also speaking`;
}

/** Line-move target that opens a new "Voice N" (the backend's NEW_VOICE). */
export const NEW_VOICE_TARGET = "voice:new";

/** `HH:MM:SS`, as in transcript.md. */
export function lineTimestamp(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(Math.floor(s / 3600))}:${p(Math.floor(s / 60) % 60)}:${p(s % 60)}`;
}

interface LineProps {
  line: TranscriptLineData;
  editable: boolean;
  /** Resolve true when saved (the editor closes), false to keep it open. */
  onEdit?: (id: number, text: string) => Promise<boolean>;
  onDelete?: (id: number) => Promise<void>;
  /** The document's speakers, offered by "Move to speaker" (#130). */
  speakers?: TranscriptSpeaker[];
  /** Move the line to a speaker id (or {@link NEW_VOICE_TARGET}). */
  onMoveSpeaker?: (id: number, speakerId: string) => Promise<void>;
  /** The line picked elsewhere (the Voice map, #144): marked. */
  selected?: boolean;
}

function SpeakerChip({ speaker }: { speaker: TranscriptSpeaker }) {
  return (
    <span className="tx-chip" style={{ background: speaker.color || undefined }} title={speaker.label}>
      <i aria-hidden="true" />
      {speaker.label}
    </span>
  );
}

export function TranscriptLine({ line, editable, onEdit, onDelete, speakers, onMoveSpeaker, selected = false }: LineProps) {
  const [draft, setDraft] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [saving, setSaving] = useState(false);
  const ts = lineTimestamp(line.start_ms);

  useEffect(() => {
    if (!confirmDelete) return;
    const t = setTimeout(() => setConfirmDelete(false), 3000);
    return () => clearTimeout(t);
  }, [confirmDelete]);

  const save = async () => {
    if (draft === null || !onEdit) return;
    if (draft.trim() === line.text.trim()) {
      setDraft(null);
      return;
    }
    setSaving(true);
    const ok = await onEdit(line.id, draft);
    setSaving(false);
    if (ok) setDraft(null);
  };

  const editing = draft !== null;
  return (
    <li
      className={`tx-line${editing ? " editing" : ""}${selected ? " selected" : ""}`}
      data-seg={line.id}
      aria-current={selected ? "true" : undefined}
    >
      <span className="tx-time" aria-label={`at ${ts}`}>{ts}</span>
      {editing ? (
        <div className="tx-edit">
          <textarea
            value={draft}
            rows={Math.min(8, Math.max(2, Math.ceil(draft.length / 70)))}
            aria-label={`Edit line at ${ts}`}
            autoFocus
            disabled={saving}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Escape") {
                e.preventDefault();
                setDraft(null);
              } else if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
                e.preventDefault();
                save();
              }
            }}
          />
          <div className="tx-actions tx-actions-open">
            <button type="button" className="tx-btn tx-btn-primary" onClick={save} disabled={saving || !draft.trim()}>
              {saving ? "Saving…" : "Save"}
            </button>
            <button type="button" className="tx-btn" onClick={() => setDraft(null)} disabled={saving}>
              Cancel
            </button>
            <span className="tx-hint">⌘/Ctrl+Enter saves · Esc cancels</span>
          </div>
        </div>
      ) : (
        <div className="tx-body">
          {line.speaker && <SpeakerChip speaker={line.speaker} />}
          {line.alsoSpeaking && (
            <span
              className="tx-also"
              title="Two people speak at once in part of this line. The line stays with its speaker."
            >
              {alsoSpeakingText(line.alsoSpeaking)}
            </span>
          )}
          {line.sttError !== undefined ? (
            <p className="tx-text tx-failed" title={`Speech-to-text failed here: ${line.sttError}`}>
              [not transcribed]
            </p>
          ) : (
            <p className="tx-text">
              {line.text}
              {line.edited && <span className="tx-edited" title="Edited in Sussurro"> · edited</span>}
            </p>
          )}
          {editable && (
            <div className="tx-actions">
              <button type="button" className="tx-btn" onClick={() => setDraft(line.text)} aria-label={`Edit line at ${ts}`}>
                Edit
              </button>
              {speakers && onMoveSpeaker && (
                <select
                  className="tx-btn tx-move"
                  aria-label={`Move line at ${ts} to another speaker`}
                  value=""
                  disabled={saving}
                  onChange={async (e) => {
                    const to = e.target.value;
                    if (!to) return;
                    setSaving(true);
                    await onMoveSpeaker(line.id, to);
                    setSaving(false);
                  }}
                >
                  <option value="">Move to speaker…</option>
                  {speakers
                    .filter((s) => s.id !== line.speaker?.id)
                    .map((s) => (
                      <option key={s.id} value={s.id}>
                        {s.label}
                      </option>
                    ))}
                  <option value={NEW_VOICE_TARGET}>New voice</option>
                </select>
              )}
              <button
                type="button"
                className={`tx-btn${confirmDelete ? " tx-btn-danger" : ""}`}
                aria-label={confirmDelete ? `Confirm: delete line at ${ts}` : `Delete line at ${ts}`}
                onClick={async () => {
                  if (!confirmDelete) {
                    setConfirmDelete(true);
                    return;
                  }
                  setConfirmDelete(false);
                  await onDelete?.(line.id);
                }}
              >
                {confirmDelete ? "Click again to delete" : "Delete"}
              </button>
            </div>
          )}
        </div>
      )}
    </li>
  );
}

export function TranscriptView({
  lines,
  editable = false,
  onEdit,
  onDelete,
  speakers,
  onMoveSpeaker,
  follow = false,
  emptyText = "No lines yet.",
  label = "Transcript",
  selectedId = null,
}: {
  lines: TranscriptLineData[];
  editable?: boolean;
  onEdit?: LineProps["onEdit"];
  onDelete?: LineProps["onDelete"];
  /** Speakers offered by "Move to speaker" (#130); omit to hide it. */
  speakers?: TranscriptSpeaker[];
  onMoveSpeaker?: LineProps["onMoveSpeaker"];
  /** Live view: keep the newest line in sight while the user is at the bottom. */
  follow?: boolean;
  emptyText?: string;
  label?: string;
  /** A line picked elsewhere (the Voice map, #144): marked as current.
   *  Every line carries `data-seg` so the host can scroll to it. */
  selectedId?: number | null;
}) {
  const endRef = useRef<HTMLLIElement>(null);
  useEffect(() => {
    if (!follow) return;
    const end = endRef.current;
    const box = end?.closest(".tx-scroll") as HTMLElement | null;
    if (!end || !box) return;
    const nearBottom = box.scrollHeight - box.scrollTop - box.clientHeight < 120;
    if (nearBottom) box.scrollTop = box.scrollHeight;
  }, [follow, lines.length]);

  if (lines.length === 0) return <p className="tx-empty">{emptyText}</p>;
  return (
    <ol className="tx-list" aria-label={label}>
      {lines.map((l) => (
        <TranscriptLine
          key={l.id}
          line={l}
          editable={editable}
          onEdit={onEdit}
          onDelete={onDelete}
          speakers={speakers}
          onMoveSpeaker={onMoveSpeaker}
          selected={l.id === selectedId}
        />
      ))}
      <li ref={endRef} className="tx-end" aria-hidden="true" />
    </ol>
  );
}

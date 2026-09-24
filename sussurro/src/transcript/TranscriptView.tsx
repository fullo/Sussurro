import { useEffect, useRef, useState } from "react";
import "./transcript.css";

/* Transcript components. Deliberately free of Tauri imports and app state —
   they take lines and callbacks — so the 0.9 browser-extension side panel
   can import them through the `@sussurro/transcript` Vite alias (E11). */

export interface TranscriptLineData {
  id: number;
  start_ms: number;
  text: string;
  /** Edited by hand in the app. */
  edited?: boolean;
  /** Speech-to-text failed on this stretch: why (shown as "[not transcribed]"). */
  sttError?: string;
}

/** Segment-shaped data → lines: blank segments are dropped, except the ones
 *  STT failed on, which stay as "[not transcribed]" rows. */
export function toLines(
  segments: { id: number; start_ms: number; text: string; edited?: boolean; stt_error?: string }[],
): TranscriptLineData[] {
  return segments
    .filter((s) => s.text.trim() || s.stt_error)
    .map((s) => ({
      id: s.id,
      start_ms: s.start_ms,
      text: s.text,
      ...(s.edited ? { edited: true } : {}),
      ...(s.stt_error && !s.text.trim() ? { sttError: s.stt_error } : {}),
    }));
}

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
}

export function TranscriptLine({ line, editable, onEdit, onDelete }: LineProps) {
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
    <li className={`tx-line${editing ? " editing" : ""}`}>
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
  follow = false,
  emptyText = "No lines yet.",
  label = "Transcript",
}: {
  lines: TranscriptLineData[];
  editable?: boolean;
  onEdit?: LineProps["onEdit"];
  onDelete?: LineProps["onDelete"];
  /** Live view: keep the newest line in sight while the user is at the bottom. */
  follow?: boolean;
  emptyText?: string;
  label?: string;
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
        <TranscriptLine key={l.id} line={l} editable={editable} onEdit={onEdit} onDelete={onDelete} />
      ))}
      <li ref={endRef} className="tx-end" aria-hidden="true" />
    </ol>
  );
}

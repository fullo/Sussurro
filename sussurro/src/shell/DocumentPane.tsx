import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { TranscriptView, toLines } from "@sussurro/transcript";
import { speakersEnabled } from "../lib/speakers";
import type { Ctl } from "../hooks/useAppController";
import { fileManagerName, formatDurationLabel, formatLongDate, parseDuration } from "../lib/format";
import { TYPE_LABEL } from "../lib/library";
import { hasParticipants } from "../lib/participants";
import { externalHostsTitle, sentExternally } from "../lib/privacy";
import type { Item, ItemMeta } from "../lib/types";
import { ChipEditor } from "./ChipEditor";
import { ContextPane, useDrawerLayout, type ContextStatus } from "./ContextPane";
import { DocumentTab } from "./DocumentTab";
import { ParticipantEditor } from "./ParticipantEditor";
import { languageLabel } from "./labels";

/** Source label for the header: "microphone", "file memo.m4a". */
function sourceLabel(source: string): string {
  if (source === "mic") return "microphone";
  if (source.startsWith("file:")) return `file ${source.slice(5)}`;
  return source;
}

export function DocumentPane({
  ctl,
  id,
  version,
  onChanged,
  onDeleted,
}: {
  ctl: Ctl;
  id: string;
  version: number;
  onChanged: () => void;
  onDeleted: () => void;
}) {
  const [item, setItem] = useState<Item | null>(null);
  const [error, setError] = useState("");
  const [title, setTitle] = useState("");
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [tab, setTab] = useState<"transcript" | "document">("transcript");
  /** Companion documents of the item (#120); null until counted. */
  const [docCount, setDocCount] = useState<number | null>(null);
  /** Bumped when the context pane writes a companion document. */
  const [docsVersion, setDocsVersion] = useState(0);
  /** Document the Document tab should show (opened from the context pane). */
  const [openDoc, setOpenDoc] = useState<{ file: string; n: number } | null>(null);
  const drawer = useDrawerLayout();
  const [ctxOpen, setCtxOpen] = useState(false);
  const [ctxStatus, setCtxStatus] = useState<ContextStatus>("idle");
  const ctxToggleRef = useRef<HTMLButtonElement>(null);
  const titleRef = useRef<HTMLInputElement>(null);

  const load = async () => {
    try {
      const it = await invoke<Item>("archive_get", { id });
      setItem(it);
      setError("");
      if (document.activeElement !== titleRef.current) setTitle(it.meta.title);
    } catch (e) {
      setError(String(e));
    }
  };

  useEffect(() => {
    load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id, version]);

  // The document count shows on the tab even while Transcript is open.
  useEffect(() => {
    setDocCount(null);
    invoke<unknown[]>("recipe_documents", { id })
      .then((d) => setDocCount(d.length))
      .catch(() => setDocCount(0));
  }, [id, docsVersion]);

  // A live item (#153) grows while its session records: follow it.
  const recording = !!item?.recording;
  useEffect(() => {
    if (!recording) return;
    const t = setInterval(load, 3000);
    return () => clearInterval(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [recording, id]);

  if (error && !item) {
    return (
      <div className="empty-state quiet" role="alert">
        <p>This item could not be opened.</p>
        <p className="sh-muted mono">{error}</p>
      </div>
    );
  }
  if (!item) return <div className="empty-state quiet"><p className="sh-muted">Loading…</p></div>;

  const meta = item.meta;
  const saveMeta = async (next: ItemMeta) => {
    try {
      const updated = await invoke<Item>("archive_update_meta", { id, meta: next });
      setItem(updated);
      onChanged();
    } catch (e) {
      ctl.setBusy(String(e));
      load();
    }
  };

  const commitTitle = () => {
    const t = title.trim();
    if (!t) {
      setTitle(meta.title);
      return;
    }
    if (t !== meta.title) saveMeta({ ...meta, title: t });
  };

  const editLine = async (segmentId: number, text: string): Promise<boolean> => {
    try {
      const updated = await invoke<Item>("archive_update_segment", { id, segmentId, text });
      setItem(updated);
      onChanged();
      return true;
    } catch (e) {
      ctl.setBusy(String(e));
      // Most likely edited outside meanwhile: reload to show the notice.
      load();
      return false;
    }
  };

  const deleteLine = async (segmentId: number) => {
    try {
      const updated = await invoke<Item>("archive_delete_segment", { id, segmentId });
      setItem(updated);
      onChanged();
    } catch (e) {
      ctl.setBusy(String(e));
      load();
    }
  };

  const moveLine = async (segmentId: number, speakerId: string) => {
    try {
      const updated = await invoke<Item>("archive_move_segment_speaker", { id, segmentId, speakerId });
      setItem(updated);
      onChanged();
    } catch (e) {
      ctl.setBusy(String(e));
      load();
    }
  };

  const reveal = () => invoke("archive_reveal", { id }).catch((e) => ctl.setBusy(String(e)));

  // Speaker chips and "Move to speaker" are part of the 0.9 preview (#130).
  const showSpeakers = speakersEnabled(ctl.settings, item);
  const docSpeakers = showSpeakers ? item.segments.speakers : undefined;
  const lines = toLines(item.segments.segments, docSpeakers);
  const editable = !item.edited_externally && !item.recording;
  const duration = formatDurationLabel(parseDuration(meta.duration));
  const facts = [
    formatLongDate(meta.date),
    duration,
    sourceLabel(meta.source),
    meta.language && meta.language !== "auto" ? languageLabel(meta.language) : "",
    meta.engine,
  ].filter(Boolean);

  const closeCtx = () => {
    setCtxOpen(false);
    ctxToggleRef.current?.focus();
  };

  const openDocument = (file: string) => {
    setOpenDoc((o) => ({ file, n: (o?.n ?? 0) + 1 }));
    setTab("document");
    if (drawer) setCtxOpen(false);
  };

  return (
    <div className="doc-split">
    <article className="doc" aria-label={meta.title}>
      <header className="doc-head">
        <div className="doc-title-row">
          <input
            ref={titleRef}
            className="doc-title"
            size={Math.max(8, title.length + 1)}
            value={title}
            aria-label="Title"
            readOnly={!!item.recording}
            onChange={(e) => setTitle(e.target.value)}
            onBlur={commitTitle}
            onKeyDown={(e) => {
              if (e.key === "Enter") (e.target as HTMLInputElement).blur();
              if (e.key === "Escape") {
                setTitle(meta.title);
                (e.target as HTMLInputElement).blur();
              }
            }}
          />
          <span className={`tb ${meta.type}`}>{TYPE_LABEL[meta.type] ?? meta.type}</span>
          {sentExternally(item) && (
            <span className="ext sent-ext" title={externalHostsTitle(item.external_hosts)}>
              ↗ Sent to external LLM
            </span>
          )}
        </div>
        <p className="doc-facts">{facts.join(" · ")}</p>
        <div className="doc-actions">
          <button type="button" className="btn-ghost sh-btn" onClick={reveal} title={`Show the item folder in ${fileManagerName()}`}>
            Open folder
          </button>
          <button type="button" className="btn-ghost sh-btn" onClick={() => setConfirmDelete(true)} disabled={!!item.recording}>
            Delete…
          </button>
          {drawer && (
            <button
              ref={ctxToggleRef}
              type="button"
              className="btn-ghost sh-btn ctx-toggle"
              aria-expanded={ctxOpen}
              aria-controls="ctx-pane"
              onClick={() => setCtxOpen((o) => !o)}
            >
              {showSpeakers ? "Speakers · Ask · Export" : "Ask · Export"}
              {ctxStatus !== "idle" && (
                <span
                  className={`ctx-dot ${ctxStatus}`}
                  role="img"
                  aria-label={ctxStatus === "running" ? "(running)" : "(answer ready)"}
                />
              )}
              <span aria-hidden="true">{ctxOpen ? "▸" : "◂"}</span>
            </button>
          )}
        </div>
      </header>

      <div className="doc-tabs" role="tablist" aria-label="Document views">
        {(["transcript", "document"] as const).map((t) => (
          <button
            key={t}
            type="button"
            role="tab"
            aria-selected={tab === t}
            className={`doc-tab${tab === t ? " active" : ""}`}
            onClick={() => setTab(t)}
          >
            {t === "transcript" ? "Transcript" : "Document"}
            {t === "document" && docCount ? <small className="doc-tab-n">{docCount}</small> : null}
          </button>
        ))}
      </div>

      <div className="doc-meta">
        <span className="opt-k">Tags</span>
        <ChipEditor label="Tags" disabled={!!item.recording} values={meta.tags} addLabel="+ tag" onChange={(tags) => saveMeta({ ...meta, tags })} />
        <span className="opt-k">Categories</span>
        <ChipEditor
          label="Categories"
          disabled={!!item.recording}
          values={meta.categories}
          addLabel="+ category"
          onChange={(categories) => saveMeta({ ...meta, categories })}
        />
        {/* P10: notes never have participants. */}
        {hasParticipants(meta.type) && (
          <>
            <span className="opt-k">Participants</span>
            <ParticipantEditor
              label="Participants"
              disabled={!!item.recording}
              values={meta.participants}
              onChange={(participants) => saveMeta({ ...meta, participants })}
            />
          </>
        )}
      </div>

      {tab === "document" ? (
        <DocumentTab
          ctl={ctl}
          item={item}
          onCount={setDocCount}
          onChanged={onChanged}
          select={openDoc}
          version={docsVersion}
        />
      ) : (
      <>
      {item.recording && (
        <div className="notice-live" role="status">
          <strong>● Recording.</strong> This item is being written by a running session; lines appear as they
          are transcribed. Editing opens when the session ends.
        </div>
      )}
      {item.interrupted && (
        <div className="notice-warn" role="note">
          <strong>Interrupted.</strong> Sussurro stopped before this session was finished; it holds what was
          transcribed until then.
        </div>
      )}
      {item.edited_externally && (
        <div className="notice-warn" role="note">
          <strong>Edited outside Sussurro.</strong> The markdown file wins: Sussurro won't overwrite{" "}
          <code>transcript.md</code>, so line editing is off.{" "}
          {hasParticipants(meta.type) ? "Title, tags, categories and participants" : "Title, tags and categories"} still
          update its frontmatter.{" "}
          <button type="button" className="link-btn" onClick={reveal}>Open folder</button>
        </div>
      )}

      <div className="doc-scroll tx-scroll">
        {lines.length > 0 ? (
          <TranscriptView
            lines={lines}
            editable={editable}
            onEdit={editLine}
            onDelete={deleteLine}
            speakers={docSpeakers && docSpeakers.length > 0 ? docSpeakers : undefined}
            onMoveSpeaker={moveLine}
            label={`Transcript of ${meta.title}`}
          />
        ) : item.body.trim() ? (
          <pre className="doc-body">{item.body.trim()}</pre>
        ) : (
          <p className="tx-empty">This item has no text.</p>
        )}
      </div>
      </>
      )}

      {confirmDelete && (
        <div className="modal-backdrop" role="presentation" onClick={() => setConfirmDelete(false)}>
          <div
            className="modal confirm-modal"
            role="alertdialog"
            aria-modal="true"
            aria-labelledby="del-title"
            aria-describedby="del-desc"
            onClick={(e) => e.stopPropagation()}
            onKeyDown={(e) => e.key === "Escape" && setConfirmDelete(false)}
          >
            <h2 id="del-title">Move “{meta.title}” to the trash?</h2>
            <p id="del-desc" className="sh-muted">
              The whole item folder goes to the trash — you can restore it from there.
            </p>
            <div className="row-gap end">
              <button type="button" className="btn-ghost sh-btn" autoFocus onClick={() => setConfirmDelete(false)}>
                Cancel
              </button>
              <button
                type="button"
                className="btn-danger"
                onClick={async () => {
                  setConfirmDelete(false);
                  try {
                    await invoke("archive_delete", { id });
                    ctl.flash(`Moved “${meta.title}” to the trash.`);
                    onDeleted();
                  } catch (e) {
                    ctl.setBusy(String(e));
                  }
                }}
              >
                Move to trash
              </button>
            </div>
          </div>
        </div>
      )}
    </article>
    {drawer && ctxOpen && <div className="ctx-scrim" aria-hidden="true" onClick={closeCtx} />}
    <ContextPane
      ctl={ctl}
      item={item}
      drawer={drawer}
      open={ctxOpen}
      onClose={closeCtx}
      onStatus={setCtxStatus}
      onDocsChanged={() => {
        setDocsVersion((v) => v + 1);
        onChanged();
      }}
      onOpenDocument={openDocument}
      onItem={(updated) => {
        setItem(updated);
        onChanged();
      }}
    />
    </div>
  );
}

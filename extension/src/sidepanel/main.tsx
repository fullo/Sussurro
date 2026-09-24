/* Side panel (Chrome) / sidebar (Firefox): the live mirror of a meeting
   (plan E3: editing happens in the app). Shows whether the extension is
   paired with the app (#127) and, once paired, the explicit Start/Stop of
   capture for the active tab with a level meter per channel (#128), then
   the live lines with speaker chips, the transcript's backlog, and "Open
   in Sussurro", "Copy as text", "Create .srt" (#129). Before the first
   Start, the recording notice (#136); while recording, a reminder line. */
import { StrictMode, useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import browser from "webextension-polyfill";
import { TranscriptView } from "@sussurro/transcript";
import { Header } from "../shared/Header";
import { MEETING_MATCHES } from "../shared/platform";
import { usePairing } from "../shared/usePairing";
import { useNoticeNeeded } from "../shared/useNotice";
import { DONT_SHOW_AGAIN_DEFAULT, RECORDING_NOTICE as N, RECORDING_PRIVACY_URL, answerNotice, startStep } from "../shared/notice";
import type { Pairing } from "../shared/pairing";
import type { PanelBroadcast, PanelRequest, PanelState } from "../shared/messages";
import {
  announceText,
  backlogView,
  currentItemId,
  emptyMirror,
  lineCount,
  mirrorReceive,
  mirrorSnapshot,
  type LiveTranscript,
} from "../shared/live";
import { exportFilename, exportItem, openItem } from "../shared/items";
import { panelView, viaText } from "./status";
import { partLines } from "./lines";
import { actionsView, copyToClipboard, downloadText, isRecording, type ItemAction } from "./actions";
import "../shared/page.css";

function PairingNote() {
  return (
    <div className="page-note" role="status">
      Not paired with the Sussurro app.{" "}
      <button type="button" className="btn link" onClick={() => browser.runtime.openOptionsPage()}>
        Open options
      </button>
    </div>
  );
}

/** The tab this panel controls: the active one, or `?tabId=` (a panel
 *  opened as a page, e.g. by the automated capture harness). */
function useTabId(): number | null {
  const [tabId, setTabId] = useState<number | null>(() => {
    const q = new URLSearchParams(location.search).get("tabId");
    return q ? Number(q) : null;
  });
  useEffect(() => {
    if (new URLSearchParams(location.search).has("tabId")) return;
    const refresh = () =>
      void browser.tabs.query({ active: true, currentWindow: true }).then(([t]) => setTabId(t?.id ?? null));
    refresh();
    browser.tabs.onActivated.addListener(refresh);
    browser.windows.onFocusChanged.addListener(refresh);
    return () => {
      browser.tabs.onActivated.removeListener(refresh);
      browser.windows.onFocusChanged.removeListener(refresh);
    };
  }, []);
  return tabId;
}

function Meter({ label, via, level }: { label: string; via: string; level: number }) {
  // A log-ish scale: speech sits around 0.02–0.2 RMS.
  const pct = Math.min(100, Math.round(Math.sqrt(level) * 180));
  return (
    <div className="meter" data-testid={`meter-${label}`}>
      <span className="meter-label">{label}</span>
      <span className="meter-bar">
        <span style={{ width: `${pct}%` }} />
      </span>
      <span className="meter-via">{viaText(via)}</span>
    </div>
  );
}

/** The tab's live transcript: a snapshot from the background, then its
 *  `panel:live` changes in order. `announce`: the newest line, for the
 *  screen-reader live region. */
function useTranscript(tabId: number): { t: LiveTranscript | null; announce: string } {
  const [t, setT] = useState<LiveTranscript | null>(null);
  const [announce, setAnnounce] = useState("");
  useEffect(() => {
    let alive = true;
    let mirror = emptyMirror();
    let fetching = false;
    let again = false;
    setT(null);
    setAnnounce("");
    const snapshot = () => {
      if (fetching) {
        again = true;
        return;
      }
      fetching = true;
      void browser.runtime
        .sendMessage({ type: "panel:transcript", tabId } satisfies PanelRequest)
        .then((snap) => {
          if (!alive || !snap) return;
          const r = mirrorSnapshot(mirror, snap as LiveTranscript);
          mirror = r.m;
          setT(mirror.t);
          if (r.refetch) again = true;
        })
        .catch(() => {})
        .finally(() => {
          fetching = false;
          if (alive && again) {
            again = false;
            snapshot();
          }
        });
    };
    const onMsg = (raw: unknown) => {
      const m = raw as PanelBroadcast;
      if (m?.type !== "panel:live" || m.tabId !== tabId) return;
      const r = mirrorReceive(mirror, m.epoch, m.rev, m.action);
      mirror = r.m;
      if (r.applied && mirror.t) {
        setT(mirror.t);
        const a = m.action;
        if (a.kind === "app" && a.msg.type === "segment" && a.msg.kind === "new") {
          const part = mirror.t.parts[mirror.t.parts.length - 1];
          const text = part ? announceText(part, a.msg.segment) : "";
          if (text) setAnnounce(text);
        }
      }
      if (r.refetch) snapshot();
    };
    browser.runtime.onMessage.addListener(onMsg);
    snapshot();
    return () => {
      alive = false;
      browser.runtime.onMessage.removeListener(onMsg);
    };
  }, [tabId]);
  return { t, announce };
}

const EMPTY_TEXT = "During a meeting, the transcript appears here as Sussurro writes it.";

/** The lines, one list per app item, following the newest line unless the
 *  user scrolled up ("Jump to live" brings it back). */
function LiveLines({ t, recording }: { t: LiveTranscript | null; recording: boolean }) {
  const box = useRef<HTMLDivElement>(null);
  const atEnd = useRef(true);
  const [away, setAway] = useState(false);
  const count = t ? lineCount(t) : 0;

  useLayoutEffect(() => {
    const el = box.current;
    if (el && atEnd.current) el.scrollTop = el.scrollHeight;
  }, [t]);

  const onScroll = () => {
    const el = box.current;
    if (!el) return;
    atEnd.current = el.scrollHeight - el.scrollTop - el.clientHeight < 48;
    setAway(!atEnd.current);
  };
  const jump = () => {
    const el = box.current;
    if (!el) return;
    atEnd.current = true;
    setAway(false);
    el.scrollTop = el.scrollHeight;
    el.focus();
  };

  const parts = (t?.parts ?? []).filter((p) => p.lines.length > 0);
  return (
    <div className="live-wrap">
      <div
        ref={box}
        className="tx-scroll live-scroll"
        role="region"
        aria-label="Live transcript"
        tabIndex={0}
        onScroll={onScroll}
        data-testid="transcript"
        data-lines={count}
      >
        {count === 0 ? (
          <TranscriptView lines={[]} emptyText={EMPTY_TEXT} />
        ) : (
          parts.map((p, i) => (
            <section key={`${i}:${p.itemId ?? ""}`} aria-label={parts.length > 1 ? `Part ${i + 1}` : undefined}>
              {i > 0 && (
                <p className="live-break" role="note">
                  Connection lost: Sussurro continued in a new item.
                </p>
              )}
              <TranscriptView lines={partLines(p)} label={parts.length > 1 ? `Transcript, part ${i + 1}` : "Transcript"} />
            </section>
          ))
        )}
      </div>
      {away && count > 0 && (
        <button type="button" className="btn jump" data-testid="jump" onClick={jump}>
          {recording ? "Jump to live ↓" : "Jump to the end ↓"}
        </button>
      )}
    </div>
  );
}

/** "Open in Sussurro", "Copy as text", "Create .srt" — on the meeting's
 *  current item (after a reconnect, the newest one). */
function ItemActions({ pairing, t, state }: { pairing: Pairing; t: LiveTranscript | null; state: PanelState | null }) {
  const [busy, setBusy] = useState<ItemAction | null>(null);
  const [note, setNote] = useState<{ text: string; ok: boolean } | null>(null);
  const itemId = t ? currentItemId(t) : null;
  const subtitles = state?.app?.ok ? state.app.subtitles : undefined;
  const view = actionsView({ itemId, lines: t ? lineCount(t) : 0, phase: state?.phase ?? null, subtitles, busy });
  useEffect(() => setNote(null), [itemId]);
  if (!view.open.visible || !itemId) return null;

  const run = async (action: ItemAction) => {
    setBusy(action);
    setNote(null);
    try {
      if (action === "open") {
        const r = await openItem(pairing, itemId);
        setNote(r.ok ? { text: "Opened in Sussurro.", ok: true } : { text: r.error, ok: false });
        return;
      }
      const format = action === "copy" ? "txt" : "srt";
      const r = await exportItem(pairing, itemId, format);
      if (!r.ok) {
        setNote({ text: r.error, ok: false });
      } else if (action === "copy") {
        const ok = await copyToClipboard(r.value);
        setNote(ok ? { text: "Transcript copied as text.", ok: true } : { text: "The browser did not allow copying to the clipboard.", ok: false });
      } else {
        const name = exportFilename(itemId, "srt");
        downloadText(r.value, name, "application/x-subrip");
        setNote({ text: `Saved ${name} to your downloads.`, ok: true });
      }
    } finally {
      setBusy(null);
    }
  };

  const button = (action: ItemAction, label: string, busyLabel: string) => {
    const v = view[action];
    if (!v.visible) return null;
    return (
      <button type="button" className="btn" data-testid={`action-${action}`} disabled={!v.enabled} title={v.hint} onClick={() => void run(action)}>
        {busy === action ? busyLabel : label}
      </button>
    );
  };
  return (
    <div className="item-actions">
      <div className="actions" role="group" aria-label="This meeting in Sussurro">
        {button("open", "Open in Sussurro", "Opening…")}
        {button("copy", "Copy as text", "Copying…")}
        {button("srt", "Create .srt", "Creating…")}
      </div>
      <p className={`action-note${note && !note.ok ? " error" : ""}`} role="status" data-testid="action-note">
        {note?.text ?? ""}
      </p>
    </div>
  );
}

/** "More about it": the README's section, in a new tab. */
function PrivacyLink({ children }: { children: string }) {
  return (
    <a href={RECORDING_PRIVACY_URL} target="_blank" rel="noopener noreferrer">
      {children}
    </a>
  );
}

/** The notice before the first Start (#136): Start goes ahead, Cancel
 *  starts nothing; "Don't show this again" is remembered in storage.local. */
function RecordingNotice({ onAnswer }: { onAnswer: (proceed: boolean, dontShowAgain: boolean) => void }) {
  const [dontShow, setDontShow] = useState(DONT_SHOW_AGAIN_DEFAULT);
  return (
    <section className="rec-notice" role="alertdialog" aria-labelledby="rec-notice-title" aria-describedby="rec-notice-desc" data-testid="notice">
      <h2 id="rec-notice-title">{N.title}</h2>
      <div id="rec-notice-desc">
        {N.body.map((p) => (
          <p key={p}>{p}</p>
        ))}
      </div>
      <p className="rec-notice-small">
        {N.local} <PrivacyLink>{N.readMore}</PrivacyLink>
      </p>
      <label className="check">
        <input type="checkbox" checked={dontShow} onChange={(e) => setDontShow(e.target.checked)} data-testid="notice-dont-show" />
        <span>{N.dontShowAgain}</span>
      </label>
      <div className="actions">
        <button type="button" className="btn primary" autoFocus data-testid="notice-proceed" onClick={() => onAnswer(true, dontShow)}>
          {N.proceed}
        </button>
        <button type="button" className="btn" data-testid="notice-cancel" onClick={() => onAnswer(false, dontShow)}>
          {N.cancel}
        </button>
      </div>
    </section>
  );
}

function Capture({ tabId, pairing }: { tabId: number; pairing: Pairing }) {
  const [state, setState] = useState<PanelState | null>(null);
  const [needsGrant, setNeedsGrant] = useState(false);
  const noticeNeeded = useNoticeNeeded();
  const [asking, setAsking] = useState(false);
  useEffect(() => setAsking(false), [tabId]);

  const refresh = useCallback(() => {
    void browser.runtime
      .sendMessage({ type: "panel:get", tabId } satisfies PanelRequest)
      .then((s) => setState(s as PanelState))
      .catch(() => setState(null));
  }, [tabId]);

  useEffect(() => {
    setState(null);
    refresh();
    const onMsg = (raw: unknown) => {
      const m = raw as PanelBroadcast;
      if (m?.type === "panel:state" && m.state.tabId === tabId) setState(m.state);
    };
    const onUpdated = (id: number, info: { status?: string }) => {
      if (id === tabId && info.status === "complete") refresh();
    };
    browser.runtime.onMessage.addListener(onMsg);
    browser.tabs.onUpdated.addListener(onUpdated);
    // Firefox lets the user withhold MV3 host permissions: offer to grant.
    void browser.permissions.contains({ origins: [...MEETING_MATCHES] }).then((ok) => setNeedsGrant(!ok));
    return () => {
      browser.runtime.onMessage.removeListener(onMsg);
      browser.tabs.onUpdated.removeListener(onUpdated);
    };
  }, [tabId, refresh]);

  const send = (type: "panel:start" | "panel:stop") => void browser.runtime.sendMessage({ type, tabId } satisfies PanelRequest);
  const grant = () => {
    void browser.permissions.request({ origins: [...MEETING_MATCHES] }).then((ok) => {
      setNeedsGrant(!ok);
      refresh();
    });
  };

  const { t, announce } = useTranscript(tabId);
  const view = panelView(state);
  const cap = state?.capture;
  const capturing = !!state && ["arming", "connecting", "live", "reconnecting"].includes(state.phase);
  const backlog = t && (capturing || state?.phase === "stopping") ? backlogView(t) : null;
  const start = () => (startStep(noticeNeeded) === "ask" ? setAsking(true) : send("panel:start"));
  const answer = (proceed: boolean, dontShowAgain: boolean) => {
    setAsking(false);
    if (proceed) send("panel:start");
    // Best effort: if it can't be stored, the notice only shows again.
    void answerNotice(proceed, dontShowAgain).catch(() => {});
  };
  return (
    <>
      {needsGrant && (
        <p className="page-note">
          Sussurro needs access to the meeting sites to capture them.{" "}
          <button type="button" className="btn link" onClick={grant}>
            Allow access
          </button>
        </p>
      )}
      <p className={`page-note tone-${view.tone}`} role="status" data-testid="status" data-phase={state?.phase ?? ""} data-transport={state?.transport ?? ""}>
        {view.line}
      </p>
      {asking && view.canStart ? (
        <RecordingNotice onAnswer={answer} />
      ) : (
        <div className="capture-controls">
          {view.canStop ? (
            <button type="button" className="btn" data-testid="stop" onClick={() => send("panel:stop")}>
              Stop
            </button>
          ) : (
            <button type="button" className="btn primary" data-testid="start" disabled={!view.canStart} onClick={start}>
              Start recording
            </button>
          )}
        </div>
      )}
      {capturing && (
        <p className="rec-reminder" role="note" data-testid="reminder">
          {N.reminder} <PrivacyLink>{N.reminderMore}</PrivacyLink>
        </p>
      )}
      {capturing && cap?.armed && (
        <div className="meters">
          <Meter label="mic" via={cap.mic.via} level={cap.mic.level} />
          <Meter label="remote" via={state?.tabCapture === "on" ? "tab audio" : cap.remote.via} level={cap.remote.level} />
          {cap.ctxState === "suspended" && <p className="page-note">Click anywhere in the meeting page to let the browser start the audio.</p>}
          {state?.tabCapture === "failed" && <p className="page-note">Could not capture the tab's audio: the other participants may be missing.</p>}
        </div>
      )}
      {backlog && (
        <p className={`backlog backlog-${backlog.tone}`} data-testid="backlog">
          {backlog.text}
        </p>
      )}
      {t?.warning && capturing && <p className="page-note">Sussurro: {t.warning}</p>}
      <ItemActions pairing={pairing} t={t} state={state} />
      <LiveLines t={t} recording={isRecording(state?.phase ?? null)} />
      {/* New lines, read out politely (never interrupting). */}
      <div className="sr-only" aria-live="polite" aria-atomic="true" data-testid="announce">
        {announce}
      </div>
    </>
  );
}

function SidePanel() {
  const pairing = usePairing();
  const tabId = useTabId();
  return (
    <main className="page sidepanel">
      <Header title="Sussurro" />
      {pairing === null && <PairingNote />}
      {pairing && tabId !== null ? <Capture tabId={tabId} pairing={pairing} /> : <LiveLines t={null} recording={false} />}
    </main>
  );
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <SidePanel />
  </StrictMode>,
);

/* Side panel (Chrome) / sidebar (Firefox): the live mirror of a meeting
   (plan E3: editing happens in the app). Shows whether the extension is
   paired with the app (#127) and, once paired, the explicit Start/Stop of
   capture for the active tab with a level meter per channel (#128). Live
   lines, speaker chips and "Open in Sussurro" arrive with #129. */
import { StrictMode, useCallback, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import browser from "webextension-polyfill";
import { TranscriptView } from "@sussurro/transcript";
import { Header } from "../shared/Header";
import { MEETING_MATCHES } from "../shared/platform";
import { usePairing } from "../shared/usePairing";
import type { PanelBroadcast, PanelRequest, PanelState } from "../shared/messages";
import { panelView, viaText } from "./status";
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

function Capture({ tabId }: { tabId: number }) {
  const [state, setState] = useState<PanelState | null>(null);
  const [needsGrant, setNeedsGrant] = useState(false);

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

  const view = panelView(state);
  const cap = state?.capture;
  const capturing = !!state && ["arming", "connecting", "live", "reconnecting"].includes(state.phase);
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
      <p className={`page-note tone-${view.tone}`} role="status" data-testid="status" data-phase={state?.phase ?? ""}>
        {view.line}
      </p>
      <div className="capture-controls">
        {view.canStop ? (
          <button type="button" className="btn" data-testid="stop" onClick={() => send("panel:stop")}>
            Stop
          </button>
        ) : (
          <button type="button" className="btn primary" data-testid="start" disabled={!view.canStart} onClick={() => send("panel:start")}>
            Start recording
          </button>
        )}
      </div>
      {capturing && cap?.armed && (
        <div className="meters">
          <Meter label="mic" via={cap.mic.via} level={cap.mic.level} />
          <Meter label="remote" via={state?.tabCapture === "on" ? "tab audio" : cap.remote.via} level={cap.remote.level} />
          {cap.ctxState === "suspended" && <p className="page-note">Click anywhere in the meeting page to let the browser start the audio.</p>}
          {state?.tabCapture === "failed" && <p className="page-note">Could not capture the tab's audio: the other participants may be missing.</p>}
        </div>
      )}
    </>
  );
}

function SidePanel() {
  const pairing = usePairing();
  const tabId = useTabId();
  return (
    <main className="page">
      <Header title="Sussurro" />
      {pairing === null && <PairingNote />}
      {pairing && tabId !== null && <Capture tabId={tabId} />}
      <div className="tx-scroll">
        <TranscriptView
          lines={[]}
          follow
          label="Live transcript"
          emptyText="During a meeting, the transcript appears here as Sussurro writes it."
        />
      </div>
    </main>
  );
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <SidePanel />
  </StrictMode>,
);

/* Side panel (Chrome) / sidebar (Firefox): the live mirror of a meeting
   (plan E3: editing happens in the app). Renders the shared transcript view,
   empty, and whether the extension is paired with the app (#127). Live
   lines, speaker chips and Start/Stop arrive with #129. */
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import browser from "webextension-polyfill";
import { TranscriptView } from "@sussurro/transcript";
import { Header } from "../shared/Header";
import { usePairing } from "../shared/usePairing";
import "../shared/page.css";

function PairingNote() {
  const pairing = usePairing();
  if (pairing === undefined) return null;
  if (pairing === null) {
    return (
      <div className="page-note" role="status">
        Not paired with the Sussurro app.{" "}
        <button type="button" className="btn link" onClick={() => browser.runtime.openOptionsPage()}>
          Open options
        </button>
      </div>
    );
  }
  return <p className="page-note">Paired with Sussurro on port {pairing.port}.</p>;
}

function SidePanel() {
  return (
    <main className="page">
      <Header title="Sussurro" />
      <PairingNote />
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

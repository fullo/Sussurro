/* Side panel (Chrome) / sidebar (Firefox): the live mirror of a meeting
   (plan E3: editing happens in the app). Scaffold only (#125): renders the
   shared transcript view, empty. Live lines, speaker chips and Start/Stop
   arrive with #129. */
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { TranscriptView } from "@sussurro/transcript";
import { Header } from "../shared/Header";
import "../shared/page.css";

function SidePanel() {
  return (
    <main className="page">
      <Header title="Sussurro" />
      <p className="page-note">Not connected to the Sussurro app yet.</p>
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

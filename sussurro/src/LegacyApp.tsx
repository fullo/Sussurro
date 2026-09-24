import { useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { AboutDialog } from "./components/AboutDialog";
import { DictatePill } from "./components/DictatePill";
import type { Ctl } from "./hooks/useAppController";
import { useEngineRuns } from "./hooks/useEngineRuns";
import { AudioFileCard, MicSessionNotice } from "./settings/AudioFileCard";
import { BehaviorCard } from "./settings/BehaviorCard";
import { CleanupCard } from "./settings/CleanupCard";
import { DictationCard } from "./settings/DictationCard";
import { ExtensionCard } from "./settings/ExtensionCard";
import { HistoryCard } from "./settings/HistoryCard";
import { PersonalizationCard } from "./settings/PersonalizationCard";
import { SetupBanner } from "./settings/SetupBanner";
import { SpeechCard } from "./settings/SpeechCard";

/** The classic single-column window (the default while `ui_v2` is off). */
export function LegacyApp({ ctl }: { ctl: Ctl }) {
  const [aboutOpen, setAboutOpen] = useState(false);
  const { state, busy, version } = ctl;
  // Long-form runs: a file from the Audio file card, or a session started in
  // the workspace before ui_v2 was switched off (#158).
  const engine = useEngineRuns();

  return (
    <main>
      <header className="masthead">
        <div className="brand">
          <span className={`daruma ${state}`} aria-hidden="true" />
          <h1>Sussurro</h1>
        </div>
        <p className="tagline">Local dictation. Your voice never leaves this machine.</p>
        <DictatePill ctl={ctl} />
        {busy && <p className="busy" role="alert">{busy}</p>}
      </header>

      <MicSessionNotice ctl={ctl} engine={engine} />
      <SetupBanner ctl={ctl} />

      <DictationCard ctl={ctl} />
      <SpeechCard ctl={ctl} />
      <CleanupCard ctl={ctl} />
      <PersonalizationCard ctl={ctl} />
      <BehaviorCard ctl={ctl} />
      <ExtensionCard ctl={ctl} />
      <HistoryCard ctl={ctl} />
      <AudioFileCard ctl={ctl} engine={engine} />

      <footer>
        <div className="footer-top">
        <span>すべてローカル — everything stays local</span>
        <span className="footer-actions">
          <button
            className="btn-ghost"
            title="Copy version, OS and configuration to the clipboard — paste it into a bug report (no personal content included)"
            onClick={ctl.copyDiagnostics}
          >
            Copy diagnostics
          </button>
          <button className="btn-ghost" onClick={ctl.checkForUpdates}>
            Check for updates
          </button>
        </span>
        </div>
        <div className="footer-credit">
          <span className="daruma-mini" aria-hidden="true" />
          Sussurro {version && `${version} `}by{" "}
          <a
            href="https://darumahq.it"
            onClick={(e) => {
              e.preventDefault();
              openUrl("https://darumahq.it/");
            }}
          >
            DarumaHQ.it
          </a>
          {" · "}
          <button
            type="button"
            className="link-btn"
            onClick={() => setAboutOpen(true)}
          >
            About
          </button>
        </div>
      </footer>

      {aboutOpen && (
        <AboutDialog version={version} onClose={() => setAboutOpen(false)} />
      )}
    </main>
  );
}

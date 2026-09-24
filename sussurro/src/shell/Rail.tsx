import { useEffect, useState, type ReactNode } from "react";
import { DictatePill } from "../components/DictatePill";
import type { Ctl } from "../hooks/useAppController";
import type { EngineRuns } from "../hooks/useEngineRuns";
import { isRunning } from "../lib/engineRuns";
import { formatClock } from "../lib/format";
import { sttLabel, cleanupLabel } from "./labels";

export type Screen = "new" | "library" | "people" | "recipes" | "models" | "settings";

const NAV: { id: Screen; label: string; icon: ReactNode }[] = [
  {
    id: "new",
    label: "New",
    icon: <path d="M12 5v14M5 12h14" />,
  },
  {
    id: "library",
    label: "Library",
    icon: (
      <>
        <rect x="4" y="4" width="16" height="16" rx="2" />
        <path d="M8 9h8M8 13h8M8 17h5" />
      </>
    ),
  },
  {
    id: "people",
    label: "People",
    icon: (
      <>
        <circle cx="9" cy="8" r="3.5" />
        <path d="M2.5 20a6.5 6.5 0 0 1 13 0" />
        <path d="M16 4.5a3.5 3.5 0 0 1 0 7M18 14.2a6.5 6.5 0 0 1 3.5 5.8" />
      </>
    ),
  },
  {
    id: "recipes",
    label: "Recipes",
    icon: (
      <>
        <path d="M7 3h8l4 4v14H7z" />
        <path d="M15 3v4h4M10 12h6M10 16h6" />
      </>
    ),
  },
  {
    id: "models",
    label: "Models",
    icon: (
      <>
        <path d="M12 3 20 7.5v9L12 21 4 16.5v-9z" />
        <path d="M4 7.5 12 12l8-4.5M12 12v9" />
      </>
    ),
  },
  {
    id: "settings",
    label: "Settings",
    icon: (
      <>
        <circle cx="12" cy="12" r="3" />
        <path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z" />
      </>
    ),
  },
];

/** Elapsed seconds since `since`, ticking once a second while `on`. */
function useElapsed(since: number | null, on: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!on) return;
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, [on]);
  return since === null ? 0 : Math.max(0, (now - since) / 1000);
}

export function Rail({
  ctl,
  engine,
  screen,
  onNavigate,
  libraryCount,
}: {
  ctl: Ctl;
  engine: EngineRuns;
  screen: Screen;
  onNavigate: (s: Screen) => void;
  libraryCount: number | null;
}) {
  const mic = engine.runs.mic;
  const micLive = isRunning(mic) && mic.status === "running";
  const elapsed = useElapsed(micLive ? mic.startedAt : null, micLive);
  // Red is the recording moment: a dictation or a mic session in progress.
  const recording = ctl.state === "recording" || micLive;
  const daruma = recording ? "recording" : ctl.state === "processing" || isRunning(engine.runs.file) || isRunning(engine.runs.link) ? "processing" : ctl.state;

  return (
    <aside className="sh-rail" aria-label="Sussurro">
      <div className="sh-brand">
        <span className={`daruma ${daruma}`} aria-hidden="true" />
        <span className="sh-brand-name">Sussurro</span>
      </div>
      <nav className="sh-nav" aria-label="Main">
        {NAV.map((n) => (
          <button
            key={n.id}
            type="button"
            className={`sh-nav-item${screen === n.id ? " active" : ""}`}
            aria-current={screen === n.id ? "page" : undefined}
            title={n.label}
            onClick={() => onNavigate(n.id)}
          >
            <svg className="sh-ic" viewBox="0 0 24 24" aria-hidden="true" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round">
              {n.icon}
            </svg>
            <span className="sh-nav-label">{n.label}</span>
            {n.id === "library" && libraryCount !== null && libraryCount > 0 && (
              <span className="sh-count" aria-label={`${libraryCount} items`}>{libraryCount}</span>
            )}
            {n.id === "new" && micLive && <span className="sh-rec-dot" aria-label="Recording" />}
          </button>
        ))}
      </nav>
      <div className="sh-foot">
        {micLive && (
          <button type="button" className="sh-live" onClick={() => onNavigate("new")} title="Microphone session recording — open New" aria-label={`Microphone session recording, ${formatClock(elapsed)} — open New`}>
            <span aria-hidden="true">●</span> <span className="sh-foot-text">Rec {formatClock(elapsed)}</span>
          </button>
        )}
        <DictatePill ctl={ctl} className="sh-pill" />
        <span className="sh-foot-text">STT: {sttLabel(ctl.settings)}</span>
        <span className="sh-foot-text">Cleanup: {cleanupLabel(ctl.settings)}</span>
      </div>
    </aside>
  );
}

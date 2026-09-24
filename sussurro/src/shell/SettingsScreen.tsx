import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { CollapsibleCard, Tip } from "../components/ui";
import type { Ctl } from "../hooks/useAppController";
import { fileManagerName, fmtCount } from "../lib/format";
import { BehaviorCard } from "../settings/BehaviorCard";
import { CleanupCard } from "../settings/CleanupCard";
import { DictationCard } from "../settings/DictationCard";
import { HistoryCard } from "../settings/HistoryCard";
import { PersonalizationCard } from "../settings/PersonalizationCard";
import { SetupBanner } from "../settings/SetupBanner";
import { SpeechOptionsCard } from "../settings/SpeechCard";
import { sttLabel } from "./labels";

export type SectionId =
  | "dictation"
  | "speech"
  | "cleanup"
  | "personalization"
  | "behavior"
  | "history"
  | "archive"
  | "about";

const SECTIONS: { id: SectionId; label: string }[] = [
  { id: "dictation", label: "Dictation" },
  { id: "speech", label: "Speech" },
  { id: "cleanup", label: "Cleanup" },
  { id: "personalization", label: "Dictionary & snippets" },
  { id: "behavior", label: "Behavior" },
  { id: "history", label: "Dictation history" },
  { id: "archive", label: "Archive" },
  { id: "about", label: "About" },
];

/** Settings: every card of the classic window, one section at a time, plus
 *  the Archive folder. The cards are the same components. */
export function SettingsScreen({
  ctl,
  section,
  onSection,
  onOpenModels,
  onAbout,
}: {
  ctl: Ctl;
  section: SectionId;
  onSection: (s: SectionId) => void;
  onOpenModels: () => void;
  onAbout: () => void;
}) {
  return (
    <div className="sh-screen">
      <header className="sh-topbar">
        <h1>Settings</h1>
      </header>
      <div className="set-body">
        <nav className="subnav" aria-label="Settings sections">
          {SECTIONS.map((s) => (
            <button
              key={s.id}
              type="button"
              className={`subnav-item${section === s.id ? " active" : ""}`}
              aria-current={section === s.id ? "page" : undefined}
              onClick={() => onSection(s.id)}
            >
              {s.label}
            </button>
          ))}
        </nav>
        <div className="sh-scroll set-content">
          {section === "dictation" && (
            <>
              <SetupBanner ctl={ctl} />
              <DictationCard
                ctl={ctl}
                collapsible={false}
                footer={
                  <div className="card-foot">
                    <p className="card-hint">
                      Dictation lives in the tray and pastes where your cursor is — this window can stay closed.
                      It does not create Library items.
                    </p>
                    <p className="card-hint">
                      History: {ctl.stats ? `${fmtCount(ctl.stats.total_dictations)} dictations` : "your recent dictations"}{" "}
                      · <button type="button" className="link-btn" onClick={() => onSection("history")}>Open history</button>
                    </p>
                  </div>
                }
              />
            </>
          )}
          {section === "speech" && (
            <SpeechOptionsCard
              ctl={ctl}
              footer={
                <p className="card-hint">
                  Engine and model: <strong>{sttLabel(ctl.settings)}</strong> ·{" "}
                  <button type="button" className="link-btn" onClick={onOpenModels}>Change in Models</button>
                </p>
              }
            />
          )}
          {section === "cleanup" && <CleanupCard ctl={ctl} collapsible={false} />}
          {section === "personalization" && <PersonalizationCard ctl={ctl} collapsible={false} />}
          {section === "behavior" && <BehaviorCard ctl={ctl} collapsible={false} />}
          {section === "history" && <HistoryCard ctl={ctl} collapsible={false} />}
          {section === "archive" && <ArchiveCard ctl={ctl} />}
          {section === "about" && <AboutCard ctl={ctl} onAbout={onAbout} />}
        </div>
      </div>
    </div>
  );
}

function ArchiveCard({ ctl }: { ctl: Ctl }) {
  const { settings, save, setBusy } = ctl;
  const [dir, setDir] = useState("");
  const [rebuilding, setRebuilding] = useState(false);

  useEffect(() => {
    invoke<string>("archive_dir").then(setDir).catch((e) => setDir(String(e)));
  }, [settings.archive_dir]);

  return (
    <CollapsibleCard storageKey="archiveOpen" title="Archive" collapsible={false}>
      <div className="field field-col">
        <div className="field-label">
          <span>Archive folder <Tip text="Notes and transcriptions are saved here as markdown files, one folder per item. Point it at another folder (an Obsidian vault, a synced drive) if you like. Changing it does not move items already saved." /></span>
          <small>{settings.archive_dir.trim() ? "custom location" : "default: Documents/Sussurro"}</small>
        </div>
        <div className="model-row">
          <span className="path" title={dir}>{dir}</span>
        </div>
        <div className="list-actions start">
          <button
            type="button"
            className="btn-ghost"
            onClick={async () => {
              const picked = await openDialog({ title: "Choose the archive folder", directory: true, multiple: false });
              if (!picked || typeof picked !== "string") return;
              if (await save({ ...settings, archive_dir: picked })) {
                ctl.flash("Archive folder changed. Items already saved stay in the old folder.");
              }
            }}
          >
            Change…
          </button>
          {settings.archive_dir.trim() && (
            <button type="button" className="btn-ghost" onClick={() => save({ ...settings, archive_dir: "" })}>
              Use the default
            </button>
          )}
          <button
            type="button"
            className="btn-ghost"
            onClick={() => invoke("archive_reveal", { id: null }).catch((e) => setBusy(String(e)))}
          >
            Show in {fileManagerName()}
          </button>
        </div>
      </div>
      <div className="field">
        <div className="field-label">
          <span>Search index <Tip text="The Library's full-text search uses an index kept in the app data folder, built from the markdown files. Rebuild it if search results look out of date." /></span>
          <small>derived from the files, safe to rebuild</small>
        </div>
        <button
          type="button"
          className="btn-ghost"
          disabled={rebuilding}
          onClick={async () => {
            setRebuilding(true);
            try {
              const n = await invoke<number>("archive_rebuild_index");
              ctl.flash(`Search index rebuilt: ${n} item${n === 1 ? "" : "s"}.`);
            } catch (e) {
              setBusy(String(e));
            } finally {
              setRebuilding(false);
            }
          }}
        >
          {rebuilding ? "Rebuilding…" : "Rebuild"}
        </button>
      </div>
    </CollapsibleCard>
  );
}

function AboutCard({ ctl, onAbout }: { ctl: Ctl; onAbout: () => void }) {
  return (
    <CollapsibleCard storageKey="aboutOpen" title={<>Sussurro {ctl.version}</>} collapsible={false}>
      <p className="card-hint">Local dictation and transcription. Your voice never leaves this machine.</p>
      <div className="list-actions start">
        <button type="button" className="btn-ghost" onClick={onAbout}>Credits & licenses</button>
        <button
          type="button"
          className="btn-ghost"
          title="Copy version, OS and configuration to the clipboard — paste it into a bug report (no personal content included)"
          onClick={ctl.copyDiagnostics}
        >
          Copy diagnostics
        </button>
        <button type="button" className="btn-ghost" onClick={ctl.checkForUpdates}>Check for updates</button>
      </div>
      <p className="card-hint">
        This is the workspace preview. To go back to the classic window, switch off Behavior → Advanced → New
        workspace.
      </p>
      <button type="button" className="btn-ghost" onClick={() => ctl.save({ ...ctl.settings, ui_v2: false })}>
        Back to the classic window
      </button>
    </CollapsibleCard>
  );
}

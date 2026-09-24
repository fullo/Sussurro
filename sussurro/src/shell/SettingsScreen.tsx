import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { Card, Switch, Tip } from "../components/ui";
import type { Ctl } from "../hooks/useAppController";
import { fileManagerName, fmtCount } from "../lib/format";
import type { SubtitlesMode } from "../lib/types";
import { BehaviorCard } from "../settings/BehaviorCard";
import { CleanupCard } from "../settings/CleanupCard";
import { DictationCard } from "../settings/DictationCard";
import { ExtensionCard } from "../settings/ExtensionCard";
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
  | "extension"
  | "about";

const SECTIONS: { id: SectionId; label: string }[] = [
  { id: "dictation", label: "Dictation" },
  { id: "speech", label: "Speech" },
  { id: "cleanup", label: "Cleanup" },
  { id: "personalization", label: "Dictionary & snippets" },
  { id: "behavior", label: "Behavior" },
  { id: "history", label: "Dictation history" },
  { id: "archive", label: "Archive" },
  { id: "extension", label: "Browser extension" },
  { id: "about", label: "About" },
];

/** Settings: one section at a time — dictation, speech, cleanup, dictionary,
 *  behavior, history, archive, browser extension and About. */
export function SettingsScreen({
  ctl,
  section,
  onSection,
  onOpenModels,
  onOpenRecipes,
  onAbout,
  onRunSetup,
}: {
  ctl: Ctl;
  section: SectionId;
  onSection: (s: SectionId) => void;
  onOpenModels: () => void;
  onOpenRecipes: () => void;
  onAbout: () => void;
  /** Reopen the first-run setup (#115). */
  onRunSetup: () => void;
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
          {section === "cleanup" && <CleanupCard ctl={ctl} onEditProfiles={onOpenRecipes} />}
          {section === "personalization" && <PersonalizationCard ctl={ctl} />}
          {section === "behavior" && <BehaviorCard ctl={ctl} />}
          {section === "history" && <HistoryCard ctl={ctl} />}
          {section === "archive" && <ArchiveCard ctl={ctl} />}
          {section === "extension" && <ExtensionCard ctl={ctl} />}
          {section === "about" && <AboutCard ctl={ctl} onAbout={onAbout} onRunSetup={onRunSetup} />}
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
    <Card title="Archive">
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
          <span>Save audio <Tip text="Preselects Save audio in New for recordings, files and links (and for meetings from the browser extension). The audio is saved as audio.wav in each item's folder, inside the archive folder above — so it syncs wherever that folder syncs (iCloud Drive, OneDrive…). About 115 MB per hour. Off by default: audio is saved only when you ask." /></span>
          <small>{settings.save_audio ? "every new item keeps its audio" : "only when ticked in New"}</small>
        </div>
        <Switch
          checked={!!settings.save_audio}
          onChange={(v) => save({ ...settings, save_audio: v })}
        />
      </div>
      <div className="field">
        <div className="field-label">
          <span>Subtitles <Tip text="For meetings and transcriptions (notes never get subtitles): transcript.srt, next to the transcript, with at most two short rows per subtitle. Automatically, it is written and kept up to date every time the transcript is saved — but never over a transcript.srt you edited yourself. Otherwise, use Create .srt or Export in the document's side panel." /></span>
          <small>transcript.srt for meetings and transcriptions</small>
        </div>
        <select
          value={settings.subtitles ?? "on_request"}
          onChange={(e) => save({ ...settings, subtitles: e.target.value as SubtitlesMode })}
          aria-label="Subtitles"
        >
          <option value="on_request">Only when I ask</option>
          <option value="always">Create .srt automatically on every save</option>
        </select>
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
    </Card>
  );
}

function AboutCard({ ctl, onAbout, onRunSetup }: { ctl: Ctl; onAbout: () => void; onRunSetup: () => void }) {
  return (
    <Card title={<>Sussurro {ctl.version}</>}>
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
      <div className="field">
        <div className="field-label">
          <span>First-run setup</span>
          <small>permissions, archive folder, speech model, cleanup and shortcut</small>
        </div>
        <button type="button" className="btn-ghost" onClick={onRunSetup}>Run the setup again</button>
      </div>
    </Card>
  );
}

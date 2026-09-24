import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { CollapsibleCard, Tip } from "../components/ui";
import { LANGUAGES } from "../lib/constants";
import {
  describeDictionaryMerge,
  describeSnippetMerge,
  mergeDictionary,
  mergeSnippets,
  parseDictionaryFile,
  parseSnippetFile,
} from "../utils";
import type { CardProps } from "./DictationCard";

export function PersonalizationCard({ ctl, collapsible }: CardProps) {
  const { settings, setSettings, save, setBusy } = ctl;
  /** Raw text of the Personal Dictionary field. The parsed list round-trips
   *  losslessly only when every line is non-empty — deriving the displayed
   *  text from split/trim/join swallowed typed newlines (issue #88) — so the
   *  textarea keeps its own raw text and resyncs from settings when the
   *  dictionary changes elsewhere (learned words, portable config import).
   */
  const [dictText, setDictText] = useState("");
  const dictRef = useRef<HTMLTextAreaElement>(null);
  const dictionaryKey = settings.dictionary.join("\n");

  useEffect(() => {
    // Resync the field from settings only when it is not focused, so
    // external updates show up without fighting the user's cursor.
    if (document.activeElement !== dictRef.current) setDictText(dictionaryKey);
  }, [dictionaryKey]);

  // Bulk import: pick a file, read its text via the narrow `read_import_file`
  // command (.txt/.csv only), then MERGE into the current list — existing
  // entries are never replaced. Empty/unparseable files change nothing.
  const readImportFile = async (title: string, name: string, ext: string) => {
    const path = await openDialog({
      title,
      multiple: false,
      directory: false,
      filters: [{ name, extensions: [ext] }],
    });
    if (!path || typeof path !== "string") return null;
    return invoke<string>("read_import_file", { path });
  };

  const handleImportDictionary = async () => {
    try {
      const content = await readImportFile("Import dictionary", "Text files", "txt");
      if (content === null) return;
      const words = parseDictionaryFile(content);
      if (words.length === 0) {
        setBusy("Import failed: no words found — expected a .txt file with one word or phrase per line.");
        return;
      }
      const result = mergeDictionary(settings.dictionary, words);
      if (result.added > 0 && !(await save({ ...settings, dictionary: result.merged }))) return;
      ctl.flash(describeDictionaryMerge(result));
    } catch (e) {
      setBusy(String(e));
    }
  };

  const handleImportSnippets = async () => {
    try {
      const content = await readImportFile("Import snippets", "CSV files", "csv");
      if (content === null) return;
      const imported = parseSnippetFile(content);
      if (imported.length === 0) {
        setBusy('Import failed: no snippets found — expected a .csv file with one "cue,text" per line.');
        return;
      }
      const result = mergeSnippets(settings.snippets, imported);
      if (result.added > 0 && !(await save({ ...settings, snippets: result.merged }))) return;
      ctl.flash(describeSnippetMerge(result));
    } catch (e) {
      setBusy(String(e));
    }
  };

  return (
    <CollapsibleCard
      storageKey="snippetsOpen"
      title={<>Personalization <span className="via">dictionary · styles · snippets</span></>}
      collapsible={collapsible}
    >
      <div className="field field-col">
        <div className="field-label">
          <span>Personal dictionary <Tip text="Names, brands and jargon the models tend to misspell (e.g. Sussurro, Tauri). One per line. They are fed to Whisper as recognition hints and to the LLM as preferred spellings." /></span>
          <small>names & jargon, one per line — biases both Whisper and the LLM</small>
        </div>
        <textarea
          ref={dictRef}
          rows={3}
          value={dictText}
          aria-label="Personal dictionary"
          onChange={(e) => {
            setDictText(e.target.value);
            setSettings({
              ...settings,
              dictionary: e.target.value.split("\n").map((w) => w.trim()).filter(Boolean),
            });
          }}
          onBlur={() => save(settings)}
          spellCheck={false}
          placeholder="Sussurro&#10;Tauri"
        />
        <div className="list-actions">
          <button
            className="btn-ghost"
            onClick={handleImportDictionary}
            title="Add words from a .txt file (one per line) — existing words are kept"
          >
            Import .txt
          </button>
        </div>
      </div>

      <div className="field field-col">
        <div className="field-label">
          <span>App styles <Tip text="Per-application rules: when you dictate into an app whose name contains the match (e.g. 'slack'), the tone instruction is added to the cleanup prompt, and the rule's output language (if set) overrides the global 'Translate to'. Example: slack → casual + English; whatsapp → informal + Italiano." /></span>
          <small>tone and output language per app</small>
        </div>
        {settings.app_styles.map((s, i) => (
          <div className="snippet-row" key={i}>
            <input
              placeholder="app name contains…"
              value={s.app_match}
              onChange={(e) => {
                const app_styles = settings.app_styles.slice();
                app_styles[i] = { ...s, app_match: e.target.value };
                setSettings({ ...settings, app_styles });
              }}
              onBlur={() => save(settings)}
              spellCheck={false}
            />
            <textarea
              placeholder="tone instruction for the LLM"
              rows={2}
              value={s.style}
              onChange={(e) => {
                const app_styles = settings.app_styles.slice();
                app_styles[i] = { ...s, style: e.target.value };
                setSettings({ ...settings, app_styles });
              }}
              onBlur={() => save(settings)}
            />
            <button
              className="btn-ghost"
              onClick={() =>
                save({ ...settings, app_styles: settings.app_styles.filter((_, j) => j !== i) })
              }
            >
              Remove
            </button>
            <select
              className="style-lang"
              title="Output language when dictating into this app — overrides the global 'Translate to'"
              value={s.language ?? ""}
              onChange={(e) => {
                const app_styles = settings.app_styles.slice();
                app_styles[i] = { ...s, language: e.target.value };
                save({ ...settings, app_styles });
              }}
            >
              <option value="">Output: global language</option>
              {LANGUAGES.filter(([code]) => code !== "auto").map(([code, label]) => (
                <option key={code} value={code}>Output: {label}</option>
              ))}
            </select>
          </div>
        ))}
        <button
          className="btn-ghost"
          onClick={() =>
            setSettings({
              ...settings,
              app_styles: [...settings.app_styles, { app_match: "", style: "", language: "" }],
            })
          }
        >
          + Add app style
        </button>
      </div>

      <div className="field field-col">
        <div className="field-label">
          <span>Snippets <Tip text="Example: cue 'firma email' → pastes your full signature. Matching ignores case and punctuation, and skips the AI cleanup entirely." /></span>
          <small>say a cue exactly — Sussurro pastes the full text instead of transcribing</small>
        </div>
        {settings.snippets.map((s, i) => (
        <div className="snippet-row" key={i}>
          <input
            placeholder="cue (what you say)"
            value={s.cue}
            onChange={(e) => {
              const snippets = settings.snippets.slice();
              snippets[i] = { ...s, cue: e.target.value };
              setSettings({ ...settings, snippets });
            }}
            onBlur={() => save(settings)}
            spellCheck={false}
          />
          <textarea
            placeholder="text to paste"
            rows={2}
            value={s.text}
            onChange={(e) => {
              const snippets = settings.snippets.slice();
              snippets[i] = { ...s, text: e.target.value };
              setSettings({ ...settings, snippets });
            }}
            onBlur={() => save(settings)}
            spellCheck={false}
          />
          <button
            className="btn-ghost"
            onClick={() =>
              save({ ...settings, snippets: settings.snippets.filter((_, j) => j !== i) })
            }
          >
            Remove
          </button>
        </div>
      ))}
        <button
          className="btn-ghost"
          onClick={() =>
            setSettings({ ...settings, snippets: [...settings.snippets, { cue: "", text: "" }] })
          }
        >
          + Add snippet
        </button>
        <div className="list-actions">
          <button
            className="btn-ghost"
            onClick={handleImportSnippets}
            title='Add snippets from a .csv file (one "cue,text" per line; quote text with commas or line breaks) — existing snippets are kept'
          >
            Import .csv
          </button>
        </div>
      </div>

      <div className="field">
        <div className="field-label">
          <span>Portable config <Tip text="Export your dictionary, snippets and app styles to a JSON file, or import one — to move your setup between machines (sync it with a file/Git/Syncthing, no cloud account). Import merges without duplicates; machine-specific settings like hotkeys and models folder are not included." /></span>
          <small>dictionary + snippets + styles</small>
        </div>
        <div className="model-row">
          <button
            className="btn-ghost"
            onClick={async () => {
              const path = await saveDialog({
                defaultPath: "sussurro-config.json",
                filters: [{ name: "JSON", extensions: ["json"] }],
              });
              if (!path) return;
              try {
                await invoke("export_config", { path });
                ctl.flash("Config exported.", 3000);
              } catch (e) {
                setBusy(String(e));
              }
            }}
          >
            Export
          </button>
          <button
            className="btn-ghost"
            onClick={async () => {
              const path = await openDialog({
                multiple: false,
                filters: [{ name: "JSON", extensions: ["json"] }],
              });
              if (!path || typeof path !== "string") return;
              try {
                const msg = await invoke<string>("import_config", { path });
                setBusy(msg);
                ctl.refresh();
              } catch (e) {
                setBusy(String(e));
              }
            }}
          >
            Import
          </button>
        </div>
      </div>
    </CollapsibleCard>
  );
}

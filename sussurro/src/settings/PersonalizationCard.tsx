import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { Card, Tip } from "../components/ui";
import { LANGUAGES } from "../lib/constants";
import type { CardProps } from "./DictationCard";
import { DictionaryManager } from "./DictionaryManager";
import { SnippetManager } from "./SnippetManager";

export function PersonalizationCard({ ctl }: CardProps) {
  const { settings, setSettings, save, setBusy } = ctl;
  /** People hold other people's emails (#132): opt-in per export, off by default. */
  const [includePeople, setIncludePeople] = useState(false);

  return (
    <Card title={<>Personalization <span className="via">dictionary · styles · snippets</span></>}>
      <DictionaryManager ctl={ctl} />

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

      <SnippetManager ctl={ctl} />

      <div className="field">
        <div className="field-label">
          <span>Portable config <Tip text="Export your dictionary, snippets and app styles to a JSON file, or import one — to move your setup between machines (sync it with a file/Git/Syncthing, no cloud account). Import merges without duplicates; machine-specific settings like hotkeys and models folder are not included." /></span>
          <small>dictionary + snippets + styles</small>
        </div>
        <div className="field-stack">
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
                await invoke("export_config", { path, includePeople });
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
        <label className="check-row">
          <input type="checkbox" checked={includePeople} onChange={(e) => setIncludePeople(e.target.checked)} />
          <span>
            Include People in the export
            <small> Names, emails and aliases of other people, from your archive. Off by default: tick it only if the file stays with you.</small>
          </span>
        </label>
        </div>
      </div>
    </Card>
  );
}

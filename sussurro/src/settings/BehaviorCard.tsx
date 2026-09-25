import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { save as saveDialog } from "@tauri-apps/plugin-dialog";
import { AdvancedGroup, Card, Switch, Tip } from "../components/ui";
import type { CardProps } from "./DictationCard";

export function BehaviorCard({ ctl }: CardProps) {
  const { settings, setSettings, save } = ctl;
  /** Whisper runs on the GPU in this build (null until known). */
  const [whisperGpu, setWhisperGpu] = useState<boolean | null>(null);
  useEffect(() => {
    invoke<boolean>("whisper_gpu").then(setWhisperGpu).catch(() => setWhisperGpu(null));
  }, []);
  const cpuWhisper = whisperGpu === false && settings.engine === "whisper";
  return (
    <Card title={<>Behavior <span className="via">feedback & extras</span></>}>
      <div className="field">
        <div className="field-label">
          <span>Live preview <Tip text="While you speak, the overlay shows the words heard so far: solid once two passes agree, faded while the model may still change them, and only the last lines of a long dictation. The model re-reads the recording every ~1.2 s, less often as it grows, so the preview never takes more than about a third of the engine's time. The pasted text always comes from the final, full-quality pass. The level bar in the overlay shows whether the microphone hears you, with or without the preview." /></span>
          <small>{cpuWhisper ? "off by default here: Whisper runs on the CPU" : "partial transcript in the overlay"}</small>
          {cpuWhisper && settings.live_preview && (
            <small className="model-note">
              This build runs Whisper on the CPU, so each preview pass competes with your dictation and the text may
              arrive later. Turn it off if dictation feels slow, or use Parakeet, which is fast on the CPU.
            </small>
          )}
        </div>
        <Switch
          checked={settings.live_preview}
          label="Live preview"
          onChange={(v) => save({ ...settings, live_preview: v })}
        />
      </div>

      <div className="field">
        <div className="field-label">
          <span>Sound feedback <Tip text="A short rising tick when recording starts and a falling one when it stops — so you know the trigger worked without looking at this window." /></span>
          <small>tick on start / stop</small>
        </div>
        <Switch
          checked={settings.sound_feedback}
          onChange={(v) => save({ ...settings, sound_feedback: v })}
        />
      </div>

      <div className="field">
        <div className="field-label">
          <span>Voice commands <Tip text="Interpret spoken editing commands instead of transcribing them. Always work: 'a capo'/'new line', 'nuovo paragrafo'/'new paragraph', 'punto e a capo'/'period new line', 'punto elenco'/'new bullet' (bulleted list). With cleanup on, also 'scratch that'/'cancella quello' (deletes the previous phrase) and 'quote … end quote'." /></span>
          <small>a capo, punto elenco, scratch that…</small>
        </div>
        <Switch
          checked={settings.voice_commands}
          onChange={(v) => save({ ...settings, voice_commands: v })}
        />
      </div>

      <div className="field">
        <div className="field-label">
          <span>Streaming typing <Tip text="Types the text into the app WHILE you speak. With Cleanup None it streams word by word (holding back the last 2); with cleanup on it streams sentence by sentence, each one LLM-cleaned before being typed. The final pass completes the tail when you release." /></span>
          <small>word or sentence streaming</small>
        </div>
        <Switch
          checked={settings.stream_injection}
          onChange={(v) => save({ ...settings, stream_injection: v })}
        />
      </div>

      <div className="field">
        <div className="field-label">
          <span>Launch at login <Tip text="Start Sussurro automatically when you log in. It starts hidden in the tray — click the tray icon or press your shortcut to use it." /></span>
          <small>starts hidden in the tray</small>
        </div>
        <Switch
          checked={settings.autostart}
          onChange={(v) => save({ ...settings, autostart: v })}
        />
      </div>

      <div className="field">
        <div className="field-label">
          <span>Dictate to file <Tip text="Note-taking mode: every dictation is APPENDED to this file (e.g. an Obsidian note) instead of being pasted into the focused app. Clear the path to go back to normal pasting. Streaming typing is suspended while this is active." /></span>
          <small>{settings.output_file.trim() ? "active — nothing is pasted" : "off — text is pasted normally"}</small>
        </div>
        <div className="model-row">
          {settings.output_file.trim() ? (
            <>
              <span className="output-file-path" title={settings.output_file}>
                {settings.output_file}
              </span>
              <button
                className="btn-ghost"
                onClick={() => save({ ...settings, output_file: "" })}
              >
                Stop
              </button>
            </>
          ) : (
            <button
              className="btn-ghost"
              onClick={async () => {
                const path = await saveDialog({
                  defaultPath: "sussurro-notes.md",
                  filters: [
                    { name: "Markdown", extensions: ["md"] },
                    { name: "Text", extensions: ["txt"] },
                  ],
                });
                if (path) save({ ...settings, output_file: path });
              }}
            >
              Choose file…
            </button>
          )}
        </div>
      </div>

      <AdvancedGroup>
        <div className="field">
          <div className="field-label">
            <span>Local API <Tip text="HTTP API on 127.0.0.1 (this computer only). The browser extension talks to Sussurro through it with its own token; the scripting routes below are a separate switch. Applied at app restart." /></span>
            <small>browser extension & scripts · restart required</small>
          </div>
          <div className="model-row">
            <Switch
              checked={settings.api_enabled}
              onChange={(v) => save({ ...settings, api_enabled: v })}
              label="Local API"
            />
            <input
              className="port-input"
              type="number"
              min={1024}
              max={65535}
              value={settings.api_port}
              onChange={(e) => setSettings({ ...settings, api_port: Number(e.target.value) })}
              onBlur={() => save(settings)}
              disabled={!settings.api_enabled}
              title="Port"
            />
          </div>
        </div>
        <div className="field">
          <div className="field-label">
            <span>Scripting routes <Tip text="POST /clean (text → cleaned), POST /transcribe?ext=wav (audio file → transcript) and GET /history?q= — without a token, so any program on this computer can use them (web pages can't: other sites' requests are refused). Leave off unless your own scripts use them. curl examples in the README." /></span>
            <small>token-less /clean, /transcribe, /history · applies at once</small>
          </div>
          <Switch
            checked={settings.api_scripting}
            onChange={(v) => save({ ...settings, api_scripting: v })}
            label="Scripting routes"
          />
        </div>
      </AdvancedGroup>
    </Card>
  );
}

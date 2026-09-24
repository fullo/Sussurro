import type { ReactNode } from "react";
import { HotkeyRecorder } from "../components/HotkeyRecorder";
import { Card, Switch, Tip } from "../components/ui";
import type { Ctl } from "../hooks/useAppController";

export interface CardProps {
  ctl: Ctl;
}

export function DictationCard({ ctl, footer }: CardProps & { footer?: ReactNode }) {
  const { settings, save } = ctl;
  return (
    <Card title="Dictation">
      <div className="field">
        <div className="field-label">
          <span>Shortcut <Tip text="The system-wide key combination that triggers dictation in any app. Click the field, then press the keys you want (Esc cancels)." /></span>
          <small>{settings.push_to_talk ? "hold to record" : "tap to start / stop"}</small>
        </div>
        <HotkeyRecorder
          value={settings.hotkey}
          onChange={(combo) => save({ ...settings, hotkey: combo })}
        />
      </div>

      <div className="field">
        <div className="field-label">
          <span>Microphone <Tip text="Which input device to record from. Default follows the system microphone; pick a specific one if you have several (headset, webcam, desk mic). If the chosen device is unplugged, Sussurro falls back to the default." /></span>
          <small>capture device</small>
        </div>
        <div className="mic-col">
          <div className="mic-row">
            <select
              value={settings.input_device}
              onChange={(e) => save({ ...settings, input_device: e.target.value })}
              aria-label="Microphone"
            >
              <option value="">System default</option>
              {ctl.inputDevices.map((d) => (
                <option key={d} value={d}>{d}</option>
              ))}
              {settings.input_device && !ctl.inputDevices.includes(settings.input_device) && (
                <option value={settings.input_device}>
                  {settings.input_device} (unavailable)
                </option>
              )}
            </select>
            <button
              className="btn-ghost"
              onClick={ctl.toggleMicTest}
              title="Listen to the input level to check the right microphone is picked up"
            >
              {ctl.micTest ? "Stop" : "Test"}
            </button>
          </div>
          {(ctl.micTest || ctl.recordingNow) && (
            <div
              className="vu"
              role="meter"
              aria-label="Input level"
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={Math.round(ctl.vuPct)}
            >
              <div className="vu-fill" style={{ width: `${ctl.vuPct}%` }} />
            </div>
          )}
        </div>
      </div>

      <div className="field">
        <div className="field-label">
          <span>Push-to-talk <Tip text="On: recording lasts while you hold the shortcut or the Dictate button, like a walkie-talkie. Off: one tap/click starts recording, a second one stops it. Applies to both the keyboard shortcut and the Dictate button at the bottom of the sidebar." /></span>
          <small>off = toggle mode</small>
        </div>
        <Switch
          checked={settings.push_to_talk}
          onChange={(v) => save({ ...settings, push_to_talk: v })}
        />
      </div>
      {footer}
    </Card>
  );
}

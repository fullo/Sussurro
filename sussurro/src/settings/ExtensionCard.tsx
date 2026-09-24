import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Card, Switch, Tip } from "../components/ui";
import { apiNotice } from "../lib/localApi";
import { encodePairingCode, maskedPairingCode } from "../lib/pairingCode";
import { needsMeetingNotice, withNoticeReset } from "../lib/meetingNotice";
import { RECORDING_NOTICE } from "../lib/recordingNotice";
import { RecordingPrivacyLink } from "../shell/RecordingNotice";
import type { ListenState } from "../lib/types";
import type { CardProps } from "./DictationCard";

const RELEASES_URL = "https://github.com/fullo/Sussurro/releases";
const EXTENSION_README_URL = "https://github.com/fullo/Sussurro/tree/main/extension#load-it-unpacked";

/** Settings → Browser extension (#127, E6): enable meetings, and pair the
 *  extension by copying one code (`sussurro:<port>:<token>`) into its
 *  options page. The token is never shown: it only goes to the clipboard. */
export function ExtensionCard({ ctl }: CardProps) {
  const { settings, save, setBusy, flash } = ctl;
  const [status, setStatus] = useState<ListenState | null>(null);
  const [confirmRegen, setConfirmRegen] = useState(false);
  const [working, setWorking] = useState(false);

  useEffect(() => {
    invoke<ListenState>("local_api_status").then(setStatus).catch(() => setStatus(null));
  }, [settings.api_enabled, settings.api_port]);

  const notice = apiNotice(settings.api_enabled, settings.api_port, status);

  const copyCode = async (token: string) => {
    await invoke("copy_text", { text: encodePairingCode({ port: settings.api_port, token }) });
  };

  const copy = async () => {
    setWorking(true);
    try {
      await copyCode(await invoke<string>("extension_token_get"));
      flash("Pairing code copied. Paste it into the Sussurro extension's options page.");
    } catch (e) {
      setBusy(String(e));
    } finally {
      setWorking(false);
    }
  };

  const regenerate = async () => {
    setWorking(true);
    try {
      await copyCode(await invoke<string>("extension_token_regenerate"));
      setConfirmRegen(false);
      flash("New token created and the new pairing code copied. Paste it into every browser you paired.", 6000);
    } catch (e) {
      setBusy(String(e));
    } finally {
      setWorking(false);
    }
  };

  return (
    <Card title={<>Browser extension <span className="via">meetings · preview</span></>}>
      <div className="field">
        <div className="field-label">
          <span>
            Meetings{" "}
            <Tip text="The Sussurro browser extension records web meetings (Google Meet, Microsoft Teams, Zoom in the browser) and sends the audio to this app, which transcribes it into your Library. Nothing leaves this computer. While this is off, the extension's routes of the local API don't exist." />
          </span>
          <small>{settings.meetings_enabled ? "on — preview" : "let the browser extension record meetings"}</small>
        </div>
        <Switch
          checked={settings.meetings_enabled}
          onChange={(v) => save({ ...settings, meetings_enabled: v })}
          label="Enable meetings from the browser extension"
        />
      </div>

      <div className="field">
        <div className="field-label">
          <span>
            {RECORDING_NOTICE.settingsTitle} <Tip text={RECORDING_NOTICE.reshowHint} />
          </span>
          <small>
            {needsMeetingNotice(settings) ? RECORDING_NOTICE.stateShown : RECORDING_NOTICE.stateHidden}
            {" · "}
            <RecordingPrivacyLink />
          </small>
        </div>
        <button
          type="button"
          className="btn-ghost"
          disabled={needsMeetingNotice(settings)}
          onClick={async () => {
            if (await save(withNoticeReset(settings))) flash(RECORDING_NOTICE.reshowDone);
          }}
        >
          {RECORDING_NOTICE.reshow}
        </button>
      </div>

      <div className="field field-col">
        <div className="field-label">
          <span>
            Local API{" "}
            <Tip text="The extension talks to Sussurro through the local HTTP API on 127.0.0.1 (this computer only), with its own token. The port is set in Behavior → Advanced; it applies when Sussurro restarts." />
          </span>
          <small>port {settings.api_port}</small>
        </div>
        <p className={notice.tone === "ok" ? "card-hint" : "endpoint-note"} role={notice.tone === "ok" ? undefined : "status"}>
          {notice.text}
        </p>
        {notice.offerEnable && (
          <div className="list-actions start">
            <button type="button" className="btn-ghost" onClick={() => save({ ...settings, api_enabled: true })}>
              Turn on the local API
            </button>
          </div>
        )}
      </div>

      <div className="field field-col">
        <div className="field-label">
          <span>
            Pairing code{" "}
            <Tip text="One code with the port and a secret token. Paste it into the extension's options page (right-click the Sussurro toolbar button → Options). Treat it like a password: anyone with it can talk to Sussurro's meeting routes from a browser extension on this computer." />
          </span>
          <small>copy it into the extension's options</small>
        </div>
        <div className="model-row">
          <span className="path" aria-label="Pairing code, token hidden">{maskedPairingCode(settings.api_port)}</span>
          <button type="button" className="btn-ghost" disabled={working} onClick={copy}>
            Copy pairing code
          </button>
        </div>
        {!confirmRegen ? (
          <div className="list-actions start">
            <button type="button" className="btn-ghost" disabled={working} onClick={() => setConfirmRegen(true)}>
              Regenerate token…
            </button>
          </div>
        ) : (
          <div role="alertdialog" aria-label="Regenerate the extension token">
            <p className="endpoint-note">
              The current token stops working at once: every browser you paired must be paired again with the new code.
            </p>
            <div className="list-actions start">
              <button type="button" className="btn-ghost" disabled={working} onClick={regenerate}>
                Regenerate and copy the new code
              </button>
              <button type="button" className="btn-ghost" disabled={working} onClick={() => setConfirmRegen(false)}>
                Cancel
              </button>
            </div>
          </div>
        )}
      </div>

      <p className="card-hint">
        Get the extension (Chrome, Edge, Brave, Firefox) from the{" "}
        <a
          href={RELEASES_URL}
          onClick={(e) => {
            e.preventDefault();
            openUrl(RELEASES_URL);
          }}
        >
          Sussurro releases page
        </a>{" "}
        — until it is in the browser stores, load it as explained in{" "}
        <a
          href={EXTENSION_README_URL}
          onClick={(e) => {
            e.preventDefault();
            openUrl(EXTENSION_README_URL);
          }}
        >
          the extension's README
        </a>
        .
      </p>
    </Card>
  );
}

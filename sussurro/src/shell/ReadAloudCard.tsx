import { useCallback, useEffect, useRef, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Card } from "../components/ui";
import type { Ctl } from "../hooks/useAppController";
import { formatBytes } from "../lib/format";
import {
  EXPERIMENTAL,
  canPreview,
  downloadBytes,
  downloadPrompt,
  languageStatus,
  progressFraction,
  progressLabel,
  withVoice,
} from "../lib/tts";
import type { TtsDownloadProgress, TtsLanguage, TtsStatus, TtsVoice } from "../lib/types";

export function ExperimentalBadge() {
  return (
    <span className="badge-experimental" title="An optional module still being tested: off by default">
      {EXPERIMENTAL}
    </span>
  );
}

/** What the user asked to download, waiting for the confirmation (P24). */
type Ask = { lang: TtsLanguage; voice?: TtsVoice; voiceOnly: boolean };

/** Models → Voices (#255, P18/P24): the read-aloud languages and voices —
 *  download (size and licence first, then a confirmation), delete, preview
 *  a sentence, and the voice each language reads with. Nothing downloads
 *  unless the module is on and the user confirms. */
export function ReadAloudCard({ ctl, onOpenExperimental }: { ctl: Ctl; onOpenExperimental: () => void }) {
  const { settings, save } = ctl;
  const [status, setStatus] = useState<TtsStatus | null>(null);
  const [progress, setProgress] = useState<TtsDownloadProgress | null>(null);
  const [ask, setAsk] = useState<Ask | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [previewing, setPreviewing] = useState<string | null>(null);
  const audio = useRef<HTMLAudioElement | null>(null);

  const refresh = useCallback(() => {
    invoke<TtsStatus>("tts_status")
      .then((s) => {
        setStatus(s);
        setProgress(s.downloading);
      })
      .catch(() => setStatus(null));
  }, []);
  useEffect(refresh, [refresh, settings.tts_enabled, settings.tts_voices]);

  useEffect(() => {
    let alive = true;
    const un = listen<TtsDownloadProgress | null>("tts-download-progress", (e) => {
      if (alive) setProgress(e.payload);
    });
    return () => {
      alive = false;
      un.then((f) => f()).catch(() => {});
      audio.current?.pause();
    };
  }, []);

  const enabled = !!settings.tts_enabled;
  const downloading = progress !== null;

  const download = async (a: Ask) => {
    setAsk(null);
    try {
      await invoke("tts_download", { language: a.lang.code, voice: a.voice?.id ?? null, voiceOnly: a.voiceOnly });
      ctl.flash(a.voiceOnly ? `Voice ${a.voice?.label ?? ""} downloaded.` : `${a.lang.label} read-aloud model downloaded.`, 3000);
    } catch (e) {
      const msg = String(e);
      if (!/cancelled/.test(msg)) ctl.setBusy(msg);
      else ctl.flash("Download cancelled — nothing was kept.", 3000);
    } finally {
      setProgress(null);
      refresh();
    }
  };

  const remove = async (lang: TtsLanguage, voice?: TtsVoice) => {
    const key = `${lang.code}/${voice?.id ?? "*"}`;
    setBusy(key);
    try {
      await invoke("tts_delete", { language: lang.code, voice: voice?.id ?? null });
      ctl.flash(voice ? `Voice ${voice.label} deleted.` : `${lang.label} read-aloud model and voices deleted.`, 3000);
    } catch (e) {
      ctl.setBusy(String(e));
    } finally {
      setBusy(null);
      refresh();
    }
  };

  const preview = async (lang: TtsLanguage, voice: TtsVoice) => {
    const key = `${lang.code}/${voice.id}`;
    setPreviewing(key);
    try {
      const path = await invoke<string>("tts_preview", { language: lang.code, voice: voice.id, text: null });
      audio.current?.pause();
      const el = new Audio(convertFileSrc(path, "sussurro-audio"));
      audio.current = el;
      await el.play();
    } catch (e) {
      ctl.setBusy(String(e));
    } finally {
      setPreviewing(null);
      refresh();
    }
  };

  const title = (
    <>
      Voices <span className="via">read aloud</span> <ExperimentalBadge />
    </>
  );

  if (!enabled) {
    return (
      <Card title={title}>
        <p className="card-hint">
          Read aloud is an experimental module and it is off: no voice model is downloaded or used. Turn it on in
          Settings → Experimental; then choose here which languages and voices to download.
        </p>
        <div className="row-gap">
          <button type="button" className="btn-ghost sh-btn" onClick={onOpenExperimental}>
            Open Settings → Experimental
          </button>
          {status && status.bytes_on_disk > 0 && (
            <span className="sh-muted">{formatBytes(status.bytes_on_disk)} of voice models still on disk.</span>
          )}
        </div>
      </Card>
    );
  }

  return (
    <Card title={title}>
      <p className="card-hint">
        {status?.engine ?? "Pocket TTS"} reads documents aloud on this computer, into an audio file. Models download
        only when you click Download, after you see their size and licence. {status?.attribution}; models under{" "}
        {status?.licence ?? "CC-BY-4.0"}, each voice with the licence of its recording.
      </p>
      {!status ? (
        <p className="card-hint" role="status">
          Can't read the read-aloud models right now.
        </p>
      ) : (
        <ul className="tts-langs">
          {status.languages.map((lang) => {
            const mine = progress?.language === lang.code;
            return (
              <li key={lang.code} className="tts-lang">
                <div className="row-gap">
                  <strong>{lang.label}</strong>
                  <span className="sh-muted">
                    {status.engine} · {lang.variant} · {status.licence}
                  </span>
                  <span className="push sh-muted" role="status">
                    {languageStatus(lang)}
                  </span>
                </div>
                {mine && progress ? (
                  <div className="row-gap tts-progress" role="status" aria-live="polite">
                    <div
                      className="progress"
                      role="progressbar"
                      aria-label={`Downloading ${lang.label}`}
                      aria-valuemin={0}
                      aria-valuemax={100}
                      aria-valuenow={Math.round(progressFraction(progress) * 100)}
                    >
                      <div className="progress-fill" style={{ width: `${progressFraction(progress) * 100}%` }} />
                    </div>
                    <span className="sh-muted">{progressLabel(progress)}</span>
                    <button type="button" className="btn-ghost sh-btn" onClick={() => invoke("tts_cancel_download").catch(() => {})}>
                      Cancel
                    </button>
                  </div>
                ) : (
                  <div className="row-gap">
                    {!lang.model_downloaded ? (
                      <button
                        type="button"
                        className="btn-ghost sh-btn"
                        disabled={downloading}
                        onClick={() => setAsk({ lang, voiceOnly: false })}
                      >
                        Download… ({formatBytes(downloadBytes(lang))})
                      </button>
                    ) : (
                      <button
                        type="button"
                        className="btn-ghost sh-btn"
                        disabled={downloading || busy !== null}
                        onClick={() => remove(lang)}
                        title="Delete this language's model and voices from the models folder"
                      >
                        Delete model
                      </button>
                    )}
                  </div>
                )}
                {ask?.lang.code === lang.code && (
                  <div className="row-gap prof-actions" role="alertdialog" aria-label="Confirm the download">
                    <span>{downloadPrompt(status, ask.lang, ask.voice)}</span>
                    <button type="button" className="btn-dark sh-btn push" onClick={() => download(ask)}>
                      Download
                    </button>
                    <button type="button" className="btn-ghost sh-btn" autoFocus onClick={() => setAsk(null)}>
                      Cancel
                    </button>
                  </div>
                )}
                <ul className="tts-voices" aria-label={`${lang.label} voices`}>
                  {lang.voices.map((v) => {
                    const key = `${lang.code}/${v.id}`;
                    return (
                      <li key={v.id} className="row-gap">
                        <label className="tts-voice-pick" title={v.downloaded ? "Read this language with this voice" : "Download the voice to use it"}>
                          <input
                            type="radio"
                            name={`tts-voice-${lang.code}`}
                            checked={v.selected}
                            disabled={!v.downloaded && !v.selected}
                            onChange={() => save({ ...settings, tts_voices: withVoice(settings.tts_voices, lang.code, v.id) })}
                          />
                          <span>{v.label}</span>
                        </label>
                        <span className="sh-muted tts-voice-source">{v.source}</span>
                        <span className="push" />
                        {v.downloaded ? (
                          <>
                            <button
                              type="button"
                              className="btn-ghost sh-btn"
                              disabled={!canPreview(lang, v) || previewing !== null || downloading}
                              onClick={() => preview(lang, v)}
                              title={lang.model_downloaded ? `Read a sample sentence with ${v.label}` : "Download the model first"}
                            >
                              {previewing === key ? "Reading…" : "Preview"}
                            </button>
                            <button
                              type="button"
                              className="btn-ghost sh-btn"
                              disabled={busy !== null || downloading}
                              onClick={() => remove(lang, v)}
                            >
                              Delete
                            </button>
                          </>
                        ) : (
                          <button
                            type="button"
                            className="btn-ghost sh-btn"
                            disabled={downloading}
                            onClick={() => setAsk({ lang, voice: v, voiceOnly: lang.model_downloaded })}
                          >
                            Download… ({formatBytes(v.bytes)}
                            {lang.model_downloaded ? "" : " + model"})
                          </button>
                        )}
                      </li>
                    );
                  })}
                </ul>
              </li>
            );
          })}
        </ul>
      )}
      {status && (
        <p className="card-hint">
          On disk: {formatBytes(status.bytes_on_disk)}
          {status.loaded
            ? ` · the ${status.languages.find((l) => l.code === status.loaded)?.label ?? status.loaded} model is in memory (freed after 5 minutes unused)`
            : ""}.
          The selected voice is the one each language reads with.
        </p>
      )}
    </Card>
  );
}

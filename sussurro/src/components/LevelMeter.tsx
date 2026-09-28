import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { levelToPercent } from "../lib/overlayText";
import {
  nextSilenceSince,
  shouldShowSilenceHint,
  silenceHintText,
  type LevelPreviewKind,
} from "../lib/levelPreview";

const POLL_MS = 150;

/** A live level preview next to an audio-source picker (#314): opens a
 *  lightweight capture on the picked device (never the recorder — a real
 *  recording, dictation or enrolment always wins the device, stopping this
 *  first) and polls its RMS level while mounted. No audio is kept, written
 *  or sent — only the level crosses to the UI.
 *
 *  - `kind`: which preview slot in the backend this uses (`"mic"` or
 *    `"system"`) — the two are independent, so a mic and a system meter can
 *    run at once.
 *  - `device`: the picked device name (`""` = system default for `kind:
 *    "mic"`; for `kind: "system"` a named device, ignored when `native`).
 *  - `native`: system audio only — the computer's own loopback (#140),
 *    `device` is ignored.
 *  - `enabled`: false while nothing is picked yet (e.g. the system picker
 *    before a device or the native option is chosen) — the meter renders
 *    nothing and no preview opens.
 */
export function LevelMeter({
  kind,
  device,
  native,
  enabled = true,
  label = "Input level",
}: {
  kind: LevelPreviewKind;
  device: string;
  native?: boolean;
  enabled?: boolean;
  label?: string;
}) {
  const [level, setLevel] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [started, setStarted] = useState(false);
  const silenceSince = useRef<number | null>(null);

  const isNative = kind === "system" && !!native;
  const noDeviceYet = kind === "system" && !isNative && !device;
  const active = enabled && !noDeviceYet;

  useEffect(() => {
    silenceSince.current = null;
    setLevel(0);
    setError(null);
    setStarted(false);
    if (!active) return;

    let alive = true;
    invoke("level_preview_start", { kind, device: device || "", native: !!isNative })
      .then(() => {
        if (alive) setStarted(true);
      })
      .catch((e) => {
        if (alive) setError(String(e));
      });
    return () => {
      alive = false;
      invoke("level_preview_stop", { kind }).catch(() => {});
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [kind, device, isNative, active]);

  useEffect(() => {
    if (!started) return;
    const id = window.setInterval(() => {
      invoke<number>("level_preview", { kind })
        .then((l) => {
          setError(null);
          setLevel(l);
          silenceSince.current = nextSilenceSince(l, silenceSince.current, Date.now());
        })
        .catch((e) => setError(String(e)));
    }, POLL_MS);
    return () => window.clearInterval(id);
  }, [started, kind]);

  if (!active) return null;

  if (error) {
    return (
      <p className="link-notice level-meter-error" role="alert">
        Can't check this device: {error}
      </p>
    );
  }

  const showHint = shouldShowSilenceHint(silenceSince.current, Date.now());
  return (
    <div className="level-meter-wrap">
      <div
        className="vu"
        role="meter"
        aria-label={label}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={Math.round(levelToPercent(level))}
      >
        <div className="vu-fill" style={{ width: `${levelToPercent(level)}%` }} />
      </div>
      {showHint && <small className="sh-muted level-meter-hint">{silenceHintText(kind)}</small>}
    </div>
  );
}

import { memo, useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { lineTimestamp, type TranscriptSpeaker } from "@sussurro/transcript";
import { parseDuration } from "../lib/format";
import {
  REPLAY_RATES,
  SKIP_MS,
  activeSegment,
  activeWord,
  adjacentLine,
  audioSrcPath,
  buildPlaylist,
  channelFile,
  formatPlayerTime,
  sortLines,
  speakerTalkTime,
  wordSpans,
  type ReplaySegment,
} from "../lib/replay";
import { ReplayPlayer } from "../lib/replayPlayer";
import type { Item } from "../lib/types";
import type { Ctl } from "../hooks/useAppController";
import { ReadAloudSection } from "./ReadAloudSection";

/* The document pane's Audio tab (0.10, #142): a player for the item's saved
   audio (#141) with "Play only: <speaker>", and the transcript lit as it
   plays. The logic lives in lib/replay.ts (playlists, highlighting) and
   lib/replayPlayer.ts (transport); this wires them to <audio> elements fed
   by the backend's range-serving sussurro-audio: scheme. */

/** The URL scheme served by archive::playback (backend). */
const SCHEME = "sussurro-audio";
/** Clock period while playing: 25 updates a second is smooth enough for a
 *  word highlight, and a span overshoots its end by at most this much. */
const TICK_MS = 40;

/** A line to move the player to (the Voice map, #144): `n` changes on
 *  every pick, so picking the same line again seeks again. */
export interface AudioSeek {
  id: number;
  n: number;
}

export function AudioTab({
  item,
  speakers,
  seek,
  ctl,
  onChanged,
  onOpenModels,
}: {
  item: Item;
  speakers?: TranscriptSpeaker[];
  seek?: AudioSeek | null;
  /** With it, the tab also shows generated speech (read aloud, #256). */
  ctl?: Ctl;
  onChanged?: () => void;
  onOpenModels?: () => void;
}) {
  const files = (item.audio ?? []).map((f) => f.name);
  const speech = ctl ? <ReadAloudSection ctl={ctl} item={item} onChanged={onChanged} onOpenModels={onOpenModels} /> : null;
  if (item.recording) {
    return (
      <div className="doc-scroll">
        {speech}
        <div className="au-empty">
          <p>
            <strong>Recording.</strong> The player opens when the session ends
            {files.length ? " — its audio is being saved." : "."}
          </p>
        </div>
      </div>
    );
  }
  if (files.length === 0) {
    return (
      <div className="doc-scroll">
        {speech}
        <div className="au-empty">
          <p>
            <strong>No recorded audio for this item.</strong> Sussurro keeps only the transcript unless you ask for the
            audio, so there is nothing to play back here.
          </p>
          <p className="sh-muted">
            To replay a recording — or one speaker at a time — tick <b>Save audio</b> in <b>New</b> before you start,
            or turn it on for every new item in <b>Settings → Archive</b>. The audio is saved in the item's folder, as
            Opus (about 11 MB per hour) or WAV (about 115 MB), per the Saved audio format there.
          </p>
        </div>
      </div>
    );
  }
  // Remount on another item or other files (Delete audio, a recovered item).
  const player = <Player key={`${item.id}|${files.join(",")}`} item={item} files={files} speakers={speakers} seek={seek ?? null} />;
  if (!speech) return player;
  return (
    <div className="au-with-speech">
      <div className="au-speech-slot">{speech}</div>
      <h3 className="au-recorded-head">Recorded audio</h3>
      {player}
    </div>
  );
}

/** Whether a key event belongs to the focused control rather than to the
 *  player's shortcuts: menus and text fields keep every key, buttons and
 *  checkboxes keep Space (it presses them). The seek bar keeps none: ← →
 *  are ±10 s there too, not its 0.1 s steps. */
function ownsKey(e: KeyboardEvent, key: string): boolean {
  const t = e.target as HTMLElement;
  const tag = t.tagName;
  if (tag === "SELECT" || tag === "TEXTAREA" || t.isContentEditable) return true;
  if (tag === "BUTTON" || (tag === "INPUT" && (t as HTMLInputElement).type === "checkbox")) return key === " ";
  return false;
}

function Player({
  item,
  files,
  speakers,
  seek,
}: {
  item: Item;
  files: string[];
  speakers?: TranscriptSpeaker[];
  seek: AudioSeek | null;
}) {
  const segments = item.segments.segments as ReplaySegment[];
  const lines = useMemo(() => sortLines(segments), [segments]);
  const byId = useMemo(() => new Map((speakers ?? []).map((s) => [s.id, s])), [speakers]);
  const choices = useMemo(() => (speakers ? speakerTalkTime(segments, files, speakers) : []), [segments, files, speakers]);

  const metaMs = (parseDuration(item.meta.duration) ?? 0) * 1000;
  const [filesMs, setFilesMs] = useState(0);
  const [speakerId, setSpeakerId] = useState<string | null>(null);
  const playlist = useMemo(
    () => buildPlaylist(segments, files, Math.max(metaMs, filesMs), speakerId),
    [segments, files, metaMs, filesMs, speakerId],
  );

  const elements = useRef(new Map<string, HTMLAudioElement>());
  const playerRef = useRef<ReplayPlayer | null>(null);
  const [playing, setPlaying] = useState(false);
  const [now, setNow] = useState(0);
  const [vNow, setVNow] = useState(0);
  const [rate, setRate] = useState(1);
  const [follow, setFollow] = useState(true);
  const [error, setError] = useState("");

  const sync = useCallback(() => {
    const p = playerRef.current;
    if (!p) return;
    setPlaying(p.playing);
    setNow(p.now());
    setVNow(p.virtualNow());
  }, []);

  // The transport, created once the elements exist; later playlists
  // (speaker filter, durations known) are swapped in keeping the position.
  useEffect(() => {
    const p = playerRef.current;
    if (!p) {
      playerRef.current = new ReplayPlayer(elements.current, playlist, (e) => {
        setError(e instanceof Error ? e.message : String(e));
        playerRef.current?.pause();
        sync();
      });
    } else {
      p.setPlaylist(playlist);
    }
    sync();
  }, [playlist, sync]);

  useEffect(
    () => () => {
      playerRef.current?.pause();
      playerRef.current = null;
    },
    [],
  );

  // The clock while playing: advance past span ends, move the highlight. A
  // timer, not requestAnimationFrame: frames stop in a minimized or hidden
  // window, and the audio would then run past a span into someone else's
  // lines. A timer keeps firing there (throttled, but it fires).
  useEffect(() => {
    if (!playing) return;
    const t = window.setInterval(() => {
      const p = playerRef.current;
      if (!p) return;
      const still = p.tick();
      setNow(p.now());
      setVNow(p.virtualNow());
      if (!still) setPlaying(false);
    }, TICK_MS);
    return () => window.clearInterval(t);
  }, [playing]);

  const act = (f: (p: ReplayPlayer) => void) => {
    const p = playerRef.current;
    if (!p) return;
    setError("");
    f(p);
    sync();
  };

  const seekTo = (ms: number) => act((p) => p.seek(ms));
  const goLine = (dir: 1 | -1) => {
    const l = adjacentLine(lines, playerRef.current?.now() ?? now, dir, speakerId);
    if (l) seekTo(l.start_ms);
    else if (dir === -1) act((p) => p.seekVirtual(0));
  };

  const onKey = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.metaKey || e.ctrlKey || e.altKey || ownsKey(e, e.key)) return;
    switch (e.key) {
      case " ":
        act((p) => p.toggle());
        break;
      case "ArrowLeft":
        act((p) => p.skip(-SKIP_MS));
        break;
      case "ArrowRight":
        act((p) => p.skip(SKIP_MS));
        break;
      case "ArrowUp":
        goLine(-1);
        break;
      case "ArrowDown":
        goLine(1);
        break;
      default:
        return;
    }
    e.preventDefault();
  };

  // Clicking a line of someone the filter hides plays everyone from there.
  const pendingSeek = useRef<{ ms: number; play: boolean } | null>(null);
  /** Move to a line; one of someone the filter hides drops the filter. */
  const goToLine = (s: ReplaySegment, play: boolean) => {
    if (speakerId !== null && s.speaker_id !== speakerId) {
      setSpeakerId(null);
      // The new playlist is swapped in by the effect; seek once it is.
      pendingSeek.current = { ms: s.start_ms, play };
      return;
    }
    seekTo(s.start_ms);
    if (play && !playerRef.current?.playing) act((p) => p.play());
  };
  const clickLine = (s: ReplaySegment) => goToLine(s, true);
  useEffect(() => {
    if (pendingSeek.current === null || playlist.speakerId !== null) return;
    const { ms, play } = pendingSeek.current;
    pendingSeek.current = null;
    act((p) => {
      p.seek(ms);
      if (play) p.play();
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [playlist]);

  // A line picked on the Voice map (#144): the player moves there (and
  // keeps playing if it was), also when the tab opens after the pick.
  const seekN = seek?.n;
  useEffect(() => {
    if (!seek) return;
    const s = lines.find((l) => l.id === seek.id);
    if (s) goToLine(s, false);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [seekN]);

  const active = activeSegment(lines, now, speakerId);
  const activeId = active?.id ?? null;

  // Follow: keep the lit line in view.
  const listRef = useRef<HTMLOListElement>(null);
  useEffect(() => {
    if (!follow || activeId === null) return;
    const el = listRef.current?.querySelector<HTMLElement>(`[data-seg="${activeId}"]`);
    const reduce = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;
    el?.scrollIntoView({ block: "center", behavior: reduce ? "auto" : "smooth" });
  }, [activeId, follow]);

  const empty = playlist.entries.length === 0;
  const chosen = speakerId ? choices.find((c) => c.id === speakerId) : undefined;

  return (
    <div className="au-tab" tabIndex={0} onKeyDown={onKey} role="region" aria-label="Audio player">
      {files.map((f) => (
        <audio
          key={f}
          preload="metadata"
          src={convertFileSrc(audioSrcPath(item.id, f), SCHEME)}
          ref={(el) => {
            if (el) elements.current.set(f, el);
            else elements.current.delete(f);
          }}
          onLoadedMetadata={(e) => {
            const d = e.currentTarget.duration;
            if (Number.isFinite(d)) setFilesMs((ms) => Math.max(ms, Math.round(d * 1000)));
          }}
          onError={() => setError(`${f} could not be loaded.`)}
        />
      ))}

      <div className="au-bar">
        <div className="au-transport">
          <button type="button" className="btn-ghost sh-btn au-skip" onClick={() => act((p) => p.skip(-SKIP_MS))} disabled={empty} aria-label="Back 10 seconds" title="Back 10 s (←)">
            −10
          </button>
          <button
            type="button"
            className="btn-dark au-play"
            onClick={() => act((p) => p.toggle())}
            disabled={empty}
            aria-label={playing ? "Pause" : "Play"}
            title={`${playing ? "Pause" : "Play"} (Space)`}
          >
            <span aria-hidden="true">{playing ? "❚❚" : "▶"}</span>
          </button>
          <button type="button" className="btn-ghost sh-btn au-skip" onClick={() => act((p) => p.skip(SKIP_MS))} disabled={empty} aria-label="Forward 10 seconds" title="Forward 10 s (→)">
            +10
          </button>
        </div>
        <div className="au-seek">
          <span className="au-time mono" aria-hidden="true">{formatPlayerTime(vNow)}</span>
          <input
            type="range"
            min={0}
            max={Math.max(1, playlist.duration_ms)}
            step={100}
            value={Math.min(vNow, playlist.duration_ms)}
            disabled={empty}
            aria-label={speakerId ? `Position in ${chosen?.label ?? "the speaker"}'s lines` : "Position"}
            aria-valuetext={`${formatPlayerTime(vNow)} of ${formatPlayerTime(playlist.duration_ms)}`}
            onChange={(e) => act((p) => p.seekVirtual(Number(e.target.value)))}
          />
          <span className="au-time mono" aria-hidden="true">{formatPlayerTime(playlist.duration_ms)}</span>
        </div>
        <div className="au-opts">
          <label className="au-opt">
            <span>Speed</span>
            <select
              value={rate}
              onChange={(e) => {
                const r = Number(e.target.value);
                setRate(r);
                playerRef.current?.setRate(r);
              }}
            >
              {REPLAY_RATES.map((r) => (
                <option key={r} value={r}>
                  {r}×
                </option>
              ))}
            </select>
          </label>
          {choices.length > 0 && (
            <label className="au-opt">
              <span>Play only</span>
              <select value={speakerId ?? ""} onChange={(e) => setSpeakerId(e.target.value || null)}>
                <option value="">All speakers</option>
                {choices.map((c) => (
                  <option key={c.id} value={c.id}>
                    {c.label} · {formatPlayerTime(c.ms)}
                  </option>
                ))}
              </select>
            </label>
          )}
          <label className="au-opt au-follow">
            <input type="checkbox" checked={follow} onChange={(e) => setFollow(e.target.checked)} />
            <span>Follow</span>
          </label>
        </div>
      </div>
      <p className="au-hint sh-muted">
        {speakerId && chosen
          ? `Playing only ${chosen.label}'s lines, back to back — everything else is skipped. `
          : files.length > 1
            ? "Both channels play together. "
            : ""}
        Space play/pause · ← → 10 s · ↑ ↓ previous/next line · click a line to play from there.
      </p>
      {error && (
        <p className="au-error" role="alert">
          The audio could not be played: {error}
        </p>
      )}

      <div className="doc-scroll tx-scroll au-scroll">
        <ol className="tx-list au-list" ref={listRef} aria-label={`Transcript of ${item.meta.title}, following the audio`}>
          {lines.map((s) => (
            <ReplayLine
              key={s.id}
              seg={s}
              speaker={s.speaker_id ? byId.get(s.speaker_id) : undefined}
              active={s.id === activeId}
              now={s.id === activeId ? now : -1}
              dimmed={speakerId !== null && s.speaker_id !== speakerId}
              playable={channelFile(s.channel, files) !== null}
              onClick={clickLine}
            />
          ))}
        </ol>
        {lines.length === 0 && <p className="tx-empty">This item has no lines — the player still plays the whole recording.</p>}
      </div>
    </div>
  );
}

const ReplayLine = memo(function ReplayLine({
  seg,
  speaker,
  active,
  now,
  dimmed,
  playable,
  onClick,
}: {
  seg: ReplaySegment;
  speaker?: TranscriptSpeaker;
  active: boolean;
  /** Real time, only for the active line (-1 otherwise, so the others
   *  don't re-render every frame). */
  now: number;
  dimmed: boolean;
  playable: boolean;
  onClick: (s: ReplaySegment) => void;
}) {
  const tokens = useMemo(() => wordSpans(seg.text, seg.words), [seg.text, seg.words]);
  const lit = active && tokens ? activeWord(tokens, now) : -1;
  const ts = lineTimestamp(seg.start_ms);
  const failed = !seg.text.trim();
  return (
    <li className={`tx-line au-line${active ? " playing" : ""}${dimmed ? " dimmed" : ""}`} data-seg={seg.id}>
      <button
        type="button"
        className="au-line-btn"
        onClick={() => onClick(seg)}
        disabled={!playable}
        aria-current={active ? "true" : undefined}
        aria-label={`Play from ${ts}${speaker ? `, ${speaker.label}` : ""}`}
        title={playable ? `Play from ${ts}` : "This channel's audio was not saved"}
      >
        <span className="tx-time">{ts}</span>
      </button>
      <div className="tx-body" onClick={() => playable && onClick(seg)}>
        {speaker && (
          <span className="tx-chip" style={{ background: speaker.color || undefined }} title={speaker.label}>
            <i aria-hidden="true" />
            {speaker.label}
          </span>
        )}
        {failed ? (
          <p className="tx-text tx-failed">[not transcribed]</p>
        ) : tokens ? (
          <p className="tx-text">
            {tokens.map((t, i) => (
              <span key={i} className={i === lit ? "au-word lit" : "au-word"}>
                {t.text}
                {i < tokens.length - 1 ? " " : ""}
              </span>
            ))}
          </p>
        ) : (
          <p className="tx-text">{seg.text}</p>
        )}
      </div>
    </li>
  );
});

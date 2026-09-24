import { useEffect, useId, useMemo, useState, type KeyboardEvent, type MouseEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  VIEW_H,
  VIEW_W,
  layoutVoiceMap,
  mapCaption,
  nearestPoint,
  pointLabel,
  pointRadius,
  stepPoint,
} from "../lib/voiceMap";
import type { Item, VoiceMap } from "../lib/types";

/** Hover reach around a dot, in screen pixels. */
const HOVER_PX = 10;

/** The Speakers panel's *Voice map* card (#144): every line with voice
 *  data as a dot, placed by the backend's 2-D projection of its voice
 *  print and coloured by speaker. Hover (or the keyboard) shows the line;
 *  a click or Enter selects it in the transcript — and moves the Audio
 *  tab's player there when audio was saved. */
export function VoiceMapCard({
  item,
  selectedId = null,
  onPick,
}: {
  item: Item;
  /** The line selected from the map (ringed). */
  selectedId?: number | null;
  onPick?: (segmentId: number) => void;
}) {
  const [map, setMap] = useState<VoiceMap | null>(null);
  const [error, setError] = useState("");
  /** Point under the pointer / keyboard cursor (index in time order). */
  const [hover, setHover] = useState(-1);
  const [active, setActive] = useState(-1);
  const id = item.id;
  // DOM ids for the points (item ids hold slashes).
  const uid = useId();
  const embedded = item.embedded_segments ?? 0;

  // The projection depends on the embeddings only: refetch when the item
  // or its voice data changes, not on a rename or a moved line.
  useEffect(() => {
    let alive = true;
    setMap(null);
    setError("");
    setHover(-1);
    setActive(-1);
    invoke<VoiceMap>("archive_voice_map", { id })
      .then((m) => alive && setMap(m))
      .catch((e) => alive && setError(String(e)));
    return () => {
      alive = false;
    };
  }, [id, embedded]);

  const layout = useMemo(() => (map ? layoutVoiceMap(map, item) : null), [map, item]);
  const points = layout?.points ?? [];
  const r = pointRadius(points.length);
  const selectedIdx = selectedId === null ? -1 : points.findIndex((p) => p.segmentId === selectedId);

  // Thousands of dots: drawn once per layout, not on every hover.
  const dots = useMemo(
    () =>
      points.map((p) => (
        <circle
          key={p.segmentId}
          id={`${uid}-${p.segmentId}`}
          role="option"
          aria-selected={p.segmentId === selectedId}
          aria-label={pointLabel(p)}
          className="vmap-dot"
          cx={p.x}
          cy={p.y}
          r={r}
          style={{ fill: p.color }}
        />
      )),
    [points, r, uid, selectedId],
  );

  if (error) return <p className="ctx-note warn">The voice map could not be drawn: {error}</p>;
  if (!map || !layout) return <p className="ctx-note">Drawing the voice map…</p>;
  if (points.length < 2) return null;

  const toView = (e: MouseEvent<SVGSVGElement>) => {
    const box = e.currentTarget.getBoundingClientRect();
    if (!box.width || !box.height) return null;
    const k = VIEW_W / box.width;
    return { x: (e.clientX - box.left) * k, y: (e.clientY - box.top) * (VIEW_H / box.height), reach: HOVER_PX * k };
  };
  const onMove = (e: MouseEvent<SVGSVGElement>) => {
    const v = toView(e);
    setHover(v ? nearestPoint(points, v.x, v.y, Math.max(v.reach, r)) : -1);
  };
  const pick = (i: number) => {
    const p = points[i];
    if (!p) return;
    setActive(i);
    onPick?.(p.segmentId);
  };
  const onClick = (e: MouseEvent<SVGSVGElement>) => {
    const v = toView(e);
    if (!v) return;
    const i = nearestPoint(points, v.x, v.y, Math.max(v.reach, r));
    if (i >= 0) pick(i);
  };
  const onKey = (e: KeyboardEvent<SVGSVGElement>) => {
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    if ((e.key === "Enter" || e.key === " ") && active >= 0) {
      e.preventDefault();
      pick(active);
      return;
    }
    if (e.key === "Escape" && active >= 0) {
      e.preventDefault();
      e.stopPropagation();
      setActive(-1);
      return;
    }
    const next = stepPoint(points.length, active, e.key);
    if (next === null) return;
    e.preventDefault();
    setActive(next);
    setHover(-1);
  };

  // The tooltip follows the pointer; without one, the keyboard cursor.
  const shown = points[hover] ? hover : points[active] ? active : -1;
  const tip = shown >= 0 ? points[shown] : null;
  const ring = (i: number, cls: string) => {
    const p = points[i];
    return p ? <circle className={cls} cx={p.x} cy={p.y} r={r + 2.5} aria-hidden="true" /> : null;
  };

  return (
    <div className="vmap" role="group" aria-labelledby={`${uid}-h`}>
      <h4 id={`${uid}-h`} className="vmap-h">
        Voice map
      </h4>
      <div className="vmap-plot">
        <svg
          className="vmap-svg"
          viewBox={`0 0 ${VIEW_W} ${VIEW_H}`}
          role="listbox"
          tabIndex={0}
          aria-label="Lines placed by how the voice sounds. Arrow keys step through the lines in time order; Enter selects one."
          aria-activedescendant={points[active] ? `${uid}-${points[active].segmentId}` : undefined}
          onMouseMove={onMove}
          onMouseLeave={() => setHover(-1)}
          onClick={onClick}
          onKeyDown={onKey}
          onFocus={(e) => {
            // A keyboard visit starts on the selected line (or the first);
            // a click focuses too, but there the clicked dot wins.
            if (active < 0 && focusVisible(e.currentTarget)) setActive(selectedIdx >= 0 ? selectedIdx : 0);
          }}
          onBlur={() => setActive(-1)}
        >
          <rect className="vmap-bg" x={0} y={0} width={VIEW_W} height={VIEW_H} rx={6} aria-hidden="true" />
          <g>{dots}</g>
          {selectedIdx >= 0 && ring(selectedIdx, "vmap-sel")}
          {shown >= 0 && shown !== selectedIdx && ring(shown, "vmap-cur")}
        </svg>
        {tip && (
          <div
            className={`vmap-tip${tip.y < VIEW_H * 0.4 ? " below" : ""}`}
            // Slides along its width with the dot (left edge at the left
            // border, right edge at the right one), so it never leaves the
            // plot; above the dot, or below it near the top.
            style={{
              left: `${(tip.x / VIEW_W) * 100}%`,
              top: `${(tip.y / VIEW_H) * 100}%`,
              transform: `translate(${-(tip.x / VIEW_W) * 100}%, ${tip.y < VIEW_H * 0.4 ? "0" : "-100%"})`,
            }}
            aria-hidden="true"
          >
            <span className="vmap-tip-who">
              <i style={{ background: tip.color }} />
              {tip.label} · <span className="mono">{tip.time}</span>
            </span>
            <span className="vmap-tip-text">{tip.text}</span>
          </div>
        )}
      </div>
      <ul className="vmap-legend" aria-label="Speakers on the map">
        {layout.legend.map((l) => (
          <li key={l.id ?? "-"}>
            <i aria-hidden="true" style={{ background: l.color }} />
            {l.label} <span className="vmap-n">{l.count}</span>
          </li>
        ))}
      </ul>
      <p className="ctx-note vmap-cap">{mapCaption(map)}</p>
    </div>
  );
}

/** Focus that came from the keyboard (not a click). */
function focusVisible(el: Element): boolean {
  try {
    return el.matches(":focus-visible");
  } catch {
    return true;
  }
}

/* Reading the Meet page through a selector set (#131): which tiles exist,
 * whose they are, which one is the user's, which are lit as speaking.
 * Read-only (never clicks, never opens panels), no layout reads, no text
 * matching on labels. Pure over a DOM root — unit tested on synthetic
 * fixtures. */
import { cleanName, tileKey } from "./names";
import type { AttrStrategy, FindStrategy, SelectorSet } from "./selectors/types";

export interface Tile {
  /** `tile:<hash>` of the participant id. */
  key: string;
  /** Display name, through the name guard (null: none or rejected). */
  name: string | null;
  self: boolean;
  speaking: boolean;
}

export interface DomProbe {
  set: string;
  tiles: Tile[];
  /** Strategy that matched, per hook (null: none did). */
  matched: Record<"tile" | "tileId" | "tileName" | "selfMarker" | "speaking", string | null>;
  /** Tiles with a name that failed the guard (a sign the name hook broke). */
  rejectedNames: number;
  /** The user's own name, when the self tile shows one. */
  selfName: string | null;
}

function queryAll(root: ParentNode, css: string): Element[] {
  try {
    return Array.from(root.querySelectorAll(css));
  } catch {
    return []; // a selector this engine can't parse
  }
}

function within(tile: Element, css: string): Element[] {
  const out: Element[] = [];
  for (const part of css.split(",").map((s) => s.trim())) {
    if (part === ":scope") out.push(tile);
    else out.push(...queryAll(tile, part));
  }
  return out;
}

function attrPasses(el: Element, s: AttrStrategy): boolean {
  const v = el.getAttribute(s.attr);
  if (v === null) return false;
  switch (s.test) {
    case "present":
      return true;
    case "true":
      return v.trim().toLowerCase() === "true";
    case "positive": {
      const n = Number.parseFloat(v);
      return Number.isFinite(n) && n > 0;
    }
  }
}

/** First strategy of `list` for which `test` holds on the tile. */
function firstAttr(tile: Element, list: AttrStrategy[]): string | null {
  for (const s of list) if (within(tile, s.css).some((el) => attrPasses(el, s))) return s.name;
  return null;
}

function firstFind(root: ParentNode, list: FindStrategy[]): { name: string; els: Element[] } | null {
  for (const s of list) {
    const els = queryAll(root, s.css);
    if (els.length) return { name: s.name, els };
  }
  return null;
}

/** Read the page with one selector set. */
export function probe(root: ParentNode, set: SelectorSet): DomProbe {
  const matched: DomProbe["matched"] = { tile: null, tileId: null, tileName: null, selfMarker: null, speaking: null };
  const tiles: Tile[] = [];
  let rejectedNames = 0;
  let selfName: string | null = null;
  const found = firstFind(root, set.hooks.tile);
  if (!found) return { set: set.id, tiles, matched, rejectedNames, selfName };
  matched.tile = found.name;
  // Nested matches (a tile inside a tile) count once, as the outer one.
  const outer = found.els.filter((el) => !found.els.some((o) => o !== el && o.contains(el)));
  const seen = new Set<string>();
  for (const el of outer) {
    let raw: string | null = null;
    for (const s of set.hooks.tileId) {
      const v = el.getAttribute(s.attr);
      if (v) {
        raw = v;
        matched.tileId ??= s.name;
        break;
      }
    }
    if (!raw) continue;
    const key = tileKey(raw);
    if (seen.has(key)) continue; // the same participant in two layouts
    seen.add(key);
    const selfBy = firstAttr(el, set.hooks.selfMarker);
    if (selfBy) matched.selfMarker ??= selfBy;
    const speakingBy = firstAttr(el, set.hooks.speaking);
    if (speakingBy) matched.speaking ??= speakingBy;
    let name: string | null = null;
    for (const s of set.hooks.tileName) {
      const leaf = within(el, s.css).find((n) => (n.textContent ?? "").trim());
      if (!leaf) continue;
      matched.tileName ??= s.name;
      name = cleanName(leaf.textContent);
      if (!name) rejectedNames++;
      break;
    }
    if (selfBy && name) selfName = name;
    tiles.push({ key, name, self: !!selfBy, speaking: !!speakingBy });
  }
  return { set: set.id, tiles, matched, rejectedNames, selfName };
}

/** Whether a set fits the page (its fingerprint passes). */
export function fits(p: DomProbe, set: SelectorSet): boolean {
  return p.tiles.length >= set.fingerprint.minTiles;
}

/** The first set that fits the page, with its probe; else the first set's
 *  probe (the health check then reports what is missing). */
export function pickSet(root: ParentNode, sets: readonly SelectorSet[]): { set: SelectorSet; probe: DomProbe } {
  let first: { set: SelectorSet; probe: DomProbe } | null = null;
  for (const set of sets) {
    const p = probe(root, set);
    if (fits(p, set)) return { set, probe: p };
    first ??= { set, probe: p };
  }
  if (!first) throw new Error("no Meet selector set");
  return first;
}

/** Every attribute name a set reads (for the MutationObserver's filter). */
export function watchedAttributes(sets: readonly SelectorSet[]): string[] {
  const out = new Set<string>();
  for (const s of sets) {
    for (const x of s.hooks.tileId) out.add(x.attr);
    for (const x of [...s.hooks.selfMarker, ...s.hooks.speaking]) out.add(x.attr);
  }
  return [...out];
}

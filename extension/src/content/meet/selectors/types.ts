/* The shape of a Meet selector set (#131): data only, no code, one file per
 * set (`meet-<yyyy>-<mm><letter>.ts`), listed newest first in `index.ts`.
 *
 * Meet's markup changes every few months and not for everyone at once
 * (phased rollouts, #105), so several sets can be live together: at join
 * and every 30 s the observer picks the first set whose fingerprint passes
 * on the current page, and records which strategy matched per hook. Every
 * hook is an ordered list of named strategies — data or ARIA attributes
 * and structure first, never obfuscated class names (they change weekly
 * and a wrong guess is worse than no name). Matching never depends on
 * visible text or labels (they are localized).
 *
 * A fix is a new set file plus a synthetic fixture under `../fixtures/`
 * rebuilt from the live page's structure (no real names), with a test. */

/** Elements found by a CSS selector (relative to the document, or to a
 *  tile for the per-tile hooks). */
export interface FindStrategy {
  name: string;
  css: string;
}

/** A tile-relative element carrying an attribute; `test` says what counts:
 *  the attribute is `present`, is `"true"`, or holds a number above 0. The
 *  element may be the tile itself (`css: ":scope"`). */
export interface AttrStrategy {
  name: string;
  css: string;
  attr: string;
  test: "present" | "true" | "positive";
}

/** Where a tile's participant id is: an attribute of the tile element. */
export interface IdStrategy {
  name: string;
  attr: string;
}

export interface SelectorSet {
  id: string;
  /** Date the values were checked on a live Meet page (null: not yet). */
  verifiedOn: string | null;
  /** UI languages the check covered. */
  locales: string[];
  hooks: {
    /** One element per participant tile. */
    tile: FindStrategy[];
    /** The tile's participant id (stable for the call). */
    tileId: IdStrategy[];
    /** The display-name element inside a tile (its text is the name). */
    tileName: FindStrategy[];
    /** The user's own tile (never a remote speaker, never bindable). */
    selfMarker: AttrStrategy[];
    /** The tile is lit as speaking. */
    speaking: AttrStrategy[];
  };
  /** The set applies when at least this many tiles with an id are found. */
  fingerprint: { minTiles: number };
}

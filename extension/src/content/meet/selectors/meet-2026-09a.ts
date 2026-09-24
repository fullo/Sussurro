/* Provisional Meet selector set (#131).
 *
 * NOT VERIFIED on a live Meet page yet (`verifiedOn: null`): the hooks are
 * the structural ones the desk study (#105) expects — a participant tile
 * carrying its participant id as a data attribute, a self marker attribute
 * on the user's own tile, the name as the text of a leaf inside the tile,
 * and a numeric audio-level attribute as the speaking candidate. The
 * live check (#184) confirms or replaces every value from our own
 * inspection of the page, records the date and locales, and adds a
 * synthetic fixture for it. Until then the health check decides at run
 * time: a hook that does not match reports "names unavailable" and the
 * app keeps "Voice N". No class names on purpose. */
import type { SelectorSet } from "./types";

export const MEET_2026_09A: SelectorSet = {
  id: "meet-2026-09a",
  verifiedOn: null,
  locales: [],
  hooks: {
    tile: [{ name: "participant-id attribute", css: "[data-participant-id]" }],
    tileId: [{ name: "participant-id attribute", attr: "data-participant-id" }],
    tileName: [
      { name: "self-name leaf", css: "[data-self-name]" },
      { name: "untranslated leaf", css: "span.notranslate" },
    ],
    selfMarker: [{ name: "self-name attribute", css: ":scope, [data-self-name]", attr: "data-self-name", test: "present" }],
    speaking: [{ name: "audio-level attribute", css: ":scope, [data-audio-level]", attr: "data-audio-level", test: "positive" }],
  },
  fingerprint: { minTiles: 1 },
};

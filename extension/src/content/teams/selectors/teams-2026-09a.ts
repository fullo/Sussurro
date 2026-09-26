/* Provisional Teams web selector set (#245).
 *
 * NOT VERIFIED on a live Teams page yet (`verifiedOn: null`). The hooks
 * are the kinds the desk study (#239) documents — a participant stream
 * wrapper found by a `data-tid` test id (test ids outlived Teams' Fluent
 * class-hash churn), the participant's id as a data attribute of that
 * wrapper, the name as the text of a leaf inside it, a local-stream
 * attribute on the user's own wrapper, and the voice-level outline under
 * the wrapper as the speaking candidates, in order. The attribute *values*
 * are placeholders of that shape: the study copied no values from other
 * projects on purpose, and ours come only from our own inspection of a
 * live page. The live check (#184) confirms or replaces every value, for
 * each UI variant Teams serves, records the date and locales, and adds a
 * synthetic fixture per variant. Until then the health check decides at
 * run time: a hook that does not match — or tiles without the outline
 * (the variant without it, #239) — reports "names unavailable" and the app
 * keeps "Voice N". No class names, no visible text or labels. */
import type { SelectorSet } from "../../names/selectors";

export const TEAMS_2026_09A: SelectorSet = {
  id: "teams-2026-09a",
  verifiedOn: null,
  locales: [],
  hooks: {
    tile: [{ name: "stream wrapper test id", css: '[data-tid="participant-stream"]' }],
    tileId: [
      { name: "participant-id attribute", attr: "data-participant-id" },
      { name: "stream-id attribute", attr: "data-stream-id" },
    ],
    tileName: [{ name: "display-name test id", css: '[data-tid="participant-name"]' }],
    selfMarker: [{ name: "local-stream attribute", css: ":scope", attr: "data-is-local", test: "true" }],
    speaking: [
      { name: "voice-level outline, speaking flag", css: '[data-tid="voice-level-outline"]', attr: "data-speaking", test: "true" },
      { name: "voice-level outline, level", css: '[data-tid="voice-level-outline"]', attr: "data-voice-level", test: "positive" },
    ],
  },
  fingerprint: { minTiles: 1 },
};

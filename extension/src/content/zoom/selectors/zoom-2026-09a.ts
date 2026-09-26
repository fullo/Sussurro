/* Provisional Zoom web client selector set (#246).
 *
 * NOT VERIFIED on a live Zoom page yet (`verifiedOn: null`). The hooks are
 * the kinds the desk study (#239) documents — participant tiles in the
 * meeting iframe (gallery tiles, or the one large active-speaker tile of
 * speaker view) carrying the participant's id, the name in the tile's
 * avatar footer, a marker on the user's own tile, an active-speaker marker
 * as the speaking signal, and the screen-share view as a pause (the
 * spotlight then holds the presenter, not the speaker). The attribute
 * *values* are placeholders of that shape: the study copied no values from
 * other projects on purpose, and ours come only from our own inspection
 * of a live page. Zoom's markup is expected to have the shortest life of
 * the three platforms (#239: its class families drift within one meeting),
 * so this set uses attributes only — the class-substring families other
 * projects fall back to are left out on purpose. The live check (#184)
 * confirms or replaces every value (speaker and gallery view), records
 * the date and locales, and adds a synthetic fixture. Until then the
 * health check decides at run time: a hook that does not match reports
 * "names unavailable" and the app keeps "Voice N". No class names, no
 * visible text or labels. */
import type { SelectorSet } from "../../names/selectors";

export const ZOOM_2026_09A: SelectorSet = {
  id: "zoom-2026-09a",
  verifiedOn: null,
  locales: [],
  hooks: {
    tile: [{ name: "user-id attribute", css: "[data-user-id]" }],
    tileId: [{ name: "user-id attribute", attr: "data-user-id" }],
    tileName: [{ name: "footer name test id", css: '[data-testid="footer-name"]' }],
    selfMarker: [{ name: "self attribute", css: ":scope", attr: "data-self", test: "true" }],
    speaking: [{ name: "active-speaker attribute", css: ":scope", attr: "data-active-speaker", test: "true" }],
    pause: [{ name: "screen-share view", css: '[data-share-view="true"]' }],
  },
  fingerprint: { minTiles: 1 },
};

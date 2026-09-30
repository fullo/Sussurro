# Store submissions (#268)

What the maintainer needs to list the extension on the Chrome Web Store
(#270), Microsoft Edge Add-ons (#271) and addons.mozilla.org, listed channel
(#269) — in that submission order (AMO → Chrome → Edge, per the roadmap in
the project's `CLAUDE.md`). Everything here is **texts and instructions**;
none of it submits anything by itself (all three child issues are `needs
maintainer`: they need accounts this repo's agents don't have).

## What's in this folder

| File | Covers |
|---|---|
| [`chrome-web-store.md`](chrome-web-store.md) | Chrome Web Store listing: single-purpose description, detailed description (English + Italian), permission justifications, data-usage disclosure, screenshots/promo assets it asks for |
| [`edge-add-ons.md`](edge-add-ons.md) | Microsoft Edge Add-ons: what differs from the Chrome Web Store submission (same package) |
| [`amo-listing.md`](amo-listing.md) | addons.mozilla.org listed-channel listing: summary, description (English + Italian), categories/tags, what AMO shows reviewers vs. users |
| [`screenshots.md`](screenshots.md) | What to capture for each store, at what size — no real screenshots are included here; a human has to run the extension against a real meeting |

Permission wording throughout is kept **consistent with the published
privacy policy**, `docs/privacy.html#extension-permissions` — that table is
the source of truth for what each permission is for; these files restate it
in each store's required format and length, they don't add new claims.

## Builds to submit

Built from `extension/` (`npm ci && npm run build`, `npm run
build:firefox:listed`; see `extension/README.md` → *Build*):

| Store | File | Built by |
|---|---|---|
| Chrome Web Store | `dist/sussurro-extension-chrome-<version>.zip` | `npm run build:chrome` |
| Microsoft Edge Add-ons | the same Chrome zip (Edge is Chromium; no separate manifest) | `npm run build:chrome` |
| AMO, listed channel | `dist/sussurro-extension-firefox-listed-<version>.zip` | `npm run build:firefox:listed` — **not** the self-hosted `sussurro-extension-firefox-<version>.zip`, which keeps its own `update_url` (#228) that AMO refuses on a listed add-on |
| AMO source review | `dist/sussurro-extension-source-<version>.zip` | built by the release workflow's "Source archive for AMO review" step, or by hand: `git archive --format=zip -o sussurro-extension-source-<version>.zip HEAD LICENSE extension sussurro/package.json sussurro/src sussurro/src-tauri/icons/32x32.png sussurro/src-tauri/icons/64x64.png sussurro/src-tauri/icons/128x128.png` from the repo root |

The release workflow (`.github/workflows/release.yml`) builds and lints all
of the above on every tag and attaches them to the GitHub release; it does
not submit anything to a store. `npm run lint:listed` runs `web-ext lint` on
the listed build in AMO's normal (non-self-hosted) mode — the check that
build must pass before an AMO listed submission.

## Before submitting (maintainer)

1. **AMO (#269)**: the account and signing keys already exist (the unlisted
   `.xpi` is signed on every release, #228). Submitting the `firefox-listed`
   zip as a **new, listed version** switches the add-on from self-distribution
   to a Mozilla-reviewed listing; `sussurro-extension-source-<version>.zip`
   and `AMO-REVIEWER-NOTES.md` go with it, same as the unlisted signing. Once
   it's listed, `browser_specific_settings.gecko.update_url` must stay out of
   every future submission (AMO refuses it on a listed add-on) — the
   `firefox-listed` build already does this; existing self-hosted installs
   keep using `docs/extension/updates.json` until they update to a listed
   build through the browser's own add-on updater, or the maintainer plans
   their migration (see `extension/README.md` → *Releases, signing and
   updates*).
2. **Chrome Web Store (#270)**: register a developer account (one-time fee),
   fill in the *Privacy practices* tab using `chrome-web-store.md`, upload the
   chrome zip and screenshots, submit for review.
3. **Edge Add-ons (#271)**: register a Partner Center account (free), submit
   the same chrome zip; `edge-add-ons.md` has what differs from the Chrome
   Web Store form.
4. Update `README.md` (the extension's *Install* section currently says "not
   in the browser stores yet") and the project site once each listing is
   live, and link to it from there.

## Verifying claims here

Every factual claim in these texts (what data goes where, which permissions
exist, what "collected"/"transferred" means for each store's own privacy
form) is checked against `extension/manifest.chrome.json`,
`extension/manifest.firefox.json` and `docs/privacy.html` as of this
writing. Store review policies change; re-check the wording of the actual
submission forms (categories, character limits, required fields) against
each store's current developer documentation before submitting — this
folder gives the maintainer accurate content to place in those forms, not a
guarantee the forms themselves haven't changed shape.

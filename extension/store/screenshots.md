# Screenshots (#268)

No screenshots are included in this repository: they need a human running
the built extension against a real Google Meet, Microsoft Teams or Zoom
call, paired to a running Sussurro app, on their own machine (nothing here
can fabricate that). This is a shot list for whoever captures them —
maintainer or a later agent session with a real meeting to join.

## What to capture

1. **Side panel with a live meeting recording** — the main shot. A short
   real (or rehearsal) call in Google Meet, Chrome or Edge, with:
   - the **REC** badge visible on the tab,
   - a few transcript lines with named speakers (not just "Voice 1"), to
     show the names-from-the-page feature working,
   - the Start/Stop controls and the language selector visible.
2. **Options page, paired** — the pairing card showing a successful **Test
   connection**, token masked (never show a real token or pairing code in
   a published screenshot — regenerate the token afterwards if one was
   shown on screen during capture, or blur it before publishing).
3. **First-recording notice** — the recording-consent notice shown before
   the first Start (`extension/README.md` → *Recording notice*), to make
   the consent-forward design visible to reviewers and users browsing the
   store listing.
4. **Optional**: the Firefox sidebar (same content as #1, in Firefox), and
   a screenshot with the app's Library showing the resulting *Meeting*
   item, to show the browser → app hand-off end to end.

Use a **test or throwaway meeting** (e.g. a solo call, or one with
consenting participants who know it's for a screenshot) — never a real
meeting with people who haven't agreed to being shown in a public store
listing image. Blur or crop out any participant's name, email or face if
they haven't separately agreed to appear in the published screenshot.

## Sizes and counts per store

Re-check each store's current upload requirements before capturing —
these have changed before and the values below are what each store's
developer documentation asked for as of this writing:

| Store | Screenshots | Promo images |
|---|---|---|
| Chrome Web Store | 1–5 images, 1280×800 or 640×400 (16:10), PNG or JPEG, no alpha channel | optional 440×280 small promo tile; 920×680 and 1400×560 marquee tiles if a featured placement is wanted |
| Microsoft Edge Add-ons | 1–10 images, at least 1280×800 recommended | a store logo/icon separate from the extension icon may be requested |
| addons.mozilla.org (AMO) | 1 or more, no strict pixel requirement, but wide/landscape screenshots read best in AMO's listing carousel | none required |

## Captions (optional, reuse across stores)

Short one-line captions to pair with each image, consistent with the
listing texts in this folder:

1. "Live transcript in the side panel, with names from the meeting page."
2. "Pair once with a code from the Sussurro app — nothing else to
   configure."
3. "A short notice before your first recording, since it may record other
   people too."
4. "The same call, transcribed and saved in Sussurro's Library."

# Microsoft Edge Add-ons listing (#271)

Edge is Chromium-based and reads the same `manifest.chrome.json`, so
**submit the same `sussurro-extension-chrome-<version>.zip`** built for the
Chrome Web Store (`npm run build:chrome` in `extension/`) — no separate
build or manifest. Reuse the texts in
[`chrome-web-store.md`](chrome-web-store.md) (single purpose description,
detailed description in English and Italian, permission justifications,
data usage, remote code) as-is: the permissions, host access and behaviour
are identical, since it is the identical package.

## What differs from the Chrome Web Store submission

- **Account**: a Microsoft Partner Center account (free registration),
  separate from a Google/Chrome Web Store developer account.
- **Category**: Edge Add-ons uses its own category list; pick
  **Productivity** (or the closest equivalent Edge offers at submission
  time — its taxonomy has changed between store redesigns).
- **Privacy dashboard fields are worded differently** than Chrome's
  Privacy practices tab, but ask the same substance: what the extension's
  single purpose is, which permissions it needs and why, and whether it
  transmits data off the device. Answer them the same way as
  `chrome-web-store.md`'s permission-justification table and data-usage
  section — same manifest, same behaviour, same answer.
- **Privacy policy URL**: same as Chrome's,
  `https://fullo.github.io/Sussurro/privacy.html`.
- **Support contact / homepage**: `https://github.com/fullo/Sussurro` (the
  same the manifest's `homepage_url` already points to).
- Edge's own review process is typically faster than Chrome's, but can
  still ask clarifying questions about host permissions — the
  justification table in `chrome-web-store.md` is written to answer those
  directly (what each meeting-site host permission is for, why
  `127.0.0.1` is requested, why there is no `<all_urls>`).

## Screenshots

See [`screenshots.md`](screenshots.md). Edge Add-ons accepts the same
image content as the Chrome Web Store listing; check its current upload
form for exact required sizes at submission time, since Microsoft has
changed them before.

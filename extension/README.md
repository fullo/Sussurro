# Sussurro browser extension (preview)

The browser half of Sussurro's meeting capture (0.9): it captures a web
meeting (Google Meet, Microsoft Teams, Zoom web client) and streams it to the
Sussurro app running on the same computer, which transcribes and saves it.
The extension is a capture device plus a live mirror; editing happens in the
app (plan decision E3).

**Status: preview.** Pairing with the app works (#127); capture (#128) and
the live side panel (#129) come next. Build it only to work on it.

## Pairing

1. In Sussurro: Settings → Browser extension → turn on **Meetings**, and
   make sure the local API is on (it starts with the app).
2. Click **Copy pairing code**: one string, `sussurro:<port>:<token>`.
3. In the extension's options page (right-click the toolbar button →
   Options), paste it and **Save and test**. Port and token can also be
   entered separately.

**Test connection** calls `GET /app/version` with the token and reports,
distinctly: app not reachable (not running, local API off, wrong port),
wrong token, meetings off, another protocol, or a reply blocked by the
browser. **Regenerate token** in the app invalidates the old token at once;
every paired browser must be paired again.

The pairing lives in `storage.local` under the keys `port` and `token`,
read and written only through `src/shared/pairing.ts` (`getPairing`,
`setPairing`, `onPairingChanged`, `liveUrl`…). The pairing-code format is
defined once in the app (`sussurro/src/lib/pairingCode.ts`) and imported
here as `@sussurro/pairing`. The token is never logged and the options
page shows it only masked once saved.

## Layout

| Path | What it is |
|---|---|
| `manifest.chrome.json` | Chrome/Edge/Brave, MV3: service-worker background, `side_panel` |
| `manifest.firefox.json` | Firefox ≥ 128, MV3: `background.scripts`, `sidebar_action` |
| `src/background/` | background worker (WebSocket to the app, from #128) |
| `src/content/main-world.ts` | MAIN-world content script (the `RTCPeerConnection` hook, E4) |
| `src/content/isolated-world.ts` | ISOLATED-world content script (relay to the background) |
| `src/sidepanel/`, `sidepanel.html` | side panel (Chrome) / sidebar (Firefox) |
| `src/options/`, `options.html` | options page: pairing with the app, Test connection (#127) |
| `src/shared/` | helpers shared by the entry points (`pairing.ts`: storage keys and URLs; `connection.ts`: the connection test) |
| `scripts/build.ts` | the build: pages + scripts + manifest + icons + zip |

Permissions stay minimal: `storage` (plus `sidePanel` on Chrome), and host
access only to the meeting pages and to `http://127.0.0.1/*` (the local app).
No `<all_urls>`. The unit tests check this.

Transcript components are shared with the app. They live in
`sussurro/src/transcript/` and are imported as `@sussurro/transcript` (a
Vite alias plus a tsconfig path); the pairing-code format likewise as
`@sussurro/pairing`. React always resolves to this package's
copy, so the extension builds without `sussurro/node_modules`.

The extension version always matches the app. The build reads it from
`sussurro/package.json`, so this `package.json`'s own version is unused.
Icons are copied from `sussurro/src-tauri/icons/`.

## Build

Needs Node.js ≥ 24. The build scripts run as TypeScript directly, through
Node's type stripping.

```bash
cd extension
npm ci
npm run build:chrome     # → dist/chrome/  + dist/sussurro-extension-chrome-<version>.zip
npm run build:firefox    # → dist/firefox/ + dist/sussurro-extension-firefox-<version>.zip
npm run build            # both

npm run typecheck        # tsc
npm test                 # vitest (pure helpers, manifest checks)
npm run lint             # web-ext lint on dist/firefox (run after build:firefox)
```

CI (`.github/workflows/test.yml`, `extension` job) runs all of the above. The
release workflow attaches both zips to the GitHub release.

`web-ext lint` passes with a few expected warnings. `innerHTML` comes from
React DOM. `data_collection_permissions` postdates Firefox 128: newer
Firefox reads it (AMO requires it) and older versions ignore it.

## Load it unpacked

### Chrome, Edge, Brave

1. `npm run build:chrome`
2. Open `chrome://extensions` (`edge://extensions`, `brave://extensions`).
3. Turn on **Developer mode**.
4. **Load unpacked** → pick `extension/dist/chrome`.
5. Click the Sussurro toolbar button to open the side panel.

After a rebuild, press the reload icon on the extension's card.

### Firefox

1. `npm run build:firefox`
2. Open `about:debugging#/runtime/this-firefox`.
3. **Load Temporary Add-on…** → pick `extension/dist/firefox/manifest.json`.
4. Click the Sussurro toolbar button to toggle the sidebar.

A temporary add-on is removed when Firefox quits. In Firefox, MV3 host
permissions can be granted per site: if a meeting page is not picked up,
allow it under the extension's **Permissions** tab in `about:addons`.

Alternatively, `npx web-ext run --source-dir dist/firefox` starts a fresh
Firefox profile with the add-on installed.

## Licenses

`npm run licenses` in `sussurro/` covers the app only. The extension's own
third-party list (React, `webextension-polyfill`, …) will be added with the
0.9 release (#138).

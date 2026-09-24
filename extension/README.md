# Sussurro browser extension (preview)

The browser half of Sussurro's meeting capture (0.9): it captures a web
meeting (Google Meet, Microsoft Teams, Zoom web client) and streams it to the
Sussurro app running on the same computer, which transcribes and saves it.
The extension is a capture device plus a live mirror; editing happens in the
app (plan decision E3).

**Status: preview.** Pairing with the app (#127), capture (#128) and the
live transcript in the side panel (#129) work; Meet names (#131) are
implemented and wait for a live check. Capture is
verified automatically against a local two-peer call (see *Capture
harness*); real Meet / Teams / Zoom calls are still checked by hand (#184).
Build it only to work on it.

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

## Capture (#128)

Nothing is captured until the user presses **Start recording** in the side
panel (Chrome) / sidebar (Firefox) of the meeting tab; while capturing, the
toolbar button shows a red **REC** badge on that tab. **Stop**, closing the
tab or navigating away ends the meeting on the app (`stop`), which keeps
what it received.

```
MAIN world (hook, AudioWorklet) ─MessagePort─▶ ISOLATED world ─runtime port─▶ background ─WebSocket─▶ app /live
                                               Chrome offscreen (tab capture) ─runtime port─▶ background
```

- **MAIN-world hook** (`src/content/main-world.ts`, `document_start`): wraps
  the `RTCPeerConnection` constructor (a `Proxy`: `track` events = remote
  audio), `addTrack` / `addTransceiver` / `addStream` / `removeTrack` /
  `close`, `RTCRtpSender.replaceTrack`, rescans senders/receivers after
  `set{Local,Remote}Description`, and observes the page's `getUserMedia`.
  Several peer connections and tracks added, replaced or ended mid-call
  are followed (`src/content/registry.ts` picks the tracks). It only
  *observes* until armed: no audio graph, nothing leaves the page.
- **Channels** match the app (`api/protocol.rs`): `0` = `mic` (what the page
  sends, else its `getUserMedia` track), `1` = `remote` (every received
  track, mixed). One `AudioContext` at the device rate → a mono bus per
  channel → an AudioWorklet that posts 2048-sample i16 blocks for both
  channels together (silence for a channel with no track), so `seq` stays
  gap-free and the channels aligned. Frames: `[u8 channel][u32 seq LE][i16…]`.
  The page's tracks are never cloned, stopped or muted.
- **Fallbacks**: with no peer connection in the page, the audio of playing
  `<audio>`/`<video>` elements (their `srcObject` tracks, else
  `captureStream()`) is the remote channel, and only then, if no mic was
  seen, the hook asks `getUserMedia` itself — **this can show a second
  microphone prompt** (a one-time grant in Chrome, a temporary one in
  Firefox). Last, on **Chrome only**, if 4 s after Start the page still shows
  no remote audio (e.g. a client that plays WebAssembly-decoded audio
  through WebAudio), the background captures the tab's audio with
  `tabCapture` through an offscreen document (`offscreen.html`) and plays
  it back so the tab stays audible. Firefox has no `tabCapture`. The
  worklet loads from a Blob URL, else a data: URL, else a ScriptProcessor
  runs instead (strict page CSPs).
- **MAIN ↔ ISOLATED** (`src/shared/handshake.ts`): a private MessageChannel
  handed over with a handshake that works whichever script the browser
  injects first (Firefox may run MAIN first) and hides the offers from the
  page. Page buffers are copied with `structuredClone` in the ISOLATED world
  (Firefox Xray wrappers refuse `new Uint8Array(buffer)`). A watchdog
  disarms the hook if the extension side goes quiet.
- **ISOLATED → background**: a runtime port. Chrome ≥ 148 carries
  `ArrayBuffer`s thanks to `"message_serialization": "structured_clone"`
  (Firefox always does); both ends negotiate with a probe buffer and fall
  back to base64 on older Chromium engines (`src/shared/transport.ts`).
- **Background** (`src/background/`): per-tab session (`session.ts`, a pure
  state machine): checks the app with `GET /app/version`, arms the page,
  opens `ws://127.0.0.1:<port>/live?token=…`, sends `start {title, url,
  platform, rate, channels: 2}`, numbers `seq` per connection, buffers up
  to ~30 s while (re)connecting, reconnects with capped exponential backoff
  (a reconnect is a new `start`, i.e. a new item), stops retrying on a
  wrong token / meetings off / other protocol, and sends `ping` after 10 s
  without audio. `platform` is `meet`, `teams`, `zoom` (else `other`).
- The Firefox manifest sets its own `content_security_policy`: Firefox's
  MV3 default includes `upgrade-insecure-requests`, which breaks
  `ws://127.0.0.1` (spike #104).
- Scope: the top frame only (no `all_frames`); if a platform runs its call
  in an iframe, that shows up in the manual checks (#184).

## Side panel (#129)

The side panel (Chrome) / sidebar (Firefox) is a **live mirror** of the
meeting in the active tab; editing happens in the app (plan E3).

- **Live lines** with their timestamp and a **speaker chip**, drawn by the
  app's own transcript components (`@sussurro/transcript`). Chips use the
  app's labels and colours (`src/shared/speakers.ts` mirrors
  `speakers/doc.rs`; a unit test compares them): `You` on the mic channel,
  `Voice N`, and the names the app sends with `speaker {id, label}`. A
  `segment` `updated` replaces the line with the same id.
- **Follows the newest line** unless the user scrolled up; then **Jump to
  live** brings it back. The list is a keyboard-scrollable region; each new
  line is also read out through a *polite* live region (never
  interrupting).
- **Backlog**: from the app's `status` (`backlog_s`): up to date, *N s
  behind*, or, past 30 s, a note that Sussurro is slower than the meeting
  (nothing is lost); *Finishing* after Stop.
- **Open in Sussurro** (`POST /items/{id}/open`), **Copy as text**
  (`GET /items/{id}/export?format=txt`, then the clipboard) and **Create
  .srt** (`…?format=srt`, saved through a Blob link: no `downloads`
  permission), on the meeting's current item. *Create .srt* shows only when
  the app's subtitles setting is *on request* (`subtitles` in
  `GET /app/version`; with *always* the app writes the `.srt` itself) and
  works once the recording ends.
- A **reconnect** is a new item on the app: its lines follow the earlier
  ones under a "connection lost" note, and the buttons act on the new item.
  A new **Start** clears the panel.

The background keeps each tab's transcript (`src/shared/live.ts`, a pure
reducer over the app's `/live` messages), so a panel opened mid-meeting,
or switching tabs, shows everything so far: the panel asks for a snapshot
(`panel:transcript`) and then applies the numbered changes the background
broadcasts (`panel:live`), asking again on a gap. The background must stay
free of page code (React, the transcript CSS): Chrome runs it as a service
worker without a DOM, and the build refuses a `background.js` that uses
`document`.

## Meet names (#131)

Who spoke, per remote line (plan §4.3, decision P8): the mic channel is
always **You** (layer 1); on **Google Meet** the page adds names (layer 2);
whatever stays unnamed is **Voice N** from the app's voice clustering
(layer 3). **Teams and Zoom web get layers 1 and 3 only** in 0.9: no
observer runs there.

On a Meet tab, while capturing, the MAIN-world observer
(`src/content/meet/`) runs every 100 ms:

- **Who is speaking, from the audio** (`csrc.ts`): Meet tags each packet of
  its few remote audio streams with the speaker's contributing source
  (CSRC), stable per participant for the call. The observer polls
  `RTCRtpReceiver.getContributingSources()` on the remote audio receivers
  and sends `speaker_active` / `speaker_idle` with `id: "csrc:<n>"`,
  `source: "rtp"` (no indicator lag). A CSRC on two receivers for several
  polls is a mirror of the current speaker and is ignored for the call.
- **Names, from the page** (`dom.ts`, `selectors/`): participant tiles,
  the user's own tile and the lit ("speaking") tile, read through a
  **versioned, data-only selector set** — attributes and structure only,
  never obfuscated classes, never visible text or labels (localized).
  Sets are picked at run time (first whose fingerprint fits, re-checked
  every 30 s; several can be live for phased rollouts). The shipped set
  `meet-2026-09a` is **provisional and unverified** (`verifiedOn: null`):
  #184 confirms or replaces its values from our own inspection of a live
  page. A MutationObserver on the set's attributes plus a 1 s refresh
  keeps the read cheap; the observer never clicks or opens panels.
- **Binding** (`binder.ts`): when exactly one CSRC speaks and exactly one
  remote tile is lit, that is a vote; 5 agreeing votes with a margin of 3
  lock the name (`speaker_name {id, name}`), which then applies to every
  line of that CSRC, even after the page breaks. One live CSRC per name
  (a rejoin may take it after 30 s), namesakes and the user's own name
  never bind, and 10 contradicting votes in a row drop a binding
  (`name: null`).
- **Without CSRCs** (another browser or transport), after 2 s of remote
  speech the lit tiles themselves become the timeline (`source: "dom"`,
  `id: "tile:<hash>"` with the name); the app compensates their lag.
- **Participants**: the names on the remote tiles (the user excluded),
  through one name guard (`names.ts`: no ids, timers or "You").
- **Health** (`health.ts`): cross-checks, not "a selector matched": 20 s
  of remote speech without a single lit tile means the speaking hook is
  broken; tiles without names mean the name hook is. A broken hook stops
  sending names — never guesses — and `observer_health {state:
  "names_unavailable"}` goes to the app and to the side panel
  (`PanelState.names`).

Times: the page stamps events in its own worklet frames; the background
turns them into milliseconds on the connection's audio clock (the clock of
the app's segments) and replays the page's state (participants, names,
active speakers, health) to a new connection (`src/shared/speakerEvents.ts`).
The app attributes each remote line to the name active for most of it
(see `sussurro/src-tauri/src/speakers/names.rs`).

**Not done yet**: Meet's live captions as an opt-in name fallback (a
follow-up); lag numbers and the real hooks come from the live check (#184).

## Layout

| Path | What it is |
|---|---|
| `manifest.chrome.json` | Chrome/Edge/Brave, MV3: service-worker background, `side_panel` |
| `manifest.firefox.json` | Firefox ≥ 128, MV3: `background.scripts`, `sidebar_action` |
| `src/background/` | background worker: per-tab session, WebSocket to the app, badge, Chrome tab capture |
| `src/content/main-world.ts` | MAIN-world content script (the `RTCPeerConnection` hook, E4) |
| `src/content/isolated-world.ts` | ISOLATED-world content script (relay to the background) |
| `src/content/meet/` | Meet name observer (#131): CSRC timeline, selector sets + fixtures, binder, health |
| `src/offscreen/`, `offscreen.html` | Chrome only: tab-capture fallback |
| `e2e/` | capture harness (Playwright + a fake app) |
| `src/sidepanel/`, `sidepanel.html` | side panel (Chrome) / sidebar (Firefox): live lines, Start/Stop, item actions (#129) |
| `src/options/`, `options.html` | options page: pairing with the app, Test connection (#127) |
| `src/shared/` | helpers shared by the entry points (`pairing.ts`: storage keys and URLs; `connection.ts`: the connection test) |
| `scripts/build.ts` | the build: pages + scripts + manifest + icons + zip |

Permissions stay minimal: `storage` (plus, on Chrome, `sidePanel` and the
tab-capture fallback's `tabCapture` and `offscreen`), and host access only
to the meeting pages and to `http://127.0.0.1/*` (the local app). No
`<all_urls>`. The unit tests check this.

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

CI (`.github/workflows/test.yml`, `extension` job) runs all of the above,
plus the capture harness.

### Capture harness

`e2e/run.ts` loads the built extension into real browsers (Playwright's
Chromium and Firefox), opens a local two-peer WebRTC call (the user's side
sends the browser's fake microphone — 440 Hz in Chromium, 1 kHz in Firefox —
the other side a 300 Hz tone) and a fake Sussurro app (`/app/version`,
`/live` and the item routes, with the app's Origin and token checks). Per configuration it
presses Start in the side panel and checks: no socket before Start, the
side panel showing the fake app's scripted lines (a correction applied,
speaker chips, timestamps, the backlog) and reaching `open` / `export`
(txt, then srt after Stop), a new Start clearing it, the
`start` message, both channels arriving with their own tone, `seq` from 0
without gaps, the call unaffected both ways, Stop, and `stop` when the tab
closes. On a fake Meet page (`e2e/meet.html`, served at `meet.google.com` by
a route; Chromium configurations only) it checks the Meet name observer:
CSRC `speaker_active`/`speaker_idle`, bound names, participants without
the user, healthy observer. Configurations: `chromium` (binary messaging, Meet-like
`replaceTrack`), `chromium-json` (the manifest key removed: base64, as on
Chrome < 148) and `firefox` (installed as a temporary add-on and driven
over the remote debugging protocol, which Playwright lacks for Firefox
extensions). About 30 s headless.

```bash
npx playwright install chromium firefox   # once (PLAYWRIGHT_BROWSERS_PATH to choose where)
npm run build && npm run test:e2e         # or: npm run test:e2e -- firefox
HEADED=1 npm run test:e2e -- chromium     # watch it
```

Temporary profiles go under `$E2E_TMPDIR` (default: the OS temp folder). The
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

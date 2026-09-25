# Sussurro browser extension

The browser half of Sussurro's meeting capture (0.9): it captures a web
meeting (Google Meet, Microsoft Teams, Zoom web client) and streams it to the
Sussurro app running on the same computer, which transcribes and saves it.
The extension is a capture device plus a live mirror; editing happens in the
app (plan decision E3).

Pairing with the app (#127), capture (#128) and the live transcript in the
side panel (#129) work; Meet names (#131) are implemented and wait for a
live check. Capture is verified automatically against a local two-peer call
in Chromium, Firefox and Edge (see *Browsers* and *Capture harness*); real
Meet / Teams / Zoom calls are still checked by hand (#184). Meetings are
always on in the app (#138): there is no switch, only pairing. How users
install and pair it is in the
[main README](../README.md#browser-extension-meetings); this file is for
working on it.

## Pairing

1. In Sussurro: Settings → Browser extension — make sure the local API is
   on (the card shows its status; it starts with the app).
2. Click **Copy pairing code**: one string, `sussurro:<port>:<token>`.
3. In the extension's options page (right-click the toolbar button →
   Options), paste it and **Save and test**. Port and token can also be
   entered separately.

**Test connection** calls `GET /app/version` with the token and reports,
distinctly: app not reachable (not running, local API off, wrong port),
wrong token, an app without the extension routes, another protocol, or a reply blocked by the
browser. **Regenerate token** in the app invalidates the old token at once;
every paired browser must be paired again.

The pairing lives in the extension origin's IndexedDB (`sussurro` →
`pairing`, keys `port` and `token`), read and written only through
`src/shared/pairing.ts` (`getPairing`, `setPairing`, `onPairingChanged`,
`liveUrl`…) — not in `storage.local`, which content scripts in the
meeting pages can read (#217; Firefox has no `setAccessLevel`, Chrome
restricts `local` only from 140). A content script's `indexedDB` is the web
page's, so the token is out of their reach. A pairing an older version
left in `storage.local` is moved on the next read. Changes are announced
on a `BroadcastChannel` (same origin only); a write to `storage.local` the
old way (the E2E harness does) is still picked up and moved. On Chrome ≥
140 the background also restricts `storage.local` to trusted contexts. The pairing-code format is
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
- **What the page can send is capped** (#217): the MAIN world is the
  page's, so a page script (or an XSS on the page) can post on its port
  while a capture runs. The ISOLATED relay drops beyond 200 speaker events
  at once / 20 per s, 100 audio blocks / 60 per s, 50 state messages / 10
  per s, blocks over 64 KiB and bad counters; the background again (400 /
  20 per s per tab, a counter jump skips at most 1500 frames, repeated
  participant lists dropped, at most 1000 speaker ids kept); the app
  caps per connection (`api/live.rs`) and bounds the silence `seq` gaps
  insert by the wall clock (`sources/browser.rs`). Constants in
  `src/shared/ratelimit.ts`.
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
  opens `ws://127.0.0.1:<port>/live` and sends `auth {token}` first (an
  app reporting `live_auth: "message"`, #217; older apps get `?token=…`,
  which Chrome logs when the connection fails), then `start {title, url,
  platform, rate, channels: 2, language?}` (the panel's language, #288),
  numbers `seq` per connection, buffers up
  to ~30 s while (re)connecting, reconnects with capped exponential backoff
  (a reconnect is a new `start`, i.e. a new item), stops retrying on a
  wrong token / app too old / other protocol, and sends `ping` after 10 s
  without audio. `platform` is `meet`, `teams`, `zoom` (else `other`).
- The Firefox manifest sets its own `content_security_policy`: Firefox's
  MV3 default includes `upgrade-insecure-requests`, which breaks
  `ws://127.0.0.1` (spike #104).
- **Supported hosts** (`src/shared/platform.ts`, both manifests; a unit
  test keeps the manifests, `MEETING_MATCHES` and `detectPlatform` in
  step):

  | Platform | Match pattern | Frames |
  |---|---|---|
  | Google Meet | `https://meet.google.com/*` | top frame |
  | Microsoft Teams | `https://teams.microsoft.com/*`, `https://teams.live.com/*`, `https://teams.cloud.microsoft/*` (organisational tenants from 2026-09-30, #287) | top frame |
  | Zoom web client | `https://*.zoom.us/wc/*` | every frame whose own URL matches (`all_frames`) |

- **Frames** (#287): Zoom runs the meeting in a same-origin iframe under
  `/wc/`, so its two entries set `all_frames` (no `match_about_blank` /
  `match_origin_as_fallback`: a frame must itself be a `/wc/` page). Each
  frame's ISOLATED script opens its own port on Start and reports what its
  hook sees; the background arms **one frame per tab**
  (`src/background/frames.ts`: a frame with a peer connection at once,
  else the best one after 500 ms; only a frame with a stronger sign
  replaces it) and drops every other frame's audio and events, so a top
  frame that also opens the mic is never captured twice. Only the top frame
  answers `page:info`, so the panel shows the tab. Meet and Teams stay
  top-frame only.

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
- **Language** (#288), next to Start: *Auto-detect* plus the languages of
  the app's active engine by native name, from `GET /app/languages`
  (extension token, like `/app/version`: `{engine, default, languages:
  [{code, name}]}` — Whisper 99, an English-only Whisper model `en`,
  Parakeet 25, Qwen3-ASR 29 codes). The choice is remembered **per
  platform** in `storage.local` under `meetingLanguage` (`{meet, teams,
  zoom, other}`, not a secret — content scripts may read it), through
  `src/shared/language.ts` only; the first time on a platform, or when the
  engine no longer offers the remembered code, the app's dictation
  language (`default`) is picked, else Auto-detect. It is sent as
  `start.language` (`panel:start {language}` → the background, which keeps
  it for that meeting's reconnects), greyed while recording, and shown in
  the recording header (`PanelState.language`, so a panel opened
  mid-meeting shows it too). The app checks it against the engine: an
  unknown code falls back to the dictation language with a `warning`,
  never stopping the meeting. With an app without the route (404) the
  selector is hidden and no `language` is sent — the app's dictation
  language applies, as before. Additive: the protocol stays 2.

The background keeps each tab's transcript (`src/shared/live.ts`, a pure
reducer over the app's `/live` messages), so a panel opened mid-meeting,
or switching tabs, shows everything so far: the panel asks for a snapshot
(`panel:transcript`) and then applies the numbered changes the background
broadcasts (`panel:live`), asking again on a gap. The background must stay
free of page code (React, the transcript CSS): Chrome runs it as a service
worker without a DOM, and the build refuses a `background.js` that uses
`document`.

## Recording notice (#136)

The first **Start** shows a short notice in the panel: the other
participants may need to be told, and some may have to agree, depending on
local rules (neutral wording, no legal advice). **Start recording** goes
ahead, **Cancel** starts nothing. *Don't show this again* (ticked by
default) is stored in `storage.local` under `recordingNoticeSeen`, read and
written only through `src/shared/notice.ts`; clearing the extension's
storage shows it again, and so does **Show the notice again** on the
options page. While recording, a "Recording other people" line stays under
the controls. The strings live once, in the app's
`sussurro/src/lib/recordingNotice.ts`, imported here as `@sussurro/notice`
(same wording as the app's New screen).

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

## Browsers (#137)

One Chrome build for the Chromium family (Chrome, Edge, Brave; ≥ 116) and
one Firefox build (desktop, ≥ 140: the first release that reads
`data_collection_permissions`, #234; Firefox for Android is not supported —
see *Firefox version and Android* below). Every feature above works in
both; where they differ:

| | Chrome / Edge / Brave | Firefox |
|---|---|---|
| Panel | `side_panel`, opened by the toolbar button | `sidebar_action`, toggled by the toolbar button |
| Which tab the panel follows | the window's active tab | the sidebar's window's active tab |
| Background | service worker | event page (`background.scripts`; MV3 Firefox has no persistent background) |
| Audio to the background | `ArrayBuffer`s (Chrome ≥ 148 with `message_serialization`), else base64 | `ArrayBuffer`s |
| Extension CSP | the default | its own: the MV3 default's `upgrade-insecure-requests` breaks `ws://127.0.0.1` (#104) |
| Host access | granted at install (users can restrict it) | can be withheld per site |
| Tab-capture fallback | yes (`tabCapture` + offscreen document) | none: not built, never shown |

Identical in both: pairing in the extension's IndexedDB (#217), the MAIN-world hook
(`world: "MAIN"`, Firefox ≥ 128), the Meet name observer
(`getContributingSources()` exists in both), the recording notice, the
live lines and the item buttons; without host access to the meeting
sites, the panel offers **Allow access** in both.

**Staying loaded.** Both browsers unload an idle background after about
30 s, and the background owns the meeting's WebSocket and the panel's
transcript. Chrome (≥ 116) counts WebSocket traffic as activity; Firefox
counts only extension events and API calls — the page's audio messages
keep it loaded while capturing, but a quiet stretch (the app finishing a
long backlog after Stop, when the socket may carry nothing for a while)
would unload it: the socket drops and the panel stays on "Stopping". So
from Start until the app's `done` the background makes a trivial API call
(`runtime.getPlatformInfo`) every second (`holdsBackground` in
`session.ts`); otherwise nothing keeps it loaded. The harness runs Firefox
with a 2 s idle timeout to prove it (see *Capture harness*).

**Edge** runs the Chrome build unchanged and is in the harness (and CI).
**Brave** too; its Shields and its *Localhost access* permission (1.54+)
govern web pages' requests to `127.0.0.1`, while Sussurro's socket and
requests come from the extension's own background and panel, and the
harness passes in Brave 1.95 with default Shields. If the options page's
**Test connection** says the app is not reachable while it runs, check
`brave://settings/content/localhostAccess` and whether trackers & ads
blocking is set to *aggressive*, and report it on #184.

**Copy as text** writes the clipboard, which needs the click's user
activation: the harness checks the export request, but its Firefox clicks
are scripted (no user activation), so the clipboard itself is a manual
check there (#184).

## Layout

| Path | What it is |
|---|---|
| `manifest.chrome.json` | Chrome/Edge/Brave, MV3: service-worker background, `side_panel` |
| `manifest.firefox.json` | Firefox ≥ 140 (desktop), MV3: `background.scripts`, `sidebar_action` |
| `src/background/` | background worker: per-tab session, WebSocket to the app, badge, Chrome tab capture |
| `src/content/main-world.ts` | MAIN-world content script (the `RTCPeerConnection` hook, E4) |
| `src/content/isolated-world.ts` | ISOLATED-world content script (relay to the background) |
| `src/content/meet/` | Meet name observer (#131): CSRC timeline, selector sets + fixtures, binder, health |
| `src/offscreen/`, `offscreen.html` | Chrome only: tab-capture fallback |
| `e2e/` | capture harness (Playwright + a fake app) |
| `src/sidepanel/`, `sidepanel.html` | side panel (Chrome) / sidebar (Firefox): live lines, Start/Stop, item actions (#129) |
| `src/options/`, `options.html` | options page: pairing with the app, Test connection (#127), recording notice reset (#136), About with the third-party licences (#138) |
| `src/shared/` | helpers shared by the entry points (`pairing.ts`: storage keys and URLs; `connection.ts`: the connection test) |
| `scripts/build.ts` | the build: pages + scripts + manifest + icons + zip |
| `scripts/amo-sign-gate.sh` | release workflow: whether to sign the Firefox build on AMO (#228) |
| `scripts/update-manifest.ts`, `scripts/updates.ts` | adds a released version to Firefox's self-hosted update manifest (`docs/extension/updates.json`) |

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
npm run lint             # web-ext lint --self-hosted on dist/firefox (run after build:firefox)
```

CI (`.github/workflows/test.yml`, `extension` job) runs all of the above,
plus the capture harness.

### Capture harness

`e2e/run.ts` loads the built extension into real browsers (Playwright's
Chromium and Firefox), opens a local two-peer WebRTC call (the user's side
sends the browser's fake microphone — 440 Hz in Chromium, 1 kHz in Firefox —
the other side a 300 Hz tone) and a fake Sussurro app (`/app/version`,
`/app/languages`, `/live` and the item routes, with the app's Origin and
token checks). Per configuration it
presses Start in the side panel and checks: no socket before Start, the
side panel showing the fake app's scripted lines (a correction applied,
speaker chips, timestamps, the backlog) and reaching `open` / `export`
(txt, then srt after Stop), a new Start clearing it, the
language selector (#288: first on the app's dictation language, English
picked → `start {language: "en"}`, greyed and shown in the header while
recording, remembered for the next Start on the site while a first Meet
start takes the app's default), the `start` message, both channels arriving with their own tone, `seq` from 0
without gaps, the call unaffected both ways, Stop (the fake app then
stays silent for 5 s before `done`, as a busy app can), and `stop` when
the tab closes; also the not-paired panel, which follows the pairing once
stored. On a fake Meet page (`e2e/meet.html`, served at
`https://meet.google.com/` by a Playwright route, so the shipped
content-script patterns match it) it checks the Meet name observer, in
every configuration: CSRC `speaker_active`/`speaker_idle`, bound names,
participants without the user, healthy observer. Two more routed pages
(#287) check capture with the shipped patterns: `e2e/loopback.html` (a
loopback call) at `https://teams.cloud.microsoft/`, and `e2e/zoom.html` at
`https://app.zoom.us/wc/e2e/join`, whose call runs in a same-origin
`/wc/e2e/meeting` iframe while the top frame holds a mic preview — the
hook in both frames, one session with the tab's platform, URL and title,
both channels carrying the iframe's call (not the preview), and audio
length matching the clock (captured once).

Configurations: `chromium` (binary messaging, Meet-like `replaceTrack`),
`chromium-json` (the manifest key removed: base64, as on Chrome < 148) and
`firefox` — installed as a temporary add-on and driven over the remote
debugging protocol (Playwright can't load Firefox extensions), through the
parent process where no extension page can: Firefox runs with a 2 s
event-page idle timeout and is told to ignore the harness's own DevTools
attachment (which would otherwise keep the page loaded), and the checks
count suspensions (suspended when idle, never from Start to `done`,
suspended again afterwards); and it opens the **real sidebar** mid-meeting
(lines so far, follows the window's active tab, closed and reopened, Stop
pressed there). Only when named: `edge` (the installed Edge, Playwright's
`msedge` channel; also in CI) and `brave` (the installed Brave, or
`BRAVE_PATH`), both with the Chrome build. About 90 s headless for the
default three.

```bash
npx playwright install chromium firefox   # once (PLAYWRIGHT_BROWSERS_PATH to choose where)
npm run build && npm run test:e2e         # chromium, chromium-json, firefox
npm run test:e2e -- firefox edge brave    # a choice
HEADED=1 npm run test:e2e -- chromium     # watch it
```

Temporary profiles go under `$E2E_TMPDIR` (default: the OS temp folder).

`npm run lint` (`scripts/lint.ts`) runs `web-ext lint --self-hosted` on
`dist/firefox`: the add-on is distributed outside AMO's listing with its
own `update_url`, which listed-mode lint rejects (`MANIFEST_UPDATE_URL`).
It fails on any error and on any warning not accepted in
`scripts/lint-policy.ts` (#234). Today: 0 errors, three accepted warnings,
the same three AMO's validator shows — explained to Mozilla's reviewers in
[`AMO-REVIEWER-NOTES.md`](AMO-REVIEWER-NOTES.md), sent with every signing
submission:

- `UNSAFE_VAR_ASSIGNMENT` ×2 (`assets/page-*.js`): React DOM's own write
  for the `dangerouslySetInnerHTML` prop — in
  `react-dom-client.production.js`, one in `setProp` and one in
  `setPropOnCustomElement`, minified as `n?.__html!==u&&(l.innerHTML=u)`
  (traced through a source-mapped build). No Sussurro code uses
  `dangerouslySetInnerHTML` or assigns `innerHTML` (`src/`, and the shared
  `sussurro/src/transcript`), so the path never runs. It can't be
  tree-shaken — it is a `case` of the prop setter every element goes
  through — and patching React's code in the build would hand AMO a
  modified library to review instead of a known one. The gate accepts only
  these two sites, matched by that exact pattern at the flagged column: any
  other `innerHTML` warning fails the lint.
- `KEY_FIREFOX_ANDROID_UNSUPPORTED_BY_MIN_VERSION`: see below.

### Firefox version and Android (#234)

- **Desktop minimum: Firefox 140** (`strict_min_version: "140.0"`). AMO
  requires `data_collection_permissions`, which Firefox reads from 140 on;
  with 128 the validator warned (`KEY_FIREFOX_UNSUPPORTED_BY_MIN_VERSION`).
  ESR 128 reached end of life on 2025-09-16; ESR 140 is the oldest
  supported Firefox (end of life 2026-10-13, when its users move to ESR
  153 — [Mozilla's ESR calendar](https://whattrainisitnow.com/release/?version=esr)).
  0.10.0, already published, keeps its 128 entry in `updates.json`: a
  Firefox 128–139 install stays on 0.10.0 instead of being offered an
  update it could not install.
- **Firefox for Android: not supported.** The add-on needs the desktop
  sidebar and a Sussurro app on the same device. The manifest has no
  `browser_specific_settings.gecko_android`, which is how AMO learns a
  version is desktop-only (declaring it, even empty, marks it
  Android-compatible), and each submission also sets the version's
  compatibility to Firefox only (`--amo-metadata`, `scripts/amo.ts`).
  Nothing offers it on Android: it is unlisted on AMO and the release
  `.xpi` is documented for the desktop; side-loading it into a Firefox for
  Android build that allows installing from a file would give an add-on
  without its sidebar — unsupported. The linter still checks the
  Android minimum — without `gecko_android` it falls back to
  `gecko.strict_min_version` (140 < 142, when Firefox for Android started
  reading the key) — hence the one accepted
  `KEY_FIREFOX_ANDROID_UNSUPPORTED_BY_MIN_VERSION` warning. Raising the
  desktop minimum to 142 would silence it but lock out ESR 140 for no
  benefit.

## Releases, signing and updates (#228)

The release workflow attaches both zips to the GitHub release. On a tag,
with the `AMO_JWT_ISSUER` / `AMO_JWT_SECRET` repository secrets set, it
also signs the Firefox build on addons.mozilla.org through the **unlisted**
channel (self-distribution: AMO signs it but does not list it) and attaches
`sussurro-extension-firefox-<version>.xpi`, which installs permanently in
release Firefox. Without the secrets, on a build-only run or for a
pre-release version, signing is skipped with a notice
(`scripts/amo-sign-gate.sh`). Maintainer setup: `docs/releases.md` →
*Firefox extension signing and updates*.

- **The gecko id `sussurro@darumahq.it` must never change**: AMO ties the
  add-on and every signed version to it, and installed copies update only
  within the same id.
- AMO signs a version number once. A re-run of the job skips a version whose
  `.xpi` is already on the release; a version whose submission failed
  after upload needs a version bump, or the signed file downloaded from
  AMO's Developer Hub and attached by hand.
- **Updates**: `browser_specific_settings.gecko.update_url` points at
  `https://fullo.github.io/Sussurro/extension/updates.json`, which GitHub
  Pages serves from `docs/extension/updates.json` on `main`. After
  **publishing** a release, add it:

  ```bash
  cd extension && npm ci
  npm run update-manifest -- 0.10.1   # downloads the release's .xpi, checks it, writes the entry
  # from a local copy: npm run update-manifest -- 0.10.1 path/to/sussurro-extension-firefox-0.10.1.xpi
  ```

  It checks the file's version, gecko id and minimum Firefox (it must be
  `manifest.firefox.json`'s, 140.0 since #234) and writes `version`,
  `update_link` (the release asset), `update_hash` (`sha256:` of the
  signed file) and that `strict_min_version` into
  `docs/extension/updates.json`, leaving the published entries as they are
  (0.10.0 keeps 128.0); commit it on a branch
  and merge. Installed copies pick it up on their next update check
  (about once a day). If the add-on is ever listed on AMO instead,
  `update_url` must go (AMO refuses it for listed add-ons).

### Source code for AMO review

The shipped scripts are bundled and minified by Vite, so every AMO
submission carries the sources they are built from: an archive of
`LICENSE`, `extension/`, `sussurro/package.json` (the version),
`sussurro/src/` (the transcript components, pairing code and recording
notice the extension imports) and the three icons in
`sussurro/src-tauri/icons/`. To rebuild the submitted add-on from it
(Node.js ≥ 24 with npm, any OS):

```bash
unzip sussurro-extension-source-<version>.zip -d sussurro && cd sussurro/extension
npm ci
npm run build:firefox      # → dist/firefox/, the directory that was signed
```

The build is deterministic: the same sources give byte-identical files.

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

A temporary add-on is removed when Firefox quits: it is for development only;
users install the signed `.xpi` from the release. In Firefox, MV3 host
permissions can be granted per site: if a meeting page is not picked up,
allow it under the extension's **Permissions** tab in `about:addons`.

Alternatively, `npx web-ext run --source-dir dist/firefox` starts a fresh
Firefox profile with the add-on installed.

## Licenses

The options page's **About** section lists the extension's third-party
licences: its npm production dependencies (React, `react-dom`,
`scheduler`, `webextension-polyfill`), never the dev tooling. The list is
`src/options/licenses.json`, generated by the app's licence script in
extension mode and committed. **Regenerate after changing dependencies:**

```bash
cd extension && npm ci && npm run licenses   # = node ../sussurro/scripts/gen-licenses.mjs --extension
```

A unit test (`src/options/licenses.test.ts`) fails when the list and
`package-lock.json` disagree, or when a dev dependency gets in. The app's own
list (`cd sussurro && npm run licenses`) is separate.

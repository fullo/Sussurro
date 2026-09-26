# CLAUDE.md — shared project memory

Project context for Claude Code sessions. This file is committed so every
machine and user working on Sussurro shares the same context — **record
project decisions here, not in per-machine memory.**

## Project layout

- `sussurro/` — the Tauri 2 app (React + TypeScript frontend, Rust backend
  in `sussurro/src-tauri/`). The repo root only holds docs and CI.
- `extension/` — the 0.9 browser extension (Vite + TS + React, one build per
  browser, `web-ext lint` in CI; see `extension/README.md`). It shares
  `sussurro/src/transcript/` via the `@sussurro/transcript` alias and takes
  its version from `sussurro/package.json`. Its third-party license list is
  separate from the app's `licenses.json` (see below).
- Build instructions per OS live in `docs/compile/{windows,macos,linux}.md`
  — keep them updated when build requirements change.
- The About dialog's third-party license list is `sussurro/public/licenses.json`,
  generated from the resolved deps (cargo + npm). **Regenerate after changing
  dependencies:** `cd sussurro && npm run licenses` (needs the Rust toolchain;
  not run in CI to keep the pipeline simple — the file is committed). The
  browser extension's own list (`extension/src/options/licenses.json`, its
  options page's About) comes from `cd extension && npm run licenses` (same
  script, `--extension`); a unit test keeps it in step with its lockfile.
  Both lists hold production deps only — the script fails on a dev one.

## Standing decisions

- **Going public at v0.5.0** (decided 2026-07-06): the repo becomes public and
  the auto-updater unfreezes (the endpoint 404'd anonymously while private).
  Sequence chosen: **publish immediately**, accepting unsigned-installer OS
  warnings until code signing lands. No code change needed to unfreeze — it's
  the GitHub visibility toggle + a published (non-draft) release.
- **No external OS code signing for now** (decided 2026-07-06). Both macOS
  (ad-hoc → right-click Open) and Windows (unsigned → SmartScreen "Run anyway")
  ship without OS signing; users click through, and this is documented in the
  README + the macOS blog post. Don't set up SignPath / Apple Developer ID /
  Azure Trusted Signing until the maintainer explicitly revisits — the options
  are written up in `docs/windows-signing-signpath.md` + `docs/releases.md` for
  when that happens. The updater's own minisign signing is independent and
  already active, so **auto-update works regardless of OS signing**.
- **The v0.2.0 draft release is kept intentionally** — do not delete it.
- **License: AGPL-3.0-or-later** (chosen 2026-07-06). Copyleft that also covers
  network use (Sussurro exposes a local HTTP API), so no one can build a
  closed/proprietary product on it. Verified compatible with the whole dep tree
  (all permissive + MPL-2.0 + two GPL-3.0-or-later crates; no GPL-2.0-only or
  proprietary). `LICENSE` holds the verbatim text; SPDX `AGPL-3.0-or-later` is
  set in `package.json` + `Cargo.toml`. Copyright is retained by the author
  (Francesco Fullone / DarumaHQ) so a **commercial/dual license** can be sold
  later — don't relicense or add a CLA-less outside contribution that would
  fragment that. A permissive license (MIT) is NOT an option here: the
  GPL-3.0-or-later deps require a copyleft-compatible license.
- **macOS is Apple Silicon only (min 11.0)**: `ort` (ONNX Runtime) has no
  prebuilt binaries for `x86_64-apple-darwin`. Don't re-add the Intel
  target to the release matrix.
- **LLM profiles replace the flat cleanup settings (0.8, #119)**:
  `Settings.llm_profiles` + `cleanup_profile` (`llm/profile.rs`). A pre-0.8
  `settings.json` is migrated on load (`Settings::normalize`) into one
  "Local" profile selected for cleanup, and saved once; the old
  `cleanup_api`/`ollama_url`/`ollama_model`/`api_key` keys are read for the
  migration and **never written again**. Consequence, accepted: downgrading
  to ≤ 0.7 loses a custom cleanup server (the old build falls back to its
  defaults, Ollama on localhost) — the updater only moves forward.
- **Profile API keys live in the OS credential store (0.8, #159)**
  (`secrets.rs`): `keyring-core` + one native store crate per OS (macOS
  Keychain, Windows Credential Manager, Secret Service over libdbus with
  pure-Rust crypto — not the zbus store, which would need a tokio context
  since ashpd enables zbus's tokio feature). Entry: service
  `com.sussurro.app`, user `llm-profile:<id>`. `settings.json` keeps only
  `api_key_storage: "keychain"`; `Settings::save` writes a key only when it
  is not in the store. At startup `load_keys` moves clear-text keys in
  (after a verified write) and reads stored ones; `set_settings` runs
  `sync_keys` (write on change, delete on clear or profile deletion; the
  UI's `api_key_storage` is never trusted). No working store → fallback to
  clear text (`"file"`) with a warning in the profile editor, retried every
  start. Keys never go into diagnostics, the portable config export, logs
  or the dev mock. Downgrading below 0.8 loses stored keys (re-enter them).
- **Link source rules (0.8, #123)** (`sources/url/`): only `http`/`https`,
  no credentials in the link (it is saved as `source: url:<link>`). Hosts on
  this computer or the local network (loopback, private, link-local incl.
  cloud metadata, CGNAT, unique-local IPv6, `localhost`) are **refused
  unless the user ticks "Allow local network addresses" for that run** —
  checked on IP literals, every resolved address and every redirect hop,
  with the connection pinned to the checked addresses (no DNS rebinding).
  Downloads: model-download timeouts, 2 GiB cap, temp file in
  `<app data>/link-downloads/` (never the archive), removed after the run
  and swept at startup. Video platforms: `yt-dlp` found on PATH or the
  Homebrew/winget/scoop/pip folders (E10, not bundled), argument vector
  only, `bestaudio[ext=m4a]/bestaudio`, `--ignore-config --no-playlist`;
  opus-only videos are refused (no ffmpeg bundled). **yt-dlp hardening
  (#216)**: it runs **only for `PLATFORM_HOSTS`** (a web page elsewhere is
  refused, never handed to the generic extractor), with `--use-extractors
  default,-generic` (a yt-dlp < 2022.08 that rejects the option is retried
  without it), `--downloader native`, proxy env vars removed, and
  `--proxy` pointing at an in-process loopback HTTP proxy
  (`sources/url/proxy.rs`: CONNECT + absolute-form http, std only) that
  runs `resolve_checked` on every host yt-dlp asks for and connects only to
  the checked addresses. The direct client uses `.no_proxy()` (a system
  proxy would bypass the pinning). Residual: yt-dlp still reaches whatever
  *public* hosts a platform extractor names.
- **Privacy gate for external LLM profiles (0.8, #122)**: enforced in the
  backend, never only in the UI. Recipes/questions on an external profile
  need a one-time consent token (`prepare_external_run`, bound to item +
  recipe/question + profile + host + model, single use, 120 s) consumed by
  `recipe_run`/`recipe_ask`; cleanup on an external profile needs the
  per-profile, host-bound `cleanup_opt_in`, else it keeps the raw text
  (checked in `cleanup::ollama::run_cleanup`, the one path every cleanup
  takes). No fallback from local to external anywhere. External sends are
  logged per item in `.sussurro/external-log.json` (metadata only, never
  content) and drive the Library's "sent externally" marker.
- **Speaker labels "Voice N" (0.9, #130)** (`speakers/`): WeSpeaker
  ResNet34-LM through the app's own `ort` (never sherpa-onnx), downloaded on
  first use into the models folder, pinned SHA-256 at a pinned HF revision,
  fail-closed; CC-BY-4.0 attribution in `licenses.json` (static "downloaded
  models" entry in `scripts/gen-licenses.mjs`). Fbank is Rust over `rustfft`,
  checked against kaldi-native-fbank. Live: online clustering 0.275 on ≤ 3 s
  windows; end of run and "Re-detect": voices under 10 s fold into the
  nearest; Re-detect = agglomerative 0.30 on the per-line embeddings stored
  in `segments.json` (mean of the line's windows), labels kept stable by
  speech overlap. Thresholds were tuned on English AMI clips (#107) — check
  on Italian before calling them final. Engine option `Job.speakers` (off
  by default); runs turn it on for every meeting (and for transcriptions
  with "Identify voices") — one channel is clustered as is; a browser meeting's mic is
  always "You" and only its remote channel is clustered. Embeddings never
  go to the UI (`Item::without_embeddings`). Linking a speaker to a person
  (`SpeakerEdit::Link`) sets `person_id`, takes the person's name unless
  the user renamed the speaker (old label kept in `label_before_link` for
  Unlink) and adds/completes the participant (never replaces an email).
- **"Identify voices" on transcriptions (0.9, #134, P11)**: a per-run
  option (`RunOptions.identify_voices`, New → File when the type is
  Transcription, and New → Link), off by default. Gating
  (`session::speaker_options`): note never; transcription = the toggle;
  meeting = always. The speaker panel shows on every transcription and
  meeting.
  After the fact, "Identify voices" in the panel re-reads the original
  file (`engine/identify.rs`: same tracker + end-of-run fold as a run),
  so it needs the file's path: kept **only on this machine** in
  `<app data>/source-files.json` (`engine/source_files.rs`, transcriptions
  only, path + size, forgotten on delete) — never in the frontmatter,
  the item folder or an export. Items from before #134 and link items
  (download deleted; re-downloading is out of scope) get an explanation
  instead of the button.
- **Local API for the browser extension (0.9, #126)** (`api/`): new routes
  (`GET /app/version`, `WS /live`, `POST /items/{id}/open`,
  `GET /items/{id}/export`) exist whenever the local API runs and need
  `Settings.extension_token` — `Authorization: Bearer` on
  HTTP, an extension `Origin` plus `?token=` or (#217, preferred; the app
  reports `live_auth: "message"`) `auth {token}` as the first message
  within 2 s on the WebSocket — the upgraded stream can't time out a
  read, so the deadline is checked when that message arrives; web-page
  origins are refused even with the token; CORS only for
  `chrome-extension://` / `moz-extension://`. `/clean`, `/transcribe`,
  `/history` stay token-less and CORS-less, behind their own switch (see
  the #215 hardening below). The token changes only via
  `extension_token_get`/`_regenerate` (`set_settings` keeps the current
  one). WebSocket = `tiny_http` upgrade + `tungstenite` on one blocking
  thread: replies are flushed after each client message (audio streams
  continuously; idle clients send `ping`), so no tokio/axum needed.
  A meeting is a two-channel engine run (`mic`, `remote`) into a `meeting`
  item, `source: browser:<host>`; page events go to
  `.sussurro/meeting-events.jsonl` for attribution (#131).
  **Page input is untrusted (#217)**: a page script or XSS drives the
  MAIN-world port during capture, so it is capped at every hop — ISOLATED
  relay and background (`extension/src/shared/ratelimit.ts`), and the app
  per connection (`api/live.rs`: 400 burst / 20 per s, 100k page events
  and 8 MB of participant lists per meeting, repeated identical
  `participants` dropped); `seq` gaps insert at most the wall clock since
  start plus 30 s of silence per channel (and 60 s per gap); the name
  timeline keeps ≤ 1000 speakers, ≤ 500 participants, ≤ 20k intervals
  (oldest half ages out; the end-of-run pass leaves older lines alone).
  The events file stays open while recording and is closed at the end of
  the audio (Windows can't rename a folder with an open file).
- **Local API hardening (#215)** (`api/guard.rs`, security notes at
  `api::spawn`): on **every** route, first `Host` must be
  `127.0.0.1:<port>`/`localhost:<port>` (DNS rebinding), then a request
  with an `Origin` must come from a browser extension (cross-site POSTs to
  the token-less routes). The token-less scripting routes answer only with
  `Settings.api_scripting` (read per request, applies at once): **off on a
  new install** — the Browser extension card's "Turn on the local API"
  sets `api_enabled` only (pairing itself sets nothing) — and a
  settings file without the key takes `api_enabled`'s value (saved once),
  so existing scripts keep working. Bodies: `Content-Length` checked
  against the cap (`/clean` 1 MiB, `/transcribe` 200 MiB) **before
  `as_reader()`** (which sends `100 Continue`), then `take(cap + 1)` into a
  buffer that never reserves past the cap. Serving: 4 workers pulling from
  the tiny_http server, `/clean` + `/transcribe` share 2 slots (503 +
  `Retry-After` when full), handlers under `catch_unwind`; `/live` moves to
  its own thread after the upgrade; a body must arrive within 120 s
  (`BODY_DEADLINE`, else 408). `/items/{id}/open|export` reach only
  `source: browser:` items (403, `code: not_extension_item`, explained by
  the extension). `settings.json` is written atomically with mode 0600 on
  Unix (`settings::write_private_atomic`).
- **tiny_http is vendored and patched (#223)**
  (`sussurro/src-tauri/vendor/tiny_http`, used via `[patch.crates-io]`;
  every change marked `Sussurro (#223)`; upstream 0.12.0 is unmaintained
  since 2022). Upstream read an unfinished body into `vec![0; remaining]`
  when a request dropped, so `Content-Length: 9223372036854775807` on any
  route **aborted the app** (reproduced); it also buffered header lines
  and chunk-size lines with no limit and took any number of headers and
  connections. Patched: an unread body closes the connection (nothing read
  or allocated), head ≤ 64 KiB / ≤ 100 headers (431), chunked framing
  lines ≤ 4 KiB, ambiguous `Content-Length`/`Transfer-Encoding` → 400,
  ≤ 128 connections, 30 s socket timeouts lifted on the `/live` upgrade
  (`tiny_http::Limits`). Residual (accepted): a local process can hold all
  128 connections and deny the API, not crash it. Don't replace the
  vendored crate with the crates.io one; moving to hyper would be a
  separate decision.
- **Archive API read routes (0.11, #250, P15)** (`api/archive.rs`, after
  the #249 token middleware): `GET /archive/items` (facet filters, opaque
  offset cursor, ≤ 200 per page, facets on `facets=true` capped at 100
  values), `/items/{id}` (frontmatter, text, speakers id + label, lines),
  `/export`, `/documents[/name]`, `/archive/people`. Read-only in 0.11.
  Responses are built **field by field** (never serialize `Item`/`Person`
  through: a field added later must not leak) — no embeddings, voice
  profiles, audio, paths or app data. Without the `people` scope emails are
  stripped (items, `.md` export frontmatter) **and can't be probed**:
  `SearchFilters.names_only` drops the FTS participants column (it indexes
  emails) and makes `participant=` match names only. Unknown query params
  are a 400; internal errors are a fixed message (anyhow errors name
  paths); bodies ≤ 16 MiB; ≤ 2 archive requests at once (`ARCHIVE_SLOTS`,
  503) so scripts can't starve the extension's workers.
- **Archive API write route (0.11, #251, P15)** (`api/archive_write.rs`):
  `POST /archive/items` (`write` scope) creates a **note** from a JSON body
  (`text`, `title?`, `tags?`, `categories?`, `cleanup?`; ≤ 1 MiB,
  declared length checked before reading, UTF-8, `application/json` or no
  Content-Type, unknown fields 400) with `source: api:<token name>`, date
  now, no audio/speakers, one segment per paragraph (raw = as sent), indexed
  at once; the UI refreshes on `archive-item-created`. **Cleanup through
  the API only on a local cleanup profile**: an external one is refused
  (409 `cleanup_external`) even with the user's cleanup opt-in — that
  consent (#122) can't be given by a script; the gate is re-checked on the
  same settings clone that cleans. Its own creation limit on top of the
  token's (per token burst 10 then 30/min, all tokens burst 30 then
  60/min), and `Idempotency-Key` per token, 1 h, in memory (same body →
  same item, `replayed: true`; other body → 422; pending → 409). The only
  write in 0.11 — metadata edits and audio uploads stay out.
- **System audio + mic (0.10 step 1, #139)** (`sources/system.rs`):
  *New → System audio + mic* (it records other people: the #136 notice
  asks first). The mic and **any second input
  device** (BlackHole, VB-Cable, a monitor source — names that look like
  loopback devices are listed first, a hint only) are two channels of one
  `meeting` item, `source: system`: mic = "You", system = clustered
  "Voice N" (`speaker_options`). Each device has its own `Recorder` +
  `StreamResampler`; the system device opens by exact name
  (`Recorder::start_exact`, never the default-input fallback). Drift:
  each channel is timestamped by its own samples from the session start
  (first chunk at `now − len`), its lag behind the wall clock is smoothed
  as the min over 2 s, and a channel more than 200 ms behind the other gets
  that much silence (audio is never dropped) plus an `engine-warning`. A
  device lost (`DeviceNotAvailable`) or silent 5 s ends its channel with a
  warning; both lost = the run ends normally and keeps the item; neither
  ever delivering = error. It shares the mic slot in `Sessions` (never
  next to a mic session); `engine_status.system_session`, RunKind
  `system` in the UI. Native loopback without a virtual device is #140.
- **Native system audio (0.10 step 2, #140)** (`sources/loopback/`):
  "This computer's sound (built-in)", preselected when available
  (`list_system_audio_devices.native` = probe + reason;
  `engine_start_system { native: true }`), the step-1 device picker is the
  fallback with the reason shown. **Windows**: WASAPI loopback through
  cpal 0.16 as is (input stream on the default *output* device →
  `AUDCLNT_STREAMFLAGS_LOOPBACK`, `Recorder::start_output_loopback`) — no
  `windows` crate code, cpal stays pinned; untested on hardware (no Windows
  cargo check in CI: whisper's Vulkan build needs the SDK), the code is
  portable cpal so every build type-checks it. **macOS 14.2+**: a global
  process tap excluding our own process + private aggregate device clocked
  by the default output + IOProc (`objc2-core-audio`, already in the
  tree); `CATapDescription`/`AudioHardwareCreateProcessTap` are looked up
  at run time (`AnyClass::get`/`dlsym`) so the minimum stays 11.0;
  `NSAudioCaptureUsageDescription` in `Info.plist`. A denied/undetermined
  permission gives digital silence, not an error → one hint after 20 s of
  exact zeros. Verified on macOS 27.0 (2026-09-25): tap + aggregate + IO
  run (48 kHz, continuous cycles, correct 16 kHz count) but only silence
  from the unbundled test binary (TCC AudioCapture undetermined) — real
  audio still needs a check in a bundled build. **Linux**: default sink
  (`pactl get-default-sink`, else `pactl info` in C locale) →
  `<sink>.monitor` from `pactl list short sources` → `parec` 16 kHz mono
  f32 on stdout (`pulseaudio-utils`; works on PipeWire via
  pipewire-pulse); cpal/ALSA can't open monitors by name. Captures that
  deliver *nothing* while nothing plays (WASAPI, parec) are `gapless`:
  `ChannelSync` fills silence after 1 s idle and places resumed audio at
  the wall clock (never "lost"/"drifted" for being quiet). The tap
  delivers zeros continuously, so it keeps the stall check. No AEC — the
  tab suggests headphones.
- **People registry (0.9, #132)** (`archive/people.rs`): lives in the
  archive at `<archive>/.sussurro/people.json` so it travels with it; not
  tied to meetings (transcriptions have participants too). Names
  match case-, accent- and whitespace-insensitively on name or alias; a name
  matching two people links to nobody. Linking only *adds* an email, and on
  save only to participants new to the item (a removed email doesn't come
  back; the chip offers "link" instead). Deleting/editing a person never
  edits items. It holds other people's emails: never logged, never in
  diagnostics, and in the portable config export only when the user ticks
  "Include People" for that export. An unreadable file reads as empty for
  linking but is never overwritten.
- **Extension pairing (0.9, #127)**: the pairing code is
  `sussurro:<port>:<token>`, defined once in `sussurro/src/lib/pairingCode.ts`
  and imported by the extension as `@sussurro/pairing`. The extension keeps
  it under `port` / `token` in **its own origin's IndexedDB** (#217:
  content scripts in meeting pages can read `storage.local` — Firefox has
  no `setAccessLevel`, Chrome restricts `local` only from 140 — but not the
  extension's IndexedDB), only through `extension/src/shared/pairing.ts`
  (also `PROTOCOL_VERSION`, `liveUrl`); a pairing left in `storage.local`
  by an older version is moved on the next read. Chrome ≥ 140 also gets
  `storage.local.setAccessLevel(TRUSTED_CONTEXTS)` at every background
  start. Content scripts must never import the pairing (a unit test
  checks).
  Settings → Browser extension never shows the token (Copy puts the code on
  the clipboard) and reads `local_api_status` (the API's settings apply at
  startup) to say when a restart is needed.
- **Extension capture (0.9, #128)** (`extension/src/{content,background}`):
  capture starts only on Start in the side panel (REC badge on the tab);
  before that the MAIN-world hook only observes. Channels: mic = tracks the
  page *sends* (else its `getUserMedia` track), remote = every received
  track mixed; one worklet emits both channels together (silence when a
  channel has no track) so `seq` is gap-free. The background renumbers `seq`
  per connection; a reconnect is a new `start` (= a new item). Fallbacks:
  media elements only when the page made no peer connection (own
  `getUserMedia` only then — a possible second prompt); Chrome-only
  `tabCapture` via an offscreen document if no remote audio 4 s after
  Start — the only reason Chrome has the `tabCapture` + `offscreen`
  permissions. Audio to the background: Chrome manifest
  `"message_serialization": "structured_clone"` (Chrome ≥ 148), negotiated
  with a probe, base64 fallback. Firefox manifest keeps its own
  `content_security_policy` (the MV3 default upgrades `ws://127.0.0.1`).
  `extension/e2e/` (Playwright, runs in CI) proves both channels in
  Chromium + Firefox against a local call; real platforms stay manual (#184).
  **Hosts and frames (#287)**: Teams is matched on `teams.microsoft.com`,
  `teams.live.com` and `teams.cloud.microsoft` (org tenants from
  2026-09-30); Meet/Teams scripts run top-frame only, Zoom's
  (`*.zoom.us/wc/*`) with `all_frames` because its meeting is a same-origin
  `/wc/` iframe (no about:blank/origin fallback). The background arms
  **one frame per tab** (`background/frames.ts`: peer connection first,
  a higher tier replaces the pick) and drops other frames' audio; only the
  top frame answers `page:info`. `MEETING_MATCHES` = `TOP_FRAME_MATCHES` +
  `ALL_FRAMES_MATCHES` in `shared/platform.ts`, pinned by the manifest test.
- **Extension side panel (0.9, #129)** (`extension/src/sidepanel/`,
  `extension/src/shared/live.ts`): a live mirror only (E3). The background
  keeps each tab's transcript (pure reducer over the app's `/live`
  `segment`/`speaker`/`status`); the panel takes a snapshot and applies the
  numbered `panel:live` changes (epoch + rev; a gap → new snapshot). A
  reconnect's new item is a new part under a "connection lost" note; the
  buttons act on the newest item; a new Start clears it. Chip labels and
  colours mirror `speakers/doc.rs` (a test compares them); the mic channel
  without a speaker id shows "You". "Create .srt" appears only with the
  subtitles setting `on_request`, which `GET /app/version` now reports
  (`subtitles`, additive), and downloads via a Blob link (no `downloads`
  permission). The background must never import page code (React,
  `@sussurro/transcript`): Chrome's service worker has no DOM, and the
  extension build fails if `background.js` uses `document`.
- **Meeting language (#288)** (`stt/languages.rs`,
  `extension/src/shared/language.ts`): the side panel's Language menu
  (Auto-detect + the active engine's languages by native name from the
  token route `GET /app/languages`: Whisper 99, `*.en` model `en`,
  Parakeet 25, Qwen3-ASR 29 codes) sends `start.language` — additive, the
  protocol stays 2. The app makes it the run's `RunOptions.language` (STT
  hint, frontmatter `language`, #218 cleanup fillers); missing = dictation
  setting; a code the engine doesn't offer falls back to it with a
  `warning` (never refuses the meeting). The extension remembers the choice
  per platform in `storage.local` `meetingLanguage` (not a secret), first
  time = the app's dictation language; greyed while recording, shown in
  the recording header; an app without the route (404) gets no selector.
- **Page names: Meet (0.9, #131, P8), Teams and Zoom web (0.11, #245,
  #246, desk study #239)** (`speakers/names.rs`,
  `extension/src/content/names/` = one observer engine,
  `content/{meet,teams,zoom}/` = per-platform profile + selector sets +
  fixtures, `content/profiles.ts`): layer 1 mic = "You", layer 2 names from
  the page on all three, layer 3 Voice N. The page's identity is an RTP
  source polled at 10 Hz: the **CSRC** on Meet and Teams
  (`getContributingSources()`; Meet excludes mirror CSRCs, Teams turns the
  mirror rule off — its redundant track repeats the mix's CSRCs — and
  hangs 800 ms), the **SSRC** per participant stream on Zoom's WebRTC
  mode (`getSynchronizationSources()`, ids `ssrc:<n>`); no receivers
  (Zoom WASM mode) = the lit tiles become a `dom` timeline. Names come
  from tiles through **versioned data-only selector sets** (attributes/
  structure, never obfuscated classes or label text; the shipped
  `meet-2026-09a`, `teams-2026-09a`, `zoom-2026-09a` are **unverified
  until #184** — the Teams/Zoom values are placeholders of the shapes the
  desk study names, replace them only from our own inspection, with a
  synthetic fixture) and are bound source → name by voting (5 votes,
  margin 3, 1:1, drop after 10 contradictions). **Teams/Zoom vote
  lag-aware** (their outline trails the audio ~1 s): only inside a turn
  (source alone ≥ 1 s, one tile lit ≥ 300/500 ms and lit after the turn
  began) and only after 2 turns or one ≥ 3 s; they learn the user's tile
  from our mic when the self marker is missing (sticky); Zoom has a
  screen-share `pause` hook; Teams a coverage check (tiles without the
  outline = the UI variant without it). The name guard strips platform
  qualifiers ("(Guest)", "(Host, me)", localized), rejects "Unknown
  user" and names cut with "…". Health cross-checks against the audio; a
  broken hook sends `names_unavailable`, never guesses. **Speaker ids by
  platform**: `meet:<name>` (also `other` and every item from before
  0.11), `teams:<name>`, `zoom:<name>` (`doc::page_name_prefix`,
  `SharedNames::for_platform`); every page-name helper accepts all three.
  **Protocol
  2** (additive; `/app/version` reports `protocol_min: 1`, the extension
  accepts `protocol_min..=protocol`): `speaker_active {t, id?, name?,
  source?}`, `speaker_idle`, `speaker_name` (last binding applies to the
  whole call), `observer_health`; `t` = ms on the connection's audio clock
  (page frames mapped by the background). App: a remote line takes the
  name covering ≥ 50 % of it and 1.5× the runner-up, after lag
  compensation (dom 400 ms — 1 s on Teams/Zoom, `AttributionParams::
  for_platform` — caption 1.5 s, rtp 0) and 250 ms edge trim —
  constants in `AttributionParams`, to re-tune from #184's measurements;
  else Voice N. An end-of-run pass re-attributes with every event;
  page-name speakers (any prefix) feed the People link suggestion (by
  label); page participants join the frontmatter with People emails.
  Captions fallback not built (follow-up); the protocol stays 2.
- **Recording notice (0.9, #136)**: before recording other people (New →
  System audio + mic, Microphone → Meeting in the room, the extension's
  side-panel Start) a notice says others may need to be told and may have
  to agree, depending on local rules — neutral, no legal advice, no named
  jurisdictions. One wording in `sussurro/src/lib/recordingNotice.ts`
  (pure; the extension imports it as `@sussurro/notice`). It never blocks:
  *Start recording* proceeds, Cancel starts nothing. *Don't show this
  again* (ticked by default) is stored per install: app
  `Settings.meeting_notice_seen` (reset from Settings → Browser extension;
  a cleared settings file shows it again), extension `storage.local`
  `recordingNoticeSeen` (reset from the options page; not touched by
  Forget pairing). A "Recording other people" line shows on those tabs and
  while such a run records. README → Privacy → *Recording meetings and
  consent* is the notice's link target — keep the anchor stable.
- **Extension browsers (0.9, #137)**: one Chrome build for Chrome, Edge
  and Brave, one Firefox build (desktop ≥ 140 since #234 — the first
  release that reads `data_collection_permissions`; no `gecko_android`,
  Firefox for Android is not a target); feature parity except the
  Chrome-only tab-capture fallback, which is compiled out of the Firefox
  build (the build fails if `background.js` calls tabCapture/offscreen).
  Firefox's MV3 background is an event page (no persistent background)
  that unloads after 30 s without extension events or API calls — its
  WebSocket doesn't count — so from Start until the app's `done` the
  background calls `runtime.getPlatformInfo()` every second
  (`holdsBackground`); nothing keeps it loaded otherwise. The harness
  proves it: Firefox runs with a 2 s idle timeout and its own DevTools
  attachment ignored (set through the parent process over RDP), the fake
  app is silent 5 s after Stop, and suspensions are counted; it also opens
  the real Firefox sidebar. `edge` runs in CI (the runner's Edge,
  `msedge` channel); `brave` only when named locally. `web-ext lint`: 0
  errors, 4 warnings justified in `extension/README.md`.
- **Library facets (0.9, #135)** (`archive/facets.rs`, `archive_facets`):
  not tied to meetings. OR within a facet, AND across facets and
  with the text query; counts are disjunctive (a facet ignores its own
  selection). Tags/categories group case-insensitively (accents kept);
  participants group by People person (same rule as the People screen's
  counts), else by normalized name — the key is stored in the index and
  recomputed when `people.json` changes. Date buckets are cumulative
  (today / week from Monday / month / year / older) on the item's
  frontmatter day vs. the viewer's local today sent by the UI; the date
  facet takes one bucket or one custom range. Index schema v2 (rebuilt
  automatically). Selection is kept in localStorage (`libraryFacets`).
- **Saved audio (0.10, #141, P9)** (`archive/audio.rs`,
  `engine/audio_out.rs`): only on request — New's "Save audio" per run
  (`RunOptions.save_audio`), else `Settings.save_audio` (off; also covers
  extension meetings). **Mono 16-bit 16 kHz WAV per channel**, not stereo:
  channels arrive independently, so interleaving would need a growing
  alignment buffer; per-channel files also suit per-speaker replay (#142).
  Written as `audio-<channel>.wav` from the ingest thread, padded to the
  run's t = 0 (segment `start_ms` = file position); a lone channel is
  renamed `audio.wav` at the end. Headers patched every 10 s and on stop;
  startup recovery (#153, `live::mark_interrupted`) repairs them from the
  file length. Cap = 32-bit WAV limit (~37 h per channel), then that
  channel stops being saved — no RF64. Frontmatter `audio: [...]` is
  app-owned (like `status:`); the UI/delete use the files present that
  match `audio(-[a-z]+)?.wav`, never names from the frontmatter.
  "Delete audio" and a cancelled/speechless run with audio go to the OS
  trash. Toggle off = no `AudioOut` at all (tests assert no `.wav`).
- **Saved audio as Ogg Opus (0.11, #247, P16/E15)** (`archive/opus.rs`):
  `Settings.saved_audio_format` (`wav` | `opus`) picks the format of every
  run's channel files (`AudioWriter` over `WavWriter`/`OpusWriter`; same
  session clock, silence padding and ~37 h duration cap `MAX_SAMPLES`);
  names `audio(-<channel>)?.(wav|opus)`, frontmatter `audio:` lists them
  (the extension is the format record); items saved earlier are never
  touched. libopus 1.6 via `opus` 0.4 + `opusic-sys` (static, CMake) and the
  `ogg` 0.9 muxer: 16 kHz mono VOIP, 24 kb/s VBR, complexity 10, 20 ms
  packets, one page per second flushed (fsync every 10 pages), pre-skip =
  lookahead in 48 kHz samples and end trimming on the last granule (decoded
  length and positions exact); generated speech uses the same writer at
  24 kHz / 32 kb/s (#309, see #256). Startup recovery (`repair_any`) cuts a crashed `.opus`
  after its last whole page and sets EOS (empty stream if the headers were
  cut); foreign files untouched. Windows MSVC link is checked on every PR
  by the `opus-windows` job (`cargo test -p opus`).
- **Opus playback, decode and Compress audio (0.11, #248, P16/E15)**:
  `AudioFormat`'s default is **Opus for new installs**; a settings file
  without `saved_audio_format` (an existing user) is pinned to WAV and saved
  once (`Settings::load_migrating`). **The WebView never sees Ogg Opus**:
  the `sussurro-audio:` scheme serves an `.opus` file as a *virtual* 16-bit
  WAV (exact decoded length) through `opus::OpusReader` — page index from
  the headers, seeks restart 200 ms early with the decoder reset,
  consecutive reads bit-exact; readers cached per file (4 entries, file
  closed between requests — Windows can't move a folder with an open file,
  keyed by size + mtime). A request **without `Range`** gets the whole
  resource (200) up to `MAX_WHOLE` = 128 MiB, else 413 — never a 206 (the
  old 1 MiB cut, WAV too); Tauri's scheme API takes whole bodies only, so no
  streaming. `FileStream` decodes Ogg Opus via the same reader (symphonia has
  no Opus decoder), and *Identify voices* falls back to the item's saved
  file-channel audio (`audio.<ext>`/`audio-file.<ext>`, WAV first) when the
  original is gone — links included (`VoiceSource.saved_audio`). *Compress
  audio* (`archive/compress.rs`; audio bar per item, Settings → Archive →
  Compress all): encode to `.sussurro/compress-*.part`, decode it through
  (exact length, no undecodable sample), then under the archive lock (not
  recording, WAV unchanged) rename in place and WAV → OS trash (trash failure
  removes the copy); one job at a time, cancel between 64 KiB blocks.
- **Speaker-aware recipes and Ask (0.10, #143)** (`recipes/`): built-ins
  *Meeting minutes* and *Who said what* are `speakers_only` — offered and
  run only on meetings/transcriptions whose transcript names its speakers
  (never on notes, like the speaker panel). Map chunks are cut on speaker turns (an over-long line
  keeps its `[ts] Name:` prefix on every piece); map/merge/reduce prompts
  keep each statement with its speaker and never merge speakers; "Voice N"
  goes as-is with "never guess who they are". **Participants go to the
  LLM by name only**; emails only with the per-run "Include participant
  emails" tick (`include_emails`, never remembered), and an external
  consent token is bound to that choice (`RunTarget.emails`). Fixed corpus:
  `recipes/testdata/*.txt` (synthetic IT/EN) + `#[ignore]`
  `live_meeting_recipes_on_ollama` (structural checks).
- **llama-server sidecar packaging (Track E, #116)**: one pinned llama.cpp
  release for all targets in `sussurro/src-tauri/sidecar/llama-server.lock.json`
  (asset URL, SHA-256, size, allowlisted libs, licence); `npm run sidecar`
  fetches + verifies fail-closed into gitignored `src-tauri/binaries/`.
  `externalBin`/`resources` live **only** in `tauri.sidecar.conf.json`,
  merged with `--config` by release builds — never move them into
  `tauri.conf.json`: tauri-build checks `externalBin` at compile time, so
  every `cargo test`/clippy would need the download. Named
  `sussurro-llama-server` (no clash with a distro `/usr/bin/llama-server`);
  upstream files ship unmodified, libs in the `llama-server-libs` resource
  folder, spawned with that folder as cwd + on
  `DYLD_LIBRARY_PATH`/`PATH`/`LD_LIBRARY_PATH` (ggml finds its backends in
  the exe dir or cwd). Linux AppImage builds need that folder on
  `LD_LIBRARY_PATH` (linuxdeploy's `ldd`). `scripts/verify-sidecar-bundle.sh`
  checks every release bundle.
- **Qwen3-ASR engine (Track E, #117)** (`stt/remote.rs`): `SttEngine::Qwen3Asr`
  = **1.7B Q8 only** (the #152 gate; 0.6B failed on Italian), optional and
  **never the default** — no dictionary prompt, and whisper large-v3-turbo
  is more accurate on Italian (#109); the Models screen says so. Model +
  `mmproj` from `ggml-org/Qwen3-ASR-1.7B-GGUF` via the HF-tree SHA-256
  downloader. `AnyTranscriber::Remote` owns the `llama-server` process, so
  the idle unload / engine change / Drop stop it; `kill_all()` runs on the
  exit event and from an `ExitGuard` in Tauri's resource table (dropped by
  `cleanup_before_exit`, which also covers `restart()` and the updater's
  Windows install). Crash of Sussurro: Windows kill-on-close job object,
  Linux `PR_SET_PDEATHSIG` (spawned from one long-lived thread — the signal
  follows the spawning *thread*), macOS nothing (documented). Spawned
  without a shell, `-ngl 99 -c 4096 -np 1 --cache-ram 0 --no-webui`
  (#109's settings) `--no-slots`. **Who can reach it (#216,
  `stt/remote/endpoint.rs`)**: macOS/Linux `--host <dir>/llama.sock`, a
  Unix socket in a fresh 0700 per-run folder under `<app data>/sidecar/`
  (the temp dir if the path exceeds 100 bytes; removed on stop, dead runs
  swept) — never a TCP port there; Windows `--host 127.0.0.1` on a random
  port and, after `/health`, the listener's owning PID
  (`GetExtendedTcpTable`, windows-sys IpHelper) must be the child's. Every
  spawn gets a random key in `LLAMA_API_KEY` (env, not `--api-key`: argv
  is world-readable), sent as a bearer token; inherited `LLAMA_*` vars are
  dropped. `/health` stays public, so the key does not authenticate the
  server — the socket/owner check does. Requests ≤ 30 s: longer
  hotkey dictations are cut with `stt::pauses` (#194) into ≤ 28 s pieces.
  Output: text after the last `<asr_text>`, `<|…|>` tokens removed,
  language name → ISO code. The app never strips macOS quarantine (manual
  `xattr -cr`, documented). Its tests re-run the test binary as a fake
  server: with a **shared `CARGO_TARGET_DIR` another worktree's build can
  replace that binary mid-run** — verify in a private target dir if they
  fail with "0 passed; N filtered out".
- **Local (bundled) LLM profile (Track E, #118)** (`llm/bundled.rs`): a
  built-in profile (`bundled: true`, id `bundled` reserved) served by the
  same pinned `llama-server` with **Qwen3 1.7B Q8_0** from
  `ggml-org/Qwen3-1.7B-GGUF` (Apache-2.0, 2.17 GB; HF-tree SHA-256 **and** a
  pinned digest, fail-closed), `--reasoning off`, `-c 8192` (= the
  profile's `context_tokens`), `--alias qwen3-1.7b`. Q8 over Q4_K_M: Q4
  kept more Italian fillers/repeats on our samples. **Two processes, not
  one shared server**: the pinned build's router mode spawns one child per
  model anyway (same RAM ~1.9 GB footprint for both, same start-up), and
  would add grandchildren PDEATHSIG doesn't cover plus one lifecycle for
  two idle rules — measurements in the module docs. Same `Sidecar` code
  (`SidecarRole::Chat`): private socket / owner-checked port + per-spawn
  key (#216; `with_server` hands out a `SidecarAccess`), no shell, crash
  restart + one retry,
  `kill_all`/exit guard; idle stop after 15 min on the setup idle thread;
  a dictation on this profile pre-starts it. Builds with the sidecar add
  the profile at startup and in `set_settings` but **never select it**;
  only `bundled_llm_use` (the user's *Use the bundled model* click, offered
  when the cleanup server is unreachable — in the onboarding's cleanup step
  once every local-server probe came back empty (`bundledSetupState`),
  Settings → Cleanup and the setup banner) does. `normalize` pins its fields
  (never external), keeps one, and moves a user profile off the reserved id
  without moving cleanup. No download at cleanup time: a missing file =
  raw text + a Download prompt. Live check: `live_bundled_llm_*`
  (`#[ignore]`, env vars in the test).
- **Parakeet long input (#194)** (`stt/pauses.rs`): transcribe-rs 0.3.11's
  Parakeet greedy decoder can emit only blanks after a sentence end + pause
  (the decoder state blocks; the encoder output is fine), dropping every
  later sentence — English mostly, pauses of any length. transcribe-rs has
  no knob for it, so `ParakeetTranscriber` cuts input at pauses (~8 s
  pieces, energy relative to the buffer) and re-decodes a piece's rest when
  ≥ 1 s of speech follows its last word; engine segments on Parakeet also
  end at the first 300 ms VAD pause after 8 s
  (`SegmenterParams::for_engine`). Whisper is untouched. Re-check (and
  maybe drop) this when transcribe-rs changes its decode loop.
- **Per-speaker replay (0.10, #142)** (`archive/playback.rs`,
  `src/lib/replay.ts`, `src/lib/replayPlayer.ts`, `shell/AudioTab.tsx`):
  the document pane's *Audio* tab plays saved audio only (#141) — there is
  no temporary audio cache (runs never buffer audio to disk unless Save
  audio is on), so an item without saved audio gets an explanation
  pointing to Save audio; while recording the player waits for the end.
  Audio reaches `<audio>` through the custom scheme `sussurro-audio:`
  (`convertFileSrc("<id>/<file>", "sussurro-audio")`), **not** Tauri's
  asset protocol (which would need the whole archive in its scope): the URL
  names an item id + an audio file name, resolved through the archive's id
  validation/confinement, regular files only (no links), GET/HEAD with
  `Range`, 1 MiB chunks, main window only. The CSP gained only
  `media-src sussurro-audio: http://sussurro-audio.localhost`.
  "Play only: X" plays X's lines back to back (lines ≤ 250 ms apart
  joined), each from **its own channel's file** (no crosstalk);
  "All speakers" plays every channel file together, kept within 150 ms
  (the browser mixes them — no Web Audio, which would mute cross-origin
  media without CORS). The seek bar is the playlist's timeline. Word
  highlight only when the displayed text has as many words as the timed
  raw words (else the line is lit as a whole). The clock is a 40 ms timer,
  not rAF (frames stop in a hidden window and the audio would run into
  other speakers' lines).
- **Voice map (0.10, #144)** (`speakers/map.rs`, `archive_voice_map`,
  `src/lib/voiceMap.ts`, `shell/VoiceMapCard.tsx`): a card in the Speakers
  panel — hidden for notes, while recording, and with voice data on < 2
  lines. PCA on the L2-normalised per-line embeddings, computed in Rust
  with **no linear-algebra crate**: covariance + subspace iteration on two
  vectors + a 2 × 2 Rayleigh–Ritz step (plain power iteration with
  deflation stalls when λ1 ≈ λ2, e.g. three equidistant voices).
  Deterministic: fixed start vectors, sign = largest loading positive, so
  line order never mirrors the map. The command returns
  `{points: [{segment_id, speaker_id, x, y}], total, explained}` — never
  the embeddings; beyond 5,000 lines the points are downsampled per
  speaker in proportion, evenly over time (projection still uses all).
  The UI colours/labels dots from the item's *current* speakers (a moved
  line recolours without a refetch; refetch only on item id or
  `embedded_segments` change). Click/Enter selects the line: marked in
  the transcript (`TranscriptView.selectedId`), and the Audio tab's player
  seeks there (no autoplay). Keyboard: the SVG is one listbox, arrows walk
  lines in time order.
- **Calendar attendees from ICS (0.11, #252, P22/E21)** (`calendar/`):
  a hand-written strict iCalendar reader (no ICS crate) behind the
  `CalendarSource` trait; TZIDs via `chrono-tz` (IANA, also behind a
  `/mozilla.org/…` prefix), else the file's own `VTIMEZONE`, else common
  Windows names; floating/unknown zones and all-day dates at the meeting's
  own UTC offset (the item's `date`). RRULE expanded only near the day asked
  (DAILY…YEARLY, BYDAY ordinals, BYMONTHDAY, BYMONTH, BYSETPOS, WKST, COUNT,
  UNTIL; anything else → first date only + a note), with EXDATE, RDATE and
  RECURRENCE-ID overrides. Matching: events overlapping the recording ±
  15 min first, else that day's events. Attendee plan: *add*, *complete a
  missing email* (offered, never ticked by default — the user may have
  removed it), *listed* (never replaced); then the usual People linking;
  People entries only suggested. Rooms/resources dropped. The **private ICS
  link is a secret**: only in the OS credential store (account
  `calendar:ics-link`), no clear-text fallback (no store → import the file),
  never in settings/exports/logs; UI sees only its host. Fetched with the URL
  source's shared client (`direct::get_checked`/`fetch_bytes`: address
  rules on every hop, pinning, no proxy, 20 MB cap); local hosts always
  refused (no "allow local" for calendars). The `.ics` picker opens from
  Rust with the list import's file guards.
- **Voice profiles (0.11, #241, P12/P13/E13)** (`speakers/profiles.rs`
  pure, `speakers/voices.rs` files): opt-in per person, built **only from
  confirmed lines** — lines of document speakers the user linked to the
  person (`SpeakerEdit::Link`; a line moved away drops out by itself).
  **One pooled centroid per person** (duration-weighted mean of the
  L2-normalised line embeddings, 256-d only) + ms per condition (`close`:
  remote/system channel, or the mic of a `browser:`/`system` item;
  `room`: mic elsewhere, and `file` — unknown acoustics take the stricter
  rule) + document ids; no per-mic centroids (spike #235). Ready at
  **60 s from 2 documents** (P12; the spike found 30 s nearly as good —
  maintainer's call). `suggest()`: best ready profile only, cosine ≥ 0.65
  and margin ≥ 0.05 over the runner-up (none = 0), **0.75 when the voice
  is room-mic and the profile has room lines**; ties by person id.
  Storage: `<app data>/voices/<person id>.json` (dir 0700, file 0600,
  `write_private_atomic`), **the file's existence is the opt-in** (kept
  on this machine, not in `people.json`: the archive syncs, and a second
  machine turns it on again → rebuild from the archive). Ids outside
  `[A-Za-z0-9_-]{1,64}` get no profile. Enable = one full archive scan
  (`read_linked_segments` byte-checks `"person_id"` before parsing);
  speaker edits, Identify voices, a deleted line or item →
  `document_changed` in the background (re-reads the profile's documents
  + the changed one, so it equals a full rebuild). Forget one / off /
  person deleted or merged away / *Forget all voices* = real delete (not
  the trash). Vectors never reach the UI (`VoiceStatus` only), the API,
  exports, diagnostics or logs (`Debug` prints sizes). Commands:
  `voices_status`, `voice_status`, `voice_set_enabled`, `voices_rebuild`,
  `voice_forget`, `voices_forget_all`. Overlapped lines (#244) are
  excluded.
- **Voice suggestions (0.11, #242, P12)** (`speakers/suggestions.rs`,
  `src/lib/voiceSuggestions.ts`, `shell/VoiceSuggestionChip.tsx`):
  `voice_suggestions(id)` returns `{speaker_id, person_id}` only (no score)
  for unlinked **`voice:N`** speakers (never "You" — the mic channel or a
  voice with `own_voice: true` from #243 — nor page names `meet:`/`teams:`/
  `zoom:`) of a
  non-recording item, from ready profiles of people still in People; a
  dismissed best match gives **no** suggestion (never the runner-up). The
  panel chip *Sounds like X · Link · Not X*: Link = the normal
  `archive_link_speaker` path; *Not X* (`voice_suggestion_dismiss`) is kept
  per item + voice + person in `<app data>/voice_dismissals.json` (0600,
  ids only, never in the archive), dropped on item delete and by *Forget
  all voices*. The UI refetches on any change of speakers/links, voice data
  or People (so after runs, Re-detect, and for items opened later).
  `Settings.voice_suggestions` (default on) hides all suggestions and keeps
  the profiles. People → person: *Recognise this voice* (turning on shows a
  first-use sheet — what, where, delete, tell the person), status line,
  *Forget this voice*. **Settings → Voices is one section** holding #243's
  *Your voice* card and the *Known voices* card (the switch, *Forget all
  voices* with confirmation — it deletes your own voice too, and the *Your
  voice* card is reloaded — and the GDPR note). Not built: stripping
  per-line embeddings from items on request (optional in the issue).
- **Own voice "You" (0.11, #243, P14)** (`speakers/own_voice.rs`,
  `shell/OwnVoiceDialog.tsx`, Settings → Voices): optional read-aloud
  enrolment (IT/EN paragraph, ~30 s, own `Recorder` on the picked mic; the
  audio is never saved): quiet frames dropped, ≤ 3 s windows embedded, one
  centroid; needs ≥ 20 s of speech, uses ≤ 90 s. File
  `<app data>/voices/you.own-voice.json` (0600, same lock and folder as
  #241; the dot keeps it out of person-id scans; *Forget all voices*
  deletes it too). Never a People entry. **Labelling** only in
  single-channel documents (not `browser:`, no `you` line, no mic line in a
  `system` item): the best-matching "Voice N" with cosine ≥ **0.45** (spike
  #235, voice level, never per line) becomes label "You" + `YOU_COLOR`,
  marked `DocSpeaker.own_voice = true` (lines keep their `voice:N` id). Runs
  at the end of a run (tracker), after Identify voices and Re-detect, when
  *Label my voice as You* is on (default on after enrolment, kept on
  re-enrol); `own_voice_find` on request. Never over a user label: a voice
  renamed/linked is skipped (no fallback to the runner-up); renaming or
  linking an automatic "You" sets `own_voice = false` = never automatic
  again in that document; a "You" that stops being the best match goes
  back to "Voice N". The centroid is also the 0.13 own-voice cloning
  reference (not built).
- **Overlapping speech (0.11, #244, E19 reshaped by spike #237)**
  (`speakers/overlap.rs` pure, `speakers/segmentation.rs` model):
  `onnx-community/pyannote-segmentation-3.0` **fp32** (`onnx/model.onnx`,
  revision `733a93b6473d019a773298e08cefa686894b1854`, SHA-256
  `057ee564…91ea25`, MIT/CNRS — MIT text in `licenses.json`'s downloaded
  models) through the app's `ort`, downloaded on first use into the models
  folder, fail-closed (`model::ensure_pinned`), 2 intra-op threads.
  **Detection runs when lines are labelled** (the tracker: every line of a
  clustered channel that got voice data, at the end of a run and in
  Identify voices — `SpeakerModels { embedder, overlap }`, tests pass an
  embedder alone = no overlap), never in Re-detect (no audio): ≤ 10 s
  zero-padded windows, P(overlap) = sum of the 3 pair classes, frames
  ≥ 0.5; a line with ≥ 300 ms **and** ≥ 10 % stores its spans in
  `Segment.overlap: [{start_ms, end_ms, speaker_id?}]` (session clock,
  additive, omitted when empty — a non-empty list is the flag).
  **Second speaker** per span = nearest line of the same channel (≤ 60 s,
  ties to the earlier) whose speaker differs from the line's
  (`assign_second_speakers`): at the end of a run, in Identify voices, in
  Re-detect and after **every** `modify_segments_with` edit. Overlapped
  lines **stay in the clustering** (exclusion gained nothing and lost a
  quiet speaker) and are **never split** (P2); they are left out of voice
  profiles (`person_lines`) and, when a voice has other lines, of
  `document_voice` (suggestions, "You"). UI: a quiet "+ Voice 3 also
  speaking" next to the chip (`toLines(..).alsoSpeaking`, only with a
  speaker list). Transcript.md, exports, SRT/VTT and the archive API keep
  one speaker per line. Thresholds are AMI English — re-check on Italian.
- **TTS text preparation (0.12, #254)** (`tts/text/`, pure, no engine,
  no dependency added): `prepare(markdown, Lang, &PrepOptions) ->
  Vec<Chunk{index, text, pause_after}>` is the only entry point #255/#256
  need. Markdown → blocks (frontmatter, code, link targets dropped;
  headings get a `Section` pause; tables read "Header: cell" row by row or
  skipped; transcript lines read "Anna: text", the name only when the
  speaker changes, or without it; timestamps never), then per-language
  normalisation for `it`/`en` only (any other language: markup and URLs
  cleaned, numbers left as written), then greedy sentence packing.
  Default chunk **160 characters** (`DEFAULT_MAX_CHUNK_CHARS`, min 40):
  Pocket TTS conditions on ≤ 50 tokens of a 4,000-piece SentencePiece
  vocabulary (~3.2–3.5 chars/token), so a chunk stays one Pocket
  generation; #255 should still count tokens. English numbers use the US
  form without "and", years as years (`eighteen sixty-one`), `a.m.`/`p.m.`
  as "ay em"/"pee em", ambiguous `3/4/2026` as month-first; Italian
  "1" before a noun stays "uno" (no gender guess). Fixtures: the #236
  bake-off text set in `tts/testdata/` — re-run the English listening
  test (P18) on this module's output.
- **Read-aloud engine (0.12, #255, P18/P24/E16/E22)** (`tts/`): Pocket
  TTS through the app's `ort` — never Python, sherpa-onnx or a GPL
  phonemizer (the tokenizer is a hand-written SentencePiece unigram reader,
  checked id-for-id against `sentencepiece` in the live test). Italian
  **24-layer** + English models, **fp32 only** (int8 diverges), the four
  speaking graphs of `KevinAHM/pocket-tts-onnx` @`58a6d00c` — **never
  `mimi_encoder.onnx`** (cloning); voice states from
  `kyutai/pocket-tts-without-voice-cloning` @`e81d79e8`, the revision whose
  tokenizer is byte-identical to the export's (mixing revisions = EOS at
  step 1). Every file pinned by size + SHA-256 in `tts/catalog.rs`
  (fail-closed, `.part` removed on failure/cancel); only voices whose
  recordings allow commercial use (CC0 / CC BY 4.0: no `jean`/`cosette`).
  Layout `<models>/pocket-tts/<bundle>/…` + `voices/<id>.safetensors`.
  **P24**: `Settings.tts_enabled` off by default (Settings → Experimental);
  `tts_download`/`tts_preview` refuse while off; downloads only from the
  user's click in Models → Voices after an inline confirmation showing size
  + licence; turning it off cancels a download, unloads the engine, clears
  previews and the UI offers `tts_delete` of everything. `tts_voices` =
  voice per language (unknown picks dropped by `normalize`). One engine
  loaded at a time (`tts::service`), unloaded after 5 min idle on the
  shared idle thread. Previews are temporary WAVs (`<app data>/tts-preview/`,
  swept at startup) served by `sussurro-audio:` under `tts-preview/` — no
  CSP change. WAVs carry a `LIST/INFO` "synthetic speech" comment; the
  watermark is #257 (E17). Generation per #254 chunk, re-cut at 50 tokens; voice
  and decoder state restart per piece; temperature 0.7, fixed seed (same
  text = same audio). Live test (`tts::live_tests`, env vars in the file)
  on the M1 Pro at 2 threads: Italian RTF ≈ 1.2–1.4, English ≈ 0.4; Whisper
  heard 91 % / 86 % of the words. `licenses.json` has the CC-BY entry
  (`scripts/gen-licenses.mjs`); regenerate with a real `npm ci` install (a
  symlinked `node_modules` makes `npm ls` list dev deps).
- **Read aloud of an item (0.12, #256, P17/P21/P24)** (`tts/read_aloud.rs`,
  `archive/speech.rs`, `tts/marking.rs`, `shell/ReadAloudSection.tsx`):
  transcript or a companion document → `tts::text::prepare` → Pocket with
  the language's picked voice (item `language:`, or one the user picks) →
  Ogg Opus **at Pocket's own 24 kHz** (#309: `opus::SPEECH`,
  `OpusWriter::create_with`; recorded audio stays 16 kHz `RECORDED`) at
  **32 kb/s** — at 24 kHz libopus codes hybrid (SILK 0–8 kHz, CELT
  8–12 kHz) and its rate table gives SILK 18 of 24 kb/s but 22 of 32, about
  what the 16 kHz core had; measured on a synthetic bright voice, 24 kb/s
  kept 89 % of the 8.5–11.5 kHz energy, 32 kb/s 100 %; ~14.4 MB/h. The
  `OpusHead` input rate says which: `OpusReader::open_native` decodes at the
  file's rate (`native_rate`: an Opus rate, else 48 kHz) and the
  `sussurro-audio:` virtual WAV header carries it (`audio::header_at`), so
  speech saved before #309 (16 kHz, 24 kb/s) still plays; `OpusReader::open`
  stays 16 kHz for STT/Identify voices; `opus::verify` counts native
  samples; `repair` accepts both rates. An engine at another rate goes
  through the windowed-sinc `tts/resample.rs` to 24 kHz (Pocket: pass-
  through). Previews were already 24 kHz WAVs. **Save** =
  `speech.opus` / `speech-<slug>.opus` (clean companion stem, else slug +
  8 hex of its SHA-256), written to `.sussurro/<file>.part` and moved in
  under the archive lock (old file → OS trash); **Listen** =
  `<app data>/tts-preview/listen-N.opus`, deleted at the next Listen, when
  the document closes (`read_aloud_discard`), at startup and module off.
  Names `speech(-[a-z0-9-]+)?.(opus|wav)` never match `audio*` (Delete/
  Compress audio don't touch them); the scheme serves both
  (`is_playable_file_name`). Frontmatter app-owned `speech: [...]` +
  `synthetic: {<file>: {document, generator, engine, voice, language, date,
  text_sha256, marked}}` (per file, not the plan's single block);
  `update_meta` ignores both from the UI. **Marking (P21)**: every file
  goes through `tts::marking::Marker` — Opus comments `SYNTHETIC=1`,
  `DIGITAL_SOURCE_TYPE=…trainedAlgorithmicMedia`, engine, voice, language
  (`OpusWriter::create_with`, `opus::read_tags`); `Marker::process` is the
  single **watermark hook for #257** (no-op now, `marked: [metadata]`), the
  preview passes it too; it gets 24 kHz blocks, and #257's "M16" (E17)
  computes the mark on their 16 kHz resample and adds it upsampled. **Stale** = SHA-256 of the speakable text (not the
  frontmatter) ≠ the recorded one. One job at a time (`read_aloud::jobs()`),
  progress `read-aloud-progress`, cancel between chunks; refused while off,
  on a live item, or with the model/voice missing (points to Models →
  Voices, never downloads); `tts_delete` refuses during a job; files list
  and delete work while the module is off. Commands `read_aloud_start`,
  `_cancel`, `_job`, `_files`, `_delete`, `_discard`. Live test
  `live_read_aloud_saves_a_marked_speech_file` (English M1 Pro, 2 threads:
  6.3 s of speech in 3.4 s).
- **Workspace only + onboarding (#115)**: the left-rail workspace is the
  only UI (the classic window and its preview flag are gone; the old
  settings key is ignored and dropped on save). The main window opens at
  1120×740, min 800×560 (`tauri.conf.json`). `Settings.onboarding`: `welcome` = fresh
  install (no settings file) → guided setup; `whats_new` = a settings file
  without the key (upgrade from 0.6.x) → one "What's new" screen; `done`.
  The archive step calls `archive_prepare` (creates + lists the folder) on
  purpose so macOS asks for Documents there, never when a recording
  starts; while the onboarding is open no screen behind it mounts. The
  cleanup step probes only local default ports (Ollama, LM Studio,
  llama.cpp-server).
- Workflow: **branch → PR → merge** — no direct pushes to `main`.
- **Product direction: speech-to-text workbench** (decided 2026-09-24).
  Full plan: `docs/superpowers/plans/2026-09-24-sussurro-speech-workbench.md`
  (decisions P1–P11 + engineering E1–E12 — read it before touching 0.7+).
  Short form: UX = proposal A (left-rail workspace, mock in
  `docs/superpowers/plans/ux-mocks/`); **command mode removed** (out of
  focus, OS voice control covers it — spoken editing commands inside
  dictation stay); cleaning is text-only (no audio cutting); archive of
  markdown items in `<Documents>/Sussurro` on every OS (frontmatter is the
  source of truth, index is derived); three item types by content —
  note / meeting / transcription (file default: note); participant emails
  from a manual People registry; LLM profiles speak only OpenAI-compatible
  or Ollama APIs; .srt is a setting (default on request) for meetings and
  transcriptions; 0.9 speakers = Meet names + generic "Voice N" only; WAV
  saved only on request; extra STT engines (Qwen3-ASR) run in a bundled
  `llama-server` sidecar, never in-process (two ggml copies can't share a
  binary); never add sherpa-onnx (second ONNX Runtime).
- **Meetings are on by default** (#138, 2026-09-25): the 0.9 preview flag
  `Settings.meetings_enabled` is gone (E12: removed in the release that
  turns the feature on — here the single final release). The extension
  routes of the local API exist whenever the API runs (token + extension
  Origin, loopback only; an unpaired app accepts nothing), New → System
  audio + mic and "Meeting in the room" always show (the #136 notice asks
  first), and meetings always get speaker labels. Settings → Browser
  extension = explanation + local API status + pairing, no switch. An old
  settings file with the key loads and drops it on save. A 404 from
  `/app/version` now tells the extension the app is too old
  (`app_outdated`). Don't reintroduce a meetings switch.

## Release process

- Push a `v*` tag (from any branch) to trigger `.github/workflows/release.yml`
  → draft release with signed installers + `latest.json`. Publish by
  un-drafting.
- Run the Release workflow manually on main to verify the three-OS build
  without publishing (`gh workflow run release.yml --ref main`, #210): off a
  tag it creates no tag and no release — the bundles (installers, updater
  archives + `.sig`) and extension zips become 3-day workflow artifacts; no
  `latest.json` is produced. Dispatching on a tag ref behaves like a tag push.
  A full run costs the same Actions minutes as a release.
- **Firefox extension signing (#228)**: on a tag, with the repo secrets
  `AMO_JWT_ISSUER` + `AMO_JWT_SECRET` (AMO API credentials), the release
  workflow signs the Firefox build on addons.mozilla.org, **unlisted
  channel** (`web-ext sign`, sources uploaded for the minified bundle), and
  attaches `sussurro-extension-firefox-<version>.xpi` to the draft.
  `extension/scripts/amo-sign-gate.sh` skips with a notice (never fails)
  off a tag, without the secrets or for a pre-release version; a build-only
  run never signs. Gecko id `sussurro@darumahq.it` is permanent (AMO ties
  the add-on to it); AMO signs a version number once. **Updates are
  self-hosted**: `gecko.update_url` = `https://fullo.github.io/Sussurro/extension/updates.json`
  (Pages serves `docs/` from `main`); after publishing a release the
  maintainer runs `cd extension && npm run update-manifest -- X.Y.Z` (fetches
  the published `.xpi`, writes version/link/`sha256` into
  `docs/extension/updates.json`) and merges it. `web-ext lint` runs with
  `--self-hosted` (listed-mode lint rejects `update_url`). Setup steps:
  `docs/releases.md`.
- Never force-move an existing release tag; bump the patch version instead
  (version lives in `sussurro/package.json`, `sussurro/src-tauri/tauri.conf.json`,
  `sussurro/src-tauri/Cargo.toml` + `Cargo.lock`).
- Updater signing key: outside the repo, uploaded as
  `TAURI_SIGNING_PRIVATE_KEY` secret (see README → Releases & auto-update).
  Changing the key pair orphans existing installs.
- **Manual release while Actions is down** (billing outage, ~until 2026-08):
  Windows is built/signed on the dev box, Linux in WSL, then
  `gh release create` with the assets + a hand-written `latest.json`.
  macOS assets are added from a Mac afterwards:
  ```bash
  git clone <repo> && cd Sussurro/sussurro && git checkout vX.Y.Z
  npm ci && npm run sidecar              # pinned llama-server (#116)
  npm run tauri build -- --config src-tauri/tauri.sidecar.conf.json   # Apple Silicon
  node_modules/.bin/tauri signer sign \
    --private-key-path <sussurro-updater.key> --password "" \
    src-tauri/target/release/bundle/macos/sussurro.app.tar.gz
  gh release upload vX.Y.Z src-tauri/target/release/bundle/macos/sussurro.app.tar.gz* \
    src-tauri/target/release/bundle/dmg/*.dmg
  ```
  then add the `darwin-aarch64` entry (URL + signature) to the release's
  `latest.json`. Beware: PowerShell drops empty `""` args and unsets
  empty env vars — sign via `cmd /c` on Windows (see scripts/ for helpers).

## CI gotchas (learned the hard way)

- **cdn.pyke.io (ort's prebuilt download) persistently 403s GitHub runners.**
  The test job links Microsoft's official ONNX Runtime release instead
  (`ORT_LIB_LOCATION` + `ORT_PREFER_DYNAMIC_LINK`); the release workflow
  still uses pyke's static binaries with an `ORT_CACHE_DIR` cache. If a
  release job fails with a 403 from cdn.pyke.io, re-run it; if it keeps
  failing, apply the MS-release fallback there too.
- Keep the pinned ONNX Runtime version in `test.yml` in sync with what the
  locked `ort-sys` expects (see the `ms@<version>` URLs in its
  `build/download/dist.txt`).
- Linux needs **glibc ≥ 2.38** (ubuntu-24.04 runners) — older glibc fails
  linking with `undefined symbol: __isoc23_strtoll`.
- macOS needs `minimumSystemVersion` ≥ 10.15 (whisper.cpp uses
  `std::filesystem`); it is set to 11.0 (arm64 baseline) in
  `tauri.conf.json`.
- **Actions minutes can run out** (private repo; macOS jobs bill 10x, Windows
  2x — one 3-OS release run burns ~200 min-equivalents). Symptom: every job
  fails in ~3 s with no steps and the annotation "job was not started …
  spending limit". Fix: Billing & plans on the owner account. Meanwhile
  `scripts/ci-local.sh` mirrors test.yml inside WSL2 Ubuntu 24.04
  (`wsl -d Ubuntu-dev -u root -- bash /mnt/f/GitHub/Sussurro/scripts/ci-local.sh
  <branch>`) — it validated PR #53 end-to-end (tests, clippy, E2E smoke).
  Releases still need GitHub runners (macOS/Windows can't be mirrored).

## Roadmap (agreed 2026-07-03; current version 0.10.1 — released 2026-09-26)

### 0.3.0 — working everywhere (gate: every platform compiled AND verified)

1. Runtime smoke test on real Windows with the CI-built installer (msi/exe):
   hotkey → recording → Vulkan GPU transcription → paste injection.
2. Runtime smoke test on real Linux (AppImage/deb on Ubuntu 24.04).
3. **Native Wayland injection** — the biggest functional gap: modern distros
   default to Wayland and injection there is fragile. Status: wtype/ydotool
   ladder shipped; the RemoteDesktop portal (ashpd, issue #40) **shipped as
   the primary backend in 0.3.9** behind the default-on `wayland-portal`
   feature. Issue #40 stays open until the reporter verifies at runtime on
   KDE Plasma 6 Wayland.

### 0.4.0 — quality & tech debt

4. Streaming typing with cleanup enabled — **done & verified** (sentence-by-
   sentence streaming with per-chunk LLM cleanup landed with the Wispr-parity
   batch; manually verified by dictation on Windows 0.3.9-3 on 2026-07-05,
   "experimental" label removed).
5. Unpin cpal 0.16 → 0.18 (retest the windows-core conflict with Tauri).
   *Rechecked 2026-07-05: crates.io max is still 0.18.1, the broken one.
   Re-try when cpal releases a version on windows-core ≥ 0.62.*
6. Move `ort` from 2.0.0-rc.12 to stable when released (coordinate with the
   pinned ONNX Runtime version in test.yml — see CI gotchas).
   *Rechecked 2026-07-05: still no stable (max = 2.0.0-rc.12);
   transcribe-rs 0.3.11 is also the latest.*
7. Optional Vulkan GPU build on Linux (feature flag; CPU-only today).
   *Validated in WSL 2026-07-05: `--features linux-vulkan` compiles, all 86
   tests pass, and with no usable Vulkan device ggml falls back to CPU
   cleanly (no crash) — ggml ignores CPU-type devices like llvmpipe, so WSL
   can't exercise the GPU-on path. Remaining before shipping it in releases:
   one run on real Linux hardware with a proper Vulkan driver (correctness +
   perf), then decide whether it becomes a separate release artifact or the
   default. Build recipe documented in docs/compile/linux.md.*
8. Minimal E2E smoke test in CI — **done** (test.yml runs the app under Xvfb
   and asserts the window exists; mirrored in scripts/ci-local.sh).

### 0.5.0 — go public

9. macOS Developer ID signing + notarization (ad-hoc today → Gatekeeper
   blocks public users). Consider Windows code signing for SmartScreen.
10. Make the repo public → auto-update unfreezes (see standing decisions).
11. **Flatpak** (deferred here from 0.3.10 on 2026-07-03): distribution-only,
    no app changes. A ready manifest + release-workflow job live on the
    closed PR #46 / branch `feat/flatpak` — reuse them, then submit to
    Flathub (needs the public repo). AppImage already ships in every release
    (it's the updater's Linux format) — nothing to add there.

### 0.6.x — shipped (current version 0.6.3)

12. **Backend-agnostic cleanup via the OpenAI-compatible API** — **shipped in
    0.6.0/0.6.1** (2026-07-07). `Settings.cleanup_api` (`Ollama` default |
    `Openai`) + `api_key`; `cleanup/ollama.rs` dispatches `chat()`/`list_models()`
    to Ollama native (`/api/chat`, `/api/tags`) or OpenAI-compatible
    (`/v1/chat/completions`, `/v1/models`), with `openai_base()` URL
    normalization. Any `/v1` server drives cleanup — llama.cpp-server, LM
    Studio, Ollama's own `/v1`, antirez's **DS4**. Additive; Ollama stays the
    default. Verified end-to-end against Ollama's `/v1`. How-to blog:
    `docs/blog/use-ds4.html`.
13. **Linux clipboard fix** — **shipped in 0.6.1** (2026-07-07). A transient
    `arboard` set doesn't persist on Linux (the selection owner drops when the
    `Clipboard` value drops), so Copy + paste were silent no-ops. Now clipboard
    writes go through `wl-copy` (Wayland) / `xclip`|`xsel` (X11), which fork a
    daemon holding the selection; `copy_text` shares `inject::set_clipboard`.
    Confirmed on X11 and (with `wtype` for the keystroke) Wayland/KDE Plasma by
    the issue-#40 reporter. Linux users now need a clipboard helper + a Wayland
    keystroke tool — documented in `docs/compile/linux.md` +
    `docs/blog/linux-injection.html`.
14. **Reuse installed models** — **shipped in 0.6.2** (2026-07-09), from a Mac
    user's onboarding feedback. Ollama: if the daemon has models but the
    configured one is absent, auto-adopt an installed model (prefer a small
    instruct) and tell the user, instead of forcing a specific download — the
    "pull a model" nag only fires when the server has zero models. Whisper: new
    `list_whisper_models` command scans the models dir for any `ggml-*.bin`; the
    picker surfaces them ("· installed") so pointing Settings → Models folder at
    a directory shared with other whisper.cpp tools reuses them (ggml only;
    OpenAI/MLX whisper files are not whisper.cpp-compatible).
15. **Security review batch + dictionary fix** — **shipped in 0.6.3**
    (2026-08-27), from the security review published as issues #89–#95
    (all closed, fixes merged to main): whisper model name validated
    (`[A-Za-z0-9._-]+`) + resolved path confined to the models dir (#89);
    downloads SHA-256-verified against the upstream-published digest
    (HuggingFace tree API `lfs.oid`), fail-closed, with connect/idle
    timeouts replacing `timeout(None)` (#90); strict CSP on the app windows
    — capabilities audited clean, devtools off in release (#91); warning in
    Settings when the cleanup endpoint is remote (#92); paste-injection
    sleeps audited per platform + residual races documented (#93); privacy
    disclosures (auto-paste without review, hotkey over password fields)
    in README + blog (#94); `Settings::save` no longer panics on
    serialization, local API security design documented (#95). Plus #88:
    Personal Dictionary textarea keeps its raw text so typed newlines are
    no longer swallowed (controlled-component round-trip bug).
16. **Memory optimization** — **shipped in 0.6.3** (2026-08-31, PR #96).
    The audio callback converts to 16 kHz mono incrementally
    (`StreamResampler`, output bit-identical to the batch path — proven by
    equivalence tests), so the raw device-rate capture is never buffered
    whole (~64 KB/s instead of ~384 KB/s for 48 kHz stereo) and the live
    preview snapshot is a plain copy instead of a per-tick full
    reconversion (was quadratic over the recording). Plus idle model
    unload: a background thread drops the transcriber after 15 min unused
    (constant for now; try_lock + recording check, so it never blocks or
    races a dictation) — model RAM is only held while dictating. The
    published v0.6.3 release includes this along with the #88–#95 batch.

### 0.7–0.10 — speech-to-text workbench (agreed 2026-09-24; status 2026-09-25)

Plan and checklists: `docs/superpowers/plans/2026-09-24-sussurro-speech-workbench.md`
(its *Status* section maps each release to its merged PRs).

Work is tracked as GitHub issues in milestones `Phase 0 — Spikes`, `0.7 — Notetaking`,
`0.8 — Advanced notetaking + links`, `0.9 — Meeting`, `0.10 — Advanced meeting`,
`Track E — Qwen3-ASR sidecar` and `Future`, with one epic issue per milestone
(#147–#152; Future is the single tracking issue #146); agents take issues
labelled `agent-ready`.

**Single release 0.10.0 — published 2026-09-25** (plan change 2026-09-25):
0.7, 0.8, 0.9, 0.10 and Track E shipped together as `v0.10.0` (tag on
`763e1d0`, release notes `docs/releases/0.10.0.md`); no intermediate
0.7/0.8/0.9 tags. The updater endpoint serves 0.10.0 on all nine platform
entries; the Firefox `.xpi` is signed on AMO (unlisted, secrets
`AMO_JWT_ISSUER`/`AMO_JWT_SECRET`) and `docs/extension/updates.json` lists
it. Still open: manual QA **#177** (0.7/0.8) and **#184** (0.9/0.10,
real Meet/Teams/Zoom calls, Meet names selector set, system audio) —
fixes from them ship as 0.10.x patch releases.

**0.10.1 — published 2026-09-26** (notes `docs/releases/0.10.1.md`): built
on the branch `release/0.10.x` (= `v0.10.0` + cherry-picks only, PR #300)
with #279 (Firefox ≥ 140 desktop, AMO warnings), #291 (Teams on
`teams.cloud.microsoft`, Zoom `/wc/` iframe) and #294 (meeting language;
backported without the 0.11 archive API code). Later 0.10.x patches go on
that branch the same way — never from `main`, which carries 0.11 work.

- **Phase 0** — spikes #106–#108 closed (Silero VAD, WeSpeaker embeddings,
  word timings). #104/#105 (browser capture, Meet names) were desk studies
  that the implementation followed; they stay open until #184 checks real
  calls. #109 (engine benchmark) is measured on the Mac only (Windows/Vulkan
  half open); on it Qwen3-ASR 1.7B Q8 passed the Track E gate, 0.6B did not.
- **0.7 — Notetaking** — **done** (merged; pending release): command mode
  removed, markdown archive + FTS index, long-form engine for mic + file
  (streamed decode, VAD segments, chunked cleanup, crash checkpoints,
  dictation priority), workspace shell A (the only UI since #115, with the
  first-run onboarding and "What's new").
- **0.8 — Advanced notetaking + links** — **done**: LLM profiles (keys in
  the OS keychain), recipes + companion document, Ask panel, privacy gate
  for external profiles, link source (direct media or `yt-dlp` on PATH),
  participants on transcriptions.
- **0.9 — Meeting** — **done** (on by default since #138): browser
  extension (Chrome/Edge/Brave + Firefox), pairing, side panel, Meet names +
  "Voice N", People registry, SRT/VTT, Identify voices on transcriptions,
  Library facets, recording notice.
- **0.10 — Advanced meeting** — **done**: system audio + mic (any input
  device, then native loopback per OS), opt-in saved WAV, per-speaker
  replay, speaker-aware recipes/Ask, voice map.
- **Track E** — **done**: pinned `llama-server` sidecar (b11146), Qwen3-ASR
  1.7B engine (optional, never default), *Local (bundled)* LLM profile
  (Qwen3 1.7B).
- **Future → planned (2026-09-25)**: voice recognition, text-to-speech and
  voice cloning with consent moved from #146 (still open as the umbrella)
  into the voices plan below.

### 0.11–0.13 — voices (planned 2026-09-25; decisions P12–P23 taken 2026-09-25)

Plan, research and checklists:
`docs/superpowers/plans/2026-09-25-sussurro-voices.md`. Product decisions
P12–P17 and P19–P23 were **accepted by the maintainer on 2026-09-25** as
recommended: voice recognition is opt-in and suggest-only, built from
confirmed lines (min. 60 s from 2 documents); voice profiles live in app
data (0600), never in the archive, exports or the API (GDPR art. 9); "You"
enrolment; archive API read-only plus `POST /archive/items` for notes;
Opus by default for saved audio; ICS attendees first, OAuth later;
single-narrator read-aloud; every generated audio file is marked (metadata
+ watermark that can't be switched off); cloning only the user's own voice
in 0.13.0, consenting others later; **0.13 ships only after a lawyer's
written review (#261)**; store listings AMO → Chrome → Edge. **P18 (default
TTS engine, decided after the #236 listening test)**: Pocket TTS — Italian
24-layer + English model — file generation only, not real time; the
English test is redone after #254, with Qwen3-TTS as the English fallback
on GPU Macs; Qwen3-TTS stays the 0.13 cloning engine. Issues that still need the maintainer (accounts,
legal review) carry `needs maintainer`. Milestones and epics:
`Phase V0 — Voices spikes` (#274, spikes #235–#240), `0.11 — Known voices`
(#275), `0.12 — Read aloud` (#276), `0.13 — Your voice, with consent`
(#277), `Track A — Accounts and stores` (#278).

- **0.11 — Known voices**: suggest-only voice recognition from confirmed
  speaker links (profiles in app data, never in the archive: GDPR art. 9),
  own-voice enrolment, overlap-aware Re-detect (pyannote segmentation-3.0,
  MIT, through `ort`), Teams/Zoom web names, Ogg Opus saved audio (WebKit
  plays Ogg Opus only from macOS 15.4, so older macOS needs a decode path),
  archive HTTP API with scoped hashed tokens (every browser Origin refused),
  calendar attendees from ICS.
- **0.12 — Read aloud** (**experimental, optional module — P24**: off by
  default under Settings → Experimental; TTS/cloning models are downloaded
  ONLY on the user's explicit request, never at install, onboarding or in
  the background): local TTS (default candidate Kyutai Pocket TTS,
  MIT + CC-BY-4.0, native Italian; decided by a bake-off + listening test),
  every generated file marked (watermark + metadata: AI Act art. 50 applies
  from 2 Aug 2026).
- **0.13 — Your voice, with consent**: own voice first; live consent with a
  nonce, transcript + voice match; never cloned from files, meetings or the
  archive; gated by a lawyer's review.
- **Track A**: store listings (Chrome, Edge, AMO listed), privacy policy
  page, Google/Microsoft calendar OAuth — maintainer accounts.
- Licence rule for all of it: code **and** weights must allow commercial
  use (dual-licence goal). Excluded: F5-TTS, XTTS-v2 (CPML), Fish-Speech,
  Spark-TTS (non-commercial), IndexTTS2, Higgs Audio (custom), DiariZen
  (CC BY-NC); no in-process GPL phonemizer (espeak-ng).

### Candidate / not committed

- **macOS Developer ID + notarization** and **Windows SignPath signing** —
  deferred (see standing decisions); installers remain unsigned for now.
- **Flatpak + Flathub** (roadmap item 11, still deferred).

## Per-machine setup

- **Windows dev machines must set a short `CARGO_TARGET_DIR`**
  (e.g. `setx CARGO_TARGET_DIR "C:\sbuild"`): whisper.cpp's Vulkan shader
  sub-build exceeds MAX_PATH otherwise (MSBuild FTK1011). Full details in
  `docs/compile/windows.md`.
- **Never commit a `.cargo/config.toml` with `target-dir`**: a drive-letter
  path is relative on Linux/macOS and its `:` breaks `cargo test`
  (`failed to join paths from $LD_LIBRARY_PATH`). One was removed from the
  repo for exactly this reason.

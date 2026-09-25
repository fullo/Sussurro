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
- **Meet names (0.9, #131, P8)** (`speakers/names.rs`,
  `extension/src/content/meet/`): layer 1 mic = "You", layer 2 names on
  **Meet only** (Teams/Zoom: layers 1 + 3), layer 3 Voice N. The page's
  identity is the RTP **CSRC** (`getContributingSources()` polled at
  10 Hz, mirror CSRCs excluded); names come from tiles through
  **versioned data-only selector sets** (attributes/structure, never
  obfuscated classes or label text; the shipped `meet-2026-09a` is
  unverified until #184 — replace values only from our own inspection,
  with a synthetic fixture) and are bound CSRC → name by voting (5 votes,
  margin 3, 1:1, drop after 10 contradictions). Health cross-checks
  against the audio; a broken hook sends `names_unavailable`, never
  guesses. Without CSRCs the lit tiles become a `dom` timeline. **Protocol
  2** (additive; `/app/version` reports `protocol_min: 1`, the extension
  accepts `protocol_min..=protocol`): `speaker_active {t, id?, name?,
  source?}`, `speaker_idle`, `speaker_name` (last binding applies to the
  whole call), `observer_health`; `t` = ms on the connection's audio clock
  (page frames mapped by the background). App: a remote line takes the
  name covering ≥ 50 % of it and 1.5× the runner-up, after lag
  compensation (dom 400 ms, caption 1.5 s, rtp 0) and 250 ms edge trim —
  constants in `AttributionParams`, to re-tune from #184's measurements;
  else Voice N. An end-of-run pass re-attributes with every event;
  `meet:<name>` speakers feed the People link suggestion; page
  participants join the frontmatter with People emails. Captions fallback
  not built (follow-up).
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

## Roadmap (agreed 2026-07-03; current version 0.10.0 — released 2026-09-25)

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
- **Future (tracked, not built)**: voice recognition after training,
  text-to-speech, voice cloning with consent (#146).

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

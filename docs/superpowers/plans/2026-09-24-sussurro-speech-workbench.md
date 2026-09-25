# Sussurro 0.7–0.10 — speech-to-text workbench: implementation plan

> First drafted 2026-09-23 as a meetings-extension plan, rewritten
> 2026-09-24 after the maintainer widened the scope. All product decisions
> below are confirmed by the maintainer (see *Decision log*); engineering
> decisions are recommendations to validate in Phase 0. No code has been
> written yet. Steps use checkbox (`- [ ]`) syntax. Workflow as per
> `CLAUDE.md`: branch → PR → merge, no direct pushes to `main`.

## Status (2026-09-25)

Everything below is merged on `main`; the text of the plan is kept as it
was written. Plan change of 2026-09-25: **one final release**, 0.10.0,
instead of separate 0.7/0.8/0.9/0.10 tags. Not yet released: the version
files still say 0.6.3, and the tag stays with the maintainer after the
manual QA below, the website (#180) and the version bump.

| Release | Merged PRs | Open |
|---|---|---|
| Phase 0 | spikes #106–#108 closed (no code PRs) | #104, #105 (desk studies, checked on real calls in #184), #109 (benchmark: Mac done, Windows half open) |
| 0.7 — Notetaking | #160, #161 (plan, mock), #162, #163 (command mode removed), #164 (archive), #165 (long-form engine), #166, #167 (checkpoints), #168 (dictation priority), #169 (shell A), #170, #171 (per-run options), #172, #209 (workspace only + onboarding) | manual QA **#177** |
| 0.8 — Advanced notetaking + links | #173 (LLM profiles), #174 (recipes), #175 (link source), #176 (Ask), #178 (participants), #179 (privacy gate), #182 (keychain) | manual QA **#177** |
| 0.9 — Meeting | #181 (extension scaffold), #183 (subtitles), #185 (`/live`), #186 + #190 (Voice N, link to People), #188 (People), #189 (pairing), #191 (facets), #192 (Identify voices), #193 (capture), #198 (side panel), #200 (Meet names), #204 (consent notice), #208 (Firefox parity), #211 (meetings on by default); fixes #201, #202 | manual QA **#184**; #128 still open for its manual checks |
| 0.10 — Advanced meeting | #195 (system audio + mic), #196 (speaker-aware recipes/Ask), #197 (saved audio), #203 (per-speaker replay), #206 (native loopback), #207 (voice map), #212 (build-only release run), #219 (multilingual fillers); security review: #220 (local API, #215), #221 (live rate limits, #217), #222 (yt-dlp and sidecar trust, #216) | manual QA **#184**; website **#180**; docs + release notes **#145** |
| Track E — Qwen3-ASR | #199 (sidecar packaging), #205 (Qwen3-ASR engine), #213 (bundled LLM profile), #214 | manual QA **#184** (engine on each OS) |

Release notes for users: `docs/releases/0.10.0.md`.

---

## 1. Goal

Sussurro becomes a local-first **speech-to-text workbench**: every audio
source ends up as a markdown document in the user's archive, optionally
followed by an LLM-formatted companion document. Hotkey dictation, today's
core feature, stays exactly as it is.

The work follows a ladder of use cases. Each rung reuses the previous one,
and each maps to a release:

| Release | Rung | What the user can do |
|---|---|---|
| **0.7** | Notetaking | Capture from the microphone or a file (wav, mp3). Clean the transcript of fillers, pauses and repetitions. Save it in the archive as `.md`, as a *note* or, for a file, a *transcription* (P10). |
| **0.8** | Advanced notetaking + link transcription | Everything in 0.7, plus: a formatted companion document next to the transcript (tl;dr, h1–h6 structure, tables). Ask external or local LLMs for summaries and analyses. Transcribe from a link (e.g. YouTube). |
| **0.9** | Meeting | Capture a web meeting through a browser extension (Chromium family, Firefox). Speaker-labelled transcript: names from the Meet interface, generic "Voice N" otherwise. Save as `.md` and, on request or always, `.srt`; subtitles and optional "Voice N" also for transcriptions (P11). Local archive with tags, categories and participants (name + email). |
| **0.10** | Advanced meeting | Everything in 0.9, plus: cleanup and formatted document on meetings, optional WAV, other sources through the OS mixer (desktop Zoom, Teams…), search by tag, category and participant, LLM questions on meetings. |
| Future | Voice recognition, text-to-speech | Tracked only, not implemented (section 12). |

Out of scope, by decision: cutting fillers and pauses out of the **audio**
file (cleaning is text-only); command mode (removed in 0.7); native
per-provider LLM adapters.

---

## 2. Decisions

### Product decisions (confirmed by the maintainer, 2026-09-24)

- **P1 — UX: proposal A**, a workspace with a left rail, adapted as in
  section 9.
- **P2 — Command mode is removed.** It is outside the project's focus and
  the operating systems ship their own voice control. Only the dictation
  hotkey remains. Spoken editing commands inside dictation ("a capo",
  "new line", "scratch that") are a different feature and stay.
- **P3 — Cleaning means the transcript text.** No audio editing.
- **P4 — The archive lives in `<Documents>/Sussurro`** on every OS.
- **P5 — Participant emails come from a People registry** filled by the
  user once and reused automatically. Meet, Teams and Zoom web do not
  expose emails reliably.
- **P6 — LLM providers: OpenAI-compatible APIs and Ollama only**, which
  covers llama.cpp's `llama-server`, LM Studio, DS4, Ollama and hosted
  services that expose `/v1`. No provider-specific adapters for now.
- **P7 — Subtitles are a setting**: "Create .srt automatically on every
  save" or "Only when I ask". Default: only when I ask *(default chosen in
  the plan, easy to flip)*.
- **P8 — Speakers in 0.9 are exactly two things**: names from the Meet
  interface and acoustic clustering with generic labels "Voice 1, Voice 2…".
  Cross-document voice recognition is future work. Embeddings are stored
  from 0.9 so that the future feature needs no re-recording.
- **P9 — WAV is saved only on request.**
- **P10 — Three item types, chosen by content, not by source.**
  *Note*: the user's own voice dictating a note (microphone, or a voice memo
  from a file); no speakers, no participants, no subtitles. *Meeting*: a
  live conversation among several people (browser extension, system audio
  plus mic, mic in a room); speakers, participants, subtitles.
  *Transcription*: audio recorded by others (link, podcast or interview
  file, lecture); speakers, participants and subtitles are optional. The
  File tab of *New* asks for the type, default *Note*.
- **P11 — Transcriptions get subtitles and speaker labels too**, from 0.9,
  with the same code as meetings: `.srt`/`.vtt` per the subtitles setting,
  and optional "Voice N" clustering (off by default, switched on per item or
  in *New*).

### Engineering decisions (recommended, validated in Phase 0)

- **E1 — One ingest pipeline for every source except hotkey dictation.**
  Sources produce 16 kHz mono frames per labelled channel; a long-form
  engine segments, transcribes, cleans and stores them (sections 4.1–4.2).
  The dictation path is not touched.
- **E2 — Markdown-first archive.** Files in the archive are the source of
  truth; the search index is derived and rebuildable (section 4.4).
- **E3 — Extension captures, app processes.** The browser extension is a
  capture device plus a live mirror; editing happens in the app.
- **E4 — Browser capture by hooking `RTCPeerConnection`** in a MAIN-world
  content script: the page's own mic track and the remote tracks arrive as
  separate streams, no second mic prompt, the tab is not muted.
  Fallbacks: `HTMLMediaElement.captureStream()` plus `getUserMedia`, then
  Chrome-only `tabCapture` through an offscreen document. Firefox has
  neither `tabCapture` nor audio in `getDisplayMedia`, so the first two
  paths are its whole story (MAIN-world scripts: Chrome ≥ 111,
  Firefox ≥ 128).
- **E5 — Transport: keep `tiny_http`, add WebSocket** via
  `Request::upgrade` + `tungstenite`. Fallback if the upgrade proves flaky:
  `axum` + `tokio` (tokio is already an optional dependency on Linux).
  Browser audio arrives at its native rate and goes through the existing,
  equivalence-tested `StreamResampler`.
- **E6 — Auth**: a random extension token, created when the user pairs the
  extension, sent as `Authorization: Bearer` (HTTP) and `?token=` plus an
  `Origin` check (WebSocket). CORS allows only `chrome-extension://` and
  `moz-extension://`. The existing `/clean`, `/transcribe`, `/history`
  routes stay token-less for local scripts. Loopback-only bind unchanged.
- **E7 — VAD: whisper.cpp's built-in Silero** (`whisper-rs` 0.16 exports
  `whisper_vad`); the `ggml-silero-v5.1.2.bin` model sits in the same
  HuggingFace repo as the whisper models, so the SHA-256-verified
  downloader covers it.
- **E8 — Speaker embeddings through `ort`**, already in the tree via
  transcribe-rs. Never add sherpa-onnx: it bundles a second ONNX Runtime.
  Candidates: WeSpeaker ResNet34-LM or 3D-Speaker ERes2NetV2, chosen in
  Phase 0 by accuracy, size and licence; SHA-256 pinned in code
  (fail-closed, as #90). Sortformer v2.1 via `parakeet-rs` is benchmarked
  against it.
  **Phase 0 result (#107, 2026-09-24): GO with WeSpeaker ResNet34-LM**
  (official ONNX, CC-BY-4.0, attribution required, 26.5 MB, SHA-256
  `7bb2f06e9df17cdf1ef14ee8a15ab08ed28e8d0ef5054ee135741560df2ec068`):
  ~48 ms per 3 s segment on 4 CPU threads (M1 Pro); online threshold 0.275
  on ≤ 3 s segments (3.3–4.5 % error, almost all extra voices), offline
  "Re-detect" threshold 0.30 on ≤ 10 s segments (0.0–0.7 %), plus a merge
  of clusters under 10 s of speech into the nearest voice. Fallback:
  3D-Speaker ERes2Net English (Apache-2.0, third-party ONNX export).
  Sortformer v2.1 is comparable in accuracy but rejected as default (492 MB,
  4-speaker cap, 10 s latency, NVIDIA licence to review; `parakeet-rs`
  ≤ 0.3.6 only matches our `ort` rc.12). Thresholds were tuned on English
  AMI clips: re-check on Italian and real meetings before pinning them.
- **E9 — Extra STT engines run in a bundled `llama-server` sidecar**, not
  in-process: `whisper-rs` and any llama.cpp binding each link their own
  ggml and cannot share one binary. Only the sidecar binary ships in the
  installer (Tauri `externalBin`, pinned release per target, checksums in
  the repo); model files download on first use (section 8).
- **E10 — Links to video platforms use `yt-dlp` found on PATH**, not
  bundled: it changes too often to pin, and downloading from those
  platforms is subject to their terms and to copyright, which the UI says.
  Direct media URLs download with `reqwest`.
- **E11 — Repository layout**: Rust modules under
  `sussurro/src-tauri/src/{archive,engine,sources,speakers,llm}/`; the
  extension in a new top-level `extension/` (Vite + TypeScript + React,
  `webextension-polyfill`, `manifest.chrome.json` / `manifest.firefox.json`).
  Shared React components for transcripts live in `sussurro/src/` and are
  imported by the extension through a Vite alias.
- **E12 — Incremental merges behind flags.** The new shell is developed
  behind a `ui_v2` dev flag until 0.7 ships; the extension routes behind
  `meetings_enabled` until 0.9 ships. Each flag is removed in the release
  that turns the feature on.

---

## 3. Architecture

```
                         ┌──────────── browser (0.9) ─────────────┐
                         │ content script: RTCPeerConnection hook │
                         │  + Meet active-speaker observer        │
                         │ AudioWorklet → PCM (mic, remote)       │
                         │ side panel: live mirror, Start/Stop    │
                         └───────────────┬────────────────────────┘
                                         │ ws://127.0.0.1:<port>/live (token)
┌──────────────────────── Sussurro (Tauri, Rust) ─────────────────────────────┐
│ sources/   mic (cpal) · file (symphonia, streamed) · url (reqwest, yt-dlp)  │
│            · browser (WS) · system audio (0.10)                             │
│                │ 16 kHz mono frames per labelled channel                    │
│ engine/    VAD (Silero) → segment queue → STT (AnyTranscriber:              │
│            whisper | parakeet | sidecar) → word timings → chunked cleanup   │
│                │                                                            │
│ speakers/  channel rule · Meet names · embeddings + clustering (0.9)        │
│                │                                                            │
│ archive/   <Documents>/Sussurro/…/transcript.md + .sussurro/segments.json   │
│            · index (app data) · people registry                             │
│                │                                                            │
│ llm/       profiles (OpenAI-compatible | Ollama) · recipes · map-reduce     │
│            → document.md (0.8)                                              │
│                                                                             │
│ UI shell A: New · Library · People · Recipes · Models · Settings            │
│ Hotkey dictation + overlay: unchanged path (recorder → STT → cleanup →      │
│ paste), minus command mode                                                  │
└───────────────────────────────┬─────────────────────────────────────────────┘
                                │ optional, on demand, loopback
                         llama-server sidecar (Qwen3-ASR; optional local LLM)
```

Nothing leaves the machine unless the user runs a recipe on an external LLM
profile, which always takes an explicit action (section 4.5).

---

## 4. Building blocks

### 4.1 Sources

One ingest interface: a source pushes 16 kHz mono frames tagged with a
logical channel (`mic`, `remote`, `system`, `file`) and a monotonic clock.

- **Mic** (0.7): the existing recorder, run for long sessions. It already
  buffers converted audio only (~64 KB/s); the engine consumes the buffer
  incrementally so memory stays flat.
- **File** (0.7): decoded **from the path, streamed** through symphonia.
  Today the file is sent as bytes over IPC and decoded whole into RAM
  (1 h ≈ 230 MB of f32) and transcribed in one call. Formats already
  enabled: wav, mp3, m4a/aac.
- **URL** (0.8): direct media links through `reqwest` with the existing
  timeouts; video platforms through `yt-dlp` on PATH, requesting the m4a
  audio track so symphonia decodes it without ffmpeg. Missing `yt-dlp`
  gives a clear message with install instructions.
- **Browser** (0.9): WebSocket frames `[u8 channel][u32 seq][i16 pcm…]` at
  the browser rate, resampled by `StreamResampler`.
- **System audio** (0.10): step 1, **any input device as a second channel**
  next to the mic. That covers BlackHole or Loopback on macOS, VB-Cable or
  Voicemeeter on Windows, and PipeWire/Pulse monitor sources on Linux.
  Step 2, native loopback: WASAPI loopback on Windows (verify on the pinned
  cpal 0.16), Core Audio process taps on macOS 14.2+ or ScreenCaptureKit
  audio on 13+, PipeWire monitor on Linux. The app's minimum macOS stays
  11.0; the native path is gated by OS version and asks its own permission.
  Mic and system on separate channels make "You" free, as in the browser.

### 4.2 Long-form engine

Any source → VAD segments (end of speech, or a 30 s cap) → queue → STT →
word timings → chunked cleanup → segment events → archive.

- **Segment cap of 30 s** keeps every engine inside its comfort zone and
  avoids the Qwen3-ASR empty-output bug on long inputs (llama.cpp #21847).
- **Word timings** where the engine provides them: whisper.cpp token
  timestamps; Parakeet through transcribe-rs *(verify the API in
  Phase 0)*; Qwen3-ASR only with its separate forced-aligner model,
  otherwise a proportional split by characters. Timings make SRT lines
  short and readable and let the transcript highlight during playback.
- **Chunked cleanup**: the cleanup LLM sees one segment plus the previous
  one as context, never a whole hour. Today's single call on a long
  transcript overflows small local models.
- **Raw and cleaned text are both kept** per segment, so a re-clean or a
  different cleanup level never needs re-transcription.
- **Transcriber sharing**: a running session counts as "in use" for the idle
  unloader; a dictation during a session waits for the current segment
  instead of colliding (extend the `unload_if_idle` tests).
- **Backlog, not drops**: on slow CPU machines segments queue and the UI
  shows how far behind the engine is; a "transcribe at the end" mode is
  available for very slow hardware.

### 4.3 Speakers (0.9)

Three layers, in order of reliability:

1. **Channel rule**: the mic channel is "You" whenever the mic is recorded
   separately (browser, system audio).
2. **Meet names**: the content script observes the Meet page for the
   active-speaker indicator and the participant list, and sends
   `speaker_active{name, t}` events; the engine assigns each remote segment
   to the name that was active for most of it. Meet's own live captions
   carry speaker names and are the fallback when the tile indicator is
   ambiguous. Selectors are versioned and expected to break; when they do,
   the segment falls back to layer 3.
3. **Acoustic clustering**: embeddings (E8) plus online clustering with a
   threshold from Phase 0 produce "Voice 1, Voice 2…" for the mic-only
   case (in-room meeting, one laptop), for sources without names, and for
   attribution failures. "Re-detect speakers" re-clusters the whole
   document offline. The user renames a voice for that document and can
   link it to a person in the People registry.

Teams web and Zoom web get layers 1 and 3 in 0.9; their name observers are
added when time allows, with the same fallback.

**People registry**: name, email, aliases. Stored in the archive at
`<archive>/.sussurro/people.json` so it travels with the archive. When a
Meet display name matches a name or alias, the participant gets the email
automatically. Calendar integration (attendees from the invite) would need
OAuth and is not planned.

### 4.4 Archive

**Location** (P4), resolved with Tauri's `document_dir()`:

| OS | Default path | Notes |
|---|---|---|
| macOS | `~/Documents/Sussurro` | First access triggers the macOS prompt for the Documents folder; ask during onboarding, never mid-recording. With "Desktop & Documents" on iCloud Drive, the archive syncs. |
| Windows | `%USERPROFILE%\Documents\Sussurro`, or the redirected Known Folder | With OneDrive folder backup on, Documents is under OneDrive and the archive syncs. |
| Linux | `$XDG_DOCUMENTS_DIR/Sussurro`, fallback `~/Documents/Sussurro` | `document_dir()` can fail without XDG user dirs; the fallback is explicit. |

The user can move it (for example into an Obsidian vault); moving offers to
relocate existing items. Because Documents is often cloud-synced, the "save
audio" option shows where the file will go. Models, settings, dictation
history, the search index and the audio cache stay in the app data
directory: they are not user documents.

**Layout**, one folder per item:

```
<Documents>/Sussurro/
  .sussurro/people.json
  2026/09/2026-09-24-weekly-sync-release-07/
    transcript.md            # frontmatter + cleaned, speaker-labelled text
    document.md              # recipe output (0.8+), optional
    transcript.srt           # per the subtitles setting (0.9+)
    audio.wav                # only when "save audio" is on (0.10)
    .sussurro/segments.json  # segments, raw + cleaned text, word timings, embeddings
```

**Frontmatter** (YAML) is the source of truth for metadata:

```yaml
type: meeting                     # note | meeting | transcription (P10)
title: Weekly sync — release 0.7
date: 2026-09-24T10:00:00+02:00
duration: 00:42:10
source: browser:meet.google.com   # mic | file:<name> | url:<link> | system
language: it
engine: whisper-small
tags: [release, roadmap]
categories: [team]
participants:
  - { name: Anna Rossi, email: anna@example.com }
  - { name: Voice 2 }
```

**Rules**
- The app regenerates `transcript.md` from `segments.json` only while the
  file is unchanged since the app last wrote it (content hash in
  `.sussurro/`). After an external edit the markdown wins, the app stops
  overwriting it and shows the item as "edited outside Sussurro".
- The app never deletes user documents on its own: there is no retention
  for the archive. Retention applies only to the audio cache and to the
  dictation history, as today.
- The search index (SQLite FTS5 through `rusqlite` with the bundled feature)
  lives in app data and is rebuilt from the folder on demand or when it is
  missing. It indexes title, text, tags, categories, participants, type and
  date.
- Hotkey dictations keep going to the JSONL history. Transcribing a file
  creates an archive item instead of a history entry from 0.7 on.
- Dictate-to-file keeps working unchanged in 0.7.

### 4.5 LLM profiles and recipes (0.8)

**Profiles** (P6): a profile is name, API (`openai` | `ollama`), base URL,
optional API key, model and a local/external flag. The flag is inferred
from the URL with the existing `is_local_endpoint` check and can be set by
hand. Today's `cleanup_api`, `ollama_url`, `ollama_model` and `api_key`
settings migrate into a default "Local" profile. Cleanup, recipes and the
Ask panel each pick a profile.

**Recipes**: a named prompt with a target, either *companion document*
(writes `document.md` or `<recipe>.md` next to the transcript) or *answer*
(shown in the Ask panel, savable as a document). Built-in: *Formatted
document* (tl;dr, h1–h6 structure, tables where the content is tabular),
*Summary*, *Action items*, *Decisions*. Users add their own. Long inputs
run map-reduce over segment chunks sized to the profile's context.

**Privacy gate**: a run on an external profile shows which document goes to
which host and needs an explicit click every time; there is no silent
fallback from local to external. Generated files carry provenance in their
frontmatter (`generated_by: <recipe> / <profile> / <model>`), and the
Library marks documents that were sent to an external host.

---

## 5. Data model

```rust
// archive/  — frontmatter of transcript.md
enum ItemType { Note, Meeting, Transcription }
struct ItemMeta {
    item_type: ItemType, title: String, date: String /* RFC 3339 */,
    duration: Option<String>, source: String, language: String, engine: String,
    tags: Vec<String>, categories: Vec<String>, participants: Vec<Participant>,
}
struct Participant { name: String, email: Option<String> }

// archive/  — .sussurro/segments.json
struct SegmentsFile { version: u32, speakers: Vec<DocSpeaker>, segments: Vec<Segment> }
struct DocSpeaker { id: String /* "you" | "meet:<name>" | "voice:<n>" */,
                    label: String, color: String, person_id: Option<String> }
struct Segment { id: u32, channel: Channel /* Mic | Remote | System | File */,
                 start_ms: u64, end_ms: u64, speaker_id: Option<String>,
                 raw: String, text: String, edited: bool,
                 words: Vec<Word>, embedding: Option<Vec<f32>> }
struct Word { w: String, start_ms: u64, end_ms: u64 }

// archive/  — <archive>/.sussurro/people.json
struct Person { id: String, name: String, email: Option<String>, aliases: Vec<String> }

// settings  — app data
struct LlmProfile { id: String, name: String, api: CleanupApi, base_url: String,
                    api_key: String, model: String, external: bool }
struct Recipe { id: String, name: String, prompt: String,
                target: RecipeTarget /* CompanionDocument | Answer */, builtin: bool }
enum SubtitlesMode { OnRequest /* default */, Always }
```

Exports: `.md` (the transcript as stored), `.txt` (plain lines
`[00:12:03] Anna: …`), `.srt` and `.vtt` (lines of at most two rows of
~42 characters and ~7 s, split on word timings), `.wav` (0.10, when saved).

---

## 6. Local API

Existing routes stay as they are and token-less: `POST /clean`,
`POST /transcribe`, `GET /history`. New routes (0.9) need the extension
token (E6):

| Method | Path | Purpose |
|---|---|---|
| GET | `/app/version` | `{app, protocol, protocol_min, subtitles}` handshake; the extension runs only when its protocol is within `protocol_min..=protocol` (#131); `subtitles` (`on_request` \| `always`) tells the side panel whether to offer "Create .srt" (#129) |
| WS | `/live?token=` | client → app: `start{title, url, platform, rate, channels}`, `speaker_active{name, t}` (protocol 2, #131: `speaker_active{t, id?, name?, source?}`, `speaker_idle`, `speaker_name{id, name}`, `observer_health`), `participants{names}`, `stop`, binary audio frames; app → client: `segment` (new or updated), `speaker{id, label, color}`, `status` (backlog, errors) |
| POST | `/items/{id}/open` | bring the app to the front on that item ("Open in Sussurro" in the side panel) |
| GET | `/items/{id}/export?format=md\|txt\|srt\|vtt` | "Copy as text" and downloads from the side panel |

The archive, the People registry and recipes are driven from the app
through Tauri commands, not the HTTP API. Exposing them to scripts is a
later decision.

---

## 7. Releases and phases

Estimates are focused working time for one developer.

Work is tracked as GitHub issues (created 2026-09-24): one milestone per
release, one epic issue per milestone, agents take issues labelled
`agent-ready`.

| Milestone | Epic issue |
|---|---|
| Phase 0 — Spikes | #147 |
| 0.7 — Notetaking | #148 |
| 0.8 — Advanced notetaking + links | #149 |
| 0.9 — Meeting | #150 |
| 0.10 — Advanced meeting | #151 |
| Track E — Qwen3-ASR sidecar | #152 |
| Future | #146 (single tracking issue, not to implement) |

### Phase 0 — Spikes and go/no-go (~1 week)

Throwaway code in the session scratchpad; results written into this file.

- [ ] **Browser capture**: minimal extension with the MAIN-world hook and an
      AudioWorklet sending mic and remote PCM to a local WebSocket echo
      server. Matrix: Meet, Teams web, Zoom web × Chrome, Firefox. Record
      whether both channels arrive, whether the call is unaffected, and
      which fallback each failure needs. (#104)
- [ ] **Meet speaker names**: find stable hooks for the active-speaker
      indicator, the participant list and the caption speaker label; measure
      the lag between the indicator and the audio. (#105)
- [ ] **VAD**: `whisper_vad` with the Silero model on a 16 kHz stream;
      boundaries and CPU cost. (#106)
- [x] **Diarization**: two embedding models through `ort` on a 3–4 speaker
      sample (own recording plus a CC-BY AMI/ICSI excerpt): time per 3 s
      segment on CPU (target < 100 ms), cluster purity with a cosine
      threshold; Sortformer v2.1 via `parakeet-rs` on the same sample.
      Licence check and pinnable SHA-256 for the winner. (#107)
- [ ] **Word timings**: confirm what whisper-rs, transcribe-rs Parakeet and
      Qwen3-ASR return. (#108)
- [ ] **Engine benchmark** (section 8). (#109)
- [ ] **Go/no-go**: capture works on at least 2 of 3 platforms in both
      browser families, clustering separates 3 speakers on the sample.
      Otherwise revisit E4 or E8 before 0.9 work starts. 0.7 and 0.8 do not
      depend on this gate. (#147)

### 0.7 — Notetaking (~4–5 weeks)

- [ ] **PR 1 — Remove command mode** (P2). Backend: drop `command_hotkey`
      and its default from `Settings`, `command_mode` from `AppState`, the
      `command` argument of `pipeline::handle_trigger` and the routing in
      `lib.rs`; delete `process_command` (`pipeline.rs`), `command_edit`
      (`cleanup/ollama.rs`) and `copy_selection` (`inject.rs`), whose only
      caller is command mode; `hotkey::apply` registers one shortcut; update
      or remove the tests on these paths. Migration: none, `Settings` is
      `#[serde(default)]` without `deny_unknown_fields`, so an old
      `settings.json` loads and drops the key on the next save; add a test
      that pins it. Frontend: remove the command hotkey recorder in
      `App.tsx`. Docs: delete `docs/blog/command-mode.html` and its links
      (blog index, site index, `sitemap.xml`), edit the mentions in
      `shortcuts-and-triggers`, `clipboard-injection`, `first-run`,
      `how-the-pipeline-works`, `cleanup-hallucination-guard`, `use-ds4`,
      `voice-commands` and the README. Release note pointing to Voice
      Control (macOS) and Voice Access (Windows 11). (#110)
- [ ] **PR 2 — Archive core**: `archive/` module with path resolution per OS
      and fallback, item folder naming (date + slug, collision-safe),
      frontmatter read/write (`serde_yaml` or a small hand-written emitter,
      decided in the PR), `segments.json`, markdown rendering, content-hash
      rule, FTS5 index with rebuild, Tauri commands for list, get, update
      metadata, delete, reveal in file manager. Unit tests with `tempfile`
      for every pure part. (#111)
- [ ] **PR 3 — Long-form engine for mic and file**: `sources/` mic and
      streamed file; `engine/` VAD segmenter, queue worker, word timings,
      chunked cleanup, raw + cleaned per segment, progress and backlog
      events, transcriber sharing with the idle unloader. File
      transcription now writes an archive item whose type the user picks
      (note by default, or transcription; P10). Integration test marked
      `#[ignore]` feeding a WAV through the engine, also run in
      `scripts/ci-local.sh`. (#113)
- [ ] **PR 4 — UI shell A** (behind `ui_v2`): split `App.tsx` (1.9k lines)
      into modules; left rail; *New* with Microphone and File; *Library*
      with list, type filter and full-text search; document pane with the
      transcript editor (edit a line, delete a line, metadata header with
      tags and categories); *Models* (today's engine and model choice);
      *Settings* with all current cards, including Dictation, which keeps
      tray-first behaviour. Responsive collapse below ~1000 px. (#114)
- [ ] **PR 5 — Onboarding and release**: Documents permission step on macOS,
      first-run pointer to the archive, remove `ui_v2`, README, blog post,
      `CLAUDE.md` roadmap and standing decisions, version bump, release. (#115)
- [ ] **Before PR 4**: update mock A to the adapted layout (section 9). (#112)

**Track E — Qwen3-ASR sidecar** (~1–1.5 weeks, parallel to 0.7 or 0.8,
only if the Phase 0 gate passes): see section 8.

### 0.8 — Advanced notetaking + link transcription (~2–3 weeks)

- [ ] **LLM profiles**: data model, migration of the four current settings
      into a default "Local" profile with a serde test, profile editor in
      *Recipes*, profile choice for cleanup. (#119)
- [ ] **Recipes**: engine with map-reduce over segment chunks, built-in
      recipes, user recipes, companion document writing with provenance
      frontmatter, *Document* tab in the document pane. (#120)
- [ ] **Ask panel**: run a recipe or a free question on the open document,
      profile selector that marks external profiles, answer saved as a
      document on request. (#121)
- [ ] **Privacy gate**: per-run confirmation for external profiles,
      "sent externally" marker in the Library, README privacy section. (#122)
- [ ] **URL source**: direct media download, `yt-dlp` detection and
      invocation, terms notice, item `type: transcription` with the link in
      `source`. *New* gains a Link tab. (#123)
- [ ] Items of type transcription accept participants in the metadata
      header (optional, P10). (#124)
- [ ] Tests: recipe chunking and prompt assembly are pure and unit-tested;
      one `#[ignore]` live test against a local Ollama. (#120)

### 0.9 — Meeting (~5–6 weeks)

- [ ] **Extension scaffold**: `extension/`, both manifests, build scripts,
      `web-ext lint` in CI, zips attached to the GitHub release (store
      listings are not on the critical path). (#125)
- [ ] **Capture**: content scripts from the spike, hardened (track add and
      remove, frames, teardown, per-tab state); background worker owning the
      WebSocket with reconnect and backoff; "app not running" detection. (#128)
- [ ] **Pairing**: token generation in *Settings → Browser extension*, copy
      and paste into the extension options, "Test connection". (#127)
- [ ] **Side panel** (`chrome.sidePanel` / Firefox `sidebar_action`): live
      lines with speaker chips, Start/Stop, recording badge on the action
      icon, "Open in Sussurro", "Copy as text", "Create .srt" when the
      subtitles setting is "on request". (#129)
- [ ] **App side**: `/live` WebSocket, token middleware, CORS, `Origin`
      check, meeting items (`type: meeting`), overlay pill shows
      "recording (meeting)". (#126)
- [ ] **Speakers**: channel rule; Meet name observer; embeddings,
      clustering, "Re-detect speakers"; speaker panel in the context pane
      (rename, move a line to another speaker, link to a person). (#130, #131)
- [ ] **People registry**: *People* screen (add, edit, aliases, email),
      automatic linking on name match, participants written to frontmatter. (#132)
- [ ] **Subtitles**: SRT and VTT writers (pure, tested), subtitles setting
      (P7) in *Settings → Archive*, Export menu; applies to meetings and
      transcriptions, never to notes (P10, P11). (#133)
- [ ] **Speaker labels on transcriptions** (P11): "Identify voices" toggle,
      off by default, in *New → File* and *New → Link* and in the item's
      speaker panel, running the same clustering as meetings. (#134)
- [ ] **Library facets**: filters by tag, category, participant and date on
      top of the index. (#135)
- [ ] **Consent**: a notice on the first meeting recording that other
      participants may need to be informed. (#136)
- [ ] **Firefox parity** and Edge/Brave smoke on the same Chromium build. (#137)
- [ ] Remove `meetings_enabled`, docs (README, blog, `docs/development.md`
      for the extension build), `licenses.json` regenerated, release. (#138)

### 0.10 — Advanced meeting (~2–3 weeks)

- [ ] **System audio, step 1**: second input device in *New → System audio
      + mic*, two-channel recording, "You" on the mic channel, clustering on
      the system channel. (#139)
- [ ] **System audio, step 2**: native loopback per OS behind version checks
      and permission prompts; documented in `docs/compile/*` and the blog. (#140)
- [ ] **Opt-in WAV** (P9): "save audio" per item and as a default in
      *New*; audio written incrementally with the header patched on stop;
      the location note from section 4.4. (#141)
- [ ] **Per-speaker replay**: available while a session's audio is still in
      the temporary cache and afterwards only when audio was saved; player
      in the *Audio* tab with transcript highlighting from word timings. (#142)
- [ ] **Recipes and Ask on meetings**: speaker-aware prompts (who said
      what), verified on the fixed corpus. (#143)
- [ ] **Voice map**: optional card in the context pane (2-D projection of
      the stored embeddings). Drop it if the maintainer does not want it. (#144)
- [ ] Docs and release. (#145)

**Total**: about 15–19 weeks of focused work, Phase 0 included, Track E
included.

---

## 8. Local STT engines beyond whisper

### Candidates (state as of September 2026)

| Model | Size · licence | Languages | Accuracy signal | Streaming | Runtime | Verdict |
|---|---|---|---|---|---|---|
| **Qwen3-ASR 0.6B / 1.7B** (Jan 2026) | 805 MB Q8 (0.6B), ~2× for 1.7B · Apache-2.0 | 52 incl. Italian | Fleurs multilingual avg 4.90 vs 5.27 for whisper-large-v3 | vLLM only; llama.cpp is one-shot | llama.cpp upstream (mtmd), official `ggml-org/*-GGUF`, `llama-server /v1/audio/transcriptions` | **Optional engine through the sidecar.** Strip the `language xx<asr_text>` prefix (llama.cpp #26749); keep inputs ≤ 30 s (#21847). Community figure on an M3 Air: ≈ 2.6 s for a 6.6 s clip; to re-measure. |
| Whisper (whisper.cpp) | 75 MB–1.5 GB · MIT | 99 | large-v3 5.27 avg | chunked | in tree | Stays; the only engine with the dictionary prompt |
| Parakeet TDT v3 | 640 MB · CC-BY-4.0 | 25 | good, CPU-fast | chunked | in tree (transcribe-rs 0.3.11) | Stays as the CPU engine |
| Voxtral Mini 4B Realtime (Feb 2026) | ≈ 2.5 GB Q4 · Apache-2.0 | 13 incl. Italian | 5.9 avg Fleurs | native, 240 ms–2.4 s | vLLM only | Watch: best fit for live meetings once a local runtime exists |
| Kyutai STT 1B / 2.6B | CC-BY-4.0 | en/fr, en | – | native | Rust/candle | No Italian |
| NVIDIA Sortformer v2.1 | NVIDIA licence | – | – | streaming diarization, ≤ 4 speakers | `parakeet-rs` (ort) | Diarization candidate in Phase 0, not STT |
| Cohere Transcribe, SenseVoice, Moonshine | non-commercial / zh-centric / English-tiny | – | – | – | transcribe-rs | No |

### Integration

- The sidecar starts on demand on a random loopback port, is health-checked
  before use, and stops on idle and on app exit, reusing the transcriber
  unload logic.
- `AnyTranscriber::Remote` sends multipart audio to
  `/v1/audio/transcriptions`; the OpenAI-compatible client already exists
  for cleanup.
- Models download from HuggingFace with the SHA-256 from the HF tree API,
  exactly like whisper models (`sha256_from_hf_tree` works unchanged).
  Baking ≥ 805 MB into installers and updater artifacts would multiply
  release size and macOS CI minutes.
- The same sidecar can serve a small instruct model as a zero-setup local
  LLM profile, which would remove the "install Ollama" onboarding step.
  Whether STT and LLM share one `llama-server` or run two is decided in the
  spike.

### Gate and tasks (Track E)

- [ ] **Benchmark** on the dev Mac (Metal) and a Windows Vulkan machine: a
      fixed 10-minute Italian and English corpus through whisper small,
      medium and large-v3-turbo, Parakeet, Qwen3-ASR 0.6B Q8 and 1.7B Q8.
      Record error count or WER, real-time factor, RAM and cold start. (#109)
- [ ] **Gate**: Qwen3-ASR ships as an optional engine if its real-time
      factor is below 0.5 on GPU and its Italian accuracy is at least that of
      whisper medium. It is never silently the default; it can become the
      suggested default only after a dictation soak, since it has no
      dictionary prompt (dictionary words keep going into the cleanup
      prompt). (#152)
- [ ] Sidecar packaging: pinned llama.cpp release per target (macOS arm64
      Metal, Windows x64 Vulkan, Linux x64 CPU), checksums in the repo, CI
      download and verification, `licenses.json`, `docs/compile/*`. (#116)
- [ ] `stt/remote.rs` lifecycle and client; output sanitiser (pure, tested);
      `SttEngine::Qwen3Asr` with model size choice; engine card in *Models*. (#117)
- [ ] Optional: sidecar-backed local LLM profile. (#118)

---

## 9. UX: proposal A, adapted

The mocks are in `docs/superpowers/plans/ux-mocks/`. Proposal A was chosen
([proposal-a-workspace.html](ux-mocks/proposal-a-workspace.html)); B and C
stay in the folder for the record. The mock predates the re-scope and
needs these changes before PR 4 of 0.7:

- **Rail**: *New* · *Library* · *People* · *Recipes* · *Models* ·
  *Settings*. People and Recipes appear when their release ships.
- **New**: one screen with a tab per source (Microphone, File; Link in 0.8;
  Meeting in the browser in 0.9; System audio + mic in 0.10) and the same
  options for each: language, cleanup level, save audio, default tags and
  category. The File tab also asks for the item type (note or
  transcription, default note; P10); from 0.9 File and Link offer
  "Identify voices" (P11).
- **Library**: list with type filter and search in 0.7, facets for tag,
  category, participant and date in 0.9, marker for documents sent to an
  external LLM.
- **Document pane**: tabs *Transcript* · *Document* (0.8) · *Audio* (0.10);
  metadata header with editable tags, categories and participant chips;
  line editor (edit, delete; move to speaker from 0.9).
- **Context pane**: *Speakers* (0.9), *Ask* (0.8), *Export*; optional voice
  map card (0.10).
- **Dictation** stays tray-first; its settings move to *Settings →
  Dictation* with a single hotkey; its history is reachable from there.
- **Size**: A is laid out at ≈ 1120×740; below ~1000 px the context pane
  becomes a drawer, since today's window is 700 wide.
- **Extension side panel**: live mirror only (lines, speaker chips,
  Start/Stop, Open in Sussurro, Copy as text, Create .srt).

---

## 10. Risks and mitigations

| Risk | Mitigation |
|---|---|
| Meet, Teams or Zoom change internals and break capture or names | Two capture fallbacks (E4), versioned selectors, fallback to clustering, Phase 0 matrix repeated before each release |
| Clustering errors on short turns and overlaps | 1 s minimum for embeddings, offline re-clustering, one-click reassignment of a line |
| Slow CPUs cannot keep live pace | Parakeet as CPU engine, queue with visible backlog, "transcribe at the end" mode |
| Local LLMs produce weak formatted documents on long inputs | Map-reduce over chunks, recipes tested on a fixed corpus, external profile as an explicit option |
| External LLM use contradicts the local-first promise | Off by default, explicit per run, visible marker, documented |
| Archive edited outside the app gets overwritten | Content-hash rule; the app never overwrites a file it did not write last |
| Documents synced to iCloud or OneDrive carries private audio off the machine | WAV is opt-in (P9) and the option names the destination folder |
| Second ONNX Runtime or second ggml in the binary | `ort` only for ONNX (E8); llama.cpp only in the sidecar (E9) |
| Sidecar port clashes, zombie processes, antivirus prompts on Windows | Random loopback port, health check, kill on exit and idle, pinned upstream binary listed in the README |
| Web pages abusing the local API | Token, extension-only CORS, `Origin` check, loopback-only bind (E6) |
| `yt-dlp` breakage and platform terms | Not bundled, detected on PATH, clear errors and a terms notice |
| Native system audio differs per OS and needs new permissions | Step 1 with any input device ships first; native paths behind version checks |
| Shell rewrite destabilises dictation | `ui_v2` flag until 0.7 ships; dictation backend untouched; manual hotkey check in every release |
| Scope growth delays everything | Each release is shippable alone; the extension waits for 0.9 and does not block 0.7 or 0.8 |

---

## 11. Test matrices

- **Every release**: `cargo test`, `cargo clippy --all-targets -- -D
  warnings`, the Xvfb E2E smoke test (or `scripts/ci-local.sh` while
  Actions is down), manual dictation check on macOS, Windows and Linux.
- **0.7**: a 60-minute mic session and a 60-minute mp3 on each OS: memory
  stays flat, the item appears in `<Documents>/Sussurro`, the archive
  survives an external edit, the index rebuilds after deletion.
- **0.8**: each built-in recipe on the fixed corpus with a local profile;
  external profile run shows the confirmation and the marker; one
  `yt-dlp` link and one direct mp3 link.
- **0.9**: Meet, Teams web, Zoom web × Chrome, Edge, Brave, Firefox × macOS,
  Windows 11, Ubuntu 24.04: both channels captured, call unaffected, live
  segments within ~2 s of a pause on GPU, Meet names attributed, at least
  two voices separated when names are missing, SRT opens in a video player
  with correct timing.
- **0.10**: desktop Zoom and Teams through a virtual device and through
  native loopback on each OS; saved WAV plays; per-speaker replay.

---

## 12. Future track (recorded, not implemented)

- **Voice recognition after training**: persisted voice profiles built from
  the embeddings stored since 0.9, a minimum number of confirmed segments
  before a voice is suggested, per-profile delete. Voiceprints are
  biometric data under GDPR article 9: local-only, explicit opt-in,
  one-click forget.
- **Text-to-speech**: archive documents to audio files (podcasts,
  audiobooks); links to audio through the URL source plus article
  extraction. Runtime candidates: the llama.cpp sidecar (work on Qwen3-TTS
  in mtmd exists) or an ONNX TTS through `ort`.
- **Voice cloning**: only the user's own voice or a speaker's recorded
  consent, metadata or watermark on generated audio, never from other
  people's meeting recordings.
- Also tracked: Teams and Zoom name observers if not done in 0.9, calendar
  attendees, Opus compression for saved audio, browser store listings,
  overlap-aware diarization (pyannote segmentation), scripting access to
  the archive through the HTTP API.

---

## 13. Documentation to update

Per release: `README.md` (features, privacy, requirements such as `yt-dlp`
and virtual audio devices), `CLAUDE.md` (roadmap and standing decisions:
P1–P9 in short form once 0.7 starts), `docs/development.md` (extension
build, sidecar), `docs/compile/*.md` (new build dependencies),
`docs/blog/` (archive, recipes, meetings extension, system audio, engine
choice update to `whisper-vs-parakeet.html`), `sussurro/public/licenses.json`
after every dependency change (`npm run licenses`).

---

## 14. Decision log

| Date | Decision |
|---|---|
| 2026-09-23 | Meetings extension planned; UX studied through three mocks; Qwen3-ASR evaluated as a sidecar engine |
| 2026-09-24 | Scope widened to the use-case ladder (notetaking → meeting → link; voice recognition and TTS future) |
| 2026-09-24 | UX proposal A chosen (P1) |
| 2026-09-24 | Command mode removed (P2), reversing a same-day decision to keep it |
| 2026-09-24 | Cleaning is text-only; audio cutting not wanted (P3) |
| 2026-09-24 | Archive in `<Documents>/Sussurro` (P4) |
| 2026-09-24 | People registry for emails (P5) |
| 2026-09-24 | LLM providers: OpenAI-compatible and Ollama only (P6) |
| 2026-09-24 | Subtitles: setting for automatic or on request (P7) |
| 2026-09-24 | 0.9 speakers: Meet names plus "Voice N" only (P8) |
| 2026-09-24 | Three item types by content; file default is note (P10) |
| 2026-09-24 | Subtitles and optional "Voice N" also for transcriptions from 0.9 (P11) |

---

## Sources consulted for the engine study

- Qwen3-ASR: https://github.com/QwenLM/Qwen3-ASR ·
  https://huggingface.co/Qwen/Qwen3-ASR-1.7B ·
  https://huggingface.co/ggml-org/Qwen3-ASR-0.6B-GGUF
- llama.cpp Qwen3-ASR issues: https://github.com/ggml-org/llama.cpp/issues/26749 ·
  https://github.com/ggml-org/llama.cpp/issues/21847 · community
  benchmarks: https://github.com/shershah1024/qwen3-asr-llamacpp
- Voxtral Mini 4B Realtime: https://huggingface.co/mistralai/Voxtral-Mini-4B-Realtime-2602 ·
  https://mistral.ai/news/voxtral-transcribe-2/
- parakeet-rs (Sortformer, Nemotron): https://github.com/altunenes/parakeet-rs
- transcribe-rs: https://docs.rs/crate/transcribe-rs/latest
- Kyutai STT: https://kyutai.org/stt/
- Overviews: https://northflank.com/blog/best-open-source-speech-to-text-stt-model-in-2026-benchmarks ·
  https://www.gladia.io/blog/best-open-source-speech-to-text-models

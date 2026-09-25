# Sussurro 0.11–0.13 — voices: implementation plan

> Drafted 2026-09-25 from issue #146, after the maintainer decided the same
> day to start the future track of the workbench plan
> (`2026-09-24-sussurro-speech-workbench.md`, section 12). **Product
> decisions below are proposals**: each is marked *needs maintainer
> decision* with a recommendation, and no issue that depends on one is
> `agent-ready` until it is confirmed. Engineering decisions are
> recommendations to validate in Phase V0. No code has been written yet.
> Steps use checkbox (`- [ ]`) syntax. Workflow as per `CLAUDE.md`:
> branch → PR → merge, no direct pushes to `main`.

## Status (2026-09-25)

Plan drafted; milestones and issues created (section 7). Nothing merged.

| Milestone | Epic | State |
|---|---|---|
| Phase V0 — Voices spikes | #274 | all six spikes agent-ready (#235–#240) |
| 0.11 — Known voices | #275 | waits on P12–P16 and the spikes; archive-API tokens (#249) and calendar files (#252) agent-ready |
| 0.12 — Read aloud | #276 | waits on P17, P18, P21 and the TTS and marking spikes; text preparation (#254) agent-ready |
| 0.13 — Your voice, with consent | #277 | waits on P19, P20 and the legal review (#261) |
| Track A — Accounts and stores | #278 | maintainer accounts (P22, P23); privacy policy page (#267) agent-ready |

#146 stays open as the umbrella. In the text, spikes V0-1 … V0-6 are
#235 … #240 in that order.

---

## 1. Goal

The workbench turns speech into documents. This phase adds **voices**:
Sussurro learns to recognise the people it has heard before, reads
documents back as speech, and, only with recorded consent, speaks in a
person's own voice. It also closes the smaller items that 0.10 left for
later. Local-first stays the rule: nothing in this plan sends audio, text or
voiceprints off the machine.

| Release | Rung | What the user can do |
|---|---|---|
| **0.11** | Known voices | People are recognised across documents after the user has confirmed a few of their lines ("this is Anna"), and suggested, never silently applied. Overlapping speech is detected when re-detecting speakers. Teams web and Zoom web get participant names like Meet. Saved audio is Opus (about 10× smaller than WAV). Scripts reach the archive through the local HTTP API with scoped tokens. |
| **0.12** | Read aloud | Any document, transcript or companion document becomes speech: play it in the app or save it as an audio file next to the item. A link to an article becomes audio too. Italian and English voices at least. All generated audio is marked as synthetic. |
| **0.13** | Your voice, with consent | The user clones their **own** voice, or the voice of someone who recorded a verified consent statement, and uses it for read-aloud. Cloned audio carries a watermark and metadata; consent is revocable and deletes the voice. |
| Track A | Accounts and stores | The extension in the Chrome Web Store, Edge Add-ons and AMO (listed); meeting attendees from a calendar (file first, then Google and Microsoft through OAuth). Everything here needs an account the maintainer owns. |

Out of scope, by earlier decision and unchanged: cutting audio (P3), command
mode (P2), provider-specific LLM adapters (P6), sherpa-onnx (second ONNX
Runtime), any ggml engine in-process (E9). New here: **cloning from meeting
recordings or archive audio is out of scope for good** (#146), and so is any
cloud TTS.

---

## 2. Decisions

### Product decisions (numbering continues the workbench plan)

Each needs a maintainer decision; the recommendation is what the issues
assume.

- **P12 — Voice recognition is opt-in, suggest-only, and built only from
  confirmed lines.** *Needs maintainer decision.* A person gets a voice
  profile only when the user links a speaker to them (the existing
  `SpeakerEdit::Link`) and turns on *Recognise this voice* for that person.
  The profile is built from the embeddings of the linked lines, and a match
  on a new document is shown as a suggestion ("Voice 2 sounds like Anna —
  link?"), never applied without a click. *Recommendation: yes, with a
  minimum of 60 s of confirmed speech from at least 2 documents before a
  profile makes suggestions (to confirm in spike V0-1).* Alternative: auto-
  apply above a high threshold. Rejected for 0.11: a wrong name in a
  transcript is worse than no name.
- **P13 — Voice profiles live in app data, not in the archive.**
  *Needs maintainer decision.* Voiceprints used to identify a person are
  biometric data under GDPR art. 9 (section 4.1). The archive is often
  synced to iCloud Drive or OneDrive (P4) and is meant to be moved and
  shared, so profiles stay in `<app data>/voices/` (mode 0600), are never
  exported with the config, never go to the UI or the HTTP API as vectors,
  and are deleted with one click per person and with *Forget all voices*.
  *Recommendation: yes.* Consequence: a second machine re-learns voices
  from the archive's confirmed links (the per-line embeddings already in
  `segments.json` make this a rebuild, not a re-recording).
- **P14 — The user's own voice ("You") is a profile too.** *Needs
  maintainer decision.* A short enrolment (read a paragraph, about 30 s)
  lets room meetings and system-audio recordings label the user's lines
  "You" even on a single channel. *Recommendation: yes, optional, offered
  in the speaker panel of a room meeting.* It is also the reference for
  cloning the user's own voice in 0.13.
- **P15 — Archive HTTP API: read-only in 0.11, writes later.** *Needs
  maintainer decision.* Scripts can list, search, read and export items
  and list people (names only by default) through scoped tokens; creating
  items from text or audio and editing metadata come in a later release
  after the read API has been used. *Recommendation: read-only first,* plus
  one write route, `POST /archive/items` for a note from text, because it
  is the most requested automation (clipboard or shortcut → note).
- **P16 — Saved audio becomes Opus by default.** *Needs maintainer
  decision.* New saves are Ogg Opus mono (24 kb/s), about 10× smaller than
  today's 256 kb/s WAV; WAV stays available as a setting. Existing WAV
  items are untouched; a *Compress audio* action converts one item or all.
  *Recommendation: yes,* since the archive is often cloud-synced.
- **P17 — Read-aloud scope for 0.12.** *Needs maintainer decision.* One
  narrator voice per document; output saved as `speech.opus` (or
  `speech-<recipe>.opus`) in the item folder when the user asks, otherwise
  played from a temporary file. A link to an article (URL source) becomes a
  *transcription*-like item whose text is the extracted article and whose
  audio is generated. A two-voice "podcast" recipe is a stretch goal.
  *Recommendation: yes; podcast in 0.12 only if the single-voice path is
  done early.*
- **P18 — Default TTS engine.** *Needs maintainer decision after spike
  V0-2.* The bake-off compares Kyutai **Pocket TTS**
  (MIT code, CC-BY-4.0 weights, native Italian 24-layer model since April
  2026, 100M parameters, streaming, no phonemizer), **Qwen3-TTS 1.7B-Base**
  (Apache-2.0 code and weights, Italian among its 10 languages, but no
  Italian preset speaker, so it needs a reference clip) and **Chatterbox
  Multilingual v3** (MIT, Italian, 0.5B, watermark built in).
  *Recommendation: Pocket TTS (Italian and English models, the variant
  without voice cloning) as the default read-aloud engine, if the
  maintainer's listening test on Italian agrees; Kokoro-82M is rejected as
  default (Italian voices graded C, and Italian needs espeak-ng, GPL-3.0).*
- **P19 — Whose voice can be cloned.** *Needs maintainer decision.* The
  user's own voice (enrolled in the app, P14) and a person who recorded the
  consent statement in the app, in a live session, verified (section 4.6).
  Never a voice taken from a meeting, a transcription, a link, a file or
  any archive audio, even with the owner's say-so in text. *Recommendation:
  own voice only in 0.13.0; consenting others in 0.13.x after the
  maintainer's legal review (P20).*
- **P20 — Legal review before cloning ships.** *Needs maintainer
  decision.* The EU AI Act transparency duties (art. 50) apply from
  2 August 2026 and Italy's AI law adds a deepfake offence (section 4.7).
  *Recommendation: the 0.13 release waits on a written check by a lawyer of
  the consent text, the marking and the README wording; the plan's reading
  is not legal advice.*
- **P21 — All generated audio is marked, cloned or not.** *Needs
  maintainer decision.* Metadata (a tag that says "synthetic speech,
  generated by Sussurro") on every file, plus an inaudible watermark on
  every file. Art. 50(2) covers any synthetic audio, not only clones.
  *Recommendation: yes, from 0.12; the watermark cannot be switched off in
  the UI.*
- **P22 — Calendar attendees: calendar files first, OAuth later.** *Needs
  maintainer decision.* 0.11 reads attendees from an `.ics` file or a
  private ICS link the user pastes (no account, no OAuth). Google and
  Microsoft OAuth come in Track A, only if the maintainer creates the
  accounts and accepts Google's verification process (section 4.9).
  *Recommendation: yes.*
- **P23 — Store listings.** *Needs maintainer decision.* List the extension
  on the Chrome Web Store, Edge Add-ons and AMO (listed). Keep the release
  zips and the self-hosted Firefox `.xpi` for users who prefer them.
  *Recommendation: yes; AMO first (the account and signing already exist),
  then Chrome, then Edge.*

### Engineering decisions (recommended, validated in Phase V0)

- **E13 — Voice profiles are centroids over WeSpeaker embeddings.** Same
  model as "Voice N" (E8, ResNet34-LM, 256-dim), so no new download. A
  profile keeps up to 8 centroids (one per recording condition: headset
  mic, room mic, remote channel), each the mean of ≥ 10 s of confirmed
  lines, plus the count and the documents they came from (ids only).
  Matching is cosine similarity of a document voice's centroid to each
  profile centroid, with a threshold and a margin over the runner-up tuned
  in spike V0-1 on Italian and English. Rebuildable from the archive at any
  time (P13).
- **E14 — Archive API = new routes, new tokens, same guard.** Routes under
  `/archive/…` behind `Settings.api_archive` (off by default), each needing
  an archive token: 32 random bytes shown once, stored as SHA-256, with a
  name, scopes (`read`, `people`, `write`) and a last-used time, managed in
  *Settings → Scripting*. The #215 guard runs first (Host, Origin, body
  caps). Archive routes refuse **every** browser `Origin`, extensions
  included, and send no CORS headers: they are for scripts. Embeddings,
  voice profiles and saved audio are never served; emails only with the
  `people` scope. The extension token keeps its own routes (E6); the
  token-less `/clean`, `/transcribe`, `/history` are unchanged.
- **E15 — Opus through libopus, Ogg through a pure-Rust muxer.** Encoder:
  the `opus` crate 0.4 (MIT/Apache-2.0) over `opusic-sys` (BSD-3, bundles
  libopus 1.6.1, built with CMake), or the pure-Rust `opus-rs` (BSD-3, a
  young 0.1.x port of libopus 1.6) if it passes interop tests, which
  removes the CMake step. Container: the `ogg` crate (BSD-3, pure Rust).
  VOIP mode, 16 kHz mono, 24 kb/s (~11 MB per hour instead of ~115 MB).
  Transcription keeps running on the PCM during capture; Opus is only what
  gets stored. Decoding for replay and *Identify voices*: symphonia has no
  Opus decoder (issue open since 2020), so the same libopus binding or
  `opus-rs` decodes. **Playback:** WebKit plays Ogg Opus only from Safari
  18.4, i.e. macOS 15.4, while the app supports macOS 11; so the
  `sussurro-audio:` scheme decodes Opus and serves WAV bytes when the
  WebView cannot play it (or always, which is simpler; spike V0-4 decides).
- **E16 — TTS runtime.** Order of preference, settled by spike V0-2:
  (a) an ONNX export through the existing `ort`, in-process (Pocket TTS and
  Chatterbox have community exports; must be SHA-256 pinned and
  re-checked for licence); (b) the `llama-tts` tool of the llama.cpp
  release we already pin: b11146 (2026-09-23) includes the mtmd TTS rework
  (llama.cpp PR #26254, merged 2026-08-04) that supports Qwen3-TTS 1.7B-Base
  and Pocket TTS. It would ship next to `llama-server` in the sidecar
  bundle. `llama-server` itself has no speech route yet (draft PR #26603,
  `POST /tts`), so (b) means a CLI run per chunk until that lands.
  (c) The Rust/Candle `pocket-tts` crate in-process is a third option (no
  ggml, no second ONNX Runtime), but it predates the multilingual release
  and adds a whole ML framework to the binary. **Never** a phonemizer under
  GPL-3.0 (espeak-ng) linked in-process: it is AGPL-compatible but would tie
  the future commercial licence (standing decision). Engines that tokenize
  text directly (Pocket TTS, Qwen3-TTS, Chatterbox) avoid the question.
- **E17 — Marking synthetic audio.** Two layers, as the Commission's Code of
  Practice on marking and labelling (final, 10 June 2026) asks: (1) an
  inaudible watermark, **Meta AudioSeal** (code and weights MIT since April
  2024; no official ONNX, so we export it ourselves and pin the SHA-256),
  run through `ort` on every generated file; (2) metadata: Vorbis comment
  tags in Ogg Opus (`SYNTHETIC=1`, generator, engine, voice, the IPTC
  digital source type `trainedAlgorithmicMedia`) and, for exports in a
  format the `c2pa` crate supports (WAV, MP3, M4A, FLAC; not Ogg), a C2PA
  manifest. The app offers a *Check a file* detector (AudioSeal detection),
  which is the "way for others to verify" the Code expects. Spike V0-6
  measures detection after Opus at 24 kb/s. Rejected: Sony SilentCipher
  (weights licence unclear), Resemble Perth (weights licence not stated);
  WavMark (MIT) is the fallback.
- **E18 — Consent verification = transcript + voice match + liveness.** The
  app shows a consent statement that includes a random 6-word phrase; the
  speaker reads it live into the mic (never from a file). The recording
  passes when the local STT transcript matches the statement (word error
  rate under a threshold) and its embedding matches the reference audio to
  be cloned (E13 threshold). The consent recording, its transcript, the
  nonce, the date and the app version are kept with the voice; revoking
  deletes the voice, the reference and the consent recording together.
- **E19 — Overlap detection with pyannote segmentation through `ort`.**
  The ungated `onnx-community/pyannote-segmentation-3.0`
  export (MIT, 6 MB fp32) runs on plain ONNX Runtime, so it goes through
  the existing `ort` next to WeSpeaker. Offline only, in "Re-detect":
  10 s windows, 7-class powerset per frame (3 local speakers, pairs), an
  overlap flag where two are active, and embeddings masked to frames with a
  single speaker. Pinned by SHA-256 at a pinned revision (fail-closed, as
  #90). Later upgrade to evaluate: NVIDIA Nemotron-3-Diarization
  (Sortformer v3, 23 Sep 2026, OpenMDW-1.1, up to 8 speakers), but
  `parakeet-rs` runs it on `ort` rc.13 while the app pins rc.12, and Italian
  is not among its training languages.
- **E20 — Teams and Zoom name observers follow the Meet design (#131).**
  Versioned, data-only selector sets per platform; CSRC binding where the
  platform exposes contributing sources; health checks; `names_unavailable`
  instead of a guess. Values come only from our own inspection of live
  pages (#184 method), never from other projects' lists.
- **E21 — Calendar sources behind one trait.** `CalendarSource` returns
  events with a start, an end, a title and attendees (name, email). ICS
  first (pure parser, tested on fixtures; RRULE expansion only for the day
  asked). OAuth sources use PKCE with a loopback redirect on a random
  `127.0.0.1` port, refresh tokens in the OS keychain (the #159 store),
  read-only scopes. Attendees become participants and People suggestions,
  never People entries without a click.
- **E22 — No new runtimes in the main process.** ONNX models (TTS, overlap,
  watermark) go through the existing `ort` (E8); any ggml TTS runs in the
  bundled `llama-server` or in a second pinned sidecar binary with the
  same packaging as #116 (lock file, SHA-256, `tauri.sidecar.conf.json`).
  Python is never shipped.

---

## 3. Architecture

```
┌─────────────────────────── Sussurro (Tauri, Rust) ───────────────────────────┐
│ speakers/  embeddings (WeSpeaker) · clustering · names (Meet, Teams, Zoom)   │
│            · overlap (pyannote segmentation, 0.11) · profiles (0.11) ────────┼─► <app data>/voices/
│                                                                              │   (never in the archive)
│ archive/   items · people · audio (WAV | Ogg Opus, 0.11) · speech.opus (0.12)│
│ api/       guard (#215) → extension routes (E6) · scripting routes           │
│            · archive routes + scoped tokens (0.11)                           │
│ tts/       text prep (markdown → speakable) → engine → marker → writer (0.12)│
│ voices/    enrolment · consent capture + check · cloned voices (0.13) ───────┼─► <app data>/voices/
│ calendar/  ICS (0.11) · Google, Microsoft OAuth (Track A)                    │
└───────────────┬──────────────────────────────────────────────────────────────┘
                │ loopback, private socket, per-spawn key (#216)
         llama-server sidecar(s): Qwen3-ASR · bundled LLM · (TTS, if E16 says so)
```

---

## 4. Building blocks

### 4.1 Voice recognition (0.11)

**What exists.** Every meeting line and every transcription line with
*Identify voices* stores its WeSpeaker embedding in `segments.json`
(`Segment.embedding`, mean of the line's ≤ 3 s windows). A document
speaker can be linked to a People entry (`person_id`). "Re-detect" clusters
a document offline. Nothing crosses documents today.

**Enrolment.** For a person with *Recognise this voice* on, the app collects
the lines of every speaker linked to that person across the archive (index
query on `person_id`), groups them by recording condition (channel and
source kind), and writes centroids to `<app data>/voices/<person id>.json`.
Lines the user moved away from that speaker are excluded; lines with
overlap (4.3) are excluded. Minimums per P12.

**Suggestion.** After a run with speakers and after "Re-detect", each
"Voice N" centroid is compared with every profile. A match above the
threshold and the margin shows a chip in the speaker panel: *Sounds like
Anna · Link · Not Anna*. *Not Anna* is remembered for that document only.
Accepting links through the existing path, so participants and emails
follow as today.

**Privacy rules** (GDPR art. 9, section 4.7).
- Off until the user turns it on per person; a first-use sheet explains
  what is stored, where, and how to delete it.
- The processing is local; profiles never leave the machine; vectors never
  reach the UI, the HTTP API, diagnostics or config export.
- *Forget this voice* on the person; *Forget all voices* in
  *Settings → Privacy*; deleting a person forgets their voice.
- A person must be told before their voice is learnt: the first-use sheet
  says so, and the README explains when the household exemption applies and
  when it does not (a company using Sussurro for colleagues).

### 4.2 Archive HTTP API (0.11)

Routes (JSON, `read` scope unless noted):

| Method | Path | Returns |
|---|---|---|
| GET | `/archive/items?q=&type=&tag=&category=&participant=&from=&to=&limit=&cursor=` | items (id, title, type, date, tags, categories, participants' names), facet counts optional |
| GET | `/archive/items/{id}` | frontmatter + rendered transcript text + speakers (labels only) |
| GET | `/archive/items/{id}/export?format=md\|txt\|srt\|vtt` | the same exports as the extension route, for any item |
| GET | `/archive/items/{id}/documents` and `/documents/{name}` | companion documents |
| GET | `/archive/people` | names and aliases; emails only with the `people` scope |
| POST | `/archive/items` (`write`) | creates a note from `{title, text, tags?}`; 1 MiB cap |

Rules: E14 plus a per-token rate limit (burst 60, 10/s), pagination with an
opaque cursor, errors with a `code`, ids validated with the archive's own
id check (#215 confinement), and no route that takes a path. A read of an
item edited outside the app returns the file as it is. The blog post
`local-api.html` and `docs/manual` gain a scripting section with `curl`
examples.

### 4.3 Overlap-aware diarization (0.11)

Today "Voice N" gives every line one speaker, and a line where two
people talk over each other gets the embedding of the mixture, which can
create a spurious voice or pull a line to the wrong person. Overlap
detection fixes the second problem and flags the first.

| Model | Licence | Size | Runtime | Verdict |
|---|---|---|---|---|
| **pyannote segmentation-3.0** (onnx-community export) | MIT (original repo gated; the export is not) | 6 MB fp32, 1.5 MB int8 | plain ONNX Runtime, CPU, ~14 s of audio in 19 ms on an M4 Max | **Chosen** (E19) |
| pyannote community-1 pipeline | CC-BY-4.0, gated | – | segmentation + WeSpeaker + VBx/PLDA | Borrow the VBx/PLDA idea later, with attribution |
| NVIDIA Sortformer v2 / v2.1 | CC-BY-4.0 / NVIDIA Open Model License | 117M | `parakeet-rs` | Rejected in #107 (4-speaker cap, size); OML has guardrail and termination clauses |
| NVIDIA Nemotron-3-Diarization | OpenMDW-1.1 | ~99M | `parakeet-rs` on `ort` rc.13 | Watch; needs an `ort` bump and an Italian check |
| DiariZen | weights CC BY-NC 4.0 | – | – | **Incompatible** |

What changes for the user: "Re-detect speakers" marks lines with overlapping
speech (a small icon, and `overlap: true` in `segments.json`), keeps them
out of voice centroids and voice profiles, and splits a line into two
speakers only when the overlap is a whole sentence (the rest stays one line
with a note). Live runs are unchanged.

### 4.4 Opus saved audio (0.11)

| Option | Licence | Build | Verdict |
|---|---|---|---|
| `opus` 0.4 + `opusic-sys` (libopus 1.6.1 bundled) | MIT/Apache-2.0 + BSD-3 | CMake on every runner | **Default choice** |
| `audiopus` / `audiopus_sys` | ISC + BSD-3 | CMake, libopus 1.3 | Older libopus |
| `opus-rs` (pure-Rust port of 1.6) | BSD-3 | none | Candidate if interop tests pass |
| `ogg` 0.9 | BSD-3 | none | Ogg muxer and demuxer |
| WebM/Matroska | – | – | Not needed for audio-only files |

Layout (P16): `audio.opus` or `audio-<channel>.opus`, with the same
per-channel design and t = 0 padding as the WAV files of #141. Ogg pages
are flushed every few seconds, so an interrupted file plays up to the last
page (the #153 recovery learns the Ogg case). WAV stays selectable, and the
file-name pattern accepts both. *Compress audio* converts existing WAV
items and moves the originals to the OS trash only after the Opus file
decodes to the same duration.

Playback needs care on macOS: WebKit added Ogg Opus in Safari 18.4
(macOS 15.4). On older macOS the scheme serves decoded PCM as WAV: either
the range request is mapped to the Opus stream through granule positions,
or a decoded temporary copy is cached per item while the tab is open
(spike V0-4 picks one). Windows (WebView2) and Linux (WebKitGTK with
GStreamer) are expected to play Ogg Opus natively; the same spike confirms
it.

### 4.5 Text-to-speech (0.12)

**Candidates** (state as of September 2026; code **and** weights
licences checked, since the dual-licence goal needs commercial use of
both):

| Engine | Code · weights | Italian | Size | Runtime path for us | Cloning | Streaming | Verdict |
|---|---|---|---|---|---|---|---|
| **Pocket TTS** (Kyutai) | MIT · CC-BY-4.0 (cloning variant gated behind use terms) | native since v2.0 (Apr 2026), `italian_24l` | 100M | ONNX (community) via `ort`; `llama-tts` (b11146); Rust/Candle crate | yes, ~5 s reference | yes, ~200 ms first audio | **Default read-aloud candidate** |
| **Qwen3-TTS 0.6B / 1.7B** (Jan 2026) | Apache-2.0 · Apache-2.0 | one of 10 languages; no Italian preset speaker | 0.9B / 1.7B | `llama-tts` 1.7B-Base (b11146); community ONNX for 0.6B | yes, 3 s | yes | **Cloning candidate** |
| **Chatterbox Multilingual v3** (Resemble) | MIT · MIT | one of 23+ | 0.5B | onnx-community export via `ort` | yes, ~10 s | not documented | Runner-up for both; PerTh watermark on by default |
| Kokoro-82M | Apache-2.0 · Apache-2.0 | 2 voices, grade C | 82M | ONNX via `ort` | no | chunked | Fallback only: Italian needs espeak-ng (GPL-3.0) |
| Piper (piper1-gpl) | GPL-3.0 · per voice | paola, riccardo | 5–30M | ONNX via `ort` + espeak-ng | no | yes | No: dated quality, GPL phonemizer |
| Orpheus 3B | Apache-2.0 · Llama 3.2 licence; Italian is a research release | preview | 3B | GGUF + SNAC | yes | yes | No: size and licence friction |
| CosyVoice 3, Zonos | Apache-2.0 | yes / unofficial | 0.5–1.6B | Python only | yes | yes | No: would need Python |
| NeuTTS, Kyutai TTS 1.6B, Dia/Dia2, Sesame CSM, MeloTTS, Kitten, Supertonic | permissive (Supertonic: OpenRAIL-M) | **no Italian** | – | – | – | – | No |
| F5-TTS, XTTS-v2, Fish-Speech/OpenAudio, Spark-TTS | weights CC-BY-NC, CPML or CC-BY-NC-SA | – | – | – | – | – | **Incompatible** (non-commercial) |
| IndexTTS2, Higgs Audio v2 | custom licences (derivative-work limits; user cap) | – | – | – | – | – | **Incompatible** |
| VibeVoice | MIT, code pulled by Microsoft (Sep 2025) | – | – | – | – | – | No: unmaintained upstream |

No published comparison covers Italian quality across these engines, so
the bake-off (V0-2) includes a blind listening test by the maintainer on a
fixed Italian text set (numbers, dates, names, a news paragraph, a
transcript with speakers). Pocket TTS's gated cloning weights come with
use terms (no cloning without consent) that match P19 but must be read by
the maintainer before they are downloaded by the app.

**Text preparation** (pure, tested): markdown to speakable text (headings
become pauses, tables are read row by row with their headers or skipped by
a setting, links read their text, code blocks skipped), transcript speaker
prefixes read as "Anna says" or dropped, numbers, dates, times and common
abbreviations expanded per language (Italian and English first), sentences
chunked to the engine's limit.

**Output**: `speech.opus` next to the item (Opus per E15) with the marking
of E17, listed under the frontmatter key `speech:` (app-owned like
`audio:`), played in the *Audio* tab through the existing `sussurro-audio:`
scheme (its file-name pattern widened to `speech(-[a-z0-9-]+)?.(opus|wav)`).
Read-aloud without saving uses a temporary file in app data, deleted on
close.

**Article links**: the URL source (#123) gains an *article* path: fetch the
page with the same local-network rules and timeouts, extract the main text
with a Readability-style extractor (a permissive Rust crate, chosen in the
PR), save it as a document item and offer *Read aloud*.

### 4.6 Voice cloning with consent (0.13)

**Flow for the user's own voice** (P19, first): the user opens
*Voices → Your voice*, records the consent statement (E18), then 1–3
minutes of reading from a fixed text shown on screen (the reference). The
app checks that consent and reference are the same speaker and that the
reference matches the "You" profile when one exists (P14). The cloned
voice then appears in the read-aloud voice list, labelled *Your voice
(cloned)*.

**Flow for another person** (P19, after the legal review): the same, run
with that person at the microphone, in the app, in one session; the
consent statement names them, the purpose and Sussurro. The voice is tied
to their People entry, and deleting the person or revoking deletes it.

**Consent statement** (Italian and English, reviewed in P20), for example:
*"I, Anna Rossi, agree that Sussurro on this computer creates a synthetic
copy of my voice to read texts aloud. I can withdraw this at any time.
Codice: tavolo verde nove fiume lampada sei."* The random phrase changes
every attempt; the check needs the phrase exactly and the rest within a
word error threshold.

**Guardrails**
- No UI path and no backend command takes a file, an archive item, a
  meeting or a link as the reference: the reference is recorded live in the
  same session as the consent. Tests assert this at the command level.
- Replay heuristic: a consent recording nearly identical to a previous one
  (embedding and waveform fingerprint) is refused.
- Every cloned output is marked (E17) and labelled "cloned voice" in its
  frontmatter; exports keep the tags.
- A cloned voice is never used by the local HTTP API and never exported
  with the config.
- Revocation deletes the reference, the consent recording and the engine's
  speaker file, and lists the generated files that used the voice (the user
  decides about them; the app does not delete documents, as in the
  workbench plan).

**Engine** (E16): Qwen3-TTS 1.7B-Base through `llama-tts` in the sidecar
(Apache-2.0 end to end, 3 s cloning, Italian supported), or Chatterbox
Multilingual v3 through `ort` (MIT), decided by spike V0-2. Pocket TTS's
cloning weights are gated behind use terms that must be read first.

### 4.7 Law and ethics (reading as of September 2026; not legal advice)

- **EU AI Act, art. 50.** From **2 August 2026**, providers of
  systems that generate synthetic audio must mark the output in a
  machine-readable way that is detectable as AI-generated (50(2)); deployers
  must disclose deepfakes (50(4)). The open-source exemption of art. 2(12)
  does **not** cover art. 50. The Digital Omnibus on AI (Regulation (EU)
  2026/1744, in force 27 July 2026) did not move art. 50; it only gives
  systems already on the market before 2 August 2026 until 2 December 2026,
  so a TTS or cloning feature launched now must mark from day one. The
  Commission's Code of Practice on marking and labelling (final 10 June
  2026, voluntary) asks for at least two marking layers and a way for others
  to verify. Sussurro's maintainer would be the **provider**; most users,
  acting privately, are outside the Act, but a company using the app
  professionally is a deployer. Hence P21 and E17.
- **GDPR art. 9.** A voice becomes biometric data when processed "for the
  purpose of uniquely identifying" a person. "Voice N" inside one document,
  with no link to a name, is arguably not identification; a voice profile
  tied to a People entry, and a clone reference, are. The EDPB guidelines
  02/2021 on voice assistants ask for explicit consent, an equivalent option
  without a voiceprint, and storage on the user's device. The household
  exemption (art. 2(2)(c)) covers purely personal use and generally not
  meetings with colleagues. Hence P12, P13 and the first-use sheet.
- **Italian Garante.** The warning of 18 December 2025 targets deepfake
  services that use other people's voices without a legal basis (arts.
  5(1)(a), 6, 9). Earlier: the deepfake vademecum.
- **Italian criminal law.** Art. 612-quater c.p. (from L. 132/2025, in
  force 10 October 2025) punishes spreading AI-falsified images, videos or
  **voices** without consent, able to deceive and causing unjust harm (1–5
  years). Voice is also protected as a personality right by analogy with
  art. 10 c.c. Hence P19: own voice first, consenting others only after the
  legal review (P20).
- **Consent designs elsewhere.** ElevenLabs asks the owner to read a
  verification line compared with the training voice; Azure Personal Voice
  requires a recorded statement naming the speaker and the company and
  checks it with speaker verification; a Hugging Face "voice consent gate"
  adds a random sentence each session and notes that a speaker match is the
  missing piece; Resemble's live check was beaten by playing a recording.
  E18 combines these: live, nonce, transcript match, voice match, replay
  heuristic.

### 4.8 Teams web and Zoom web names (0.11)

Desk research, same method as #105. We studied approaches only and copied
no selector values; ours must come from our own inspection of live pages
during the #184 manual QA.

- **Teams web** delivers one mixed remote audio stream, so the DOM is the
  only "who is speaking" signal. Projects that track it use a voice-level
  outline element on each participant's stream wrapper, found through
  `data-tid`-style attributes, and take names from roster and tile
  attributes and aria labels; they call these attributes unstable across
  Teams builds. Captions carry an author attribute but depend on the tenant
  allowing captions. Vexa moved to VAD-first segmentation with the DOM as
  enrichment only, which is already Sussurro's design.
- **Zoom web client** delivers one WebRTC track per participant, so the
  per-channel audio is reliable and the channel → name mapping is the weak
  part. The signals are the active-speaker frame classes and the name in
  the avatar footer, bound with votes and hysteresis. Caption markup has no
  names.
- Both get the Meet treatment (E20): a versioned selector set per
  platform, CSRC binding where remote receivers expose contributing sources
  (to check), health checks, and "Voice N" when a hook misses. A missing DOM
  signal never gates audio.

### 4.9 Calendar attendees (0.11 file, Track A OAuth)

**Without accounts (0.11, P22).** The user imports an `.ics` file or pastes
a private ICS link: Google's "secret address in iCal format" (Workspace
admins can turn it off) or Outlook's published calendar link. Whether a
feed has `ATTENDEE` lines with emails depends on the provider and on the
detail level the user publishes, so the UI says what it found. The link is
a secret and is kept in the OS keychain. macOS EventKit (Calendar
permission, covers every account already in Calendar.app, no OAuth) is a
later option.

**Google (Track A).** An OAuth client of type *Desktop app*, a loopback
redirect `http://127.0.0.1:<random port>` and PKCE (S256). The desktop
client secret ships in the app; Google does not treat it as a secret. The
scope `calendar.events.readonly` (or the narrower
`calendar.events.owned.readonly`) is **sensitive, not restricted**, so no
CASA security assessment applies. Sensitive scopes need verification: the
domain verified in Search Console, a homepage and a privacy policy on that
domain, a justification for each scope, and an unlisted demo video. It
typically takes 3–5 business days, with no fee. An unverified app shows a
warning and has a lifetime cap of 100 users; an app left in *Testing* gets
refresh tokens that expire after 7 days.

**Microsoft (Track A).** An Entra ID app registration as a public client
("Mobile and desktop applications", no secret), a loopback redirect (the
port is ignored, and `127.0.0.1` has to be added through the manifest),
PKCE, and delegated `Calendars.ReadBasic`. Since late 2025 Microsoft's
managed default consent policy blocks user consent for `Calendars.*` in
many work tenants, so organisations need an admin consent step whatever the
publisher status. Publisher verification is free but needs a Microsoft AI
Cloud Partner Program account, a work (not personal) Entra account and a
DNS-verified domain.

**Maintainer actions**
- Google: a Cloud project with the Calendar API; a consent screen (name,
  support email, logo); a Desktop client id; a homepage and privacy policy
  on a domain the maintainer owns, verified in Search Console; a demo
  video; the verification request.
- Microsoft: an Entra tenant with a custom domain; the app registration
  (multi-tenant + personal accounts, public client, loopback redirect);
  Partner Program enrolment; publisher verification; a documented admin
  consent step for organisations.

### 4.10 Browser store listings (Track A)

| Store | Cost and account | Requirements that matter for us | Review |
|---|---|---|---|
| **Chrome Web Store** | $5 once; 2-Step Verification; EU trader / non-trader declaration (a trader's address is public) | single purpose; a justification for every permission and host permission (meeting hosts, `tabCapture`, `offscreen`); no remote code; data-use certification; **a privacy policy even for data processed only locally** | days; weeks for new developers and host permissions |
| **Edge Add-ons** | free; Partner Center (a personal Microsoft account verifies quickly) | the same privacy fields; a privacy policy when personal data is accessed | up to 7 business days |
| **AMO (listed)** | free; the account and signing keys already exist (unlisted `.xpi` today) | source code + build instructions for bundled code (reviewers rebuild on Ubuntu 24.04 / Node 24 and diff); `data_collection_permissions` mandatory for new extensions since 3 Nov 2025, supported from Firefox 140 (see #234: the manifest says 128 and `["none"]`); the policies explicitly cover data sent to a native application, so declaring `personalCommunications` / `websiteContent` is the conservative choice | human review |

**Maintainer actions**: pay the Chrome fee, turn on 2-Step Verification and
file the trader declaration; create a Partner Center account; decide the
AMO listed/unlisted switch; approve the privacy policy page (drafted in
Track A, hosted on the project site); submit and answer the reviewers.

---

## 5. Data model

```rust
// <app data>/voices/<person id>.json  (0600; never in the archive)
struct VoiceProfile { version: u32, person_id: String, enabled: bool,
                      model: String /* "wespeaker-resnet34-lm" */,
                      centroids: Vec<Centroid>, updated: String }
struct Centroid { condition: String /* "mic" | "remote" | "system" | "file" */,
                  vector: Vec<f32>, speech_ms: u64, documents: Vec<String> }

// settings — archive API tokens (hash only)
struct ArchiveToken { id: String, name: String, sha256: String,
                      scopes: Vec<Scope> /* Read | People | Write */,
                      created: String, last_used: Option<String> }

// archive — frontmatter additions (app-owned keys, kept in ItemMeta::extra)
//   audio: [audio.opus]            0.11, Opus or WAV
//   speech: [speech.opus]          0.12
//   synthetic: { engine, voice, marked: [metadata, watermark] }   0.12

// <app data>/voices/cloned/<voice id>/  (0.13)
struct ClonedVoice { id: String, owner: VoiceOwner /* You | Person(id) */,
                     engine: String, reference: String /* file name */,
                     consent: Consent, created: String }
struct Consent { statement: String, nonce: String, recording: String,
                 transcript: String, wer: f32, similarity: f32,
                 app_version: String, date: String }
```

Segments gain `overlap: bool` (0.11, default false, omitted when false).

---

## 6. Local API after this plan

| Group | Routes | Auth |
|---|---|---|
| Scripting (unchanged) | `POST /clean`, `POST /transcribe`, `GET /history` | none, behind `api_scripting` |
| Extension (unchanged) | `/app/version`, `WS /live`, `/items/{id}/open`, `/items/{id}/export` | extension token (E6) |
| Archive (0.11) | section 4.2 | archive tokens with scopes (E14), behind `api_archive` |

The #215 guard applies to all three. The loopback-only bind is unchanged.

---

## 7. Releases and phases

Estimates are focused working time for one developer.

| Milestone | Epic issue |
|---|---|
| Phase V0 — Voices spikes | #274 |
| 0.11 — Known voices | #275 |
| 0.12 — Read aloud | #276 |
| 0.13 — Your voice, with consent | #277 |
| Track A — Accounts and stores | #278 |
| Umbrella | #146 |

Issues created 2026-09-25 (#235–#278). `agent-ready` marks issues with no
open decision or account; `needs maintainer` marks the rest.

### Phase V0 — Voices spikes (~1–1.5 weeks)

Throwaway code in the session scratchpad; results written into this file.
No private recordings of other people: CC0/CC-BY corpora, and the
maintainer's own voice only if he provides it.

- [ ] **Voice profiles across documents**: identification on Italian and
      English multi-session speakers (VoxPopuli, CC0; AMI): threshold,
      margin, minimum confirmed speech, per-condition centroids; re-check
      the #107 thresholds on Italian. (#235)
- [ ] **TTS bake-off**: Pocket TTS, Qwen3-TTS 1.7B-Base, Chatterbox v3,
      Kokoro as baseline; one no-Python runtime per engine (`ort` or
      `llama-tts` b11146); speed, memory, licences; samples for the
      maintainer's blind listening test (P18). (#236)
- [ ] **Overlap detection**: pyannote segmentation-3.0 through `ort` on AMI
      overlaps; cost; pinned SHA-256. (#237)
- [ ] **Opus**: crate, build on three OSes, crash safety, WebView playback
      matrix (macOS < 15.4 needs a decode path), decode for re-reading. (#238)
- [ ] **Teams web / Zoom web names**: desk study with the #105 method and a
      live-check checklist for #184. (#239)
- [ ] **Marking**: AudioSeal through `ort`, detection after Opus 24 kb/s,
      Vorbis tags, C2PA on supported exports. (#240)
- [ ] **Go/no-go** (#274): ≤ 2 % wrong voice suggestions at useful recall on
      Italian; one TTS engine passes the Italian listening test with a
      compatible licence and a no-Python runtime; the watermark survives
      Opus 24 kb/s.

### 0.11 — Known voices (~5–6 weeks)

- [ ] **Voice profiles**: store in app data, enrolment from confirmed lines,
      rebuild, forget (P12, P13, E13). (#241)
- [ ] **Suggestions**: speaker panel chip, People toggle, first-use sheet,
      *Forget all voices*. (#242)
- [ ] **"You" enrolment** (P14). (#243)
- [ ] **Overlap-aware Re-detect** (E19). (#244)
- [ ] **Teams web names** (E20). (#245)
- [ ] **Zoom web names** (E20). (#246)
- [ ] **Opus writer** and *Saved audio format* setting (P16, E15). (#247)
- [ ] **Opus playback, decode, *Compress audio***. (#248)
- [ ] **Archive API tokens**: scopes, hash storage, `api_archive`,
      Settings → Scripting (E14). *Agent-ready.* (#249)
- [ ] **Archive API read routes** (P15). (#250)
- [ ] **Archive API: note from text** (P15). (#251)
- [ ] **Attendees from an ICS file or link** (P22, E21). *Agent-ready.* (#252)
- [ ] Docs and release. (#253)

### 0.12 — Read aloud (~3–4 weeks)

- [ ] **Text preparation**: markdown to speakable text, Italian and English
      normalisation, chunking. *Agent-ready.* (#254)
- [ ] **Engine** per the bake-off, model download, *Models → Voices* (P18,
      E16, E22). (#255)
- [ ] **Read aloud**: speech file next to the item, Audio tab, temporary
      playback (P17). (#256)
- [ ] **Marking** of every generated file and *Check a file* (P21, E17). (#257)
- [ ] **Article links** to text items and audio (P17). (#258)
- [ ] Stretch: **two-voice podcast recipe** (P17). (#259)
- [ ] Docs and release. (#260)

### 0.13 — Your voice, with consent (~3–4 weeks, after the legal review)

- [ ] **Legal review** by a lawyer, recorded in the decision log (P20;
      maintainer). (#261)
- [ ] **Consent capture and verification** (E18). (#262)
- [ ] **Cloned voices**: store, revoke, guardrails. (#263)
- [ ] **Cloning engine** per the bake-off. (#264)
- [ ] ***Your voice*** flow (P19). (#265)
- [ ] Docs and release. (#266)
- [ ] Later, own issue after the review: cloning a consenting other person.

### Track A — Accounts and stores (parallel, maintainer-paced)

- [ ] **Privacy policy page** on the project site. *Agent-ready.* (#267)
- [ ] **Store-ready builds**, listing texts, permission justifications,
      `data_collection_permissions` (P23; related #234). (#268)
- [ ] **AMO listed** (maintainer). (#269)
- [ ] **Chrome Web Store** (maintainer). (#270)
- [ ] **Edge Add-ons** (maintainer). (#271)
- [ ] **Google Calendar OAuth** (maintainer project + code). (#272)
- [ ] **Microsoft calendar OAuth** (maintainer registration + code). (#273)

**Total**: about 13–17 weeks of focused work for 0.11–0.13 with Phase V0;
Track A depends on the maintainer's accounts and the stores' review times.

### Maintainer actions (need accounts or a decision)

| Action | For | Where |
|---|---|---|
| Record P12–P23 (confirm or change each recommendation) | every milestone | decision log below |
| Blind listening test of the bake-off samples (P18) | 0.12 | #236 |
| Lawyer's review of cloning (P20) | 0.13 | #261 |
| Read the Pocket TTS gated-weights terms before the app downloads them | 0.12/0.13 | #236 |
| Chrome Web Store: $5 fee, 2-Step Verification, trader declaration | Track A | #270 |
| Partner Center account (Edge) | Track A | #271 |
| AMO: switch or add a listed version; answer reviewers | Track A | #269 |
| Approve the privacy policy page | Track A | #267 |
| Google Cloud project, consent screen, Desktop client id, verified domain, demo video, verification | Track A | #272 |
| Entra ID app registration, Partner Program, publisher verification | Track A | #273 |

---

## 8. Risks and mitigations

| Risk | Mitigation |
|---|---|
| A wrong name suggested for a voice | Suggest-only (P12), margin over the runner-up, minimum confirmed speech, *Not X* per document |
| Voiceprints leak through a synced archive | Profiles in app data only (P13); `segments.json` embeddings are anonymous per-document vectors, and *Forget all voices* can also strip them from items on request |
| Thresholds tuned on English fail on Italian | Spike V0-1 measures both before any threshold ships; thresholds are constants with the spike's numbers in the doc comment |
| Overlap model slows long documents | Offline only ("Re-detect"), never live; cost measured in V0-3 |
| Opus decode missing in the WebView | V0-4 matrix before the default flips; WAV fallback setting |
| A TTS licence turns out to be non-commercial | Licence of code **and** weights checked in V0-2; only permissive or CC-BY weights; `licenses.json` lists downloaded models |
| GPL-3.0 phonemizer in-process ties the future commercial licence | Phonemizer in a separate process or a permissive one (E16) |
| Italian TTS quality too low | Bake-off on an Italian corpus with the maintainer listening (V0-2); English-only engines rejected as default |
| Cloning used to impersonate someone | Own voice first (P19), live consent with nonce + voice match (E18), no cloning from files or the archive, watermark + metadata (P21), revocation deletes everything |
| Legal duties change (AI Act code of practice, omnibus delays) | Legal review gate (P20); marking is on from 0.12 regardless |
| Teams/Zoom pages change | Versioned selector sets, health checks, fallback to Voice N (as Meet) |
| Google verification blocks OAuth | ICS first (P22); OAuth only after the maintainer's accounts exist |
| Store review rejects host permissions | Justifications written in advance, the `<all_urls>` path avoided, listing text says what is captured and where it goes |
| Archive API abused by a local page | #215 guard, archive routes refuse every browser Origin, scoped tokens, off by default |

---

## 9. Test matrices

- **Every release**: `cargo test`, `cargo clippy --all-targets -- -D
  warnings`, `npm run build`, `npm test`, the extension's tests and
  Playwright E2E, manual dictation check on macOS, Windows and Linux.
- **0.11**: voice suggestions on a fixed Italian + English multi-document
  corpus (true suggestions, wrong suggestions, missed voices); *Forget* removes
  the file and suggestions stop; archive API: every route with no token, a
  wrong token, a `read` token on a `write` route, a browser `Origin`, a
  foreign `Host`; Opus: a 60-minute recording on each OS plays in the
  *Audio* tab, seeks, and *Identify voices* re-reads it; a crash mid-run
  leaves a playable file; overlap: AMI clip with known overlaps, lines
  flagged; Teams web and Zoom web names on real calls (with #184).
- **0.12**: every voice on the Italian and English text corpus (numbers,
  dates, abbreviations, tables); a 30-minute document on CPU-only Linux and
  Windows; marking present in the file's tags and detected by the watermark
  detector after Opus encoding; article link end to end.
- **0.13**: consent flow: wrong phrase, replayed recording, different
  speaker, all refused; revoke deletes the voice files; a clone from a file
  or an archive item is impossible (no UI path, backend refuses); watermark
  detected on every cloned output.
- **Track A**: store builds pass each store's validator; OAuth sign-in,
  refresh and revoke against test accounts; ICS fixtures (recurring events,
  time zones, attendees without names).

---

## 10. Documentation to update

Per release: `README.md` (features, privacy: voice profiles, synthetic
audio marking, consent), `CLAUDE.md` (roadmap and standing decisions),
`docs/blog/` (voice recognition and its privacy, archive API, read aloud,
cloning with consent), `docs/manual/`, `docs/compile/*.md` (new build
dependencies such as libopus or a TTS sidecar), `sussurro/public/licenses.json`
after every dependency or downloaded-model change (`npm run licenses`), and a
privacy policy page for the stores (Track A).

---

## 11. Decision log

| Date | Decision |
|---|---|
| 2026-09-25 | Maintainer starts the future track of #146; this plan drafted with P12–P23 as proposals |

---

## Sources

Checked 2026-09-25. Approaches only; no code copied.

**TTS**
- Qwen3-TTS: https://github.com/QwenLM/Qwen3-TTS · https://huggingface.co/Qwen/Qwen3-TTS-12Hz-0.6B-CustomVoice
- llama.cpp TTS: https://github.com/ggml-org/llama.cpp/pull/26254 · https://github.com/ggml-org/llama.cpp/pull/26603 · https://github.com/ggml-org/llama.cpp/tree/master/tools/tts · https://github.com/ggml-org/llama.cpp/issues/29088
- Qwen3-TTS ONNX/Rust: https://huggingface.co/wavekat/Qwen3-TTS-0.6B-Base-ONNX · https://github.com/SuzukiDaishi/Qwen3-TTS-ONNX-Rust
- Pocket TTS: https://github.com/kyutai-labs/pocket-tts · https://huggingface.co/kyutai/pocket-tts · https://huggingface.co/kyutai/pocket-tts-without-voice-cloning · https://kyutai.org/blog/2026-05-04-pocket-tts-multilingual/ · https://lib.rs/crates/pocket-tts
- Kokoro: https://huggingface.co/hexgrad/Kokoro-82M/blob/main/VOICES.md · https://github.com/lucasjinreal/Kokoros
- Piper: https://github.com/rhasspy/piper · https://github.com/OHF-Voice/piper1-gpl/blob/main/docs/VOICES.md · https://huggingface.co/rhasspy/piper-voices/blob/main/it/it_IT/paola/medium/MODEL_CARD
- Chatterbox: https://github.com/resemble-ai/chatterbox · https://www.resemble.ai/resources/chatterbox-multilingual-v3-tts-with-embedded-watermarking-for-25-languages · https://huggingface.co/onnx-community/chatterbox-multilingual-ONNX
- Others: https://github.com/canopyai/Orpheus-TTS · https://huggingface.co/OuteAI/OuteTTS-1.0-0.6B · https://huggingface.co/FunAudioLLM/Fun-CosyVoice3-0.5B-2512 · https://github.com/Zyphra/Zonos · https://huggingface.co/neuphonic/neutts-air · https://huggingface.co/kyutai/tts-1.6b-en_fr · https://github.com/nari-labs/dia2 · https://huggingface.co/sesame/csm-1b · https://github.com/myshell-ai/MeloTTS · https://github.com/KittenML/KittenTTS · https://huggingface.co/Supertone/supertonic-2
- Non-commercial or custom licences: https://github.com/swivid/f5-tts · https://huggingface.co/coqui/XTTS-v2/blob/main/LICENSE.txt · https://huggingface.co/fishaudio/openaudio-s1-mini · https://huggingface.co/SparkAudio/Spark-TTS-0.5B · https://github.com/index-tts/index-tts/blob/main/LICENSE · https://huggingface.co/bosonai/higgs-audio-v2-generation-3B-base/blob/main/LICENSE
- Overview: https://pinggy.io/blog/best_open_source_self_hosted_text_to_speech_models/

**Law, consent and marking**
- AI Act art. 50: https://artificialintelligenceact.eu/transparency-rules-article-50/ · https://digital-strategy.ec.europa.eu/en/faqs/transparency-obligations-under-article-50-ai-act · https://linuxfoundation.eu/newsroom/ai-act-explainer · https://www.williamfry.com/knowledge/part-1-ai-act-articles-501-and-502-transparency-obligations/
- Digital Omnibus: https://lawandtechnology.eu/en/digital-omnibus-on-ai-official-journal-regulation-2026-1744/ · https://www.gibsondunn.com/eu-ai-act-omnibus-agreement-postponed-high-risk-deadlines-and-other-key-changes/
- Code of Practice: https://digital-strategy.ec.europa.eu/en/policies/code-practice-ai-generated-content · https://www.legal500.com/intelligence/european-union/technology/european-commission-publishes-final-code-of-practice-on-marking-and-labelling-ai-generated-content
- GDPR, EDPB, Garante: https://www.edpb.europa.eu/our-work-tools/our-documents/guidelines/guidelines-022021-virtual-voice-assistants_en · https://www.garanteprivacy.it/home/docweb/-/docweb-display/docweb/10207132 · https://www.garanteprivacy.it/garante/document?ID=9512226
- Italian law: https://www.studiolegalecalogiuri.it/2025/10/23/art-612-quater-c-p-illecita-diffusione-di-contenuti-generati-o-alterati-con-sistemi-di-intelligenza-artificiale/ · https://www.brocardi.it/codice-civile/libro-primo/titolo-i/art10.html
- Consent designs: https://elevenlabs.io/docs/eleven-creative/voices/voice-cloning/professional-voice-cloning · https://learn.microsoft.com/en-us/azure/ai-services/speech-service/personal-voice-create-consent · https://www.resemble.ai/our-commitment-to-consent/ · https://huggingface.co/blog/voice-consent-gate · https://www.itbrew.com/stories/2025/03/17/popular-ai-voice-cloning-softwares-lack-proper-safeguards-against-misuse-report
- Watermarks and metadata: https://github.com/facebookresearch/audioseal · https://huggingface.co/facebook/audioseal · https://github.com/resemble-ai/Perth · https://github.com/wavmark/wavmark · https://github.com/sony/silentcipher/issues/9 · https://github.com/contentauth/c2pa-rs/blob/main/docs/supported-formats.md · https://cv.iptc.org/newscodes/digitalsourcetype/

**Diarization**
- https://huggingface.co/pyannote/segmentation-3.0 · https://huggingface.co/onnx-community/pyannote-segmentation-3.0 · https://huggingface.co/pyannote/speaker-diarization-community-1 · https://huggingface.co/nvidia/diar_streaming_sortformer_4spk-v2.1 · https://www.nvidia.com/en-us/agreements/enterprise-software/nvidia-open-model-license/ · https://huggingface.co/nvidia/Nemotron-3-Diarization · https://lib.rs/crates/parakeet-rs · https://huggingface.co/BUT-FIT/diarizen-wavlm-large-s80-md

**Opus**
- https://lib.rs/crates/opusic-sys · https://github.com/restsend/opus-rs · https://crates.io/crates/ogg · https://github.com/pdeljanov/Symphonia/issues/8 · https://webkit.org/blog/16574/webkit-features-in-safari-18-4/ · https://caniuse.com/opus

**Teams and Zoom names**
- https://github.com/Vexa-ai/vexa/tree/main/core/meetings/modules/teams-capture · https://github.com/Vexa-ai/vexa/tree/main/core/meetings/modules/zoom-capture · https://github.com/Vexa-ai/vexa/issues/191 · https://www.recall.ai/blog/how-to-build-a-microsoft-teams-bot · https://www.recall.ai/blog/how-to-build-a-zoom-bot

**Calendar**
- https://developers.google.com/identity/protocols/oauth2/native-app · https://developers.google.com/workspace/calendar/api/auth · https://developers.google.com/identity/protocols/oauth2/production-readiness/sensitive-scope-verification · https://support.google.com/cloud/answer/7454865 · https://learn.microsoft.com/en-us/entra/identity-platform/reply-url · https://learn.microsoft.com/en-us/graph/permissions-reference · https://blog-en.topedia.com/2025/11/microsoft-managed-default-app-consent-policy-now-blocks-20-additional-permissions/ · https://learn.microsoft.com/en-us/entra/identity-platform/publisher-verification-overview · https://support.google.com/calendar/answer/37648 · https://developer.apple.com/documentation/technotes/tn3153-adopting-api-changes-for-eventkit-in-ios-macos-and-watchos

**Browser stores**
- https://developer.chrome.com/docs/webstore/cws-dashboard-privacy · https://developer.chrome.com/docs/webstore/program-policies/privacy · https://developer.chrome.com/docs/webstore/program-policies/trader-disclosure · https://developer.chrome.com/docs/webstore/review-process · https://learn.microsoft.com/en-us/microsoft-edge/extensions/publish/publish-extension · https://extensionworkshop.com/documentation/publish/add-on-policies/ · https://extensionworkshop.com/documentation/publish/source-code-submission/ · https://extensionworkshop.com/documentation/develop/firefox-builtin-data-consent/

**Local API prior art**
- https://www.nccgroup.com/research/technical-advisory-ollama-dns-rebinding-attack-cve-2024-28224/ · https://github.com/coddingtonbear/obsidian-local-rest-api · https://joplinapp.org/help/dev/spec/clipper_auth/ · https://www.zotero.org/support/dev/web_api/v3/local_api

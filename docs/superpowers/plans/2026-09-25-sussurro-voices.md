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
| 0.12 — Read aloud | #276 | P18 decided (Pocket TTS, file generation); engine (#255), read aloud (#256) and #264 agent-ready; marking (#257) unblocked by the watermark spike (#240, results in E17 and 4.7); the signed-metadata choice for `speech.opus` is open (4.7) |
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
  confirmed lines.** *Decided by the maintainer (2026-09-25): recommendation accepted.* A person gets a voice
  profile only when the user links a speaker to them (the existing
  `SpeakerEdit::Link`) and turns on *Recognise this voice* for that person.
  The profile is built from the embeddings of the linked lines, and a match
  on a new document is shown as a suggestion ("Voice 2 sounds like Anna —
  link?"), never applied without a click. *Recommendation: yes, with a
  minimum of 60 s of confirmed speech from at least 2 documents before a
  profile makes suggestions (to confirm in spike V0-1).* *Spike V0-1
  (#235, 4.1): 30 s from 2 documents measured almost as good as 60 s
  (0.4–2 points less recall across conditions, same wrong-suggestion
  rate), so the minimum could drop to 30 s; 60 s stays until the
  maintainer changes it.* Alternative: auto-
  apply above a high threshold. Rejected for 0.11: a wrong name in a
  transcript is worse than no name.
- **P13 — Voice profiles live in app data, not in the archive.**
  *Decided by the maintainer (2026-09-25): recommendation accepted.* Voiceprints used to identify a person are
  biometric data under GDPR art. 9 (section 4.1). The archive is often
  synced to iCloud Drive or OneDrive (P4) and is meant to be moved and
  shared, so profiles stay in `<app data>/voices/` (mode 0600), are never
  exported with the config, never go to the UI or the HTTP API as vectors,
  and are deleted with one click per person and with *Forget all voices*.
  *Recommendation: yes.* Consequence: a second machine re-learns voices
  from the archive's confirmed links (the per-line embeddings already in
  `segments.json` make this a rebuild, not a re-recording).
- **P14 — The user's own voice ("You") is a profile too.** *Decided by the maintainer (2026-09-25): recommendation accepted.* A short enrolment (read a paragraph, about 30 s)
  lets room meetings and system-audio recordings label the user's lines
  "You" even on a single channel. *Recommendation: yes, optional, offered
  in the speaker panel of a room meeting.* It is also the reference for
  cloning the user's own voice in 0.13. *Spike V0-1 (#235, 4.1): 30 s is
  enough (10 s already scores almost the same). Label the best-matching
  voice of the document, not single lines: at the voice level "You" was
  right in 100 % of AMI far-field trials. Line by line, about 7 % of the
  user's lines are missed at 2 % false "You".*
- **P15 — Archive HTTP API: read-only in 0.11, writes later.** *Decided by the maintainer (2026-09-25): recommendation accepted.* Scripts can list, search, read and export items
  and list people (names only by default) through scoped tokens; creating
  items from text or audio and editing metadata come in a later release
  after the read API has been used. *Recommendation: read-only first,* plus
  one write route, `POST /archive/items` for a note from text, because it
  is the most requested automation (clipboard or shortcut → note).
- **P16 — Saved audio becomes Opus by default.** *Decided by the maintainer (2026-09-25): recommendation accepted.* New saves are Ogg Opus mono (24 kb/s), about 10× smaller than
  today's 256 kb/s WAV; WAV stays available as a setting. Existing WAV
  items are untouched; a *Compress audio* action converts one item or all.
  *Recommendation: yes,* since the archive is often cloud-synced.
- **P17 — Read-aloud scope for 0.12.** *Decided by the maintainer (2026-09-25): recommendation accepted.* One
  narrator voice per document; output saved as `speech.opus` (or
  `speech-<recipe>.opus`) in the item folder when the user asks, otherwise
  played from a temporary file. A link to an article (URL source) becomes a
  *transcription*-like item whose text is the extracted article and whose
  audio is generated. A two-voice "podcast" recipe is a stretch goal.
  *Recommendation: yes; podcast in 0.12 only if the single-voice path is
  done early.*
- **P18 — Default TTS engine.** *Decided by the maintainer (2026-09-25),
  after the blind listening test of spike V0-2 (#236): Pocket TTS — the
  Italian 24-layer model (mean 4.40/5, first of four) and the English
  model — generating files only, not real-time playback. The English test
  is repeated after text preparation (#254), since Pocket English lost
  points on numbers and news copy (3.00 vs Chatterbox 4.50 and Qwen3-TTS
  4.00); if it still falls short, Qwen3-TTS is the English option on GPU
  Macs. Qwen3-TTS stays the cloning engine for 0.13. Optional and
  experimental per P24.* The bake-off compares Kyutai **Pocket TTS**
  (MIT code, CC-BY-4.0 weights, native Italian 24-layer model since April
  2026, 100M parameters, streaming, no phonemizer), **Qwen3-TTS 1.7B-Base**
  (Apache-2.0 code and weights, Italian among its 10 languages, but no
  Italian preset speaker, so it needs a reference clip) and **Chatterbox
  Multilingual v3** (MIT, Italian, 0.5B, watermark built in).
  *Recommendation: Pocket TTS (Italian and English models, the variant
  without voice cloning) as the default read-aloud engine, if the
  maintainer's listening test on Italian agrees; Kokoro-82M is rejected as
  default (Italian voices graded C, and Italian needs espeak-ng, GPL-3.0).*
- **P19 — Whose voice can be cloned.** *Decided by the maintainer (2026-09-25): recommendation accepted.* The
  user's own voice (enrolled in the app, P14) and a person who recorded the
  consent statement in the app, in a live session, verified (section 4.6).
  Never a voice taken from a meeting, a transcription, a link, a file or
  any archive audio, even with the owner's say-so in text. *Recommendation:
  own voice only in 0.13.0; consenting others in 0.13.x after the
  maintainer's legal review (P20).*
- **P20 — Legal review before cloning ships.** *Decided by the maintainer (2026-09-25): recommendation accepted.* The EU AI Act transparency duties (art. 50) apply from
  2 August 2026 and Italy's AI law adds a deepfake offence (section 4.7).
  *Recommendation: the 0.13 release waits on a written check by a lawyer of
  the consent text, the marking and the README wording; the plan's reading
  is not legal advice.*
- **P21 — All generated audio is marked, cloned or not.** *Decided by the maintainer (2026-09-25): recommendation accepted.* Metadata (a tag that says "synthetic speech,
  generated by Sussurro") on every file, plus an inaudible watermark on
  every file. Art. 50(2) covers any synthetic audio, not only clones.
  *Recommendation: yes, from 0.12; the watermark cannot be switched off in
  the UI.* *Spike V0-6 (#240, E17 and 4.7): AudioSeal survives the app's
  own Opus 24 kb/s and MP3 with no false positive on unmarked speech, so
  the watermark layer is settled. The Code of Practice wants the metadata
  layer **signed**; Ogg cannot carry C2PA, so a signed `speech.opus` needs
  a sidecar manifest or another container (open question in 4.7).*
- **P22 — Calendar attendees: calendar files first, OAuth later.** *Decided by the maintainer (2026-09-25): recommendation accepted.* 0.11 reads attendees from an `.ics` file or a
  private ICS link the user pastes (no account, no OAuth). Google and
  Microsoft OAuth come in Track A, only if the maintainer creates the
  accounts and accepts Google's verification process (section 4.9).
  *Recommendation: yes.*
- **P23 — Store listings.** *Decided by the maintainer (2026-09-25): recommendation accepted.* List the extension
  on the Chrome Web Store, Edge Add-ons and AMO (listed). Keep the release
  zips and the self-hosted Firefox `.xpi` for users who prefer them.
  *Recommendation: yes; AMO first (the account and signing already exist),
  then Chrome, then Edge.*

- **P24 — Text-to-speech is an experimental, optional module.** *Decided
  by the maintainer (2026-09-25).* Read aloud (0.12) and voice cloning
  (0.13) are off by default and live under Settings → Experimental; the
  rest of the app never depends on them. **No TTS or cloning model is ever
  downloaded without an explicit request by the user**: not at install,
  not in onboarding, not on first use of another feature, not in the
  background. Enabling the module shows each model's size and licence and
  downloads only after the user confirms; turning it off offers to delete
  the models. The UI labels every TTS feature "Experimental". Builds keep
  the code (no separate artefact) but ship no TTS weights.

### Engineering decisions (recommended, validated in Phase V0)

- **E13 — Voice profiles are centroids over WeSpeaker embeddings.**
  *Settled by spike V0-1 (#235, results in 4.1).* Same model as "Voice N"
  (E8, ResNet34-LM, 256-dim), so no new download. A profile keeps **one
  centroid**: the duration-weighted mean of all confirmed lines, whatever
  the recording condition. The profile also keeps the seconds per
  condition (close mic: headset or remote channel; room mic) and the
  documents the lines came from (ids only). Per-condition centroids were
  measured and dropped: they never raised recall and, scored as the best
  of several centroids, they raised wrong suggestions on room audio.
  Matching is cosine similarity of a document voice's centroid (the mean
  of its lines, as for "Voice N") to each profile. A suggestion needs
  **cosine ≥ 0.65 and a margin ≥ 0.05 over the runner-up**. When the
  voice comes from a room mic **and** the profile has room-mic lines, the
  threshold is **0.75**. Room-to-room pairs share the room's echo, which
  lifts the scores of different people. Rebuildable from the archive at
  any time (P13).
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
- **E15 — Opus through libopus, Ogg through a pure-Rust muxer.** *Settled
  by spike V0-4 (#238, results in 4.4).* Encoder and decoder: the `opus`
  crate 0.4 (MIT/Apache-2.0) over `opusic-sys` 0.7 (BSD-3, bundles libopus
  1.6.1, linked statically, built with CMake, which the app already needs
  for whisper.cpp). Container: the `ogg` crate 0.9 (BSD-3, pure Rust).
  VOIP mode, 16 kHz mono, 20 ms packets, VBR, complexity 10, **24 kb/s**
  (10.7 MB per hour instead of 115 MB). One Ogg page per second, flushed to
  the OS at each page. Transcription keeps running on the PCM during
  capture; Opus is only what gets stored. `opus-rs` is rejected for now:
  its VBR mode ignores the target bitrate (about 87 kb/s whatever is
  asked), and its decoder output is 13 samples (0.8 ms) behind libopus.
  symphonia 0.6.1 still has no Opus decoder (checked: it demuxes the Ogg
  file, then refuses the codec), so libopus decodes for replay and
  *Identify voices*. **Playback:** the `sussurro-audio:` scheme **always**
  serves Opus items as a decoded 16-bit WAV, by byte range, through the
  Ogg page index. macOS before 15.4 cannot play Ogg Opus, and macOS 11
  (Safari 16 at most) cannot play Opus in any container. On macOS 15.4+
  WebKit plays Ogg Opus but only estimates its duration (+2–3 % on speech,
  up to +19 % on silence), and `ended` fires late by the same amount.
  Serving WAV everywhere gives one code path, exact durations, the same
  `audio/wav` content type and CSP as today, and no need for GStreamer's
  Opus plugin on Linux.
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
- **E17 — Marking synthetic audio.** *Settled by spike V0-6 (#240, numbers
  in 4.7), except where the signed metadata of `speech.opus` lives, which
  the maintainer confirms in #257.* Two layers, as the Commission's Code of
  Practice on marking and labelling (final, 10 June 2026) asks:
  1. **Watermark: Meta AudioSeal 0.2, the 16-bit base models**
     (`audioseal_wm_16bits` + `audioseal_detector_16bits`). Code MIT; the
     weights are MIT too since 2 April 2024 (README news entry; Hugging
     Face card `license: mit`, not gated), so the dual-licence rule holds.
     There is no official ONNX, so we export it ourselves with PyTorch's
     tracer (opset 17) after making two padding helpers trace-friendly:
     with the input padded to a multiple of the 320-sample hop the extra
     padding is always 0, and unpadding uses negative indices, so the
     length stays dynamic (the dynamo exporter fails on the LSTM). The
     graphs use only standard ops (Conv, ConvTranspose, LSTM, Elu, shape
     ops) and match the unpatched PyTorch package to 5·10⁻⁶ (generator) and
     10⁻⁷ (detector). Generator 58.8 MB, detector 34.7 MB, through the
     app's `ort` (ONNX Runtime 1.24.2, the version rc.12 links, measured
     with the matching Python wheel; the Rust `ort` path needs only the
     usual `#[ignore]` live test in #257). **Cost** at 2 threads on an
     M-series Mac: marking 3.2–3.4 s per minute of audio, detection 1.2–2.0 s
     per minute; memory grows with the window (generator ~150 MB peak at
     1 s windows, ~260 MB at 10 s, ~1 GB for a whole minute), so both run
     on ≤ 10 s windows. Pocket TTS writes 24 kHz: the watermark is computed
     on the 16 kHz resample, upsampled and added to the 24 kHz audio
     (**"M16"**), so one 16 kHz detector reads every file the app writes,
     including its 16 kHz Opus. The 16-bit payload is a fixed Sussurro code,
     never per user or per install (it must not identify anyone). Applied
     to the PCM before any encoding, on every generated file, previews
     included; no switch; generation fails closed without the model, which
     is downloaded with the TTS models on the same explicit request (P24),
     pinned by SHA-256 from our own hosted export (the export script is a
     dev tool in the repo; Python never ships, E22).
  2. **Metadata.** Vorbis comments in every Ogg Opus file the app writes
     (`SYNTHETIC=1`, `GENERATOR=Sussurro <version>`, `ENGINE`, `VOICE`,
     `DIGITAL_SOURCE_TYPE=http://cv.iptc.org/newscodes/digitalsourcetype/trainedAlgorithmicMedia`,
     `COMMENT`) — our own `ogg` muxer writes the `OpusTags` packet, no new
     crate — and `LIST/INFO` in WAV (already done by #255). The Code asks
     for **digitally signed** metadata (sub-measure 1.1.1), and plain tags
     are not signed. C2PA (`c2pa` crate 0.91, MIT OR Apache-2.0; use the
     `rust_native_crypto` feature, not the default OpenSSL) embeds in WAV,
     FLAC, MP3 and M4A (M4A holding Opus too) for ~3.4 KB, **not in Ogg**;
     for Ogg it can write a sidecar manifest (data hash over the whole
     file) that verifies and catches a flipped byte. **Recommendation**:
     sign with a **per-install self-signed** certificate (key in the OS
     credential store, as #159) — a sidecar `speech.c2pa` next to every
     `speech.opus`, embedded in every exported WAV/FLAC/MP3/M4A. Verifiers
     show such a manifest as valid but from an unknown signer. A
     certificate on the C2PA trust list needs the C2PA conformance
     programme (legal agreement, security architecture, a certificate from
     a listed CA) and a key that the app could not keep secret on users'
     machines; the Code waives key confidentiality "in a local deployment
     scenario". Trust-list signing is revisited only if the project ever
     runs a signing service.
  3. **Verification.** *Check a file* (Voices) runs the detector, reads the
     tags and any C2PA manifest, and says which layer answered and whether
     the payload is Sussurro's (Code measures 2.1 and 2.3: free, local,
     nothing uploaded). "Made by Sussurro" = at least half the frames
     marked **and** at most 2 of the 16 payload bits wrong. The frame score
     alone is not enough (pure tones reach it, 4.7), so a high score with
     the wrong payload is reported as "inconclusive", never as another
     tool's mark; nothing found is never reported as "human". The signed
     detection result the Code offers on request (2.1.2) can come later.
  Rejected: Sony SilentCipher (weights licence unclear), Resemble Perth
  (weights licence not stated). WavMark (MIT) stays the fallback but was
  not needed. The C2PA soft-binding list has an AudioSeal entry
  (`com.aiwatermark.audioseal.1`), registered by a third party with its own
  payload and resolution API, so our manifests carry no soft-binding
  assertion that points to it.
- **E18 — Consent verification = transcript + voice match + liveness.** The
  app shows a consent statement that includes a random 6-word phrase; the
  speaker reads it live into the mic (never from a file). The recording
  passes when the local STT transcript matches the statement (word error
  rate under a threshold) and its embedding matches the reference audio to
  be cloned (E13 threshold). The consent recording, its transcript, the
  nonce, the date and the app version are kept with the voice; revoking
  deletes the voice, the reference and the consent recording together.
- **E19 — Overlap detection with pyannote segmentation through `ort`**
  (reshaped by spike #237, built in #244). The ungated
  `onnx-community/pyannote-segmentation-3.0` export (MIT, CNRS; fp32
  `onnx/model.onnx`, 5 986 908 bytes, revision `733a93b6…`, SHA-256
  `057ee564…`) runs on plain ONNX Runtime, so it goes through the existing
  `ort` next to WeSpeaker, downloaded on first use and pinned fail-closed
  (as #90). **When lines are labelled**, not in "Re-detect" (which has
  only the stored embeddings, no audio): at the end of a run with speakers
  and in "Identify voices", each line of a clustered channel is cut into
  ≤ 10 s zero-padded windows; P(overlap) per frame is the sum of the three
  pair classes of the 7-class powerset; frames ≥ 0.5 are overlap and a
  line with ≥ 300 ms and ≥ 10 % of it stores its spans in
  `segments.json`. Each span gets a **second speaker** = the nearest line
  of the same channel with another speaker (DER 17.7 → 13.4 % on AMI),
  recomputed after "Re-detect" and every speaker edit. **Overlapped lines
  stay in the clustering** (keeping them out gained nothing and lost a
  quiet speaker) and **no masked embeddings** (no gain); they are kept out
  of voice profiles only. Later upgrade to evaluate: NVIDIA Nemotron-3-Diarization
  (Sortformer v3, 23 Sep 2026, OpenMDW-1.1, up to 8 speakers), but
  `parakeet-rs` runs it on `ort` rc.13 while the app pins rc.12, and Italian
  is not among its training languages.
- **E20 — Teams and Zoom name observers follow the Meet design (#131).**
  Versioned, data-only selector sets per platform; CSRC binding where the
  platform exposes contributing sources; health checks; `names_unavailable`
  instead of a guess. Values come only from our own inspection of live
  pages (#184 method), never from other projects' lists. Per-platform
  tracker parameters (#239): hang time, mirror rule, CSRC vs SSRC, and
  lag-aware voting; the capture prerequisites (Teams on
  `teams.cloud.microsoft`, Zoom's `/wc/` iframe with `all_frames`) landed
  in #287.
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
the lines of every speaker linked to that person across the archive (a
scan of the items' `segments.json` on `person_id`), and writes one pooled
centroid (E13) with the seconds per recording condition (channel and
source kind) to `<app data>/voices/<person id>.json`.
Lines the user moved away from that speaker are excluded; lines with
overlap (4.3) are excluded. Minimums per P12.

**Suggestion.** After a run with speakers and after "Re-detect", each
"Voice N" centroid is compared with every profile. A match above the
threshold and the margin shows a chip in the speaker panel: *Sounds like
Anna · Link · Not Anna*. *Not Anna* is remembered for that document only.
Accepting links through the existing path, so participants and emails
follow as today.

**Spike V0-1 results (#235, 2026-09-25).** Method: the app's pipeline,
re-implemented outside the repo. Kaldi fbank (`kaldi-native-fbank`,
identical to `fbank.rs` on the repo's test vector), CMN, WeSpeaker
ResNet34-LM (SHA-256 as pinned), windows of ≤ 3 s per line, lines of
≤ 10 s. A profile is `T` seconds of lines from `k` documents. A test voice
is one speaker's lines in a document not used for enrolment: up to 60 s,
at least 10 s. Galleries of 20 profiles, 30 random draws. Unknown voices
come from speakers without a profile (about 3–9 per known voice). A
*wrong suggestion* is a suggested name that is not the speaker's.

| Corpus (licence) | Speakers (eligible*) | Documents | Conditions |
|---|---|---|---|
| VoxPopuli **Italian**, test + validation (CC0) | 102 (41) | plenary days, 8.3 h | clean; Opus 16 kb/s; simulated room (RT60 0.6 s, 15 dB SNR, one room per day) |
| VoxPopuli English, test + validation (CC0) | 339 (41) | plenary days | clean |
| AMI, 43 meetings of 11 series (CC BY 4.0; speaker turns from pyannote's AMI setup, Apache-2.0) | 48 (36) | meetings | headset; real far-field (Array1-01); headset → Opus 16 kb/s ("remote") |

\* eligible = ≥ 4 documents with ≥ 10 s of speech each, and ≥ 120 s in
the 3 largest; the same speakers are used in every configuration. No
audio was kept after embedding; nothing was added to the repo.

*Same condition, cosine ≥ 0.65, margin ≥ 0.05* (recall = known voices
named correctly):

| Enrolment | Italian recall / wrong | English recall / wrong | AMI headset recall / wrong |
|---|---|---|---|
| 10 s, 1 doc | 97.8 % / 0.3 % | 98.1 % / 0.1 % | 99.5 % / 0 % |
| 30 s, 2 docs | 99.7 % / 0.7 % | 99.7 % / 0.3 % | 100 % / 0 % |
| 60 s, 2 docs (P12) | 99.8 % / 0.7 % | 99.4 % / 0.3 % | 100 % / 0 % |
| 120 s, 3 docs | 99.7 % / 1.4 % | 100 % / 0 % | 100 % / 0 % |

The margin barely matters: in these data the runner-up is rarely within
0.05 of the best match. It is kept as a cheap guard for larger People
lists. The gallery size (5, 10, 20, 40 profiles) changed nothing
measurable. 0.65 is the lowest threshold at which none of the 108
configurations measured (enrolment amounts, test-voice lengths, galleries,
conditions; room-to-room aside, see below) exceeds 2 % wrong suggestions:
the worst is 2.0 % (Italian, 120 s from a single document), all others
≤ 1.5 %. Thresholds tuned per corpus alone were 0.62–0.64 (VoxPopuli) and
0.55–0.58 (AMI).

*Across conditions* (profile from close-mic lines, voice from another
condition, 0.65 / 0.05, wrong suggestions ≤ 0.2 % in every row):

| Profile → voice | Test voice ≤ 10 s | ≤ 20 s | ≤ 60 s |
|---|---|---|---|
| Italian, 10 s / 1 doc → simulated room | 72 % | 80 % | 88 % |
| Italian, 30 s / 2 docs → simulated room | 83 % | 90 % | 94 % |
| Italian, 60 s / 2 docs → simulated room | 84 % | 91 % | 94 % |
| Italian, 120 s / 3 docs → simulated room | 88 % | 94 % | 96 % |
| AMI, 30 s / 2 docs → real far-field | 87 % | 95 % | 99.5 % |
| AMI, 60 s / 2 docs → real far-field | 89 % | 96 % | 99.5 % |

Clean → Opus 16 kb/s costs under 1 point (Italian 99.2 %). Adding room
lines to the profile lifts room recall (Italian 94.7 % → 98.7 %, AMI
99.5 % → 100 %). But room-to-room matching on the simulated rooms then
needs the 0.75 threshold (at 0.65: 5.9 % wrong with one pooled centroid,
12.7 % with per-condition centroids; at 0.75: 0.1 % wrong, 94 % recall).
Real AMI rooms stayed at 0 % wrong even at 0.65. Hence E13: one pooled
centroid, 0.75 for room-to-room.

*Minimum confirmed speech.* Going from 10 s / 1 document to 30 s / 2
documents is what matters (cross-condition recall +5 to +11 points).
60 s / 2 documents adds 0.4–2 points over 30 s / 2, and 120 s / 3
documents 2–4 more on Italian. Shorter document voices mostly lower recall, not
precision, so voices keep the existing 10 s minimum
(`MIN_VOICE_SPEECH_MS`).

*Own voice (P14).* "You" = `s` seconds of the user's speech from one
document. Tested on the lines of another document recorded by one room mic
(AMI far-field: 3 other people in the room; Italian: simulated room,
3 other speakers).

| Enrolment | Per line: EER / recall at 2 % false "You" | Per voice: EER / best voice is "You" |
|---|---|---|
| AMI headset 10 s → far-field | 4.4 % / 92.4 % | 0.2 % / 100 % |
| AMI headset 30 s → far-field | 4.0 % / 93.7 % | 0.1 % / 100 % |
| AMI headset 60 s → far-field | 4.1 % / 93.3 % | 0 % / 100 % |
| AMI far-field 30 s → far-field | 4.0 % / 93.5 % | 0 % / 100 % |
| Italian clean 30 s → simulated room | 1.8 % / 98.3 % | 0.7 % / 99.8 % |

So 30 s of enrolment is enough, and more does not help. "You" should be
decided on the document's voices after clustering: the best-scoring voice
is "You" if its cosine is ≥ 0.45 (the 2 % false-"You" point was 0.40 on
AMI and 0.44 on Italian). Single lines are not reliable enough to relabel
one by one. Caveat: AMI enrolment is spontaneous speech, not a read
paragraph, and the Italian room is simulated. A check on the maintainer's
own voice (read paragraph → laptop mic in a room) is still worth doing
before 0.11.

*"Voice N" thresholds (#107) on Italian.* Synthetic documents made of the
speakers of one real document (a plenary day or an AMI meeting). Their
lines were cut into alternating turns of 1–3 lines, then run through the
ported online tracker (window assignment, majority per line) and the
offline agglomerative clustering, both followed by `fold_small`. The
figure is the speech-time error after the best mapping of voices to
speakers; *voices* is found minus true.

| Threshold | Online: Italian (3 conditions) | Online: English VoxPopuli | Online: AMI (3 conditions) | Offline: Italian | Offline: English VoxPopuli | Offline: AMI |
|---|---|---|---|---|---|---|
| 0.275 (online today) | 14.9 % | 5.9 % | 5.4 % | 20.1 % | 7.8 % | 4.8 % |
| 0.30 (re-detect today) | 11.2 % | 4.3 % | 3.5 % | 16.4 % | 4.7 % | 3.7 % |
| 0.35 | 6.3 % | 1.5 % | 2.3 % | 8.4 % | 1.6 % | 2.5 % |
| 0.375 | 4.7 % | 0.7 % | 2.1 % | 6.2 % | 1.0 % | 2.1 % |
| 0.40 | 3.1 % | 0.3 % | 2.2 % | 4.5 % | 0.6 % | 2.1 % |

At today's values Italian documents **lose voices**: two speakers merge
(−0.8 to −1.2 voices per document on the simulated room). Language and
corpus both play a part. On the same corpus (VoxPopuli), Italian
different-speaker pairs score higher than English ones (mean cosine 0.16
vs 0.11, 99th percentile 0.48 vs 0.40), so the error at the current
thresholds is about 2.5× the English one. Read parliamentary speech also
favours higher thresholds than meetings. On AMI far-field, raising the
thresholds adds voices: +0.28 per meeting at 0.275, +0.51 at 0.35, +0.67
at 0.40 after the fold. **Proposed: `ONLINE_THRESHOLD` 0.275 → 0.35,
`REDETECT_THRESHOLD` 0.30 → 0.375.** This more than halves the Italian
error and lowers the AMI error too, at the cost of an occasional extra
voice on far-field meetings; the user can merge it. The change is one constant each,
in a code PR with this table in the doc comment. A check on a real Italian
meeting recording is still wanted before it ships: the synthetic documents
have no crosstalk, overlaps or room noise between turns.

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

*Spike #237 (8 AMI test meetings, 4.2 h): frame precision 0.87 / recall
0.62 at θ 0.5, overlapped lines precision 0.95 / recall 0.67–0.72; int8
and uint8 match fp32 but are no faster on arm64, so fp32 ships; ~30 s per
hour of lines on an M1 Pro with 2 threads. Keeping overlapped lines out of
the clustering: line confusion 4.24 → 4.12 %, and a quiet speaker lost in
one meeting; masked embeddings: no gain. A second voice per overlap from
the nearest neighbouring line with another voice: DER 17.7 → 13.4 %,
missed speech in overlap halved. Thresholds are from English AMI (in-domain
for the model): re-check on Italian and laptop mics.*

What changes for the user (built in #244, the spike's shape, not the one
first planned here): lines are checked **when they are labelled** — at the
end of a run with speakers (live runs included, line by line on the
worker; ~30 s per hour) and in "Identify voices" — because "Re-detect" has
no audio. An overlapped line keeps its speaker and shows a quiet
"+ Voice 3 also speaking" next to the chip ("overlapping speech" when no
other line is within 60 s); `segments.json` stores the spans on the line
(`overlap: [{start_ms, end_ms, speaker_id?}]`, omitted when none, older
files read as none). "Re-detect" clusters overlapped lines like the others
and then names the second speakers again from the new voices; every
speaker edit (move a line, Identify voices, delete a line) does too.
Overlapped lines are left out of voice profiles (4.1) and, when a voice has
other lines, of the voice mean that suggestions and "You" match. **No line
is split** (P2: no audio cutting; the spike found the second speaker is
what helps). Transcript, exports, subtitles and the archive API keep one
speaker per line: the overlap is app metadata. Items recorded before #244
(or without the model) have no spans until they are labelled again.

### 4.4 Opus saved audio (0.11)

| Option | Licence | Build | Verdict (spike V0-4, #238) |
|---|---|---|---|
| `opus` 0.4 + `opusic-sys` 0.7.5 (libopus 1.6.1 bundled) | MIT/Apache-2.0 + BSD-3 (libopus: BSD-3 with royalty-free patent licences) | CMake, static; no runtime dependency (macOS binary links only libSystem) | **Chosen**, encoder and decoder |
| `audiopus` / `audiopus_sys` | ISC + BSD-3 | CMake, libopus 1.3 | Older libopus, not needed |
| `opus-rs` 0.1.34 (pure-Rust port of 1.6) | BSD-3 | none | Rejected for now: VBR ignores the bitrate (~87 kb/s); CBR works; decoder 13 samples late |
| `ogg` 0.9.2 | BSD-3 | none | **Chosen**, muxer and demuxer |
| symphonia 0.6.1 | MPL-2.0 | – | No Opus decoder (confirmed) |
| WebM/Matroska | – | – | Not needed: WebM Opus would only add Safari 17.4–18.3, not macOS 11; the scheme serves WAV instead |

Layout (P16): `audio.opus` or `audio-<channel>.opus`, with the same
per-channel design and t = 0 padding as the WAV files of #141. WAV stays
selectable, and the file-name pattern accepts both. *Compress audio*
converts existing WAV items and moves the originals to the OS trash only
after the Opus file decodes to the same duration.

**Writer.** 20 ms packets. The granule position counts 48 kHz samples,
pre-skip included. Pre-skip is the encoder lookahead × 3 (104 × 3 = 312 at
16 kHz). At the end, zeros flush the lookahead, and the last page's granule
is `pre_skip + samples × 3` (end trimming), so decoders return exactly the
recorded length. A page is closed every 50 packets (1 s) and the buffered
writer is flushed then. A crash loses at most the last second: in the spike
a writer killed with SIGKILL lost 0.1–0.8 s with 1 s pages and 4–4.9 s with
5 s pages. The extra page headers cost 0.7 % of the file compared with
5 s pages. An `fsync` every ~10 s covers power loss (not measured). A
truncated file has no end-of-stream page. The spike decoder and Chromium
play it up to the last complete page, and a cut at any of 300 offsets gave
no hard error. The #153 recovery only needs to accept a file without EOS;
it may drop the trailing partial page.

**Reading and seeking.** A page index is built from the headers alone:
(byte offset, end granule), 3,600 entries and about 1 ms for one hour.
Seeking starts decoding after the last page that ends before
`target − pre-roll`. Measured against a linear decode: with an 80 ms
pre-roll the worst case over 200 seeks was 29.6 dB; with **200 ms** it was
49 dB. A reader that continues from where it stopped is bit-identical to
the linear decode.

**Playback: the scheme always serves WAV.** For an `.opus` item the
`sussurro-audio:` scheme answers range requests on a virtual
`audio/wav` file (44-byte header plus 16-bit PCM). The byte range maps to a
sample range, the page index finds where to start (200 ms pre-roll), and
libopus decodes. The decoder and read position are cached per open file,
so sequential requests from the `<audio>` element are bit-exact and cheap
(40–60 ms per 1 MiB chunk, i.e. 33 s of audio). Random 64 KiB reads took a
median 1.5 ms (max about 60 ms) on one hour. Reasons, from the playback
matrix below:

| WebView | Ogg Opus | WebM Opus | Notes |
|---|---|---|---|
| WKWebView, macOS 11–15.3 | no | no on macOS 11; yes with Safari 17.4+ (macOS 12+) | caniuse; not tested here |
| WKWebView, macOS 15.4+ (tested on macOS 27) | plays, seeks, range requests | plays | Ogg duration is an estimate: 612.3 s for 600 s; `currentTime` runs on past the real end to the estimate, so `ended` comes about 24 s late in a 10-minute file |
| WebView2 (tested as Chromium 152) | exact duration (granule), seeks, `ended` on time; truncated file 35.0 s | plays; unknown-length WebM has no duration | – |
| WebKitGTK | GStreamer `oggdemux` + `opusdec` (plugins-base) | – | not tested; AppImage relies on the host's GStreamer as for WAV today |
| any, served as WAV by the scheme | – | – | exact duration, sample-accurate seeks; WKWebView and Chromium tested |

For files over 1 MiB, the current scheme answers a request without a
`Range` header with a `206` holding only the first MiB. WebKit sends a
`Range` header for WAV and Ogg, but it fetched WebM without one and
stopped at 1 MiB. Serving WAV avoids the case; the scheme should still
answer `200` with the whole body (or refuse) when there is no `Range`
header.

**Size and quality (24 kb/s recommended).** One hour of speech, 16 kHz
mono, libopus VOIP, VBR, complexity 10, measured on this Mac (Apple
Silicon). Encoding costs about 32 s of CPU per hour (≈ 110× real time,
about 1 % of a core while recording); complexity 5 halves that at the same
size. Decoding takes 1.5 s per hour.

| Bitrate | MB per hour | Whisper WER EN / IT | Transcript changed vs PCM, EN / IT | WeSpeaker cos(PCM, Opus), mean / min | Same-speaker score (PCM 0.865) |
|---|---|---|---|---|---|
| PCM (WAV) | 115.2 | 1.56 % / 12.6 % | – | 1 | 0.865 |
| 12 kb/s | – | 2.40 % / 14.5 % | 1.2 % / 6.6 % | 0.911 / 0.762 | 0.805 |
| 16 kb/s | 7.2 | 1.80 % / 14.4 % | 0.8 % / 5.5 % | 0.946 / 0.847 | 0.831 |
| **24 kb/s** | **10.7** | **1.44 % / 14.3 %** | **0.2 % / 4.2 %** | **0.975 / 0.935** | **0.851** |
| 32 kb/s | 14.2 | 1.56 % / 14.2 % | 0.5 % / 3.2 % | 0.985 / 0.932 | 0.858 |

Corpus: 73 clips, 1,866 words. English: LibriSpeech dev-clean (CC-BY-4.0,
FLAC), 41 clips from 8 speakers. Italian: Multilingual LibriSpeech test
(CC-BY-4.0), 32 clips from 8 speakers. The Italian sources are already
Opus-coded, so the Italian column is a double encode and pessimistic. STT:
whisper large-v3-turbo q5_0. Embeddings: the app's own fbank + WeSpeaker
ResNet34-LM. Speaker identification was 73/73 correct at every bitrate in
both directions (profiles from PCM tested on Opus, and the reverse); the
best other speaker stays at about 0.29. 24 kb/s is where the curve
flattens: 16 kb/s is acceptable for speech but costs embedding margin.
12 kb/s is not recommended.

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
- **What the Code of Practice asks of a provider like us** (Section 1,
  read for spike V0-6, #240). *Marking* (measure 1.1): at least **two
  machine-readable layers** for audio — digitally signed and time-stamped
  metadata (1.1.1) **and** an imperceptible watermark (1.1.2);
  fingerprinting or logging is optional and never enough alone (1.1.3).
  Key confidentiality for the signature is waived "in a local deployment
  scenario" (1.1.1). Richer provenance (system name, provider, time, model
  and version) is encouraged (1.3), without privacy-sensitive data. *Free
  and open-source* systems meet the anti-removal rule by telling users in
  the documentation not to strip the marks (1.2 b); tools that circumvent
  marks must not be promoted. *Detection* (2.1): free of charge, as a
  public specification, a library or local software (a cloud API is not
  required), one mechanism per marking technique, and on request a
  **digitally signed detection result** with at least a hash of the
  content, an identifier of the detector and a timestamp (2.1.2); results
  say which layer answered and include the information carried in the
  marks (2.3). *Quality* (3.2–3.3): low error rates on marked and unmarked
  content of varied length, robustness to recompression, format change,
  noise, filtering, cropping, time and pitch changes and the analogue hole,
  plus an adversarial assessment. *Interoperability* (3.4): open metadata
  standards now (C2PA), and a watermark interoperability solution (an
  industry access method, a public "signpost" in the content, or a shared
  detector) **by 2 February 2027**. *Compliance* (4.1–4.2): a documented
  process, proportionate for small providers; relying on open-source
  marking and detection tools is allowed if that reliance is documented,
  and testing may follow internal benchmarks until official ones exist —
  the table at the end of this section is that first internal test.
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

**Marking measured (spike V0-6, #240, 2026-09-26).** AudioSeal 16-bit via
our ONNX export on ONNX Runtime 1.24.2, "M16" marking (E17) on 22 ten-second
clips of the bake-off's Pocket TTS output (Italian 24- and 6-layer, English;
24 kHz), detector at 16 kHz. "Found" = at least half the frames marked;
"+ payload" also needs at most 2 of the 16 bits wrong. Opus through libopus
at the app's settings (VOIP, VBR, complexity 10, 20 ms), MP3 through LAME.
Watermark level: median 27.9 dB below the speech.

| Processing after marking | Found | Found + payload |
|---|---|---|
| none (16-bit WAV, 24 kHz) | 22/22 | 22/22 |
| **Opus 24 kb/s, 16 kHz (the app's writer)** | **22/22** | **22/22** |
| Opus 16 / 12 kb/s, 16 kHz | 22/22 | 22/22 |
| Opus 24 kb/s at 24 kHz | 22/22 | 22/22 |
| MP3 64 kb/s / 32 kb/s | 22/22 | 22/22 / 18/22 |
| resampled to 8 kHz and back | 22/22 | 22/22 |
| Opus 24 kb/s, then MP3 64 kb/s | 22/22 | 22/22 |
| other speech mixed in at 10 dB SNR | 17/22 | 17/22 |
| reverb (RT60 0.4 s) | 21/22 | 0/22 |
| white noise at 20 dB / 10 dB SNR | 5/22 / 0/22 | 5/22 / 0/22 |
| speed ×1.05 (pitch and tempo) | 0/22 | 0/22 |
| 1 s excerpt / Opus + 3 s / Opus + 1 s | 11/22 / 15/22 / 5/22 | 1/22 / 3/22 / 1/22 |

Nine clips of the other bake-off engines (Chatterbox, Qwen3-TTS) behaved
the same. **False positives**: on unmarked speech (the same Pocket clips
unmarked and 26 human LibriSpeech/MLS clips, each clean and after Opus,
MP3, 8 kHz, noise, reverb and a 1 s cut, 272 checks) no clip reached half
the frames (highest 0.29). A pure 440 Hz tone and a chord did, up to 0.89:
**the frame score alone is not safe on tonal audio (music)**. The payload
check removes them all: no unmarked check (320) came within 2 bits of the
payload (the closest had 4 wrong). Running the generator directly at
24 kHz ("M24", detector at 24 kHz) did as well on the codecs (22/22) and a
little better on white noise at 20 dB (7/22), with no tone false positives.
But only the 16 kHz scheme is readable by the stock AudioSeal detector as
others use it, so M16 stays. Not tested: the analogue hole (played and
re-recorded), pitch shift alone, adversarial removal (the models are
public, so a determined attacker can strip or forge the mark; the signed
metadata is what binds the file to Sussurro).

### 4.8 Teams web and Zoom web names (0.11)

Desk research, same method as #105. We studied approaches only and copied
no selector values; ours must come from our own inspection of live pages
during the #184 manual QA.

The desk study #239 (closed; its comment is the reference) refined this:

- **Teams web** delivers one server-mixed remote audio stream **with
  per-participant CSRCs** (several at once = overlap), plus a redundant
  track carrying the same CSRCs. The DOM gives the names: a voice-level
  outline under a `data-tid` stream wrapper that trails the audio by about
  1 s, lights on typing too, and is missing in one of the UI variants
  Teams serves in the same tenant. Captions carry an author attribute but
  depend on the tenant allowing captions.
- **Zoom web client** runs the meeting in a same-origin `/wc/` iframe and
  has two transports: in WebRTC mode one RTP stream per participant (the
  SSRC is the participant), in WASM mode (older, and the automatic
  fallback) no receivers at all, so attribution is temporal only. The
  active-speaker marker lags, flickers and sticks to the presenter during
  a screen share; the name is in the avatar footer. Caption markup has no
  names.
- Both got the Meet treatment (E20) in #245/#246: one observer engine with
  a profile per platform — versioned data-only selector sets
  (`teams-2026-09a`, `zoom-2026-09a`, placeholders of the documented
  shapes, **unverified until #184**), CSRC binding on Teams (mirror rule
  off, 800 ms hang), SSRC binding on Zoom (the page timeline in WASM
  mode), **lag-aware votes** (inside a turn only, two turns or one ≥ 3 s),
  the user's tile learned from our mic, a Zoom screen-share pause, a Teams
  outline-coverage health check, and "Voice N" when a hook misses. Named
  speakers are `teams:<name>` / `zoom:<name>` (Meet and older items keep
  `meet:`); the app shifts the Teams/Zoom page timeline by 1 s. A missing
  DOM signal never gates audio.

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
// As built in #241 (E13): the file existing = *Recognise this voice* on.
struct VoiceProfile { version: u32, person_id: String,
                      model: String /* "wespeaker-resnet34-lm" */,
                      centroid: Vec<f32> /* one pooled, L2-normalised; empty = no lines yet */,
                      speech_ms: u64, close_ms: u64, room_ms: u64,
                      documents: Vec<String> /* ids only, sorted */, updated: String }

// settings — archive API tokens (hash only)
struct ArchiveToken { id: String, name: String, sha256: String,
                      scopes: Vec<Scope> /* Read | People | Write */,
                      created: String, last_used: Option<String> }

// archive — frontmatter additions (app-owned keys, kept in ItemMeta::extra)
//   audio: [audio.opus]            0.11, Opus or WAV
//   speech: [speech.opus]          0.12
//   synthetic: { engine, voice, marked: [metadata, watermark, c2pa] }   0.12
//     watermark = AudioSeal 16-bit, fixed Sussurro payload (E17)

// <app data>/voices/you.own-voice.json  (0600; #243, P14) — the user's own voice
struct OwnVoiceProfile { version: u32, model: String, centroid: Vec<f32>,
                         speech_ms: u64, label_as_you: bool, updated: String }
// segments.json speakers: DocSpeaker.own_voice: Option<bool>
//   true = labelled "You" by the match; false = the user took it off

// <app data>/voices/cloned/<voice id>/  (0.13)
struct ClonedVoice { id: String, owner: VoiceOwner /* You | Person(id) */,
                     engine: String, reference: String /* file name */,
                     consent: Consent, created: String }
struct Consent { statement: String, nonce: String, recording: String,
                 transcript: String, wer: f32, similarity: f32,
                 app_version: String, date: String }
```

Segments gain `overlap: Vec<OverlapSpan>` (0.11, #244; `OverlapSpan {
start_ms, end_ms, speaker_id? }` on the session clock, `speaker_id` = the
second speaker; empty and omitted unless the line is overlapped — a
non-empty list is the flag).

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

- [x] **Voice profiles across documents**: identification on Italian and
      English multi-session speakers (VoxPopuli, CC0; AMI): threshold,
      margin, minimum confirmed speech, per-condition centroids; re-check
      the #107 thresholds on Italian. (#235; results in 4.1)
- [x] **TTS bake-off**: Pocket TTS, Qwen3-TTS 1.7B-Base, Chatterbox v3,
      Kokoro as baseline; one no-Python runtime per engine (`ort` or
      `llama-tts` b11146); speed, memory, licences; samples for the
      maintainer's blind listening test (P18, decided). (#236)
- [x] **Overlap detection**: pyannote segmentation-3.0 through `ort` on AMI
      overlaps; cost; pinned SHA-256. (#237) Results in 4.3 and E19: GO
      with fp32, flag + second voice from the neighbouring line, no
      exclusion from the clustering.
- [x] **Opus**: crate, build on three OSes, crash safety, WebView playback
      matrix (macOS < 15.4 needs a decode path), decode for re-reading. (#238)
      Results in 4.4 and E15: libopus through `opus`/`opusic-sys`, 24 kb/s,
      1 s pages, the scheme always serves WAV. The Windows MSVC and Linux
      builds and the WebView2/WebKitGTK cells are checked by #247's CI and
      the 0.11 test matrix.
- [ ] **Teams web / Zoom web names**: desk study with the #105 method and a
      live-check checklist for #184. (#239)
- [x] **Marking**: AudioSeal through `ort`, detection after Opus 24 kb/s,
      Vorbis tags, C2PA on supported exports. (#240) Results in 4.7 and
      E17: own ONNX export of AudioSeal 16-bit (MIT code and weights),
      watermark at 16 kHz added to the 24 kHz output, survives the app's
      Opus 24 kb/s and MP3; Vorbis comments; C2PA with a per-install
      self-signed key (sidecar for Ogg).
- [ ] **Go/no-go** (#274): ≤ 2 % wrong voice suggestions at useful recall on
      Italian; one TTS engine passes the Italian listening test with a
      compatible licence and a no-Python runtime; the watermark survives
      Opus 24 kb/s.

### 0.11 — Known voices (~5–6 weeks)

- [ ] **Voice profiles**: store in app data, enrolment from confirmed lines,
      rebuild, forget (P12, P13, E13). (#241)
- [ ] **Suggestions**: speaker panel chip, People toggle, first-use sheet,
      *Forget all voices*. (#242)
- [ ] **"You" enrolment** (P14): read-aloud enrolment, best voice ≥ 0.45 labelled "You" in single-channel documents. (#243)
- [ ] **Overlap-aware speaker labels** (E19 as reshaped by #237): spans
      found when lines are labelled, second speaker per span, recomputed by
      Re-detect and speaker edits, overlapped lines out of profiles. (#244)
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
| Confirm the signed metadata of `speech.opus` (sidecar `speech.c2pa` with a per-install key, or no signature on the Ogg file) and host the pinned AudioSeal ONNX export (e.g. a release asset) | 0.12 | #257 |
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
| Thresholds tuned on English fail on Italian | Spike V0-1 measured both (4.1): identification holds on Italian at the same threshold; the "Voice N" clustering thresholds merge Italian speakers and should rise (0.275 → 0.35, 0.30 → 0.375); thresholds are constants with the spike's numbers in the doc comment |
| Overlap model slows long documents | Line by line when lines are labelled, ~30 s per hour of lines with 2 threads (#237); one model load per run, and a model that fails to load only turns the check off |
| Opus decode missing in the WebView | V0-4: the scheme always serves decoded WAV, so no WebView ever sees Opus; WAV save setting stays |
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
  *Audio* tab with its exact duration, seeks, and *Identify voices*
  re-reads it; a crash mid-run leaves a playable file that loses at most
  the last second; overlap: AMI clip with known overlaps, lines
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
| 2026-09-25 | Spike V0-4 (#238): libopus via `opus` 0.4 / `opusic-sys` 0.7, `ogg` 0.9, 24 kb/s VOIP, 1 s pages; `opus-rs` rejected for now; the `sussurro-audio:` scheme always serves Opus items as decoded WAV |
| 2026-09-26 | Spike V0-6 (#240): AudioSeal 16-bit (MIT code + weights) exported to ONNX by us and run through `ort`; watermark computed at 16 kHz and added to the 24 kHz TTS output; Vorbis comments + C2PA signed with a per-install self-signed key (embedded where C2PA supports the format, sidecar for Ogg — to confirm in #257); WavMark not needed |

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
- Watermarks and metadata: https://github.com/facebookresearch/audioseal · https://huggingface.co/facebook/audioseal · https://github.com/resemble-ai/Perth · https://github.com/wavmark/wavmark · https://github.com/sony/silentcipher/issues/9 · https://github.com/contentauth/c2pa-rs/blob/main/docs/supported-formats.md · https://cv.iptc.org/newscodes/digitalsourcetype/ · Code of Practice text (PDF, 10 June 2026): https://ec.europa.eu/newsroom/dae/redirection/document/129555 · https://opensource.contentauthenticity.org/docs/conformance/trust-lists/ · https://opensource.contentauthenticity.org/docs/signing/get-cert/ · https://github.com/c2pa-org/softbinding-algorithm-list · https://crates.io/crates/c2pa

**Diarization**
- https://huggingface.co/pyannote/segmentation-3.0 · https://huggingface.co/onnx-community/pyannote-segmentation-3.0 · https://huggingface.co/pyannote/speaker-diarization-community-1 · https://huggingface.co/nvidia/diar_streaming_sortformer_4spk-v2.1 · https://www.nvidia.com/en-us/agreements/enterprise-software/nvidia-open-model-license/ · https://huggingface.co/nvidia/Nemotron-3-Diarization · https://lib.rs/crates/parakeet-rs · https://huggingface.co/BUT-FIT/diarizen-wavlm-large-s80-md

**Opus**
- https://lib.rs/crates/opusic-sys · https://github.com/restsend/opus-rs · https://crates.io/crates/ogg · https://github.com/pdeljanov/Symphonia/issues/8 · https://webkit.org/blog/16574/webkit-features-in-safari-18-4/ · https://caniuse.com/opus · https://caniuse.com/webm · https://www.rfc-editor.org/rfc/rfc7845 (Ogg Opus: pre-skip, granule, end trimming, 80 ms pre-roll)
- Spike corpora: https://huggingface.co/datasets/openslr/librispeech_asr · https://huggingface.co/datasets/facebook/multilingual_librispeech (both CC-BY-4.0) · model https://huggingface.co/Wespeaker/wespeaker-voxceleb-resnet34-LM

**Teams and Zoom names**
- https://github.com/Vexa-ai/vexa/tree/main/core/meetings/modules/teams-capture · https://github.com/Vexa-ai/vexa/tree/main/core/meetings/modules/zoom-capture · https://github.com/Vexa-ai/vexa/issues/191 · https://www.recall.ai/blog/how-to-build-a-microsoft-teams-bot · https://www.recall.ai/blog/how-to-build-a-zoom-bot

**Calendar**
- https://developers.google.com/identity/protocols/oauth2/native-app · https://developers.google.com/workspace/calendar/api/auth · https://developers.google.com/identity/protocols/oauth2/production-readiness/sensitive-scope-verification · https://support.google.com/cloud/answer/7454865 · https://learn.microsoft.com/en-us/entra/identity-platform/reply-url · https://learn.microsoft.com/en-us/graph/permissions-reference · https://blog-en.topedia.com/2025/11/microsoft-managed-default-app-consent-policy-now-blocks-20-additional-permissions/ · https://learn.microsoft.com/en-us/entra/identity-platform/publisher-verification-overview · https://support.google.com/calendar/answer/37648 · https://developer.apple.com/documentation/technotes/tn3153-adopting-api-changes-for-eventkit-in-ios-macos-and-watchos

**Browser stores**
- https://developer.chrome.com/docs/webstore/cws-dashboard-privacy · https://developer.chrome.com/docs/webstore/program-policies/privacy · https://developer.chrome.com/docs/webstore/program-policies/trader-disclosure · https://developer.chrome.com/docs/webstore/review-process · https://learn.microsoft.com/en-us/microsoft-edge/extensions/publish/publish-extension · https://extensionworkshop.com/documentation/publish/add-on-policies/ · https://extensionworkshop.com/documentation/publish/source-code-submission/ · https://extensionworkshop.com/documentation/develop/firefox-builtin-data-consent/

**Local API prior art**
- https://www.nccgroup.com/research/technical-advisory-ollama-dns-rebinding-attack-cve-2024-28224/ · https://github.com/coddingtonbear/obsidian-local-rest-api · https://joplinapp.org/help/dev/spec/clipper_auth/ · https://www.zotero.org/support/dev/web_api/v3/local_api

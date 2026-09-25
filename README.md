# Sussurro

**A local-first speech-to-text workbench for Windows, macOS and Linux.**
Dictate into any app with a hotkey, and turn voice notes, audio files, links
and meetings into markdown documents in your own archive. Transcription runs
on your computer (whisper.cpp, NVIDIA Parakeet or Qwen3-ASR), and cleanup and
summaries use a local LLM: Ollama, any OpenAI-compatible server, or a model
Sussurro runs itself. Nothing leaves your machine unless you send it
to a server yourself.

> **Hold `Ctrl+Shift+Space` (⌘⇧Space on Mac), speak, release.**
> The cleaned-up text appears wherever your cursor is.

🌐 Project site: [`docs/index.html`](docs/index.html) · 📖 User manual:
[`docs/manual/`](docs/manual/index.html) · 📰 Guides:
[`docs/blog/`](docs/blog/index.html) · 🛠️ Building & contributing:
[`docs/development.md`](docs/development.md) · 📝 What's new in 0.10:
[`docs/releases/0.10.0.md`](docs/releases/0.10.0.md)

## Why Sussurro

- **Local, private by design.** Audio is captured and transcribed on your
  device. No account, no telemetry, no cloud service. Text leaves the machine
  only when you choose an **external LLM profile**, and Sussurro asks first.
  See [Privacy](#privacy).
- **Your files, not a database.** Every note, transcription and meeting is a
  folder of plain markdown in `Documents/Sussurro`: readable, editable and
  searchable without Sussurro.
- **Dictation that works in every app.** The result is pasted into whatever
  has focus (your clipboard is restored), so there's nothing to integrate.
- **Free and open.** AGPL-3.0, no subscription.

## What it does

The app is a workspace with a left rail: **New · Library · People · Recipes ·
Models · Settings**.

### Dictation (hotkey)

- **Hold or toggle a global shortcut**, speak, and the cleaned text is pasted
  into the focused app. The Dictate button at the bottom of the rail does the
  same without the keyboard. A small overlay shows recording and
  transcribing, with a live preview.
- **Cleanup levels** None / Light / Medium / High remove fillers, fix
  punctuation and adapt tone, with a fallback to the raw transcript if the
  model is unreachable. Fillers are recognised per language (Italian “ehm”,
  English “um”…).
- **Streaming typing** (word by word, or sentence by sentence with cleanup),
  **translation** into another language, **per-app tone styles**, **voice
  snippets**, spoken **formatting commands** (“new line”, “a capo”), a
  **self-learning dictionary**, **whisper mode** for quiet dictation, and
  **dictate to file** (append to a note instead of pasting).
- Dictations are not Library items. Their **history** (search, re-clean,
  translate, retention, export to Markdown or JSON) is under
  Settings → Dictation history.

### Notes, files and links → the archive

- **New → Microphone** records a **note**. Tick *Meeting in the room* for
  several people on one mic, told apart by voice.
- **New → File** transcribes wav, mp3, m4a, aac, flac or ogg as a **note**
  (your own voice memo) or a **transcription** (someone else's recording).
  Files are decoded as a stream, so hour-long recordings fit.
- **New → Link** downloads a direct media link, or a video page through
  [`yt-dlp`](https://github.com/yt-dlp/yt-dlp) if you have it installed, and
  transcribes it as a **transcription**. The download is temporary.
- Long recordings are cut at pauses (Silero voice-activity detection),
  transcribed and cleaned segment by segment while you talk, and saved after
  every segment, so a crash leaves an *Interrupted* item, not nothing. A hotkey
  dictation during a long session still goes first.
- **The archive** lives in `Documents/Sussurro` (changeable in
  Settings → Archive): one folder per item with `transcript.md` (YAML
  frontmatter: type, title, date, source, language, tags, categories,
  participants…), companion documents, optional subtitles and audio. The
  frontmatter is the source of truth. Edit it in any editor and your edit
  wins. The search index is derived and can be rebuilt.
- **The Library** searches titles, text and tags and filters by type and by
  **tag, category, participant and date**. It marks items that are
  recording, interrupted, edited outside, have audio, or were sent to an
  external LLM.
- **Export** as `.md` or `.txt`, and for meetings and transcriptions as
  **`.srt` / `.vtt` subtitles**. Subtitles are created when you ask
  (default) or automatically on every save (Settings → Archive).

### Recipes, Ask and LLM profiles

- **Recipes** turn a transcript into a companion document next to it:
  *Formatted document* (tl;dr, headings, tables), *Summary*, *Action items*,
  *Decisions*, and, on transcripts with named speakers, *Meeting minutes* and
  *Who said what*. Duplicate one or write your own. Long transcripts are
  processed in chunks and merged.
- **Ask** answers free questions about the open document. Answers are saved
  only if you click *Save as document*.
- **LLM profiles** (Recipes screen) pair a server with a model: **Ollama** or
  any **OpenAI-compatible** server (llama.cpp `llama-server`, LM Studio, DS4,
  vLLM, hosted `/v1` services). One profile does cleanup, and recipes and
  Ask pick one per run. **API keys are kept in the OS keychain.**
- **Local (bundled)**: a built-in profile that runs **Qwen3 1.7B**
  (Apache-2.0, ~2.2 GB, downloaded when you ask) in Sussurro's own
  `llama-server`. You get cleanup and recipes with nothing else installed.
  It is offered when no cleanup server answers, never selected for you,
  starts on first use and stops after 15 idle minutes.
- **Privacy gate**: a profile whose server is not on this machine is
  *external*. Recipes and Ask on it ask for confirmation **every time**, and
  cleanup on it needs a standing opt-in. See [Privacy](#privacy).

### Meetings

- **Web meetings**: the [browser extension](#browser-extension-meetings)
  records Google Meet, Microsoft Teams and Zoom in the browser (Chrome,
  Edge, Brave, Firefox) and streams them to the app, which transcribes
  live.
- **Desktop calls**: **New → System audio + mic** records your microphone
  and the computer's sound as two channels. The computer's sound comes
  through **native loopback** (WASAPI on Windows, a Core Audio tap on
  macOS 14.2+, the PulseAudio/PipeWire monitor on Linux) or any **virtual
  audio device** (BlackHole, VB-Cable, a monitor source).
- **Speakers**: your microphone is always **You**. On Google Meet, remote
  lines take the participants' **names from the page**. Everyone else is
  told apart by voice as **Voice 1, Voice 2…**, on this computer, with a
  small speaker model (WeSpeaker, downloaded on first use). Rename voices,
  link them to people, or **Re-detect speakers** over the whole call.
  Transcriptions can do the same with *Identify voices*.
- **People**: a registry of names, emails and aliases. Participants whose
  name matches get the person's email automatically, and the registry
  travels with the archive.
- **Voice map**: every line as a dot placed by how the voice sounds, so
  lines that sound alike sit together. Click a dot to find its line.
- **Saved audio and replay**: tick *Save audio* (off by default) to keep a
  16 kHz WAV per channel next to the transcript. The **Audio** tab replays
  it with the transcript highlighted, and *Play only* plays one speaker's
  lines back to back.
- **Speaker-aware recipes**: *Meeting minutes* (with an *Action | Owner |
  Due* table) and *Who said what* keep every statement with its speaker.
  Participants go to the LLM by name only, unless you tick *Include
  participant emails* for that run.

### Speech engines

Choose on the **Models** screen:

- **Whisper** (whisper.cpp; GPU through Vulkan on Windows and Metal on
  macOS, CPU on Linux by default): any language, from Base English 148 MB
  to Large v3 Turbo 574 MB.
- **NVIDIA Parakeet TDT v3** (a single 456 MB int8 model, CPU-optimized,
  auto-detects 25 European languages).
- **Qwen3-ASR 1.7B** (optional, ~2.5 GB): runs in the bundled
  `llama-server` and detects the language itself. It takes no dictionary
  prompt, and Whisper large-v3-turbo stays more accurate on Italian, so it
  is never the default.

## Getting started

Download the installer for your OS from the
[releases](https://github.com/fullo/Sussurro/releases). The first launch
opens a short **guided setup**:

1. **Permissions**: microphone, and on macOS Accessibility (to paste into
   other apps).
2. **Archive folder**: `Documents/Sussurro` by default. On macOS this is
   when the system asks for access to Documents, once.
3. **A speech model**: download one Whisper model, or pick Parakeet.
4. **Cleanup**: Sussurro looks for a local server (Ollama on port 11434,
   LM Studio on 1234, a llama.cpp server on 8080). If none answers, it
   offers the **bundled model**, or you can install
   [Ollama](https://ollama.com) (`ollama pull llama3.2:3b`). Without any,
   you get the raw transcript.
5. **Your shortcut**.

Every step can be skipped. Settings → About → *Run the setup again* reopens
it. Upgrading from 0.6 shows a single **What's new** screen instead, and
your settings carry over (see the [release notes](docs/releases/0.10.0.md)).

Guides for each feature are on the [blog](docs/blog/index.html). Building
from source: [`docs/development.md`](docs/development.md).

## Requirements

| | Windows | macOS | Linux |
|---|---|---|---|
| System | Windows 10/11, x64; WebView2 (built into Windows 11) | macOS 11+ on **Apple Silicon** (no Intel build) | glibc ≥ 2.38: Ubuntu 24.04+, Debian 13+, Fedora 39+ |
| Paste into apps | nothing extra | Accessibility permission | a clipboard helper: `xclip`/`xsel` (X11) or `wl-clipboard` (Wayland); on Wayland the RemoteDesktop portal, or `wtype`/`ydotool` |
| GPU | Vulkan (any modern driver) | Metal | CPU by default ([optional Vulkan build](docs/compile/linux.md)) |
| System audio + mic, built-in | WASAPI loopback, nothing to install | **macOS 14.2+**; asks for *System Audio Recording* once | `pactl` + `parec` from **`pulseaudio-utils`** (PulseAudio or PipeWire) |
| System audio + mic, otherwise | VB-Cable, Voicemeeter or Stereo Mix | BlackHole or Loopback | a monitor source exposed as an ALSA device |
| LLM API keys | Credential Manager | Keychain | a Secret Service keyring (GNOME Keyring, KWallet…); without one the key stays in `settings.json` |

**Optional, on every OS:**

- An LLM server for cleanup, recipes and Ask: Ollama, LM Studio, a llama.cpp
  server, or any OpenAI-compatible service. Alternatively, the bundled model
  needs nothing.
- [`yt-dlp`](https://github.com/yt-dlp/yt-dlp) for links to video platforms
  (`brew install yt-dlp`, `winget install yt-dlp.yt-dlp`, `pipx install
  yt-dlp`). Sussurro finds it on your PATH or in the usual package-manager
  folders. Direct media links need nothing.
- Chrome, Edge or Brave 116+, or Firefox 128+, for the meetings extension.
- **Headphones** for calls: Sussurro does not cancel echo, so on speakers
  your microphone also records the others.

**The first launch on macOS**: the app is not notarized, so right-click it →
*Open* once ([why](docs/blog/macos-signing-gatekeeper.html)). If the
Qwen3-ASR engine or the bundled model then fails to start because macOS
quarantined its helper, run `xattr -cr /Applications/sussurro.app` once (the
error message says so). Sussurro never removes the quarantine flag itself.

Per-OS details, including the virtual-device setup step by step, are in
[`docs/compile/`](docs/compile/) and the
[system audio guide](docs/blog/system-audio.html).

## Browser extension (meetings)

The Sussurro browser extension records web meetings — **Google Meet,
Microsoft Teams and Zoom in the browser** — and streams them to the Sussurro
app on the same computer, which transcribes them live into a *Meeting* in
your Library. It works in Chrome, Edge and Brave (116 or later) and Firefox
(128 or later). Meetings need no switch in the app: pair the extension and
it can record.

### Install

The extension is not in the browser stores yet. Every
[release](https://github.com/fullo/Sussurro/releases) carries two zips,
`sussurro-extension-chrome-<version>.zip` and
`sussurro-extension-firefox-<version>.zip`; use the one with the same
version as your app.

- **Chrome, Edge, Brave — load unpacked**: unzip it into a folder you keep,
  open `chrome://extensions` (`edge://extensions`, `brave://extensions`),
  turn on **Developer mode**, click **Load unpacked** and pick that folder.
  To update, replace the folder's content and press the reload icon on the
  extension's card.
- **Firefox — temporary add-on**: open `about:debugging#/runtime/this-firefox`,
  click **Load Temporary Add-on…** and pick the zip. Firefox removes a
  temporary add-on when it quits, so load it again after a restart.

In either browser, if the extension has no access to a meeting site its
panel offers **Allow access**.

Building it from source: [`extension/README.md`](extension/README.md).

### Pair it with the app

1. In Sussurro, open **Settings → Browser extension**. The extension talks to
   the app through the local API on `127.0.0.1`: the card says whether it is
   listening, and offers to turn it on (it applies when Sussurro restarts).
2. Click **Copy pairing code** — one string with the port and a secret token.
3. Open the extension's options (right-click its toolbar button →
   *Options*), paste the code and click **Save and test**.

Treat the code like a password. **Regenerate token…** in the same card
invalidates the old one at once; every browser you paired must then be
paired again.

### What it captures

- **Nothing until you press Start recording** in the extension's side panel
  (Chrome) or sidebar (Firefox) on the meeting tab. While it records, the
  toolbar button shows a red **REC** badge on that tab; Stop, closing the tab
  or leaving the page ends the recording, and the app keeps what it
  received.
- **Two channels**: your microphone (what the page sends) is always
  **You**; everyone else (the call's incoming audio, mixed) is told apart by
  voice as **Voice 1, Voice 2…**, which you can rename or link to People in
  the document. The page's own audio is never changed or muted.
- The audio goes **only to the Sussurro app on this computer**; the side
  panel mirrors the live transcript, and offers *Open in Sussurro*, *Copy as
  text* and (with subtitles *on request*) *Create .srt*. A `.wav` is saved
  only if *Save audio* is on in Settings. If the connection drops, the extension reconnects and the rest
  of the call becomes a new item.

### Meet names

On **Google Meet** the extension also reads the participants' names from the
page and matches them to who is speaking, so remote lines are named instead
of *Voice N*, and the participants join the item's frontmatter (with their
email if they are in People). It reads the page's structure only, never
clicks or opens panels, and when it can't read the names reliably it stops
and says so rather than guess: those lines stay *Voice N*. Meet changes its
page often, so this is the part most likely to need an update. **Teams and
Zoom** get *You* and *Voice N* only.

### Privacy and consent

The extension sends audio and page events only to the Sussurro app at
`127.0.0.1`, using the pairing token; the app refuses web pages even when
they know the token, and nothing goes to any other server. It has no
analytics. It asks for storage, access to the meeting sites and to
`127.0.0.1`, and on Chrome the side panel plus the tab-capture fallback
(used only when a page plays the call audio in a way the extension can't
otherwise reach).

Recording a meeting records other people: read
[Recording meetings and consent](#recording-meetings-and-consent). The side
panel shows the same notice as the app before your first recording.

## Privacy

Speech-to-text always runs on your device, and the default LLM profile is
Ollama on this machine. An LLM profile whose server is not on this machine
(anything but `localhost`, a loopback address or a `.local` host — or one you
mark *external* by hand) is **external**: text sent to it leaves your
computer. Sussurro never falls back from a local profile to an external one.
The built-in *Local (bundled)* profile is always local: Sussurro's own
`llama-server`, which it can't be pointed away from. That server (and the
Qwen3-ASR one) is reachable only by Sussurro: a Unix socket in a private
folder on macOS and Linux, a loopback port checked to belong to it on
Windows, and a random key per start that every request must carry.

**Links** (New → Link) never reach this computer or your local network
unless you tick *Allow local network addresses* for that run — checked on
every address a host resolves to and every redirect. yt-dlp is used only for
known video sites (YouTube, Vimeo, SoundCloud…), without its generic "any
page" extractor, and every connection it makes goes through the same check.

- **Recipes and Ask questions** on an external profile show a confirmation
  **every time**: which document, roughly how much text (characters and
  tokens), which server and which model. Nothing is sent until you click
  *Send*; Cancel sends nothing. The app enforces this in the backend, not
  just in the UI: a run on an external profile needs a one-time token issued
  for that exact run (item, recipe or question, server, model), valid once
  and for two minutes.
- **Cleanup** (hotkey dictation, microphone sessions, file transcriptions,
  re-clean, translate and the local API's `/clean`) has no moment for a
  dialog, so an external cleanup profile needs a **persistent opt-in** in
  Settings → Cleanup ("Send dictations to *host* for cleanup"), stored per
  profile and tied to its server — pointing the profile elsewhere turns it
  off. Without the opt-in, cleanup keeps the **raw text** and sends nothing.
- **The Library marks** every item whose text went to an external server
  (*↗ Sent externally*, with the hosts in the tooltip). The marker comes from
  each item's `.sussurro/external-log.json` — date, host, profile, model and
  recipe of every external send, **never the content** — and from the
  provenance of its generated documents, which record `external: true` and
  the `host` in their frontmatter.
- Anything sent to an external server is processed under that provider's
  terms; over plain `http` it also travels unencrypted.

### Recording meetings and consent

Meeting recordings capture other people's voices: the browser extension's
*Start recording*, *System audio + mic* and *Meeting in the room*. Depending
on where you and the other participants are, and on your organisation's
rules, you may need to tell them that you are recording, and some may have to
agree first. Sussurro can't tell which rules apply to you, so it doesn't try:
before the first such recording, the app and the extension's side panel show
a short notice saying so (hide it with *Don't show this again*; bring it back
from Settings → Browser extension or the extension's options), and a
"Recording other people" line stays visible while you record. The audio is
transcribed on this computer and is not sent anywhere; a saved `.wav` is
kept only if you ask for it.

### Where your data lives

Everything stays on this computer, as ordinary files under your user
account:

| What | Where |
|---|---|
| Notes, transcriptions, meetings (text, companion documents, subtitles, saved audio) | the archive, `Documents/Sussurro/YYYY/MM/<date>-<title>/` (Settings → Archive) |
| People registry | `<archive>/.sussurro/people.json` (travels with the archive) |
| Settings, including LLM profiles and the extension's pairing token | `settings.json` in the app's config folder (readable only by you on macOS and Linux) |
| LLM profile API keys | the OS credential store, service `com.sussurro.app` |
| Dictation history and usage stats | `history.jsonl`, `stats.json` in the app's data folder |
| Search index (rebuildable), paths of files kept for *Identify voices*, temporary link downloads | `archive-index.sqlite`, `source-files.json`, `link-downloads/` in the app's data folder |
| Speech, speaker and bundled LLM models | `models/` in the app's data folder, or the Models folder you choose |

*Delete…* and *Delete audio…* move files to the OS trash. The
app's config and data folders follow each OS's convention (for example
`~/Library/Application Support/com.sussurro.app` on macOS). The
[data guide](docs/blog/where-data-lives.html) has more.

## Local API (scripting)

Enable it in Behavior → Advanced: switch on **Local API** (loopback only;
applied at restart) and **Scripting routes** (applies at once). Both are off
on a new install. *Turn on the local API* in Settings → Browser extension
(for pairing) never turns on the scripting routes. If you used the API
before the switch existed, it stays on for you. Then, from any script:

```bash
# clean up / translate a text with your current settings
curl -X POST --data "um so this is uh a test" http://127.0.0.1:4525/clean

# transcribe an audio file (wav/mp3/m4a/flac/ogg)
curl -X POST --data-binary @meeting.mp3 "http://127.0.0.1:4525/transcribe?ext=mp3"

# search your dictation history
curl "http://127.0.0.1:4525/history?n=10&q=sussurro"
```

Loopback-only means no network exposure, but any process on your machine can
call the scripting routes without a token — that's why they ship disabled.
Web pages can't use them: every route accepts only `Host: 127.0.0.1:<port>`
or `localhost:<port>` (so a site that points its own name at your machine —
DNS rebinding — is refused), and any request carrying a browser `Origin`
other than a browser extension's is refused. Bodies are capped (1 MiB for
`/clean`, 200 MiB for `/transcribe`, `413` beyond); at most two
`/clean`/`/transcribe` requests run at once (`503` with `Retry-After`
otherwise), so a long transcription never stalls the extension.

The same API serves the [browser extension](#browser-extension-meetings):
its routes (`/app/version`, `/live`, `/items/…`) always need the pairing
token and accept only browser-extension origins, never a web page, and
`/items/…` reach only the meetings the extension recorded — not your notes,
dictations or other transcriptions. `settings.json`, which holds the pairing
token, is readable only by your user on macOS and Linux.

## Removed in 0.10

- **Command mode**, the second hotkey that applied a spoken instruction to
  the selected text, is gone. Use your OS voice control instead: Voice
  Control on macOS, Voice Access on Windows 11. Spoken formatting commands
  *inside* a dictation (“new line”, “a capo”, “scratch that”) still work.
- **The classic single-window UI** is gone. The workspace is the only UI,
  and its old preview switch is ignored.
- **The flat cleanup settings** (server, model, API key) became the *Local*
  LLM profile on upgrade. See the [release notes](docs/releases/0.10.0.md).

## Known limits

- **Speaker labels are approximate.** *Voice N* clustering was tuned on
  English recordings. Two similar voices can merge, and one voice can split.
  Rename, move lines, or *Re-detect speakers*. **Meet names** depend on
  Meet's page, which changes often. When the extension can't read the names
  it says so and falls back to *Voice N*. Teams and Zoom get *You* and
  *Voice N* only.
- **No echo cancellation.** Record calls with headphones, or the others'
  voices can land on your *You* channel too.
- **Built-in system audio** follows the output device that was the default
  when the recording started. If you switch outputs mid-call, start a new
  recording. On macOS it needs 14.2 or later, and on Linux `pulseaudio-utils`.
  The Windows loopback has not been verified on real hardware yet.
- **Links**: video pages work only on known platforms, through `yt-dlp`.
  Videos whose only audio is Opus or AC-3 are refused, because no ffmpeg is
  bundled. Links are capped at 2 GB and ignore system proxy settings (so the
  local-network check can't be bypassed).
- **The browser extension is not in the stores yet.** Firefox loads it as a
  temporary add-on, which is removed when Firefox quits.
- **Linux Wayland injection** goes through the XDG **RemoteDesktop portal**
  first (zero setup on KDE/GNOME; the OS asks for consent on first use, and
  KDE may ask again after a reboot, kde#480235). Fallbacks: `wtype`,
  `ydotool`, enigo. See [issue #40](https://github.com/fullo/Sussurro/issues/40).
- **Linux builds are CPU-only by default** (Vulkan needs the SDK; Windows uses
  Vulkan, macOS uses Metal). An opt-in Vulkan build is documented in
  [`docs/compile/linux.md`](docs/compile/linux.md).
- **macOS is Apple Silicon only** (min 11.0): ONNX Runtime has no prebuilt
  binaries for Intel Macs.
- **Installers aren't OS-code-signed.** macOS builds are ad-hoc signed, so
  Gatekeeper needs a one-time right-click → *Open* (see
  [the blog](docs/blog/macos-signing-gatekeeper.html)). Windows shows a
  SmartScreen prompt (*More info → Run anyway*), and an antivirus may ask
  about `sussurro-llama-server.exe` the first time it runs. The **updater**
  artifacts are always signed with the project's own key, independent of OS
  signing.
- **Cleanup output is pasted without review.** The cleaned text goes straight
  into the focused app, with no step between the model and your cursor. By
  design the model only ever generates text and never triggers actions, so
  instructions carried by a dictation or a personal-dictionary entry can at
  most change what gets pasted, not what Sussurro does.
- **The global hotkey works over password fields too.** It fires wherever
  focus is, including another app's password field, and starts recording
  there (as designed). The audio goes only to local STT, and the text only to
  your cleanup profile (an external one only with your opt-in).

## Documentation

- [`docs/releases/0.10.0.md`](docs/releases/0.10.0.md): what's new in 0.10
  and upgrade notes.
- [`docs/manual/`](docs/manual/index.html): the user manual, screen by screen.
- [`docs/blog/`](docs/blog/index.html): a guide for each feature.
- [`docs/development.md`](docs/development.md): build from source, tests, CI,
  the sidecar and the extension.
- [`docs/compile/`](docs/compile/): per-OS build guides (Windows/macOS/Linux),
  including system-audio setup.
- [`docs/releases.md`](docs/releases.md): release process, auto-update,
  code-signing.
- [`extension/README.md`](extension/README.md): working on the browser
  extension.
- [`CLAUDE.md`](CLAUDE.md): roadmap and standing project decisions.

## License

Copyright © 2026 Francesco Fullone (DarumaHQ).

Sussurro is free software licensed under the **GNU Affero General Public
License v3.0 or later** ([AGPL-3.0-or-later](LICENSE)). You may use, study,
share and modify it — but any distributed derivative, **and any network
service built on it**, must make its complete corresponding source available
under the same license. This keeps Sussurro open and prevents it from being
turned into a closed, proprietary product.

The copyright is held by the author, so a separate **commercial license** can
be granted on request for anyone who needs to use Sussurro outside the AGPL's
terms — contact [DarumaHQ.it](https://darumahq.it).

Bundled third-party components keep their own (permissive/compatible) licenses;
see the in-app About dialog or [`sussurro/public/licenses.json`](sussurro/public/licenses.json).
The browser extension lists its own (React, `webextension-polyfill`) in the
About section of its options page
([`extension/src/options/licenses.json`](extension/src/options/licenses.json)).

### Third-party binaries

The installers include one prebuilt program that Sussurro does not compile:
**`sussurro-llama-server`**, the unmodified `llama-server` from a pinned
[llama.cpp](https://github.com/ggml-org/llama.cpp) release (MIT; currently
`b11146` — Metal on macOS, Vulkan on Windows, CPU on Linux) with its shared
libraries in `llama-server-libs/`. It runs the optional extra speech
engines (Qwen3-ASR) and the optional *Local (bundled)* LLM profile (Qwen3
1.7B), each as its own local process that only Sussurro can reach (see
[Privacy](#privacy)), only when you choose them; their models download when
you pick them. The release and the SHA-256 of
every upstream archive are committed in
[`sussurro/src-tauri/sidecar/llama-server.lock.json`](sussurro/src-tauri/sidecar/llama-server.lock.json),
and the build refuses any file that doesn't match. The Windows build also
ships the LLVM OpenMP runtime (`libomp.dll`, Apache-2.0 WITH
LLVM-exception) that llama.cpp needs. Like Sussurro itself these binaries
are not OS-code-signed, so an antivirus may ask about
`sussurro-llama-server.exe` the first time it runs.

---

Made by [DarumaHQ.it](https://darumahq.it).

# Sussurro

**Fully-local voice dictation for Windows, macOS and Linux.** A Wispr Flow
alternative with no cloud: your voice never leaves your machine. whisper.cpp
for speech-to-text, a local LLM (via Ollama) for AI cleanup, paste-injection
into any app.

> **Hold `Ctrl+Shift+Space` (⌘⇧Space on Mac), speak, release.**
> The cleaned-up text appears wherever your cursor is.

🌐 Project site: [`docs/index.html`](docs/index.html) · 🛠️ Building & contributing:
[`docs/development.md`](docs/development.md)

## Why Sussurro

- **100% local, private by design.** Audio is captured, transcribed and cleaned
  entirely on your device. No account, no telemetry, no network round-trip —
  it works on a plane.
- **AI cleanup, not just transcription.** A small local model removes fillers,
  fixes punctuation and adapts tone — with graceful fallback to the raw
  transcript if the model isn't running.
- **Works in every app.** The result is pasted into whatever has focus (your
  clipboard is restored), so there's nothing to integrate.
- **Free and open.** No subscription.

## How it works

```
global hotkey (press / release)
  → microphone capture (cpal, resampled to 16 kHz mono)
  → local STT: whisper.cpp (GPU: Vulkan/Metal) or NVIDIA Parakeet TDT v3
    (ONNX, CPU-optimized ~10x faster than Whisper without a GPU)
  → local LLM cleanup — None / Light / Medium / High,
    falls back to the raw transcript if the model is unreachable
  → clipboard-paste injection into the focused app (clipboard restored)
  → local JSONL history
```

The research behind the design:
[`docs/whisperflow-clone-research.md`](docs/whisperflow-clone-research.md).

## Getting started

Download the installer for your OS from the
[releases](https://github.com/fullo/Sussurro/releases), then:

1. **[Ollama](https://ollama.com)** running locally, with a small instruct
   model — this powers the AI cleanup:
   ```
   ollama pull llama3.2:3b
   ```
   Sussurro still works without it — you just get the raw transcript (set
   Cleanup to "None", or let the automatic fallback handle it).
2. **A Whisper model** — pick one in Settings and click *Download*
   (Base English 148 MB → Large v3 Turbo 574 MB). Or switch to the Parakeet
   engine for a single CPU-optimized model.
3. **A microphone.**

First run opens on Settings: pick a model, click *Download*, set your shortcut
with the click-to-record hotkey widget, and dictate.

Building from source instead? See
[`docs/development.md`](docs/development.md).

## Features

### Dictation
- **Two STT engines** — Whisper (GPU, any language, multiple sizes) or NVIDIA
  Parakeet TDT v3 (single 456 MB int8 model, CPU-optimized, auto-detects 25
  European languages). Switch in Settings → Engine.
- **Language** — pick your dictation language or auto-detect; a fixed language
  is more accurate on smaller multilingual models.
- **Streaming typing** — text is typed while you speak: word by word with
  Cleanup None, or sentence by sentence with cleanup on (each completed
  sentence is LLM-cleaned before it's typed; the final pass finishes the tail).
- **Live preview** — the overlay shows a rolling partial transcript while you
  speak; the pasted text always comes from the final full-quality pass.
- **Whisper mode** — dictate quietly: 3× mic gain and a lower silence gate.
- **Command mode** — select text anywhere, hold the command shortcut (default
  `Ctrl+Alt+Space`) and speak an instruction ("make it shorter", "translate to
  English"): the LLM applies it and the result replaces the selection.

### Cleanup & tone
- **Cleanup levels** — None / Light / Medium / High, editable in Cleanup →
  Advanced (override the built-in instructions; empty = defaults).
- **Translation** — dictate in one language, get the cleaned text in another
  (works even with Cleanup None — translate-only). Something Wispr Flow can't
  do locally.
- **Per-app tone styles & language** — rules like `slack → "casual, emojis
  welcome"` adapt the cleanup prompt to whatever app you dictate into; each
  rule can force its own output language, overriding the global "Translate to".
- **Voice snippets** — say a cue exactly (e.g. "firma email") and Sussurro
  pastes the snippet's full text instead of transcribing.
- **Voice commands** — say "a capo" / "new line" (and paragraph/bullet
  variants) for deterministic line breaks with no LLM involved; contextual
  commands like "scratch that" ride the cleanup prompt.
- **Self-learning dictionary** — correct a history entry and the words you fix
  are added to your personal dictionary automatically, Wispr-style.

### History & workflow
- **History** — hover an entry to Copy, Re-clean, Translate or Edit it;
  full-text search over raw + cleaned text; retention auto-deletes entries
  older than N days (0 = keep forever); export to Markdown or JSON.
- **Usage statistics** — persistent total / today / last-7-days dictation and
  word counts; clearing history never resets them.
- **Dictate to file** — note-taking mode: append every dictation to a
  `.md`/`.txt` file (e.g. an Obsidian note) instead of pasting into the app.
- **Audio file transcription** — feed a wav/mp3/m4a/flac/ogg recording through
  the same STT + cleanup pipeline.
- **Portable config** — export/import dictionary + snippets + app styles as a
  JSON file to move your setup between machines (import merges, no duplicates).

### Interface
The UI follows the **Daruma design system**: warm paper surfaces, ink text,
and daruma-red reserved for the moment that matters — the daruma "eye" next to
the wordmark is hollow when idle and painted red while recording.

- **Dictate button** — the header status pill is a live button: hold it
  (push-to-talk) or click it (toggle) to dictate without the keyboard.
- **Recording overlay** — a small floating pill near the bottom of the screen
  while recording (red, pulsing) and transcribing (spinner). Always on top,
  never steals focus.
- **Sound feedback** — a rising tick when recording starts, a falling one when
  it stops (toggle in Settings).
- **Microphone selector + VU meter** — pick the capture device (falls back to
  the system default if unplugged); a live input-level bar helps you test it.
- **Tray** — left-click to show/hide; closing the window hides to tray.
- **Setup banner** — lists anything missing (Ollama not running, model not
  downloaded) with a one-click fix.
- **Copy diagnostics** — a footer button copies version + OS + configuration
  for bug reports (configuration only — never dictated text or dictionary).

## Local API (scripting)

Enable it in Behavior → Advanced (off by default; loopback only; applied at
restart). Then, from any script:

```bash
# clean up / translate a text with your current settings
curl -X POST --data "um so this is uh a test" http://127.0.0.1:4525/clean

# transcribe an audio file (wav/mp3/m4a/flac/ogg)
curl -X POST --data-binary @meeting.mp3 "http://127.0.0.1:4525/transcribe?ext=mp3"

# search your dictation history
curl "http://127.0.0.1:4525/history?n=10&q=sussurro"
```

Loopback-only means no network exposure, but any process on your machine can
call it — that's why it ships disabled.

## Roadmap

Current version **0.4.1**. Full detail (and standing decisions) in
[`CLAUDE.md`](CLAUDE.md).

- **0.5.0 — go public**: macOS Developer ID signing + notarization, the repo
  goes public (which unfreezes auto-update), Flatpak + Flathub distribution.
- **0.6.0 (candidate)**: backend-agnostic cleanup via the OpenAI-compatible
  `/v1/chat/completions` API — drive cleanup with any local runtime (Ollama,
  llama.cpp-server, LM Studio, …) instead of only Ollama's native schema.

## Known limits

- **Linux Wayland injection** goes through the XDG **RemoteDesktop portal**
  first (zero setup on KDE/GNOME; the OS asks for consent on first use — KDE
  may re-ask after a reboot, kde#480235). Fallbacks: `ydotool`, `wtype`,
  enigo. See [issue #40](https://github.com/fullo/Sussurro/issues/40).
- **Linux builds are CPU-only by default** (Vulkan needs the SDK; Windows uses
  Vulkan, macOS uses Metal). An opt-in Vulkan build is documented in
  [`docs/compile/linux.md`](docs/compile/linux.md).
- **macOS is Apple Silicon only** (min 11.0) — ONNX Runtime has no prebuilt
  binaries for Intel Macs.
- **Installers aren't OS-code-signed yet.** macOS builds are ad-hoc signed, so
  Gatekeeper needs a one-time right-click → *Open* (see
  [the blog](docs/blog/macos-signing-gatekeeper.html)); Windows shows a
  SmartScreen prompt (*More info → Run anyway*). Signing is on the roadmap
  (Windows via SignPath; macOS Developer ID later). The **updater** artifacts
  are always signed with the project's own key, independent of OS signing.

## Documentation

- [`docs/development.md`](docs/development.md) — build from source, tests, CI.
- [`docs/compile/`](docs/compile/) — per-OS build guides (Windows/macOS/Linux).
- [`docs/releases.md`](docs/releases.md) — release process, auto-update,
  code-signing.
- [`docs/index.html`](docs/index.html) — the project landing page (+ `docs/blog/`).

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

---

Made by [DarumaHQ.it](https://darumahq.it).

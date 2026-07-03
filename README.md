# Sussurro

Fully-local voice dictation for Windows, macOS and Linux — a Wispr Flow clone
with no cloud: whisper.cpp for speech-to-text, Ollama for AI cleanup,
paste-injection into any app.

**Hold `Ctrl+Shift+Space` (⌘⇧Space on Mac), speak, release.** The cleaned-up
text appears wherever your cursor is.

## How it works

```
global hotkey (press/release)
  → microphone capture (cpal, resampled to 16 kHz mono)
  → local STT: whisper.cpp (GPU: Vulkan/Metal) or NVIDIA Parakeet TDT v3
    (ONNX, CPU-optimized ~10x faster than Whisper without a GPU)
  → Ollama /api/chat cleanup — None / Light / Medium / High,
    falls back to the raw transcript if Ollama is unreachable
  → clipboard-paste injection into the focused app (clipboard restored)
  → local JSONL history
```

Research behind the design: `docs/whisperflow-clone-research.md`.

## Runtime requirements (all platforms)

1. **[Ollama](https://ollama.com)** running locally, with a small instruct model:
   ```
   ollama pull llama3.2:3b
   ```
   Sussurro still works without Ollama — you just get the raw transcript
   (set Cleanup to "None", or let the automatic fallback handle it).
2. **A Whisper model** — pick one in Settings and click *Download*
   (Base English 148 MB → Large v3 Turbo 574 MB).
3. **A microphone.**

## Building from source

Toolchain (all platforms): **Node.js ≥ 24** and **Rust stable**. Follow the
guide for your OS — prerequisites, GPU notes and platform-specific caveats:

- **[Windows 10/11](docs/compile/windows.md)** — Vulkan GPU build,
  MAX_PATH workaround, WebView2
- **[macOS 11+](docs/compile/macos.md)** — Apple Silicon only, Metal GPU,
  required permissions
- **[Linux](docs/compile/linux.md)** — glibc ≥ 2.38, apt dependencies,
  X11/Wayland notes

The short version, once the prerequisites are in place:

```bash
git clone https://github.com/fullo/Sussurro && cd Sussurro/sussurro
npm install
npm run tauri dev      # development, hot-reload
npm run tauri build    # production bundle (installer per platform)
```

First run: the window opens on Settings — pick a Whisper model, click
*Download*, set your shortcut with the hotkey recorder, and dictate.

## UI

The interface follows the **Daruma design system**: warm paper surfaces, ink
text, and daruma-red reserved for the moment that matters — the daruma "eye"
next to the wordmark is hollow when idle and painted red while recording.
The shortcut is set via a click-to-record hotkey widget (press the actual
combination; Esc cancels).

- **Dictate button** — the status pill in the header is a live button: hold it
  (push-to-talk) or click it (toggle mode) to dictate without touching the
  keyboard. It follows the same Push-to-talk setting as the shortcut.
- **Sound feedback** — a rising tick when recording starts and a falling one
  when it stops, so the trigger is audible even with the window hidden
  (toggle in Settings).
- **Recording overlay** — a small floating pill near the bottom of the screen
  while recording (red, pulsing) and transcribing (spinner). Always on top,
  never steals focus, disappears when idle.
- **Live preview** — while you speak, the overlay shows a rolling partial
  transcript, re-transcribed every ~1.2 s from the growing buffer. The pasted
  text always comes from the final full-quality pass (toggle in Settings).
- **Tray** — left-click the tray icon to show/hide the window (menu on
  Linux); closing the window hides to tray.
- All sections are collapsible and remember their state; the Cleanup card
  reads the installed model list live from your Ollama server.
- **History actions** — hover an entry to Copy the cleaned text or Re-clean
  the raw transcript with the current cleanup level (appends a new entry).
- **Language** — pick your dictation language (or auto-detect) in Settings;
  a fixed language is more accurate on smaller multilingual models.
- **Voice snippets** — say a cue exactly (e.g. "firma email") and Sussurro
  pastes the snippet's full text instead of transcribing. Matching ignores
  case and punctuation; the AI cleanup is skipped.
- **Self-learning dictionary** — Edit a history entry to correct it: words
  you introduce (real misspelling fixes, not case-only changes) are added to
  your personal dictionary automatically, Wispr-style.
- **Two STT engines** — Whisper (GPU, any language, multiple sizes) or
  NVIDIA Parakeet TDT v3 (single 456 MB int8 model, CPU-optimized,
  auto-detects 25 European languages). Switch in Settings → Engine.
- **Per-app tone styles** — Wispr-style tone matching: rules like
  slack → "casual, emojis welcome" adapt the cleanup prompt to whatever app
  you dictate into (detected at the moment you release the trigger).
- **Command mode** — select text anywhere, hold the command shortcut
  (default `Ctrl+Alt+Space`) and speak an instruction ("make it shorter",
  "translate to English"): the LLM applies it and the result replaces the
  selection.
- **Whisper mode** — dictate quietly: 3x mic gain and a lower silence gate.
- **Streaming typing** (experimental) — with Cleanup None, text is typed into
  the app while you speak, holding back the last two words until the final
  pass completes them.
- **Models folder** — settable in Settings (default: app data); silence is
  trimmed before inference (VAD-lite) so long pauses don't cost GPU time.

## Releases & auto-update

> **⚠️ Auto-update is frozen until v0.5.0.** The repo is private while the
> per-platform builds stabilize, so installed apps cannot reach `latest.json`
> anonymously anyway (the endpoint 404s). The in-app **Check for updates**
> button and the signing pipeline below keep working — releases are simply
> not reachable by the updater until the repo (or at least its releases)
> goes public, planned for v0.5.0.

Pushing a `v*` tag (e.g. `git tag v0.2.1 && git push origin v0.2.1`) triggers
the GitHub Actions release workflow: signed installers for Windows, macOS
(Apple Silicon) and Linux, published as a draft release together with
the updater manifest (`latest.json`). The in-app **Check for updates** button
(footer) downloads and installs the new version.

### Updater signing key (one-time setup)

The updater artifacts are signed with a minisign key **kept outside the
repo**. If you don't have the key yet, generate one (any OS):

```bash
cd sussurro
npm run tauri signer generate -- -w ~/.tauri/sussurro-updater.key
```

This prints the public key — it must match `plugins.updater.pubkey` in
`sussurro/src-tauri/tauri.conf.json` (changing the key pair orphans every
previously installed app, which would no longer trust new releases).

Then upload the **private** key to the repo secrets, from the directory
where the key lives.

macOS / Linux:

```bash
gh secret set TAURI_SIGNING_PRIVATE_KEY < ~/.tauri/sussurro-updater.key
gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD --body '""'
```

Windows (PowerShell):

```powershell
gh secret set TAURI_SIGNING_PRIVATE_KEY --body (Get-Content $HOME\.tauri\sussurro-updater.key -Raw)
gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD --body '""'
```

(The password secret is the passphrase chosen at key generation; `'""'`
means "empty passphrase". Back the key file up — losing it means a new key
pair and orphaned installs.)

## Known limits

- Streaming typing is experimental and only works with Cleanup level None;
  with cleanup enabled the text lands after you release the hotkey.
- Linux Wayland: paste injection falls back to `wtype` when enigo fails —
  install it (`sudo apt install wtype`); untested on real Wayland yet.
- Linux builds are CPU-only by default (Vulkan needs the SDK; Windows uses
  Vulkan, macOS uses Metal — see the Cargo.toml target-specific deps).
- cpal is pinned to 0.16 (0.18 has a windows-core version conflict with the
  Tauri stack on Windows).

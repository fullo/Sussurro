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
  → whisper.cpp transcription (whisper-rs, fully offline)
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

## Setup per platform

### Windows 10/11

Build prerequisites (one-time, elevated PowerShell):

```powershell
winget install Rustlang.Rustup OpenJS.NodeJS.LTS Kitware.CMake LLVM.LLVM Ollama.Ollama
winget install Microsoft.VisualStudio.2022.BuildTools --override "--passive --wait --add Microsoft.VisualStudio.Workload.VCTools;includeRecommended"
winget install KhronosGroup.VulkanSDK   # GPU acceleration (whisper.cpp Vulkan backend)
setx LIBCLANG_PATH "C:\Program Files\LLVM\bin"
```

GPU notes:
- Transcription runs on the GPU via **Vulkan** (NVIDIA/AMD/Intel alike). The
  Vulkan SDK is needed at *build* time only; end users just need a Vulkan
  driver (any modern GPU driver ships one).
- `src-tauri/.cargo/config.toml` pins the cargo target dir to a short path
  (`F:/sbuild`): whisper.cpp's Vulkan shader sub-build otherwise exceeds
  Windows' 260-char MAX_PATH and MSBuild fails with FTK1011. Adjust the path
  to your drive, or delete the file on macOS/Linux.
- The very first transcription on a machine compiles GPU shaders (~10 s,
  one-time); the driver caches them afterwards.

Then open a **new** terminal and build (see *Build & run* below).

Runtime notes:
- Settings → Privacy & security → Microphone → enable **"Let desktop apps
  access your microphone"**.
- WebView2 is preinstalled on Windows 11; on older Windows 10 install the
  [WebView2 runtime](https://developer.microsoft.com/microsoft-edge/webview2/).

### macOS 13+

Build prerequisites:

```bash
xcode-select --install                 # Xcode Command Line Tools (clang, git)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
brew install node cmake ollama
```

Runtime notes — macOS will prompt for two permissions on first use; both are
required:
- **Microphone** (System Settings → Privacy & Security → Microphone)
- **Accessibility** (System Settings → Privacy & Security → Accessibility) —
  needed to synthesize the ⌘V paste into other apps. If text never appears,
  re-check this permission for Sussurro (or your terminal, in dev mode).

### Linux (X11 recommended)

Build prerequisites (Debian/Ubuntu — adapt package names for your distro):

```bash
sudo apt update
sudo apt install build-essential curl cmake clang pkg-config \
  libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev \
  librsvg2-dev libasound2-dev libxdo-dev libxkbcommon-dev
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
# Node LTS: use your distro package or https://github.com/nvm-sh/nvm
curl -fsSL https://ollama.com/install.sh | sh
```

Runtime notes:
- **X11**: everything works out of the box (hotkey + paste injection).
- **Wayland**: paste injection relies on enigo's experimental Wayland support
  and may not work in all compositors; global shortcuts may also be
  restricted. Workarounds: run the app under XWayland, or switch to an X11
  session. Native Wayland injection (wtype/ydotool) is on the roadmap.
- Audio uses ALSA (`libasound2`); PipeWire and PulseAudio expose ALSA
  compatibility by default.

## Build & run (all platforms)

```bash
git clone https://github.com/fullo/Sussurro && cd Sussurro/sussurro
npm install
npm run tauri dev      # development, hot-reload
npm run tauri build    # production bundle (installer per platform)
cd src-tauri && cargo test   # headless test suite (27 tests)
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
- **Tray** — left-click the tray icon to show/hide the window (menu on
  Linux); closing the window hides to tray.
- All sections are collapsible and remember their state; the Cleanup card
  reads the installed model list live from your Ollama server.

## Known limits (v1)

- No streaming transcription yet — text lands after you release the hotkey.
- Linux Wayland injection is experimental (see above).
- Linux builds are CPU-only by default (Vulkan needs the SDK; Windows uses
  Vulkan, macOS uses Metal — see the Cargo.toml target-specific deps).
- cpal is pinned to 0.16 (0.18 has a windows-core version conflict with the
  Tauri stack on Windows).

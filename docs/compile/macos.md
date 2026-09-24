# Building on macOS 11+ (Apple Silicon only)

Intel Macs are not supported: the Parakeet engine's ONNX runtime (`ort`)
ships no prebuilt binaries for `x86_64-apple-darwin`. The bundle targets
macOS 11.0+ (whisper.cpp needs ≥ 10.15 for `std::filesystem`; arm64 raises
that to 11.0).

## Prerequisites

```bash
xcode-select --install                 # Xcode Command Line Tools (clang, git)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
brew install node cmake ollama
```

Transcription runs on the GPU via **Metal** — no extra setup needed.

## Build & run

```bash
git clone https://github.com/fullo/Sussurro && cd Sussurro/sussurro
npm install
npm run tauri dev      # development, hot-reload
npm run tauri build    # production bundle (.dmg + .app)
cd src-tauri && cargo test   # headless test suite
```

### The llama-server sidecar (release bundles)

Release bundles ship a pinned upstream `llama-server` (llama.cpp, Metal) for
the optional Qwen3-ASR engine (plan E9). It is not in git: fetch it once, and
again whenever `src-tauri/sidecar/llama-server.lock.json` changes — the
script downloads the pinned asset, checks its SHA-256 and refuses anything
else:

```bash
npm run sidecar                                               # → src-tauri/binaries/
npm run tauri build -- --config src-tauri/tauri.sidecar.conf.json
```

Without the `--config` the bundle simply has no sidecar; `cargo test`,
clippy and a plain `tauri dev` never need it (add the same `--config` to
`tauri dev` to get it in `target/debug/`). The executable lands in
`Contents/MacOS/sussurro-llama-server`, its dylibs in
`Contents/Resources/llama-server-libs/`; they are upstream's ad-hoc
(linker-)signed files, unmodified, and run in the ad-hoc-signed app. The
app starts it with the lib folder as working directory and
`DYLD_LIBRARY_PATH` (no hardened runtime, so dyld honours it — if Developer
ID signing + notarization ever land, the dylibs must move next to the
binary instead). A quarantined copy (downloaded DMG) should be covered by
the one-time right-click → *Open* of the app; if the engine then fails to
start, `xattr -cr /Applications/sussurro.app` clears the flag.

## Runtime notes

macOS will prompt for two permissions on first use; both are required:

- **Microphone** (System Settings → Privacy & Security → Microphone)
- **Accessibility** (System Settings → Privacy & Security → Accessibility) —
  needed to synthesize the ⌘V paste into other apps. If text never appears,
  re-check this permission for Sussurro (or your terminal, in dev mode).

If you run a downloaded, unsigned build (e.g. a CI artifact) and macOS
reports it as damaged, clear the quarantine flag:
`xattr -cr /Applications/sussurro.app`.

### System audio + mic: a loopback device

*New → System audio + mic* (meetings preview, Settings → Browser extension)
records a call from a desktop app — Zoom, Teams, anything that plays through
the computer — as two channels: your microphone ("You") and a second input
device that carries the computer's sound (the others, told apart as
"Voice 1, Voice 2…"). Sussurro reads any input device; the OS needs a
virtual device that turns the output into an input.

1. Install [BlackHole](https://github.com/ExistentialAudio/BlackHole)
   (`brew install blackhole-2ch`, free) or Rogue Amoeba's Loopback.
2. Open **Audio MIDI Setup** → **+** → *Create Multi-Output Device*, tick
   your speakers or headphones **and** BlackHole 2ch (drift correction on
   BlackHole), and choose it as the sound output (System Settings → Sound,
   or in the meeting app). You keep hearing the call; BlackHole gets a copy.
   The volume keys don't work on a Multi-Output Device — set the volume on
   the real output.
3. In Sussurro choose **BlackHole 2ch** as the system audio device (devices
   whose names look like loopback devices are listed first).

Microphone permission covers both devices. The two devices have separate
clocks: Sussurro realigns them when they drift more than 200 ms apart and
says so in the session.

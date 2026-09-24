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
start, `xattr -cr /Applications/sussurro.app` clears the flag (the error
message says so). The app never strips `com.apple.quarantine` itself: an
unapproved app runs translocated from a read-only copy, so it could not,
and removing Gatekeeper's flag from inside the app is not something an
ad-hoc-signed app should do behind the user's back — the manual step stays
the user's choice.

The sidecar runs only while the Qwen3-ASR engine is loaded, on a random
`127.0.0.1` port; it stops with the idle model unload (15 min), an engine
change and on quit. macOS has no way to tie a child to its parent's life,
so if Sussurro itself crashes or is force-quit while the engine is loaded,
`sussurro-llama-server` can stay behind: quit it from Activity Monitor.

## Runtime notes

macOS will prompt for two permissions on first use; both are required:

- **Microphone** (System Settings → Privacy & Security → Microphone)
- **Accessibility** (System Settings → Privacy & Security → Accessibility) —
  needed to synthesize the ⌘V paste into other apps. If text never appears,
  re-check this permission for Sussurro (or your terminal, in dev mode).

If you run a downloaded, unsigned build (e.g. a CI artifact) and macOS
reports it as damaged, clear the quarantine flag:
`xattr -cr /Applications/sussurro.app`.

### System audio + mic: this computer's sound

*New → System audio + mic*
records a call from a desktop app — Zoom, Teams, anything that plays through
the computer — as two channels: your microphone ("You") and a second input
source that carries the computer's sound (the others, told apart as
"Voice 1, Voice 2…").

**macOS 14.2 or later: nothing to install.** Choose **This computer's sound
(built-in)** (preselected). Sussurro opens a Core Audio *process tap* on
every app's output except its own, read through a private aggregate device
clocked by the current output — no virtual device, no change to your sound
setup, and you keep hearing the call as usual. The first time, macOS asks
to allow Sussurro to record system audio (`NSAudioCaptureUsageDescription`
in the app's `Info.plist`); the switch lives in System Settings → Privacy &
Security → **Screen & System Audio Recording** (*System Audio Recording
Only*). If it is off, the tap records silence — Sussurro says so after 20 s
of digital silence. The app still runs on macOS 11: below 14.2 the choice
is hidden with the reason, and the loopback device below is the way.

Dev builds (`npm run tauri dev`, `cargo test`) run unbundled, so the
permission is asked for (and granted to) the terminal, and without the
`Info.plist` key the tap may deliver only silence — check the capture
with a bundled build. Manual check:
`cargo test tap_delivers_audio -- --ignored --nocapture` while something
plays.

**Before 14.2 (or to use a device anyway): a loopback device.** Sussurro
reads any input device; the OS needs a virtual device that turns the output
into an input.

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

**Use headphones.** On speakers your microphone hears the others too, so
their words can also land on your channel ("You"); Sussurro does not cancel
echo.

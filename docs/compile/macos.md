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

## Runtime notes

macOS will prompt for two permissions on first use; both are required:

- **Microphone** (System Settings → Privacy & Security → Microphone)
- **Accessibility** (System Settings → Privacy & Security → Accessibility) —
  needed to synthesize the ⌘V paste into other apps. If text never appears,
  re-check this permission for Sussurro (or your terminal, in dev mode).

If you run a downloaded, unsigned build (e.g. a CI artifact) and macOS
reports it as damaged, clear the quarantine flag:
`xattr -cr /Applications/sussurro.app`.

# Building on Linux (X11 recommended)

Requires **glibc ≥ 2.38** (Ubuntu 24.04+, Debian 13+, Fedora 39+): the
Parakeet engine links `ort`'s prebuilt ONNX Runtime, which is compiled
against it. Older distros fail at link time with
`undefined symbol: __isoc23_strtoll`.

## Prerequisites (Debian/Ubuntu — adapt package names for your distro)

```bash
sudo apt update
sudo apt install build-essential curl cmake clang pkg-config \
  libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev \
  librsvg2-dev libasound2-dev libxdo-dev libxkbcommon-dev
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
# Node.js >= 24: use your distro package or https://github.com/nvm-sh/nvm
curl -fsSL https://ollama.com/install.sh | sh
```

Linux builds are CPU-only by default (the whisper.cpp Vulkan backend needs
the Vulkan SDK; see the target-specific dependencies in `Cargo.toml`).

## Build & run

```bash
git clone https://github.com/fullo/Sussurro && cd Sussurro/sussurro
npm install
npm run tauri dev      # development, hot-reload
npm run tauri build    # production bundle (AppImage, .deb, .rpm)
cd src-tauri && cargo test   # headless test suite
```

## Runtime notes

- **X11**: everything works out of the box (hotkey + paste injection).
- **Wayland**: paste injection relies on enigo's experimental Wayland support
  and may not work in all compositors; global shortcuts may also be
  restricted. Workarounds: run the app under XWayland, or switch to an X11
  session. Native Wayland injection (wtype/ydotool) is on the roadmap.
  When enigo fails, Sussurro falls back to `wtype` — install it with
  `sudo apt install wtype`.
- Audio uses ALSA (`libasound2`); PipeWire and PulseAudio expose ALSA
  compatibility by default.

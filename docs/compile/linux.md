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

### Optional: GPU transcription (Vulkan)

```bash
sudo apt install libvulkan-dev glslc mesa-vulkan-drivers
npm run tauri build -- --features linux-vulkan   # or: tauri dev -- --features linux-vulkan
```

The `linux-vulkan` cargo feature enables the whisper.cpp Vulkan backend
(same code path Windows uses). At runtime any GPU with a Vulkan driver
works; the first transcription compiles shaders once (~10 s), then the
driver caches them.

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
- **Wayland**: injection is native. Sussurro tries, in order:
  1. **wtype** (virtual-keyboard protocol — wlroots compositors: Sway,
     Hyprland, river; and KDE Plasma): `sudo apt install wtype`
  2. **ydotool** (uinput — works on ANY compositor, GNOME included):
     `sudo apt install ydotool`, then enable the daemon:
     `systemctl --user enable --now ydotool` (or run `ydotoold`; your user
     needs access to `/dev/uinput`, usually via the `input` group or the
     udev rule shipped with the package)
  3. enigo (experimental) as a last resort, which still covers XWayland apps.

  The clipboard uses the native `wayland-data-control` protocol, with
  `wl-copy`/`wl-paste` as fallback: `sudo apt install wl-clipboard`.
  Recommended install for GNOME users: `ydotool + wl-clipboard`; for
  Sway/Hyprland/KDE: `wtype + wl-clipboard`.

  **Global shortcuts** may still be restricted by the compositor on Wayland
  (that's a compositor policy, not an injection issue) — the in-app Dictate
  button and the tray always work.
- Audio uses ALSA (`libasound2`); PipeWire and PulseAudio expose ALSA
  compatibility by default.

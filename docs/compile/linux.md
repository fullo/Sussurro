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

CMake builds the two bundled C/C++ libraries: whisper.cpp and libopus (saved
audio as Ogg Opus, via the `opusic-sys` crate). Both are linked statically,
so the app needs no extra runtime library.

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

Validated 2026-07-05 (WSL2 Ubuntu 24.04): the feature compiles, the full
test suite passes, and **without a usable Vulkan device the app falls back
to CPU cleanly** — `ggml_vulkan: No devices found` → CPU transcription, no
crash. Note that ggml deliberately ignores CPU-type Vulkan devices
(llvmpipe), so software Vulkan is never picked up. The GPU-accelerated
path itself still needs a run on real Linux hardware with proper drivers.

## Build & run

```bash
git clone https://github.com/fullo/Sussurro && cd Sussurro/sussurro
npm install
npm run tauri dev      # development, hot-reload
npm run tauri build    # production bundle (AppImage, .deb, .rpm)
cd src-tauri && cargo test   # headless test suite
```

Without the updater signing key, `tauri build` stops at the updater
artifacts: add `-- --config '{"bundle":{"createUpdaterArtifacts":false}}'`
(details in [development.md](../development.md#build--run)).

### The llama-server sidecar (release bundles)

Release bundles ship a pinned upstream `llama-server` (llama.cpp, CPU build;
upstream's Vulkan Linux build is a possible later addition, like
`linux-vulkan` above) for the optional Qwen3-ASR engine and the *Local
(bundled)* LLM profile (plan E9). Fetch it
once, and again whenever `src-tauri/sidecar/llama-server.lock.json` changes
(SHA-256-checked, fails closed):

```bash
sudo apt install patchelf        # linuxdeploy, for the AppImage
npm run sidecar
LD_LIBRARY_PATH="$PWD/src-tauri/binaries/llama-server-libs:$LD_LIBRARY_PATH" \
  npm run tauri build -- --config src-tauri/tauri.sidecar.conf.json
../scripts/verify-sidecar-bundle.sh x86_64-unknown-linux-gnu src-tauri/target   # optional check
```

The `LD_LIBRARY_PATH` is for the AppImage step only: linuxdeploy resolves
every executable's libraries with `ldd` and fails on the sidecar's otherwise
(it then copies them into the AppImage and rewrites that copy's rpath).
Without the `--config` there is no sidecar; `cargo test` and clippy never
need it. Installed layout: `/usr/bin/sussurro-llama-server` (prefixed so it
never clashes with a distro `llama-server`) and its `.so` files in
`/usr/lib/sussurro/llama-server-libs/`; the app starts it with that folder as
working directory and on `LD_LIBRARY_PATH` (ggml loads its CPU backends from
there), with `PR_SET_PDEATHSIG` so the kernel stops it if the app dies.
Runtime needs `libgomp1` and OpenSSL 3, declared as `.deb`/`.rpm`
dependencies.

The D-Bus client library (`libdbus-1-dev`) comes in with `libgtk-3-dev`;
the tray, `enigo` and the Secret Service keyring below all link it.

## Runtime notes

- **API keys of LLM profiles** are kept in the Secret Service keyring
  (GNOME Keyring, KWallet, KeePassXC with Secret Service enabled…), under
  the service `com.sussurro.app`. Without one — a headless box, a minimal
  window manager with no keyring daemon — Sussurro falls back to saving the
  key in clear text in `settings.json`, and the profile editor says so. Start
  a keyring (e.g. `gnome-keyring-daemon`) and restart Sussurro: keys still
  in the file are moved into it at startup.

- **Clipboard (required on both X11 and Wayland).** Sussurro's Copy buttons and
  paste-injection put text on the system clipboard, and on Linux a clipboard set
  only persists if a helper holds the selection. Install one for your session:
  - **X11**: `sudo apt install xclip` (or `xsel`).
  - **Wayland**: `sudo apt install wl-clipboard` (`wl-copy`/`wl-paste`).

  Without it, Copy silently does nothing and nothing pastes (the in-process
  clipboard set reports success but the selection empties immediately).
- **X11**: hotkey + paste injection work once `xclip`/`xsel` is installed.
- **Wayland**: injection is native. Sussurro tries, in order:
  0. the XDG **RemoteDesktop portal** (default `wayland-portal` feature):
     zero setup on KDE Plasma and GNOME; the desktop asks for consent on
     first use (KDE may ask again after a reboot, kde#480235). See
     [issue #40](https://github.com/fullo/Sussurro/issues/40).
  1. **wtype** (virtual-keyboard protocol — wlroots compositors: Sway,
     Hyprland, river; and KDE Plasma): `sudo apt install wtype`
  2. **ydotool** (uinput — works on ANY compositor, GNOME included):
     `sudo apt install ydotool`, then enable the daemon:
     `systemctl --user enable --now ydotool` (or run `ydotoold`; your user
     needs access to `/dev/uinput`, usually via the `input` group or the
     udev rule shipped with the package)
  3. enigo (experimental) as a last resort, which still covers XWayland apps.

  The clipboard on Wayland is driven by `wl-copy`/`wl-paste`
  (`sudo apt install wl-clipboard`) — see the Clipboard note above.
  Recommended install for GNOME users: `ydotool + wl-clipboard`; for
  Sway/Hyprland/KDE: `wtype + wl-clipboard`.

  **Global shortcuts** may still be restricted by the compositor on Wayland
  (that's a compositor policy, not an injection issue) — the in-app Dictate
  button and the tray always work.
- Audio uses ALSA (`libasound2`); PipeWire and PulseAudio expose ALSA
  compatibility by default.

### System audio + mic: this computer's sound

*New → System audio + mic*
records a call from a desktop app — Zoom, Teams, anything that plays through
the computer — as two channels: your microphone ("You") and a second input
source that carries the computer's sound (the others, told apart as
"Voice 1, Voice 2…").

**Built-in: the default output's monitor.** On PulseAudio and PipeWire
(through `pipewire-pulse`) every output has a **monitor source** that
carries what it plays. Choose **This computer's sound (built-in)**
(preselected): Sussurro finds the default sink with `pactl
get-default-sink` (or `pactl info` on PulseAudio < 15), picks its
`<sink>.monitor` from `pactl list short sources`, and records it with
`parec` as 16 kHz mono — no `~/.asoundrc`, nothing linked at build time.
Both tools come from **`pulseaudio-utils`** (`sudo apt install
pulseaudio-utils`; installed with most desktops, also on PipeWire). When
they are missing, no sound server answers, or the output has no monitor,
the choice is hidden and the reason shown; the device picker below is the
fallback. It follows the output that was the default at the start: if you
switch outputs mid-call, start a new recording. Manual check:
`cargo test native_capture_hears_the_computer -- --ignored --nocapture`
while something plays.

**Or a monitor exposed as an ALSA device.** Sussurro reads any input
device (through ALSA, where monitors are not listed).

List the monitors with `pactl list short sources | grep monitor` and
expose one as an ALSA device with a `hint` (so it is listed) in
`~/.asoundrc`, using the pulse plugin (`libasound2-plugins`; on PipeWire it
goes through `pipewire-pulse`):

```
pcm.system_monitor {
    type pulse
    device "alsa_output.pci-0000_00_1f.3.analog-stereo.monitor"   # yours from pactl
    hint {
        show on
        description "Computer sound (monitor)"
    }
}
```

Restart Sussurro and choose **system_monitor** as the system audio device.
Alternative without the file: choose the `pulse` (or `pipewire`) device and,
while recording, route that stream to *Monitor of …* in `pavucontrol` →
Recording.

**Use headphones.** On speakers your microphone hears the others too, so
their words can also land on your channel ("You"); Sussurro does not cancel
echo.

# Building on Windows 10/11

## Prerequisites (one-time, elevated PowerShell)

```powershell
winget install Rustlang.Rustup OpenJS.NodeJS.LTS Kitware.CMake LLVM.LLVM Ollama.Ollama
winget install Microsoft.VisualStudio.2022.BuildTools --override "--passive --wait --add Microsoft.VisualStudio.Workload.VCTools;includeRecommended"
winget install KhronosGroup.VulkanSDK   # GPU acceleration (whisper.cpp Vulkan backend)
setx LIBCLANG_PATH "C:\Program Files\LLVM\bin"
setx CARGO_TARGET_DIR "C:\sbuild"       # SHORT path — see note below
```

Then open a **new** terminal (so the `setx` variables are picked up) and
build (see below).

## GPU notes

- Transcription runs on the GPU via **Vulkan** (NVIDIA/AMD/Intel alike). The
  Vulkan SDK is needed at *build* time only; end users just need a Vulkan
  driver (any modern GPU driver ships one).
- The very first transcription on a machine compiles GPU shaders (~10 s,
  one-time); the driver caches them afterwards.

## CARGO_TARGET_DIR — required, and why

On Windows the cargo target directory **must be a short path** (e.g.
`C:\sbuild`). whisper.cpp's Vulkan shader sub-build nests CMake/MSBuild
output dozens of directories deep inside cargo's `target/`; from a normal
clone location the generated paths exceed Windows' 260-char MAX_PATH and
the build dies with MSBuild **FileTracker error FTK1011** ("could not create
the new file tracking log file"). Enabling NTFS long paths
(`LongPathsEnabled`) does **not** help — FileTracker chokes regardless.

Set it once, globally (then open a new terminal):

```powershell
setx CARGO_TARGET_DIR "C:\sbuild"
```

or per session only:

```powershell
$env:CARGO_TARGET_DIR = "C:\sbuild"
```

Practical consequences:

- **All cargo output moves there** — the dev build, `cargo test` artifacts,
  and the release bundles: after `npm run tauri build` the installers are in
  `C:\sbuild\release\bundle\` (`nsis\` and `msi\`), not in
  `src-tauri\target\`. `cargo clean` cleans `C:\sbuild` too.
- The variable applies to *every* Rust project in that shell/user profile.
  If you don't want that, scope it per session (second form above), or give
  each project its own dir (e.g. `C:\sbuild\sussurro`) — any short prefix
  works.
- **Do not commit** a `.cargo/config.toml` with `target-dir` instead: the
  repo used to ship one pinned to `F:/claude/builds/sussurro` and on
  Linux/macOS the drive-letter path is treated as *relative*, its `:` breaks
  cargo's `LD_LIBRARY_PATH` joining and `cargo test` fails on every clone
  (`error: failed to join paths from $LD_LIBRARY_PATH together`). That's why
  it was removed in favour of the environment variable. If you prefer a
  file, keep it out of git (`.git/info/exclude`).
- CI does the same: the release workflow sets `CARGO_TARGET_DIR=C:\st` for
  the Windows job (see `.github/workflows/release.yml`).

## Build & run

```powershell
git clone https://github.com/fullo/Sussurro; cd Sussurro/sussurro
npm install
npm run tauri dev      # development, hot-reload
npm run tauri build    # production bundle (NSIS .exe + .msi)
cd src-tauri; cargo test   # headless test suite
```

### The llama-server sidecar (release bundles)

Release installers ship a pinned upstream `llama-server` (llama.cpp, Vulkan
build) for the optional Qwen3-ASR engine (plan E9). Fetch it once, and again
whenever `src-tauri/sidecar/llama-server.lock.json` changes (SHA-256-checked,
fails closed):

```powershell
npm run sidecar
npm run tauri build -- --config src-tauri/tauri.sidecar.conf.json
```

Without the `--config` there is no sidecar in the installer; `cargo test`
and clippy never need it. It installs as `sussurro-llama-server.exe` next to
`sussurro.exe`, with its DLLs (llama, ggml, the Vulkan and CPU backends,
`libomp.dll`) in `llama-server-libs\`; the app starts it with that folder as
working directory and prepended to `PATH`. It needs the Visual C++ runtime
(`MSVCP140.dll`, already required by Sussurro itself) and, for the GPU, the
Vulkan loader that comes with the graphics driver — without it ggml falls
back to the CPU. Antivirus tools may flag an unsigned helper executable the
first time it runs.

## Runtime notes

- Settings → Privacy & security → Microphone → enable **"Let desktop apps
  access your microphone"**.
- WebView2 is preinstalled on Windows 11; on older Windows 10 install the
  [WebView2 runtime](https://developer.microsoft.com/microsoft-edge/webview2/).

### System audio + mic: a virtual cable

*New → System audio + mic* (meetings preview, Settings → Browser extension)
records a call from a desktop app — Zoom, Teams, anything that plays through
the computer — as two channels: your microphone ("You") and a second input
device that carries the computer's sound (the others, told apart as
"Voice 1, Voice 2…"). Sussurro reads any input device; the OS needs a
virtual device that turns the output into an input.

1. Install [VB-Cable](https://vb-audio.com/Cable/) (free; Voicemeeter
   works too).
2. Send the call to **CABLE Input**: set it as the meeting app's speaker,
   or as the Windows output. To keep hearing it, open Sound → Recording →
   **CABLE Output** → Properties → Listen, tick *Listen to this device* and
   pick your headphones.
3. In Sussurro choose **CABLE Output (VB-Audio Virtual Cable)** as the
   system audio device. Some sound cards also offer **Stereo Mix**, which
   works without installing anything.

Native WASAPI loopback (no virtual cable) is planned (#140).

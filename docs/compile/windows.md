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

## Runtime notes

- Settings → Privacy & security → Microphone → enable **"Let desktop apps
  access your microphone"**.
- WebView2 is preinstalled on Windows 11; on older Windows 10 install the
  [WebView2 runtime](https://developer.microsoft.com/microsoft-edge/webview2/).

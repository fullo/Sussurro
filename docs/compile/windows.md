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
- `CARGO_TARGET_DIR` **must point to a short path** (e.g. `C:\sbuild`):
  whisper.cpp's Vulkan shader sub-build otherwise exceeds Windows' 260-char
  MAX_PATH and MSBuild fails with FTK1011. Windows only — macOS/Linux don't
  need it (and a drive-letter path would actually break `cargo test` there).
- The very first transcription on a machine compiles GPU shaders (~10 s,
  one-time); the driver caches them afterwards.

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

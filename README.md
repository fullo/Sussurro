# Sussurro

Fully-local voice dictation for Windows/macOS/Linux — a Wispr Flow clone with
no cloud: whisper.cpp for speech-to-text, Ollama for AI cleanup, paste-injection
into any app.

**Hold `Ctrl+Shift+Space`, speak, release.** The cleaned-up text appears where
your cursor is.

## Requirements
- [Ollama](https://ollama.com) running locally with a small model: `ollama pull llama3.2:3b`
- A microphone, and (Windows) desktop-app microphone access enabled

## Development
Prerequisites: Rust (MSVC), Node LTS, CMake, LLVM (`LIBCLANG_PATH` set). See
`docs/superpowers/plans/2026-07-02-sussurro-v1.md` Task 0.

```
cd sussurro
npm install
npm run tauri dev            # run the app
cd src-tauri && cargo test   # headless test suite
```

First run: open Settings (tray icon), download a Whisper model.

## Architecture
hotkey (press/release) → cpal mic capture → 16 kHz mono → whisper.cpp
(whisper-rs) → Ollama `/api/chat` cleanup (None/Light/Medium/High, falls back
to raw transcript) → clipboard-paste injection (clipboard restored) → history.

Research behind the design: `docs/whisperflow-clone-research.md`.

## Known limits (v1)
- Linux Wayland: paste injection depends on enigo's experimental Wayland
  support; X11 works. wtype/ydotool fallbacks are on the roadmap.
- macOS needs Accessibility + Microphone permissions granted manually.
- No streaming transcription yet — text lands after you release the hotkey.
- cpal is pinned to 0.16 (0.18 has a windows-core version conflict with the
  Tauri stack on Windows).

# CLAUDE.md — shared project memory

Project context for Claude Code sessions. This file is committed so every
machine and user working on Sussurro shares the same context — **record
project decisions here, not in per-machine memory.**

## Project layout

- `sussurro/` — the Tauri 2 app (React + TypeScript frontend, Rust backend
  in `sussurro/src-tauri/`). The repo root only holds docs and CI.
- Build instructions per OS live in `docs/compile/{windows,macos,linux}.md`
  — keep them updated when build requirements change.

## Standing decisions

- **Repo stays private until every platform builds and works.** Because of
  this, the Tauri auto-updater gets 404 on `latest.json` (anonymous access).
- **Auto-update is frozen until v0.5.0** (decided 2026-07-03): the signing
  pipeline keeps running on every release, but the repo/releases go public
  — and the updater becomes functional — only with 0.5.0. Don't propose
  making the repo public before that.
- **The v0.2.0 draft release is kept intentionally** — do not delete it.
- **macOS is Apple Silicon only (min 11.0)**: `ort` (ONNX Runtime) has no
  prebuilt binaries for `x86_64-apple-darwin`. Don't re-add the Intel
  target to the release matrix.
- Workflow: **branch → PR → merge** — no direct pushes to `main`.

## Release process

- Push a `v*` tag (from any branch) to trigger `.github/workflows/release.yml`
  → draft release with signed installers + `latest.json`. Publish by
  un-drafting.
- Never force-move an existing release tag; bump the patch version instead
  (version lives in `sussurro/package.json`, `sussurro/src-tauri/tauri.conf.json`,
  `sussurro/src-tauri/Cargo.toml` + `Cargo.lock`).
- Updater signing key: outside the repo, uploaded as
  `TAURI_SIGNING_PRIVATE_KEY` secret (see README → Releases & auto-update).
  Changing the key pair orphans existing installs.

## CI gotchas (learned the hard way)

- **cdn.pyke.io (ort's prebuilt download) persistently 403s GitHub runners.**
  The test job links Microsoft's official ONNX Runtime release instead
  (`ORT_LIB_LOCATION` + `ORT_PREFER_DYNAMIC_LINK`); the release workflow
  still uses pyke's static binaries with an `ORT_CACHE_DIR` cache. If a
  release job fails with a 403 from cdn.pyke.io, re-run it; if it keeps
  failing, apply the MS-release fallback there too.
- Keep the pinned ONNX Runtime version in `test.yml` in sync with what the
  locked `ort-sys` expects (see the `ms@<version>` URLs in its
  `build/download/dist.txt`).
- Linux needs **glibc ≥ 2.38** (ubuntu-24.04 runners) — older glibc fails
  linking with `undefined symbol: __isoc23_strtoll`.
- macOS needs `minimumSystemVersion` ≥ 10.15 (whisper.cpp uses
  `std::filesystem`); it is set to 11.0 (arm64 baseline) in
  `tauri.conf.json`.

## Roadmap (agreed 2026-07-03, current version 0.2.1)

### 0.3.0 — working everywhere (gate: every platform compiled AND verified)

1. Runtime smoke test on real Windows with the CI-built installer (msi/exe):
   hotkey → recording → Vulkan GPU transcription → paste injection.
2. Runtime smoke test on real Linux (AppImage/deb on Ubuntu 24.04).
3. **Native Wayland injection (wtype/ydotool)** — the biggest functional
   gap: modern distros default to Wayland and injection there is fragile.
   Recommended starting point for development work.

### 0.4.0 — quality & tech debt

4. Streaming typing with cleanup enabled (today only works with Cleanup None).
5. Unpin cpal 0.16 → 0.18 (retest the windows-core conflict with Tauri).
   *Retested 2026-07-03 with cpal 0.18.1: still broken — cpal's WASAPI code
   compiles against a different `windows_core` than the Tauri stack
   (`IActivateAudioInterfaceCompletionHandler: Interface not satisfied`).
   Re-try when cpal releases a version on windows-core ≥ 0.62.*
6. Move `ort` from 2.0.0-rc.12 to stable when released (coordinate with the
   pinned ONNX Runtime version in test.yml — see CI gotchas).
   *Checked 2026-07-03: still no stable on crates.io (max = 2.0.0-rc.12);
   transcribe-rs 0.3.11 is also the latest.*
7. Optional Vulkan GPU build on Linux (feature flag; CPU-only today).
8. Minimal E2E smoke test in CI (app launches, window exists).

### 0.5.0 — go public

9. macOS Developer ID signing + notarization (ad-hoc today → Gatekeeper
   blocks public users). Consider Windows code signing for SmartScreen.
10. Make the repo public → auto-update unfreezes (see standing decisions).

## Per-machine setup

- **Windows dev machines must set a short `CARGO_TARGET_DIR`**
  (e.g. `setx CARGO_TARGET_DIR "C:\sbuild"`): whisper.cpp's Vulkan shader
  sub-build exceeds MAX_PATH otherwise (MSBuild FTK1011). Full details in
  `docs/compile/windows.md`.
- **Never commit a `.cargo/config.toml` with `target-dir`**: a drive-letter
  path is relative on Linux/macOS and its `:` breaks `cargo test`
  (`failed to join paths from $LD_LIBRARY_PATH`). One was removed from the
  repo for exactly this reason.

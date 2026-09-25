# Development

How to build Sussurro from source and work on it. For using the app, see the
[README](../README.md).

## Repository layout

- `sussurro/` — the Tauri 2 app: React + TypeScript frontend, Rust backend in
  `sussurro/src-tauri/`.
- `extension/` — the browser extension for meeting capture:
  Vite + TypeScript + React, one build per browser (Chrome/Edge/Brave and
  Firefox). See [Browser extension](#browser-extension) below.
- The repo root holds docs and CI only.
- `docs/compile/{windows,macos,linux}.md` — per-OS build prerequisites, GPU
  notes and platform caveats. Keep them updated when build requirements change.
- `docs/releases.md` — the release + auto-update + code-signing process.
- `docs/whisperflow-clone-research.md` — the research behind the design.
- `docs/index.html` + `docs/blog/` — the project's English marketing site
  (static; served by GitHub Pages from the `/docs` folder). Dev docs (this
  file, `releases.md`, `compile/`) live alongside it in `docs/`.

## Toolchain

All platforms need **Node.js ≥ 24** and **Rust stable**. Then follow the guide
for your OS — it lists the system packages, GPU SDKs and the gotchas:

- **[Windows 10/11](compile/windows.md)** — Vulkan GPU build, the `MAX_PATH`
  workaround (short `CARGO_TARGET_DIR`), WebView2.
- **[macOS 11+](compile/macos.md)** — Apple Silicon only, Metal GPU, the
  microphone/accessibility permissions.
- **[Linux](compile/linux.md)** — glibc ≥ 2.38, apt dependencies, X11/Wayland
  injection notes, the optional Vulkan build.

## Build & run

Once the prerequisites are in place:

```bash
git clone https://github.com/fullo/Sussurro && cd Sussurro/sussurro
npm install
npm run tauri dev      # development, hot-reload
npm run tauri build    # production bundle (installer per platform)
```

`tauri.conf.json` asks for signed updater artifacts, so a plain
`npm run tauri build` stops with *"A public key has been found, but no
private key"* unless `TAURI_SIGNING_PRIVATE_KEY` is set (see
[releases.md](releases.md#updater-signing-key-one-time-setup)). For a local
bundle without the key, turn them off for that build:

```bash
npm run tauri build -- --config '{"bundle":{"createUpdaterArtifacts":false}}'
```

First run: a short setup opens over the workspace (permissions, the archive
folder, a speech model to download, cleanup, your shortcut) — then dictate.
Settings → About → *Run the setup again* reopens it. In the browser preview
(`npm run dev`), `?onboarding=welcome` or `?onboarding=whats_new` shows it.

## llama-server sidecar

Extra STT engines (Qwen3-ASR, Track E) and the *Local (bundled)* LLM
profile (Qwen3 1.7B, #118) run in a bundled `llama-server` (plan E9): a
**pinned upstream llama.cpp release**, never built here and never
in git. `sussurro/src-tauri/sidecar/llama-server.lock.json` pins the release
and, per target, the asset URL, SHA-256, size, the companion libraries to
ship and the licence. Run once per machine (and after the lock changes):

```bash
cd sussurro
npm run sidecar        # host target; --target <triple> for another one
```

It downloads, checks the SHA-256 (a mismatch aborts and writes nothing),
and writes `src-tauri/binaries/sussurro-llama-server-<target-triple>` (Tauri's
`externalBin` naming) plus `src-tauri/binaries/llama-server-libs/`
(gitignored; archives cached in `binaries/.cache/`). Builds pick them up only
with `--config src-tauri/tauri.sidecar.conf.json` (`tauri build`/`tauri dev`)
— `tauri-build` checks `externalBin` at *compile* time, so keeping it out of
`tauri.conf.json` means `cargo test`, clippy and plain `tauri dev` never need
the download. The release workflow always merges it and then runs
`scripts/verify-sidecar-bundle.sh` on every bundle; `test.yml` does the same
for the Linux `.deb` + AppImage. Per-OS layout and runtime notes are in
`docs/compile/*.md`. The Rust side locates it (`stt::sidecar`; debug builds
also find `npm run sidecar`'s output in `src-tauri/binaries/`, so a plain
`tauri dev` can use Qwen3-ASR) and runs it (`stt::remote`, #117: spawn on a
private Unix socket — a 0700 folder under `<app data>/sidecar/` — or, on
Windows, a random loopback port whose listener must be the child's; a random
`LLAMA_API_KEY` per spawn sent as a bearer token, `--no-slots` (#216, see
`stt::remote::endpoint`); health check, restart with backoff, stop on idle
unload / engine change / exit, output sanitiser). Its tests run the test
binary itself as a fake `llama-server`; the real one is an `#[ignore]` test
that downloads nothing:

```bash
SUSSURRO_TEST_LLAMA_SERVER=/path/to/llama-server \
SUSSURRO_TEST_QWEN3_ASR_DIR=/folder/with/Qwen3-ASR-1.7B-Q8_0.gguf+mmproj \
SUSSURRO_TEST_FLEURS_WAV=/path/to/clip.wav \
cargo test live_qwen3_asr -- --ignored --nocapture
```

(`SUSSURRO_TEST_LLAMA_LIBS` when the libraries are not next to the binary,
e.g. `src-tauri/binaries/llama-server-libs`.) With the same variables minus
the clip, `cargo test live_sidecar_enforces -- --ignored` checks that the
real server enforces the per-spawn key and hides `/slots` — run it when
bumping llama.cpp. The server log of the app is
`llama-server.log` in the app's log folder.

**Bumping llama.cpp**: change `release`/`commit`/`version` and every target's
`asset`/`url`/`sha256`/`size` (the GitHub release lists each asset's
digest), re-check the `libs` lists against the new archives (`otool -L`,
`objdump -p`), run `npm test` and `npm run sidecar -- --force`, and
regenerate `licenses.json` if a licence text changed (the script refuses an
archive whose licence differs from the committed copy in `sidecar/licenses/`).

## Tests & CI

```bash
cd sussurro
npm test               # vitest: frontend helpers (src/**/*.test.ts) + scripts/
npm run build          # tsc + vite build (the type check; the crate embeds dist/)
cd src-tauri
cargo test             # unit + integration (live tests are #[ignore])
cargo clippy --all-targets -- -D warnings   # stricter than CI, which allows warnings
```

Run `npm run build` once before the first `cargo` command in a fresh
checkout: the crate embeds the built frontend.

Live tests that need a running Ollama are marked `#[ignore]`; run them
explicitly, e.g.:

```bash
cargo test live_english_stays_english -- --ignored --nocapture
```

CI (`.github/workflows/test.yml`, Ubuntu 24.04) has two jobs:

- **test**: links Microsoft's ONNX Runtime release, builds the frontend,
  runs the sidecar script's tests (`npx vitest run scripts/`) and
  `npm run sidecar` (+ a `--version` check of the fetched binary),
  `cargo test`, `cargo clippy --all-targets`, an Xvfb E2E smoke test
  (launches the app, asserts the window exists), then builds the `.deb` and
  AppImage with the sidecar and checks them with
  `scripts/verify-sidecar-bundle.sh`.
- **extension**: typecheck, unit tests, build, `web-ext lint`, the capture
  harness in Chromium + Firefox and again in Edge (see
  [Browser extension](#browser-extension)), and uploads the zips.

The frontend unit tests under `sussurro/src/` run locally (`npm test`) and
in `scripts/ci-local.sh`, not in `test.yml`. When GitHub Actions minutes
are unavailable, `scripts/ci-local.sh` mirrors the **test** job inside WSL2
Ubuntu 24.04 (plus `npm test` and the ignored `engine_end_to_end` test; it
does not run the extension harness):

```bash
wsl -d Ubuntu-dev -u root -- bash /mnt/f/GitHub/Sussurro/scripts/ci-local.sh <branch>
```

## Browser extension

`extension/` builds independently of the app (Node.js ≥ 24, no Rust):

```bash
cd extension
npm ci
npm run build          # dist/chrome/, dist/firefox/ + one zip per browser
npm run build:chrome   # or one browser at a time (build:firefox)
npm run typecheck      # tsc
npm test               # vitest: pure helpers, manifests, licence list
npm run lint           # web-ext lint on dist/firefox (after the build)
```

The capture harness (Playwright, runs in CI) loads the built extension into
real Chromium and Firefox against a local two-peer call and a fake app:

```bash
npx playwright install chromium firefox   # once (Linux: add --with-deps)
npm run build && npm run test:e2e         # Chromium + Firefox; or: -- firefox
npm run test:e2e -- edge                  # the installed Edge (CI runs it too)
npm run test:e2e -- brave                 # the installed Brave (or BRAVE_PATH), local only
HEADED=1 npm run test:e2e -- chromium     # watch it
```

Playwright keeps its browsers in `~/Library/Caches/ms-playwright` (macOS),
`~/.cache/ms-playwright` (Linux) or `%USERPROFILE%\AppData\Local\ms-playwright`
(Windows); set `PLAYWRIGHT_BROWSERS_PATH` to keep them elsewhere (for
example inside the repo or a scratch folder) and use the same value for
`install` and `test:e2e`. On a Linux box without a sound server, Firefox's
audio stays suspended and the harness fails: start one first, as CI does
(`sudo apt install pulseaudio && pulseaudio --start --exit-idle-time=-1`).

It imports the app's transcript components from `sussurro/src/transcript/`
(`@sussurro/transcript`), the pairing-code format (`@sussurro/pairing`) and
the recording notice (`@sussurro/notice`), so changes there must keep
building in both places. Its version always equals the app's (read from
`sussurro/package.json`).

To try it against a running app: load `dist/chrome` unpacked
(`chrome://extensions` → Developer mode → Load unpacked) or
`dist/firefox/manifest.json` as a temporary add-on
(`about:debugging#/runtime/this-firefox`), then pair it from Settings →
Browser extension (the local API must be on). Meetings are always available
in the app; there is no feature flag. Details, architecture and the manual
checks are in [`extension/README.md`](../extension/README.md). CI builds,
tests, lints and runs the harness in its own job; the release workflow
attaches both zips to the release.

## Third-party licenses

The About dialog's license list is `sussurro/public/licenses.json`, generated
from the resolved cargo + npm production dependencies (never dev ones — the
script fails if one gets in). **Regenerate after changing dependencies**
(needs the Rust toolchain; committed, not run in CI):

```bash
cd sussurro && npm run licenses
```

The browser extension has its own list, shown in the About section of its
options page: `extension/src/options/licenses.json`, from the extension's npm
production dependencies. Regenerate it after changing them (a unit test
compares it with `extension/package-lock.json`):

```bash
cd extension && npm ci && npm run licenses
```

## Conventions

- Workflow is **branch → PR → merge**; no direct pushes to `main`.
- Never commit a `.cargo/config.toml` with `target-dir` — a drive-letter path
  is relative on Linux/macOS and its `:` breaks `cargo test`.
- The roadmap and standing project decisions live in
  [`CLAUDE.md`](../CLAUDE.md) at the repo root.

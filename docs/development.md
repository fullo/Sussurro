# Development

How to build Sussurro from source and work on it. For using the app, see the
[README](../README.md).

## Repository layout

- `sussurro/` — the Tauri 2 app: React + TypeScript frontend, Rust backend in
  `sussurro/src-tauri/`. The repo root holds docs and CI only.
- `docs/compile/{windows,macos,linux}.md` — per-OS build prerequisites, GPU
  notes and platform caveats. Keep them updated when build requirements change.
- `docs/releases.md` — the release + auto-update + code-signing process.
- `docs/whisperflow-clone-research.md` — the research behind the design.
- `site/` — the project's English landing page (static, deployable to any host).

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

First run: the window opens on Settings — pick a Whisper model, click
*Download*, set your shortcut with the hotkey recorder, and dictate.

## Tests & CI

```bash
cd sussurro/src-tauri
cargo test             # unit + integration (Ollama-backed tests are #[ignore])
cargo clippy --all-targets -- -D warnings
```

Live tests that need a running Ollama are marked `#[ignore]`; run them
explicitly, e.g.:

```bash
cargo test live_english_stays_english -- --ignored --nocapture
```

CI (`.github/workflows/test.yml`) runs the suite, clippy and an Xvfb E2E smoke
test (launches the app, asserts the window exists). When GitHub Actions minutes
are unavailable, `scripts/ci-local.sh` mirrors it inside WSL2 Ubuntu 24.04:

```bash
wsl -d Ubuntu-dev -u root -- bash /mnt/f/GitHub/Sussurro/scripts/ci-local.sh <branch>
```

## Third-party licenses

The About dialog's license list is `sussurro/public/licenses.json`, generated
from the resolved cargo + npm dependencies. **Regenerate after changing
dependencies** (needs the Rust toolchain; committed, not run in CI):

```bash
cd sussurro && npm run licenses
```

## Conventions

- Workflow is **branch → PR → merge**; no direct pushes to `main`.
- Never commit a `.cargo/config.toml` with `target-dir` — a drive-letter path
  is relative on Linux/macOS and its `:` breaks `cargo test`.
- The roadmap and standing project decisions live in
  [`CLAUDE.md`](../CLAUDE.md) at the repo root.

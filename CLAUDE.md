# CLAUDE.md — shared project memory

Project context for Claude Code sessions. This file is committed so every
machine and user working on Sussurro shares the same context — **record
project decisions here, not in per-machine memory.**

## Project layout

- `sussurro/` — the Tauri 2 app (React + TypeScript frontend, Rust backend
  in `sussurro/src-tauri/`). The repo root only holds docs and CI.
- Build instructions per OS live in `docs/compile/{windows,macos,linux}.md`
  — keep them updated when build requirements change.
- The About dialog's third-party license list is `sussurro/public/licenses.json`,
  generated from the resolved deps (cargo + npm). **Regenerate after changing
  dependencies:** `cd sussurro && npm run licenses` (needs the Rust toolchain;
  not run in CI to keep the pipeline simple — the file is committed).

## Standing decisions

- **Going public at v0.5.0** (decided 2026-07-06): the repo becomes public and
  the auto-updater unfreezes (the endpoint 404'd anonymously while private).
  Sequence chosen: **publish immediately**, accepting unsigned-installer OS
  warnings until code signing lands. No code change needed to unfreeze — it's
  the GitHub visibility toggle + a published (non-draft) release.
- **No external OS code signing for now** (decided 2026-07-06). Both macOS
  (ad-hoc → right-click Open) and Windows (unsigned → SmartScreen "Run anyway")
  ship without OS signing; users click through, and this is documented in the
  README + the macOS blog post. Don't set up SignPath / Apple Developer ID /
  Azure Trusted Signing until the maintainer explicitly revisits — the options
  are written up in `docs/windows-signing-signpath.md` + `docs/releases.md` for
  when that happens. The updater's own minisign signing is independent and
  already active, so **auto-update works regardless of OS signing**.
- **The v0.2.0 draft release is kept intentionally** — do not delete it.
- **License: AGPL-3.0-or-later** (chosen 2026-07-06). Copyleft that also covers
  network use (Sussurro exposes a local HTTP API), so no one can build a
  closed/proprietary product on it. Verified compatible with the whole dep tree
  (all permissive + MPL-2.0 + two GPL-3.0-or-later crates; no GPL-2.0-only or
  proprietary). `LICENSE` holds the verbatim text; SPDX `AGPL-3.0-or-later` is
  set in `package.json` + `Cargo.toml`. Copyright is retained by the author
  (Francesco Fullone / DarumaHQ) so a **commercial/dual license** can be sold
  later — don't relicense or add a CLA-less outside contribution that would
  fragment that. A permissive license (MIT) is NOT an option here: the
  GPL-3.0-or-later deps require a copyleft-compatible license.
- **macOS is Apple Silicon only (min 11.0)**: `ort` (ONNX Runtime) has no
  prebuilt binaries for `x86_64-apple-darwin`. Don't re-add the Intel
  target to the release matrix.
- **LLM profiles replace the flat cleanup settings (0.8, #119)**:
  `Settings.llm_profiles` + `cleanup_profile` (`llm/profile.rs`). A pre-0.8
  `settings.json` is migrated on load (`Settings::normalize`) into one
  "Local" profile selected for cleanup, and saved once; the old
  `cleanup_api`/`ollama_url`/`ollama_model`/`api_key` keys are read for the
  migration and **never written again**. Consequence, accepted: downgrading
  to ≤ 0.7 loses a custom cleanup server (the old build falls back to its
  defaults, Ollama on localhost) — the updater only moves forward. API keys
  stay in `settings.json` in clear, as before.
- **Link source rules (0.8, #123)** (`sources/url/`): only `http`/`https`,
  no credentials in the link (it is saved as `source: url:<link>`). Hosts on
  this computer or the local network (loopback, private, link-local incl.
  cloud metadata, CGNAT, unique-local IPv6, `localhost`) are **refused
  unless the user ticks "Allow local network addresses" for that run** —
  checked on IP literals, every resolved address and every redirect hop,
  with the connection pinned to the checked addresses (no DNS rebinding).
  Downloads: model-download timeouts, 2 GiB cap, temp file in
  `<app data>/link-downloads/` (never the archive), removed after the run
  and swept at startup. Video platforms: `yt-dlp` found on PATH or the
  Homebrew/winget/scoop/pip folders (E10, not bundled), argument vector
  only, `bestaudio[ext=m4a]/bestaudio`, `--ignore-config --no-playlist`;
  opus-only videos are refused (no ffmpeg bundled).
- Workflow: **branch → PR → merge** — no direct pushes to `main`.
- **Product direction: speech-to-text workbench** (decided 2026-09-24).
  Full plan: `docs/superpowers/plans/2026-09-24-sussurro-speech-workbench.md`
  (decisions P1–P11 + engineering E1–E12 — read it before touching 0.7+).
  Short form: UX = proposal A (left-rail workspace, mock in
  `docs/superpowers/plans/ux-mocks/`); **command mode removed** (out of
  focus, OS voice control covers it — spoken editing commands inside
  dictation stay); cleaning is text-only (no audio cutting); archive of
  markdown items in `<Documents>/Sussurro` on every OS (frontmatter is the
  source of truth, index is derived); three item types by content —
  note / meeting / transcription (file default: note); participant emails
  from a manual People registry; LLM profiles speak only OpenAI-compatible
  or Ollama APIs; .srt is a setting (default on request) for meetings and
  transcriptions; 0.9 speakers = Meet names + generic "Voice N" only; WAV
  saved only on request; extra STT engines (Qwen3-ASR) run in a bundled
  `llama-server` sidecar, never in-process (two ggml copies can't share a
  binary); never add sherpa-onnx (second ONNX Runtime).

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
- **Manual release while Actions is down** (billing outage, ~until 2026-08):
  Windows is built/signed on the dev box, Linux in WSL, then
  `gh release create` with the assets + a hand-written `latest.json`.
  macOS assets are added from a Mac afterwards:
  ```bash
  git clone <repo> && cd Sussurro/sussurro && git checkout vX.Y.Z
  npm ci && npm run tauri build          # Apple Silicon
  node_modules/.bin/tauri signer sign \
    --private-key-path <sussurro-updater.key> --password "" \
    src-tauri/target/release/bundle/macos/sussurro.app.tar.gz
  gh release upload vX.Y.Z src-tauri/target/release/bundle/macos/sussurro.app.tar.gz* \
    src-tauri/target/release/bundle/dmg/*.dmg
  ```
  then add the `darwin-aarch64` entry (URL + signature) to the release's
  `latest.json`. Beware: PowerShell drops empty `""` args and unsets
  empty env vars — sign via `cmd /c` on Windows (see scripts/ for helpers).

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
- **Actions minutes can run out** (private repo; macOS jobs bill 10x, Windows
  2x — one 3-OS release run burns ~200 min-equivalents). Symptom: every job
  fails in ~3 s with no steps and the annotation "job was not started …
  spending limit". Fix: Billing & plans on the owner account. Meanwhile
  `scripts/ci-local.sh` mirrors test.yml inside WSL2 Ubuntu 24.04
  (`wsl -d Ubuntu -u root -- bash /mnt/f/GitHub/Sussurro/scripts/ci-local.sh
  <branch>`) — it validated PR #53 end-to-end (tests, clippy, E2E smoke).
  Releases still need GitHub runners (macOS/Windows can't be mirrored).

## Roadmap (agreed 2026-07-03, current version 0.6.3 — released)

### 0.3.0 — working everywhere (gate: every platform compiled AND verified)

1. Runtime smoke test on real Windows with the CI-built installer (msi/exe):
   hotkey → recording → Vulkan GPU transcription → paste injection.
2. Runtime smoke test on real Linux (AppImage/deb on Ubuntu 24.04).
3. **Native Wayland injection** — the biggest functional gap: modern distros
   default to Wayland and injection there is fragile. Status: wtype/ydotool
   ladder shipped; the RemoteDesktop portal (ashpd, issue #40) **shipped as
   the primary backend in 0.3.9** behind the default-on `wayland-portal`
   feature. Issue #40 stays open until the reporter verifies at runtime on
   KDE Plasma 6 Wayland.

### 0.4.0 — quality & tech debt

4. Streaming typing with cleanup enabled — **done & verified** (sentence-by-
   sentence streaming with per-chunk LLM cleanup landed with the Wispr-parity
   batch; manually verified by dictation on Windows 0.3.9-3 on 2026-07-05,
   "experimental" label removed).
5. Unpin cpal 0.16 → 0.18 (retest the windows-core conflict with Tauri).
   *Rechecked 2026-07-05: crates.io max is still 0.18.1, the broken one.
   Re-try when cpal releases a version on windows-core ≥ 0.62.*
6. Move `ort` from 2.0.0-rc.12 to stable when released (coordinate with the
   pinned ONNX Runtime version in test.yml — see CI gotchas).
   *Rechecked 2026-07-05: still no stable (max = 2.0.0-rc.12);
   transcribe-rs 0.3.11 is also the latest.*
7. Optional Vulkan GPU build on Linux (feature flag; CPU-only today).
   *Validated in WSL 2026-07-05: `--features linux-vulkan` compiles, all 86
   tests pass, and with no usable Vulkan device ggml falls back to CPU
   cleanly (no crash) — ggml ignores CPU-type devices like llvmpipe, so WSL
   can't exercise the GPU-on path. Remaining before shipping it in releases:
   one run on real Linux hardware with a proper Vulkan driver (correctness +
   perf), then decide whether it becomes a separate release artifact or the
   default. Build recipe documented in docs/compile/linux.md.*
8. Minimal E2E smoke test in CI — **done** (test.yml runs the app under Xvfb
   and asserts the window exists; mirrored in scripts/ci-local.sh).

### 0.5.0 — go public

9. macOS Developer ID signing + notarization (ad-hoc today → Gatekeeper
   blocks public users). Consider Windows code signing for SmartScreen.
10. Make the repo public → auto-update unfreezes (see standing decisions).
11. **Flatpak** (deferred here from 0.3.10 on 2026-07-03): distribution-only,
    no app changes. A ready manifest + release-workflow job live on the
    closed PR #46 / branch `feat/flatpak` — reuse them, then submit to
    Flathub (needs the public repo). AppImage already ships in every release
    (it's the updater's Linux format) — nothing to add there.

### 0.6.x — shipped (current version 0.6.3)

12. **Backend-agnostic cleanup via the OpenAI-compatible API** — **shipped in
    0.6.0/0.6.1** (2026-07-07). `Settings.cleanup_api` (`Ollama` default |
    `Openai`) + `api_key`; `cleanup/ollama.rs` dispatches `chat()`/`list_models()`
    to Ollama native (`/api/chat`, `/api/tags`) or OpenAI-compatible
    (`/v1/chat/completions`, `/v1/models`), with `openai_base()` URL
    normalization. Any `/v1` server drives cleanup — llama.cpp-server, LM
    Studio, Ollama's own `/v1`, antirez's **DS4**. Additive; Ollama stays the
    default. Verified end-to-end against Ollama's `/v1`. How-to blog:
    `docs/blog/use-ds4.html`.
13. **Linux clipboard fix** — **shipped in 0.6.1** (2026-07-07). A transient
    `arboard` set doesn't persist on Linux (the selection owner drops when the
    `Clipboard` value drops), so Copy + paste were silent no-ops. Now clipboard
    writes go through `wl-copy` (Wayland) / `xclip`|`xsel` (X11), which fork a
    daemon holding the selection; `copy_text` shares `inject::set_clipboard`.
    Confirmed on X11 and (with `wtype` for the keystroke) Wayland/KDE Plasma by
    the issue-#40 reporter. Linux users now need a clipboard helper + a Wayland
    keystroke tool — documented in `docs/compile/linux.md` +
    `docs/blog/linux-injection.html`.
14. **Reuse installed models** — **shipped in 0.6.2** (2026-07-09), from a Mac
    user's onboarding feedback. Ollama: if the daemon has models but the
    configured one is absent, auto-adopt an installed model (prefer a small
    instruct) and tell the user, instead of forcing a specific download — the
    "pull a model" nag only fires when the server has zero models. Whisper: new
    `list_whisper_models` command scans the models dir for any `ggml-*.bin`; the
    picker surfaces them ("· installed") so pointing Settings → Models folder at
    a directory shared with other whisper.cpp tools reuses them (ggml only;
    OpenAI/MLX whisper files are not whisper.cpp-compatible).
15. **Security review batch + dictionary fix** — **shipped in 0.6.3**
    (2026-08-27), from the security review published as issues #89–#95
    (all closed, fixes merged to main): whisper model name validated
    (`[A-Za-z0-9._-]+`) + resolved path confined to the models dir (#89);
    downloads SHA-256-verified against the upstream-published digest
    (HuggingFace tree API `lfs.oid`), fail-closed, with connect/idle
    timeouts replacing `timeout(None)` (#90); strict CSP on the app windows
    — capabilities audited clean, devtools off in release (#91); warning in
    Settings when the cleanup endpoint is remote (#92); paste-injection
    sleeps audited per platform + residual races documented (#93); privacy
    disclosures (auto-paste without review, hotkey over password fields)
    in README + blog (#94); `Settings::save` no longer panics on
    serialization, local API security design documented (#95). Plus #88:
    Personal Dictionary textarea keeps its raw text so typed newlines are
    no longer swallowed (controlled-component round-trip bug).
16. **Memory optimization** — **shipped in 0.6.3** (2026-08-31, PR #96).
    The audio callback converts to 16 kHz mono incrementally
    (`StreamResampler`, output bit-identical to the batch path — proven by
    equivalence tests), so the raw device-rate capture is never buffered
    whole (~64 KB/s instead of ~384 KB/s for 48 kHz stereo) and the live
    preview snapshot is a plain copy instead of a per-tick full
    reconversion (was quadratic over the recording). Plus idle model
    unload: a background thread drops the transcriber after 15 min unused
    (constant for now; try_lock + recording check, so it never blocks or
    races a dictation) — model RAM is only held while dictating. The
    published v0.6.3 release includes this along with the #88–#95 batch.

### 0.7–0.10 — speech-to-text workbench (agreed 2026-09-24)

Plan and checklists: `docs/superpowers/plans/2026-09-24-sussurro-speech-workbench.md`.

Work is tracked as GitHub issues in milestones `Phase 0 — Spikes`, `0.7 — Notetaking`,
`0.8 — Advanced notetaking + links`, `0.9 — Meeting`, `0.10 — Advanced meeting`,
`Track E — Qwen3-ASR sidecar` and `Future`, with one epic issue per milestone
(#147–#152; Future is the single tracking issue #146); agents take issues
labelled `agent-ready`.

- **Phase 0** — spikes (browser capture on Meet/Teams/Zoom web, Meet
  speaker names, Silero VAD, speaker embeddings vs Sortformer, word
  timings, Qwen3-ASR benchmark). Gates 0.9 and Track E only.
- **0.7 — Notetaking**: remove command mode; archive core; long-form
  engine for mic + file (streamed decode, VAD segments, chunked cleanup);
  UI shell A behind `ui_v2` until release.
- **0.8 — Advanced notetaking + links**: LLM profiles, recipes (formatted
  companion document with tl;dr/headings/tables), Ask panel, URL source
  (`yt-dlp` on PATH).
- **0.9 — Meeting**: browser extension (Chromium + Firefox), Meet names +
  "Voice N", People registry, SRT/VTT, facet search.
- **0.10 — Advanced meeting**: system audio as a second channel, opt-in
  WAV, per-speaker replay.
- **Track E** (parallel): Qwen3-ASR via `llama-server` sidecar if the
  benchmark gate passes.
- **Future (tracked, not built)**: voice recognition after training,
  text-to-speech, voice cloning with consent.

### Candidate / not committed

- **macOS Developer ID + notarization** and **Windows SignPath signing** —
  deferred (see standing decisions); installers remain unsigned for now.
- **Flatpak + Flathub** (roadmap item 11, still deferred).

## Per-machine setup

- **Windows dev machines must set a short `CARGO_TARGET_DIR`**
  (e.g. `setx CARGO_TARGET_DIR "C:\sbuild"`): whisper.cpp's Vulkan shader
  sub-build exceeds MAX_PATH otherwise (MSBuild FTK1011). Full details in
  `docs/compile/windows.md`.
- **Never commit a `.cargo/config.toml` with `target-dir`**: a drive-letter
  path is relative on Linux/macOS and its `:` breaks `cargo test`
  (`failed to join paths from $LD_LIBRARY_PATH`). One was removed from the
  repo for exactly this reason.

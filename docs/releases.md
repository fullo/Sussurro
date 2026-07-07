# Releases & auto-update

Maintainer-facing. How Sussurro is built, signed and shipped, and how the
in-app updater works.

## Auto-update (live once the repo is public)

The updater is fully wired and needs **no code**: the app's **Check for
updates** button calls `check()` → `downloadAndInstall()` → `relaunch()`
([App.tsx](../sussurro/src/App.tsx)), reading the signed `latest.json` from

    https://github.com/fullo/Sussurro/releases/latest/download/latest.json

`latest` resolves to the newest **published** (non-draft) release. Every
release's updater artifacts are signed with the project's minisign key
(`TAURI_SIGNING_PRIVATE_KEY`), verified against `plugins.updater.pubkey` in
`tauri.conf.json`. The only thing that gates it is repository visibility: while
the repo was **private** the endpoint 404'd for anonymous clients. **Once the
repo is public (v0.5.0), the updater works immediately** — no further change.

**Go-live steps:** (1) GitHub → *Settings → General → Danger Zone → Change
visibility → Public*. (2) Confirm the latest release (currently v0.4.1) is
**published, not draft**. (3) Smoke-test: install an older build, click *Check
for updates*, confirm it fetches and installs the newer version.

## OS code signing (status)

Separate from the updater's own signing. **Decision (2026-07-06): no external
OS signing for now** — the installers ship unsigned and users click through the
OS warnings. The updater still works (its minisign signature is independent).
The options below are documented for when the maintainer revisits.

- **macOS** — **ad-hoc signed** today (no Apple Developer ID / notarization),
  so Gatekeeper blocks first launch; users right-click → *Open*. See
  [`docs/blog/macos-signing-gatekeeper.html`](blog/macos-signing-gatekeeper.html).
  Developer ID + notarization would need an Apple Developer account ($99/yr).
- **Windows** — **unsigned** today (SmartScreen prompt). Future option:
  **SignPath** (free for OSS); setup guide kept in
  [`windows-signing-signpath.md`](windows-signing-signpath.md). Not being
  pursued right now.
- **Linux** — AppImage/deb/rpm are unsigned by OS convention; the updater
  artifacts are minisign-signed like the others.

## Cutting a release

The version lives in four files, kept in sync:
`sussurro/package.json`, `sussurro/src-tauri/tauri.conf.json`,
`sussurro/src-tauri/Cargo.toml` (+ `Cargo.lock`).

Pushing a `v*` tag (e.g. `git tag v0.4.1 && git push origin v0.4.1`) triggers
`.github/workflows/release.yml`: signed installers for Windows, macOS (Apple
Silicon) and Linux, published as a **draft** release together with the updater
manifest (`latest.json`). Publish by un-drafting.

- Never force-move an existing release tag — bump the patch version instead.
- The `v0.2.0` draft release is kept intentionally; do not delete it.

## Updater signing key (one-time setup)

The updater artifacts are signed with a minisign key **kept outside the repo**.
Generate one (any OS):

```bash
cd sussurro
npm run tauri signer generate -- -w ~/.tauri/sussurro-updater.key
```

This prints the public key — it must match `plugins.updater.pubkey` in
`sussurro/src-tauri/tauri.conf.json`. **Changing the key pair orphans every
previously installed app**, which would no longer trust new releases.

Upload the **private** key to the repo secrets, from the directory where it
lives:

```bash
# macOS / Linux
gh secret set TAURI_SIGNING_PRIVATE_KEY < ~/.tauri/sussurro-updater.key
gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD --body '""'
```

```powershell
# Windows (PowerShell)
gh secret set TAURI_SIGNING_PRIVATE_KEY --body (Get-Content $HOME\.tauri\sussurro-updater.key -Raw)
gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD --body '""'
```

The password secret is the passphrase chosen at key generation; `'""'` means
"empty passphrase". Back the key file up — losing it means a new key pair and
orphaned installs.

## Manual release (GitHub Actions unavailable)

When Actions minutes are exhausted, releases are built locally:

- **Windows** — built and signed on the dev box (`npm run tauri build`, then
  `tauri signer sign`). PowerShell drops empty `""` args and unsets empty env
  vars, so sign via `cmd /c` on Windows.
- **Linux** — built in WSL (Ubuntu 24.04) via `scripts/build-linux-release.sh`,
  which clones the tag, builds, and runs `scripts/sign-linux-bundles.sh`.
- **macOS** — added from a Mac afterwards:

  ```bash
  git clone <repo> && cd Sussurro/sussurro && git checkout vX.Y.Z
  npm ci && npm run tauri build          # Apple Silicon
  node_modules/.bin/tauri signer sign \
    --private-key-path <sussurro-updater.key> --password "" \
    src-tauri/target/release/bundle/macos/sussurro.app.tar.gz
  gh release upload vX.Y.Z src-tauri/target/release/bundle/macos/sussurro.app.tar.gz* \
    src-tauri/target/release/bundle/dmg/*.dmg
  ```

Then hand-write `latest.json` with the per-platform `signature` (from each
`.sig`) + download URL, and `gh release create` / `gh release upload` the
assets. Add the `darwin-aarch64` entry once the macOS assets are up.

## Planned signing (v0.5.0, go-public)

- macOS **Developer ID signing + notarization** — ad-hoc today, so Gatekeeper
  blocks public users.
- Consider **Windows code signing** for SmartScreen.

See [`CLAUDE.md`](../CLAUDE.md) for the full CI gotchas (ort/ONNX Runtime
pinning, cdn.pyke.io 403s, glibc baseline) and the release roadmap.

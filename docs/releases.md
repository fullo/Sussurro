# Releases & auto-update

Maintainer-facing. How Sussurro is built, signed and shipped, and how the
in-app updater works.

> **⚠️ Auto-update is frozen until v0.5.0.** The repo is private while the
> per-platform builds stabilize, so installed apps cannot reach `latest.json`
> anonymously (the endpoint 404s). The in-app **Check for updates** button and
> the signing pipeline below keep working — releases are simply not reachable
> by the updater until the repo (or at least its releases) goes public, planned
> for v0.5.0.

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

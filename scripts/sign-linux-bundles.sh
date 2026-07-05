#!/usr/bin/env bash
# Sign the Linux release bundles with the updater key (used when releases are
# built locally in WSL instead of on GitHub runners).
set -euo pipefail
KEY=/mnt/f/claude/keys/sussurro-updater.key
cd "$HOME/ci/sussurro/sussurro"
for f in "$HOME"/ci/target/release/bundle/deb/*.deb \
         "$HOME"/ci/target/release/bundle/rpm/*.rpm \
         "$HOME"/ci/target/release/bundle/appimage/*.AppImage; do
  node_modules/.bin/tauri signer sign --private-key-path "$KEY" --password "" "$f" >/dev/null
  echo "signed: $(basename "$f")"
done
ls -la "$HOME"/ci/target/release/bundle/*/*.sig

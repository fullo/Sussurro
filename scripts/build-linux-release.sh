#!/usr/bin/env bash
# Build the signed Linux release bundles (deb/rpm/AppImage) from a tag,
# inside the Ubuntu-dev WSL distro. From Windows:
#   wsl -d Ubuntu-dev -u root -- bash /mnt/f/GitHub/Sussurro/scripts/build-linux-release.sh vX.Y.Z
set -euo pipefail
TAG="${1:?usage: build-linux-release.sh <tag>}"
source "$HOME/.cargo/env"
export CARGO_TARGET_DIR="$HOME/ci/target"
export ORT_LIB_LOCATION="$HOME/ci/onnxruntime-linux-x64-1.24.2/lib"
export ORT_PREFER_DYNAMIC_LINK=1
export LD_LIBRARY_PATH="$ORT_LIB_LOCATION:${LD_LIBRARY_PATH:-}"

rm -rf "$HOME/ci/sussurro"
git clone -q --branch "$TAG" /mnt/f/GitHub/Sussurro "$HOME/ci/sussurro"
cd "$HOME/ci/sussurro/sussurro"
npm ci --no-audit --no-fund >/dev/null 2>&1
# llama-server sidecar (#116): pinned + SHA-256-verified (fails closed), then
# bundled via the merge config; linuxdeploy (AppImage) needs its libs on the
# loader path.
npm run sidecar
LD_LIBRARY_PATH="$PWD/src-tauri/binaries/llama-server-libs:$LD_LIBRARY_PATH" \
  npm run tauri build -- --config src-tauri/tauri.sidecar.conf.json 2>&1 | tail -8
bash ../scripts/verify-sidecar-bundle.sh x86_64-unknown-linux-gnu "$CARGO_TARGET_DIR"

bash /mnt/f/GitHub/Sussurro/scripts/sign-linux-bundles.sh

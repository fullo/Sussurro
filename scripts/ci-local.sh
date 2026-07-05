#!/usr/bin/env bash
# Local mirror of .github/workflows/test.yml — for when GitHub Actions is
# unavailable (e.g. private-repo minutes exhausted, runner outages).
#
# Runs inside WSL2 Ubuntu 24.04 (same glibc >= 2.38 baseline as the CI
# runner). From Windows:
#
#   wsl -d Ubuntu -u root -- bash /mnt/f/GitHub/Sussurro/scripts/ci-local.sh <branch>
#
# The branch is cloned from the Windows working copy (committed state only:
# uncommitted changes are NOT tested, exactly like real CI). Toolchains and
# the ONNX Runtime download persist in $HOME across runs; only the checkout
# is refreshed.
set -euo pipefail
BRANCH="${1:-main}"
ORT_VERSION=1.24.2   # keep in sync with test.yml / locked ort-sys
export DEBIAN_FRONTEND=noninteractive

echo "== [1/7] System dependencies"
apt-get update -qq
# clang/libclang-dev: preinstalled on GitHub runners, needed by bindgen
# (whisper-rs-sys). The rest mirrors test.yml's apt list.
apt-get install -y -qq build-essential cmake pkg-config curl git file \
  clang libclang-dev \
  libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev \
  librsvg2-dev libasound2-dev libxdo-dev libxkbcommon-dev libssl-dev \
  xvfb xdotool >/dev/null

echo "== [2/7] Node 24 (Vite 7 needs >= 22.12)"
if ! node --version 2>/dev/null | grep -qE '^v2[4-9]'; then
  curl -fsSL https://deb.nodesource.com/setup_24.x | bash - >/dev/null 2>&1
  apt-get install -y -qq nodejs >/dev/null
fi
node --version

echo "== [3/7] Rust stable + clippy"
if [ ! -x "$HOME/.cargo/bin/cargo" ]; then
  curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs \
    | sh -s -- -y --profile minimal -c clippy >/dev/null
fi
# shellcheck disable=SC1091
source "$HOME/.cargo/env"
rustc --version

echo "== [4/7] ONNX Runtime ${ORT_VERSION} (Microsoft build — pyke CDN 403s)"
mkdir -p "$HOME/ci" && cd "$HOME/ci"
if [ ! -d "onnxruntime-linux-x64-${ORT_VERSION}" ]; then
  curl -fsSL --retry 5 --retry-delay 10 -o ort.tgz \
    "https://github.com/microsoft/onnxruntime/releases/download/v${ORT_VERSION}/onnxruntime-linux-x64-${ORT_VERSION}.tgz"
  tar xzf ort.tgz && rm ort.tgz
fi
export ORT_LIB_LOCATION="$HOME/ci/onnxruntime-linux-x64-${ORT_VERSION}/lib"
export ORT_PREFER_DYNAMIC_LINK=1
export LD_LIBRARY_PATH="$ORT_LIB_LOCATION:${LD_LIBRARY_PATH:-}"

echo "== [5/7] Checkout ${BRANCH} from the Windows working copy"
# The Windows repo is owned by a different uid than the WSL user: git refuses
# it ("dubious ownership") for the repo AND its .git dir. This is a throwaway
# root-only CI environment, so trust everything.
git config --global --add safe.directory '*' 2>/dev/null || true
rm -rf "$HOME/ci/sussurro"
git clone -q --branch "$BRANCH" /mnt/f/GitHub/Sussurro "$HOME/ci/sussurro"
cd "$HOME/ci/sussurro/sussurro"

echo "== [6/7] Frontend build (type-check)"
npm ci --no-audit --no-fund >/dev/null
npm run build

echo "== [7/7] Rust tests + clippy + E2E smoke"
# Persistent target dir: the checkout is wiped every run, the build cache
# must not go with it.
export CARGO_TARGET_DIR="$HOME/ci/target"
cd src-tauri
cargo test
cargo clippy --all-targets
cd ..
# WSL has no GPU for WebKit: force software rendering everywhere (harmless
# on real hardware, required under Xvfb-in-WSL where Mesa/ZINK finds no pdev).
export WEBKIT_DISABLE_DMABUF_RENDERER=1
export WEBKIT_DISABLE_COMPOSITING_MODE=1
export LIBGL_ALWAYS_SOFTWARE=1
export GDK_BACKEND=x11
npm run tauri build -- --debug --no-bundle
xvfb-run -a bash -c '
  "$CARGO_TARGET_DIR/debug/sussurro" &
  APP_PID=$!
  for i in $(seq 1 30); do
    if xdotool search --name "Sussurro" >/dev/null 2>&1; then
      echo "OK: window found after ${i}s"
      kill $APP_PID
      exit 0
    fi
    if ! kill -0 $APP_PID 2>/dev/null; then
      echo "FAIL: app process died"
      exit 1
    fi
    sleep 1
  done
  echo "FAIL: window never appeared"
  kill $APP_PID
  exit 1
'
echo "== LOCAL CI PASSED (${BRANCH}) =="

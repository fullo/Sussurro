#!/usr/bin/env bash
# Checks that a release build really contains the verified llama-server
# sidecar (#116): the bundled executable is byte-identical to the one
# `npm run sidecar` verified, its libraries are there, and it starts
# (`--version`) with the documented runtime contract (cwd + library path =
# the lib folder).
#
#   scripts/verify-sidecar-bundle.sh <target-triple> <cargo-target-dir> [release|debug]
#
# e.g. scripts/verify-sidecar-bundle.sh aarch64-apple-darwin sussurro/src-tauri/target/aarch64-apple-darwin
#      scripts/verify-sidecar-bundle.sh x86_64-unknown-linux-gnu sussurro/src-tauri/target
set -euo pipefail
TRIPLE="${1:?usage: verify-sidecar-bundle.sh <target-triple> <cargo-target-dir>}"
TARGET_DIR="${2:?usage: verify-sidecar-bundle.sh <target-triple> <cargo-target-dir>}"
PROFILE="${3:-release}"
HERE="$(cd "$(dirname "$0")/.." && pwd)"
BIN_DIR="$HERE/sussurro/src-tauri/binaries"
NAME=sussurro-llama-server
EXE=""
[[ "$TRIPLE" == *windows* ]] && EXE=".exe"
VERIFIED="$BIN_DIR/$NAME-$TRIPLE$EXE"
[ -f "$VERIFIED" ] || { echo "FAIL: $VERIFIED missing — run npm run sidecar first"; exit 1; }
BUNDLE="$TARGET_DIR/$PROFILE/bundle"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

run_version() { # <binary> <lib dir> — the runtime contract: cwd + library path
  local bin libs out var
  bin="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
  libs="$(cd "$2" && pwd)"
  if [[ "$TRIPLE" == *apple-darwin ]]; then var=DYLD_LIBRARY_PATH
  elif [[ "$TRIPLE" == *windows* ]]; then var=PATH
  else var=LD_LIBRARY_PATH; fi
  out="$(cd "$libs" && env "$var=$libs${!var:+:${!var}}" "$bin" --version 2>&1)" ||
    { echo "$out"; echo "FAIL: $bin --version exited non-zero"; exit 1; }
  echo "$out" | grep -E "^version:" || { echo "$out"; echo "FAIL: no version line"; exit 1; }
}

check() { # <bundled binary> <bundled lib dir> <label>
  local bin="$1" libs="$2" label="$3"
  [ -f "$bin" ] || { echo "FAIL [$label]: no $NAME in the bundle"; exit 1; }
  cmp -s "$bin" "$VERIFIED" || { echo "FAIL [$label]: bundled $NAME differs from the verified one"; exit 1; }
  [ -d "$libs" ] || { echo "FAIL [$label]: no llama-server-libs in the bundle"; exit 1; }
  for f in "$BIN_DIR"/llama-server-libs/*; do
    cmp -s "$f" "$libs/$(basename "$f")" || { echo "FAIL [$label]: $(basename "$f") missing or different"; exit 1; }
  done
  run_version "$bin" "$libs"
  echo "OK [$label]: $NAME + $(ls "$libs" | wc -l | tr -d ' ') files in llama-server-libs"
}

case "$TRIPLE" in
  *apple-darwin)
    app="$(ls -d "$BUNDLE"/macos/*.app | head -1)"
    check "$app/Contents/MacOS/$NAME" "$app/Contents/Resources/llama-server-libs" "macOS .app"
    ;;
  *linux*)
    deb="$(ls "$BUNDLE"/deb/*.deb | head -1)"
    dpkg-deb -x "$deb" "$WORK/deb"
    check "$WORK/deb/usr/bin/$NAME" "$(find "$WORK/deb" -type d -name llama-server-libs | head -1)" ".deb"
    dpkg-deb -f "$deb" Depends | grep -q libgomp1 || { echo "FAIL [.deb]: Depends lacks libgomp1"; exit 1; }
    # AppImage: linuxdeploy rewrites the executable's rpath and copies its
    # NEEDED libs into usr/lib, so only presence + start-up are checked.
    img="$(cd "$BUNDLE/appimage" && pwd)/$(ls "$BUNDLE"/appimage | grep '\.AppImage$' | head -1)"
    (cd "$WORK" && "$img" --appimage-extract >/dev/null)
    [ -f "$WORK/squashfs-root/usr/bin/$NAME" ] || { echo "FAIL [AppImage]: no $NAME"; exit 1; }
    run_version "$WORK/squashfs-root/usr/bin/$NAME" "$(find "$WORK/squashfs-root" -type d -name llama-server-libs | head -1)"
    echo "OK [AppImage]"
    ;;
  *windows*)
    # tauri-build stages externalBin + resources next to the exe; the NSIS
    # and MSI installers are built from there.
    check "$TARGET_DIR/$PROFILE/$NAME$EXE" "$TARGET_DIR/$PROFILE/llama-server-libs" "$PROFILE dir"
    # Best effort on the installer itself: 7-Zip reads NSIS installers (the
    # MSI's file table isn't listable by name, so it's not checked).
    for inst in "$BUNDLE"/nsis/*.exe; do
      [ -f "$inst" ] || continue
      if 7z l "$inst" 2>/dev/null | grep -q "$NAME"; then
        echo "OK [$(basename "$inst")]: lists $NAME"
      else
        echo "::warning::could not confirm $NAME inside $(basename "$inst") with 7z"
      fi
    done
    ;;
  *) echo "FAIL: unknown target $TRIPLE"; exit 1 ;;
esac

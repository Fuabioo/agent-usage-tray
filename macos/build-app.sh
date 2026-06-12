#!/usr/bin/env bash
# Assemble AgentUsageMenuBar.app with the `agent-usage` CLI bundled inside, ad-hoc signed.
#
# Usage: macos/build-app.sh [--debug] [OUTPUT_DIR]
#   --debug      build unoptimized (faster); default is release
#   OUTPUT_DIR   where to write the .app (default: macos/build)
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PKG="$ROOT/macos/AgentUsageMenuBar"

CONFIG="release"
CARGO_FLAG="--release"
CARGO_DIR="release"
OUT="$ROOT/macos/build"

for arg in "$@"; do
  case "$arg" in
    --debug) CONFIG="debug"; CARGO_FLAG=""; CARGO_DIR="debug" ;;
    *) OUT="$arg" ;;
  esac
done

echo "==> Building agent-usage CLI ($CONFIG)"
( cd "$ROOT" && cargo build $CARGO_FLAG -p agent-usage-cli )
CLI="$ROOT/target/$CARGO_DIR/agent-usage"

echo "==> Building AgentUsageMenuBar ($CONFIG)"
swift build -c "$CONFIG" --package-path "$PKG"
APP_BIN="$(swift build -c "$CONFIG" --package-path "$PKG" --show-bin-path)/AgentUsageMenuBar"

APP="$OUT/AgentUsageMenuBar.app"
echo "==> Assembling $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$APP_BIN" "$APP/Contents/MacOS/AgentUsageMenuBar"
cp "$CLI" "$APP/Contents/Resources/agent-usage"
cp "$PKG/Resources/Info.plist" "$APP/Contents/Info.plist"

# Bundle per-agent logos as vector PDFs (regenerate from the SVGs with macos/render-logos.sh
# when a logo changes). The app loads <agent-id>.pdf from Resources and tints it per pace.
AGENTS_DIR="$PKG/Resources/agents"
if [ -d "$AGENTS_DIR" ]; then
  for pdf in "$AGENTS_DIR"/*.pdf; do
    [ -e "$pdf" ] || continue
    cp "$pdf" "$APP/Contents/Resources/$(basename "$pdf")"
  done
fi

echo "==> Ad-hoc signing"
codesign --force --deep --sign - "$APP"

echo "==> Done: $APP"
echo "    Launch with: open \"$APP\"   (adds the menu bar item)"

#!/usr/bin/env bash
# Regenerate per-agent logo PNGs from their SVGs.
#
# We render with headless Chrome because it honors SVGs exactly as a browser does — viewBox,
# `em` sizing, and `fill-rule` knockouts (the Codex mark's `>_` are transparent holes). Earlier
# attempts via QuickLook (opaque white background) and AppKit's CoreSVG (mis-positioned the
# viewBox) both got it wrong. The SVG is inlined with `color:#000` so `currentColor` resolves to
# black on a transparent page — yielding a black silhouette with transparent holes that the app
# tints to any pace color.
#
# Run this only when an SVG under Resources/agents/ changes; the resulting PNGs are committed and
# build-app.sh just copies them. Requires Google Chrome (override with $CHROME).
set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/AgentUsageMenuBar/Resources/agents"
[ -d "$DIR" ] || { echo "no agents dir: $DIR"; exit 1; }

CHROME="${CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
[ -x "$CHROME" ] || { echo "Chrome not found at: $CHROME (set \$CHROME)"; exit 1; }

SIZE="${SIZE:-1024}"

render() { # $1 = svg in, $2 = png out
  local svg html
  svg="$(cat "$1")"
  html="$(mktemp -t logo-XXXXXX).html"
  cat > "$html" <<HTML
<!doctype html><html><head><style>
html,body{margin:0;padding:0;background:transparent}
#b{width:100vw;height:100vh;color:#000;display:flex;align-items:center;justify-content:center}
#b svg{width:100%;height:100%;display:block}
</style></head><body><div id="b">$svg</div></body></html>
HTML
  "$CHROME" --headless --disable-gpu --hide-scrollbars --force-device-scale-factor=1 \
    --default-background-color=00000000 --window-size="$SIZE,$SIZE" \
    --screenshot="$2" "file://$html" >/dev/null 2>&1
  rm -f "$html"
}

for svg in "$DIR"/*.svg; do
  [ -e "$svg" ] || continue
  name="$(basename "${svg%.svg}")"
  echo "==> rendering $name"
  render "$svg" "$DIR/$name.png"
  echo "    wrote $DIR/$name.png"
done
echo "Done. PNGs are transparent black templates (commit them)."

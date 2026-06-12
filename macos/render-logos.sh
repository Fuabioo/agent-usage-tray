#!/usr/bin/env bash
# Regenerate per-agent logo assets from their SVGs as **vector PDFs**.
#
# We want true vectors so the glyphs stay crisp at any size (menu bar ~13pt, dashboard rings).
# AppKit's CoreSVG mis-positions these SVGs, and a raster PNG looks grainy when downscaled. So
# we print each SVG to a vector PDF with headless Chrome (which renders SVGs exactly like a
# browser — viewBox, `em` sizing, `fill-rule` knockouts) on a transparent page. NSImage loads
# the PDF as a resolution-independent NSPDFImageRep; the app tints it per pace.
#
# Run this only when an SVG under Resources/agents/ changes; the resulting PDFs are committed and
# build-app.sh just copies them. Requires Google Chrome (override with $CHROME).
set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/AgentUsageMenuBar/Resources/agents"
[ -d "$DIR" ] || { echo "no agents dir: $DIR"; exit 1; }

CHROME="${CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
[ -x "$CHROME" ] || { echo "Chrome not found at: $CHROME (set \$CHROME)"; exit 1; }

# Page size in CSS px (the logo fills it edge to edge); only sets the PDF's nominal size — it
# stays vector, so the value doesn't affect crispness.
PX="${PX:-256}"

render() { # $1 = svg in, $2 = pdf out
  local svg html
  svg="$(cat "$1")"
  html="$(mktemp -t logo-XXXXXX).html"
  cat > "$html" <<HTML
<!doctype html><html><head><style>
@page{size:${PX}px ${PX}px;margin:0}
html,body{margin:0;padding:0;background:transparent}
#b{width:${PX}px;height:${PX}px;color:#000}
#b svg{width:100%;height:100%;display:block}
</style></head><body><div id="b">$svg</div></body></html>
HTML
  "$CHROME" --headless --disable-gpu --no-pdf-header-footer \
    --print-to-pdf="$2" "file://$html" >/dev/null 2>&1
  rm -f "$html"
}

for svg in "$DIR"/*.svg; do
  [ -e "$svg" ] || continue
  name="$(basename "${svg%.svg}")"
  echo "==> rendering $name"
  render "$svg" "$DIR/$name.pdf"
  echo "    wrote $DIR/$name.pdf"
done
echo "Done. Vector PDFs written (commit them)."

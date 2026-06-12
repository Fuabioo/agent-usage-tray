#!/usr/bin/env bash
# Regenerate per-agent logo PNGs from their SVGs.
#
# AppKit's NSImage rasterizes an SVG on a transparent background, honoring its viewBox. We draw
# each logo into a fixed square canvas and force it to solid black (preserving the shape's
# alpha) — yielding a tintable template the app recolors to any pace color. This replaces an
# earlier QuickLook approach, which baked an opaque white background and mis-sized `em` SVGs.
#
# Run this only when an SVG under Resources/agents/ changes; the resulting PNGs are committed and
# build-app.sh just copies them. Requires macOS (swift + AppKit).
set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/AgentUsageMenuBar/Resources/agents"
[ -d "$DIR" ] || { echo "no agents dir: $DIR"; exit 1; }

render() { # $1 = svg in, $2 = png out
  swift - "$1" "$2" <<'SWIFT'
import AppKit
let args = CommandLine.arguments
guard let img = NSImage(contentsOfFile: args[1]), img.isValid else {
    print("load fail: \(args[1])"); exit(1)
}
let n = 512
let rect = NSRect(x: 0, y: 0, width: n, height: n)
let canvas = NSImage(size: NSSize(width: n, height: n))
canvas.lockFocus()
img.draw(in: rect)                    // SVG renders on transparent bg
NSColor.black.set()
rect.fill(using: .sourceAtop)         // force shape to solid black, keep its alpha
canvas.unlockFocus()
guard let tiff = canvas.tiffRepresentation,
      let rep = NSBitmapImageRep(data: tiff),
      let png = rep.representation(using: .png, properties: [:]) else {
    print("encode fail"); exit(1)
}
try! png.write(to: URL(fileURLWithPath: args[2]))
SWIFT
}

for svg in "$DIR"/*.svg; do
  [ -e "$svg" ] || continue
  name="$(basename "${svg%.svg}")"
  echo "==> rendering $name"
  render "$svg" "$DIR/$name.png"
  echo "    wrote $DIR/$name.png"
done
echo "Done. PNGs are transparent black templates (commit them)."

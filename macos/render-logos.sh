#!/usr/bin/env bash
# Regenerate per-agent logo PNGs from their SVGs.
#
# QuickLook (qlmanage) rasterizes SVGs onto an opaque white background, which is useless as a
# tintable template. So we render, then convert black-on-white to transparent black via
# alpha = 1 - luminance — yielding an anti-aliased silhouette the app can tint to any pace color.
#
# Run this only when an SVG under Resources/agents/ changes; the resulting PNGs are committed and
# build-app.sh just copies them. Requires macOS (qlmanage + swift).
set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/AgentUsageMenuBar/Resources/agents"
[ -d "$DIR" ] || { echo "no agents dir: $DIR"; exit 1; }

transform() { # $1 = white-bg png in, $2 = transparent png out
  swift - "$1" "$2" <<'SWIFT'
import AppKit
import CoreGraphics
let args = CommandLine.arguments
guard let img = NSImage(contentsOfFile: args[1]),
      let cg = img.cgImage(forProposedRect: nil, context: nil, hints: nil)
else { print("load fail: \(args[1])"); exit(1) }

let w = cg.width, h = cg.height
let bpr = w * 4
var buf = [UInt8](repeating: 0, count: bpr * h)
let cs = CGColorSpaceCreateDeviceRGB()
guard let ctx = CGContext(
    data: &buf, width: w, height: h, bitsPerComponent: 8, bytesPerRow: bpr, space: cs,
    bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
else { print("ctx fail"); exit(1) }
ctx.draw(cg, in: CGRect(x: 0, y: 0, width: w, height: h))

// black-on-white -> transparent black: alpha = 1 - luminance, rgb = 0 (premultiplied).
var i = 0
while i < buf.count {
    let r = Double(buf[i]), g = Double(buf[i + 1]), b = Double(buf[i + 2])
    let lum = (0.299 * r + 0.587 * g + 0.114 * b) / 255.0
    let a = max(0.0, min(1.0, 1.0 - lum))
    buf[i] = 0; buf[i + 1] = 0; buf[i + 2] = 0; buf[i + 3] = UInt8(a * 255.0)
    i += 4
}

guard let outCg = ctx.makeImage() else { print("make fail"); exit(1) }
let rep = NSBitmapImageRep(cgImage: outCg)
guard let png = rep.representation(using: .png, properties: [:]) else { print("encode fail"); exit(1) }
try! png.write(to: URL(fileURLWithPath: args[2]))
SWIFT
}

for svg in "$DIR"/*.svg; do
  [ -e "$svg" ] || continue
  name="$(basename "${svg%.svg}")"
  tmp="$(mktemp -d)"
  echo "==> rendering $name"
  qlmanage -t -s 512 -o "$tmp" "$svg" >/dev/null 2>&1
  transform "$tmp/$(basename "$svg").png" "$DIR/$name.png"
  rm -rf "$tmp"
  echo "    wrote $DIR/$name.png"
done
echo "Done. PNGs are transparent black templates (commit them)."

import AppKit
import SwiftUI

/// Per-agent brand glyphs.
///
/// A logo is a black-on-transparent PNG named `<agent-id>.png`, bundled in the app's Resources
/// (rendered from the committed SVG under `Resources/agents/`). It's loaded as a *template*
/// image so it can be tinted to any pace color. Agents without a bundled logo fall back to an
/// SF Symbol, so dropping in `codex.png` later is all it takes to brand a new agent.
enum AgentAssets {
    private static var cache: [String: NSImage?] = [:]

    /// SF Symbol fallback per agent id.
    static func symbolName(forID id: String) -> String {
        switch id {
        case "claude": return "sparkle"
        case "codex": return "chevron.left.forwardslash.chevron.right"
        case "antigravity": return "a.circle"
        case "hyper": return "bolt.fill"
        default: return "cpu"
        }
    }

    /// The bundled template logo for an agent, or nil if none is shipped (use the SF Symbol).
    static func logo(forID id: String) -> NSImage? {
        if let cached = cache[id] { return cached }
        let image = loadLogo(id)
        cache[id] = image
        return image
    }

    private static func loadLogo(_ id: String) -> NSImage? {
        guard let url = Bundle.main.resourceURL?.appendingPathComponent("\(id).png"),
              let image = NSImage(contentsOf: url)
        else { return nil }
        image.isTemplate = true // tintable; we recolor by pace
        return image
    }

    /// An agent glyph recolored to `color` at `size` points, preferring the bundled logo and
    /// falling back to the SF Symbol. Used for menu bar drawing.
    static func tintedGlyph(forID id: String, color: NSColor, size: CGFloat) -> NSImage? {
        let base: NSImage?
        if let logo = logo(forID: id) {
            base = fit(logo, to: size)
        } else {
            let cfg = NSImage.SymbolConfiguration(pointSize: size, weight: .medium)
            base = NSImage(systemSymbolName: symbolName(forID: id), accessibilityDescription: nil)?
                .withSymbolConfiguration(cfg)
        }
        guard let base else { return nil }

        let dims = base.size
        let tinted = NSImage(size: dims, flipped: false) { rect in
            base.draw(in: rect)
            color.set()
            rect.fill(using: .sourceAtop)
            return true
        }
        tinted.isTemplate = false
        return tinted
    }

    /// A square copy of `image` scaled to `size` points (keeps menu bar glyphs uniform).
    private static func fit(_ image: NSImage, to size: CGFloat) -> NSImage {
        let target = NSSize(width: size, height: size)
        let out = NSImage(size: target, flipped: false) { rect in
            image.draw(in: rect, from: .zero, operation: .sourceOver, fraction: 1.0)
            return true
        }
        return out
    }
}

/// SwiftUI agent glyph: the bundled logo (tinted) or the SF Symbol fallback.
struct AgentGlyphView: View {
    let agentID: String
    let color: Color
    var size: CGFloat = 16

    var body: some View {
        if let logo = AgentAssets.logo(forID: agentID) {
            Image(nsImage: logo)
                .resizable()
                .renderingMode(.template)
                .interpolation(.high)
                .aspectRatio(contentMode: .fit)
                .frame(width: size, height: size)
                .foregroundStyle(color)
        } else {
            Image(systemName: AgentAssets.symbolName(forID: agentID))
                .font(.system(size: size, weight: .semibold))
                .foregroundStyle(color)
        }
    }
}

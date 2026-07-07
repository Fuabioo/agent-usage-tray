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
        case "hyper": return "diamond.fill" // Crush renders hypercredits as ◆ (U+25C6)
        default:
            // A secondary Claude login ("claude-…") gets a near-variant so it doesn't read as the
            // primary account when no bundled logo is present.
            return isSecondaryClaude(id) ? "sparkles" : "cpu"
        }
    }

    /// A configured extra Claude account renders as its own agent id, always "claude"-prefixed
    /// (see `ClaudeAccount.makeID`). It shares the Claude-family artwork via a distinct variant so
    /// two Claude accounts never show an identical glyph.
    static func isSecondaryClaude(_ id: String) -> Bool {
        id != "claude" && id.hasPrefix("claude")
    }

    /// The bundled-logo base name for an agent id: the primary Claude and every built-in map to
    /// `<id>`, while any secondary Claude account maps to the derived `claude-alt` variant.
    private static func logoBaseName(forID id: String) -> String {
        isSecondaryClaude(id) ? "claude-alt" : id
    }

    /// The bundled template logo for an agent, or nil if none is shipped (use the SF Symbol).
    static func logo(forID id: String) -> NSImage? {
        if let cached = cache[id] { return cached }
        let image = loadLogo(id)
        cache[id] = image
        return image
    }

    private static func loadLogo(_ id: String) -> NSImage? {
        // A vector PDF (rendered from the SVG), loaded as a resolution-independent
        // NSPDFImageRep so the glyph stays crisp at any size. We tint it manually, so it is not
        // a template image.
        guard let url = Bundle.main.resourceURL?.appendingPathComponent("\(logoBaseName(forID: id)).pdf"),
              let image = NSImage(contentsOf: url)
        else { return nil }
        image.isTemplate = false
        return image
    }

    /// The untinted glyph (bundled logo fitted to `size`, or the SF Symbol fallback).
    private static func baseGlyph(forID id: String, size: CGFloat) -> NSImage? {
        if let logo = logo(forID: id) {
            return fit(logo, to: size)
        }
        let cfg = NSImage.SymbolConfiguration(pointSize: size, weight: .medium)
        return NSImage(systemSymbolName: symbolName(forID: id), accessibilityDescription: nil)?
            .withSymbolConfiguration(cfg)
    }

    /// An agent glyph recolored to `color` at `size` points, preferring the bundled logo and
    /// falling back to the SF Symbol. Used for menu bar drawing.
    static func tintedGlyph(forID id: String, color: NSColor, size: CGFloat) -> NSImage? {
        guard let base = baseGlyph(forID: id, size: size) else { return nil }
        let tinted = NSImage(size: base.size, flipped: false) { rect in
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

/// SwiftUI agent glyph. Renders through the same Core Graphics path the menu bar uses — drawing
/// the vector logo at the target size and tinting it — so the dashboard glyphs are as crisp as
/// the bar. Falls back to an SF Symbol for agents without a bundled logo.
struct AgentGlyphView: View {
    let agentID: String
    let nsColor: NSColor
    var size: CGFloat = 16

    var body: some View {
        if let glyph = AgentAssets.tintedGlyph(forID: agentID, color: nsColor, size: size) {
            Image(nsImage: glyph)
                .frame(width: size, height: size)
        } else {
            Image(systemName: AgentAssets.symbolName(forID: agentID))
                .font(.system(size: size, weight: .semibold))
                .foregroundStyle(Color(nsColor: nsColor))
        }
    }
}

import AppKit
import SwiftUI

// MARK: - Pace color (mirrors the CLI's "green"/"yellow"/"red" contract)

/// Appearance-adaptive pace colors. As in the original app we avoid the stock
/// `.systemGreen/.systemYellow` (they lose contrast as text on the light popover, yellow
/// especially): each pace resolves to a darker, saturated variant in Light mode and a brighter
/// one in Dark mode, so text stays legible whether the app follows Light, Dark, or Auto.
enum PaceColor: String, Codable {
    case surplus, green, yellow, red

    /// Higher = more severe; used to pick an agent's "worst" pace for the menu bar glyph.
    var severity: Int {
        switch self {
        case .surplus: return 0
        case .green: return 1
        case .yellow: return 2
        case .red: return 3
        }
    }

    /// Whether this pace warrants attention (drives the "only yellow/red" menu bar mode).
    var needsAttention: Bool { self == .yellow || self == .red }

    var nsColor: NSColor {
        switch self {
        case .surplus:
            // A fresh mint/teal — clearly distinct from both the warning amber and the on-track
            // green (it leans cyan). Deep enough to read on the light popover, bright on dark.
            return .paceAdaptive(light: (0.000, 0.482, 0.420), dark: (0.341, 0.929, 0.792))
        case .green:
            return .paceAdaptive(light: (0.082, 0.502, 0.239), dark: (0.290, 0.871, 0.502))
        case .yellow:
            return .paceAdaptive(light: (0.706, 0.325, 0.035), dark: (0.984, 0.749, 0.141))
        case .red:
            return .paceAdaptive(light: (0.776, 0.157, 0.157), dark: (0.973, 0.443, 0.443))
        }
    }

    var swiftUIColor: Color { Color(nsColor: nsColor) }
}

private extension NSColor {
    static func paceAdaptive(
        light: (r: CGFloat, g: CGFloat, b: CGFloat),
        dark: (r: CGFloat, g: CGFloat, b: CGFloat)
    ) -> NSColor {
        NSColor(name: nil) { appearance in
            let isDark = appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
            let c = isDark ? dark : light
            return NSColor(srgbRed: c.r, green: c.g, blue: c.b, alpha: 1.0)
        }
    }
}

// MARK: - JSON DTOs (mirror crates/agent-usage-cli/src/output.rs; `agent-usage all --json`)

/// One agent's snapshot. `agent-usage all --json` returns an array of these.
struct AgentSnapshot: Codable, Identifiable {
    let agent: AgentDTO
    let fetchedAt: Date
    let config: ConfigDTO
    let windows: [WindowDTO]?
    let pace: PaceSummaryDTO?
    let error: ErrorDTO?
    /// Set by the CLI when it served a cached snapshot after a transient fetch failure.
    let stale: Bool?

    var id: String { agent.id }
    var isError: Bool { error != nil }
    var isStale: Bool { stale == true }

    /// The first window of a given kind ("weekly", "session", "credits").
    func window(_ kind: String) -> WindowDTO? {
        windows?.first { $0.kind == kind }
    }

    /// The headline window for the ring gauge: weekly, else credits, else session, else first.
    var primaryWindow: WindowDTO? {
        window("weekly") ?? window("credits") ?? window("session") ?? windows?.first
    }

    /// Most severe pace across all windows (drives the menu bar glyph tint).
    var worstPace: PaceColor {
        (windows ?? []).map(\.pace).max(by: { $0.severity < $1.severity }) ?? .green
    }

    /// Pace shown on the agent's glyph: anything needing attention dominates, otherwise a
    /// surplus is celebrated (gold), otherwise green.
    var displayPace: PaceColor {
        let paces = (windows ?? []).map(\.pace)
        if paces.contains(.red) { return .red }
        if paces.contains(.yellow) { return .yellow }
        if paces.contains(.surplus) { return .surplus }
        return .green
    }
}

struct AgentDTO: Codable {
    let id: String
    let label: String
    let source: String
}

struct ConfigDTO: Codable {
    let dailyBudget: Double
    let workDays: Int
}

struct WindowDTO: Codable {
    let kind: String
    let label: String
    let usedPct: Double
    let remainingPct: Double
    let resetsAt: Date?
    let resetsInSecs: Int?
    let pace: PaceColor
    let pool: PoolDTO?
}

struct PoolDTO: Codable {
    let total: Double
    let remaining: Double
    let burnPerDay: Double?
    let projectedDepletion: Date?
    let depletesBeforeReset: Bool
}

struct PaceSummaryDTO: Codable {
    let workDayIndex: Int
    let dailyCeiling: Double
    let remaining: Double
    let resetDayLocal: String
}

struct ErrorDTO: Codable {
    let kind: String
    let message: String
}

// MARK: - Decoding

extension JSONDecoder {
    /// Decoder for the CLI's snake_case keys and RFC3339 timestamps (chrono emits fractional
    /// seconds or not, depending on the value).
    static func agentUsage() -> JSONDecoder {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase

        let withFraction = ISO8601DateFormatter()
        withFraction.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        let plain = ISO8601DateFormatter()
        plain.formatOptions = [.withInternetDateTime]

        decoder.dateDecodingStrategy = .custom { d in
            let container = try d.singleValueContainer()
            let s = try container.decode(String.self)
            if let date = withFraction.date(from: s) ?? plain.date(from: s) {
                return date
            }
            throw DecodingError.dataCorruptedError(
                in: container, debugDescription: "Unrecognized date format: \(s)")
        }
        return decoder
    }
}

// MARK: - Duration / projection formatting

/// Format seconds as "3d 12h" / "2h 45m" / "45m" (minimum "1m").
func formatDuration(seconds: Int) -> String {
    if seconds <= 0 { return "0m" }
    let days = seconds / 86_400
    let hours = (seconds % 86_400) / 3_600
    let minutes = (seconds % 3_600) / 60
    switch (days, hours, minutes) {
    case let (d, h, _) where d > 0 && h > 0: return "\(d)d \(h)h"
    case let (d, _, _) where d > 0: return "\(d)d"
    case let (_, h, m) where h > 0 && m > 0: return "\(h)h \(m)m"
    case let (_, h, _) where h > 0: return "\(h)h"
    case let (_, _, m): return "\(max(m, 1))m"
    }
}

/// Short weekday like "Thu" for projection labels ("out ~Thu").
func shortWeekday(_ date: Date) -> String {
    let f = DateFormatter()
    f.dateFormat = "EEE"
    return f.string(from: date)
}

/// The exact reset moment in the user's local timezone, e.g. "Mon Jun 15, 8:00 PM".
/// `DateFormatter` uses the current timezone and locale by default, so this follows whatever
/// timezone the machine is in.
func localResetString(_ date: Date) -> String {
    let f = DateFormatter()
    f.dateFormat = "EEE MMM d, h:mm a"
    return f.string(from: date)
}

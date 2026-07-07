import AppKit
import Combine
import CryptoKit
import SwiftUI

/// User preferences, persisted in `UserDefaults`. Mirrors the prototype's settings that affect
/// behavior: how many work days the pace budget splits across, which agents to show, and the
/// app appearance. (Menu-bar display modes from the prototype are a later refinement.)
final class AppSettings: ObservableObject {
    enum Appearance: String, CaseIterable, Identifiable {
        case system, light, dark
        var id: String { rawValue }
        var label: String {
            switch self {
            case .system: return "System"
            case .light: return "Light"
            case .dark: return "Dark"
            }
        }
    }

    /// What the menu bar item shows (the prototype's "Menu bar shows:" options, plus an
    /// "only attention" filter).
    enum MenuBarMode: String, CaseIterable, Identifiable {
        case iconOnly          // each agent's glyph, tinted by its pace; no numbers
        case worstMetric       // the single highest utilization across agents, e.g. "88%"
        case iconWorst         // the worst agent's glyph + that number
        case perAgentPercent   // each agent: glyph + one % (its worst window)
        case perAgentBoth      // each agent: glyph + weekly · session %
        case onlyAttention     // per-agent · both, but only agents at yellow/red
        case selectedAgent     // only the chosen agent: glyph + weekly · session %

        var id: String { rawValue }

        var label: String {
            switch self {
            case .iconOnly: return "Icon only"
            case .worstMetric: return "Worst metric"
            case .iconWorst: return "Icon + worst"
            case .perAgentPercent: return "Per-agent %"
            case .perAgentBoth: return "Per-agent · both windows"
            case .onlyAttention: return "Only yellow / red"
            case .selectedAgent: return "Selected agent only"
            }
        }

        var detail: String {
            switch self {
            case .iconOnly: return "color-coded glyphs"
            case .worstMetric: return "e.g. “88%”"
            case .iconWorst: return "glyph + number"
            case .perAgentPercent: return "one per agent"
            case .perAgentBoth: return "5h + weekly"
            case .onlyAttention: return "hide on-track agents"
            case .selectedAgent: return "pick one"
            }
        }
    }

    // Range is enforced by the Stepper (1...7) and clamped on load; we deliberately do NOT
    // reassign `workDays` inside its own didSet (that republishes mid-update and, combined with a
    // synchronous subscriber, crashes).
    @Published var workDays: Int {
        didSet { defaults.set(workDays, forKey: Keys.workDays) }
    }

    @Published var appearance: Appearance {
        didSet {
            defaults.set(appearance.rawValue, forKey: Keys.appearance)
            applyAppearance()
        }
    }

    @Published var disabledAgentIDs: Set<String> {
        didSet { defaults.set(Array(disabledAgentIDs), forKey: Keys.disabledAgents) }
    }

    @Published var menuBarMode: MenuBarMode {
        didSet { defaults.set(menuBarMode.rawValue, forKey: Keys.menuBarMode) }
    }

    /// The agent shown in `.selectedAgent` mode (its id).
    @Published var selectedAgentID: String {
        didSet { defaults.set(selectedAgentID, forKey: Keys.selectedAgentID) }
    }

    /// How credit-pool windows read out their balance (raw credits, percentage, or both).
    enum CreditDisplay: String, CaseIterable, Identifiable {
        case credits      // the raw balance the API returns, e.g. "1,620"
        case percentage   // remaining percentage, e.g. "98%"
        case both         // "1,620 · 98%"

        var id: String { rawValue }
        var label: String {
            switch self {
            case .credits: return "Hypercredits"
            case .percentage: return "Percentage"
            case .both: return "Both"
            }
        }
    }

    @Published var creditDisplay: CreditDisplay {
        didSet { defaults.set(creditDisplay.rawValue, forKey: Keys.creditDisplay) }
    }

    /// Additional Claude Code logins to monitor alongside the primary account, each shown as its
    /// own agent (own id, glyph, and menu-bar segment). The app runs one extra `agent-usage claude`
    /// per account with `--id`/`--label`/`--config-dir` overrides so each resolves *its own* token.
    @Published var claudeAccounts: [ClaudeAccount] {
        didSet { persistClaudeAccounts() }
    }

    private let defaults: UserDefaults

    private enum Keys {
        static let workDays = "workDays"
        static let appearance = "appearance"
        static let disabledAgents = "disabledAgentIDs"
        static let menuBarMode = "menuBarMode"
        static let selectedAgentID = "selectedAgentID"
        static let creditDisplay = "creditDisplay"
        static let claudeAccounts = "claudeAccounts"
    }

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        let storedWorkDays = defaults.object(forKey: Keys.workDays) as? Int ?? 5
        self.workDays = min(max(storedWorkDays, 1), 7)
        self.appearance = Appearance(rawValue: defaults.string(forKey: Keys.appearance) ?? "")
            ?? .system
        self.disabledAgentIDs = Set(defaults.stringArray(forKey: Keys.disabledAgents) ?? [])
        self.menuBarMode = MenuBarMode(rawValue: defaults.string(forKey: Keys.menuBarMode) ?? "")
            ?? .perAgentBoth
        self.selectedAgentID = defaults.string(forKey: Keys.selectedAgentID) ?? ""
        self.creditDisplay = CreditDisplay(rawValue: defaults.string(forKey: Keys.creditDisplay) ?? "")
            ?? .both
        self.claudeAccounts = Self.loadClaudeAccounts(defaults)
    }

    private static func loadClaudeAccounts(_ defaults: UserDefaults) -> [ClaudeAccount] {
        guard let data = defaults.data(forKey: Keys.claudeAccounts),
              let accounts = try? JSONDecoder().decode([ClaudeAccount].self, from: data)
        else { return [] }
        return accounts
    }

    private func persistClaudeAccounts() {
        if let data = try? JSONEncoder().encode(claudeAccounts) {
            defaults.set(data, forKey: Keys.claudeAccounts)
        }
    }

    // MARK: - Claude accounts

    /// Add a Claude account, deriving a stable, unique agent id from its label. No-op if a blank
    /// label somehow slips through (the UI already guards against it).
    func addClaudeAccount(label: String, configDir: String, keychainService: String?) {
        let trimmedLabel = label.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmedLabel.isEmpty else { return }
        let id = ClaudeAccount.makeID(label: trimmedLabel, existing: claudeAccounts)
        let svc = keychainService?.trimmingCharacters(in: .whitespacesAndNewlines)
        claudeAccounts.append(ClaudeAccount(
            id: id,
            label: trimmedLabel,
            configDir: configDir.trimmingCharacters(in: .whitespacesAndNewlines),
            keychainService: (svc?.isEmpty ?? true) ? nil : svc))
    }

    func removeClaudeAccount(id: String) {
        claudeAccounts.removeAll { $0.id == id }
        // Drop any now-orphaned enable/selection state so it doesn't linger in UserDefaults.
        disabledAgentIDs.remove(id)
        if selectedAgentID == id { selectedAgentID = "" }
    }

    /// Expected percent consumed per work day, so a full week of work days totals 100%
    /// ("5 selected → 20% per day", as the prototype phrases it).
    var dailyBudget: Double { 100.0 / Double(workDays) }

    func isEnabled(_ agentID: String) -> Bool { !disabledAgentIDs.contains(agentID) }

    func setEnabled(_ enabled: Bool, agentID: String) {
        if enabled {
            disabledAgentIDs.remove(agentID)
        } else {
            disabledAgentIDs.insert(agentID)
        }
    }

    /// Push the chosen appearance onto the app (nil = follow the system setting).
    func applyAppearance() {
        switch appearance {
        case .system: NSApp.appearance = nil
        case .light: NSApp.appearance = NSAppearance(named: .aqua)
        case .dark: NSApp.appearance = NSAppearance(named: .darkAqua)
        }
    }
}

/// A configured extra Claude Code login, monitored as its own agent alongside the primary.
struct ClaudeAccount: Codable, Identifiable, Equatable {
    /// Stable agent id (e.g. "claude-personal"): the emitted `agent.id`, cache key, and glyph key.
    /// Always carries the "claude" prefix so it renders with the Claude-family glyph.
    let id: String
    /// Display name shown in the menu bar / dashboard (e.g. "Claude (personal)").
    var label: String
    /// The account's Claude Code config dir, e.g. "~/.claude-personal". Its `.credentials.json` is
    /// read first; on macOS the login Keychain is the fallback (see `resolvedKeychainService`).
    var configDir: String
    /// Optional explicit macOS Keychain service override. Normally left nil — the service is
    /// derived from `configDir` to match Claude Code's own scheme (see `resolvedKeychainService`).
    var keychainService: String?

    /// The macOS Keychain service holding this account's OAuth token. Claude Code stores the
    /// default config dir (`~/.claude`) under the bare service `Claude Code-credentials`, and every
    /// other config dir under that name plus a suffix that is the first 4 bytes of SHA-256 over the
    /// dir's absolute path — e.g. `~/.claude-personal` → `Claude Code-credentials-791fb149`. An
    /// explicit `keychainService` override wins when set.
    var resolvedKeychainService: String {
        if let override = keychainService, !override.isEmpty { return override }
        let path = Self.expandTilde(configDir)
        if path == NSHomeDirectory() + "/.claude" { return "Claude Code-credentials" }
        let digest = SHA256.hash(data: Data(path.utf8))
        let suffix = digest.prefix(4).map { String(format: "%02x", $0) }.joined()
        return "Claude Code-credentials-\(suffix)"
    }

    /// Expand a leading `~`/`~/` to the home dir and strip trailing slashes, so the path hashes
    /// identically to the absolute `CLAUDE_CONFIG_DIR` Claude Code sees.
    static func expandTilde(_ p: String) -> String {
        var s = p.trimmingCharacters(in: .whitespacesAndNewlines)
        if s == "~" { return NSHomeDirectory() }
        if s.hasPrefix("~/") { s = NSHomeDirectory() + String(s.dropFirst(1)) }
        while s.count > 1 && s.hasSuffix("/") { s.removeLast() }
        return s
    }

    /// Derive a stable, unique "claude-…" id from a label, avoiding collisions with `existing` and
    /// with the primary "claude" agent.
    static func makeID(label: String, existing: [ClaudeAccount]) -> String {
        var slug = ""
        var lastDash = false
        for ch in label.lowercased() {
            if ch.isLetter || ch.isNumber {
                slug.append(ch)
                lastDash = false
            } else if !lastDash {
                slug.append("-")
                lastDash = true
            }
        }
        slug = slug.trimmingCharacters(in: CharacterSet(charactersIn: "-"))
        var base = slug.hasPrefix("claude") ? slug : "claude-\(slug)"
        if base == "claude" || base.isEmpty { base = "claude-account" }

        let taken = Set(existing.map(\.id) + ["claude"])
        if !taken.contains(base) { return base }
        var n = 2
        while taken.contains("\(base)-\(n)") { n += 1 }
        return "\(base)-\(n)"
    }
}

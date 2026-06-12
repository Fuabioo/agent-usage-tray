import AppKit
import Combine
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

    private let defaults: UserDefaults

    private enum Keys {
        static let workDays = "workDays"
        static let appearance = "appearance"
        static let disabledAgents = "disabledAgentIDs"
        static let menuBarMode = "menuBarMode"
        static let selectedAgentID = "selectedAgentID"
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

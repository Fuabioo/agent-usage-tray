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

    @Published var workDays: Int {
        didSet {
            workDays = min(max(workDays, 1), 7)
            defaults.set(workDays, forKey: Keys.workDays)
        }
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

    private let defaults: UserDefaults

    private enum Keys {
        static let workDays = "workDays"
        static let appearance = "appearance"
        static let disabledAgents = "disabledAgentIDs"
    }

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        let storedWorkDays = defaults.object(forKey: Keys.workDays) as? Int ?? 5
        self.workDays = min(max(storedWorkDays, 1), 7)
        self.appearance = Appearance(rawValue: defaults.string(forKey: Keys.appearance) ?? "")
            ?? .system
        self.disabledAgentIDs = Set(defaults.stringArray(forKey: Keys.disabledAgents) ?? [])
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

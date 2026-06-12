import SwiftUI

/// The settings window content. A focused subset of the prototype's settings: appearance, the
/// work-days pace budget, and per-agent enable toggles. (The prototype's menu-bar display modes
/// and notification toggles are a later refinement.)
struct SettingsView: View {
    @ObservedObject var settings: AppSettings
    @ObservedObject var controller: DataController

    private let weekdaySymbols = ["S", "M", "T", "W", "T", "F", "S"]

    /// Agents to list — whatever the CLI has reported (enabled or not).
    private var knownAgents: [AgentDTO] {
        var seen = Set<String>()
        var result: [AgentDTO] = []
        for snap in controller.merged where !seen.contains(snap.agent.id) {
            seen.insert(snap.agent.id)
            result.append(snap.agent)
        }
        return result
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            Text("Agent Usage Settings").font(.title3).bold()

            row("Appearance") {
                Picker("", selection: $settings.appearance) {
                    ForEach(AppSettings.Appearance.allCases) { Text($0.label).tag($0) }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .frame(width: 220)
            }

            Divider()

            row("Work days") {
                VStack(alignment: .leading, spacing: 6) {
                    Stepper(value: $settings.workDays, in: 1...7) {
                        Text("\(settings.workDays) work day\(settings.workDays == 1 ? "" : "s")")
                    }
                    .frame(width: 220)
                    Text("Pace splits each budget across these days. \(settings.workDays) selected → \(Int(settings.dailyBudget.rounded()))% per day.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }

            Divider()

            row("Agents") {
                VStack(alignment: .leading, spacing: 8) {
                    if knownAgents.isEmpty {
                        Text("No agents reported yet.").font(.caption).foregroundStyle(.secondary)
                    }
                    ForEach(knownAgents, id: \.id) { agent in
                        Toggle(isOn: Binding(
                            get: { settings.isEnabled(agent.id) },
                            set: { settings.setEnabled($0, agentID: agent.id) }
                        )) {
                            HStack(spacing: 8) {
                                AgentGlyphView(agentID: agent.id, color: .primary, size: 15)
                                    .frame(width: 16)
                                VStack(alignment: .leading, spacing: 1) {
                                    Text(agent.label).font(.callout)
                                    Text("via \(agent.source)").font(.caption2)
                                        .foregroundStyle(.secondary)
                                }
                            }
                        }
                        .toggleStyle(.switch)
                        .frame(width: 300)
                    }
                }
            }
        }
        .padding(20)
        .frame(width: 460, alignment: .leading)
    }

    @ViewBuilder
    private func row<Content: View>(_ label: String, @ViewBuilder content: () -> Content) -> some View {
        HStack(alignment: .top, spacing: 16) {
            Text("\(label):")
                .font(.callout)
                .foregroundStyle(.secondary)
                .frame(width: 90, alignment: .trailing)
            content()
            Spacer(minLength: 0)
        }
    }
}

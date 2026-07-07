import SwiftUI

/// The settings window content. A focused subset of the prototype's settings: appearance, the
/// work-days pace budget, and per-agent enable toggles. (The prototype's menu-bar display modes
/// and notification toggles are a later refinement.)
struct SettingsView: View {
    @ObservedObject var settings: AppSettings
    @ObservedObject var controller: DataController

    private let weekdaySymbols = ["S", "M", "T", "W", "T", "F", "S"]

    // New-Claude-account form fields.
    @State private var newAccountLabel = ""
    @State private var newAccountConfigDir = ""
    @State private var newAccountService = ""

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
        ScrollView {
        VStack(alignment: .leading, spacing: 18) {
            Text("Agent Usage Settings").font(.title3).bold()

            row("Menu bar shows") {
                VStack(alignment: .leading, spacing: 7) {
                    ForEach(AppSettings.MenuBarMode.allCases) { mode in
                        modeRow(mode)
                    }
                    if settings.menuBarMode == .selectedAgent {
                        Picker("", selection: $settings.selectedAgentID) {
                            if knownAgents.isEmpty {
                                Text("No agents yet").tag("")
                            }
                            ForEach(knownAgents, id: \.id) { Text($0.label).tag($0.id) }
                        }
                        .labelsHidden()
                        .frame(width: 200)
                        .padding(.leading, 24)
                    }
                }
            }

            Divider()

            row("Credits show") {
                VStack(alignment: .leading, spacing: 6) {
                    Picker("", selection: $settings.creditDisplay) {
                        ForEach(AppSettings.CreditDisplay.allCases) { Text($0.label).tag($0) }
                    }
                    .pickerStyle(.segmented)
                    .labelsHidden()
                    .frame(width: 260)
                    Text("How credit pools like Hyper read out their balance.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }

            Divider()

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
                VStack(alignment: .leading, spacing: 10) {
                    if knownAgents.isEmpty {
                        Text("No agents reported yet.").font(.caption).foregroundStyle(.secondary)
                    }
                    ForEach(knownAgents, id: \.id) { agent in
                        HStack(spacing: 10) {
                            AgentGlyphView(agentID: agent.id, nsColor: .labelColor, size: 16)
                                .frame(width: 18, height: 18)
                            VStack(alignment: .leading, spacing: 1) {
                                Text(agent.label).font(.callout)
                                Text("via \(agent.source)").font(.caption2)
                                    .foregroundStyle(.secondary)
                            }
                            Spacer(minLength: 12)
                            Toggle("", isOn: Binding(
                                get: { settings.isEnabled(agent.id) },
                                set: { settings.setEnabled($0, agentID: agent.id) }
                            ))
                            .labelsHidden()
                            .toggleStyle(.switch)
                        }
                        .frame(width: 300)
                    }
                }
            }
            Divider()

            row("Claude accounts") {
                claudeAccountsSection
            }
        }
        .padding(20)
        .frame(width: 460, alignment: .leading)
        }
        .frame(width: 460, height: 520)
    }

    /// Manage extra Claude Code logins: a row per configured account plus a small add form.
    private var claudeAccountsSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Monitor a second Claude Code login (e.g. a personal account) as its own agent.")
                .font(.caption).foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)

            ForEach(settings.claudeAccounts) { account in
                HStack(spacing: 10) {
                    AgentGlyphView(agentID: account.id, nsColor: .labelColor, size: 16)
                        .frame(width: 18, height: 18)
                    VStack(alignment: .leading, spacing: 1) {
                        Text(account.label).font(.callout)
                        Text(account.configDir + (account.keychainService.map { " · \($0)" } ?? ""))
                            .font(.caption2).foregroundStyle(.secondary)
                    }
                    Spacer(minLength: 12)
                    Button {
                        settings.removeClaudeAccount(id: account.id)
                    } label: {
                        Image(systemName: "trash")
                    }
                    .buttonStyle(.borderless)
                    .help("Remove this account")
                }
                .frame(width: 300)
            }

            VStack(alignment: .leading, spacing: 6) {
                TextField("Label (e.g. Personal)", text: $newAccountLabel)
                TextField("Config dir (e.g. ~/.claude-personal)", text: $newAccountConfigDir)
                TextField("Keychain service (optional — auto-derived)", text: $newAccountService)
                HStack {
                    Spacer()
                    Button("Add account", action: addAccount)
                        .disabled(!canAddAccount)
                }
            }
            .textFieldStyle(.roundedBorder)
            .frame(width: 300)
        }
    }

    private var canAddAccount: Bool {
        !newAccountLabel.trimmingCharacters(in: .whitespaces).isEmpty
            && !newAccountConfigDir.trimmingCharacters(in: .whitespaces).isEmpty
    }

    private func addAccount() {
        settings.addClaudeAccount(
            label: newAccountLabel, configDir: newAccountConfigDir, keychainService: newAccountService)
        newAccountLabel = ""
        newAccountConfigDir = ""
        newAccountService = ""
        controller.refresh(force: true)
    }

    /// A radio-style row for one menu bar display mode.
    @ViewBuilder
    private func modeRow(_ mode: AppSettings.MenuBarMode) -> some View {
        let selected = settings.menuBarMode == mode
        Button {
            settings.menuBarMode = mode
            if mode == .selectedAgent, settings.selectedAgentID.isEmpty, let first = knownAgents.first {
                settings.selectedAgentID = first.id
            }
        } label: {
            HStack(spacing: 8) {
                Image(systemName: selected ? "largecircle.fill.circle" : "circle")
                    .foregroundStyle(selected ? Color.accentColor : Color.secondary)
                Text(mode.label)
                Text(mode.detail).font(.caption).foregroundStyle(.secondary)
                Spacer(minLength: 0)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
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

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

    // Hyper API key entry. Held only long enough to hand to the CLI — never persisted here.
    @State private var hyperKeyEntry = ""
    @State private var hyperKeyError: String?

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

            row("Credits reset") { creditsResetField }

            Divider()

            row("Hyper key") { hyperKeyField }

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

    /// When Hyper's daily credits refresh. Its API reports only a balance — no reset instant — so
    /// this is the one thing the app has to be told. It lives here rather than in
    /// `HYPER_RESET_TIME` because an app launched from Finder inherits no shell profile, so a
    /// shell export never reaches it.
    private var creditsResetField: some View {
        VStack(alignment: .leading, spacing: 6) {
            TextField("08:00 local", text: $settings.hyperResetTime)
                .textFieldStyle(.roundedBorder)
                .frame(width: 200)
            Text("Time of day your credits refresh — `HH:MM`, optionally with a zone: "
                 + "`local`, `Z`, or `±HH:MM`. A bare time means UTC.")
                .font(.caption)
                .foregroundStyle(.secondary)
            creditsResetFeedback
        }
    }

    /// Echo back what the CLI actually made of the value — the resolved next reset, or its parse
    /// error. Rather than re-implementing the parser in Swift (two grammars that would drift),
    /// this reads the result out of the snapshot the CLI just returned.
    ///
    /// Reads `agents`, not `merged`: the stale fallback substitutes the last *good* reading for an
    /// agent that errored, which is right for the dashboard but would silently swallow the parse
    /// error for the very field being edited.
    @ViewBuilder
    private var creditsResetFeedback: some View {
        if let hyper = controller.agents.first(where: { $0.agent.id == "hyper" }) {
            if let error = hyper.error {
                Text(error.message)
                    .font(.caption)
                    .foregroundStyle(PaceColor.red.swiftUIColor)
                    .fixedSize(horizontal: false, vertical: true)
            } else if let reset = hyper.window("credits")?.resetsAt {
                Text("Next refresh \(localResetString(reset))")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        } else if settings.hyperResetTime.isEmpty {
            Text("Unset — falls back to HYPER_RESET_TIME, then midnight UTC.")
                .font(.caption)
                .foregroundStyle(.tertiary)
        }
    }

    /// The Charm Hyper API key. Stored by the CLI in its `0600` config file, not here — an app
    /// started by launchd at login inherits no shell profile, so a `HYPER_API_KEY` export is
    /// invisible to it and Hyper would drop off the menu bar after every reboot.
    ///
    /// The field is never populated with the stored key; we only report whether one exists. The
    /// key reaches the CLI on stdin, so it never appears in any process's arguments.
    private var hyperKeyField: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                SecureField(controller.hyperKeyIsSet ? "•••••••• (stored)" : "hyper_…",
                            text: $hyperKeyEntry)
                    .textFieldStyle(.roundedBorder)
                    .frame(width: 200)
                Button("Save") { saveHyperKey(hyperKeyEntry) }
                    .disabled(hyperKeyEntry.trimmingCharacters(in: .whitespaces).isEmpty)
                if controller.hyperKeyIsSet {
                    Button("Remove") { saveHyperKey("") }
                }
            }
            if let error = hyperKeyError {
                Text(error)
                    .font(.caption)
                    .foregroundStyle(PaceColor.red.swiftUIColor)
                    .fixedSize(horizontal: false, vertical: true)
            } else {
                Text(controller.hyperKeyIsSet
                     ? "Stored in the CLI config (owner-only). Survives a reboot — unlike a shell export."
                     : "Optional. Without a key here or a HYPER_API_KEY export, Hyper stays hidden.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private func saveHyperKey(_ key: String) {
        controller.saveHyperAPIKey(key) { error in
            hyperKeyError = error
            if error == nil { hyperKeyEntry = "" }
        }
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

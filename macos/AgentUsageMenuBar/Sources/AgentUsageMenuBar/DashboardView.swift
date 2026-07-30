import SwiftUI

/// The popover dashboard — reproduces the prototype: "Today's pace" header, a ring gauge per
/// agent, alert banners for any credit pool projected to run dry or multi-day budget on course to
/// be spent early, per-agent detail rows, and a footer with the last update, Refresh, and a
/// settings gear. Anything that is true of one agent rather than of the run — a cached reading, a
/// refresh in flight — is shown on that agent, since agents fail and recover independently.
struct DashboardView: View {
    @ObservedObject var controller: DataController
    @ObservedObject var settings: AppSettings
    var onOpenSettings: () -> Void

    /// The enabled agents (with the controller's per-agent stale fallback already applied).
    private var displayAgents: [AgentSnapshot] {
        controller.merged.filter { settings.isEnabled($0.agent.id) }
    }


    /// Windows (with their owning agent) that are credit pools projected to run dry before reset.
    private var depletionAlerts: [(agent: AgentSnapshot, window: WindowDTO)] {
        displayAgents.flatMap { agent in
            (agent.windows ?? [])
                .filter { $0.pool?.depletesBeforeReset == true }
                .map { (agent, $0) }
        }
    }

    /// Agents whose multi-day budget is on course to be spent before it resets — the warning that
    /// stands in for a short window an agent no longer enforces.
    private var burnAlerts: [AgentSnapshot] {
        displayAgents.filter { !$0.isError && $0.burnsOutEarly }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            header

            if displayAgents.isEmpty {
                Text(controller.runtimeError ?? "No data yet…")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.vertical, 8)
            } else {
                HStack(alignment: .top, spacing: 14) {
                    ForEach(displayAgents) { snapshot in
                        AgentRingView(
                            snapshot: snapshot,
                            isRefreshing: controller.refreshingAgentIDs.contains(snapshot.id),
                            onRefresh: { controller.refresh(agentID: snapshot.id) })
                    }
                    if displayAgents.count < 2 { Spacer(minLength: 0) }
                }

                ForEach(Array(depletionAlerts.enumerated()), id: \.offset) { _, item in
                    DepletionBanner(agent: item.agent, window: item.window)
                }

                ForEach(burnAlerts) { BurnRateBanner(agent: $0) }

                Divider()
                detailRows
            }

            Divider()
            footer
        }
        .padding(14)
        .frame(width: 380)
    }

    private var header: some View {
        // No global "work day N of M": each agent's pace is relative to its own weekly reset
        // (agents renew on different days), so the work day is shown per agent under its ring.
        HStack {
            Text("Today's pace").font(.headline)
            Spacer()
        }
    }

    private var detailRows: some View {
        VStack(alignment: .leading, spacing: 10) {
            ForEach(displayAgents) { AgentDetail(snapshot: $0, creditDisplay: settings.creditDisplay) }
        }
    }

    // No global "Updated": with per-agent refresh it only ever reported whichever agent was asked
    // last, and each agent now carries its own reading time.
    private var footer: some View {
        HStack(spacing: 8) {
            Spacer()
            Button("Refresh") { controller.refresh(force: true) }
                .buttonStyle(.borderless)
                .font(.caption)
                .pointingHandCursor()
                .help("Refresh every agent")
            Button {
                onOpenSettings()
            } label: {
                Image(systemName: "gearshape")
            }
            .buttonStyle(.borderless)
            .pointingHandCursor()
            .help("Settings")
        }
    }
}

// MARK: - Ring gauge

/// One agent's ring — and its refresh control. The ring *is* the button: hovering swaps the
/// agent's glyph for a refresh arrow, so the affordance costs no extra chrome in a popover that
/// has none to spare, and it lands on the thing being refreshed rather than beside it.
private struct AgentRingView: View {
    let snapshot: AgentSnapshot
    let isRefreshing: Bool
    let onRefresh: () -> Void

    @State private var hovering = false

    /// What the ring reports: today's pace headroom (`ceiling − used`), gauged against one
    /// day's budget. A full ring means "a whole day's allowance still available"; empty/over
    /// means you've spent through today's pace.
    private struct Model {
        var fraction: Double
        var caption: String
        var nsColor: NSColor
        var isError: Bool
        var isSurplus: Bool = false
    }

    private var model: Model {
        if snapshot.isError {
            return Model(fraction: 1, caption: "error", nsColor: .secondaryLabelColor, isError: true)
        }
        // Today's pace: how much of today's daily budget is still available.
        if let pace = snapshot.pace {
            let daily = max(snapshot.config.dailyBudget, 0.0001)
            let frac = min(max(pace.remaining / daily, 0), 1)
            let weeklyPace = snapshot.window("weekly")?.pace ?? .green
            let caption = pace.remaining >= 0
                ? "\(Int(pace.remaining.rounded()))% left"
                : "\(Int((-pace.remaining).rounded()))% over"
            return Model(fraction: frac, caption: caption, nsColor: weeklyPace.nsColor,
                         isError: false, isSurplus: weeklyPace == .surplus)
        }
        // Pool / fallback agents without a weekly pace window.
        if let p = snapshot.primaryWindow {
            if let pool = p.pool, pool.depletesBeforeReset, let dep = pool.projectedDepletion {
                return Model(fraction: min(max(p.remainingPct / 100, 0), 1),
                             caption: "out ~\(shortWeekday(dep))", nsColor: p.pace.nsColor, isError: false)
            }
            return Model(fraction: min(max(p.remainingPct / 100, 0), 1),
                         caption: "\(Int(p.remainingPct.rounded()))% left", nsColor: p.pace.nsColor, isError: false)
        }
        return Model(fraction: 0, caption: "—", nsColor: .secondaryLabelColor, isError: false)
    }

    @Environment(\.colorScheme) private var scheme

    /// A metallic mint gradient for the surplus ring — built from mints that stay readable on the
    /// current background (deeper teal in Light mode, brighter in Dark) so it reads premium
    /// without washing out. Only the ring stroke uses it; the text and glyph use a solid mint.
    private var mintRing: AngularGradient {
        let stops: [Color] = scheme == .dark
            ? [Color(red: 0.42, green: 0.98, blue: 0.84), Color(red: 0.22, green: 0.82, blue: 0.68),
               Color(red: 0.38, green: 0.94, blue: 0.80), Color(red: 0.16, green: 0.74, blue: 0.62),
               Color(red: 0.42, green: 0.98, blue: 0.84)]
            : [Color(red: 0.00, green: 0.62, blue: 0.52), Color(red: 0.00, green: 0.44, blue: 0.38),
               Color(red: 0.00, green: 0.56, blue: 0.48), Color(red: 0.00, green: 0.40, blue: 0.34),
               Color(red: 0.00, green: 0.62, blue: 0.52)]
        return AngularGradient(gradient: Gradient(colors: stops), center: .center)
    }

    var body: some View {
        let m = model
        let color = Color(nsColor: m.nsColor)
        let ringStyle: AnyShapeStyle = m.isError
            ? AnyShapeStyle(Color.secondary.opacity(0.4))
            : (m.isSurplus ? AnyShapeStyle(mintRing) : AnyShapeStyle(color))

        Button(action: onRefresh) {
            VStack(spacing: 6) {
                ZStack {
                    Circle()
                        .stroke(Color.secondary.opacity(0.18), lineWidth: 5)
                    Circle()
                        .trim(from: 0, to: m.isError ? 1 : m.fraction)
                        .stroke(ringStyle, style: StrokeStyle(lineWidth: 5, lineCap: .round))
                        .rotationEffect(.degrees(-90))
                        // Golden glow when you're a full day or more ahead of pace (surplus).
                        .shadow(color: m.isSurplus ? color.opacity(0.8) : .clear, radius: 5)
                        .shadow(color: m.isSurplus ? color.opacity(0.5) : .clear, radius: 9)
                    ringCenter(m, color: color)
                }
                .frame(width: 52, height: 52)

                Text(m.caption)
                    .font(.caption).bold()
                    .foregroundStyle(m.isError ? Color.secondary : color)
                    .lineLimit(1)
                Text(snapshot.agent.label)
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                if let pace = snapshot.pace, !snapshot.isError {
                    Text("day \(pace.workDayIndex)/\(snapshot.config.workDays)")
                        .font(.caption2)
                        .foregroundStyle(.tertiary)
                        .lineLimit(1)
                }
            }
            .frame(maxWidth: .infinity)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .pointingHandCursor()
        .help("Refresh \(snapshot.agent.label)")
    }

    /// The glyph at rest, a refresh arrow under the pointer, a spinner while the agent is being
    /// re-asked. Tinted with the agent's own pace color throughout, so swapping it doesn't read as
    /// the reading itself having changed.
    @ViewBuilder
    private func ringCenter(_ m: Model, color: Color) -> some View {
        if isRefreshing {
            ProgressView()
                .controlSize(.small)
        } else if hovering {
            Image(systemName: "arrow.clockwise")
                .font(.system(size: 21, weight: .medium))
                .foregroundStyle(m.isError ? Color.secondary : color)
        } else {
            AgentGlyphView(agentID: snapshot.agent.id,
                           nsColor: m.isError ? .secondaryLabelColor : m.nsColor,
                           size: 24)
        }
    }
}

// MARK: - Per-agent detail

/// Each agent's windows shown as remaining ("left") percentages — the headline rings answer
/// "how much can I still use today"; these rows answer "how much of each window is left".
private struct AgentDetail: View {
    let snapshot: AgentSnapshot
    let creditDisplay: AppSettings.CreditDisplay

    /// Session first, then weekly, then anything else — a stable reading order across agents.
    private var orderedWindows: [WindowDTO] {
        let ws = snapshot.windows ?? []
        return ws.filter { $0.kind == "session" }
            + ws.filter { $0.kind == "weekly" }
            + ws.filter { $0.kind != "session" && $0.kind != "weekly" }
    }

    /// How old this agent's reading is, and whether it's a fallback. Both come from the agent's
    /// own snapshot: `fetchedAt` is when the CLI actually obtained the usage (not when it rendered
    /// the document), so a cached reading shows its true age instead of claiming to be current.
    @ViewBuilder
    private var freshness: some View {
        if snapshot.isStale {
            Text("cached · \(formatAge(snapshot.fetchedAt))")
                .font(.caption2)
                .foregroundStyle(PaceColor.yellow.swiftUIColor)
                .lineLimit(1)
                .help(snapshot.staleReason
                      ?? "Showing the last good reading — the latest refresh failed.")
        } else {
            Text(formatAge(snapshot.fetchedAt))
                .font(.caption2)
                .foregroundStyle(.tertiary)
                .lineLimit(1)
                .help("Fetched \(localResetString(snapshot.fetchedAt))")
        }
    }

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            VStack(alignment: .leading, spacing: 1) {
                Text(snapshot.agent.label)
                // Per agent, not per popover: agents are fetched, fail and recover one at a time,
                // so a footer badge said "something here is old" without saying which — and the
                // refresh that fixes it is now per agent too.
                if !snapshot.isError { freshness }
            }
            .frame(width: 106, alignment: .leading)
            if snapshot.isError || orderedWindows.isEmpty {
                Text("error").foregroundStyle(.secondary)
                Spacer(minLength: 0)
            } else {
                VStack(alignment: .leading, spacing: 8) {
                    ForEach(orderedWindows, id: \.label) { WindowLine(window: $0, creditDisplay: creditDisplay) }
                    if let burst = snapshot.trend?.recent {
                        BurstLine(burst: burst, burnPerDay: snapshot.trend?.burnPerDay)
                    }
                }
            }
        }
        .font(.callout)
    }
}

/// One window: its remaining ("left") percentage, plus the exact local reset moment and a
/// countdown beneath it.
private struct WindowLine: View {
    let window: WindowDTO
    let creditDisplay: AppSettings.CreditDisplay

    /// Credit pools read out the raw balance the API returns ("1,620 / 1,656"), a remaining
    /// percentage, or both, per the preference; utilization windows read out remaining percentage.
    private var isCredits: Bool { window.kind == "credits" }
    private var label: String { isCredits ? window.label : "\(window.label) left" }
    private var value: String {
        let pct = "\(Int(window.remainingPct.rounded()))%"
        guard isCredits, let pool = window.pool else { return pct }
        let credits = "\(formatCredits(pool.remaining)) / \(formatCredits(pool.total))"
        switch creditDisplay {
        case .credits: return credits
        case .percentage: return pct
        case .both: return "\(credits) · \(pct)"
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 1) {
            HStack(spacing: 8) {
                Text(label).foregroundStyle(.secondary)
                Spacer(minLength: 12)
                Text(value)
                    .bold()
                    .foregroundStyle(window.pace.swiftUIColor)
            }
            if let reset = window.resetsAt {
                HStack(spacing: 8) {
                    Text("resets \(localResetString(reset))")
                    Spacer(minLength: 12)
                    if let secs = window.resetsInSecs, secs > 0 {
                        Text("in \(formatDuration(seconds: secs))")
                    }
                }
                .font(.caption2)
                .foregroundStyle(.tertiary)
            }
        }
    }
}

/// The short-horizon burst: how much of the multi-day budget went in the last few hours, with the
/// observed daily rate beneath it. Reads as consumption ("12% in the last 5h") rather than as a
/// remainder, because its point is the speed — the window it stands in for is the one that used
/// to stop you mid-sitting.
private struct BurstLine: View {
    let burst: BurstDTO
    let burnPerDay: Double?

    var body: some View {
        VStack(alignment: .leading, spacing: 1) {
            HStack(spacing: 8) {
                Text("last \(formatDuration(seconds: burst.spanSecs))")
                    .foregroundStyle(.secondary)
                Spacer(minLength: 12)
                Text("\(Int(burst.usedPct.rounded()))% used")
                    .bold()
                    .foregroundStyle(burst.pace.swiftUIColor)
            }
            if let burn = burnPerDay {
                Text("burning ≈\(Int(burn.rounded()))% of the cycle per day")
                    .font(.caption2)
                    .foregroundStyle(.tertiary)
            }
        }
        .help("How much of this agent's multi-day budget you've spent in the last "
              + "\(formatDuration(seconds: burst.spanSecs)). Colored against one work day's "
              + "allowance, so it warns on how fast you're going rather than on what's left.")
    }
}

// MARK: - Burn-rate alert banner

/// The multi-day equivalent of `DepletionBanner`: the budget isn't gone, but at the rate of the
/// last day it will be — before it resets.
private struct BurnRateBanner: View {
    let agent: AgentSnapshot

    private var title: String {
        guard let out = agent.trend?.projectedExhaustion else {
            return "\(agent.agent.label) — burning through the cycle early"
        }
        return "\(agent.agent.label) — weekly budget gone ~\(shortWeekday(out)) at this rate"
    }

    private var detail: String {
        var parts: [String] = []
        if let burn = agent.trend?.burnPerDay { parts.append("burning ≈\(Int(burn.rounded()))%/day") }
        if let burst = agent.trend?.recent {
            parts.append("\(Int(burst.usedPct.rounded()))% in the last \(formatDuration(seconds: burst.spanSecs))")
        }
        if let weekly = agent.window("weekly"), let secs = weekly.resetsInSecs, secs > 0 {
            parts.append("resets in \(formatDuration(seconds: secs))")
        }
        return parts.joined(separator: " · ")
    }

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            Circle().fill(PaceColor.red.swiftUIColor).frame(width: 8, height: 8).padding(.top, 4)
            VStack(alignment: .leading, spacing: 2) {
                Text(title).font(.callout).bold().foregroundStyle(PaceColor.red.swiftUIColor)
                Text(detail).font(.caption).foregroundStyle(.secondary)
            }
            Spacer(minLength: 0)
        }
        .padding(10)
        .background(PaceColor.red.swiftUIColor.opacity(0.10), in: RoundedRectangle(cornerRadius: 8))
    }
}

// MARK: - Depletion alert banner

private struct DepletionBanner: View {
    let agent: AgentSnapshot
    let window: WindowDTO

    private var title: String {
        if let dep = window.pool?.projectedDepletion {
            return "\(agent.agent.label) — \(window.label) out ~\(shortWeekday(dep)) at this rate"
        }
        return "\(agent.agent.label) — \(window.label) running out"
    }

    private var detail: String {
        guard let pool = window.pool else { return "" }
        var parts = ["\(Int(pool.remaining)) of \(Int(pool.total)) left"]
        if let burn = pool.burnPerDay { parts.append("burning ≈\(Int(burn))/day") }
        parts.append(window.resetsAt == nil ? "no auto-refill" : "refills at reset")
        return parts.joined(separator: " · ")
    }

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            Circle().fill(PaceColor.red.swiftUIColor).frame(width: 8, height: 8).padding(.top, 4)
            VStack(alignment: .leading, spacing: 2) {
                Text(title).font(.callout).bold().foregroundStyle(PaceColor.red.swiftUIColor)
                Text(detail).font(.caption).foregroundStyle(.secondary)
            }
            Spacer(minLength: 0)
        }
        .padding(10)
        .background(PaceColor.red.swiftUIColor.opacity(0.10), in: RoundedRectangle(cornerRadius: 8))
    }
}

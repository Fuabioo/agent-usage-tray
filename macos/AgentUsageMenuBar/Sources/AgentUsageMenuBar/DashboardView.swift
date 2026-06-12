import SwiftUI

/// The popover dashboard — reproduces the prototype: "Today's pace" header, a ring gauge per
/// agent, a burn-rate alert banner for any credit pool projected to run dry, per-agent detail
/// rows, and a footer with the last update, Refresh, and a settings gear.
struct DashboardView: View {
    @ObservedObject var controller: DataController
    @ObservedObject var settings: AppSettings
    var onOpenSettings: () -> Void

    /// The enabled agents (with the controller's per-agent stale fallback already applied).
    private var displayAgents: [AgentSnapshot] {
        controller.merged.filter { settings.isEnabled($0.agent.id) }
    }

    /// Pace context from the first agent that has it (all agents share the same clock/config).
    private var paceContext: (index: Int, workDays: Int)? {
        for a in displayAgents {
            if let p = a.pace { return (p.workDayIndex, a.config.workDays) }
        }
        return nil
    }

    /// Windows (with their owning agent) that are credit pools projected to run dry before reset.
    private var depletionAlerts: [(agent: AgentSnapshot, window: WindowDTO)] {
        displayAgents.flatMap { agent in
            (agent.windows ?? [])
                .filter { $0.pool?.depletesBeforeReset == true }
                .map { (agent, $0) }
        }
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
                    ForEach(displayAgents) { AgentRingView(snapshot: $0) }
                    if displayAgents.count < 2 { Spacer(minLength: 0) }
                }

                ForEach(Array(depletionAlerts.enumerated()), id: \.offset) { _, item in
                    DepletionBanner(agent: item.agent, window: item.window)
                }

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
        HStack {
            Text("Today's pace").font(.headline)
            Spacer()
            if let ctx = paceContext {
                Text("Work day \(ctx.index) of \(ctx.workDays)")
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
            }
        }
    }

    private var detailRows: some View {
        VStack(alignment: .leading, spacing: 10) {
            ForEach(displayAgents) { AgentDetail(snapshot: $0) }
        }
    }

    private var footer: some View {
        HStack(spacing: 8) {
            if let updated = controller.lastUpdated {
                Text("Updated \(updated.formatted(date: .omitted, time: .shortened))")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
            Spacer()
            Button("Refresh") { controller.refresh() }
                .buttonStyle(.borderless)
                .font(.caption)
            Button {
                onOpenSettings()
            } label: {
                Image(systemName: "gearshape")
            }
            .buttonStyle(.borderless)
            .help("Settings")
        }
    }
}

// MARK: - Ring gauge

private struct AgentRingView: View {
    let snapshot: AgentSnapshot

    /// What the ring reports: today's pace headroom (`ceiling − used`), gauged against one
    /// day's budget. A full ring means "a whole day's allowance still available"; empty/over
    /// means you've spent through today's pace.
    private struct Model {
        var fraction: Double
        var caption: String
        var nsColor: NSColor
        var isError: Bool
    }

    private var model: Model {
        if snapshot.isError {
            return Model(fraction: 1, caption: "error", nsColor: .secondaryLabelColor, isError: true)
        }
        // Today's pace: how much of today's daily budget is still available.
        if let pace = snapshot.pace {
            let daily = max(snapshot.config.dailyBudget, 0.0001)
            let frac = min(max(pace.remaining / daily, 0), 1)
            let nsColor = snapshot.window("weekly")?.pace.nsColor ?? PaceColor.green.nsColor
            let caption = pace.remaining >= 0
                ? "\(Int(pace.remaining.rounded()))% left"
                : "\(Int((-pace.remaining).rounded()))% over"
            return Model(fraction: frac, caption: caption, nsColor: nsColor, isError: false)
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

    var body: some View {
        let m = model
        let color = Color(nsColor: m.nsColor)
        VStack(spacing: 6) {
            ZStack {
                Circle()
                    .stroke(Color.secondary.opacity(0.18), lineWidth: 5)
                Circle()
                    .trim(from: 0, to: m.isError ? 1 : m.fraction)
                    .stroke(m.isError ? Color.secondary.opacity(0.4) : color,
                            style: StrokeStyle(lineWidth: 5, lineCap: .round))
                    .rotationEffect(.degrees(-90))
                AgentGlyphView(agentID: snapshot.agent.id,
                               nsColor: m.isError ? .secondaryLabelColor : m.nsColor,
                               size: 24)
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
        }
        .frame(maxWidth: .infinity)
    }
}

// MARK: - Per-agent detail

/// Each agent's windows shown as remaining ("left") percentages — the headline rings answer
/// "how much can I still use today"; these rows answer "how much of each window is left".
private struct AgentDetail: View {
    let snapshot: AgentSnapshot

    /// Session first, then weekly, then anything else — a stable reading order across agents.
    private var orderedWindows: [WindowDTO] {
        let ws = snapshot.windows ?? []
        return ws.filter { $0.kind == "session" }
            + ws.filter { $0.kind == "weekly" }
            + ws.filter { $0.kind != "session" && $0.kind != "weekly" }
    }

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Text(snapshot.agent.label)
                .frame(width: 96, alignment: .leading)
            if snapshot.isError || orderedWindows.isEmpty {
                Text("error").foregroundStyle(.secondary)
                Spacer(minLength: 0)
            } else {
                VStack(alignment: .leading, spacing: 3) {
                    ForEach(orderedWindows, id: \.label) { w in
                        HStack(spacing: 8) {
                            Text("\(w.label) left").foregroundStyle(.secondary)
                            Spacer(minLength: 12)
                            Text("\(Int(w.remainingPct.rounded()))%")
                                .bold()
                                .foregroundStyle(w.pace.swiftUIColor)
                        }
                    }
                }
            }
        }
        .font(.callout)
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

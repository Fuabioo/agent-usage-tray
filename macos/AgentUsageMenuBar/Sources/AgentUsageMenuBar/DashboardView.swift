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
        let columns = [GridItem(.flexible(), alignment: .leading),
                       GridItem(.flexible(), alignment: .leading)]
        return LazyVGrid(columns: columns, alignment: .leading, spacing: 8) {
            ForEach(displayAgents) { DetailRow(snapshot: $0) }
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

    private var primary: WindowDTO? { snapshot.primaryWindow }

    /// Fraction of the ring to fill = remaining budget (so a full ring means "lots left").
    private var fraction: Double {
        guard let p = primary else { return 0 }
        return min(max(p.remainingPct / 100.0, 0), 1)
    }

    private var color: Color { primary?.pace.swiftUIColor ?? Color.secondary }
    private var nsColor: NSColor { primary?.pace.nsColor ?? .secondaryLabelColor }

    /// Headline under the ring: "out ~Thu" for a depleting pool, else "N% left", else "error".
    private var caption: String {
        if snapshot.isError { return "error" }
        if let pool = primary?.pool, pool.depletesBeforeReset, let dep = pool.projectedDepletion {
            return "out ~\(shortWeekday(dep))"
        }
        if let p = primary { return "\(Int(p.remainingPct.rounded()))% left" }
        return "—"
    }

    var body: some View {
        VStack(spacing: 6) {
            ZStack {
                Circle()
                    .stroke(Color.secondary.opacity(0.18), lineWidth: 5)
                Circle()
                    .trim(from: 0, to: snapshot.isError ? 1 : fraction)
                    .stroke(snapshot.isError ? Color.secondary.opacity(0.4) : color,
                            style: StrokeStyle(lineWidth: 5, lineCap: .round))
                    .rotationEffect(.degrees(-90))
                AgentGlyphView(agentID: snapshot.agent.id,
                               nsColor: snapshot.isError ? .secondaryLabelColor : nsColor,
                               size: 18)
            }
            .frame(width: 52, height: 52)

            Text(caption)
                .font(.caption).bold()
                .foregroundStyle(snapshot.isError ? Color.secondary : color)
                .lineLimit(1)
            Text(snapshot.agent.label)
                .font(.caption2)
                .foregroundStyle(.secondary)
                .lineLimit(1)
        }
        .frame(maxWidth: .infinity)
    }
}

// MARK: - Detail row

private struct DetailRow: View {
    let snapshot: AgentSnapshot

    /// Show the secondary window (the one the ring doesn't), else the primary.
    private var window: WindowDTO? { snapshot.secondaryWindow ?? snapshot.primaryWindow }

    var body: some View {
        HStack(spacing: 4) {
            Text(snapshot.agent.label).foregroundStyle(.primary)
            if let w = window, !snapshot.isError {
                Text("· \(w.label)").foregroundStyle(.secondary)
                Spacer(minLength: 4)
                Text("\(Int(w.usedPct.rounded()))%")
                    .bold()
                    .foregroundStyle(w.pace.swiftUIColor)
            } else {
                Text("· error").foregroundStyle(.secondary)
                Spacer(minLength: 4)
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

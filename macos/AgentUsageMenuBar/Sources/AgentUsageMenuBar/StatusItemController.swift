import AppKit
import Combine
import ServiceManagement
import SwiftUI

/// Owns the menu bar item. Draws an at-a-glance, per-agent indicator (each agent's glyph plus
/// `weekly · session %`, tinted by pace) into the bar button, toggles the SwiftUI dashboard
/// popover on left-click, and shows a context menu on right-click.
final class StatusItemController {
    private let statusItem: NSStatusItem
    private let popover = NSPopover()
    private let hostingController: NSHostingController<DashboardView>
    private let controller: DataController
    private let settings: AppSettings
    private var settingsWindow: NSWindow?
    private var cancellables = Set<AnyCancellable>()
    private var appearanceObservation: NSKeyValueObservation?

    init(controller: DataController, settings: AppSettings) {
        self.controller = controller
        self.settings = settings
        self.statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)

        let dashboard = DashboardView(controller: controller, settings: settings, onOpenSettings: {})
        self.hostingController = NSHostingController(rootView: dashboard)
        hostingController.sizingOptions = [.preferredContentSize]
        // Rebuild the root view with a working settings callback now that `self` exists.
        hostingController.rootView = DashboardView(
            controller: controller, settings: settings,
            onOpenSettings: { [weak self] in self?.openSettings() })

        popover.behavior = .transient
        popover.contentViewController = hostingController
        popover.contentSize = NSSize(width: 380, height: 220)

        if let button = statusItem.button {
            button.image = Self.placeholderImage()
            button.imagePosition = .imageOnly
            button.target = self
            button.action = #selector(handleClick(_:))
            button.sendAction(on: [.leftMouseUp, .rightMouseUp])

            appearanceObservation = button.observe(\.effectiveAppearance, options: [.new]) { [weak self] _, _ in
                DispatchQueue.main.async { self?.updateBar() }
            }
        }

        controller.$agents.receive(on: RunLoop.main)
            .sink { [weak self] _ in self?.updateBar() }.store(in: &cancellables)
        controller.$runtimeError.receive(on: RunLoop.main)
            .sink { [weak self] _ in self?.updateBar() }.store(in: &cancellables)
        settings.$disabledAgentIDs.receive(on: RunLoop.main)
            .sink { [weak self] _ in self?.updateBar() }.store(in: &cancellables)
        settings.$menuBarMode.receive(on: RunLoop.main)
            .sink { [weak self] _ in self?.updateBar() }.store(in: &cancellables)
        settings.$selectedAgentID.receive(on: RunLoop.main)
            .sink { [weak self] _ in self?.updateBar() }.store(in: &cancellables)
    }

    // MARK: - Clicks

    @objc private func handleClick(_ sender: NSStatusBarButton) {
        if NSApp.currentEvent?.type == .rightMouseUp {
            showMenu()
        } else {
            togglePopover(sender)
        }
    }

    private func togglePopover(_ sender: NSStatusBarButton) {
        if popover.isShown {
            popover.performClose(sender)
        } else {
            hostingController.view.layoutSubtreeIfNeeded()
            var fitting = hostingController.view.fittingSize
            if fitting.width <= 0 { fitting.width = 380 }
            if fitting.height <= 0 { fitting.height = 220 }
            popover.contentSize = fitting
            popover.show(relativeTo: sender.bounds, of: sender, preferredEdge: .minY)
            popover.contentViewController?.view.window?.makeKey()
        }
    }

    private func showMenu() {
        let menu = NSMenu()
        menu.addItem(withTitle: "Refresh", action: #selector(refresh), keyEquivalent: "r").target = self
        menu.addItem(withTitle: "Settings…", action: #selector(openSettingsMenu), keyEquivalent: ",").target = self

        let launch = NSMenuItem(title: "Launch at Login",
                                action: #selector(toggleLaunchAtLogin), keyEquivalent: "")
        launch.target = self
        launch.state = (SMAppService.mainApp.status == .enabled) ? .on : .off
        menu.addItem(launch)

        menu.addItem(.separator())
        menu.addItem(withTitle: "Quit", action: #selector(quit), keyEquivalent: "q").target = self

        statusItem.menu = menu
        statusItem.button?.performClick(nil)
        statusItem.menu = nil
    }

    @objc private func refresh() { controller.refresh(force: true) }
    @objc private func quit() { NSApp.terminate(nil) }
    @objc private func openSettingsMenu() { openSettings() }

    @objc private func toggleLaunchAtLogin() {
        do {
            if SMAppService.mainApp.status == .enabled {
                try SMAppService.mainApp.unregister()
            } else {
                try SMAppService.mainApp.register()
            }
        } catch {
            NSLog("AgentUsageMenuBar: Launch at Login toggle failed: \(error.localizedDescription)")
        }
    }

    // MARK: - Settings window

    private func openSettings() {
        if popover.isShown { popover.performClose(nil) }

        if settingsWindow == nil {
            let view = SettingsView(settings: settings, controller: controller)
            let window = NSWindow(
                contentRect: NSRect(x: 0, y: 0, width: 460, height: 520),
                styleMask: [.titled, .closable], backing: .buffered, defer: false)
            window.title = "Agent Usage Settings"
            window.contentView = NSHostingView(rootView: view)
            window.isReleasedWhenClosed = false
            window.center()
            settingsWindow = window
        }
        NSApp.activate(ignoringOtherApps: true)
        settingsWindow?.makeKeyAndOrderFront(nil)
    }

    // MARK: - Bar rendering

    private func updateBar() {
        guard let button = statusItem.button else { return }
        let agents = controller.merged.filter { settings.isEnabled($0.agent.id) }
        if agents.isEmpty {
            button.image = controller.runtimeError == nil ? Self.placeholderImage() : Self.errorImage()
            return
        }
        let out = renderBar(agents: agents)
        button.image = out.length > 0 ? Self.render(out) : Self.placeholderImage()
    }

    // MARK: - Image drawing

    private static let barFont = NSFont.monospacedDigitSystemFont(ofSize: 12, weight: .medium)

    /// Build the menu bar content for the chosen display mode.
    private func renderBar(agents: [AgentSnapshot]) -> NSAttributedString {
        let font = Self.barFont
        let out = NSMutableAttributedString()
        let credit = settings.creditDisplay

        switch settings.menuBarMode {
        case .iconOnly:
            for (i, agent) in agents.enumerated() {
                if i > 0 { out.append(Self.segment(" ", color: .secondaryLabelColor, font: font)) }
                Self.appendGlyph(out, agent: agent, font: font, size: font.pointSize + 3)
            }

        case .worstMetric:
            if let (w, _) = Self.worstAcross(agents) {
                out.append(Self.segment(Self.barValue(w, credit: credit), color: w.pace.nsColor, font: font))
            }

        case .iconWorst:
            if let (w, agent) = Self.worstAcross(agents) {
                Self.appendGlyph(out, agent: agent, font: font, size: font.pointSize + 1)
                out.append(Self.segment(" ", color: .secondaryLabelColor, font: font))
                out.append(Self.segment(Self.barValue(w, credit: credit), color: w.pace.nsColor, font: font))
            }

        case .perAgentPercent:
            Self.appendPerAgent(out, agents: agents, font: font, both: false, credit: credit)

        case .perAgentBoth:
            Self.appendPerAgent(out, agents: agents, font: font, both: true, credit: credit)

        case .onlyAttention:
            let attention = agents.filter { !$0.isError && $0.worstPace.needsAttention }
            if attention.isEmpty {
                // Everything's on track — show the color-coded glyphs so the bar isn't blank.
                for (i, agent) in agents.enumerated() {
                    if i > 0 { out.append(Self.segment(" ", color: .secondaryLabelColor, font: font)) }
                    Self.appendGlyph(out, agent: agent, font: font, size: font.pointSize + 3)
                }
            } else {
                Self.appendPerAgent(out, agents: attention, font: font, both: true, credit: credit)
            }

        case .selectedAgent:
            let agent = agents.first { $0.agent.id == settings.selectedAgentID } ?? agents.first
            if let agent { Self.appendPerAgent(out, agents: [agent], font: font, both: true, credit: credit) }
        }
        return out
    }

    /// A faint vertical divider between agents, padded with spaces so adjacent agents'
    /// readings don't crowd the bar.
    private static func separator(_ font: NSFont) -> NSAttributedString {
        segment("  │  ", color: .tertiaryLabelColor, font: font)
    }

    /// Append a pace-tinted agent glyph (gold when the agent is in surplus, via displayPace).
    private static func appendGlyph(
        _ out: NSMutableAttributedString, agent: AgentSnapshot, font: NSFont, size: CGFloat
    ) {
        let color = agent.isError ? NSColor.secondaryLabelColor : agent.displayPace.nsColor
        if let glyph = AgentAssets.tintedGlyph(forID: agent.agent.id, color: color, size: size) {
            out.append(attachment(glyph, font: font))
        }
    }

    /// Append each agent as `glyph weekly · session %` (both) or `glyph worst%`, divided.
    private static func appendPerAgent(
        _ out: NSMutableAttributedString, agents: [AgentSnapshot], font: NSFont, both: Bool,
        credit: AppSettings.CreditDisplay
    ) {
        let sep = NSColor.secondaryLabelColor
        for (i, agent) in agents.enumerated() {
            if i > 0 { out.append(separator(font)) }
            appendGlyph(out, agent: agent, font: font, size: font.pointSize + 1)
            out.append(segment(" ", color: sep, font: font))

            if agent.isError {
                out.append(segment("—", color: sep, font: font))
                continue
            }

            if both, let w = agent.window("weekly"), let s = agent.window("session") {
                out.append(segment(intPct(w.usedPct), color: w.pace.nsColor, font: font))
                out.append(segment(" · ", color: sep, font: font))
                out.append(segment(intPct(s.usedPct) + "%", color: s.pace.nsColor, font: font))
            } else if let w = worstWindow(agent) {
                out.append(segment(Self.barValue(w, credit: credit), color: w.pace.nsColor, font: font))
            }
        }
    }

    /// The menu-bar reading for a window. A credit pool reports its raw remaining balance
    /// (the number the credits API returns), its remaining percentage, or both — per the
    /// `credit` preference; every other window shows used percentage.
    private static func barValue(_ w: WindowDTO, credit: AppSettings.CreditDisplay) -> String {
        guard w.kind == "credits", let pool = w.pool else {
            return intPct(w.usedPct) + "%"
        }
        let credits = formatCredits(pool.remaining)
        let pct = intPct(w.remainingPct) + "%"
        switch credit {
        case .credits: return credits
        case .percentage: return pct
        case .both: return "\(credits) · \(pct)"
        }
    }

    /// The window with the highest utilization for one agent.
    private static func worstWindow(_ agent: AgentSnapshot) -> WindowDTO? {
        (agent.windows ?? []).max { $0.usedPct < $1.usedPct }
    }

    /// The single highest-utilization window across all (non-error) agents, with its agent.
    private static func worstAcross(_ agents: [AgentSnapshot]) -> (WindowDTO, AgentSnapshot)? {
        var best: (WindowDTO, AgentSnapshot)?
        for agent in agents where !agent.isError {
            for w in agent.windows ?? [] where best == nil || w.usedPct > best!.0.usedPct {
                best = (w, agent)
            }
        }
        return best
    }

    private static func intPct(_ v: Double) -> String { String(Int(v.rounded())) }

    private static func errorImage() -> NSImage {
        render(segment("⚠ agents", color: PaceColor.red.nsColor,
                       font: .systemFont(ofSize: 12, weight: .medium)))
    }

    private static func placeholderImage() -> NSImage {
        render(segment("… agents", color: .secondaryLabelColor,
                       font: .monospacedDigitSystemFont(ofSize: 12, weight: .medium)))
    }

    private static func segment(_ s: String, color: NSColor, font: NSFont) -> NSAttributedString {
        NSAttributedString(string: s, attributes: [.foregroundColor: color, .font: font])
    }

    /// Wrap an already-tinted glyph image as a baseline-aligned text attachment so it sits
    /// inline with the percentage text in the menu bar.
    private static func attachment(_ image: NSImage, font: NSFont) -> NSAttributedString {
        let size = image.size
        let attachment = NSTextAttachment()
        attachment.image = image
        // Center the glyph on the text's cap height for a tidy baseline.
        let y = (font.capHeight - size.height) / 2
        attachment.bounds = CGRect(x: 0, y: y, width: size.width, height: size.height)
        return NSAttributedString(attachment: attachment)
    }

    /// Build a non-template (colored) menu bar image via a drawing handler so AppKit re-draws it
    /// in the button's current appearance (adaptive pace colors stay correct on Light/Dark toggle).
    private static func render(_ attributed: NSAttributedString) -> NSImage {
        let size = attributed.size()
        let height: CGFloat = 18
        let imageSize = NSSize(width: ceil(size.width), height: height)

        let image = NSImage(size: imageSize, flipped: false) { _ in
            let y = (height - size.height) / 2
            attributed.draw(at: NSPoint(x: 0, y: y))
            return true
        }
        image.isTemplate = false
        return image
    }
}

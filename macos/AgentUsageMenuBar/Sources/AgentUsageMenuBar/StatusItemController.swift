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

    @objc private func refresh() { controller.refresh() }
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
        } else {
            button.image = Self.barImage(agents: agents)
        }
    }

    // MARK: - Image drawing

    /// One segment per agent: a pace-tinted glyph followed by `weekly · session %`.
    private static func barImage(agents: [AgentSnapshot]) -> NSImage {
        let font = NSFont.monospacedDigitSystemFont(ofSize: 12, weight: .medium)
        let sep = NSColor.secondaryLabelColor
        let out = NSMutableAttributedString()

        for (i, agent) in agents.enumerated() {
            if i > 0 { out.append(segment("  ", color: sep, font: font)) }

            if let symbol = tintedSymbol(AgentGlyph.symbol(forID: agent.agent.id),
                                         color: agent.isError ? sep : agent.worstPace.nsColor,
                                         font: font) {
                out.append(symbol)
                out.append(segment(" ", color: sep, font: font))
            }

            if agent.isError {
                out.append(segment("—", color: sep, font: font))
                continue
            }

            let weekly = agent.window("weekly")
            let session = agent.window("session") ?? agent.secondaryWindow
            if let w = weekly, let s = session {
                out.append(segment(intPct(w.usedPct), color: w.pace.nsColor, font: font))
                out.append(segment(" · ", color: sep, font: font))
                out.append(segment(intPct(s.usedPct) + "%", color: s.pace.nsColor, font: font))
            } else if let only = agent.primaryWindow {
                out.append(segment(intPct(only.usedPct) + "%", color: only.pace.nsColor, font: font))
            }
        }
        return render(out)
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

    /// An SF Symbol recolored to `color`, wrapped as a baseline-aligned text attachment.
    private static func tintedSymbol(_ name: String, color: NSColor, font: NSFont) -> NSAttributedString? {
        let cfg = NSImage.SymbolConfiguration(pointSize: font.pointSize, weight: .medium)
        guard let base = NSImage(systemSymbolName: name, accessibilityDescription: nil)?
            .withSymbolConfiguration(cfg) else { return nil }

        let size = base.size
        let tinted = NSImage(size: size, flipped: false) { rect in
            base.draw(in: rect)
            color.set()
            rect.fill(using: .sourceAtop)
            return true
        }
        tinted.isTemplate = false

        let attachment = NSTextAttachment()
        attachment.image = tinted
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

import AppKit
import Combine
import ServiceManagement
import SwiftUI

/// Owns the menu bar item. Draws an at-a-glance, per-agent indicator (each agent's glyph plus
/// its weekly reading with the near-term one tucked diagonally underneath, tinted by pace)
/// into the bar button, toggles the SwiftUI dashboard popover on left-click, and shows a
/// context menu on right-click.
final class StatusItemController {
    private let statusItem: NSStatusItem
    private let popover = NSPopover()
    private let hostingController: NSHostingController<DashboardView>
    private let controller: DataController
    private let settings: AppSettings
    private var settingsWindow: NSWindow?
    private var cancellables = Set<AnyCancellable>()
    private var appearanceObservation: NSKeyValueObservation?
    /// Appearance the bar was last baked under. Suppresses no-op `effectiveAppearance`
    /// observations — see where it's compared in `init`.
    private var lastRenderedAppearance: NSAppearance.Name?

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
            button.image = placeholderImage()
            button.imagePosition = .imageOnly
            button.target = self
            button.action = #selector(handleClick(_:))
            button.sendAction(on: [.leftMouseUp, .rightMouseUp])

            // Re-bake when the system flips Light/Dark so the adaptive pace colors re-resolve.
            // The observation is only a HINT that the appearance may have changed, never a
            // source of values: replacing the button's image re-fires it (KVO does not
            // suppress equal values), and its `newValue` reports the app-level appearance
            // (Aqua/DarkAqua), not the status button's vibrant variant — so an unguarded, or
            // payload-comparing, `updateBar` here bakes → sets the image → re-fires → bakes
            // again, forever. That loop pegged a full core (~3ms of PDF rasterization +
            // typesetting per turn, hundreds of turns/sec). The truth is the button's own
            // `effectiveAppearance` (stable across image sets, measured): re-render only when
            // what we'd bake under differs from what we last baked under. `updateBar` re-syncs
            // the record after every bake it does.
            lastRenderedAppearance = button.effectiveAppearance.name
            appearanceObservation = button.observe(\.effectiveAppearance, options: [.new]) { [weak self] _, _ in
                DispatchQueue.main.async {
                    guard let self else { return }
                    let name = button.effectiveAppearance.name
                    guard name != self.lastRenderedAppearance else { return }
                    self.lastRenderedAppearance = name
                    self.updateBar()
                }
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
                contentRect: NSRect(x: 0, y: 0, width: 460, height: 640),
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
            button.image = controller.runtimeError == nil ? placeholderImage() : errorImage()
        } else if let layout = renderBar(agents: agents) {
            button.image = render(layout)
        } else {
            button.image = placeholderImage()
        }
        // Every path above replaced the button's image, which re-fires the appearance
        // observation (see `init`); recording the appearance just baked under lets that
        // re-fire be recognized as the no-op it is.
        lastRenderedAppearance = button.effectiveAppearance.name
    }

    // MARK: - Image drawing

    private static let barFont = NSFont.monospacedDigitSystemFont(ofSize: 12, weight: .medium)

    /// The tucked-in near-term reading's font in the two-number layout: smaller than the
    /// weekly number it sits beneath, monospaced digits to hold the layout steady poll to poll.
    private static let nearFont = NSFont.monospacedDigitSystemFont(ofSize: 10, weight: .medium)

    /// Fallback bake height until the status button reports its real one. The system centers
    /// the button's 22pt band in the bar (taller on a notched display), so this only matters
    /// for the first placeholder bake.
    private static let barImageHeight: CGFloat = 18

    /// Padding that keeps the glyph's ink off the button band's edges, the way the system
    /// items' icons do.
    private static let elementPad: CGFloat = 3
    /// Horizontal gap between a glyph and its agent's numbers.
    private static let textGap: CGFloat = 4
    /// Horizontal gap between adjacent agents' columns — no divider, just air.
    private static let columnGap: CGFloat = 7
    /// Horizontal gap between glyphs in the icon-only modes.
    private static let iconGap: CGFloat = 4

    /// Vertical space the system gives this item's content: the status button's height. On
    /// both classic and notched bars that is 22pt — the bar is taller on a notched display,
    /// but the system centers a 22pt band in it for every item (measured: the button's
    /// superview sits at y≈5.5 in a 33pt window), so this is the full height any menu bar
    /// item gets to draw in.
    private var barHeight: CGFloat {
        let height = statusItem.button?.bounds.height ?? 0
        return height > 0 ? height : Self.barImageHeight
    }

    /// One positioned piece of bar content, in the bitmap's coordinate space (y measured
    /// from the bottom — the `NSImage.lockFocus` context is non-flipped). A text run's line
    /// box bottom lands at its point (baseline = `point.y + |descender|`); an image's
    /// bottom-left corner lands at its point.
    private enum BarRun {
        case text(NSAttributedString, NSPoint)
        case image(NSImage, NSPoint)
    }

    /// The full content of one baked bar image: positioned runs, the width they span, and the
    /// bitmap height they are composed into (the button's full band — see `barHeight`).
    private struct BarLayout {
        let runs: [BarRun]
        let width: CGFloat
        let height: CGFloat
    }

    /// A single text line, vertically centered in the bar's band.
    private func singleLine(_ text: NSAttributedString) -> BarLayout {
        let height = barHeight
        let y = (height - text.size().height) / 2
        return BarLayout(runs: [.text(text, NSPoint(x: 0, y: y))],
                         width: text.size().width, height: height)
    }

    /// Build the menu bar content for the chosen display mode. Returns nil when the mode has
    /// nothing to show for these agents (e.g. every agent errored in "worst metric").
    private func renderBar(agents: [AgentSnapshot]) -> BarLayout? {
        let font = Self.barFont
        let height = barHeight
        let credit = settings.creditDisplay

        switch settings.menuBarMode {
        case .iconOnly:
            return Self.iconRow(agents, barHeight: height)

        case .worstMetric:
            guard let (w, _) = Self.worstAcross(agents) else { return nil }
            return singleLine(Self.segment(Self.barValue(w, credit: credit), color: w.pace.nsColor, font: font))

        case .iconWorst:
            // The globally worst window belongs to some agent, and that agent's own worst
            // window is the same one, so its column shows the same reading as before.
            guard let (_, agent) = Self.worstAcross(agents) else { return nil }
            return Self.perAgentLine([agent], font: font, credit: credit, barHeight: height)

        case .perAgentPercent:
            return Self.perAgentLine(agents, font: font, credit: credit, barHeight: height)

        case .perAgentBoth:
            return Self.perAgentDiagonal(agents, credit: credit, barHeight: height)

        case .onlyAttention:
            let attention = agents.filter { !$0.isError && $0.worstPace.needsAttention }
            if attention.isEmpty {
                // Everything's on track — show the color-coded glyphs so the bar isn't blank.
                return Self.iconRow(agents, barHeight: height)
            }
            return Self.perAgentDiagonal(attention, credit: credit, barHeight: height)

        case .selectedAgent:
            let agent = agents.first { $0.agent.id == settings.selectedAgentID } ?? agents.first
            guard let agent else { return nil }
            return Self.perAgentDiagonal([agent], credit: credit, barHeight: height)
        }
    }

    /// The color-coded glyph row shared by "icon only" and "only attention" when nothing
    /// needs attention.
    private static func iconRow(_ agents: [AgentSnapshot], barHeight: CGFloat) -> BarLayout? {
        var runs: [BarRun] = []
        var x: CGFloat = 0
        for agent in agents {
            if !runs.isEmpty { x += iconGap }
            guard let (run, width) = glyphRun(agent, barHeight: barHeight, at: x) else { continue }
            runs.append(run)
            x += width
        }
        return runs.isEmpty ? nil : BarLayout(runs: runs, width: x, height: barHeight)
    }

    /// One agent's logo as a full-height bar element: vertically centered in the item's
    /// band and sized to fill it (minus `elementPad`), like other menu bar items' icons —
    /// not an inline attachment riding one text line's cap height.
    private static func glyphRun(
        _ agent: AgentSnapshot, barHeight: CGFloat, at x: CGFloat
    ) -> (run: BarRun, width: CGFloat)? {
        let color = agent.isError ? NSColor.secondaryLabelColor : agent.displayPace.nsColor
        guard let glyph = AgentAssets.tintedGlyph(
            forID: agent.agent.id, color: color, size: barHeight - 2 * elementPad) else { return nil }
        let y = (barHeight - glyph.size.height) / 2
        return (.image(glyph, NSPoint(x: x, y: y)), glyph.size.width)
    }

    /// The near-term reading paired with the weekly one in the bar's two-number mode.
    ///
    /// Normally the agent's own short window. When an agent bills a single multi-day quota and
    /// exposes no short window, the sampled burst takes that slot — the second number's job is to
    /// answer "how hot am I running right now", and without it a bar showing only a slow-moving
    /// weekly percentage gives no sign that the cycle is being eaten in one sitting.
    private static func nearTermReading(_ agent: AgentSnapshot) -> (text: String, color: NSColor)? {
        if let s = agent.window("session") {
            return (intPct(s.usedPct) + "%", s.pace.nsColor)
        }
        if let burst = agent.trend?.recent {
            return (intPct(burst.usedPct) + "%", burst.pace.nsColor)
        }
        return nil
    }

    /// Each agent as `glyph worst%` (its highest-utilization window), columns separated
    /// by a plain gap — the single-number per-agent line.
    private static func perAgentLine(
        _ agents: [AgentSnapshot], font: NSFont, credit: AppSettings.CreditDisplay,
        barHeight: CGFloat
    ) -> BarLayout? {
        let sep = NSColor.secondaryLabelColor
        var runs: [BarRun] = []
        var x: CGFloat = 0
        for (i, agent) in agents.enumerated() {
            if i > 0 { x += columnGap }
            if let (run, width) = glyphRun(agent, barHeight: barHeight, at: x) {
                runs.append(run)
                x += width + textGap
            }

            let text = agent.isError
                ? segment("—", color: sep, font: font)
                : worstWindow(agent).map { segment(barValue($0, credit: credit), color: $0.pace.nsColor, font: font) }
            guard let text else { continue }
            runs.append(.text(text, NSPoint(x: x, y: (barHeight - text.size().height) / 2)))
            x += text.size().width
        }
        return runs.isEmpty ? nil : BarLayout(runs: runs, width: x, height: barHeight)
    }

    /// The two-number per-agent layout: the weekly reading at the bar's usual size with the
    /// near-term one tucked diagonally underneath, right-aligned to the weekly number's
    /// trailing edge. The glyph stands apart from the numbers as a full-height element (see
    /// `glyphRun`); columns are separated by a plain gap, nothing else.
    ///
    /// The stagger is what buys the font size back. A strictly stacked pair needs a full line
    /// of separation and lands around 9pt to fit the bar's fixed height; staggered diagonally,
    /// the two line boxes overlap, and since a reading is digits and `%` (ink that never
    /// descends below its baseline), the only real constraint is that the tucked reading's
    /// ink top clear the weekly number's baseline by `gap`. Every weekly reading shares one
    /// baseline and every tucked one another, so the two ink bands are globally disjoint —
    /// no per-column collision checking is needed.
    ///
    /// Y semantics in the `NSImage.lockFocus` context (non-flipped): `draw(at:)` puts a run's
    /// line box bottom at its point and the baseline at `point.y + |descender|`. The tucked
    /// runs' points are therefore negative — their boxes' empty descender space hangs below
    /// the bitmap — which is what pins their ink to the centered block's bottom edge.
    private static func perAgentDiagonal(
        _ agents: [AgentSnapshot], credit: AppSettings.CreditDisplay, barHeight: CGFloat
    ) -> BarLayout? {
        let topFont = barFont
        let nearFont = Self.nearFont
        let sep = NSColor.secondaryLabelColor

        // All readings are digits + `%`, so each font's ink height is one constant; measure
        // it rather than hard-coding SF Pro's `%` overshoot past cap height.
        let nearInkH = inkHeight(segment("0%", color: sep, font: nearFont))
        let topInkH = inkHeight(segment("0%", color: sep, font: topFont))
        let gap: CGFloat = 1.75
        // Center the two-number ink block in the item's band. Agents that fall back to one
        // number still share the weekly baseline, so every column stays on one grid.
        let blockH = nearInkH + gap + topInkH
        let topBaseline = max((barHeight - blockH) / 2, 0) + nearInkH + gap
        let nearY = topBaseline - nearInkH - gap + nearFont.descender
        let topY = topBaseline + topFont.descender

        var runs: [BarRun] = []
        var x: CGFloat = 0
        var anyNear = false
        for (i, agent) in agents.enumerated() {
            if i > 0 { x += columnGap }
            if let (run, width) = glyphRun(agent, barHeight: barHeight, at: x) {
                runs.append(run)
                x += width + textGap
            }
            let numberX = x

            if agent.isError {
                let text = segment("—", color: sep, font: topFont)
                runs.append(.text(text, NSPoint(x: numberX, y: topY)))
                x = numberX + text.size().width
                continue
            }

            if let w = agent.window("weekly"), let reading = nearTermReading(agent) {
                let weekly = segment(intPct(w.usedPct) + "%", color: w.pace.nsColor, font: topFont)
                let near = segment(reading.text, color: reading.color, font: nearFont)
                runs.append(.text(weekly, NSPoint(x: numberX, y: topY)))
                // Right-align under the weekly number's trailing edge, clamped to its left
                // edge so a wider reading protrudes right instead of sliding back over the glyph.
                let nearX = max(numberX, numberX + weekly.size().width - near.size().width)
                runs.append(.text(near, NSPoint(x: nearX, y: nearY)))
                anyNear = true
                x = max(numberX + weekly.size().width, nearX + near.size().width)
            } else if let w = worstWindow(agent) {
                let text = segment(barValue(w, credit: credit), color: w.pace.nsColor, font: topFont)
                runs.append(.text(text, NSPoint(x: numberX, y: topY)))
                x = numberX + text.size().width
            }
        }
        // No tucked readings at all (every agent errored or reports a single window): the
        // stagger would just strand one line beside full-height glyphs.
        guard anyNear else { return perAgentLine(agents, font: topFont, credit: credit, barHeight: barHeight) }
        return BarLayout(runs: runs, width: x, height: barHeight)
    }

    /// Ink (not line-box) height of a single-line string — the clearance math in
    /// `perAgentDiagonal` needs real glyph extents because `%` overshoots cap height.
    private static func inkHeight(_ s: NSAttributedString) -> CGFloat {
        s.boundingRect(with: CGSize(width: 10_000, height: 10_000),
                       options: [.usesLineFragmentOrigin, .usesDeviceMetrics]).height
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

    private func errorImage() -> NSImage {
        render(singleLine(Self.segment("⚠ agents", color: PaceColor.red.nsColor,
                                         font: .systemFont(ofSize: 12, weight: .medium))))
    }

    private func placeholderImage() -> NSImage {
        render(singleLine(Self.segment("… agents", color: .secondaryLabelColor,
                                        font: .monospacedDigitSystemFont(ofSize: 12, weight: .medium))))
    }

    private static func segment(_ s: String, color: NSColor, font: NSFont) -> NSAttributedString {
        NSAttributedString(string: s, attributes: [.foregroundColor: color, .font: font])
    }

    /// Bake a non-template (colored) menu bar image eagerly into a bitmap on the main thread.
    ///
    /// We deliberately do NOT use a lazily `drawingHandler`-backed `NSImage` here: AppKit re-invokes
    /// that handler at unpredictable times (menu bar redraws, Space switches, and occasionally off
    /// the main thread), and an async `NSAttributedString.draw` can yield an empty frame — which
    /// makes the status item blink out and reappear erratically. A static bitmap can't blank out.
    /// Adaptive pace colors stay correct on Light/Dark toggle because `appearanceObservation`
    /// re-runs `updateBar()` on every appearance change, re-baking under the new appearance below.
    private func render(_ layout: BarLayout) -> NSImage {
        let imageSize = NSSize(width: max(1, ceil(layout.width)), height: layout.height)
        let appearance = statusItem.button?.effectiveAppearance ?? NSAppearance.currentDrawing()

        let image = NSImage(size: imageSize)
        image.lockFocus()
        appearance.performAsCurrentDrawingAppearance {
            for run in layout.runs {
                switch run {
                case .text(let text, let point):
                    text.draw(at: point)
                case .image(let glyph, let point):
                    glyph.draw(at: point, from: .zero, operation: .sourceOver, fraction: 1)
                }
            }
        }
        image.unlockFocus()
        image.isTemplate = false
        return image
    }
}

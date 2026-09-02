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

    /// Height of the baked single-line bar image. The two-number layout bakes taller
    /// (see `perAgentDiagonal`); both fit the system menu bar's status item area.
    private static let barImageHeight: CGFloat = 18

    /// One independently positioned line of bar content. `point` is where `draw(at:)` puts
    /// the line box's bottom-left corner (see `perAgentDiagonal` for the y semantics).
    private struct BarRun {
        let text: NSAttributedString
        let point: NSPoint
    }

    /// The full content of one baked bar image: positioned runs plus the bitmap height they
    /// are composed into. Single-line modes are one run at the standard height; the
    /// two-number layout staggers two runs per agent into a taller bitmap.
    private struct BarLayout {
        let runs: [BarRun]
        let height: CGFloat

        var width: CGFloat {
            max(1, ceil(runs.map { $0.point.x + $0.text.size().width }.max() ?? 0))
        }

        /// One run, vertically centered in the standard single-line image height.
        static func single(_ text: NSAttributedString) -> BarLayout {
            let y = (StatusItemController.barImageHeight - text.size().height) / 2
            return BarLayout(runs: [BarRun(text: text, point: NSPoint(x: 0, y: y))],
                             height: StatusItemController.barImageHeight)
        }
    }

    /// Build the menu bar content for the chosen display mode. Returns nil when the mode has
    /// nothing to show for these agents (e.g. every agent errored in "worst metric").
    private func renderBar(agents: [AgentSnapshot]) -> BarLayout? {
        let font = Self.barFont
        let credit = settings.creditDisplay

        switch settings.menuBarMode {
        case .iconOnly:
            return Self.iconRow(agents, font: font)

        case .worstMetric:
            guard let (w, _) = Self.worstAcross(agents) else { return nil }
            return .single(Self.segment(Self.barValue(w, credit: credit), color: w.pace.nsColor, font: font))

        case .iconWorst:
            guard let (w, agent) = Self.worstAcross(agents) else { return nil }
            let out = NSMutableAttributedString()
            Self.appendGlyph(out, agent: agent, font: font, size: font.pointSize + 1)
            out.append(Self.segment(" ", color: .secondaryLabelColor, font: font))
            out.append(Self.segment(Self.barValue(w, credit: credit), color: w.pace.nsColor, font: font))
            return .single(out)

        case .perAgentPercent:
            return Self.perAgentLine(agents, font: font, credit: credit)

        case .perAgentBoth:
            return Self.perAgentDiagonal(agents, credit: credit)

        case .onlyAttention:
            let attention = agents.filter { !$0.isError && $0.worstPace.needsAttention }
            if attention.isEmpty {
                // Everything's on track — show the color-coded glyphs so the bar isn't blank.
                return Self.iconRow(agents, font: font)
            }
            return Self.perAgentDiagonal(attention, credit: credit)

        case .selectedAgent:
            let agent = agents.first { $0.agent.id == settings.selectedAgentID } ?? agents.first
            guard let agent else { return nil }
            return Self.perAgentDiagonal([agent], credit: credit)
        }
    }

    /// The color-coded glyph row shared by "icon only" and "only attention" when nothing
    /// needs attention.
    private static func iconRow(_ agents: [AgentSnapshot], font: NSFont) -> BarLayout? {
        let out = NSMutableAttributedString()
        for (i, agent) in agents.enumerated() {
            if i > 0 { out.append(segment(" ", color: .secondaryLabelColor, font: font)) }
            appendGlyph(out, agent: agent, font: font, size: font.pointSize + 3)
        }
        return out.length > 0 ? .single(out) : nil
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

    /// Each agent as `glyph worst%` (its highest-utilization window), divided — the
    /// single-number per-agent line.
    private static func perAgentLine(
        _ agents: [AgentSnapshot], font: NSFont, credit: AppSettings.CreditDisplay
    ) -> BarLayout? {
        let sep = NSColor.secondaryLabelColor
        let out = NSMutableAttributedString()
        for (i, agent) in agents.enumerated() {
            if i > 0 { out.append(separator(font)) }
            appendGlyph(out, agent: agent, font: font, size: font.pointSize + 1)
            out.append(segment(" ", color: sep, font: font))

            if agent.isError {
                out.append(segment("—", color: sep, font: font))
                continue
            }
            if let w = worstWindow(agent) {
                out.append(segment(barValue(w, credit: credit), color: w.pace.nsColor, font: font))
            }
        }
        return out.length > 0 ? .single(out) : nil
    }

    /// The two-number per-agent layout: the weekly reading at the bar's usual size with the
    /// near-term one tucked diagonally underneath, right-aligned to the weekly number's
    /// trailing edge.
    ///
    /// The stagger is what buys the font size back. A strictly stacked pair needs a full line
    /// of separation and lands around 9pt to fit the bar's fixed height; staggered diagonally,
    /// the two line boxes overlap, and since a reading is digits and `%` (ink that never
    /// descends below its baseline), the only real constraint is that the tucked reading's
    /// ink top clear the weekly number's baseline by `gap`. Every weekly reading shares one
    /// baseline and every tucked one another, so the two ink bands are globally disjoint —
    /// no per-column collision checking is needed. The agent glyph is the one exception: it
    /// hangs below the weekly baseline, so a tucked reading is clamped to start past it.
    ///
    /// Y semantics in the `NSImage.lockFocus` context (non-flipped): `draw(at:)` puts a run's
    /// line box bottom at its point and the baseline at `point.y + |descender|`. The tucked
    /// runs' points are therefore negative — their boxes' empty descender space hangs below
    /// the bitmap — which is what pins their ink to the bottom padding.
    private static func perAgentDiagonal(
        _ agents: [AgentSnapshot], credit: AppSettings.CreditDisplay
    ) -> BarLayout? {
        let topFont = barFont
        let nearFont = Self.nearFont
        let sep = NSColor.secondaryLabelColor

        // All near-term readings are digits + `%`, so their ink height is one constant;
        // measure it rather than hard-coding SF Pro's `%` overshoot past cap height.
        let nearInkH = inkHeight(segment("0%", color: sep, font: nearFont))
        let glyphH = topFont.pointSize + 1
        let gap: CGFloat = 1.75, bottomPad: CGFloat = 0.5, topPad: CGFloat = 0.5
        let nearBaseline = bottomPad
        let topBaseline = bottomPad + nearInkH + gap
        // Tallest ink above the weekly baseline: the agent glyph (centered on cap height),
        // or the inter-agent `│` divider, which can reach the ascender in its fallback font.
        let inkAboveTop = max((topFont.capHeight + glyphH) / 2, topFont.ascender)
        let height = ceil(topBaseline + inkAboveTop + topPad)
        let nearY = nearBaseline + nearFont.descender
        let topY = topBaseline + topFont.descender

        var runs: [BarRun] = []
        var x: CGFloat = 0
        var anyNear = false
        for (i, agent) in agents.enumerated() {
            let top = NSMutableAttributedString()
            // A tighter divider than the single-line mode's: the tucked reading already
            // narrows each agent's column, so wide gutters would dominate the bar.
            if i > 0 { top.append(segment(" │ ", color: .tertiaryLabelColor, font: topFont)) }
            appendGlyph(top, agent: agent, font: topFont, size: glyphH)
            top.append(segment(" ", color: sep, font: topFont))

            var near: (text: NSAttributedString, x: CGFloat)?

            if agent.isError {
                top.append(segment("—", color: sep, font: topFont))
            } else if let w = agent.window("weekly"), let reading = nearTermReading(agent) {
                // Where the weekly number starts, past the glyph: the tucked reading may
                // not slide left of this, whatever its width.
                let numberLeft = top.size().width
                top.append(segment(intPct(w.usedPct) + "%", color: w.pace.nsColor, font: topFont))
                let text = segment(reading.text, color: reading.color, font: nearFont)
                // Right-align under the weekly number's trailing edge; a wider reading
                // protrudes right instead of crowding the glyph.
                let bx = max(x + numberLeft, x + top.size().width - text.size().width)
                near = (text, bx)
            } else if let w = worstWindow(agent) {
                top.append(segment(barValue(w, credit: credit), color: w.pace.nsColor, font: topFont))
            }

            runs.append(BarRun(text: top, point: NSPoint(x: x, y: topY)))
            if let near {
                runs.append(BarRun(text: near.text, point: NSPoint(x: near.x, y: nearY)))
                anyNear = true
            }
            x = max(x + top.size().width, near.map { $0.x + $0.text.size().width } ?? 0)
        }
        // No tucked readings at all (every agent errored or reports a single window): the
        // stagger would just strand one line at the top of an oversized bitmap.
        guard anyNear else { return perAgentLine(agents, font: topFont, credit: credit) }
        return BarLayout(runs: runs, height: height)
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
        render(Self.segment("⚠ agents", color: PaceColor.red.nsColor,
                            font: .systemFont(ofSize: 12, weight: .medium)))
    }

    private func placeholderImage() -> NSImage {
        render(Self.segment("… agents", color: .secondaryLabelColor,
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

    /// Bake a non-template (colored) menu bar image eagerly into a bitmap on the main thread.
    ///
    /// We deliberately do NOT use a lazily `drawingHandler`-backed `NSImage` here: AppKit re-invokes
    /// that handler at unpredictable times (menu bar redraws, Space switches, and occasionally off
    /// the main thread), and an async `NSAttributedString.draw` can yield an empty frame — which
    /// makes the status item blink out and reappear erratically. A static bitmap can't blank out.
    /// Adaptive pace colors stay correct on Light/Dark toggle because `appearanceObservation`
    /// re-runs `updateBar()` on every appearance change, re-baking under the new appearance below.
    private func render(_ layout: BarLayout) -> NSImage {
        let imageSize = NSSize(width: layout.width, height: layout.height)
        let appearance = statusItem.button?.effectiveAppearance ?? NSAppearance.currentDrawing()

        let image = NSImage(size: imageSize)
        image.lockFocus()
        appearance.performAsCurrentDrawingAppearance {
            for run in layout.runs {
                run.text.draw(at: run.point)
            }
        }
        image.unlockFocus()
        image.isTemplate = false
        return image
    }

    /// The fixed-height single-line bake, used by the placeholder and error images.
    private func render(_ attributed: NSAttributedString) -> NSImage {
        render(BarLayout.single(attributed))
    }
}

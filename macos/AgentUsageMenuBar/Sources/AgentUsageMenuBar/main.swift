import AppKit

/// AppKit bootstrap for a menu-bar-only agent app. `LSUIElement` (Info.plist) keeps it out of
/// the Dock; `.accessory` is the runtime equivalent so it also behaves when launched directly
/// via `swift run` (no bundle).
final class AppDelegate: NSObject, NSApplicationDelegate {
    private let settings = AppSettings()
    private lazy var dataController = DataController(settings: settings)
    private var statusController: StatusItemController?

    func applicationDidFinishLaunching(_ notification: Notification) {
        settings.applyAppearance()
        statusController = StatusItemController(controller: dataController, settings: settings)
        dataController.start()
    }
}

let app = NSApplication.shared
let delegate = AppDelegate()
app.delegate = delegate
app.setActivationPolicy(.accessory)
app.run()

// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "AgentUsageMenuBar",
    platforms: [.macOS(.v13)], // AppKit NSStatusItem + SwiftUI popover; Gauge/SMAppService era
    targets: [
        .executableTarget(
            name: "AgentUsageMenuBar",
            path: "Sources/AgentUsageMenuBar"
        )
    ]
)

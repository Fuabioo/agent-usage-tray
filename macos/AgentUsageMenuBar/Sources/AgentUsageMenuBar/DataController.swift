import Combine
import Foundation

/// A spawn/IO/decoding failure from running the CLI (distinct from a per-agent `error`
/// document, which decodes successfully into an `AgentSnapshot`).
struct CLIError: Error {
    let message: String
}

/// Polls the bundled `agent-usage all --json` binary on a timer and publishes the decoded
/// per-agent snapshots. Keeps the last good set across transient failures so the UI shows
/// stale-but-useful data rather than blanking out.
final class DataController: ObservableObject {
    /// Latest decoded agents (each may itself be a per-agent error document).
    @Published private(set) var agents: [AgentSnapshot] = []
    /// Last set that contained at least one successful agent, retained across failures.
    @Published private(set) var lastGood: [AgentSnapshot] = []
    /// A spawn/IO/decoding problem that isn't a CLI-reported error (e.g. binary missing).
    @Published private(set) var runtimeError: String?
    @Published private(set) var lastUpdated: Date?

    private let settings: AppSettings
    private var timer: Timer?
    private var cancellables = Set<AnyCancellable>()
    private let decoder = JSONDecoder.agentUsage()

    /// Fixed poll interval (seconds). The CLI is one-shot and cheap; 5 min matches the core
    /// defaults. Manual Refresh is always available.
    private let pollInterval: TimeInterval = 300

    init(settings: AppSettings) {
        self.settings = settings
        // Re-fetch when the work-days budget changes so coloring updates immediately.
        settings.$workDays
            .dropFirst()
            .sink { [weak self] _ in self?.refresh() }
            .store(in: &cancellables)
    }

    /// Latest agents with a per-agent stale fallback: if an agent errored this run but we have
    /// a good prior reading for it, substitute that. Not filtered by settings — callers filter.
    var merged: [AgentSnapshot] {
        let current = agents.isEmpty ? lastGood : agents
        return current.map { snap in
            guard snap.isError,
                  let good = lastGood.first(where: { $0.id == snap.id && !$0.isError })
            else { return snap }
            return good
        }
    }

    func start() {
        refresh()
        timer = Timer.scheduledTimer(withTimeInterval: pollInterval, repeats: true) { [weak self] _ in
            self?.refresh()
        }
    }

    func refresh() {
        let workDays = settings.workDays
        let dailyBudget = settings.dailyBudget
        DispatchQueue.global(qos: .utility).async { [weak self] in
            guard let self else { return }
            let result = Self.runCLI(decoder: self.decoder, workDays: workDays, dailyBudget: dailyBudget)
            DispatchQueue.main.async { self.apply(result) }
        }
    }

    private func apply(_ result: Result<[AgentSnapshot], CLIError>) {
        switch result {
        case .success(let snaps):
            self.agents = snaps
            self.runtimeError = nil
            if snaps.contains(where: { !$0.isError }) { self.lastGood = snaps }
            self.lastUpdated = Date()
        case .failure(let error):
            self.runtimeError = error.message
            self.lastUpdated = Date()
        }
    }

    /// Run `agent-usage all --json --work-days N --daily-budget B` and decode the array.
    private static func runCLI(
        decoder: JSONDecoder, workDays: Int, dailyBudget: Double
    ) -> Result<[AgentSnapshot], CLIError> {
        let launch = resolveLaunch()

        let process = Process()
        process.executableURL = URL(fileURLWithPath: launch.executable)
        process.arguments = launch.leadingArgs + [
            "all", "--json",
            "--work-days", String(workDays),
            "--daily-budget", String(format: "%.4f", dailyBudget),
        ]

        let stdout = Pipe()
        process.standardOutput = stdout
        process.standardError = Pipe()

        do {
            try process.run()
        } catch {
            return .failure(CLIError(message: "failed to launch agent-usage: \(error.localizedDescription)"))
        }

        let data = stdout.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()

        do {
            let snaps = try decoder.decode([AgentSnapshot].self, from: data)
            return .success(snaps)
        } catch {
            let raw = String(data: data, encoding: .utf8) ?? "<non-utf8>"
            return .failure(CLIError(
                message: "could not decode agent-usage output: \(error.localizedDescription)\n\(raw)"))
        }
    }

    struct Launch {
        let executable: String
        let leadingArgs: [String]
    }

    /// Resolution order: $AGENT_USAGE_BIN → bundled Resources → next to the executable → PATH.
    static func resolveLaunch() -> Launch {
        let fm = FileManager.default

        if let env = ProcessInfo.processInfo.environment["AGENT_USAGE_BIN"],
           fm.isExecutableFile(atPath: env) {
            return Launch(executable: env, leadingArgs: [])
        }
        if let res = Bundle.main.resourceURL?.appendingPathComponent("agent-usage").path,
           fm.isExecutableFile(atPath: res) {
            return Launch(executable: res, leadingArgs: [])
        }
        let exeDir = Bundle.main.bundleURL.deletingLastPathComponent()
        let sibling = exeDir.appendingPathComponent("agent-usage").path
        if fm.isExecutableFile(atPath: sibling) {
            return Launch(executable: sibling, leadingArgs: [])
        }
        return Launch(executable: "/usr/bin/env", leadingArgs: ["agent-usage"])
    }
}

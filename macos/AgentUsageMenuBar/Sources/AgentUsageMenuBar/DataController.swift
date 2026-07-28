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
        // `receive(on:)` defers the refresh to the next runloop tick so it doesn't re-enter the
        // @Published property's getter while it's still publishing the change (a reentrant read
        // there crashes — EXC_BAD_ACCESS in swift_dynamicCast).
        settings.$workDays
            .dropFirst()
            .receive(on: RunLoop.main)
            .sink { [weak self] _ in self?.refresh() }
            .store(in: &cancellables)

        // Same for Hyper's reset time, debounced: it's typed a character at a time, and every
        // intermediate value ("0", "08", "08:") would otherwise spawn a CLI run and flash a
        // parse error. `force` bypasses the cache so the new reset takes effect immediately.
        settings.$hyperResetTime
            .dropFirst()
            .debounce(for: .milliseconds(600), scheduler: RunLoop.main)
            .removeDuplicates()
            .sink { [weak self] _ in self?.refresh(force: true) }
            .store(in: &cancellables)

        // Mirror what the CLI can't derive into its config file, so `agent-usage` run by hand
        // agrees with the menu bar. Debounced for the same reason as above.
        Publishers.Merge(
            settings.$workDays.map { _ in () },
            settings.$hyperResetTime.map { _ in () }
        )
        .dropFirst()
        .debounce(for: .milliseconds(600), scheduler: RunLoop.main)
        .sink { [weak self] in self?.syncConfig(allowClear: true) }
        .store(in: &cancellables)
    }

    /// Whether the CLI's config file currently holds a Hyper API key. Read back from the CLI
    /// rather than stored here — the key itself must live in exactly one place, and that place is
    /// the `0600` config file, not a world-readable preferences plist.
    @Published private(set) var hyperKeyIsSet = false

    /// Mirror the settings the CLI can't derive into its config file.
    ///
    /// The app passes these as flags too, which is what makes them take effect *now*. Writing
    /// them as well is what makes `agent-usage` correct when run by hand.
    ///
    /// `allowClear` distinguishes the two callers. A user emptying the reset field means "remove
    /// it", so a change-driven sync passes an empty value through. The **startup** sync must not:
    /// a fresh install has an empty setting and would otherwise erase a `reset_time` the user had
    /// hand-written into the config file, having never touched the app at all.
    func syncConfig(allowClear: Bool) {
        let workDays = settings.workDays
        let resetTime = settings.hyperResetTime.trimmingCharacters(in: .whitespaces)
        var args = ["--work-days", String(workDays)]
        if allowClear || !resetTime.isEmpty {
            args += ["--reset-time", resetTime]
        }
        DispatchQueue.global(qos: .utility).async {
            _ = Self.runConfigSave(args: args, stdin: nil)
        }
    }

    /// Store (or, with an empty string, remove) the Hyper API key, then refresh.
    ///
    /// The key travels on the child's **stdin**, never in its arguments: an argument is readable
    /// by any process on the machine through `ps`.
    func saveHyperAPIKey(_ key: String, completion: @escaping (String?) -> Void) {
        let trimmed = key.trimmingCharacters(in: .whitespacesAndNewlines)
        DispatchQueue.global(qos: .utility).async { [weak self] in
            let error = Self.runConfigSave(args: ["--hyper-api-key", "-"], stdin: trimmed)
            DispatchQueue.main.async {
                self?.refreshConfigState()
                self?.refresh(force: true)
                completion(error)
            }
        }
    }

    /// Read back what the config file holds. Only *whether* a key is set — the CLI never prints
    /// the key itself.
    func refreshConfigState() {
        DispatchQueue.global(qos: .utility).async { [weak self] in
            let isSet = Self.runConfigRead()
            DispatchQueue.main.async { self?.hyperKeyIsSet = isSet }
        }
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
        refreshConfigState()
        // Seed the config file from current settings so a hand-run `agent-usage` agrees with the
        // menu bar even if the user never opens Settings. Non-destructive: see `syncConfig`.
        syncConfig(allowClear: false)
        refresh()
        timer = Timer.scheduledTimer(withTimeInterval: pollInterval, repeats: true) { [weak self] _ in
            self?.refresh()
        }
    }

    /// Refresh. `force` (a manual Refresh) bypasses the CLI's fresh-cache reuse so it always tries
    /// the live source — but the CLI still serves stale data on a transient error.
    func refresh(force: Bool = false) {
        let workDays = settings.workDays
        let dailyBudget = settings.dailyBudget
        let accounts = settings.claudeAccounts
        let hyperReset = settings.hyperResetTime
        DispatchQueue.global(qos: .utility).async { [weak self] in
            guard let self else { return }
            let result = Self.runCLI(
                decoder: self.decoder, workDays: workDays, dailyBudget: dailyBudget,
                bypassCache: force, claudeAccounts: accounts, hyperResetTime: hyperReset)
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

    /// Run the built-in agents (`agent-usage all`) plus one extra `agent-usage claude` per
    /// configured account, and return the combined snapshot array. Extra accounts are appended
    /// after the built-ins; a spawn/decode failure on the base run is fatal, but an individual
    /// account that fails to spawn/decode is skipped (its per-agent error snapshot, if any,
    /// decodes normally and is kept).
    private static func runCLI(
        decoder: JSONDecoder, workDays: Int, dailyBudget: Double, bypassCache: Bool,
        claudeAccounts: [ClaudeAccount], hyperResetTime: String
    ) -> Result<[AgentSnapshot], CLIError> {
        let base = runAll(
            decoder: decoder, workDays: workDays, dailyBudget: dailyBudget, bypassCache: bypassCache,
            hyperResetTime: hyperResetTime)
        guard case .success(var snaps) = base else { return base }

        for account in claudeAccounts {
            if let snap = runClaudeAccount(
                account, decoder: decoder, workDays: workDays, dailyBudget: dailyBudget,
                bypassCache: bypassCache) {
                snaps.append(snap)
            }
        }
        return .success(snaps)
    }

    /// Run `agent-usage config --save …`, optionally feeding `stdin` to the child. Returns an
    /// error message on failure, or nil on success.
    private static func runConfigSave(args: [String], stdin: String?) -> String? {
        let launch = resolveLaunch()
        let process = Process()
        process.executableURL = URL(fileURLWithPath: launch.executable)
        process.arguments = launch.leadingArgs + ["config", "--save", "--json"] + args

        let input = Pipe()
        let errPipe = Pipe()
        process.standardInput = input
        process.standardOutput = Pipe()
        process.standardError = errPipe

        do {
            try process.run()
        } catch {
            return "failed to launch agent-usage: \(error.localizedDescription)"
        }
        // Always close stdin — the CLI blocks reading it when passed `-`.
        if let stdin { input.fileHandleForWriting.write(Data(stdin.utf8)) }
        input.fileHandleForWriting.closeFile()

        let errData = errPipe.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        guard process.terminationStatus == 0 else {
            let message = String(data: errData, encoding: .utf8)?
                .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
            return message.isEmpty ? "agent-usage config failed" : message
        }
        return nil
    }

    /// Ask the CLI whether a Hyper key is stored. Returns false on any failure — a key we can't
    /// confirm is one the user should be told to set, not one we assume is fine.
    private static func runConfigRead() -> Bool {
        let launch = resolveLaunch()
        let process = Process()
        process.executableURL = URL(fileURLWithPath: launch.executable)
        process.arguments = launch.leadingArgs + ["config", "--json"]

        let stdout = Pipe()
        process.standardOutput = stdout
        process.standardError = Pipe()
        do {
            try process.run()
        } catch {
            return false
        }
        let data = stdout.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        guard let json = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] else {
            return false
        }
        return json["hyper_api_key_set"] as? Bool ?? false
    }

    /// Run `agent-usage all --json --work-days N --daily-budget B` and decode the array.
    private static func runAll(
        decoder: JSONDecoder, workDays: Int, dailyBudget: Double, bypassCache: Bool,
        hyperResetTime: String
    ) -> Result<[AgentSnapshot], CLIError> {
        let launch = resolveLaunch()

        let process = Process()
        process.executableURL = URL(fileURLWithPath: launch.executable)
        var args = launch.leadingArgs + [
            "all", "--json",
            "--work-days", String(workDays),
            "--daily-budget", String(format: "%.4f", dailyBudget),
        ]
        // A forced refresh skips fresh-cache reuse (still serves stale on a transient error).
        if bypassCache { args += ["--cache-ttl", "0"] }
        // Hyper's API reports no reset instant. Passing it explicitly is what makes the setting
        // work from Finder, where the app inherits no shell profile and so no HYPER_RESET_TIME.
        let reset = hyperResetTime.trimmingCharacters(in: .whitespaces)
        if !reset.isEmpty { args += ["--reset-time", reset] }
        process.arguments = args

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

    /// Run `agent-usage claude --json` for one extra account, overriding its identity and pointing
    /// it at that account's config dir (and optional Keychain account). Returns the decoded
    /// single-agent snapshot — including a per-agent error document — or nil if the process failed
    /// to spawn or its output couldn't be decoded.
    private static func runClaudeAccount(
        _ account: ClaudeAccount, decoder: JSONDecoder, workDays: Int, dailyBudget: Double,
        bypassCache: Bool
    ) -> AgentSnapshot? {
        let launch = resolveLaunch()

        let process = Process()
        process.executableURL = URL(fileURLWithPath: launch.executable)
        var args = launch.leadingArgs + [
            "claude", "--json",
            "--id", account.id,
            "--label", account.label,
            "--config-dir", account.configDir,
            // On macOS the token lives in the Keychain under a config-dir-specific service; the CLI
            // reads `<config-dir>/.credentials.json` first (Linux) and falls back to this service.
            "--keychain-service", account.resolvedKeychainService,
            "--work-days", String(workDays),
            "--daily-budget", String(format: "%.4f", dailyBudget),
        ]
        if bypassCache { args += ["--cache-ttl", "0"] }
        process.arguments = args

        let stdout = Pipe()
        process.standardOutput = stdout
        process.standardError = Pipe()

        do {
            try process.run()
        } catch {
            return nil
        }

        let data = stdout.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()

        // `claude --json` emits a single snapshot object (valid JSON even on a per-agent error).
        return try? decoder.decode(AgentSnapshot.self, from: data)
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

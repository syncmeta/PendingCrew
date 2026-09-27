#if os(macOS)
import Foundation

/// 人写给机长的选择条件；它们是建议，不代表 CLI 当前可用。
enum CaptainRunnerPreferences {
    static let claudeKey = "pendingcrew.captainPreference.claude"
    static let codexKey = "pendingcrew.captainPreference.codex"

    static func get(_ kind: LocalCodingAgentKind, defaults: UserDefaults = .standard) -> String {
        defaults.string(forKey: kind == .claudeCode ? claudeKey : codexKey) ?? ""
    }

    static func set(_ text: String, for kind: LocalCodingAgentKind,
                    defaults: UserDefaults = .standard) {
        let key = kind == .claudeCode ? claudeKey : codexKey
        if text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            defaults.removeObject(forKey: key)
        } else {
            defaults.set(text, forKey: key)
        }
    }
}

/// 两边同一口径：PATH 首命中、CLI 可执行、CLI 自身认证状态、已有会话健康。
/// unknown 永远不当作已认证；自动选 runner 时只接受 confirmed + normal。
struct CaptainRunnerCapability: Equatable, Sendable {
    enum Authentication: Equatable, Sendable { case confirmed, required, unknown }
    enum Health: Equatable, Sendable { case normal, unhealthy, unknown }

    let kind: LocalCodingAgentKind
    let executable: URL?
    let authentication: Authentication
    let health: Health
    var healthReason: String? = nil

    var selectable: Bool {
        executable != nil && authentication == .confirmed && health == .normal
    }

    var summary: String {
        guard let executable else { return "PATH 中未找到" }
        let auth: String
        switch authentication {
        case .confirmed: auth = "已登录"
        case .required: auth = "需要登录"
        case .unknown: auth = "登录状态未知"
        }
        let healthText: String
        switch health {
        case .normal: healthText = "正常"
        case .unhealthy: healthText = "异常"
        case .unknown: healthText = "健康未知"
        }
        let reason = healthReason.map { "（\($0)）" } ?? ""
        return "\(auth) · \(healthText)\(reason) · \(executable.path)"
    }
}

enum CaptainRunnerChoice {
    /// 继承原 runner 仅是默认顺序；不可用时选另一边，两边都不可用则停下。
    static func select(inherited: LocalCodingAgentKind,
                       claude: CaptainRunnerCapability,
                       codex: CaptainRunnerCapability) -> LocalCodingAgentKind? {
        let first = inherited == .codex ? codex : claude
        let second = inherited == .codex ? claude : codex
        return first.selectable ? first.kind : (second.selectable ? second.kind : nil)
    }
}

/// Session launch obtains this off the main actor before rendering either prompt format.
struct CaptainRunnerCapabilities: Sendable {
    let claude: CaptainRunnerCapability
    let codex: CaptainRunnerCapability

    static func capture() async -> Self {
        let claude = Task.detached(priority: .utility) {
            CaptainRunnerProbe.inspect(.claudeCode)
        }
        let codex = Task.detached(priority: .utility) {
            CaptainRunnerProbe.inspect(.codex)
        }
        return await Self(claude: claude.value, codex: codex.value)
    }
}

/// `claude auth status` / `codex login status` 加 `--version` 只读本机状态；不会发起登录。
/// 退出码失败在 Claude 的 JSON 明确 loggedIn=false 时才判需登录；其余保守判未知。
enum CaptainRunnerProbe {
    static func inspect(_ kind: LocalCodingAgentKind) -> CaptainRunnerCapability {
        guard let executable = LocalCodingAgentExecutable.resolve(kind) else {
            return .init(kind: kind, executable: nil, authentication: .unknown, health: .unknown)
        }
        let arguments = kind == .claudeCode ? ["auth", "status"] : ["login", "status"]
        let authResult = run(executable, arguments, kind: kind)
        let versionResult = run(executable, ["--version"], kind: kind)
        let health: CaptainRunnerCapability.Health
        if let versionResult {
            let output = String(decoding: versionResult.1, as: UTF8.self)
            health = versionResult.0 == 0 && LocalCodingAgentExecutable.cliVersion(output) != nil
                ? .normal : .unhealthy
        } else {
            health = .unhealthy
        }
        let auth: CaptainRunnerCapability.Authentication
        if let authResult {
            let output = authResult.1
            if kind == .claudeCode,
               let object = try? JSONSerialization.jsonObject(with: output) as? [String: Any],
               let loggedIn = object["loggedIn"] as? Bool {
                auth = loggedIn && authResult.0 == 0 ? .confirmed : .required
            } else if kind == .codex {
                // The short-lived CLI status can fail in an app launch environment even
                // when Codex's own app-server can read the active account. Ask that
                // native control surface before rejecting a captain handoff.
                let account = authResult.0 == 0 ? nil : readCodexAccount(executable)
                auth = codexAuthentication(cliExit: authResult.0, account: account)
            } else {
                auth = authResult.0 == 0 ? .confirmed : .unknown
            }
        } else if kind == .codex {
            auth = codexAuthentication(cliExit: nil, account: readCodexAccount(executable))
        } else {
            auth = .unknown
        }
        let observedProblem = observedHealthProblem(kind: kind, snapshot: loadRuntimeSnapshot())
        return .init(kind: kind, executable: executable, authentication: auth,
                     health: observedProblem == nil ? health : .unhealthy,
                     healthReason: observedProblem)
    }

    /// 复用 daemon 每 2 秒写的会话健康快照；旧版/过期快照不当成当前故障。
    static func observedHealthProblem(kind: LocalCodingAgentKind,
                                      snapshot: CrewSessionsSnapshot?, now: Date = Date()) -> String? {
        guard let snapshot,
              let updated = ISO8601DateFormatter().date(from: snapshot.updatedAt),
              now.timeIntervalSince(updated) >= -5,
              now.timeIntervalSince(updated) <= 30 else { return nil }
        for crewId in snapshot.crews.keys.sorted() {
            for entry in (snapshot.crews[crewId] ?? []).sorted(by: { $0.sessionId < $1.sessionId })
                where entry.runnerKind == kind.rawValue {
                if ["error", "rateLimited", "launchFailed"].contains(entry.state) {
                    return entry.healthDetail ?? "现有会话报告 \(entry.state)"
                }
            }
        }
        return nil
    }

    private static func loadRuntimeSnapshot() -> CrewSessionsSnapshot? {
        let file = LocalWhiteboardStore.defaultDirectory
            .appendingPathComponent(CrewSessionsSnapshot.fileName)
        guard let data = try? Data(contentsOf: file) else { return nil }
        return try? JSONDecoder().decode(CrewSessionsSnapshot.self, from: data)
    }

    /// `account/read` is Codex's documented auth-state query. Unknown or malformed
    /// responses remain unknown; neither CLI installation nor a healthy version
    /// response is authentication evidence.
    static func codexAuthentication(cliExit: Int32?, account: [String: Any]?)
        -> CaptainRunnerCapability.Authentication {
        if cliExit == 0 { return .confirmed }
        guard let account else { return .unknown }
        if account["account"] is NSNull {
            return account["requiresOpenaiAuth"] as? Bool == true ? .required : .unknown
        }
        guard let current = account["account"] as? [String: Any],
              let type = current["type"] as? String,
              ["chatgpt", "apiKey", "amazonBedrock"].contains(type) else { return .unknown }
        return .confirmed
    }

    /// Bounded one-shot app-server query, used only after the CLI status is
    /// inconclusive. Never read or expose tokens, email, or the account payload.
    static func readCodexAccount(_ executable: URL) -> [String: Any]? {
        let process = Process()
        process.executableURL = executable
        process.arguments = ["app-server"]
        process.environment = LocalCodingAgentEnv.build(additionalEnv: [:], kind: .codex)
        let input = Pipe(), output = Pipe()
        process.standardInput = input
        process.standardOutput = output
        process.standardError = FileHandle.nullDevice
        do { try process.run() } catch { return nil }
        let watchdog = DispatchWorkItem { if process.isRunning { process.terminate() } }
        DispatchQueue.global(qos: .utility).asyncAfter(deadline: .now() + 12, execute: watchdog)
        defer {
            watchdog.cancel()
            if process.isRunning { process.terminate() }
        }

        func send(_ object: [String: Any]) -> Bool {
            guard var line = try? JSONSerialization.data(withJSONObject: object) else { return false }
            line.append(0x0a)
            do { try input.fileHandleForWriting.write(contentsOf: line); return true }
            catch { return false }
        }
        guard send(["jsonrpc": "2.0", "id": 0, "method": "initialize",
                    "params": ["clientInfo": ["name": "PendingCrew", "title": "PendingCrew",
                                               "version": "1.0"],
                               "capabilities": ["experimentalApi": false,
                                                "requestAttestation": false]]]) else { return nil }
        var buffer = Data()
        var initialized = false
        while true {
            let chunk = output.fileHandleForReading.availableData
            if chunk.isEmpty { return nil }
            buffer.append(chunk)
            while let newline = buffer.firstIndex(of: 0x0a) {
                let line = Data(buffer[buffer.startIndex..<newline])
                buffer.removeSubrange(buffer.startIndex...newline)
                guard let message = try? JSONSerialization.jsonObject(with: line) as? [String: Any]
                else { continue }
                if message["id"] as? Int == 0, !initialized {
                    initialized = true
                    guard send(["jsonrpc": "2.0", "method": "initialized", "params": [:]]),
                          send(["jsonrpc": "2.0", "id": 1, "method": "account/read",
                                "params": ["refreshToken": false]]) else { return nil }
                } else if message["id"] as? Int == 1 {
                    return message["result"] as? [String: Any]
                }
            }
        }
    }

    private static func run(_ executable: URL, _ arguments: [String],
                            kind: LocalCodingAgentKind) -> (Int32, Data)? {
        let process = Process()
        process.executableURL = executable
        process.arguments = arguments
        process.environment = LocalCodingAgentEnv.build(additionalEnv: [:], kind: kind)
        let output = Pipe()
        process.standardOutput = output
        process.standardError = FileHandle.nullDevice
        do {
            try process.run()
        } catch {
            return nil
        }
        // CLI status is local and normally immediate. Bound a broken CLI so creation cannot hang.
        let done = DispatchSemaphore(value: 0)
        DispatchQueue.global(qos: .utility).async {
            process.waitUntilExit()
            done.signal()
        }
        guard done.wait(timeout: .now() + 5) == .success else {
            if process.isRunning { process.terminate() }
            return nil
        }
        let data = output.fileHandleForReading.readDataToEndOfFile()
        return (process.terminationStatus, data)
    }
}
#endif

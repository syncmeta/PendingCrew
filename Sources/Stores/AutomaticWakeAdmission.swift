import Foundation

/// Durable, machine-wide budget for app-originated model turns. The lock covers the
/// read, decision, reservation and write, including callers in another process.
/// Every reservation remains an attempt in the short window even if a backend
/// rejects it; acceptance is tracked separately for settlement.
final class AutomaticWakeAdmission: @unchecked Sendable {
    enum Priority: Equatable { case automatic, human, emergency }
    enum Decision {
        case allowed(String)
        case deferred(String)
        case hardStopped(String)

        var isAllowed: Bool { if case .allowed = self { return true }; return false }
        var reason: String? {
            switch self {
            case .allowed: return nil
            case let .deferred(reason), let .hardStopped(reason): return reason
            }
        }
        var recoveryHint: String {
            switch self {
            case .allowed: return ""
            case .deferred: return "请检查目标会话身份及本机 admission 账本；恢复前自动出站保持暂停。"
            case .hardStopped: return "须由人从界面明确选择对应范围的恢复操作。"
            }
        }
    }

    enum Submission<Result> {
        case denied(Decision)
        case attempted(Result, recorded: Bool)
    }

    struct Event: Codable {
        let token: String
        let sessionId: String
        let crewId: String
        let sourceKey: String
        let at: TimeInterval
        var accepted: Bool
    }

    /// Every reserved outbound attempt remains in the sliding window even if
    /// the backend rejects it. `accepted` separately records real acceptance.
    struct Attempt: Codable {
        let token: String
        let sessionId: String
        let crewId: String
        let at: TimeInterval
        var accepted: Bool
    }

    /// A whiteboard wake that reached the app but has not been acknowledged by
    /// its target. IDs only: the original text remains in the whiteboard store.
    struct PendingWhiteboard: Codable, Hashable {
        let crewId: String
        let entryId: String
        let targetId: String
    }

    enum RecoveryScope: Codable, Hashable {
        case session(String)
        case crew(String)
        case machine
    }

    /// Constructed only by the explicit UI action, locally or by its trusted
    /// viewer-to-daemon orchestration handler. It is never exposed as an agent tool.
    struct HumanRecoveryRequest {
        let token: String
        let scope: RecoveryScope

        static func directUI(scope: RecoveryScope) -> Self {
            Self(token: UUID().uuidString, scope: scope)
        }
    }

    struct RecoveryRecord: Codable {
        let token: String
        let scope: RecoveryScope
        let at: TimeInterval
        let actor: String
    }

    struct State: Codable {
        var logicalNow: TimeInterval = 0
        var lastWall: TimeInterval = 0
        var lastUptime: TimeInterval = 0
        var events: [Event] = []
        var attempts: [Attempt]? = nil
        var sourceDenials: [String: Int] = [:]
        var sourceDenialAt: [String: TimeInterval] = [:]
        var sourceBlockedUntil: [String: TimeInterval] = [:]
        // Optional so admission ledgers written by the first #170 candidate decode
        // without a migration that could accidentally reset their rate history.
        var hardStoppedSessions: Set<String>? = nil
        var hardStoppedCrews: Set<String>? = nil
        var hardStoppedMachine: Bool? = nil
        var pendingWhiteboard: Set<PendingWhiteboard>? = nil
        var recoveryRecords: [RecoveryRecord]? = nil
        /// Backend accepted, but the source lease has not yet been acknowledged.
        /// Kept separately from the rate window so a restart never resubmits it.
        var acceptedLeases: Set<String> = []
    }

    let directory: URL
    var fileURL: URL { directory.appendingPathComponent("automatic-wake-admission.json") }
    private var lockURL: URL { directory.appendingPathComponent("automatic-wake-admission.lock") }

    init(directory: URL? = nil) {
        self.directory = directory ?? LocalWhiteboardStore.defaultDirectory
    }

    static func fingerprint(_ text: String) -> String {
        let hash = text.utf8.reduce(UInt64(14_695_981_039_346_656_037)) {
            ($0 ^ UInt64($1)) &* 1_099_511_628_211
        }
        return String(hash, radix: 16)
    }

    /// Human and emergency input bypass only session/crew automation latches;
    /// every priority reserves from the same hard machine attempt budget.
    func reserve(sessionId: String, crewId: String, sourceKey: String,
                 priority: Priority = .automatic, now: Date = Date(),
                 uptime: TimeInterval = ProcessInfo.processInfo.systemUptime) -> Decision {
        return withLock { () -> Decision in
            guard var state = read() else { return .deferred("admission 账本读不出来") }
            if state.hardStoppedMachine == true { return .hardStopped("全机自动唤醒已熔断") }
            if priority == .automatic && state.hardStoppedCrews?.contains(crewId) == true {
                return .hardStopped("crew 自动唤醒已熔断")
            }
            if priority == .automatic && state.hardStoppedSessions?.contains(sessionId) == true {
                return .hardStopped("session 自动唤醒已熔断")
            }
            advanceClock(&state, wall: now.timeIntervalSince1970, uptime: uptime)
            let clock = state.logicalNow
            if state.attempts == nil {
                state.attempts = state.events.map {
                    Attempt(token: $0.token, sessionId: $0.sessionId,
                            crewId: $0.crewId, at: $0.at, accepted: $0.accepted)
                }
            }
            state.events.removeAll { clock - $0.at >= ($0.accepted ? 3600 : 120) }
            state.attempts = (state.attempts ?? []).filter { clock - $0.at < 60 }
            state.sourceDenialAt = state.sourceDenialAt.filter { clock - $0.value < 3600 }
            state.sourceDenials = state.sourceDenials.filter {
                state.sourceDenialAt[$0.key] != nil
            }
            state.sourceBlockedUntil = state.sourceBlockedUntil.filter { $0.value > clock }
            let source = sessionId + "|" + sourceKey
            let window = state.attempts ?? []
            let sameSource = state.events.contains {
                $0.sessionId == sessionId && $0.sourceKey == sourceKey && clock - $0.at < 900
            }
            let reason: String?
            if window.count >= 12 { reason = "全机滑窗已满" }
            else if priority != .automatic { reason = nil }
            else if sameSource { reason = "同一唤醒仍在冷却" }
            else if (state.sourceBlockedUntil[source] ?? 0) > clock { reason = "来源熔断中" }
            else if window.filter({ $0.sessionId == sessionId }).count >= 2 {
                reason = "session 滑窗已满"
            } else if window.filter({ $0.crewId == crewId }).count >= 5 {
                reason = "crew 滑窗已满"
            }
            else { reason = nil }
            if let reason {
                // Any exhausted window or accepted-source repetition is evidence of
                // an automatic loop. Latch the scope on disk until the explicit
                // scoped UI recovery is audited; waiting out a window is not recovery.
                if window.count >= 12 {
                    state.hardStoppedMachine = true
                } else if sameSource || (state.sourceBlockedUntil[source] ?? 0) > clock
                    || window.filter({ $0.sessionId == sessionId }).count >= 2 {
                    state.hardStoppedSessions = (state.hardStoppedSessions ?? []).union([sessionId])
                } else if window.filter({ $0.crewId == crewId }).count >= 5 {
                    state.hardStoppedCrews = (state.hardStoppedCrews ?? []).union([crewId])
                } else {
                    state.hardStoppedMachine = true
                }
                let recentCount = clock - (state.sourceDenialAt[source] ?? .infinity) < 60
                    ? (state.sourceDenials[source] ?? 0) : 0
                let count = min(8, recentCount + 1)
                state.sourceDenials[source] = count
                state.sourceDenialAt[source] = clock
                if count >= 3 {
                    state.sourceBlockedUntil[source] = clock + Double(min(900, 15 * (1 << (count - 3))))
                }
                guard write(state) else { return .deferred("admission 账本写不进去") }
                return .hardStopped(reason)
            }
            let token = UUID().uuidString
            state.events.append(Event(token: token, sessionId: sessionId, crewId: crewId,
                                      sourceKey: sourceKey, at: clock, accepted: false))
            state.attempts?.append(Attempt(token: token, sessionId: sessionId,
                                            crewId: crewId, at: clock, accepted: false))
            guard write(state) else { return .deferred("admission 账本写不进去") }
            return .allowed(token)
        } ?? .deferred("admission 锁打不开")
    }

    /// The operation is the real backend submit. A denial never invokes it;
    /// only the backend's receipt marks the reservation accepted.
    func performSend<Result>(sessionId: String, crewId: String, sourceKey: String,
                             priority: Priority = .automatic,
                             isAccepted: (Result) -> Bool,
                             operation: () async -> Result) async -> Submission<Result> {
        let decision = reserve(sessionId: sessionId, crewId: crewId,
                               sourceKey: sourceKey, priority: priority)
        guard case let .allowed(token) = decision else { return .denied(decision) }
        let result = await operation()
        return .attempted(result, recorded: finish(token: token, accepted: isAccepted(result)))
    }

    /// The closure is the actual launch body. Throwing before it completes
    /// records a failed attempt without claiming that a session was started.
    func performLaunch(sessionId: String, crewId: String, sourceKey: String,
                       priority: Priority = .automatic,
                       deniedError: (Decision) -> Error,
                       operation: () async throws -> Void) async throws -> Bool {
        let decision = reserve(sessionId: sessionId, crewId: crewId,
                               sourceKey: sourceKey, priority: priority)
        guard case let .allowed(token) = decision else { throw deniedError(decision) }
        do {
            try await operation()
            return finish(token: token, accepted: true)
        } catch {
            _ = finish(token: token, accepted: false)
            throw error
        }
    }

    /// A separate, scoped direct-UI action is the only recovery path. No normal
    /// launch, message, agent command or backend receipt clears a hard latch.
    /// The one-use token and scope are audited atomically with the state change.
    @discardableResult
    func recover(_ request: HumanRecoveryRequest, now: Date = Date(),
                 uptime: TimeInterval = ProcessInfo.processInfo.systemUptime) -> Bool {
        withLock {
            guard var state = read() else { return false }
            guard !(state.recoveryRecords ?? []).contains(where: { $0.token == request.token })
            else { return false }
            advanceClock(&state, wall: now.timeIntervalSince1970, uptime: uptime)
            let recent = (state.attempts ?? state.events.map {
                Attempt(token: $0.token, sessionId: $0.sessionId,
                        crewId: $0.crewId, at: $0.at, accepted: $0.accepted)
            }).filter { state.logicalNow - $0.at < 60 }
            switch request.scope {
            case .session(let id):
                guard state.hardStoppedSessions?.contains(id) == true else { return false }
                guard recent.filter({ $0.sessionId == id }).count < 2,
                      !state.events.contains(where: {
                          $0.sessionId == id && state.logicalNow - $0.at < 900
                      }) else {
                    _ = write(state) // persist new-boot uptime baseline without clearing latch
                    return false
                }
                state.hardStoppedSessions?.remove(id)
            case .crew(let id):
                guard state.hardStoppedCrews?.contains(id) == true else { return false }
                guard recent.filter({ $0.crewId == id }).count < 5 else {
                    _ = write(state)
                    return false
                }
                state.hardStoppedCrews?.remove(id)
            case .machine:
                guard state.hardStoppedMachine == true else { return false }
                guard recent.count < 12 else {
                    _ = write(state)
                    return false
                }
                state.hardStoppedMachine = false
            }
            state.recoveryRecords = (state.recoveryRecords ?? []) + [RecoveryRecord(
                token: request.token, scope: request.scope,
                at: now.timeIntervalSince1970, actor: "explicit UI recovery")]
            return write(state)
        } ?? false
    }

    private func advanceClock(_ state: inout State, wall: TimeInterval,
                              uptime: TimeInterval) {
        if state.logicalNow == 0 {
            state.logicalNow = wall
        } else if uptime >= state.lastUptime && uptime - state.lastUptime < 7 * 86400 {
            // Uptime ignores wall clock jumps and survives app restart on this boot.
            state.logicalNow += uptime - state.lastUptime
        }
        // A lower uptime means reboot. Wait for measured time on this boot.
        state.lastWall = wall
        state.lastUptime = uptime
    }

    func recoveryHistory() -> [RecoveryRecord]? {
        withLock {
            guard let state = read() else { return nil }
            return state.recoveryRecords ?? []
        } ?? nil
    }

    /// Record before queuing or starting a whiteboard wake. If this write fails,
    /// callers must not attempt an automatic outbound request.
    @discardableResult
    func rememberWhiteboard(_ pending: PendingWhiteboard) -> Bool {
        withLock {
            guard var state = read() else { return false }
            state.pendingWhiteboard = (state.pendingWhiteboard ?? []).union([pending])
            return write(state)
        } ?? false
    }

    /// nil is a failed read, not an empty queue. Callers must leave cursors alone.
    func pendingWhiteboard(crewId: String) -> [PendingWhiteboard]? {
        withLock {
            guard let state = read() else { return nil }
            return (state.pendingWhiteboard ?? []).filter { $0.crewId == crewId }
                .sorted { ($0.entryId, $0.targetId) < ($1.entryId, $1.targetId) }
        } ?? nil
    }

    @discardableResult
    func acknowledgeWhiteboard(_ pending: PendingWhiteboard) -> Bool {
        withLock {
            guard var state = read() else { return false }
            state.pendingWhiteboard?.remove(pending)
            return write(state)
        } ?? false
    }

    /// Only a true backend acceptance marks an event delivered. Rejection removes
    /// its pending event but leaves the attempt in the rate window.
    @discardableResult
    func finish(token: String, accepted: Bool) -> Bool {
        return withLock {
            guard var state = read(), let index = state.events.firstIndex(where: { $0.token == token })
            else { return false }
            if accepted {
                state.events[index].accepted = true
                if let attemptIndex = state.attempts?.firstIndex(where: { $0.token == token }) {
                    state.attempts?[attemptIndex].accepted = true
                }
                let source = state.events[index].sessionId + "|" + state.events[index].sourceKey
                if state.events[index].sourceKey.hasPrefix("scheduled:")
                    || state.events[index].sourceKey.hasPrefix("supervision:") {
                    state.acceptedLeases.insert(String(
                        state.events[index].sourceKey.components(separatedBy: "|target:").first
                            ?? state.events[index].sourceKey))
                }
                state.sourceDenials[source] = nil
                state.sourceDenialAt[source] = nil
                state.sourceBlockedUntil[source] = nil
            } else {
                state.events.remove(at: index)
            }
            return write(state)
        } ?? false
    }

    /// nil means the ledger cannot be trusted, so the caller must leave its
    /// original lease in place and retry the read later without sending a turn.
    func acceptedLease(sourceKey: String) -> Bool? {
        withLock { read()?.acceptedLeases.contains(sourceKey) } ?? nil
    }

    @discardableResult
    func acknowledgeLease(sourceKey: String) -> Bool {
        withLock {
            guard var state = read() else { return false }
            state.acceptedLeases.remove(sourceKey)
            return write(state)
        } ?? false
    }

    private func withLock<T>(_ body: () -> T) -> T? {
        do { try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true) }
        catch { return nil }
        let fd = open(lockURL.path, O_CREAT | O_RDWR, 0o600)
        guard fd >= 0 else { return nil }
        defer { close(fd) }
        guard flock(fd, LOCK_EX) == 0 else { return nil }
        defer { flock(fd, LOCK_UN) }
        return body()
    }

    private func read() -> State? {
        var metadata = stat()
        if lstat(fileURL.path, &metadata) != 0 {
            // `fileExists` also returns false for EACCES/EMFILE. Only a real
            // ENOENT is a new ledger; every other failure keeps the gate shut.
            return errno == ENOENT ? State() : nil
        }
        guard (metadata.st_mode & mode_t(S_IFMT)) == mode_t(S_IFREG) else { return nil }
        guard let data = try? Data(contentsOf: fileURL) else { return nil }
        return try? JSONDecoder().decode(State.self, from: data)
    }

    private func write(_ state: State) -> Bool {
        guard let data = try? JSONEncoder().encode(state) else { return false }
        do { try data.write(to: fileURL, options: .atomic); return true }
        catch { return false }
    }
}

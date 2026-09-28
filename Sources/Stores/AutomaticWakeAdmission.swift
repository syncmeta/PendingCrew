import Foundation

/// Durable, machine-wide budget for app-originated model turns. The lock covers the
/// read, decision, reservation and write, including callers in another process.
/// A reservation counts against the budget until the backend accepts or rejects it.
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
    }

    struct Event: Codable {
        let token: String
        let sessionId: String
        let crewId: String
        let sourceKey: String
        let at: TimeInterval
        var accepted: Bool
    }

    struct State: Codable {
        var logicalNow: TimeInterval = 0
        var lastWall: TimeInterval = 0
        var lastUptime: TimeInterval = 0
        var events: [Event] = []
        var sourceDenials: [String: Int] = [:]
        var sourceDenialAt: [String: TimeInterval] = [:]
        var sourceBlockedUntil: [String: TimeInterval] = [:]
        // Optional so admission ledgers written by the first #170 candidate decode
        // without a migration that could accidentally reset their rate history.
        var hardStoppedSessions: Set<String>? = nil
        var hardStoppedCrews: Set<String>? = nil
        var hardStoppedMachine: Bool? = nil
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

    /// Explicit human input and urgent events have a separate lane. They are never
    /// labelled delivered by this decision; their caller still needs backend receipt.
    func reserve(sessionId: String, crewId: String, sourceKey: String,
                 priority: Priority = .automatic, now: Date = Date(),
                 uptime: TimeInterval = ProcessInfo.processInfo.systemUptime) -> Decision {
        guard priority == .automatic else { return .allowed("priority:" + UUID().uuidString) }
        return withLock { () -> Decision in
            guard var state = read() else { return .deferred("admission 账本读不出来") }
            if state.hardStoppedMachine == true { return .hardStopped("全机自动唤醒已熔断") }
            if state.hardStoppedCrews?.contains(crewId) == true {
                return .hardStopped("crew 自动唤醒已熔断")
            }
            if state.hardStoppedSessions?.contains(sessionId) == true {
                return .hardStopped("session 自动唤醒已熔断")
            }
            let wall = now.timeIntervalSince1970
            if state.logicalNow == 0 {
                state.logicalNow = wall
            } else if uptime >= state.lastUptime && uptime - state.lastUptime < 7 * 86400 {
                // A live machine's uptime, unlike wall time, does not jump when the
                // clock is changed. It also survives an app restart on the same boot.
                state.logicalNow += uptime - state.lastUptime
            } else {
                // A lower uptime means reboot (or an invalid clock sample). The
                // wall clock may have jumped arbitrarily; wait for measured uptime
                // on this boot before replenishing the budget.
            }
            state.lastWall = wall
            state.lastUptime = uptime
            let clock = state.logicalNow
            state.events.removeAll { clock - $0.at >= ($0.accepted ? 3600 : 120) }
            state.sourceDenialAt = state.sourceDenialAt.filter { clock - $0.value < 3600 }
            state.sourceDenials = state.sourceDenials.filter {
                state.sourceDenialAt[$0.key] != nil
            }
            state.sourceBlockedUntil = state.sourceBlockedUntil.filter { $0.value > clock }
            let source = sessionId + "|" + sourceKey
            let window = state.events.filter { clock - $0.at < 60 }
            let sameSource = state.events.contains {
                $0.sessionId == sessionId && $0.sourceKey == sourceKey && clock - $0.at < 900
            }
            let reason: String?
            if sameSource { reason = "同一唤醒仍在冷却" }
            else if (state.sourceBlockedUntil[source] ?? 0) > clock { reason = "来源熔断中" }
            else if window.filter({ $0.sessionId == sessionId }).count >= 2 {
                reason = "session 滑窗已满"
            } else if window.filter({ $0.crewId == crewId }).count >= 5 {
                reason = "crew 滑窗已满"
            } else if window.count >= 12 { reason = "全机滑窗已满" }
            else { reason = nil }
            if let reason {
                // Any exhausted window or accepted-source repetition is evidence of
                // an automatic loop. Latch the *scope* on disk until a human starts
                // a session explicitly; waiting out a clock window is not recovery.
                if sameSource || (state.sourceBlockedUntil[source] ?? 0) > clock
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
            guard write(state) else { return .deferred("admission 账本写不进去") }
            return .allowed(token)
        } ?? .deferred("admission 锁打不开")
    }

    /// An explicit human start is the recovery action. Existing rate history and
    /// accepted source leases remain, so an unresolved loop trips again promptly.
    @discardableResult
    func resetHardStopsAfterHumanStart() -> Bool {
        withLock {
            guard var state = read() else { return false }
            state.hardStoppedSessions = []
            state.hardStoppedCrews = []
            state.hardStoppedMachine = false
            return write(state)
        } ?? false
    }

    /// Only a true backend acceptance makes an event durable. Rejection frees its
    /// reservation, leaving the original wake available for retry.
    @discardableResult
    func finish(token: String, accepted: Bool) -> Bool {
        if token.hasPrefix("priority:") { return true }
        return withLock {
            guard var state = read(), let index = state.events.firstIndex(where: { $0.token == token })
            else { return false }
            if accepted {
                state.events[index].accepted = true
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
        guard FileManager.default.fileExists(atPath: fileURL.path) else { return State() }
        guard let data = try? Data(contentsOf: fileURL) else { return nil }
        return try? JSONDecoder().decode(State.self, from: data)
    }

    private func write(_ state: State) -> Bool {
        guard let data = try? JSONEncoder().encode(state) else { return false }
        do { try data.write(to: fileURL, options: .atomic); return true }
        catch { return false }
    }
}

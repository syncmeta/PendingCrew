import Foundation

/// A continuation is deliberately narrower than a Todo or plan status: it is a
/// promise made by this session in this turn that there is safe, in-scope work it
/// can do immediately after the turn ends. Historical ledgers are intentionally
/// absent from this decision so an idle crew is never kept alive by stale work.
enum SessionContinuationPolicy {
    enum Outcome: String, Codable, CaseIterable {
        case continuing
        case completed
        case blocked
        case awaitingExternal
    }

    struct Input: Equatable {
        let promisedThisTurn: Bool
        let outcome: Outcome
        /// Kept in the input to pin the negative contract: old Todo/plan state is
        /// descriptive history, never a continuation trigger.
        let hasHistoricalInProgressTodo: Bool
        let alreadyConsumed: Bool
    }

    static func shouldResume(_ input: Input) -> Bool {
        input.promisedThisTurn && input.outcome == .continuing && !input.alreadyConsumed
    }
}

/// Durable, one-shot current-turn continuation leases.
///
/// The MCP helper arms a lease while the turn is running. The authoritative turn
/// completion hook seals it as ready. The runner retains it until the backend
/// accepts, with a durable acceptance receipt protecting settlement failures.
/// Task-bound captain promises can move to a successor; legacy orphans require
/// explicit review before adoption.
final class SessionContinuationStore: @unchecked Sendable {
    struct Lease: Codable, Equatable {
        enum Phase: String, Codable { case armed, ready }

        let id: String
        let crewId: String
        var sessionId: String
        let note: String
        var phase: Phase
        let createdAt: String
        var captainPromise: Bool? = nil
        var todoNumbers: [Int]? = nil
        var originalSessionId: String? = nil
        var offeredToSessionId: String? = nil
    }

    private let directory: URL

    init(directory: URL? = nil) {
        self.directory = directory ?? LocalWhiteboardStore.defaultDirectory
        try? FileManager.default.createDirectory(at: self.directory, withIntermediateDirectories: true)
    }

    /// At most one unconsumed promise per session. Repeated calls in one turn keep
    /// the first promise instead of manufacturing multiple future turns.
    /// `onWriteFailure`: called with the IO error when the promise could not be
    /// persisted (the method then returns false). Without it, "write failed" and
    /// "already armed" are the same `false` — and the receipt for the two must differ.
    @discardableResult
    func arm(crewId: String, sessionId: String, note: String,
             captainPromise: Bool = false, todoNumbers: [Int] = [], adoptingLeaseId: String? = nil,
             liveSessionIds: Set<String> = [],
             onWriteFailure: ((Error) -> Void)? = nil) -> Bool {
        let trimmed = note.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return false }
        return withLock(fallback: false) {
            guard var rows = loadLocked() else { return false }
            guard !rows.contains(where: { $0.sessionId == sessionId }) else { return false }
            if let adoptingLeaseId {
                guard captainPromise, let old = rows.firstIndex(where: {
                    $0.id == adoptingLeaseId && $0.crewId == crewId && $0.phase == .ready
                    && !liveSessionIds.contains($0.sessionId)
                }), !todoNumbers.isEmpty else { return false }
                rows.remove(at: old)
            }
            rows.append(Lease(
                id: UUID().uuidString.lowercased(), crewId: crewId, sessionId: sessionId,
                note: trimmed, phase: .armed,
                createdAt: ISO8601DateFormatter().string(from: Date()),
                captainPromise: captainPromise, todoNumbers: Array(Set(todoNumbers)).sorted()))
            if let failure = MultiProcessJSONStore.saveRowsLocked(rows, to: fileURL) {
                onWriteFailure?(failure)
                return false
            }
            return true
        }
    }

    /// Called only by authoritative turn completion. Terminal outcomes erase any
    /// arm left in this turn; only `.continuing` promotes it to runnable.
    func finishTurn(crewId: String, sessionId: String,
                    outcome: SessionContinuationPolicy.Outcome) {
        withLock(fallback: ()) {
            guard var rows = loadLocked() else { return }
            guard let index = rows.firstIndex(where: {
                $0.crewId == crewId && $0.sessionId == sessionId
            }) else { return }
            let shouldResume = SessionContinuationPolicy.shouldResume(.init(
                promisedThisTurn: true, outcome: outcome,
                hasHistoricalInProgressTodo: false, alreadyConsumed: false))
            if shouldResume {
                rows[index].phase = .ready
            } else {
                rows.remove(at: index)
            }
            MultiProcessJSONStore.saveRowsLocked(rows, to: fileURL)
        }
    }

    /// Atomic claim: removal happens in the same file lock as the read. This is
    /// intentionally at-most-once; caller must take only after it has a live run.
    func takeReady(sessionId: String) -> Lease? {
        withLock(fallback: nil) {
            guard var rows = loadLocked() else { return nil }
            guard let index = rows.firstIndex(where: {
                $0.sessionId == sessionId && $0.phase == .ready
            }) else { return nil }
            let lease = rows.remove(at: index)
            // 取走没落盘 = 这条租约还在文件里，下次会被再取一次（at-most-once 破了）。
            // 宁可这一轮不续跑，也不要续两轮。
            guard MultiProcessJSONStore.saveRowsLocked(rows, to: fileURL) == nil else { return nil }
            return lease
        }
    }

    /// Look without consuming. Automatic wake admission and backend submission may
    /// refuse a turn; the lease must survive either refusal and an app restart.
    func peekReady(sessionId: String, eligibleTodoNumbers: Set<Int>? = nil) -> Lease? {
        withLock(fallback: nil) {
            guard let rows = loadLocked() else { return nil }
            return rows.first {
                $0.sessionId == sessionId && $0.phase == .ready
                    && (($0.todoNumbers ?? []).isEmpty
                        || Set($0.todoNumbers ?? []).isSubset(of: eligibleTodoNumbers ?? []))
            }
        }
    }

    @discardableResult
    func acknowledgeReady(id: String) -> Bool {
        withLock(fallback: false) {
            guard var rows = loadLocked(), let index = rows.firstIndex(where: { $0.id == id && $0.phase == .ready })
            else { return false }
            rows.remove(at: index)
            return MultiProcessJSONStore.saveRowsLocked(rows, to: fileURL) == nil
        }
    }

    /// Rebind only a task-bound captain promise, with current-task evidence and
    /// no live source owner. Persist the binding before submitting the wake;
    /// backend rejection leaves the same lease ready across process restart.
    func readyForCaptain(crewId: String, sessionId: String, liveSessionIds: Set<String>,
                         eligibleTodoNumbers: Set<Int>) -> Lease? {
        withLock(fallback: nil) {
            guard var rows = loadLocked(), let index = rows.firstIndex(where: {
                $0.crewId == crewId && $0.phase == .ready && $0.sessionId != sessionId
                    && !liveSessionIds.contains($0.sessionId) && $0.captainPromise == true
                    && !($0.todoNumbers ?? []).isEmpty
                    && Set($0.todoNumbers ?? []).isSubset(of: eligibleTodoNumbers)
            }), !rows.contains(where: { $0.sessionId == sessionId }) else { return nil }
            rows[index].originalSessionId = rows[index].originalSessionId ?? rows[index].sessionId
            rows[index].sessionId = sessionId
            guard MultiProcessJSONStore.saveRowsLocked(rows, to: fileURL) == nil else { return nil }
            return rows[index]
        }
    }

    /// Old unbound promises are review debt, never executable instructions.
    func orphanedReady(crewId: String, sessionId: String, liveSessionIds: Set<String>) -> [Lease] {
        withLock(fallback: []) {
            guard let rows = loadLocked() else { return [] }
            return rows.filter { $0.crewId == crewId && $0.phase == .ready
                && $0.sessionId != sessionId && !liveSessionIds.contains($0.sessionId)
                && $0.offeredToSessionId != sessionId }
        }
    }

    @discardableResult
    func acknowledgeReview(ids: Set<String>, sessionId: String) -> Bool {
        withLock(fallback: false) {
            guard var rows = loadLocked() else { return false }
            for index in rows.indices where ids.contains(rows[index].id) {
                rows[index].offeredToSessionId = sessionId
            }
            return MultiProcessJSONStore.saveRowsLocked(rows, to: fileURL) == nil
        }
    }

    /// Direct human stop is cancellation, not a crash/handoff to be replayed.
    func cancel(sessionId: String) {
        withLock(fallback: ()) {
            guard var rows = loadLocked() else { return }
            rows.removeAll { $0.sessionId == sessionId }
            MultiProcessJSONStore.saveRowsLocked(rows, to: fileURL)
        }
    }

    private var fileURL: URL { directory.appendingPathComponent("session-continuations.json") }

    private func withLock<T>(fallback: T, _ body: () -> T) -> T {
        let lock = directory.appendingPathComponent("session-continuations.lock")
        let fd = open(lock.path, O_CREAT | O_RDWR | O_CLOEXEC, 0o600)
        guard fd >= 0 else { return fallback }
        defer { close(fd) }
        guard flock(fd, LOCK_EX) == 0 else { return fallback }
        defer { _ = flock(fd, LOCK_UN) }
        return body()
    }

    private func loadLocked() -> [Lease]? {
        do {
            let data = try Data(contentsOf: fileURL)
            return try JSONDecoder().decode([Lease].self, from: data)
        } catch let error as NSError where error.domain == NSCocoaErrorDomain
            && (error.code == NSFileReadNoSuchFileError || error.code == NSFileNoSuchFileError) {
            return []
        } catch { return nil }
    }
}

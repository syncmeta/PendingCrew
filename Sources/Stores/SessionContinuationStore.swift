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
/// completion hook seals it as ready. The runner atomically takes (and removes) a
/// ready lease before sending the next prompt, so repeated idle/directory events
/// cannot duplicate it and app/store recreation does not lose it.
final class SessionContinuationStore: @unchecked Sendable {
    struct Lease: Codable, Equatable {
        enum Phase: String, Codable { case armed, ready }

        let id: String
        let crewId: String
        let sessionId: String
        let note: String
        var phase: Phase
        let createdAt: String
    }

    private let directory: URL

    init(directory: URL? = nil) {
        self.directory = directory ?? LocalWhiteboardStore.defaultDirectory
        try? FileManager.default.createDirectory(at: self.directory, withIntermediateDirectories: true)
    }

    /// At most one unconsumed promise per session. Repeated calls in one turn keep
    /// the first promise instead of manufacturing multiple future turns.
    @discardableResult
    func arm(crewId: String, sessionId: String, note: String) -> Bool {
        let trimmed = note.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return false }
        return withLock {
            var rows = loadLocked()
            guard !MultiProcessJSONStore.refuseEmptyRewriteIfNonEmptyFile(rows, at: fileURL) else {
                return false
            }
            guard !rows.contains(where: { $0.sessionId == sessionId }) else { return false }
            rows.append(Lease(
                id: UUID().uuidString.lowercased(), crewId: crewId, sessionId: sessionId,
                note: trimmed, phase: .armed,
                createdAt: ISO8601DateFormatter().string(from: Date())))
            MultiProcessJSONStore.saveRowsLocked(rows, to: fileURL)
            return true
        }
    }

    /// Called only by authoritative turn completion. Terminal outcomes erase any
    /// arm left in this turn; only `.continuing` promotes it to runnable.
    func finishTurn(crewId: String, sessionId: String,
                    outcome: SessionContinuationPolicy.Outcome) {
        withLock {
            var rows = loadLocked()
            guard !MultiProcessJSONStore.refuseEmptyRewriteIfNonEmptyFile(rows, at: fileURL) else {
                return
            }
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
        withLock {
            var rows = loadLocked()
            guard !MultiProcessJSONStore.refuseEmptyRewriteIfNonEmptyFile(rows, at: fileURL) else {
                return nil
            }
            guard let index = rows.firstIndex(where: {
                $0.sessionId == sessionId && $0.phase == .ready
            }) else { return nil }
            let lease = rows.remove(at: index)
            MultiProcessJSONStore.saveRowsLocked(rows, to: fileURL)
            return lease
        }
    }

    private var fileURL: URL { directory.appendingPathComponent("session-continuations.json") }

    private func withLock<T>(_ body: () -> T) -> T {
        MultiProcessJSONStore.withFileLock(
            directory.appendingPathComponent("session-continuations.lock"), body)
    }

    private func loadLocked() -> [Lease] {
        MultiProcessJSONStore.loadRowsLocked(Lease.self, at: fileURL, onIncident: { _ in })
    }
}

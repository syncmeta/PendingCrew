import Darwin
import Foundation

/// Durable source assignment and one-shot human takeover state for worker runs.
final class WorkerAssignmentStore: @unchecked Sendable {
    static let shared = WorkerAssignmentStore()
    struct Assignment: Codable, Equatable {
        enum TerminalReason: String, Codable { case completed, userStopped, failed, hitLimit }
        let crewId: String
        let sessionId: String
        let generation: String
        let runner: String
        let brief: String
        let workingDirectory: String
        let isolation: Bool
        let model: String?
        let effort: String?
        let title: String?
        var agentSessionId: String?
        var terminalReason: TerminalReason?
        var successorSessionId: String?
        var identityUntrusted: Bool = false
        var terminalUntrusted: Bool = false
    }

    private let directory: URL
    init(directory: URL? = nil) {
        self.directory = directory ?? LocalWhiteboardStore.defaultDirectory
    }

    var fileURL: URL { directory.appendingPathComponent("worker-assignments.json") }

    enum StoreError: LocalizedError {
        case lockUnavailable
        case incompleteLedger
        case conflictingGeneration
        case missingGeneration
        case conflictingIdentity
        case conflictingTerminal

        var errorDescription: String? {
            switch self {
            case .lockUnavailable: return "worker assignment 锁不可用，拒绝操作"
            case .incompleteLedger: return "worker assignment 账本不完整，拒绝恢复或覆盖"
            case .conflictingGeneration: return "worker assignment 代际已存在且内容不同"
            case .missingGeneration: return "worker assignment 缺少指定代际的原记录"
            case .conflictingIdentity: return "worker assignment 原会话身份冲突"
            case .conflictingTerminal: return "worker assignment 终止原因冲突"
            }
        }
    }

    func recordLaunch(_ assignment: Assignment) throws {
        guard !assignment.crewId.isEmpty, !assignment.sessionId.isEmpty,
              !assignment.generation.isEmpty, !assignment.brief.isEmpty,
              !assignment.workingDirectory.isEmpty,
              assignment.runner == "claude_code" || assignment.runner == "codex",
              assignment.agentSessionId == nil, assignment.terminalReason == nil,
              assignment.successorSessionId == nil,
              !assignment.identityUntrusted, !assignment.terminalUntrusted
        else { throw StoreError.conflictingGeneration }
        try mutate { rows in
            if let existing = rows.first(where: {
                $0.crewId == assignment.crewId && $0.sessionId == assignment.sessionId
                    && $0.generation == assignment.generation
            }) {
                guard existing == assignment else { throw StoreError.conflictingGeneration }
                return false
            }
            rows.append(assignment)
            return true
        }
    }
    func recordAgentIdentity(crewId: String, sessionId: String, generation: String,
                             agentSessionId: String) throws {
        guard !agentSessionId.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        else { throw StoreError.conflictingIdentity }
        var conflict = false
        try mutate { rows in
            guard let i = rows.firstIndex(where: {
                $0.crewId == crewId && $0.sessionId == sessionId && $0.generation == generation
            }) else { throw StoreError.missingGeneration }
            if let known = rows[i].agentSessionId {
                guard known == agentSessionId else {
                    rows[i].agentSessionId = nil
                    rows[i].identityUntrusted = true
                    conflict = true
                    return true
                }
                return false
            }
            guard !rows[i].identityUntrusted else { throw StoreError.conflictingIdentity }
            rows[i].agentSessionId = agentSessionId
            return true
        }
        if conflict { throw StoreError.conflictingIdentity }
    }
    func recordTerminal(crewId: String, sessionId: String, generation: String,
                        reason: Assignment.TerminalReason) throws {
        var conflict = false
        try mutate { rows in
            guard let i = rows.firstIndex(where: {
                $0.crewId == crewId && $0.sessionId == sessionId && $0.generation == generation
            }) else { throw StoreError.missingGeneration }
            if let known = rows[i].terminalReason {
                guard known == reason else {
                    rows[i].terminalUntrusted = true
                    conflict = true
                    return true
                }
                return false
            }
            guard !rows[i].terminalUntrusted else { throw StoreError.conflictingTerminal }
            rows[i].terminalReason = reason
            return true
        }
        if conflict { throw StoreError.conflictingTerminal }
    }
    func claimManualTakeover(crewId: String, sessionId: String, generation: String,
                             agentSessionId: String, successorSessionId: String) throws -> Assignment? {
        guard !successorSessionId.isEmpty, successorSessionId != sessionId,
              !agentSessionId.isEmpty else { return nil }
        var claimed: Assignment?
        try mutate { rows in
            guard let i = rows.lastIndex(where: {
                $0.crewId == crewId && $0.sessionId == sessionId && $0.generation == generation
            }),
            // A later launch of the same local member retires this generation.
            !rows[(i + 1)...].contains(where: {
                $0.crewId == crewId && $0.sessionId == sessionId
            }),
            rows[i].agentSessionId == agentSessionId,
            !rows[i].identityUntrusted, !rows[i].terminalUntrusted,
            rows[i].successorSessionId == nil,
            rows[i].terminalReason == nil || rows[i].terminalReason == .failed
                || rows[i].terminalReason == .hitLimit
            else { return false }
            rows[i].successorSessionId = successorSessionId
            claimed = rows[i]
            return true
        }
        return claimed
    }
    func records() throws -> [Assignment] {
        guard FileManager.default.fileExists(atPath: directory.path) else { return [] }
        return try locked { try loadLocked() }
    }

    func latest(crewId: String, sessionId: String) throws -> Assignment? {
        try records().last { $0.crewId == crewId && $0.sessionId == sessionId }
    }

    private func mutate(_ body: (inout [Assignment]) throws -> Bool) throws {
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        try locked {
            var rows = try loadLocked()
            guard try body(&rows) else { return }
            try MultiProcessJSONStore.saveRowsLockedReportingFailure(rows, to: fileURL)
        }
    }

    private func loadLocked() throws -> [Assignment] {
        guard let data = try MultiProcessJSONStore.readDataIfExists(at: fileURL) else { return [] }
        guard let rows = try? JSONDecoder().decode([Assignment].self, from: data) else {
            throw StoreError.incompleteLedger
        }
        return rows
    }

    /// Unlike the generic JSON store's compatibility lock, authority writes fail closed.
    private func locked<T>(_ body: () throws -> T) throws -> T {
        let path = directory.appendingPathComponent("worker-assignments.lock").path
        let fd = Darwin.open(path, O_CREAT | O_RDWR | O_CLOEXEC, 0o600)
        guard fd >= 0 else { throw StoreError.lockUnavailable }
        defer { _ = Darwin.close(fd) }
        guard flock(fd, LOCK_EX) == 0 else { throw StoreError.lockUnavailable }
        defer { _ = flock(fd, LOCK_UN) }
        return try body()
    }
}

enum WorkerManualTakeoverError: LocalizedError {
    case viewerUnavailable
    case invalidRunner
    case sourceStillRunning
    case sourceWorkspaceUnavailable
    case ineligibleSource

    var errorDescription: String? {
        switch self {
        case .viewerUnavailable: return "须由持有 worker 编排权的后台执行人工接手"
        case .invalidRunner: return "接手 runner 必须是 Claude 或 Codex"
        case .sourceStillRunning: return "原 worker 仍在运行，不能并发接手同一派工"
        case .sourceWorkspaceUnavailable: return "原 worker 工作目录不存在，拒绝丢失原进度后接手"
        case .ineligibleSource: return "原 worker 缺少可信身份、代际或可接手的终止状态"
        }
    }
}

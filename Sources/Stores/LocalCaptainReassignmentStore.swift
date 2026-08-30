import Foundation

/// captain handoff 的授权边界。省略目标保持历史的“只操作本 crew”；显式目标只接受
/// 发起 crew 的直系子。live runner 还会复核发起 session 仍是父 crew 当前机长，避免
/// 排队期间已经失权的旧机长继续改动子 crew。
struct CaptainHandoffAuthorization {
    static func resolveTargetCrewId(
        sourceCrewId: String,
        requestedTargetCrewId: String?,
        targetParentIds: [String]
    ) throws -> String {
        guard let requestedTargetCrewId else { return sourceCrewId }
        let target = requestedTargetCrewId.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !target.isEmpty else { throw CaptainHandoffAuthorizationError.notDirectChild }
        if target == sourceCrewId { return sourceCrewId }
        guard targetParentIds.contains(sourceCrewId) else {
            throw CaptainHandoffAuthorizationError.notDirectChild
        }
        return target
    }

    static func validateLiveRequester(
        sourceCrewId: String,
        targetCrewId: String,
        requesterSessionId: String?,
        currentCaptainSessionId: String?
    ) throws {
        // MCP 门禁是入队时快照；真正切换前仍须复核 live 权限，防止排队期间已经
        // 失权的旧机长（包括刚被父 crew 救援替换的旧子机长）继续执行陈旧命令。
        guard let requesterSessionId, requesterSessionId == currentCaptainSessionId else {
            throw CaptainHandoffAuthorizationError.notCurrentCaptain
        }
    }
}

enum CaptainHandoffAuthorizationError: LocalizedError, Equatable {
    case notDirectChild
    case notCurrentCaptain

    var errorDescription: String? {
        switch self {
        case .notDirectChild:
            return "目标 crew 不是本 crew 的直系子；不能操作上级、平级或孙 crew。"
        case .notCurrentCaptain:
            return "发起 session 已不是父 crew 当前运行中的机长，无权救援子 crew。"
        }
    }
}

/// 现有成员接任时，runner 与 agent 会话号只能来自持久账本，不能从显示名猜。
/// 调用方把本 crew 的成员表与目标账本行交进来；这里把四道门集中成纯函数，供
/// human 右键、captain MCP 和单测共用同一口径。
struct CaptainHandoffCandidate: Equatable {
    let sessionId: String
    let displayName: String
    let kind: String
    let agentSessionId: String

    static func resolve(
        crewId: String,
        sessionId: String,
        members: [LocalSessionMember],
        record: LocalAgentSessionStore.Record?
    ) throws -> Self {
        guard let member = members.first(where: { $0.sessionId == sessionId }) else {
            throw CaptainHandoffValidationError.notCrewMember
        }
        guard let record,
              record.crewId == crewId,
              record.sessionId == sessionId,
              record.kind == "claude_code" || record.kind == "codex",
              !record.agentSessionId.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        else { throw CaptainHandoffValidationError.notAgentSession }
        return Self(sessionId: sessionId, displayName: member.displayName,
                    kind: record.kind, agentSessionId: record.agentSessionId)
    }
}

enum CaptainHandoffValidationError: LocalizedError, Equatable {
    case notCrewMember
    case notAgentSession

    var errorDescription: String? {
        switch self {
        case .notCrewMember:
            return "目标不是本 crew 的现有 session 成员。"
        case .notAgentSession:
            return "目标没有可接管的 Claude/Codex 会话账本，不能从显示名猜 runner。"
        }
    }
}

/// 机长交接的最小事务骨架。先停旧、再起新、最后持久化；任何后半段失败都先停掉
/// 可能已起的新机长，再恢复旧机长。`restoreOld` 同时负责把持久 kind 恢复成旧值。
/// 这样所有生产入口共享同一个 fail-closed 顺序，测试也能直接钉住“最多一个 live captain”。
struct CaptainHandoffTransaction {
    static func perform(
        stopOld: () async throws -> Void,
        startNew: () async throws -> Void,
        persistNew: () async throws -> Void,
        stopNew: () async throws -> Void,
        restoreOld: () async throws -> Void
    ) async throws {
        try await stopOld()
        var newStarted = false
        do {
            try await startNew()
            newStarted = true
            try await persistNew()
        } catch {
            do {
                // 新机长若停不掉，绝不能再拉起旧机长制造双 captain。把这也视作
                // rollbackFailed，保留当前唯一 live captain 并交给人处理。
                if newStarted { try await stopNew() }
                try await restoreOld()
            } catch let rollbackError {
                throw CaptainHandoffTransactionError.rollbackFailed(
                    original: error, rollback: rollbackError)
            }
            throw error
        }
    }
}

enum CaptainHandoffTransactionError: LocalizedError {
    case rollbackFailed(original: Error, rollback: Error)

    var errorDescription: String? {
        switch self {
        case let .rollbackFailed(original, rollback):
            return "机长交接失败（\(original.localizedDescription)），回滚也失败（\(rollback.localizedDescription)）。"
        }
    }
}

/// 一次「把现有 session 重新指定为机长」的可恢复意图。
///
/// 这不能只放在 `CrewSessionRunner.runs` 内存里：安装新版 PendingCrew 必然要重启
/// app，而 session 正是 app 的子进程。先把意图落到独立文件，新版启动后即可从
/// `LocalAgentSessionStore` 找回原 agent thread/conversation，再用机长世界观与权限
/// 重新挂起。独立文件也避免老版本 `LocalCrewStore` 随后持久化时把未知字段抹掉。
struct CaptainReassignmentRequest: Codable, Equatable, Identifiable {
    let id: String
    let crewId: String
    let sourceSessionId: String
    let sourceDisplayName: String
    let agentKind: String
    let requestedAt: String
}

enum LocalCaptainReassignmentStoreError: LocalizedError {
    case persistenceFailed

    var errorDescription: String? {
        "机长交接请求未能可靠落盘；原 session 未停止，请检查本机数据目录后重试。"
    }
}

final class LocalCaptainReassignmentStore: @unchecked Sendable {
    static let shared = LocalCaptainReassignmentStore()

    private let directory: URL

    init(directory: URL? = nil) {
        self.directory = directory ?? LocalWhiteboardStore.defaultDirectory
        try? FileManager.default.createDirectory(at: self.directory, withIntermediateDirectories: true)
    }

    /// 每个 crew 最多保留一条待执行意图；后一次明确选择覆盖前一次。
    @discardableResult
    func request(
        crewId: String,
        sourceSessionId: String,
        sourceDisplayName: String,
        agentKind: String,
        now: Date = Date()
    ) throws -> CaptainReassignmentRequest {
        let row = CaptainReassignmentRequest(
            id: UUID().uuidString.lowercased(),
            crewId: crewId,
            sourceSessionId: sourceSessionId,
            sourceDisplayName: sourceDisplayName,
            agentKind: agentKind,
            requestedAt: ISO8601DateFormatter().string(from: now))
        do {
            try withFileLock {
                var rows = try MultiProcessJSONStore.loadRowsLockedReportingFailure(
                    CaptainReassignmentRequest.self, at: fileURL, onIncident: { _ in })
                guard !MultiProcessJSONStore.refuseEmptyRewriteIfNonEmptyFile(rows, at: fileURL)
                else { throw LocalCaptainReassignmentStoreError.persistenceFailed }
                rows.removeAll { $0.crewId == crewId }
                rows.append(row)
                try MultiProcessJSONStore.saveRowsLockedReportingFailure(rows, to: fileURL)
            }
        } catch {
            throw LocalCaptainReassignmentStoreError.persistenceFailed
        }
        return row
    }

    func pending() -> [CaptainReassignmentRequest] {
        withFileLock { loadLocked() }
    }

    /// 只按 request id 删除：如果人已在执行期间改选另一条，旧执行完成不能误删新意图。
    func complete(_ requestId: String) {
        withFileLock {
            let rows = loadLocked()
            let remaining = rows.filter { $0.id != requestId }
            guard remaining.count != rows.count else { return }
            MultiProcessJSONStore.saveRowsLocked(remaining, to: fileURL)
        }
    }

    private var fileURL: URL {
        directory.appendingPathComponent("captain-reassignments.json")
    }

    private func withFileLock<T>(_ body: () throws -> T) rethrows -> T {
        try MultiProcessJSONStore.withFileLock(
            directory.appendingPathComponent("captain-reassignments.lock"), body)
    }

    private func loadLocked() -> [CaptainReassignmentRequest] {
        MultiProcessJSONStore.loadRowsLocked(
            CaptainReassignmentRequest.self, at: fileURL, onIncident: { _ in })
    }
}

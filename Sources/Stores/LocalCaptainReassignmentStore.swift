import Foundation

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

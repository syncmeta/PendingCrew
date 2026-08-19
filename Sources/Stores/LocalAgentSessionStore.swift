import Foundation

/// agent 侧会话号账本（人类 Todo #28：session 关掉再点恢复要真的接上原对话）。
///
/// 我们自己的 `localSessionId` 一直是稳定的（身份/成员登记/白板游标都挂在它上面），
/// 但 agent 那侧另有一套会话号 —— claude 的 session uuid、codex 的 threadId —— 此前
/// 从没落过盘，所以重启时无从传给 `--resume` / `thread/resume`。这个 store 就是那本账：
/// 按 `crewId + sessionId` 记一条，重启时查出来带上。
///
/// 走 `MultiProcessJSONStore` 基座三件套（flock / 逐条 lenient / corrupt 归档 fail-loud），
/// 与 `LocalWakeupStore` 等同一套写法与硬化策略。单文件跨 crew 共用（行内带 crewId）。
/// **自包含 Foundation**（编进 PendingCrewTests bundle 单测）。
final class LocalAgentSessionStore: @unchecked Sendable {
    /// 一条会话号记录。`kind` 是 runner 名 —— 写进来的是
    /// `LocalCodingAgentKind.rawValue`（`claude_code` / `codex`），不是 `claude` ——
    /// `WorkdirMigrationPlan` 按它决定「这条会话要不要搬」，所以它**不只是留痕**，
    /// 写入方别改成别的字面量。`updatedAt` = ISO8601 最近一次写入时刻。
    struct Record: Codable, Equatable {
        let crewId: String
        let sessionId: String
        var kind: String
        var agentSessionId: String
        var updatedAt: String
    }

    static let shared = LocalAgentSessionStore()

    private let directory: URL

    init(directory: URL? = nil) {
        self.directory = directory ?? LocalWhiteboardStore.defaultDirectory
        try? FileManager.default.createDirectory(at: self.directory, withIntermediateDirectories: true)
    }

    /// 记下（或更新）某个 session 的 agent 侧会话号。同 crew+session 覆盖旧值 ——
    /// 一个 localSessionId 同时只可能有一个活着的 agent 会话。
    func record(crewId: String, sessionId: String, kind: String, agentSessionId: String,
                now: Date = Date(), onIncident: (MultiProcessJSONStore.LedgerIncident) -> Void = { _ in }) {
        let trimmed = agentSessionId.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        withFileLock {
            var rows = loadLocked(onIncident: onIncident)
            guard !MultiProcessJSONStore.refuseEmptyRewriteIfNonEmptyFile(
                rows, at: fileURL) else { return }
            let stamp = ISO8601DateFormatter().string(from: now)
            if let i = rows.firstIndex(where: { $0.crewId == crewId && $0.sessionId == sessionId }) {
                rows[i].kind = kind
                rows[i].agentSessionId = trimmed
                rows[i].updatedAt = stamp
            } else {
                rows.append(Record(crewId: crewId, sessionId: sessionId, kind: kind,
                                   agentSessionId: trimmed, updatedAt: stamp))
            }
            MultiProcessJSONStore.saveRowsLocked(rows, to: fileURL)
        }
    }

    /// 查某个 session 记着的 agent 侧会话号（没有 → nil，调用方按「新开一轮」处理）。
    func agentSessionId(crewId: String, sessionId: String,
                        onIncident: (MultiProcessJSONStore.LedgerIncident) -> Void = { _ in }) -> String? {
        withFileLock {
            loadLocked(onIncident: onIncident)
                .first { $0.crewId == crewId && $0.sessionId == sessionId }?
                .agentSessionId
        }
    }

    /// 全部记录（排查/测试用）。
    func list(onIncident: (MultiProcessJSONStore.LedgerIncident) -> Void = { _ in }) -> [Record] {
        withFileLock { loadLocked(onIncident: onIncident) }
    }

    // MARK: - Persistence（基座三件套）

    private var fileURL: URL { directory.appendingPathComponent("agent-sessions.json") }

    private func withFileLock<T>(_ body: () -> T) -> T {
        MultiProcessJSONStore.withFileLock(
            directory.appendingPathComponent("agent-sessions.lock"), body)
    }

    private func loadLocked(onIncident: (MultiProcessJSONStore.LedgerIncident) -> Void) -> [Record] {
        MultiProcessJSONStore.loadRowsLocked(Record.self, at: fileURL, onIncident: onIncident)
    }
}

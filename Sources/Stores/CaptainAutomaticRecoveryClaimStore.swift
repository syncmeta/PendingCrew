import Foundation

/// 自动机长救援的持久一次性 claim。它在任何候选探测或启动之前落盘：同一 crew
/// 的自动救援只允许消费一次，连失败后的旧机长回滚又再次报错也不能形成自救环。
/// source id 留作审计；写不进去或解不完整时 fail-closed，由调用方只发人可见告警，
/// 绝不退回内存去重。
final class CaptainAutomaticRecoveryClaimStore: @unchecked Sendable {
    struct Claim: Codable, Equatable {
        let crewId: String
        let sourceSessionId: String
        let claimedAt: Date
    }

    private let directory: URL

    init(directory: URL? = nil) {
        self.directory = directory ?? LocalWhiteboardStore.defaultDirectory
    }

    var fileURL: URL { directory.appendingPathComponent("captain-auto-recovery-claims.json") }

    /// true 仅表示本调用取得了本 crew 的唯一启动资格；false 表示已有自动救援被
    /// 消费。失败不会清 claim，因此不会借重启、重复 health 回调或回滚后的新 session
    /// id 形成自动循环。
    @discardableResult
    func claim(crewId: String, sourceSessionId: String, now: Date = Date()) throws -> Bool {
        guard !crewId.isEmpty, !sourceSessionId.isEmpty else { return false }
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        return try MultiProcessJSONStore.withFileLock(
            directory.appendingPathComponent("captain-auto-recovery-claims.lock")) {
                var rows = try loadLocked()
                guard !rows.contains(where: { $0.crewId == crewId }) else { return false }
                rows.append(.init(crewId: crewId, sourceSessionId: sourceSessionId, claimedAt: now))
                try MultiProcessJSONStore.saveRowsLockedReportingFailure(rows, to: fileURL)
                return true
            }
    }

    func records() throws -> [Claim] {
        try MultiProcessJSONStore.withFileLock(
            directory.appendingPathComponent("captain-auto-recovery-claims.lock")) {
                try loadLocked()
            }
    }

    private func loadLocked() throws -> [Claim] {
        guard let data = try MultiProcessJSONStore.readDataIfExists(at: fileURL) else { return [] }
        guard let rows = try? JSONDecoder().decode([Claim].self, from: data) else {
            throw PersistenceError.incompleteRows(fileURL)
        }
        return rows
    }

    private enum PersistenceError: LocalizedError {
        case incompleteRows(URL)

        var errorDescription: String? {
            switch self {
            case .incompleteRows(let url):
                return "自动救援 claim 账本不完整，拒绝覆盖：\(url.path)"
            }
        }
    }
}

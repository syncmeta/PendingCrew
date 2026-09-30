import Foundation

/// daemon 明确承诺仍在管理的本地 session。
///
/// 这不是 `LocalAgentSessionStore` 的替身：后者只保存 resume / model 资料，旧行不能
/// 推出进程仍活着。这里的行只在 daemon 编排器真正把一个 `CrewSessionRun` 放进 roster
/// 后才创建，并以 daemon pid + 精确启动时刻圈定其所有权；终止回调会删掉它。
final class ExpectedLiveSessionStore: @unchecked Sendable {
    struct DaemonEpoch: Codable, Equatable {
        var pid: Int32
        var startedAt: Date
    }

    struct Record: Codable, Equatable {
        var crewId: String
        var sessionId: String
        var epoch: DaemonEpoch
        var generation: String
        var activatedAt: Date
        /// 只有白板写入**和**这笔确认都落盘后才置值。它不是重启许可；若两本账
        /// 之间崩溃，宁可下次重复可见提示，也不许永久丢掉提示。
        var noticeDeliveredAt: Date?
    }

    private let directory: URL

    init(directory: URL? = nil) {
        self.directory = directory ?? LocalWhiteboardStore.defaultDirectory
    }

    /// 只由 daemon 的真实 run 生命周期调用。相同 owner + crew + session 的旧行被替换，
    /// 防止同一 session 再起后把前一代错误地当成仍 live。
    func activate(crewId: String, sessionId: String, epoch: DaemonEpoch,
                  now: Date = Date()) throws {
        guard !crewId.isEmpty, !sessionId.isEmpty else { return }
        try ensureDirectory()
        try withFileLock {
            var rows = try loadLocked()
            try rejectUnsafeEmptyRewrite(rows)
            rows.removeAll {
                $0.crewId == crewId && $0.sessionId == sessionId && $0.epoch == epoch
            }
            rows.append(.init(crewId: crewId, sessionId: sessionId, epoch: epoch,
                              generation: UUID().uuidString, activatedAt: now,
                              noticeDeliveredAt: nil))
            try saveLocked(rows)
        }
    }

    /// terminal event 和显式 stop 都经 `CrewSessionRun.onEnded` 落到这里。epoch 不同
    /// 的旧行绝不能由新 daemon 代删，否则会掩盖一次尚未核对的缺席。
    func clear(crewId: String, sessionId: String, epoch: DaemonEpoch) throws {
        try withFileLock {
            var rows = try loadLocked()
            try rejectUnsafeEmptyRewrite(rows)
            let before = rows.count
            rows.removeAll {
                $0.crewId == crewId && $0.sessionId == sessionId && $0.epoch == epoch
            }
            guard rows.count != before else { return }
            try saveLocked(rows)
        }
    }

    func records() throws -> [Record] {
        try withFileLock { try loadLocked() }
    }

    func activeRecord(crewId: String, sessionId: String, epoch: DaemonEpoch) throws -> Record? {
        try withFileLock {
            try loadLocked().first {
                $0.crewId == crewId && $0.sessionId == sessionId && $0.epoch == epoch
            }
        }
    }

    /// 取一个尚未**确认投递**的记录。这里故意不写盘：先把「已告警」落盘再去写白板，
    /// 白板拒写时就会永远丢掉提示。daemon 锁保证同一时刻只有一个编排器；同次启动的
    /// 去重留在 host 内存，跨启动只以 `markNoticeDelivered` 为准。
    func nextUndeliveredNotice(crewId: String, sessionId: String, epoch: DaemonEpoch) throws -> Record? {
        try withFileLock {
            try loadLocked().first {
                $0.crewId == crewId && $0.sessionId == sessionId && $0.epoch == epoch
                    && $0.noticeDeliveredAt == nil
            }
        }
    }

    /// 白板 append 已成功后才确认。若这步写失败，下一次受控 daemon 启动会再提示一次；
    /// 这是跨两个独立文件无法实现 exactly-once 时的安全方向（宁可可见重复，不可静默丢）。
    @discardableResult
    func markNoticeDelivered(_ record: Record, now: Date = Date()) throws -> Bool {
        try withFileLock {
            var rows = try loadLocked()
            try rejectUnsafeEmptyRewrite(rows)
            guard let index = rows.firstIndex(where: {
                $0.crewId == record.crewId && $0.sessionId == record.sessionId
                    && $0.epoch == record.epoch && $0.generation == record.generation
            }), rows[index].noticeDeliveredAt == nil else { return false }
            rows[index].noticeDeliveredAt = now
            try saveLocked(rows)
            return true
        }
    }

    private var fileURL: URL { directory.appendingPathComponent("expected-live-sessions.json") }

    private enum PersistenceError: LocalizedError {
        case unsafeEmptyRewrite(URL)
        case incompleteRows(URL)

        var errorDescription: String? {
            switch self {
            case .unsafeEmptyRewrite(let url):
                return "expected-live 账本读数不安全，拒绝覆盖：\(url.path)"
            case .incompleteRows(let url):
                return "expected-live 账本含未完整解码的行，保留原件并等待恢复：\(url.path)"
            }
        }
    }

    private func ensureDirectory() throws {
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    }

    private func withFileLock<T>(_ body: () throws -> T) throws -> T {
        try MultiProcessJSONStore.withFileLock(
            directory.appendingPathComponent("expected-live-sessions.lock"), body)
    }

    private func loadLocked() throws -> [Record] {
        guard let data = try MultiProcessJSONStore.readDataIfExists(at: fileURL) else {
            return []
        }
        // expected-live 之后会整表写回，不能使用通用 store 的逐行宽容解码：只要
        // 有一行不完整，幸存行回写就会永久抹掉未通知的旧 epoch。这里 fail-closed，
        // 不归档、不重写；外部恢复完整原件后，下一次受控核对自然可再读并提示。
        guard let rows = try? JSONDecoder().decode([Record].self, from: data) else {
            throw PersistenceError.incompleteRows(fileURL)
        }
        return rows
    }

    private func saveLocked(_ rows: [Record]) throws {
        try MultiProcessJSONStore.saveRowsLockedReportingFailure(rows, to: fileURL)
    }

    private func rejectUnsafeEmptyRewrite(_ rows: [Record]) throws {
        guard !MultiProcessJSONStore.refuseEmptyRewriteIfNonEmptyFile(rows, at: fileURL) else {
            throw PersistenceError.unsafeEmptyRewrite(fileURL)
        }
    }
}

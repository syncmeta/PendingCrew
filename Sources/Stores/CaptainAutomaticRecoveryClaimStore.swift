import Darwin
import Foundation

private func captainAutomaticRecoveryFlock(_ fd: Int32, _ operation: Int32) -> Int32 {
    flock(fd, operation)
}

/// `CaptainAutomaticRecoveryClaimStore` 的锁必须 fail-closed。通用 JSON helper 为了
/// 兼容非关键账本而允许退化无锁；自动救援 claim 不能继承那个语义。
struct CaptainAutomaticRecoveryClaimLockOperations {
    struct Result {
        let value: Int32
        let errorNumber: Int32
    }

    let open: (String) -> Result
    let flock: (Int32, Int32) -> Result
    let close: (Int32) -> Void

    static let live = Self(
        open: { path in
            let fd = Darwin.open(path, O_CREAT | O_RDWR | O_CLOEXEC, 0o600)
            return .init(value: fd, errorNumber: fd < 0 ? errno : 0)
        },
        flock: { fd, operation in
            let result = captainAutomaticRecoveryFlock(fd, operation)
            return .init(value: result, errorNumber: result == 0 ? 0 : errno)
        },
        close: { fd in _ = Darwin.close(fd) })
}

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
    private let lockOperations: CaptainAutomaticRecoveryClaimLockOperations

    init(directory: URL? = nil,
         lockOperations: CaptainAutomaticRecoveryClaimLockOperations = .live) {
        self.directory = directory ?? LocalWhiteboardStore.defaultDirectory
        self.lockOperations = lockOperations
    }

    var fileURL: URL { directory.appendingPathComponent("captain-auto-recovery-claims.json") }

    /// true 仅表示本调用取得了本 crew 的唯一启动资格；false 表示已有自动救援被
    /// 消费。失败不会清 claim，因此不会借重启、重复 health 回调或回滚后的新 session
    /// id 形成自动循环。
    @discardableResult
    func claim(crewId: String, sourceSessionId: String, now: Date = Date()) throws -> Bool {
        guard !crewId.isEmpty, !sourceSessionId.isEmpty else { return false }
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        return try withExclusiveLock {
                var rows = try loadLocked()
                guard !rows.contains(where: { $0.crewId == crewId }) else { return false }
                rows.append(.init(crewId: crewId, sourceSessionId: sourceSessionId, claimedAt: now))
                try MultiProcessJSONStore.saveRowsLockedReportingFailure(rows, to: fileURL)
                return true
        }
    }

    func records() throws -> [Claim] {
        guard FileManager.default.fileExists(atPath: directory.path) else { return [] }
        return try withExclusiveLock { try loadLocked() }
    }

    /// 这个适配器只属于自动救援 claim，绝不改变通用 helper 的既有调用者。任何一层
    /// 拿不到锁，都不能读取、写入或宣称拿到了单次救援资格。
    private func withExclusiveLock<T>(_ body: () throws -> T) throws -> T {
        let lockURL = directory.appendingPathComponent("captain-auto-recovery-claims.lock")
        let opened = lockOperations.open(lockURL.path)
        guard opened.value >= 0 else {
            throw PersistenceError.lockOpen(lockURL, opened.errorNumber)
        }
        defer { lockOperations.close(opened.value) }
        let locked = lockOperations.flock(opened.value, LOCK_EX)
        guard locked.value == 0 else {
            throw PersistenceError.lockAcquire(lockURL, locked.errorNumber)
        }
        defer { _ = lockOperations.flock(opened.value, LOCK_UN) }
        return try body()
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
        case lockOpen(URL, Int32)
        case lockAcquire(URL, Int32)

        var errorDescription: String? {
            switch self {
            case .incompleteRows(let url):
                return "自动救援 claim 账本不完整，拒绝覆盖：\(url.path)"
            case .lockOpen(let url, let errorNumber):
                return "自动救援 claim 锁打不开，拒绝救援：\(url.path)（errno \(errorNumber)）"
            case .lockAcquire(let url, let errorNumber):
                return "自动救援 claim 锁拿不到，拒绝救援：\(url.path)（errno \(errorNumber)）"
            }
        }
    }
}

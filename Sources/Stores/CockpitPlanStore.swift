import Foundation
import Combine

/// 机长作战板的存储（人类 Todo #66）—— 每 crew 一本：`<dir>/<crewId>.plan.json`。
///
/// **谁能写**：只有机长（MCP 侧 `plan_add` / `plan_update` 前面站着 `guard isCaptain`）。
/// 人类和 worker 都写不进来 —— 这是本账与人类 Todo 那两本**方向上的第三种**：
/// `.agent` 那本人类写、`.human` 那本 agent 写请人拍板、本账**机长写给人看**。
///
/// **没有 fork 存储**：flock 互斥 / 逐条 lenient 解码 / corrupt 归档 / 「读失败 ≠ 内容
/// 损坏」四件套全部来自 `MultiProcessJSONStore` 基座——与白板、两本 Todo 用的是同一层。
/// 没有共用的是**账本形状**（四档 + 卡住引用，见 `CockpitPlan`），那本来就不该共用。
///
/// **为什么也要跨进程锁**：写入口在 helper 子进程（MCP 工具），读在 app（驾驶舱面板），
/// 与 Todo 那本同构。锁文件 `<crewId>.plan.lock`，与另外几本各自一把——一本忙不该挡住另一本。
///
/// 自包含 Foundation（编进 app / re-exec helper / PendingCrewTests bundle）。
/// `@unchecked Sendable`：实例状态全 `let`，共享可变资源是磁盘文件，互斥归基座。
final class CockpitPlanStore: @unchecked Sendable {
    static let shared = CockpitPlanStore()

    private let directory: URL

    /// 进程内变更信号：本进程每次 save 后发一个 `crewId`。跨进程写（helper 的 MCP 工具）
    /// 由 `planChanges` 合流的目录监听补齐 —— 与 Todo 那本同一套，无轮询。
    let changes = PassthroughSubject<String, Never>()

    init(directory: URL? = nil) {
        self.directory = directory ?? LocalWhiteboardStore.defaultDirectory
        try? FileManager.default.createDirectory(at: self.directory, withIntermediateDirectories: true)
    }

    // MARK: - 变更流

    func planChanges(crewId: String) -> AsyncStream<Void> {
        LocalWhiteboardStore.shared.startWatching()
        let inProcess = changes
        let crossProcess = LocalWhiteboardStore.shared.directoryChanged
        return AsyncStream { continuation in
            let c1 = inProcess.filter { $0 == crewId }.sink { _ in continuation.yield(()) }
            let c2 = crossProcess.sink { _ in continuation.yield(()) }
            continuation.onTermination = { _ in c1.cancel(); c2.cancel() }
        }
    }

    // MARK: - 读

    /// 活着的条目（墓碑行不出现在任何视图 / 写入路径里）。
    func list(crewId: String) -> [CockpitPlanItem] {
        withFileLock(crewId) { loadLocked(crewId).filter { !$0.isDeleted } }
    }

    func item(crewId: String, number: Int) -> CockpitPlanItem? {
        list(crewId: crewId).first { $0.number == number }
    }

    // MARK: - 写（全部只给机长）

    /// 新排一条活。**nil = 没写进去**（列表读不出来 / 读到空但磁盘非空）——
    /// 与 Todo 那本同一条纪律：绝不返回一个根本没落盘的 #N 让调用方拿去对外宣布。
    @discardableResult
    func add(crewId: String, title: String,
             bySessionId: String? = nil, byName: String? = nil) -> CockpitPlanItem? {
        let trimmed = title.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return nil }
        return withFileLock(crewId) {
            var rows = loadLocked(crewId)
            guard !refuseUnsafeEmptyRewrite(crewId: crewId, rows: rows) else { return nil }
            let now = Self.timestamp()
            let item = CockpitPlanItem(
                id: UUID().uuidString.lowercased(),
                number: (rows.map(\.number).max() ?? 0) + 1,
                title: trimmed,
                status: CockpitPlanStatus.notStarted.rawValue,
                createdAt: now,
                createdBySessionId: bySessionId,
                createdByName: byName,
                updatedAt: now)
            rows.append(item)
            saveLocked(crewId: crewId, rows: rows)
            return item
        }
    }

    /// 推进一条：追加进度描述 / 翻状态 / 改标题，**三件可以一次做完**（机长通常同时发生）。
    ///
    /// - 守卫在**锁内**过 `CockpitPlan.validate`：`blocked` 必须有引用，离开 `blocked`
    ///   时引用跟着清。守卫与写在同一把锁里，中间没有别的进程能把状态挪走。
    /// - 拒绝时返回 `.failure(reason)`，**不写盘**——调用方（MCP）把这句原样回给机长。
    /// - `updatedAt` 只有真改动才动：三样都没给 = 什么也没发生，不该把照妖镜擦亮。
    @discardableResult
    func update(crewId: String, number: Int,
                progress: String? = nil,
                statusRaw: String? = nil,
                blocker: CockpitPlanBlocker? = nil,
                title: String? = nil,
                bySessionId: String? = nil,
                byName: String? = nil) -> Result<CockpitPlanItem, UpdateFailure> {
        withFileLock(crewId) {
            var rows = loadLocked(crewId)
            guard !refuseUnsafeEmptyRewrite(crewId: crewId, rows: rows) else {
                return .failure(.ledgerUnavailable)
            }
            guard let idx = rows.firstIndex(where: { $0.number == number && !$0.isDeleted }) else {
                return .failure(.notFound)
            }
            let resolved: CockpitPlanBlocker?
            switch CockpitPlan.validate(nextRaw: statusRaw,
                                        incomingBlocker: blocker,
                                        existingBlocker: rows[idx].blockedBy) {
            case let .success(b): resolved = b
            case let .failure(refusal): return .failure(.refused(refusal))
            }

            let progressText = progress?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
            let newTitle = title?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
            let statusChanged = (statusRaw?.isEmpty == false)
            guard !progressText.isEmpty || !newTitle.isEmpty || statusChanged else {
                return .failure(.nothingToDo)
            }

            let now = Self.timestamp()
            if !progressText.isEmpty {
                rows[idx].updates.append(CockpitPlanUpdate(
                    id: UUID().uuidString.lowercased(),
                    text: progressText,
                    status: statusChanged ? CockpitPlan.status(statusRaw ?? "")?.rawValue : nil,
                    createdAt: now,
                    bySessionId: bySessionId,
                    byName: byName))
            }
            if !newTitle.isEmpty { rows[idx].title = newTitle }
            if statusChanged, let s = CockpitPlan.status(statusRaw ?? "") { rows[idx].status = s.rawValue }
            rows[idx].blockedBy = resolved
            rows[idx].updatedAt = now
            saveLocked(crewId: crewId, rows: rows)
            return .success(rows[idx])
        }
    }

    /// 撤下一条（软删，保号）。机长「整理」的一部分——排错了的活留在板上只会脏板面。
    @discardableResult
    func drop(crewId: String, number: Int) -> Bool {
        withFileLock(crewId) {
            var rows = loadLocked(crewId)
            guard !refuseUnsafeEmptyRewrite(crewId: crewId, rows: rows) else { return false }
            guard let idx = rows.firstIndex(where: { $0.number == number && !$0.isDeleted }) else { return false }
            let now = Self.timestamp()
            rows[idx].deletedAt = now
            rows[idx].updatedAt = now
            saveLocked(crewId: crewId, rows: rows)
            return true
        }
    }

    /// 写不成的三种原因 —— **都要能原样说给机长听**，不许含糊成一句「失败」。
    enum UpdateFailure: Error, Equatable {
        /// 找不到这条 #N（或已撤下）。
        case notFound
        /// 账本这次读不出来/坏了，本次写已拒（基座已如实报到白板，这里不重复报）。
        case ledgerUnavailable
        /// 三样都没给：这次调用什么也没要求。
        case nothingToDo
        /// 过不了 `CockpitPlan` 的守卫。
        case refused(CockpitPlan.Refusal)

        var summary: String {
            switch self {
            case .notFound: return "找不到这条计划（可能已撤下）"
            case .ledgerUnavailable: return "任务列表这次读不出来，本次写已拒——白板上有一条如实的警示"
            case .nothingToDo: return "这次调用没给进度、没给状态、也没给标题，什么都没改"
            case let .refused(r): return r.summary
            }
        }
    }

    // MARK: - 持久化（基座四件套：flock / 逐条 lenient / corrupt 归档 / 读失败不动原件）

    private func fileURL(_ crewId: String) -> URL {
        directory.appendingPathComponent("\(crewId).plan.json")
    }

    private func withFileLock<T>(_ crewId: String, _ body: () -> T) -> T {
        MultiProcessJSONStore.withFileLock(
            directory.appendingPathComponent("\(crewId).plan.lock"), body)
    }

    private func loadLocked(_ crewId: String) -> [CockpitPlanItem] {
        MultiProcessJSONStore.loadRowsLocked(
            CockpitPlanItem.self, at: fileURL(crewId),
            onIncident: { self.reportIncident(crewId: crewId, $0) })
    }

    /// 事故如实落白板。主语是「机长任务列表」—— 一条「读不出来」不指名是哪本账的话，
    /// 群里没人知道该去翻哪个文件（2026-08-12 那晚的教训之一）。
    private func reportIncident(crewId: String, _ incident: MultiProcessJSONStore.LedgerIncident) {
        LocalWhiteboardStore(directory: directory).appendSessionMessage(
            crewId: crewId, sessionId: "system",
            text: "机长任务列表：" + incident.summary,
            senderName: "系统")
    }

    private func saveLocked(crewId: String, rows: [CockpitPlanItem]) {
        MultiProcessJSONStore.saveRowsLocked(rows, to: fileURL(crewId))
        changes.send(crewId)
    }

    /// 读-改-写路径开头的那道闸：读不出来时 `loadLocked` 返回空表，光靠「找不到 #N」
    /// 回执会说成「这条不存在」，听起来像机长自己撤过——其实是账读不出来。
    private func refuseUnsafeEmptyRewrite(crewId: String, rows: [CockpitPlanItem]) -> Bool {
        MultiProcessJSONStore.refuseEmptyRewriteIfNonEmptyFile(rows, at: fileURL(crewId))
    }

    private static func timestamp() -> String {
        ISO8601DateFormatter().string(from: Date())
    }
}

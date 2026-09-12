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

    /// 本 crew 计划文件的变更指纹（mtime+size）。文件不存在 → nil，也是一种状态。
    /// **别在主线程调**：一次 stat 比整份解码便宜几个数量级，但它仍是磁盘 IO。
    func fingerprint(crewId: String) -> FileChangeGate.Fingerprint? {
        FileChangeGate.fingerprint(of: fileURL(crewId))
    }

    /// 计划变更流。两个上游合流成 `Void` tick：
    /// 1. `changes` 按 crewId 过滤 —— **本进程**写（app 内）即推。
    /// 2. `directoryChanged` —— **跨进程**目录监听（写入口在 helper 子进程的 MCP 工具）。
    ///
    /// 第 2 条要自己做相关性判定，这是人类 Todo #96 的一半：目录事件**不带文件名**，
    /// 它的语义只是「白板目录里有东西动过」。而那个目录是全机共用的 —— 本机 42 个
    /// crew 的群聊 json、两本 Todo、审批、唤醒、回合 marker 全在里面（实测 2000+ 个
    /// 文件）。此前这里不加过滤直接 yield，于是**别的 crew 发一条群消息，驾驶舱就在
    /// 主线程上重读一遍自己的计划文件**。指纹门控只放行「这个 crew 的 `.plan.json`
    /// 真的变了」那一次。
    ///
    /// 判定只做在**这条流**上：`directoryChanged` 本身语义不变，todo / approvals /
    /// 改名通道各自关心别的文件，不能一刀切。这套闸不是这里发明的 —— 群聊中栏那条流
    /// （`PendingCrewBackend.whiteboardChanges`）早就用同一个 `FileChangeGateBox`
    /// 治过同一个病，**照着接，别写第三套**。
    func planChanges(crewId: String) -> AsyncStream<Void> {
        LocalWhiteboardStore.shared.startWatching()
        let inProcess = changes
        let crossProcess = LocalWhiteboardStore.shared.directoryChanged
        return AsyncStream { continuation in
            // 种子取建流那一刻的指纹 —— 调用方建流前已经先全量读过一次，从当下起步，
            // 不会因为第一个无关事件白刷一遍。
            let gate = FileChangeGateBox(seed: fingerprint(crewId: crewId))
            let c1 = inProcess.filter { $0 == crewId }.sink { [weak self] _ in
                // 本进程自己的写：无条件推，同时把指纹记下 —— 同一次写盘随后还会
                // 触发一个目录事件，不吞掉就成了双份刷新。
                gate.sync(self?.fingerprint(crewId: crewId))
                continuation.yield(())
            }
            let c2 = crossProcess.sink { [weak self] _ in
                guard let self else { return }
                guard gate.shouldYield(self.fingerprint(crewId: crewId)) else { return }
                continuation.yield(())
            }
            continuation.onTermination = { _ in c1.cancel(); c2.cancel() }
        }
    }

    // MARK: - 读

    /// 活着的条目（墓碑行不出现在任何视图 / 写入路径里）。
    ///
    /// ⚠️ **这是阻塞调用**：里面是 `flock(LOCK_EX)`（阻塞式跨进程锁）+ 整份 JSON 解码。
    /// 只要有 helper 正在写这个 crew 的 `.plan.lock`，调用方就停在这儿等。
    /// UI 路径一律走 `listOffMain`（人类 Todo #96：驾驶舱打开时卡的就是这一下）。
    func list(crewId: String) -> [CockpitPlanItem] {
        // 补发也挂在读路径上：只在「下一次写成功」时补，意味着一块板只要之后没人再排
        // 新条目，存下来的就永远补不回来、也永远看不见。**这条不是推论** ——
        // Todo 那本先犯过一次，这一本的回归测试在接线时又当场抓了一次。
        recoverSpooledIfAny(crewId: crewId)
        return withFileLock(crewId) { loadLocked(crewId).filter { !$0.isDeleted } }
    }

    /// 这一本账这次读到了什么 —— **把「读不出来」交还给调用方，不压成空表。**
    ///
    /// 同一个病在本仓已经被点名修过两次（`CrewSessionRunner` 里那两处都写着
    /// 「必须走 `read`，不能走 `list`：后者把读失败压成空表」）。
    /// **`plan_list` 是同一条链上漏掉的第三处** —— 2026-09-12 那次断线里机长调它，
    /// 拿到「任务列表是空的」，而盘上那本账 186 KB、92 条。
    /// 「一条都没有」会让机长把排过的活再排一遍。
    ///
    /// 跟 `list` 是**同一条读**（同一把锁、同一个解码、同一份事故上报），
    /// 没有新造第三种读法。
    enum LedgerRead: Equatable {
        case rows([CockpitPlanItem])
        /// 这次没读到可信内容。**任何一种事故都算**（打不开 / 读到空但文件非空 / 解不开）。
        ///
        /// 跟另外两本账的 `LedgerRead` 唯一的区别：**这里把事故本身带出来**。
        /// 因为 `plan_list` 要把它讲给人听 —— 只说「读不出来」而不说是哪份文件、
        /// 底层是什么错，人只知道失败、不知道往哪查（`McpDirectoryIOFailureTests`
        /// 立的就是这个口径）。
        case unreadable(MultiProcessJSONStore.LedgerIncident)

        static func == (lhs: LedgerRead, rhs: LedgerRead) -> Bool {
            switch (lhs, rhs) {
            case let (.rows(a), .rows(b)): return a == b
            // 事故对象里装着 `Error`，没有值语义可比；两边都是事故即视为相等 ——
            // 调用方要的就是「可不可信」这一位。
            case (.unreadable, .unreadable): return true
            default: return false
            }
        }
    }

    func read(crewId: String) -> LedgerRead {
        withFileLock(crewId) {
            var incident: MultiProcessJSONStore.LedgerIncident?
            let rows = MultiProcessJSONStore.loadRowsLocked(
                CockpitPlanItem.self, at: fileURL(crewId),
                onIncident: { got in
                    incident = got
                    self.reportIncident(crewId: crewId, got)
                })
            if let incident { return .unreadable(incident) }
            return .rows(rows.filter { !$0.isDeleted })
        }
    }

    /// `list` 的后台版 —— **UI 唯一该用的那条**（人类 Todo #96）。
    ///
    /// 驾驶舱正文的 `.task` 继承 MainActor，此前直接调同步的 `list`，把那把阻塞锁和
    /// 整份解码压在主线程上；而 `planChanges` 每来一个 tick 还要再来一遍。挪到
    /// detached 任务里之后，主线程只剩最后那一次赋值。
    ///
    /// 调用方仍要自己处理**取消与迟到**（切 crew 时旧的那次读会后回来）——
    /// 见 `CockpitPlanFeed`。
    func listOffMain(crewId: String) async -> [CockpitPlanItem] {
        await Task.detached(priority: .userInitiated) { [self] in
            list(crewId: crewId)
        }.value
    }

    func item(crewId: String, number: Int) -> CockpitPlanItem? {
        list(crewId: crewId).first { $0.number == number }
    }

    // MARK: - 写
    //
    // ⚠️ 曾经写着「全部只给机长」。**现在不是了**（人类 Todo #115 的 (D)）：
    // 机长能做四样，worker 能追加进展 / 标卡住 —— 门禁在
    // `CrewCockpitWritePermission`，不在这一层。这里照旧不判身份。

    /// 新排一条活。**nil = 没写进去**（列表读不出来 / 读到空但磁盘非空）——
    /// 与 Todo 那本同一条纪律：绝不返回一个根本没落盘的 #N 让调用方拿去对外宣布。
    @discardableResult
    func add(crewId: String, title: String,
             bySessionId: String? = nil, byName: String? = nil) -> CockpitPlanItem? {
        let trimmed = title.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return nil }
        recoverSpooledIfAny(crewId: crewId)
        return withFileLock(crewId) {
            var rows = loadLocked(crewId)
            guard !refuseUnsafeEmptyRewrite(crewId: crewId, rows: rows) else {
                // 这本账这一刻读不出来 —— 以前到这儿就 `return nil`，那条标题就没了。
                // 三本账同一个病、同一个办法（`LedgerSpool`）：存的是**那次请求**，
                // `#N` 留到补发那一刻现算。
                spool.spool(
                    PendingPlanRequest(title: trimmed, bySessionId: bySessionId, byName: byName),
                    key: crewId)
                return nil
            }
            drainRequests(crewId: crewId, rows: &rows)
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
            guard saveLocked(crewId: crewId, rows: rows) == nil else { return nil }
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
            if let failure = saveLocked(crewId: crewId, rows: rows) {
                return .failure(.notWritten(failure.localizedDescription))
            }
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
            guard saveLocked(crewId: crewId, rows: rows) == nil else { return false }
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
        /// 读到了、也改好了，**但这一笔没落到磁盘上**。板上还是旧的。
        case notWritten(String)

        var summary: String {
            switch self {
            case .notFound: return "找不到这条计划（可能已撤下）"
            case .ledgerUnavailable: return "任务列表这次读不出来，本次写已拒——白板上有一条如实的警示"
            case .nothingToDo: return "这次调用没给进度、没给状态、也没给标题，什么都没改"
            case let .refused(r): return r.summary
            case let .notWritten(why): return "改动\(WriteReceipt.notWrittenMarker)（\(why)）——板上还是原来那样"
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
    /// **它经常写不进去，而那不是 bug**（2026-09-12 补的话）：append 要先把整份白板
    /// 读一遍，读不了就整条拒写。整个数据目录读不出来的那种事故里，账本和白板是
    /// 一起瞎的，所以这条警示**必然落不了盘** —— 写不成就退到系统日志，别让一条
    /// 专门用来留痕的东西自己静默失败。三本账（Todo / 机长任务列表 / codex 审批）
    /// 用同一个形状，别只改一处。
    private func reportIncident(crewId: String, _ incident: MultiProcessJSONStore.LedgerIncident) {
        let text = "机长任务列表：" + incident.summary
        do {
            _ = try LocalWhiteboardStore(directory: directory)
                .appendSessionMessageReportingFailure(
                    crewId: crewId, sessionId: "system", text: text, senderName: "系统")
        } catch {
            NSLog("[PendingCrew] 账本事故没能写进白板（白板多半也读不出来）：%@ / %@",
                  text, error.localizedDescription)
        }
    }

    /// **返回 nil = 真的落到磁盘上了**（与两本 Todo 同一口径）。
    private func saveLocked(crewId: String, rows: [CockpitPlanItem]) -> Error? {
        let failure = MultiProcessJSONStore.saveRowsLocked(rows, to: fileURL(crewId))
        guard failure == nil else { return failure }
        changes.send(crewId)
        return nil
    }

    /// 读-改-写路径开头的那道闸：读不出来时 `loadLocked` 返回空表，光靠「找不到 #N」
    /// 回执会说成「这条不存在」，听起来像机长自己撤过——其实是账读不出来。
    // MARK: - 账读不动时把请求先存着（2026-09-12）

    private var spool: LedgerSpool<PendingPlanRequest> {
        LedgerSpool(directory: fileURL("x").deletingLastPathComponent()
            .appendingPathComponent("outbox-plans", isDirectory: true))
    }

    /// 有积压就补一次。**补发不能只挂在写路径上**：一块板只要之后没人再排新条目，
    /// 存下来的就永远补不回来、也永远看不见（`list` 里没有它）。
    private func recoverSpooledIfAny(crewId: String) {
        guard spool.hasPending(key: crewId) else { return }
        withFileLock(crewId) {
            var rows = loadLocked(crewId)
            // 还是读不动就原地不动 —— 绝不在这时候落号，空表算出来的 #N 会跟盘上撞车。
            guard !refuseUnsafeEmptyRewrite(crewId: crewId, rows: rows) else { return }
            let before = rows.count
            drainRequests(crewId: crewId, rows: &rows)
            guard rows.count > before else { return }
            _ = saveLocked(crewId: crewId, rows: rows)
        }
    }

    private func drainRequests(crewId: String, rows: inout [CockpitPlanItem]) {
        var appended: [CockpitPlanItem] = []
        var nextNumber = (rows.map(\.number).max() ?? 0) + 1
        _ = spool.drain(key: crewId) { req in
            let now = Self.timestamp()
            appended.append(CockpitPlanItem(
                id: UUID().uuidString.lowercased(),
                number: nextNumber, title: req.title,
                status: CockpitPlanStatus.notStarted.rawValue,
                createdAt: now, createdBySessionId: req.bySessionId,
                createdByName: req.byName, updatedAt: now))
            nextNumber += 1
            return true
        }
        rows.append(contentsOf: appended)
    }

    private func refuseUnsafeEmptyRewrite(crewId: String, rows: [CockpitPlanItem]) -> Bool {
        MultiProcessJSONStore.refuseEmptyRewriteIfNonEmptyFile(rows, at: fileURL(crewId))
    }

    private static func timestamp() -> String {
        ISO8601DateFormatter().string(from: Date())
    }
}

/// 板读不动时存下来的**一次 `add` 请求**（不是一条算好号的条目 —— 号要数着现有行
/// 才算得出来，而存它的那一刻正是数不着的时候）。
struct PendingPlanRequest: Codable {
    let title: String
    let bySessionId: String?
    let byName: String?
}

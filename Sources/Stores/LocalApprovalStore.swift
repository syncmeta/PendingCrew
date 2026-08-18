import Foundation
import Combine

/// 本地待审批 / 待决策 store（spec 2026-06-08-pendingcrew-ask-approval-design §4）。
///
/// 每 crew 一个 JSON：`<dir>/<crewId>.approvals.json` = `[ApprovalItem]`。
/// - **决策类（decision）**：session 经 `ask` 工具 raise 一条 pending；人类（先 captain，
///   chunk 2）在 PendingCrew UI 答 → `answer`；helper 的 `ask` long-poll `item().status
///   == answered` 拿 `reply` 返回 agent。
/// - **权限类（permission）**：PreToolUse hook raise 一条 pending；人类在待审批列表
///   allow/deny → `decide`；hook long-poll 拿 `decision` 返回 allow/deny。
///
/// **自包含 Foundation**（编进 `pendingcrew-mcp` re-exec helper + PendingCrewTests bundle）。
/// `@unchecked Sendable`：实例状态全 `let`，共享可变资源是磁盘文件 —— app↔helper
/// 并发经 `MultiProcessJSONStore` 基座三件套（`<crewId>.approvals.lock` 上的 flock
/// 互斥 + 逐条 lenient 解码 + corrupt 归档 fail-loud，#528；helper 的 ask/权限 hook
/// long-poll 依赖这份文件，损坏静默当空会让在途审批凭空消失、agent 永远等不到答复）。
final class LocalApprovalStore: @unchecked Sendable {
    static let shared = LocalApprovalStore()

    private let directory: URL

    /// 进程内变更信号（去轮询）。本进程每次 `save` 后发一个 `crewId` —— UI
    /// (`SessionApprovalCardsView` / `CrewCenterView`) 订阅 `approvalChanges(crewId:)`
    /// 把 1.5s/2s 轮询换成「写即刷新」，对齐 `LocalWhiteboardStore.changes`。
    /// app 侧人类 answer/decide 走 `.shared` 实例,立即推。
    ///
    /// **跨进程写**（helper `--mcp-serve`/`--mcp-hook` 子进程 raise pending）走另一个
    /// 进程的实例、不经此 subject —— 由 `approvalChanges` 合流的目录监听补齐：
    /// approvals JSON 与白板 JSON **同目录**(`LocalWhiteboardStore.defaultDirectory`),
    /// 已被 `LocalWhiteboardStore.startWatching()` 的 `DispatchSource` 一并监听,
    /// 所以 helper 跨进程 raise 也即时推到 UI,无需轮询。
    ///
    /// `PassthroughSubject` 引用类型作 `let` 持有,不破坏 `@unchecked Sendable`
    /// （实例其余皆 `let`）；低频单写,`.send` 在 app MainActor + helper 各自调,
    /// 无跨线程竞态热点（与 `LocalWhiteboardStore.changes` 同模式）。
    let changes = PassthroughSubject<String, Never>()

    init(directory: URL? = nil) {
        self.directory = directory ?? LocalWhiteboardStore.defaultDirectory
        try? FileManager.default.createDirectory(at: self.directory, withIntermediateDirectories: true)
    }

    // MARK: - 变更流（去轮询）

    /// 本 crew 的待审批/待决策变更流（去 1.5s/2s 轮询）。两个上游合流成 `Void` tick:
    /// 1. 本进程 `changes` 按 crewId 过滤 —— app 侧 answer/decide 即推。
    /// 2. **跨进程**目录监听 `LocalWhiteboardStore.shared.directoryChanged` —— helper
    ///    子进程 raise pending(写 `<dir>/<crewId>.approvals.json`)落在同一被监听目录,
    ///    收到即让调用方重拉本 crew(廉价 JSON 读)。目录事件不带 crewId,故不过滤。
    ///
    /// 首次订阅 `startWatching()` 起目录监听(幂等;白板订阅可能已起过)。订阅 `Task`
    /// 取消时 `onTermination` 退订两条 Combine(目录监听是 app 级共享,留着不停)。
    /// 与 `LocalBackend.whiteboardChanges` 同结构(nonisolated;由 MainActor `.task` 调)。
    func approvalChanges(crewId: String) -> AsyncStream<Void> {
        LocalWhiteboardStore.shared.startWatching()
        let inProcess = changes
        let crossProcess = LocalWhiteboardStore.shared.directoryChanged
        return AsyncStream { continuation in
            let c1 = inProcess
                .filter { $0 == crewId }
                .sink { _ in continuation.yield(()) }
            let c2 = crossProcess
                .sink { _ in continuation.yield(()) }
            continuation.onTermination = { _ in c1.cancel(); c2.cancel() }
        }
    }

    // MARK: - Read

    func list(crewId: String) -> [ApprovalItem] {
        withFileLock(crewId) { loadLocked(crewId) }
    }
    func pending(crewId: String) -> [ApprovalItem] {
        list(crewId: crewId).filter { $0.status == "pending" }
    }
    func item(crewId: String, id: String) -> ApprovalItem? {
        list(crewId: crewId).first { $0.id == id }
    }

    /// 本 crew 审批账本文件的指纹（mtime+size，**只 stat 不读内容、不拿锁**）。
    /// 点名快照那 2 秒一拍的门控用（`SessionAwaitingReplyInputsCache`）：指纹没变
    /// 就不必再来一次 flock + 整份解码。语义论证见那个类型的注释。
    func fingerprint(crewId: String) -> FileChangeGate.Fingerprint? {
        FileChangeGate.fingerprint(atPath: directoryPath + "/" + crewId + ".approvals.json")
    }
    /// `directory.path` 缓存一份 —— 指纹路径拼接每拍要走几十次，别每次都问 URL。
    private var directoryPath: String { directory.path }

    // MARK: - Write

    /// raise 一条待处理（kind: "decision" | "permission"）。返回新 id。
    /// **nil = 没落盘**（列表读不出来 / 读到空但磁盘非空，已归档 + 白板警示）——
    /// 此前照样返回 id，调用方就会拿着一个磁盘上根本不存在的 reqId 去 long-poll，
    /// 干等到 30 分钟超时才知道没人答（#577）。
    @discardableResult
    func raise(crewId: String, kind: String, sessionId: String, summary: String) -> String? {
        let item = ApprovalItem(
            id: UUID().uuidString.lowercased(), kind: kind, sessionId: sessionId,
            summary: summary, status: "pending", reply: nil, decision: nil,
            createdAt: ISO8601DateFormatter().string(from: Date()))
        return withFileLock(crewId) {
            var rows = loadLocked(crewId)
            guard !refuseUnsafeEmptyRewrite(crewId: crewId, rows: rows) else { return nil }
            rows.append(item)
            saveLocked(crewId: crewId, rows: rows)
            return item.id
        }
    }

    /// 决策类答复（人类 / captain 答文本）。
    func answer(crewId: String, id: String, reply: String) {
        update(crewId: crewId, id: id) { $0.status = "answered"; $0.reply = reply }
    }

    /// 权限类决定（allow / deny）。
    func decide(crewId: String, id: String, decision: String) {
        update(crewId: crewId, id: id) { $0.status = "answered"; $0.decision = decision }
    }

    // MARK: - Persistence（基座三件套：flock / 逐条 lenient / corrupt 归档，#528）

    private func fileURL(_ crewId: String) -> URL {
        directory.appendingPathComponent("\(crewId).approvals.json")
    }
    /// 跨进程互斥：app（answer/decide）与 helper（raise + long-poll 读）的
    /// read-modify-write 都在 `<crewId>.approvals.lock` 内做。只在 public 入口拿
    /// 一次，锁内一律走 `*Locked` 变体。
    private func withFileLock<T>(_ crewId: String, _ body: () -> T) -> T {
        MultiProcessJSONStore.withFileLock(
            directory.appendingPathComponent("\(crewId).approvals.lock"), body)
    }
    /// 锁内读。坏一条丢一条；整文件损坏 → 归档 + 白板系统警示（在途审批凭空消失
    /// 必须有人看见，卡死的 agent 才有人去救），从空重新开始。警示走白板自己的
    /// `<crewId>.lock`（不同文件、无反向嵌套，不死锁）。
    private func loadLocked(_ crewId: String) -> [ApprovalItem] {
        MultiProcessJSONStore.loadRowsLocked(
            ApprovalItem.self, at: fileURL(crewId),
            onIncident: { self.reportIncident(crewId: crewId, $0) })
    }

    /// 往白板落一条如实的系统警示。**两种事故两套文案**（2026-08-12）：只有确认解不开
    /// 才谈得上「在途审批丢了」；读不出来时原件完好，说丢是吓人。
    private func reportIncident(crewId: String, _ incident: MultiProcessJSONStore.LedgerIncident) {
        let tail = incident.isDataIntact
            ? "在途的 ask/审批没丢，等这次读得动就还在。"
            : "在途的 ask/审批已丢失，等答复的 session 需要重新发起。"
        LocalWhiteboardStore(directory: directory).appendSessionMessage(
            crewId: crewId, sessionId: "system",
            text: "待审批/待决策列表：" + incident.summary + tail,
            senderName: "系统")
    }
    private func saveLocked(crewId: String, rows: [ApprovalItem]) {
        MultiProcessJSONStore.saveRowsLocked(rows, to: fileURL(crewId))
        // 单一写入漏斗 —— raise / answer / decide 都经此,发一个变更信号(去轮询)。
        // 订阅方(`approvalChanges`)按 crewId 过滤。
        changes.send(crewId)
    }
    private func refuseUnsafeEmptyRewrite(crewId: String, rows: [ApprovalItem]) -> Bool {
        // 拒写闸不再自己报警：读失败 / 损坏都已由上面的 `loadLocked` 如实报过一次
        // （2026-08-12 起宽松读也报 `.unreadable`）。这道闸现在只负责**拒写**——
        // 它能触发的前提就是刚才那次读已经出过事，再报一遍就是同一件事说两遍，
        // 而群聊里的重复噪音正是这次事故要治的东西之一。
        return MultiProcessJSONStore.refuseEmptyRewriteIfNonEmptyFile(rows, at: fileURL(crewId))
    }
    private func update(crewId: String, id: String, _ mut: (inout ApprovalItem) -> Void) {
        withFileLock(crewId) {
            var rows = loadLocked(crewId)
            // 读不出来时 rows 是空表，光凭 firstIndex 找不到就返回等于静默作废这次
            // 答复。先过拒写闸（归档 + 白板警示），至少群里知道审批账本出事了（#577）。
            guard !refuseUnsafeEmptyRewrite(crewId: crewId, rows: rows) else { return }
            guard let idx = rows.firstIndex(where: { $0.id == id }) else { return }
            mut(&rows[idx])
            saveLocked(crewId: crewId, rows: rows)
        }
    }
}

/// 一条待审批 / 待决策。`reply`（决策答复）/ `decision`（权限 allow|deny）按 kind 用其一。
struct ApprovalItem: Codable, Equatable {
    let id: String
    /// "decision"（ask）| "permission"（权限 hook）
    let kind: String
    let sessionId: String
    let summary: String
    /// "pending" | "answered"
    var status: String
    /// 决策类的人类 / captain 答复文本。
    var reply: String?
    /// 权限类的决定 "allow" | "deny"。
    var decision: String?
    let createdAt: String
}

/// Codex manual-review bridge: persist the card first, then announce it, then wait
/// for the same stored item to be decided. Keeping this beside the store makes the
/// ordering invariant independently testable without an app-server process.
enum CodexManualApprovalBridge {
    static func provider(
        crewId: String,
        sessionId: String,
        directory: URL = LocalWhiteboardStore.defaultDirectory,
        pollIntervalNanoseconds: UInt64 = 500_000_000,
        maxWaits: Int = 3600
    ) -> (_ summary: String, _ decisions: [String]) async -> String {
        return { summary, decisions in
            let approvals = LocalApprovalStore(directory: directory)
            let board = LocalWhiteboardStore(directory: directory)
            guard let reqId = approvals.raise(
                crewId: crewId, kind: "permission", sessionId: sessionId, summary: summary) else {
                board.appendSessionMessage(
                    crewId: crewId, sessionId: sessionId,
                    text: "待审批没能记进审批账本（文件读不出来或漏读，已归档），已代为回绝："
                        + "\(summary)\n审批列表这会儿不可用，修好后让它重试。",
                    category: "question",
                    mentions: [LocalWhiteboardMention(kind: "human", targetId: nil),
                               LocalWhiteboardMention(kind: "captain", targetId: nil)])
                return "decline"
            }
            // The notice is deliberately after the durable pending write. A user who
            // follows it can therefore always find an operable card for this session.
            board.appendSessionMessage(
                crewId: crewId, sessionId: sessionId,
                text: "待审批：\(summary)\n（去该 session 详情的审批卡 allow / deny）",
                category: "question",
                mentions: [LocalWhiteboardMention(kind: "human", targetId: nil),
                           LocalWhiteboardMention(kind: "captain", targetId: nil)])

            var waits = 0
            while waits < maxWaits {
                if let item = approvals.item(crewId: crewId, id: reqId),
                   item.status == "answered" {
                    if item.decision == "allow" {
                        return decisions.contains("accept")
                            ? "accept" : (decisions.first ?? "accept")
                    }
                    return decisions.contains("decline")
                        ? "decline" : (decisions.last ?? "decline")
                }
                try? await Task.sleep(nanoseconds: pollIntervalNanoseconds)
                waits += 1
            }
            // The server request is no longer waiting after this return. Close the
            // durable card too, otherwise the UI keeps offering an action whose reply
            // can never reach Codex (a stale-card variant of the same lifecycle bug).
            approvals.decide(crewId: crewId, id: reqId, decision: "deny")
            return decisions.contains("decline") ? "decline" : (decisions.last ?? "decline")
        }
    }
}

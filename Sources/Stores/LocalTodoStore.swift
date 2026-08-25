import Foundation
import Combine

/// 两本 Todo 账（Todo #62）—— 同一套存储基座，**方向相反**。
///
/// 人类原话：「弄一个给人类的 todo。需要人类拍板、决策、需要人才能做的事放进这里面，
/// 不要一股脑塞进群聊了，不然很容易漏。」所以新的这本是现有那本的**镜像**：
/// 谁加、谁回应、群里那行怎么写，全都反过来。
///
/// 命名按**谁来办**（不是谁来提）：`.agent` = 派给 agent 干的（原有的唯一一本），
/// `.human` = 要人类拍板的（新增）。驾驶舱两个药丸「Agent 的 / 人类的」照此对应。
///
/// 🚫 **不许 fork 出第二个 store**：两本账共用 `LocalTodoStore` 这一套
/// flock / 逐条 lenient 解码 / corrupt 归档基座。复制粘贴出第二份必然漏掉其中一件，
/// 而这两本都是人手输/人要看的数据，最不能静默清空。
enum TodoLedger: String, Codable, Sendable, CaseIterable {
    /// 人类派给 agent 的那本（task #478 起就有的唯一一本）。落 `<crewId>.todos.json`。
    case agent
    /// agent 请人类拍板的那本（Todo #62 新增）。落 `<crewId>.human-todos.json`。
    case human

    /// 列表文件后缀。`.agent` 保持原名不动 —— 已有机器上的账都在那个文件里。
    var fileSuffix: String {
        switch self {
        case .agent: return ".todos.json"
        case .human: return ".human-todos.json"
        }
    }

    /// flock 文件后缀。两本各一把锁 —— 一本忙不该挡住另一本。
    var lockSuffix: String {
        switch self {
        case .agent: return ".todos.lock"
        case .human: return ".human-todos.lock"
        }
    }

    /// 驾驶舱药丸上的名字（人类原话「弄两个药丸选择：Agent 的、人类的」）。
    var pillTitle: String {
        switch self {
        case .agent: return "Agent 的"
        case .human: return "人类的"
        }
    }

    /// 事故警示的主语 —— 两本各说各的，否则白板上一条「Todo 列表读不出来」
    /// 没人知道是哪本坏了。
    var incidentSubject: String {
        switch self {
        case .agent: return "Todo 列表（Agent 的）"
        case .human: return "Todo 列表（人类的）"
        }
    }

    /// 谁能**新增**条目。方向反过来的那一半就在这儿。
    var author: TodoParty {
        switch self {
        case .agent: return .human     // 人类派活
        case .human: return .agent     // agent 请人拍板
        }
    }

    /// 谁来**回应**条目。
    var responder: TodoParty {
        switch self {
        case .agent: return .agent
        case .human: return .human
        }
    }

    /// 新建条目时群里那行。**两本各自从 #1 编号，会打架** —— 所以人类那本带
    /// 「人类」二字，一眼分得清是哪本账的 #N。
    func newItemAnnouncement(number: Int, text: String) -> String {
        switch self {
        case .agent: return "To do +1: #\(number) \(text)"
        case .human: return "人类 To do +1: #\(number) \(text)"
        }
    }

    /// 回应条目时群里那行（目前只有人类那本会往群里发回应 —— agent 回应留在面板里）。
    func responseAnnouncement(number: Int, text: String) -> String {
        switch self {
        case .agent: return "回应 To Do #\(number)：\(text)"
        case .human: return "回应 人类 To Do #\(number)：\(text)"
        }
    }
}

/// 一本账里的两个角色。`TodoLedger.author` / `.responder` 用它把方向说明白。
enum TodoParty: String, Codable, Sendable {
    case human
    case agent
}

/// 每 crew 一本 Todo 列表（task #478；#487 后列表在驾驶舱 CrewTodoPanel。
/// Todo #62 起同一套基座跑**两本账**，见 `TodoLedger`）。
///
/// 每 crew 一个 JSON：`<dir>/<crewId><ledger.fileSuffix>` = `[LocalTodoItem]`。
///
/// **`.agent` 那本（人类派给 agent）**：
/// - **只有人类能加条目**：唯一的新增入口是 `add`（CrewChatView 的 Todo 模式发送调用）；
///   MCP 侧不暴露新增工具，机器人加不了。
/// - **机器人只能回应**：helper 的 `respond_todo` 走 `respond` —— **追加式**回应
///   （每次追加一条，绝不覆盖旧回应），可顺带推进状态：
///   待办 pending → 进行中 in_progress → 完成 completed。
/// - **人类可改/删/追问**（Todo #21）：`edit` 改正文、`delete` 软删、`followUp`
///   追问——三件都不看状态，已完成、已被回复过的条目照样动得了（人类原话
///   「todo 要随时能修改、删除、追问（如已经被回复）」）。追问与重开是同一条通道：
///   completed 被追问就翻回 pending（`reopen` = `followUp` 的 completed-only 守卫版，
///   Todo #12）。这三条只给人类（详细窗口 UI），MCP 侧不暴露。
///
/// **`.human` 那本（agent 请人类拍板，Todo #62）**：方向整个反过来 ——
/// **只有 agent 能加**（MCP 新工具），**人类回应**（详细窗口，走同一个 `respond`），
/// 人类同样能改/删/重开。条目额外记 `createdBySessionId`：人类回应时得知道叫醒谁。
///
/// 两边共通：条目编号 `number` 从 1 自增、**crew 内 + 账本内**唯一 ——
/// 两本账各自从 #1 起，同一个 #1 在两本里指两件事，所以群里那行必须带账本前缀
/// （见 `TodoLedger.newItemAnnouncement`）。
///
/// **自包含 Foundation**（编进 `pendingcrew-mcp` re-exec helper + PendingCrewTests
/// bundle）。`@unchecked Sendable`：实例状态全 `let`，共享可变资源是磁盘文件 ——
/// app↔helper 并发经 `MultiProcessJSONStore` 基座三件套（`<crewId>` 那把 lock 上的
/// flock 互斥 + 逐条 lenient 解码 + corrupt 归档 fail-loud，#528；人手输的 Todo
/// 是最不能「文件损坏 → 静默清空」的一类数据）。
final class LocalTodoStore: @unchecked Sendable {
    /// 人类派给 agent 的那本（原有唯一一本，调用方一个字不用改）。
    static let shared = LocalTodoStore()
    /// agent 请人类拍板的那本（Todo #62）。
    static let humanShared = LocalTodoStore(ledger: .human)

    /// 按账本取共享实例 —— UI 药丸切换直接拿这个，别自己 new。
    static func shared(_ ledger: TodoLedger) -> LocalTodoStore {
        ledger == .human ? humanShared : shared
    }

    /// 合法状态集（待办 → 进行中 → 完成）。
    static let validStatuses: Set<String> = ["pending", "in_progress", "completed"]

    /// 这个实例管哪本账。文件名、锁名、事故警示主语全从它来。
    let ledger: TodoLedger

    private let directory: URL

    /// 进程内变更信号：本进程每次 save 后发一个 `crewId`（app 侧人类 add 即推）。
    /// 跨进程写（helper `respond_todo`）由 `todoChanges` 合流的目录监听补齐 ——
    /// todos JSON 与白板 JSON 同目录（`LocalWhiteboardStore.defaultDirectory`），
    /// 已被 `LocalWhiteboardStore.startWatching()` 的 DispatchSource 一并监听。
    let changes = PassthroughSubject<String, Never>()

    /// 同目录下的**另一本账**。helper 只拿到一个 `--dir`，用它开第二本，
    /// 别让调用方漏传就静默退回默认目录（helper 的 `--dir` 不是默认目录）。
    func sibling(_ other: TodoLedger) -> LocalTodoStore {
        other == ledger ? self : LocalTodoStore(directory: directory, ledger: other)
    }

    init(directory: URL? = nil, ledger: TodoLedger = .agent) {
        self.directory = directory ?? LocalWhiteboardStore.defaultDirectory
        self.ledger = ledger
        try? FileManager.default.createDirectory(at: self.directory, withIntermediateDirectories: true)
    }

    // MARK: - 变更流（去轮询；与 LocalApprovalStore.approvalChanges 同模式）

    /// 本 crew 的 todo 变更流：本进程 `changes` 按 crewId 过滤 + 跨进程目录监听
    ///（helper `respond_todo` 写盘落在同一被监听目录，事件不带 crewId 不过滤）。
    func todoChanges(crewId: String) -> AsyncStream<Void> {
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

    /// 活着的条目（删掉的墓碑行不出现在任何 UI / MCP 视图里）。
    func list(crewId: String) -> [LocalTodoItem] {
        withFileLock(crewId) { loadLocked(crewId).filter { !$0.isDeleted } }
    }

    func item(crewId: String, number: Int) -> LocalTodoItem? {
        list(crewId: crewId).first { $0.number == number }
    }

    /// 锁内按 #N 找一条**活着的**条目。删掉的墓碑行对所有写入路径都不可见 ——
    /// 机器人回应 / 人类改删追问都不该打在已删的行上。
    private func liveIndexLocked(_ rows: [LocalTodoItem], _ number: Int) -> Int? {
        rows.firstIndex { $0.number == number && !$0.isDeleted }
    }

    // MARK: - Write

    /// 新增一条 Todo。返回新条目（含分到的 #N）。**谁能调由账本方向定**
    /// （`.agent` 那本只有人类调 —— 面板 UI；`.human` 那本只有 agent 调 ——
    /// MCP `add_human_todo`）。
    /// **nil = 没写进去**（列表文件读不出来 / 读到空但磁盘非空，已归档 + 白板警示）——
    /// 此前这里照样返回条目，调用方会拿着一个根本不存在的 #N 去群里宣布（#577）。
    ///
    /// `attachments`（Todo #52）：人类建 Todo 时附的图/文件，已由
    /// `CrewChatAttachmentStore` 落进与群聊**同一个**附件目录，这里只记条目。
    ///
    /// `bySessionId` / `bySenderName`（Todo #62）：**谁提的**。`.human` 那本
    /// 缺了它整个功能落不了地 —— 人类回应时根本不知道该叫醒谁（回落规则见
    /// `HumanTodoWakePlan`）。`.agent` 那本由人类新增，两个都留 nil。
    @discardableResult
    func add(crewId: String, text: String,
             attachments: [LocalWhiteboardAttachment]? = nil,
             bySessionId: String? = nil,
             bySenderName: String? = nil) -> LocalTodoItem? {
        withFileLock(crewId) {
            var rows = loadLocked(crewId)
            let item = LocalTodoItem(
                id: UUID().uuidString.lowercased(),
                number: (rows.map(\.number).max() ?? 0) + 1,
                text: text,
                status: "pending",
                createdAt: ISO8601DateFormatter().string(from: Date()),
                attachments: (attachments?.isEmpty ?? true) ? nil : attachments,
                createdBySessionId: bySessionId,
                createdBySenderName: bySenderName)
            guard !refuseUnsafeEmptyRewrite(crewId: crewId, rows: rows) else { return nil }
            rows.append(item)
            saveLocked(crewId: crewId, rows: rows)
            return item
        }
    }

    /// 机器人回应某条 Todo（追加式）：往 `responses` 尾部加一条，`newStatus` 非 nil
    /// 且合法时同时推进条目状态。找不到 #N → nil；非法 status → 忽略状态只追加回应
    ///（合法性卫生归 McpServer，那里会先拒掉并报错，不走到这）。
    @discardableResult
    func respond(crewId: String, number: Int, sessionId: String,
                 senderName: String? = nil, text: String,
                 newStatus: String? = nil) -> LocalTodoItem? {
        withFileLock(crewId) {
            var rows = loadLocked(crewId)
            guard !refuseUnsafeEmptyRewrite(crewId: crewId, rows: rows) else { return nil }
            guard let idx = liveIndexLocked(rows, number) else { return nil }
            rows[idx].responses.append(LocalTodoResponse(
                id: UUID().uuidString.lowercased(),
                sessionId: sessionId,
                senderName: senderName,
                text: text,
                status: newStatus,
                createdAt: ISO8601DateFormatter().string(from: Date())))
            if let s = newStatus, Self.validStatuses.contains(s) {
                rows[idx].status = s
            }
            saveLocked(crewId: crewId, rows: rows)
            return rows[idx]
        }
    }

    /// 人类改条目正文（Todo #21）。**任何状态都能改** —— 已完成、已被机器人回应过
    /// 的条目照样改得动（人类原话「todo 要随时能修改」）。改动不动状态、不动回应
    /// 时间线。找不到 #N（或已删）→ nil；正文全空白 → nil 不动（空 Todo 无意义）。
    @discardableResult
    func edit(crewId: String, number: Int, text: String) -> LocalTodoItem? {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return nil }
        return withFileLock(crewId) {
            var rows = loadLocked(crewId)
            guard !refuseUnsafeEmptyRewrite(crewId: crewId, rows: rows) else { return nil }
            guard let idx = liveIndexLocked(rows, number) else { return nil }
            rows[idx].text = trimmed
            saveLocked(crewId: crewId, rows: rows)
            return rows[idx]
        }
    }

    /// 人类删条目（Todo #21）。**软删** —— 打 `deletedAt` 墓碑而不是抹掉行：
    /// #N 是群消息「To do +1: #N」里对外说过的编号，物理删会让 `add` 的 max+1
    /// 把它发回去，同一个 #N 指两件事。墓碑行对 `list` / 写入路径一律不可见。
    /// 找不到 #N（或已删）→ false。
    @discardableResult
    func delete(crewId: String, number: Int) -> Bool {
        withFileLock(crewId) {
            var rows = loadLocked(crewId)
            guard !refuseUnsafeEmptyRewrite(crewId: crewId, rows: rows) else { return false }
            guard let idx = liveIndexLocked(rows, number) else { return false }
            rows[idx].deletedAt = ISO8601DateFormatter().string(from: Date())
            saveLocked(crewId: crewId, rows: rows)
            return true
        }
    }

    /// 人类「看过了，不打算回应」（Todo #62）：只打 `dismissedAt` 标记，**不加回应、
    /// 不动状态**。用途是把黄点按灭 —— 有些事人类看过就决定不办，没有这个开关，
    /// 那一条会把黄点永久钉死（黄点判据是「有没有未回应条目」，见 `isUnanswered`）。
    /// `dismissed: false` = 反悔，重新算作未回应。找不到 #N（或已删）→ false。
    @discardableResult
    func setDismissed(crewId: String, number: Int, dismissed: Bool = true) -> Bool {
        withFileLock(crewId) {
            var rows = loadLocked(crewId)
            guard !refuseUnsafeEmptyRewrite(crewId: crewId, rows: rows) else { return false }
            guard let idx = liveIndexLocked(rows, number) else { return false }
            rows[idx].dismissedAt = dismissed
                ? ISO8601DateFormatter().string(from: Date())
                : nil
            saveLocked(crewId: crewId, rows: rows)
            return true
        }
    }

    /// 人类追问（Todo #21，Todo #12 `reopen` 的一般化）：**任何状态都能追问** ——
    /// 已经被机器人回复过、已经完成的条目照样接着问。追问作为一条
    /// `LocalTodoResponse` 落在条目时间线上（sessionId 固定 `"human"`、senderName
    /// 「人」），与机器人回应同列按时间序渲染。
    ///
    /// 状态语义：completed 的条目被追问 = 事情没完，翻回 pending（这就是原
    /// `reopen` 那条通道，追问入口与它合流）；pending / in_progress 保持不动
    /// （已经在待办里了，追问不该把进行中打回去）。note 留空落默认文案。
    /// 找不到 #N（或已删）→ nil。
    ///
    /// `attachments`（Todo #52）：追问也能附图 —— 「如图，这里还不对」是人追问时
    /// 最常见的一句话。图挂在这条追问（`LocalTodoResponse`）上，不动条目本身的图。
    @discardableResult
    func followUp(crewId: String, number: Int, note: String,
                  attachments: [LocalWhiteboardAttachment]? = nil) -> LocalTodoItem? {
        followUp(crewId: crewId, number: number, note: note,
                 attachments: attachments, requireCompleted: false)
    }

    /// 重开：completed → pending（Todo #12）。现在只是 `followUp` 的守卫版 ——
    /// 状态不是 completed 就 nil 不动，语义与调用方（旧「重开」按钮、单测）不变。
    @discardableResult
    func reopen(crewId: String, number: Int, note: String) -> LocalTodoItem? {
        followUp(crewId: crewId, number: number, note: note,
                 attachments: nil, requireCompleted: true)
    }

    /// 追问/重开共用核心。`requireCompleted` = 只接受 completed（重开的守卫），
    /// 守卫与写在同一把锁内做，中间没有别的进程能把状态挪走。
    private func followUp(crewId: String, number: Int, note: String,
                          attachments: [LocalWhiteboardAttachment]?,
                          requireCompleted: Bool) -> LocalTodoItem? {
        withFileLock(crewId) {
            var rows = loadLocked(crewId)
            guard !refuseUnsafeEmptyRewrite(crewId: crewId, rows: rows) else { return nil }
            guard let idx = liveIndexLocked(rows, number) else { return nil }
            let wasCompleted = rows[idx].status == "completed"
            guard !requireCompleted || wasCompleted else { return nil }
            let text = note.trimmingCharacters(in: .whitespacesAndNewlines)
            rows[idx].responses.append(LocalTodoResponse(
                id: UUID().uuidString.lowercased(),
                sessionId: "human",
                senderName: "人",
                text: text.isEmpty ? (wasCompleted ? "重开了这条 Todo" : "追问了这条 Todo") : text,
                // status 记这条追问把条目推到了哪 —— 只有重开那次真动了状态。
                status: wasCompleted ? "pending" : nil,
                createdAt: ISO8601DateFormatter().string(from: Date()),
                attachments: (attachments?.isEmpty ?? true) ? nil : attachments))
            if wasCompleted { rows[idx].status = "pending" }
            saveLocked(crewId: crewId, rows: rows)
            return rows[idx]
        }
    }

    // MARK: - Persistence（基座三件套：flock / 逐条 lenient / corrupt 归档，#528）

    private func fileURL(_ crewId: String) -> URL {
        directory.appendingPathComponent("\(crewId)\(ledger.fileSuffix)")
    }

    /// 跨进程互斥：app（面板 add/回应/重开）与 helper（`respond_todo` /
    /// `add_human_todo`）的 read-modify-write 都在 `<crewId><ledger.lockSuffix>`
    /// 内做。**两本账各一把锁** —— 一本正在被写不该挡住另一本。只在 public 入口
    /// 拿一次，锁内一律走 `*Locked` 变体。
    private func withFileLock<T>(_ crewId: String, _ body: () -> T) -> T {
        MultiProcessJSONStore.withFileLock(
            directory.appendingPathComponent("\(crewId)\(ledger.lockSuffix)"), body)
    }

    /// 锁内读。坏一条丢一条；出事就 fail-loud 到白板（人类手输的 Todo 蒸发必须有人
    /// 看见），绝不静默当空让下一次写清史。**两种事故两套文案**（2026-08-12）：
    /// 「读不出来」= 原件完好、本次写已拒；「确认解不开」= 已归档、列表从空重来。
    /// 警示走白板自己的 `<crewId>.lock`（与本 store 锁不同文件、无反向嵌套，不死锁）。
    private func loadLocked(_ crewId: String) -> [LocalTodoItem] {
        MultiProcessJSONStore.loadRowsLocked(
            LocalTodoItem.self, at: fileURL(crewId),
            onIncident: { self.reportIncident(crewId: crewId, $0) })
    }

    /// 往白板落一条如实的系统警示。主语点名**是哪本账**（Todo #62 起有两本，
    /// 只说「Todo 列表」没人知道坏的是哪一本），其余措辞由事故类型定。
    private func reportIncident(crewId: String, _ incident: MultiProcessJSONStore.LedgerIncident) {
        LocalWhiteboardStore(directory: directory).appendSessionMessage(
            crewId: crewId, sessionId: "system",
            text: ledger.incidentSubject + "：" + incident.summary,
            senderName: "系统")
    }

    private func saveLocked(crewId: String, rows: [LocalTodoItem]) {
        MultiProcessJSONStore.saveRowsLocked(rows, to: fileURL(crewId))
        changes.send(crewId)
    }

    /// **每一条**读-改-写路径开头都要过这道闸（#577）：文件读不出来时 `loadLocked`
    /// 返回空表，光靠 `liveIndexLocked` 找不到 #N 就返回 nil 的话，回执会说「找不到
    /// 这条 Todo」—— 听起来像人类删过，其实是列表读不出来。过闸后至少归档 + 白板
    /// 警示，群里看得见真正的原因。
    private func refuseUnsafeEmptyRewrite(crewId: String, rows: [LocalTodoItem]) -> Bool {
        // 拒写闸不再自己报警：读失败 / 损坏都已由上面的 `loadLocked` 如实报过一次
        // （2026-08-12 起宽松读也报 `.unreadable`）。这道闸现在只负责**拒写**——
        // 它能触发的前提就是刚才那次读已经出过事，再报一遍就是同一件事说两遍，
        // 而群聊里的重复噪音正是这次事故要治的东西之一。
        return MultiProcessJSONStore.refuseEmptyRewriteIfNonEmptyFile(rows, at: fileURL(crewId))
    }
}

/// 一条人类 Todo。`number` = 群消息「To do +1: #N」的 N（crew 内自增唯一）。
struct LocalTodoItem: Codable, Equatable, Identifiable {
    let id: String
    let number: Int
    /// 条目正文。人类可随时改（`edit`），任何状态、回应过也能改。
    var text: String
    /// "pending"（待办）| "in_progress"（进行中）| "completed"（完成）。
    var status: String
    let createdAt: String
    /// 机器人回应（追加式，按时间序）。新条目从空开始。
    var responses: [LocalTodoResponse] = []
    /// 软删墓碑（Todo #21）：非 nil = 人类删掉了这条。留着行只为把 #N 占住，
    /// 不让 `add` 的 max+1 把已对外说过的编号发第二遍。老文件没这字段 → nil。
    var deletedAt: String? = nil
    /// 条目自带的图/文件（Todo #52）。落盘走的是**群聊那同一套** attachment store
    /// （`Application Support/PendingCrew/attachments/<crewId>/`），所以 `path` 是
    /// 本机绝对路径，渲染（`file://`）与「请 Read 查看」的措辞都与群聊一致。
    /// 老文件没这字段 → nil。
    var attachments: [LocalWhiteboardAttachment]? = nil
    /// **谁提的这条**（Todo #62）。`.human` 那本必带 —— 人类回应时要按它决定叫醒谁
    /// （回落规则见 `HumanTodoWakePlan`：已退出 / 没记 / 机长自己提的 → 回落机长）。
    /// `.agent` 那本由人类新增 → nil；老文件没这字段 → nil。
    var createdBySessionId: String? = nil
    /// 提问者的显示名（session label，如「机长」）。只用于渲染，唤醒不看它。
    var createdBySenderName: String? = nil

    var isDeleted: Bool { deletedAt != nil }

    /// **还没被回应过** —— 黄点亮灭的判据（Todo #62）：`.human` 那本只要还有一条
    /// 没人回应就亮，全部有回应就灭。用「有没有回应」而不是「有没有条目」：
    /// 一本长期待办列表会让黄点永远亮着，等于没有。
    ///
    /// 已删的墓碑行不算（`list` 本来就不返回它们）；`dismissedAt` 非 nil = 人类
    /// 看过、决定不办、直接按灭，同样不再算未回应。
    var isUnanswered: Bool { responses.isEmpty && dismissedAt == nil }

    /// 人类「看过了，不打算回应」的标记（Todo #62）。没有它，一条人类不打算处理的
    /// 条目会把黄点永久钉死。不动 `status`、不加回应 —— 只是不再算未回应。
    var dismissedAt: String? = nil

    /// 讲给 agent 听的条目正文：正文 + 每个附件一行绝对路径提示（与群聊同一措辞）。
    var agentText: String {
        LocalWhiteboardAttachment.appendingAgentHints(to: text, attachments)
    }
}

/// 一条机器人回应。`status` = 本条回应把条目推进到的状态（nil = 只回应没动状态）。
struct LocalTodoResponse: Codable, Equatable, Identifiable {
    let id: String
    let sessionId: String
    /// 回应者显示名（session label，如「机长」）；nil → 渲染退回 session id。
    var senderName: String? = nil
    let text: String
    var status: String? = nil
    let createdAt: String
    /// 这条回应/追问带的图（Todo #52）。人类追问「如图还不对」走这里；机器人回应
    /// 目前不带附件（`respond_todo` 没这参数）。老文件没这字段 → nil。
    var attachments: [LocalWhiteboardAttachment]? = nil

    /// 讲给 agent 听的回应正文：正文 + 附件绝对路径提示（与群聊同一措辞）。
    var agentText: String {
        LocalWhiteboardAttachment.appendingAgentHints(to: text, attachments)
    }
}

extension LocalTodoItem {
    /// 状态的中文显示（面板徽章 + MCP 回执共用）。
    static func statusLabel(_ status: String) -> String {
        switch status {
        case "pending": return "待办"
        case "in_progress": return "进行中"
        case "completed": return "完成"
        default: return status
        }
    }
}

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

    /// 提出者撤回自己那条时群里那行（Todo #102）。
    ///
    /// **这一行是撤回这件事的全部意义所在**：一条从人眼前消失的账，绝不许静默消失。
    /// 所以原因是必填的，而且必须出现在这里 —— 人回头看群聊，要能看出「那条我本来
    /// 在等的事，为什么不用我管了」。
    func withdrawAnnouncement(number: Int, reason: String) -> String {
        switch self {
        case .agent: return "撤回 To Do #\(number)：\(reason)"
        case .human: return "撤回 人类 To Do #\(number)：\(reason)"
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
    /// 注入时钟只用于把所有真实写路径钉到同一口径；产品默认取当前时间。
    private let now: @Sendable () -> Date

    /// 进程内变更信号：本进程每次 save 后发一个 `crewId`（app 侧人类 add 即推）。
    /// 跨进程写（helper `respond_todo`）由 `todoChanges` 合流的目录监听补齐 ——
    /// todos JSON 与白板 JSON 同目录（`LocalWhiteboardStore.defaultDirectory`），
    /// 已被 `LocalWhiteboardStore.startWatching()` 的 DispatchSource 一并监听。
    let changes = PassthroughSubject<String, Never>()

    /// 同目录下的**另一本账**。helper 只拿到一个 `--dir`，用它开第二本，
    /// 别让调用方漏传就静默退回默认目录（helper 的 `--dir` 不是默认目录）。
    func sibling(_ other: TodoLedger) -> LocalTodoStore {
        other == ledger ? self : LocalTodoStore(directory: directory, ledger: other, now: now)
    }

    init(directory: URL? = nil, ledger: TodoLedger = .agent,
         now: @escaping @Sendable () -> Date = { Date() }) {
        self.directory = directory ?? LocalWhiteboardStore.defaultDirectory
        self.ledger = ledger
        self.now = now
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
    /// 活着的条目。**读不出来时返回空表** —— 这是历史行为，几十个调用点都按它写的，
    /// 这一笔不动它。要区分「真的没有」和「读不到」的调用方走 `read(crewId:)`
    /// （判「有没有事要做」的那类路径**必须**走那条，理由见 `LedgerRead`）。
    func list(crewId: String) -> [LocalTodoItem] {
        if case let .rows(rows) = read(crewId: crewId) { return rows }
        return []
    }

    /// 一次读的结果 —— **三态里的后两态**（真空 / 读不到）不许再压成同一个空数组。
    ///
    /// 病根（2026-09-08 由「机长空闲核账」那条路暴露）：`list(crewId:)` 把
    /// `loadLocked` 的失败压成 `[]`，于是「这本账一条未完成都没有」和「这本账这次
    /// 读不出来」在调用方眼里长得**一模一样**。判「有没有事要做」的那条路照着空表
    /// 一算，就会在账本坏掉时**安静地说没事**。
    ///
    /// **要澄清一件事**：`MultiProcessJSONStore.LedgerIncident.unreadable`
    /// （`MultiProcessJSONStore.swift` 里那个枚举）**一直都在**，`loadLocked` 也一直
    /// 在收它、往白板报它 —— 只有**写**路径拿它做判断（`refuseUnsafeEmptyRewrite`），
    /// **读**路径把它扔了。所以这从来不是「store 缺一个返回形状」，是**三态压成了一个值**。
    /// 我第一次报这个缺口时归因归错了，而错的那个方向（"要改 store 的返回形状"）
    /// 听起来是大改，正好会让它一直排不上。
    enum LedgerRead: Equatable {
        case rows([LocalTodoItem])
        /// 这次没读到可信内容。**任何一种事故都算** —— `.unreadable`（打不开）、
        /// `.misread`（读到空但文件非空）、`.corrupt`（解不开、已归档）。
        /// 三种的共同点就是「此刻这本账的内容不可信」，而调用方要的正是这一位。
        case unreadable
    }

    /// 跟 `list` 是**同一条读**（同一把锁、同一个解码、同一份事故上报），
    /// 区别只在于**把「读不出来」交还给调用方**，而不是压成空表。
    ///
    /// 没有新造第三种读法：底下仍然是 `MultiProcessJSONStore.loadRowsLocked`
    /// 加它本来就有的 `onIncident`，这里只是顺手把「响过没有」记下来。
    func read(crewId: String) -> LedgerRead {
        withFileLock(crewId) {
            var hadIncident = false
            let rows = MultiProcessJSONStore.loadRowsLocked(
                LocalTodoItem.self, at: fileURL(crewId),
                onIncident: { incident in
                    hadIncident = true
                    self.reportIncident(crewId: crewId, incident)
                })
            return hadIncident ? .unreadable : .rows(rows.filter { !$0.isDeleted })
        }
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
    /// `onWriteFailure`：**落盘失败时拿到那个错误**（同时本方法返回 nil）。
    /// 回执必须如实的调用点（MCP 写工具）传它；不在乎的调用点照旧不传。
    /// 形状照抄本仓库既有的 `loadRowsLocked(onIncident:)` —— 别发明第二种。
    @discardableResult
    func add(crewId: String, text: String,
             attachments: [LocalWhiteboardAttachment]? = nil,
             bySessionId: String? = nil,
             bySenderName: String? = nil,
             resumeNote: String? = nil,
             expectsResume: Bool = false,
             permissionTool: String? = nil,
             onWriteFailure: ((Error) -> Void)? = nil) -> LocalTodoItem? {
        withFileLock(crewId) {
            var rows = loadLocked(crewId)
            guard !refuseUnsafeEmptyRewrite(crewId: crewId, rows: rows) else { return nil }
            let stamp = timestamp()
            let item = LocalTodoItem(
                id: UUID().uuidString.lowercased(),
                number: (rows.map(\.number).max() ?? 0) + 1,
                text: text,
                status: "pending",
                createdAt: stamp,
                updatedAt: stamp,
                attachments: (attachments?.isEmpty ?? true) ? nil : attachments,
                createdBySessionId: bySessionId,
                createdBySenderName: bySenderName,
                resumeNote: resumeNote?.trimmingCharacters(in: .whitespacesAndNewlines)
                    .isEmpty == false ? resumeNote : nil,
                expectsResume: expectsResume,
                permissionTool: permissionTool)
            rows.append(item)
            if let failure = saveLocked(crewId: crewId, rows: rows) {
                onWriteFailure?(failure)
                return nil
            }
            return item
        }
    }

    /// 机器人回应某条 Todo（追加式）：往 `responses` 尾部加一条，`newStatus` 非 nil
    /// 且合法时同时推进条目状态。找不到 #N → nil；非法 status → 忽略状态只追加回应
    ///（合法性卫生归 McpServer，那里会先拒掉并报错，不走到这）。
    @discardableResult
    func respond(crewId: String, number: Int, sessionId: String,
                 senderName: String? = nil, text: String,
                 newStatus: String? = nil,
                 onWriteFailure: ((Error) -> Void)? = nil) -> LocalTodoItem? {
        withFileLock(crewId) {
            var rows = loadLocked(crewId)
            guard !refuseUnsafeEmptyRewrite(crewId: crewId, rows: rows) else { return nil }
            guard let idx = liveIndexLocked(rows, number) else { return nil }
            let stamp = timestamp()
            rows[idx].responses.append(LocalTodoResponse(
                id: UUID().uuidString.lowercased(),
                sessionId: sessionId,
                senderName: senderName,
                text: text,
                status: newStatus,
                createdAt: stamp))
            if let s = newStatus, Self.validStatuses.contains(s) {
                rows[idx].status = s
            }
            rows[idx].updatedAt = stamp
            if let failure = saveLocked(crewId: crewId, rows: rows) {
                onWriteFailure?(failure)
                return nil
            }
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
            guard rows[idx].text != trimmed else { return rows[idx] }
            rows[idx].text = trimmed
            rows[idx].updatedAt = timestamp()
            // 写不进去就别把改好的那一行交出去 —— 磁盘上还是旧文本。
            guard saveLocked(crewId: crewId, rows: rows) == nil else { return nil }
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
            let stamp = timestamp()
            rows[idx].deletedAt = stamp
            rows[idx].updatedAt = stamp
            guard saveLocked(crewId: crewId, rows: rows) == nil else { return false }
            return true
        }
    }

    /// 撤回一条人类 Todo（Todo #102）。**不是删除** —— 见 `LocalTodoItem.withdrawnAt`。
    /// 谁撤得动：提出者本人，**或这个 crew 的机长**（`isCaptain`，见 `withdrawObstacle`）。
    ///
    /// 三条约束，一条都不能松：
    /// - **提出者本人、或这个 crew 的机长**。提出者的判据是条目上记着的
    ///   `createdBySessionId`，不是显示名 —— 名字会重、会改。机长的判据是身份本身
    ///   （helper 的 `--captain`），跟条目上记着什么无关，见 `withdrawObstacle`。
    /// - **原因必填**。撤回是让一条事从人的待办里消失，没有原因它就是静默消失。
    /// - **调用方必须在群里把原因说出来**（`TodoLedger.withdrawAnnouncement`）。
    ///   这一步在 MCP 那层做，但它是这个动作的一部分，不是可选装饰。
    ///
    /// 返回**具名结果**而不是 `Bool`：撤不动的几种原因（不是你提的 / 找不到 / 已经
    /// 撤过 / 账读不出来）在调用方那里要说成不同的话，压成一个 false 就等于把
    /// 「为什么」丢在这一层，agent 只能猜。
    enum WithdrawOutcome: Equatable {
        /// 撤成了，带上撤完之后的条目。
        case withdrawn(LocalTodoItem)
        /// 这本账上没有这个 #N（或者已经被人类删了）。
        case notFound
        /// 这条不是你提的，而且你也不是本 crew 的机长 —— 带上账上记着的提出者显示名
        /// （记不到就是 nil）。**这时候的出口是机长**，不是「让人类自己删」：
        /// 提出者已经消失、或老条目根本没记提出者时，只有机长撤得动。
        case notYours(owner: String?)
        /// 已经撤过了，不重复动账。
        case alreadyWithdrawn(LocalTodoItem)
        /// 原因是空的。
        case reasonRequired
        /// 列表文件这次读不出来 / 读到空但磁盘非空 —— 什么都没改。
        case ledgerUnavailable
        /// 读到了、也改好了，**但这一笔没落到磁盘上**。原件仍是撤回之前的样子：
        /// 那条 Todo 还挂在人的账上等他回应。带的是错误原文。
        case notWritten(String)
    }

    /// 撤回资格的**唯一判据**（纯函数）。`nil` = 撤得动；非 nil = 撤不动的那个原因。
    ///
    /// 抽出来是因为它有第二个调用点：`add_human_todo(supersedes: N)` 里的 N **必须
    /// 在落任何账之前先验一遍** —— 给了一个指针就要解引用，不能只记下来。
    /// （今晚全机那笔假账带着一个根本不存在的 commit hash，挂了 191 小时，
    /// 因为没有任何人去解析它。）两条路各写一套判据，迟早会分叉。
    ///
    /// ## `isCaptain`：判据从「你是不是提出者」换成「你是不是这个 crew 的机长」
    /// （2026-09-08，人类原话「我希望机长能处理所有的 todo，不要出现这种撤不掉的情况」）
    ///
    /// 原来只认 `createdBySessionId == sessionId`，而 **session 是会消失的实体** ——
    /// 后台重启、正常收工、被停掉都会带走它，这是常态不是异常。把一条永久性的权限
    /// 挂在一个会消失的东西上，就是那个 bug 的形状本身：提出者一没，那条就**谁也
    /// 撤不掉**，只能一直挂在人的待办里亮灯，催他答一个他早就答过的问题。当天真撞上
    /// 三类：提出者已消失的、`createdBySessionId` 根本没记的老条目（字段是后加的）、
    /// 别人代提的。
    ///
    /// 换成机长身份，是因为**机长是常驻角色**：这个 session 没了，crew 还有机长；
    /// 子 crew 换了机长，新机长照样撤得动 —— 问题就已经解决了。
    ///
    /// 放宽**只到这里为止**：
    /// - 只解开「谁能撤」这一道闸。已被人类删掉（`.notFound`）、已经撤过
    ///   （`.alreadyWithdrawn`）、原因必填（在 `withdraw` 里）三道，机长一道都绕不过。
    /// - **不跨 crew**：这个函数根本看不见 crew —— 它只判断手上这一条。真正的边界在
    ///   调用方：`withdraw(crewId:)` 和 MCP 那层的 `crewId` 都是本 session 那一个，
    ///   父 crew 的机长伸不进子 crew（那会绕过人家自己的机长）。
    static func withdrawObstacle(item: LocalTodoItem?, sessionId: String,
                                 isCaptain: Bool = false) -> WithdrawOutcome? {
        guard let item, !item.isDeleted else { return .notFound }
        if !isCaptain {
            guard let owner = item.createdBySessionId, owner == sessionId else {
                return .notYours(owner: item.createdBySenderName)
            }
        }
        if item.withdrawnAt != nil { return .alreadyWithdrawn(item) }
        return nil
    }

    /// `isCaptain` 默认 `false` —— 默认值刻意选在**收紧**那一侧：新调用点漏传只会
    /// 让机长少一项权限（不方便），不会让谁多出一项权限（伤人）。
    func withdraw(crewId: String, number: Int, sessionId: String,
                  senderName: String? = nil, reason: String,
                  isCaptain: Bool = false) -> WithdrawOutcome {
        let trimmed = reason.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return .reasonRequired }
        return withFileLock(crewId) {
            var rows = loadLocked(crewId)
            guard !refuseUnsafeEmptyRewrite(crewId: crewId, rows: rows) else {
                return .ledgerUnavailable
            }
            guard let idx = liveIndexLocked(rows, number) else { return .notFound }
            if let obstacle = Self.withdrawObstacle(item: rows[idx], sessionId: sessionId,
                                                    isCaptain: isCaptain) {
                return obstacle
            }
            let stamp = timestamp()
            // 原因落成一条回应 —— 详细窗口的时间线本来就画回应，撤回的理由跟着
            // 条目走，不用人去翻群聊记录。
            rows[idx].responses.append(LocalTodoResponse(
                id: UUID().uuidString.lowercased(),
                sessionId: sessionId,
                senderName: senderName,
                text: "撤回：\(trimmed)",
                status: nil,
                createdAt: stamp))
            rows[idx].withdrawnAt = stamp
            rows[idx].withdrawnBySessionId = sessionId
            rows[idx].updatedAt = stamp
            if let failure = saveLocked(crewId: crewId, rows: rows) {
                return .notWritten(failure.localizedDescription)
            }
            return .withdrawn(rows[idx])
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
            guard (rows[idx].dismissedAt != nil) != dismissed else { return true }
            let stamp = timestamp()
            rows[idx].dismissedAt = dismissed ? stamp : nil
            rows[idx].updatedAt = stamp
            guard saveLocked(crewId: crewId, rows: rows) == nil else { return false }
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
            let stamp = timestamp()
            rows[idx].responses.append(LocalTodoResponse(
                id: UUID().uuidString.lowercased(),
                sessionId: "human",
                senderName: "人",
                text: text.isEmpty ? (wasCompleted ? "重开了这条 Todo" : "追问了这条 Todo") : text,
                // status 记这条追问把条目推到了哪 —— 只有重开那次真动了状态。
                status: wasCompleted ? "pending" : nil,
                createdAt: stamp,
                attachments: (attachments?.isEmpty ?? true) ? nil : attachments))
            if wasCompleted { rows[idx].status = "pending" }
            rows[idx].updatedAt = stamp
            guard saveLocked(crewId: crewId, rows: rows) == nil else { return nil }
            return rows[idx]
        }
    }

    /// 新写入统一保留小数秒，避免同一秒内回应/编辑后 `updatedAt` 看起来没推进。
    private func timestamp() -> String {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return formatter.string(from: now())
    }

    // MARK: - Persistence（基座三件套：flock / 逐条 lenient / corrupt 归档，#528）

    /// 这本账的列表文件指纹（mtime+size，**只 stat 不读内容**）。给侧栏黄点那条
    /// 指纹门控快照用（Todo #62 ④）—— 「有没有未回应条目」不能在 SwiftUI body 里
    /// 现读：那就是 2026-08-17「开久了卡」的同一个形状（flock + 整份 JSON 解码 ×
    /// 每个 crew × 每帧）。不上 flock：只读元数据，判定本身允许保守。
    func fingerprint(crewId: String) -> FileChangeGate.Fingerprint? {
        FileChangeGate.fingerprint(of: fileURL(crewId))
    }

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

    /// **返回 nil = 这些行真的落到磁盘上了。**
    ///
    /// 读那一侧早就是 fail-closed 的（`loadLocked` 报 `.unreadable`、
    /// `refuseUnsafeEmptyRewrite` 拒写）；**写这一侧以前是敞开的** —— `saveRowsLocked`
    /// 吞掉 IO 错误、这里返回 `Void`、于是 `respond` 照常返回改好的那一行、
    /// `respond_todo` 照常回一句「已回应 Todo #N」，而账本上什么都没多。
    private func saveLocked(crewId: String, rows: [LocalTodoItem]) -> Error? {
        let failure = MultiProcessJSONStore.saveRowsLocked(rows, to: fileURL(crewId))
        // 写失败就别发变更信号：那会让界面去重读一份没变的文件，
        // 并把「刷新过了」误当成「改动生效了」。
        if failure == nil { changes.send(crewId) }
        return failure
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
    /// 最近一次真实变更。#95 之前的旧 JSON 没这个字段，保持 nil 并由
    /// `effectiveUpdatedAt` 回落到已有时间线，不因升级而丢整条。
    var updatedAt: String? = nil
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
    /// 提这条问题时，agent 自己写下的「答复回来后接着做什么」（驾驶舱计划 #75）。
    /// 老文件没这字段 → nil。
    var resumeNote: String? = nil
    /// 这条是**半路上问的**吗（`ask` 提的恒为 true；`add_human_todo` 提的为 false）。
    /// 决定 `resumeNote` 为空时要不要明说出来 —— 详见 `TodoLandingFlow.wakeText`。
    ///
    /// ⚠️ **必须是 optional，不能写成 `Bool = false`。** Swift 合成的 `Decodable`
    /// **不使用属性默认值** —— 非可选字段缺键就是整条解码失败。写成 `Bool = false`
    /// 的话，这次改动之前落盘的每一条 Todo 都会当场解不开（逐条 lenient 解码会把它们
    /// 一条条丢掉，账**看起来是空的**）。既有的 `TodoLedgerIsolationTests` 当场抓到了。
    /// nil = 老数据，按 `isMidFlowAsk` 的安全默认（false）处理。
    var expectsResume: Bool? = nil
    /// 这条是**权限放行请求**吗（#75 ②）：非 nil = agent 要跑这个工具、在等人放行。
    /// hook 靠它去重（同一个工具已经挂着一条就别再提），app 侧靠它决定要不要写放行票。
    var permissionTool: String? = nil

    /// 「这条是不是半路上问的」的取值口径。老数据（nil）按 false —— 那条路只会
    /// **少说一句提示**，不会误报，是安全的一侧。
    var isMidFlowAsk: Bool { expectsResume ?? false }

    var isDeleted: Bool { deletedAt != nil }

    /// **还没被回应过** —— 黄点亮灭的判据（Todo #62）：`.human` 那本只要还有一条
    /// 没人回应就亮，全部有回应就灭。用「有没有回应」而不是「有没有条目」：
    /// 一本长期待办列表会让黄点永远亮着，等于没有。
    ///
    /// 已完成、已删墓碑都不算（即使坏数据/旧调用绕过了 `list`）；`dismissedAt`
    /// 非 nil = 人类看过、决定不办、直接按灭，同样不再算未回应。
    var isUnanswered: Bool {
        !isDeleted && status != "completed" && responses.isEmpty
            && dismissedAt == nil && withdrawnAt == nil
    }

    /// 提出者自己撤回了这条（Todo #102）。**不是删除**：条目留在列表里、原因写在
    /// 时间线上，只是不再算「等人回应」。
    ///
    /// ## 为什么必须有这扇门
    /// 一条人类 Todo 会死，最常见的原因不是人不想答，是**世界变了**（版本发出去了、
    /// 站上线了、那条线被别的决定取代了）。能判断「世界变了」的只有提这条的那一方 ——
    /// 人类判断不了（他不知道下游走到哪一版了），别人要读完全部条目才判断得出来。
    /// 在这扇门开出来之前，提出者**明知道**自己那条已经作废，也只能眼看着它挂在
    /// 人的账上继续亮灯。
    ///
    /// ## 为什么不复用 `deletedAt`
    /// `deletedAt` 是人类删的墓碑，`list` 一律不返回 —— 那条会**凭空消失**。撤回是
    /// 另一方做的动作，人有权知道发生了什么、也有权不同意（他可以追问，把它问回来）。
    /// 所以撤回**留在列表里**，配一条写着原因的回应，外加群里一行。
    var withdrawnAt: String? = nil

    /// 撤回它的那个 session（`createdBySessionId` 的对照面）。只用于展示/追责，
    /// 判定谁撤得动在 `LocalTodoStore.withdraw` 里。
    var withdrawnBySessionId: String? = nil

    /// 人类「看过了，不打算回应」的标记（Todo #62）。没有它，一条人类不打算处理的
    /// 条目会把黄点永久钉死。不动 `status`、不加回应 —— 只是不再算未回应。
    var dismissedAt: String? = nil

    /// 展示用更新时间：新数据优先看 `updatedAt`；旧数据从回应、dismiss/软删墓碑与
    /// 创建时间里取真正最晚的一刻。时间戳解析失败时仍给出一个可见的原始兜底。
    var effectiveUpdatedAt: String {
        let stamps = [createdAt, updatedAt, deletedAt, dismissedAt].compactMap { $0 }
            + responses.map(\.createdAt)
        if let latest = stamps.compactMap({ stamp in
            CrewTimestamp.parse(stamp).map { (stamp: stamp, date: $0) }
        }).max(by: { $0.date < $1.date }) {
            return latest.stamp
        }
        return updatedAt ?? responses.last?.createdAt ?? dismissedAt ?? deletedAt ?? createdAt
    }

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

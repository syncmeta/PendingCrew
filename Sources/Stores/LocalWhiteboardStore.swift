import Foundation
import Combine

/// BYOK 本地白板持久化（spec 2026-06-05-pendingcrew-local-first-crew-design §2
/// 本地 crew-comms）。
///
/// 每 crew 一个 JSON 文件：`<dir>/<crewId>.json` = `[LocalWhiteboardMessage]`。
/// 单 crew 消息量可控，整文件 append-rewrite 足够（白板不是高频 IO）。
///
/// **自包含**：只依赖 Foundation，**不引** edge 的 `CrewWhiteboardEntry`。
/// 这样能单独编进 PendingCrewTests bundle **和 `pendingcrew-mcp` helper target**
/// （chunk 4：claude 子进程的 MCP server / hook 读写同一份白板文件）。映射成
/// `CrewWhiteboardEntry` 的活由 `LocalBackend` 在 app 模块里做。
///
/// **非 `@MainActor`**：helper CLI（非 main-actor）要用。app 侧只从 `@MainActor`
/// 调（LocalBackend / CrewChatView），不跨 actor 传实例。`@unchecked Sendable`：
/// 实例状态全 `let`（不可变），共享可变资源是磁盘文件 —— app 与 helper 并发
/// read-modify-write 由 `<crewId>.lock` 上的 flock 互斥（#483；此前 last-write-wins
/// 真丢过历史）。更彻底的单写者模型（helper 投递意图文件、app 代理写）另记 tech-debt。
///
/// 不依赖 PendingBot/edge —— 是 PendingCrew 开源核心的一部分。
final class LocalWhiteboardStore: @unchecked Sendable {
    /// 本机 BYOK "人类成员" 的合成发言者 id —— 给 `CrewSenderResolver` 认
    /// "自己"（右对齐）。与 `LocalBackend.localSubjectId` 区分：那是责任主体，
    /// 这是发言者身份。
    static let localUserId = "local-byok-user"

    /// 读失败时 `list()` 返回的那条合成警示行的 id（见 `readFailureWarning`）。
    /// **只存在于内存**，磁盘上没有这一行 —— 所以任何「拿末条当游标」的地方都得先
    /// 认出它并跳过（#595：钉了它就是当场钉一个表里不存在的 id → 下次扫描全量重放）。
    static let readFailureRowId = "whiteboard-read-failure"

    static let shared = LocalWhiteboardStore()

    /// 默认白板目录（app 与 helper 共用同一路径）。
    static let defaultDirectory: URL = {
        let base = (try? FileManager.default.url(
            for: .applicationSupportDirectory, in: .userDomainMask,
            appropriateFor: nil, create: true))
            ?? FileManager.default.temporaryDirectory
        return base
            .appendingPathComponent("PendingCrew", isDirectory: true)
            .appendingPathComponent("whiteboards", isDirectory: true)
    }()

    private let directory: URL

    /// 进程内变更信号（Phase 5：去轮询）。本进程每次 append 后发一个 `crewId` ——
    /// `LocalBackend.whiteboardChanges` 订阅它、按 crewId 过滤，把中栏/右栏的
    /// 「3s 轮询」换成「append 即刷新」。app 侧人类发送走 app 进程内的 `.shared`
    /// 实例，立即推。
    ///
    /// **跨进程写**（helper `--mcp-serve` 子进程经 `post_to_crew` 写 agent 进展、
    /// `--mcp-hook` 注入）走另一个进程的 `LocalWhiteboardStore` 实例、不经此
    /// subject —— 由下面的 `directoryChanged` 文件监听补齐（FSEvents/DispatchSource
    /// 监 whiteboards 目录），所以跨进程 agent 进展也能即时推到 UI，无需轮询。
    ///
    /// `PassthroughSubject` 是引用类型，作 `let` 持有不破坏 `@unchecked Sendable`
    /// （实例状态仍全 `let`）；本仓低频单写、`.send` 在 MainActor app 路径 + helper
    /// 进程各自调，无跨线程竞态热点。
    let changes = PassthroughSubject<String, Never>()

    /// **跨进程**白板目录变更信号。`startWatching()` 起一个 DispatchSource 目录
    /// 监听，本进程**或子进程**对 `<dir>/*.json` 的写（atomic rename）会触发，
    /// 发一个 `Void` tick。`LocalBackend.whiteboardChanges` 收到即重拉本 crew ——
    /// 目录事件不带 crewId（rename 粒度到目录），但重拉单 crew 的 JSON 很廉价。
    let directoryChanged = PassthroughSubject<Void, Never>()

    /// 目录事件合流窗口（#443）。目录里 600+ 个 session 状态文件持续在写，逐个
    /// 事件直发 tick 会把订阅方按写盘频率反复拉起。250ms 内的事件并成一个。
    static let directoryCoalesceWindow: TimeInterval = 0.25

    /// 目录监听的可变状态 —— 锁保护，维持 `@unchecked Sendable`（实例其余皆 `let`）。
    private let watchLock = NSLock()
    private var watchSource: DispatchSourceFileSystemObject?
    private var watchFD: Int32 = -1

    /// 目录事件合流（判定见 `DirectoryEventCoalescer`；定时排在下面这条队列上，
    /// 全程不碰主线程）。
    private let coalescer = DirectoryEventCoalescer()
    private let watchQueue = DispatchQueue(
        label: "com.pendingname.pendingcrew.whiteboard-watch", qos: .utility)

    init(directory: URL? = nil) {
        self.directory = directory ?? Self.defaultDirectory
        try? FileManager.default.createDirectory(at: self.directory, withIntermediateDirectories: true)
    }

    // MARK: - Cross-process directory watch（去轮询：补齐 helper 子进程的写）

    /// 起目录监听（幂等）。监 whiteboards 目录的写/rename —— atomic write 落盘
    /// （写临时文件 → rename 覆盖）在目录上表现为写/rename 事件，per-file fd 会因
    /// inode 被换而失效，所以监**目录**而非单文件。事件 → `directoryChanged` tick。
    /// app UI（`LocalBackend`）首次订阅时调；短命的 helper stdio 进程不调（无需）。
    func startWatching() {
        watchLock.lock()
        defer { watchLock.unlock() }
        guard watchSource == nil else { return }
        let fd = open(directory.path, O_EVTONLY)
        guard fd >= 0 else { return }
        let source = DispatchSource.makeFileSystemObjectSource(
            fileDescriptor: fd,
            eventMask: [.write, .rename, .delete],
            queue: watchQueue)
        source.setEventHandler { [weak self] in
            // 合流（#443）：窗口内第一个事件排一次 flush，其余被吸收 —— 订阅方
            // 至多每 `directoryCoalesceWindow` 收到一个 tick。语义没变（仍是
            // 「目录里有东西动过」），只是不再按别人的写盘频率逐个转发。
            guard let self, self.coalescer.noteEvent() else { return }
            self.watchQueue.asyncAfter(deadline: .now() + Self.directoryCoalesceWindow) {
                [weak self] in
                guard let self, self.coalescer.flush() else { return }
                self.directoryChanged.send(())
            }
        }
        source.setCancelHandler { close(fd) }
        watchFD = fd
        watchSource = source
        source.resume()
    }

    // MARK: - Public

    /// 某 crew 白板文件的变更指纹（mtime+size）。文件不存在 → nil。
    ///
    /// 给订阅方在收到**不带文件名**的目录 tick 后做相关性判定用（#443）：别的
    /// session 写自己的 `.cursor`/`.turn`/approvals 时白板 json 指纹不动，就不该
    /// 让整条群聊重拉重排。不上 flock —— 只读元数据，且判定本身就允许保守
    /// （宁可多 yield 一次，也不该阻塞写者）。
    func fingerprint(crewId: String) -> FileChangeGate.Fingerprint? {
        FileChangeGate.fingerprint(of: fileURL(crewId))
    }

    /// 列出某 crew 的全部白板消息（按写入顺序）。文件缺失 → 空。
    func list(crewId: String) -> [LocalWhiteboardMessage] {
        withFileLock(crewId) { loadLocked(crewId) }
    }

    /// 该游标位置**之后**的消息（读未读用）。语义见下面的纯函数。
    func entries(crewId: String, after position: WhiteboardCursorPosition?)
        -> [LocalWhiteboardMessage] {
        Self.entries(in: list(crewId: crewId), after: position)
    }

    /// 上一行的纯函数半边（不碰磁盘，单测直接钉）。**fail-closed**（#595）：
    ///
    /// - `position == nil` → 全部。这是「真的没有游标」= 首次投递的合法语义。
    ///   谁有资格传 nil 是关键：只有游标文件**确实不存在**才算首次，「文件在但锚点
    ///   悬空」不算 —— 分家在 `WhiteboardCursor.read()` 的三态里做。
    /// - 锚点 id 在表里 → 它之后那批（原语义，分毫不动）。
    /// - 锚点 id **不在**表里 → **绝不返回全部**。这里正是 2026-08-12 全机重放的病根：
    ///   白板被归档重建换了一批新 id（或 lenient 解码丢了锚点那一行），旧实现的
    ///   「找不到 → 返回全部」当场把整部历史当成新增，逐条把 session 拉起来照几周前
    ///   的过期指令返工。有时间戳就按时间戳切出真正更新的那批；连时间戳都没有
    ///   （旧格式纯 id 游标）就一条都不给，交给 `WhiteboardCursor` resync 到当前尾。
    ///
    /// 时间戳解析不出来的行按「不是新的」处理 —— 同样是宁可少投，不可重放。
    static func entries(in all: [LocalWhiteboardMessage],
                        after position: WhiteboardCursorPosition?) -> [LocalWhiteboardMessage] {
        guard let position else { return all }
        if let idx = all.firstIndex(where: { $0.id == position.id }) {
            return Array(all.suffix(from: all.index(after: idx)))
        }
        guard let anchorAt = position.createdAt.flatMap(CrewTimestamp.parse) else { return [] }
        return all.filter { row in
            guard let at = CrewTimestamp.parse(row.createdAt) else { return false }
            return at > anchorAt
        }
    }

    /// 追加一条本机人类消息。`senderName` = 人类显示名（如已知；nil → 渲染退回「我/人类」）。
    /// `inReplyTo` = 被回复消息的白板 id（Phase 6 回复；nil = 非回复）。
    /// `attachments` = 本地落盘附件（Todo #3 群聊图片；nil/空 = 纯文本）。
    ///
    /// `mentions` = 人类 composer 的定向 @（Todo #62 ③）。**在此之前这个形参根本
    /// 不存在** —— `LocalBackend.postCrewMessage` 收了 mentions 却一个字不落盘，@
    /// 只喂给了旁边那条直投唤醒链。后果是人类 @ 谁都是全组可见（`isVisible` 看的是
    /// 消息上的 mentions，消息上压根没有），而 `broadcast`（composer 的「全体」）在
    /// 这条路上从来没落过盘 —— 全库 114 条真白板里 user 发的 mentions 全是空的。
    /// 于是 A 线刚做好的可见性 + 注入面消歧对人类发的消息**一概不生效**。这里补上，
    /// 两条路（人类 composer / MCP `post_to_crew`）才用同一套语义。
    func appendUserMessage(
        crewId: String, text: String, senderName: String? = nil, inReplyTo: String? = nil,
        attachments: [LocalWhiteboardAttachment]? = nil,
        mentions: [LocalWhiteboardMention]? = nil) {
        append(crewId: crewId, LocalWhiteboardMessage(
            id: UUID().uuidString.lowercased(),
            senderKind: "user",
            senderUserId: Self.localUserId,
            senderSessionId: nil,
            category: nil,
            text: text,
            createdAt: ISO8601DateFormatter().string(from: Date()),
            senderName: senderName,
            inReplyTo: inReplyTo,
            mentions: (mentions?.isEmpty == true) ? nil : mentions,
            attachments: (attachments?.isEmpty == true) ? nil : attachments))
    }

    /// 追加一条 session（编码 agent）消息（chunk 4：`post_to_crew`）。`senderName` =
    /// 该 session 的显示 label（如「机长」/「Claude Code · abc123」；nil → 渲染退回
    /// `session:<id>`，给 agent 看的白板就不再只剩裸 uuid）。
    ///
    /// `mentions` = `post_to_crew` 的定向 @ 列表（Phase 7；nil/空 = 广播）。
    /// `inReplyTo` = 被回复消息的白板 id（Phase 7 reply_to；nil = 非回复）。
    /// 两项都落进白板：`mentions` 喂本地唤醒（`CrewLocalMentionWaker`），
    /// `inReplyTo` 喂中栏的回复引用条（#377）。
    ///
    /// `senderKind` 默认 "session"；机长 run 的发言传 "captain" —— 让渲染端
    /// 按稳定的 captain 身份（captainBotId）取头像种子，而不是每次启动都变的
    /// run sessionId（否则机长在成员列表和群聊气泡长两张脸）。
    func appendSessionMessage(crewId: String, sessionId: String, text: String,
                              category: String? = nil, senderName: String? = nil,
                              mentions: [LocalWhiteboardMention]? = nil,
                              inReplyTo: String? = nil,
                              senderKind: String = "session",
                              externalContactFrom: String? = nil) {
        _ = try? appendSessionMessageReportingFailure(
            crewId: crewId, sessionId: sessionId, text: text, category: category,
            senderName: senderName, mentions: mentions, inReplyTo: inReplyTo,
            senderKind: senderKind, externalContactFrom: externalContactFrom)
    }

    /// 与 `appendSessionMessage` 相同，但把编码/落盘错误抛给调用者 —— 用于回执
    /// 必须如实的调用点（MCP 写工具、create_child_crew 的开场任务）。
    ///
    /// 返回 nil = 干干净净写进去了；返回非 nil = 写进去了，**但**追加前发现白板
    /// 出过事（原文件已归档、白板已从一条系统警示重建），这句话要原样报给人 ——
    /// 只说「已发送」就等于瞒报。
    @discardableResult
    func appendSessionMessageReportingFailure(
        crewId: String, sessionId: String, text: String,
        category: String? = nil, senderName: String? = nil,
        mentions: [LocalWhiteboardMention]? = nil,
        inReplyTo: String? = nil,
        senderKind: String = "session",
        externalContactFrom: String? = nil
    ) throws -> String? {
        try appendReportingFailure(crewId: crewId, LocalWhiteboardMessage(
            id: UUID().uuidString.lowercased(),
            senderKind: senderKind,
            senderUserId: nil,
            senderSessionId: sessionId,
            category: category,
            text: text,
            createdAt: ISO8601DateFormatter().string(from: Date()),
            senderName: senderName,
            inReplyTo: inReplyTo,
            mentions: (mentions?.isEmpty == true) ? nil : mentions,
            externalContactFrom: externalContactFrom))
    }

    // MARK: - Persistence

    private func fileURL(_ crewId: String) -> URL {
        // crewId 是受控本地 id（"local-"+uuid），无路径分隔符，直接当文件名安全。
        directory.appendingPathComponent("\(crewId).json")
    }

    /// 跨进程互斥（#483 ③ → 基座 ①）：flock sidecar 锁文件 `<crewId>.lock`。app 与
    /// 各 helper 子进程的 read-modify-write 都在锁内做，last-write-wins 竞态消除。
    /// 只在 public 入口拿一次，锁内一律走 `*Locked` 变体，不嵌套。
    private func withFileLock<T>(_ crewId: String, _ body: () throws -> T) rethrows -> T {
        try MultiProcessJSONStore.withFileLock(
            directory.appendingPathComponent("\(crewId).lock"), body)
    }

    /// 锁内读（基座 ②③）：逐条 lenient 解码，数组元素坏一条丢一条，不连坐；
    /// 外层 JSON 本身解析不了（半截写入 / 乱码）→ 归档 + 以「一条系统警示」重建
    /// 白板 —— 群里看得到出过事，且下一次 append 合并的是警示而不是空数组。
    private func loadLocked(_ crewId: String) -> [LocalWhiteboardMessage] {
        do {
            return try loadLockedReportingFailure(crewId).rows
        } catch {
            return [readFailureWarning(error)]
        }
    }

    /// 锁内严格读的结果。`incident` 非 nil = 这次读撞上事故且已就地处理（原文件已
    /// 归档、白板已从一条系统警示重建），文案给调用方原样报给人 —— 写路径的回执
    /// 不能只说「已发送」。
    private struct StrictLoad {
        var rows: [LocalWhiteboardMessage]
        var incident: String?
    }

    /// append 使用的严格读：IO / 权限 / 就地替换窗口中的读失败向上抛，禁止把它
    /// 伪装成空表；结构损坏仍走既有 quarantine + 持久化警示路径。
    private func loadLockedReportingFailure(_ crewId: String) throws -> StrictLoad {
        let url = fileURL(crewId)
        var rebuilt: [LocalWhiteboardMessage]?
        var incident: String?
        var quarantineError: Error?
        let rows = try MultiProcessJSONStore.loadRowsLockedReportingFailure(
            LocalWhiteboardMessage.self, at: url,
            onIncident: { incidentKind in
                // 这条严格读只可能报 `.corrupt`（读失败走 throw，不进回调）。
                guard case .corrupt(let maybeArchive) = incidentKind else { return }
                guard let archive = maybeArchive else {
                    quarantineError = WhiteboardPersistenceError.corruptQuarantineFailed(url)
                    return
                }
                rebuilt = self.rebuildWithWarningLocked(url: url, archive: archive, reason: .corrupt)
                incident = Self.incidentText(archive: archive, reason: .corrupt)
            })
        if let quarantineError { throw quarantineError }
        return StrictLoad(rows: rebuilt ?? rows, incident: incident)
    }

    /// 只在内存里呈现，不写磁盘。读失败可能是瞬态；**读**路径保留原文件并明确告诉
    /// UI / agent 当前不可读，不能用一条警示去覆盖仍可能完好的历史。
    ///（**写**路径同样一个字节都不动 —— 见 `appendReportingFailure`：拒写 + 抛错，
    /// 回执如实说没发出去。新消息不许悄悄丢（#577），历史更不许被处置掉（8-12 P0）。）
    private func readFailureWarning(_ error: Error) -> LocalWhiteboardMessage {
        LocalWhiteboardMessage(
            id: Self.readFailureRowId,
            senderKind: "session",
            senderUserId: nil,
            senderSessionId: "system",
            category: nil,
            text: "白板文件存在但暂时无法读取，原始记录未被改动。"
                + "错误：\(error.localizedDescription)",
            createdAt: ISO8601DateFormatter().string(from: Date()),
            senderName: "系统")
    }

    /// 白板出事、且**确认是内容坏了**的唯一形态。
    ///
    /// 2026-08-12 之前这里还有 `.unreadable` / `.emptyRead` 两种，收尾体例都是
    /// 「归档 + 警示行重建」—— 那正是把「我读不动」当成「它坏了」在处置，一次瞬时
    /// EMFILE 就扫掉全机白板。现在那两条各自走「拒写 + 保留原件 + 如实报错」，
    /// 不再有事故形态，枚举只剩真损坏这一种。
    private enum WhiteboardIncident {
        /// 两次独立读到的字节都解析不了（半截写入 / 乱码）。判定在
        /// `MultiProcessJSONStore.loadRowsLockedReportingFailure` 的复验里。
        case corrupt

        var cause: String {
            switch self {
            case .corrupt: return "白板文件损坏"
            }
        }
    }

    /// 白板已不可用的 fail-loud 收尾：损坏/读不出来的原字节已归档为
    /// `<crewId>.json.corrupt-<unix毫秒>`，这里再写入「一条系统警示」重建白板 ——
    /// 群里看得见这个 crew 的白板出过事，后续 append 合并的是警示而不是空数组。
    /// 警示消息复用 `CrewStore.postSystemNotice` 的形态（senderKind "session" +
    /// sessionId "system" + senderName "系统"），渲染端零改动。
    private func rebuildWithWarningLocked(url: URL, archive: URL,
                                          reason: WhiteboardIncident) -> [LocalWhiteboardMessage] {
        let warning = LocalWhiteboardMessage(
            id: UUID().uuidString.lowercased(),
            senderKind: "session",
            senderUserId: nil,
            senderSessionId: "system",
            category: nil,
            text: "\(reason.cause)，已归档为 \(archive.lastPathComponent)"
                + "（whiteboards 目录），本板从这条警示重新开始。",
            createdAt: ISO8601DateFormatter().string(from: Date()),
            senderName: "系统")
        MultiProcessJSONStore.saveRowsLocked([warning], to: url)
        return [warning]
    }

    /// 给**回执**用的事故说明（写进白板的警示行由 `rebuildWithWarningLocked` 负责）。
    private static func incidentText(archive: URL, reason: WhiteboardIncident) -> String {
        "\(reason.cause)，原文件已归档为 \(archive.lastPathComponent)（whiteboards 目录，"
            + "可人工找回），本板已从一条系统警示重新开始。"
    }

    private func append(crewId: String, _ msg: LocalWhiteboardMessage) {
        _ = try? appendReportingFailure(crewId: crewId, msg)
    }

    /// 锁内追加：重读-合并-写。load 与 write 之间没有别的写者能插进来（flock）。
    ///
    /// 追加前发现白板不可用时的统一规矩（#577 + 2026-08-12 P0）：
    /// - **确认内容损坏**（两次读都解不开）→ 归档 + 从警示行重建 + 本条消息照落，
    ///   回执带上事故说明；
    /// - **只是读不出来 / 漏读**（fd 打满、权限抖动、IO）→ **一个字节都不动**，
    ///   拒写并抛错，回执如实说没发出去。
    /// 两种结局都不是「悄悄丢掉、回一句已发送」，第二种也不再拿历史陪葬。
    ///
    /// 返回 nil = 一切正常；非 nil = 写成功但白板出过事，调用方要把这句话报给人。
    ///
    @discardableResult
    private func appendReportingFailure(
        crewId: String, _ msg: LocalWhiteboardMessage
    ) throws -> String? {
        try withFileLock(crewId) {
            let url = fileURL(crewId)
            var rows: [LocalWhiteboardMessage]
            var incident: String?
            do {
                let loaded = try loadLockedReportingFailure(crewId)
                rows = loaded.rows
                incident = loaded.incident
            } catch {
                // 读不出来 ≠ 内容损坏（2026-08-12 P0）。这里以前是「先归档搬走原件、
                // 再从警示行重建」—— 瞬时 EPERM/EMFILE 撞上它，一趟扫掉全机 19-24 份
                // 完好白板。现在一个字节都不动：拒写 + 如实回执，历史留在原地。
                throw WhiteboardPersistenceError.unreadableAndPreserved(url, error)
            }
            if incident == nil {
                // 漏读兜底同理：只拒写、不归档、不重建（`refuseEmptyRewrite…` 已改成
                // 非破坏性）。宁可这条消息发不出去并如实报错，也不拿空表覆盖历史。
                if MultiProcessJSONStore.refuseEmptyRewriteIfNonEmptyFile(rows, at: url) {
                    throw WhiteboardPersistenceError.unsafeEmptyRewrite(url)
                }
            }
            rows.append(msg)
            try MultiProcessJSONStore.saveRowsLockedReportingFailure(rows, to: url)
            // 单一写入漏斗 —— 所有 public append 变体都经此，发一个变更信号即覆盖全部
            // （人类发送 / session 进展）。订阅方按 crewId 过滤。
            changes.send(crewId)
            return incident
        }
    }
}

private enum WhiteboardPersistenceError: LocalizedError {
    case unsafeEmptyRewrite(URL)
    case corruptQuarantineFailed(URL)
    case unreadableAndPreserved(URL, Error)

    var errorDescription: String? {
        switch self {
        case .unsafeEmptyRewrite(let url):
            return "白板读取为空但磁盘文件非空，已拒绝覆盖：\(url.lastPathComponent)"
        case .corruptQuarantineFailed(let url):
            return "白板文件损坏且归档失败，原始记录已保留：\(url.lastPathComponent)"
        case .unreadableAndPreserved(let url, let cause):
            return "白板文件读不出来且归档失败（\(cause.localizedDescription)），"
                + "原始记录已原地保留、本次一个字都没写：\(url.lastPathComponent)"
        }
    }
}

/// 本地白板的持久化行。与 edge wire 解耦（edge 的 `CrewWhiteboardEntry` 只有
/// Decodable 且依赖 app 模块）。`LocalBackend` 把它映射成 `CrewWhiteboardEntry`
/// 给中栏渲染。新增可选字段向后兼容（旧 JSON 缺键 → nil）。
struct LocalWhiteboardMessage: Codable, Equatable {
    let id: String
    /// 'user' | 'session' | 'captain' | …
    let senderKind: String
    let senderUserId: String?
    /// session 作者的 id（senderKind == "session" 时）。
    let senderSessionId: String?
    /// post_to_crew 的 category（progress/question/milestone），可选。
    let category: String?
    let text: String
    /// ISO8601 字符串，与 edge `created_at` 同形，便于 UI 复用时间分隔逻辑。
    let createdAt: String
    /// 本地产生消息（session post_to_crew / 人类输入）的发送者显示名 ——
    /// 给 agent 看的白板（`HookEmitter.render`）用它替掉裸 `session:<uuid>` / 「人类」。
    /// 新增可选字段向后兼容（旧 JSON 缺键 → nil）。
    ///
    /// 这里曾经还有一个 `senderDisplayName`（relay 从 edge 搬进来的远端发送者名），
    /// 与本字段是两条来源。它随 #63 第二期删除跨端遥控整层一起去掉 —— 唯一的写入
    /// 方是 `appendRelayMessage`。注意**别跟线上模型 `CrewWhiteboardEntry`
    /// 的同名字段搞混**：那个是活的，`LocalBackend` 用本字段主动合成它。
    var senderName: String? = nil
    /// 被回复消息的白板 id（Phase 6 回复）。非 nil = 这条是对 `inReplyTo` 的回复。
    /// 新增可选字段向后兼容（旧 JSON 缺键 → nil）。
    var inReplyTo: String? = nil
    /// 定向 @ 列表（Phase 7：session `post_to_crew(mentions:)`）。非 nil/非空 = 这条
    /// 点名了某些对象（@session / @captain / @human）。按 mention 唤醒目标 session
    /// 由 `CrewLocalMentionWaker` / `CrewLocalMentionDelivery` 接。
    /// 新增可选字段向后兼容（旧 JSON 缺键 → nil）。
    var mentions: [LocalWhiteboardMention]? = nil
    /// 本地落盘附件（Todo #3 群聊图片）。非 nil/非空 = 这条带图/文件，`path` 是
    /// 本机绝对路径（app 数据目录 attachments/<crewId>/ 下，随聊天记录持久）。
    /// 新增可选字段向后兼容（旧 JSON 缺键 → nil）。
    var attachments: [LocalWhiteboardAttachment]? = nil
    /// **跨 crew 来电**（通讯录 `contact` 工具）的来源号码，如 `"1-1"`。非 nil =
    /// 这条不是本群成员的发言，是外面打进来的。两个用处：① 渲染时一眼看出是外线；
    /// ② 唤醒面据此把「外部来电的广播」当 @机长处理（`CrewLocalMentionWakeLogic`）——
    /// 本群成员之间的普通广播仍然不唤醒任何人，语义不变。
    /// 新增可选字段向后兼容（旧 JSON 缺键 → nil）。
    var externalContactFrom: String? = nil

    /// 每个附件一行「绝对路径提示」（claude 用 Read 即可看图）。空 = 无附件。
    var attachmentAgentHints: [String] {
        (attachments ?? []).map(\.agentHint)
    }

    /// 给 agent 看的渲染文本：正文 + 附件路径提示行。所有白板→agent 的渲染口
    /// （HookEmitter / read_whiteboard / 近期群聊上下文）统一走这里，晚醒的
    /// session 也能拿到图片路径。
    var agentText: String {
        LocalWhiteboardAttachment.appendingAgentHints(to: text, attachments)
    }
}

/// 一个本地附件（Todo #3）。`path` = 本机绝对路径（落盘见
/// `CrewChatAttachmentStore`）；`mime` 判定图/文件渲染分支；`filename` = 原始
/// 文件名（文件 chip 显示用，粘贴图片为 nil）。
struct LocalWhiteboardAttachment: Codable, Equatable {
    let id: String
    let mime: String
    let size: Int?
    let path: String
    var filename: String? = nil

    var isImage: Bool { mime.lowercased().hasPrefix("image/") }

    /// 给 agent 的路径提示行（PTY 注入 / 白板渲染共用同一文案）。
    var agentHint: String {
        isImage
            ? "用户发来图片：\(path)（请 Read 查看）"
            : "用户发来文件：\(path)（请 Read 查看）"
    }

    /// 正文 + 每个附件一行路径提示。**所有**「带附件的东西讲给 agent 听」都走这里
    /// —— 白板消息（`LocalWhiteboardMessage.agentText`）、人类 Todo 条目与追问
    /// （Todo #52）共用同一套措辞，session 在哪条通道上看到图都认得出「去 Read」。
    static func appendingAgentHints(
        to body: String, _ attachments: [LocalWhiteboardAttachment]?
    ) -> String {
        let hints = (attachments ?? []).map(\.agentHint)
        guard !hints.isEmpty else { return body }
        let tail = hints.joined(separator: "\n")
        return body.isEmpty ? tail : body + "\n" + tail
    }
}

/// 一条定向 @（Phase 7）。形状对齐线上模型 `CrewMention`（`{kind, target_id?}`），
/// 中栏渲染 / 唤醒判定两端因此不必各写一套。
struct LocalWhiteboardMention: Codable, Equatable {
    /// 'session' | 'captain' | 'human' | 'broadcast'
    /// —— 前两种收窄可见范围，`human` 是「讲给人听、别叫醒 agent」的附加标记，
    /// `broadcast` 是**显式放宽器**（#62，见 `CrewWhiteboardVisibility`）。
    let kind: String
    /// kind == "session" 时是目标 session 的 id；其余 kind 可空。
    let targetId: String?

    // 序列化成 snake_case，与 `CrewMention` 的 `{kind, target_id?}` 逐字节对齐
    // （默认 Codable 会是 camelCase）。
    enum CodingKeys: String, CodingKey {
        case kind
        case targetId = "target_id"
    }
}

/// 从 wire 层的 `CrewMention` 转过来（Todo #62 ③）。两个类型字段完全同形 ——
/// 差别只是一个走 edge/composer、一个落本地白板 JSON。人类 composer 的 @ 要落盘
/// 就得过这道桥；桥只有这一座，别在调用点手写
/// `LocalWhiteboardMention(kind: m.kind, targetId: m.targetId)`（漏一个字段就是
/// 静默降级）。写在 extension 里是为了**不吃掉逐字段的 memberwise init** ——
/// 那个 init 有一堆现存调用方。
extension LocalWhiteboardMention {
    init(_ mention: CrewMention) {
        self.init(kind: mention.kind, targetId: mention.targetId)
    }
}

/// 反方向的同一座桥（Todo #14 ①）：落盘的 mention → 纯逻辑层吃的 `CrewMention`。
/// `McpServer` 的 `post_to_crew(reply_to:)` 要把调用方给的 mentions 喂进
/// `CrewComposerMentionParser.mentionsToSend(staged:replyTo:)`，就得过这道桥。
/// 同样别在调用点手写 —— 两个字段，漏一个就是静默降级。
extension CrewMention {
    init(_ mention: LocalWhiteboardMention) {
        self.init(kind: mention.kind, targetId: mention.targetId)
    }
}

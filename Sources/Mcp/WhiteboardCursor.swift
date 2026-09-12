import Foundation

/// 白板游标的位置 —— **(id, createdAt) 复合**，不是裸 id（#595）。
///
/// 纯 id 指针太脆：白板文件一旦被归档重建（或 lenient 解码丢了那一行），锚点 id
/// 就永远查不到了，而「查不到」在旧实现里等于「全是新的」。带上写游标那一刻的
/// 时间戳，锚点悬空时还能退到时间比较，切出**真正更新**的那批。
///
/// `createdAt == nil` = 旧格式（纯 id）游标，只会从磁盘上的历史游标文件读出来；
/// 本仓写出去的游标一律带时间戳。
struct WhiteboardCursorPosition: Equatable {
    let id: String
    /// ISO8601（带不带小数秒都认，解析走 `CrewTimestamp.parse`）。nil = 旧格式游标。
    let createdAt: String?
}

/// 白板投递账本的**键** —— 跟着「对话」走，不跟着进程走（人类 Todo #105 ①）。
///
/// 病根：机长每次启动都新造 `captain-<uuid8>`（`CrewSessionRunner.startCaptain`），
/// 而**对话是 `--resume` 接回来的**。游标文件名带 sessionId ⇒ 新 id ⇒ 文件不存在
/// ⇒ 判「真首次」⇒ 把最近 `firstDeliveryLimit` 条重投给一个已经读过、已经回过它们
/// 的对话。**对话记得，游标不记得。** 本机实测：408 个机长游标 / 47 个 crew =
/// 361 次「非首任」，每次上限 30 条。
///
/// 归并的依据是代码里本来就成立的不变量：**每个 crew 同时只允许一个机长**
/// （`startCaptain` 那道 guard + `runs.removeAll { role == .captain }`），所以
/// 「本 crew 的机长」是一个无歧义的对话身份。worker 走 `restartMember` **复用原
/// sessionId**，本来就没有这个问题，因此**不归并** —— 归并了反而会让同 crew 的
/// 不同 worker 互相吃掉未读。
enum CrewConversationKey {
    /// 机长家族的前缀。`CrewSessionRunner` 造 id 的那一处是唯一产地。
    static let captainPrefix = "captain-"
    /// 归并后机长共用的键。
    static let captain = "captain"

    static func forSession(_ sessionId: String) -> String {
        sessionId.hasPrefix(captainPrefix) ? captain : sessionId
    }

    /// 这个键是归并出来的吗（= 需要去认领旧的按 sessionId 命名的游标）。
    static func isMerged(_ key: String) -> Bool { key == captain }
}

/// Per-session 白板**未读游标**的单一真值 —— PostToolUse hook（`HookEmitter`）与
/// **唤醒/提及注入**（`CrewLocalMentionWaker` / `CrewChatView` 本地直投）共用同一份。
///
/// 文件 `<directory>/<crewId>.<sessionId>.cursor` 存 last-delivered 位置，行格式
/// `<id>\t<createdAt>`（旧文件只有 `<id>`，见 `read()` 的迁移分支）。
/// 两条注入路都「读同一游标 → 取该 session 还没看过的 → 注入后推进同一游标」，
/// 于是一条消息对某 session **至多注入一次**，跨 hook 路与唤醒路不重复
/// （病根：唤醒路此前不接游标，每次唤醒重发最近 15 条已注入过的历史）。
///
/// **推进 forward-only + flock**：claude session 现在有两个**跨进程**写者 ——
/// helper 子进程的 PostToolUse hook 与 app 进程的唤醒注入，同写一份游标。
/// 用 `<crewId>.<sessionId>.cursor.lock` 上的 `flock` 串行化「读游标→比位置→落盘」，
/// 且仅当目标**位于当前游标之后**才写，杜绝并发下游标被推回旧位导致的重复注入。
/// 「更靠后」优先按白板列表里的下标判（同一张表最准），锚点悬空时退到时间戳。
///
/// ## fail-closed（#595，2026-08-12 全机重放事故的最后两环）
///
/// 六环因果链：fd 打满（GUI app 从 launchd 继承的软上限只有 256）→ `open()` 失败被
/// Foundation 包成误导性的 `NSFileReadNoPermissionError` → 误判成损坏 → 归档 + 重建
/// 空板 → **白板换了一批新 id** → **全机游标集体悬空** → **每次唤醒全量重放**。
/// 前四环归 P0「读失败不许销毁原件」；这里管最后两环，规矩只有一条：
/// **游标认不得，绝不等于「全是新的」。**
///
/// 落到实现上是三件事：
/// 1. `read()` 分三态。「游标文件不存在」= 真首次；「文件在但读不出来 / 锚点悬空」
///    绝不当首次 —— 这跟 2026-08-11 那次 P0（把「读不出来」和「本来就空」混成一态、
///    下一次写就把历史清了）是同一个错误形状，见
///    `LocalWhiteboardStore.loadLockedReportingFailure` 的同款分家。
/// 2. 锚点悬空 → `LocalWhiteboardStore.entries(in:after:)` 按时间戳切；连时间戳都
///    没有（旧格式游标）就一条都不给 + resync 到当前尾，绝不当首次全量重放。
/// 3. `advance` 在悬空态也要能推出去。旧实现「目标 id 查不到就不推进」会把游标
///    永久钉在悬空位 —— 那是这条 bug 的**第二个放大器**：同一批消息每次唤醒再来一遍。
struct WhiteboardCursor {
    let directory: URL
    let crewId: String
    let sessionId: String

    /// 真首次投递的条数上限。首次没有游标可依，语义上「在场历史全是未读」，但
    /// 白板动辄 200KB+，整部灌进 session 既撑爆上下文又毫无价值 —— 只给最近这批。
    static let firstDeliveryLimit = 30

    /// 游标的三态。**不许合并**——「没有游标」与「游标读不出来」是两回事。
    enum State: Equatable {
        /// 游标文件不存在 = 这个 session 从没被投递过 = 真首次。
        case absent
        /// 读到了一个锚点位置（`createdAt == nil` 表示是旧格式的纯 id 游标）。
        case anchored(WhiteboardCursorPosition)
        /// 文件在，但读不出来 / 内容是空的。**不是**首次 —— 按「已经投过、只是不知道
        /// 投到哪」处理：一条都不给，resync 到当前尾。
        case unreadable
    }

    /// 落盘用的**会话键**（#105 ①）：机长归并成一本，worker 仍按自己的 sessionId。
    private var conversationKey: String { CrewConversationKey.forSession(sessionId) }

    private var cursorURL: URL {
        directory.appendingPathComponent("\(crewId).\(conversationKey).cursor")
    }
    private var lockURL: URL {
        directory.appendingPathComponent("\(crewId).\(conversationKey).cursor.lock")
    }

    /// 改键当天，磁盘上全是旧格式 `<crewId>.captain-<uuid8>.cursor`。**不认领 =
    /// 全机每个 crew 再各重放一次**，等于这次修复自己先触发一遍它要修的 bug。
    ///
    /// 只在归并键上做，且只做一次：认领最近写过的那一份（同一 crew 同时只有一个
    /// 机长，"最近写过的"就是上一任），写到新位置之后这条路再也不会被走到。
    private func adoptLegacyCursorIfNeeded() {
        guard CrewConversationKey.isMerged(conversationKey),
              !FileManager.default.fileExists(atPath: cursorURL.path) else { return }
        let prefix = "\(crewId).\(CrewConversationKey.captainPrefix)"
        let files = (try? FileManager.default.contentsOfDirectory(
            at: directory, includingPropertiesForKeys: [.contentModificationDateKey]))?
            .filter { $0.lastPathComponent.hasPrefix(prefix)
                      && $0.pathExtension == "cursor" } ?? []
        let newest = files.max { a, b in
            let da = (try? a.resourceValues(forKeys: [.contentModificationDateKey]))?
                .contentModificationDate ?? .distantPast
            let db = (try? b.resourceValues(forKeys: [.contentModificationDateKey]))?
                .contentModificationDate ?? .distantPast
            return da < db
        }
        guard let newest, let raw = try? String(contentsOf: newest, encoding: .utf8) else { return }
        try? MultiProcessJSONStore.writeStaged(Data(raw.utf8), to: cursorURL)
    }

    /// 当前游标位置（三态，见 `State`）。
    func read() -> State {
        adoptLegacyCursorIfNeeded()
        guard FileManager.default.fileExists(atPath: cursorURL.path) else { return .absent }
        guard let raw = try? String(contentsOf: cursorURL, encoding: .utf8) else { return .unreadable }
        let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return .unreadable }
        let parts = trimmed.split(separator: "\t", maxSplits: 1, omittingEmptySubsequences: false)
        let id = String(parts[0]).trimmingCharacters(in: .whitespacesAndNewlines)
        guard !id.isEmpty else { return .unreadable }
        let stamp = parts.count > 1
            ? String(parts[1]).trimmingCharacters(in: .whitespacesAndNewlines) : ""
        return .anchored(WhiteboardCursorPosition(id: id, createdAt: stamp.isEmpty ? nil : stamp))
    }

    /// 一次投递能给出的最多条数 —— **有锚点的那条路也要有上限**（#105 ①）。
    ///
    /// 在 ① 之前，「新 sessionId ⇒ 游标 absent ⇒ 只投 30 条」这个行为**同时也在挡
    /// 另一件事**：机长隔几天醒来被灌几百条。① 把游标接回对话身份之后那道挡板就没了，
    /// 所以这里补回来。**但截掉的部分必须自报**（`Unread.omitted`）——
    /// 我们已经有两道方向相反的截断了，再加一道静默的，收的人会以为看到的就是全部。
    struct Unread: Equatable {
        let messages: [LocalWhiteboardMessage]
        /// 因为上限被截掉的条数。>0 时渲染层必须明说。
        let omitted: Int

        static let none = Unread(messages: [], omitted: 0)
    }

    /// 该 session 的**未读**（游标之后的白板消息，按写入序）。
    ///
    /// 三态各走各的（#595）：
    /// - `.absent`：真首次 —— 在场历史当未读，但只给最近 `firstDeliveryLimit` 条。
    /// - `.unreadable`：文件在、读不出来 —— **不当首次**。一条都不给，游标 resync 到
    ///   当前尾，免得它永远认不出、此后新消息也送不出去。
    /// - `.anchored`：锚点在表里就取它之后那批（原语义分毫不动）；悬空则由
    ///   `entries(in:after:)` 按时间戳 fail-closed 地切。悬空且是旧格式（无时间戳）
    ///   游标时同样 resync 到当前尾 —— **修复上线那一刻磁盘上全是旧格式游标，
    ///   把它们当首次就是再触发一次全机重放，比 bug 本身还难看。**
    ///
    /// 三态**都**过一遍 `capped`（#105 ①）：接回对话身份之后，锚点那条路也会
    /// 攒出几百条，上限不能只有首次那一份。
    func unread(in store: LocalWhiteboardStore) -> Unread {
        let all = store.list(crewId: crewId)
        // 白板整份读不出来时 `list` 回的是**一行内存警示**（磁盘上没有这条）。
        // 不认它的话有两条路都会坏：
        //   ① `.absent` 那支把它当成一条新消息投出去，调用方随后 `advance` 到它身上
        //      —— **游标就钉在一个磁盘上不存在的 id 上**（#595 那个病）；
        //   ② `.unreadable` 那支会 `resync(toTailOf:)` 到它，同样悬空。
        // 更坏的是那条警示的时间戳是**此刻**：悬空之后走时间戳兜底，比它旧的消息
        // 一律被当成「已投过」，于是**故障窗口里别人写进来的消息在恢复之后被静默跳过**。
        // 所以这一拍什么都不做：不投、不推、不 resync，留到白板读得动那一拍。
        //
        // **代价说清楚**：改之前那条内存警示会被当成一条新消息投给目标，目标因此
        // 「被动地」知道白板读不出来；现在它什么都收不到。**这是有意换的** ——
        // 投出去的那一份必须配一次游标推进，而推进就是把持久状态钉死在一个不存在的
        // id 上，那个错会活过恢复。**agent 仍然会知道**：`post_to_crew` / `plan_list`
        // 这些工具在同一时刻会当场拒绝并说清原因，那条路是主动的、也更准。
        guard LocalWhiteboardStore.readFailure(in: all) == nil else { return .none }
        switch read() {
        case .absent:
            return capped(Array(all))
        case .unreadable:
            resync(toTailOf: all)
            return .none
        case .anchored(let position):
            let fresh = LocalWhiteboardStore.entries(in: all, after: position)
            if let anchor = all.first(where: { $0.id == position.id }) {
                // 旧格式游标但锚点还在 → 就地升格成 (id, 时间戳)，下次万一悬空有得切。
                if position.createdAt == nil {
                    persist(WhiteboardCursorPosition(id: anchor.id, createdAt: anchor.createdAt))
                }
            } else if position.createdAt == nil {
                // 悬空 + 无时间戳：无从判新旧。fresh 已经是空（fail-closed），这里把
                // 游标接回当前尾，否则它永远认不出，之后的新消息也一并送不出去。
                resync(toTailOf: all)
            }
            return capped(fresh)
        }
    }

    private func capped(_ msgs: [LocalWhiteboardMessage]) -> Unread {
        guard msgs.count > Self.firstDeliveryLimit else {
            return Unread(messages: msgs, omitted: 0)
        }
        return Unread(messages: Array(msgs.suffix(Self.firstDeliveryLimit)),
                      omitted: msgs.count - Self.firstDeliveryLimit)
    }

    /// **这条对这个对话来说已经投过了吗** —— 四本账合一之后（#105 ④），这是唯一判据。
    ///
    /// 在此之前「已投递」有四本账：这一本（盘上、per 对话）、唤醒队列的
    /// `deliveredKeys`（内存、512 上限）、唤醒器的 per-crew 扫描游标（内存）、
    /// 以及 `listenCursors`（内存）。后三本互相看不见，一条消息可以同时在这一本里
    /// 「已投」、在另一本里「未投」——那正是重复投递的形状。
    ///
    /// **拿不准时一律答 false**：把没投过的判成已投 = 丢消息，比重复贵得多。
    func hasDelivered(_ entry: LocalWhiteboardMessage, in store: LocalWhiteboardStore) -> Bool {
        guard case .anchored(let position) = read() else { return false }
        let all = store.list(crewId: crewId)
        if let cur = all.firstIndex(where: { $0.id == position.id }),
           let tgt = all.firstIndex(where: { $0.id == entry.id }) {
            return tgt <= cur
        }
        // 锚点悬空 → 退到时间戳；两边都得有，缺一律答 false。
        guard let curAt = position.createdAt.flatMap(CrewTimestamp.parse),
              let tgtAt = CrewTimestamp.parse(entry.createdAt) else { return false }
        return tgtAt <= curAt
    }

    /// 推进游标到 `entry`（forward-only + flock）。
    ///
    /// 收**整条消息**而不是裸 id：目标的时间戳必须跟着一起落盘，游标才有能力在下一次
    /// 锚点悬空时按时间切。
    ///
    /// 谁算「更靠后」：
    /// - 当前锚点与目标都在白板表里 → 按下标（同一张表最准，同秒多条也分得清）。
    /// - 锚点悬空但两边都有时间戳 → 按时间戳；目标更旧就不写（并发写者不互相回退）。
    /// - 锚点悬空且比不出来 → **写**。旧实现在这里选择不动，游标就永久钉死在悬空位，
    ///   同一批消息每次唤醒再来一遍（#595 第二个放大器）。修好锚点比守住一个
    ///   认不出的旧位置重要。
    func advance(to entry: LocalWhiteboardMessage, in store: LocalWhiteboardStore) {
        // 绝不把游标推到那条**只存在于内存**的读失败警示上 —— 磁盘上没有这个 id，
        // 钉上去就是当场悬空。上面 `unread` 已经不会把它交出来了，这里是第二道：
        // 调用方不止一个，而这一步一旦写下去就是持久的。
        guard entry.id != LocalWhiteboardStore.readFailureRowId else { return }
        let target = WhiteboardCursorPosition(id: entry.id, createdAt: entry.createdAt)
        withCursorLock {
            let all = store.list(crewId: crewId)
            switch read() {
            case .absent, .unreadable:
                writeLocked(target)
            case .anchored(let current):
                let currentIdx = all.firstIndex { $0.id == current.id }
                let targetIdx = all.firstIndex { $0.id == entry.id }
                if let currentIdx, let targetIdx {
                    guard currentIdx < targetIdx else { return }   // 不回退（原语义）
                } else if let currentAt = current.createdAt.flatMap(CrewTimestamp.parse),
                          let targetAt = CrewTimestamp.parse(entry.createdAt) {
                    guard targetAt >= currentAt else { return }
                }
                writeLocked(target)
            }
        }
    }

    // MARK: - Persistence

    /// 游标接回当前尾（悬空修复）。白板本身是空的 → 删掉这个认不出的游标：没有任何
    /// 东西被投递过，回到「真首次」才是准确的，留着它反而让首条新消息被吃掉。
    private func resync(toTailOf all: [LocalWhiteboardMessage]) {
        guard let tail = all.last else {
            withCursorLock { try? FileManager.default.removeItem(at: cursorURL) }
            return
        }
        persist(WhiteboardCursorPosition(id: tail.id, createdAt: tail.createdAt))
    }

    private func persist(_ position: WhiteboardCursorPosition) {
        withCursorLock { writeLocked(position) }
    }

    private func writeLocked(_ position: WhiteboardCursorPosition) {
        var line = position.id
        if let createdAt = position.createdAt, !createdAt.isEmpty { line += "\t" + createdAt }
        try? MultiProcessJSONStore.writeStaged(Data(line.utf8), to: cursorURL)
    }

    private func withCursorLock(_ body: () -> Void) {
        try? FileManager.default.createDirectory(
            at: directory, withIntermediateDirectories: true)
        let fd = open(lockURL.path, O_CREAT | O_WRONLY, 0o644)
        if fd >= 0 { flock(fd, LOCK_EX) }
        defer { if fd >= 0 { flock(fd, LOCK_UN); close(fd) } }
        body()
    }
}

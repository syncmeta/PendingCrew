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

/// Per-session 白板**未读游标**的单一真值 —— PostToolUse hook（`HookEmitter`）与
/// **唤醒/提及注入**（`CrewMailboxWaker` 登录态、`CrewChatView` 本地直投）共用同一份。
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

    private var cursorURL: URL {
        directory.appendingPathComponent("\(crewId).\(sessionId).cursor")
    }
    private var lockURL: URL {
        directory.appendingPathComponent("\(crewId).\(sessionId).cursor.lock")
    }

    /// 当前游标位置（三态，见 `State`）。
    func read() -> State {
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
    func unread(in store: LocalWhiteboardStore) -> [LocalWhiteboardMessage] {
        let all = store.list(crewId: crewId)
        switch read() {
        case .absent:
            return Array(all.suffix(Self.firstDeliveryLimit))
        case .unreadable:
            resync(toTailOf: all)
            return []
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
            return fresh
        }
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
        try? line.write(to: cursorURL, atomically: true, encoding: .utf8)
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

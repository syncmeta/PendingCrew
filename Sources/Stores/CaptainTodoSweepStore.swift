import Foundation

/// 机长核账机制的一点点持久态（驾驶舱计划 #71）：**上一次被接受的确认**，
/// 以及**上一次真提醒过的时刻**。每 crew 一份：`<dir>/<crewId>.todo-sweep.json`。
///
/// **为什么必须落盘、不能只在内存里**：app 重启很频繁。放内存里的话，每次重启后
/// 机长第一次空闲都会被重新问一遍同一批已经交代过的条目 —— 那正是「一条永远在响的
/// 提醒」，它会把整个通道训练成背景噪音。
///
/// 沿用与白板 / 两本 Todo / 作战板同一个基座（`MultiProcessJSONStore`）：
/// flock 互斥 / 逐条 lenient 解码 / corrupt 归档 / 「读失败 ≠ 内容损坏」。
/// **跨进程锁是必须的**：写在 helper 子进程（`confirm_todo_sweep` 工具），
/// 读在 app（机长变空闲那一刻），与 Todo 那本同构。
///
/// 存的是**单行**（最新一次确认覆盖到哪、上次什么时候提醒的），不是流水账 ——
/// 判定只需要「最近一次」。
final class CaptainTodoSweepStore: @unchecked Sendable {
    static let shared = CaptainTodoSweepStore()

    struct Row: Codable, Equatable {
        /// 上一次被接受的确认。nil = 从没确认过。
        var confirmation: CaptainTodoSweep.Confirmation?
        /// 上一次**真的发出去**的提醒时刻（ISO8601）。没发过 = nil。
        var lastRemindedAt: String?
        /// 连续几次是因为「账读不出来」而提醒的（计划 #98 的退避档位）。nil = 0。
        var unreadableStreak: Int?
    }

    // MARK: - 机长变闲的那一拍（计划 #98 从 CrewSessionRunner 挪进来，好让它能被真跑）

    /// Todo 账那一眼 → 判定用的三态。**必须喂 `read` 的结果，不能喂 `list`**
    /// （后者把读失败压成空表，理由见 `LocalTodoStore.LedgerRead`）。
    static func snapshot(of read: LocalTodoStore.LedgerRead) -> CaptainTodoSweep.LedgerSnapshot {
        switch read {
        case let .rows(rows):
            // 「还欠着」= 既没做完、也没被叫停（`isSettled`）。写成「不等于
            // completed」的话，人类喊停的那几条会永远算作欠账，督办为它一直响。
            return .read(Set(rows.filter { !$0.isSettled }.map(\.number)))
        case .unreadable:
            return .unreadable
        }
    }

    /// 机长变闲时的一整拍：读存量 → 判定 → 记账。返回要发给机长的正文，nil = 这一拍不发。
    ///
    /// **记账在返回之前做**：调用方拿到正文就发（`run.send` 只是入队），
    /// 先记后发和先发后记在这里没有可观察的差别，而这样这一拍整个能在单测里跑。
    func idleTick(crewId: String, snapshot: CaptainTodoSweep.LedgerSnapshot, now: Date,
                  minimumInterval: TimeInterval = CaptainTodoSweep.minimumRemindInterval) -> String? {
        let stored = row(crewId: crewId)
        let streak = stored.unreadableStreak ?? 0
        let decision = CaptainTodoSweep.decide(
            open: snapshot,
            confirmation: stored.confirmation,
            lastRemindedAt: stored.lastRemindedAt.flatMap(McpServer.parseISO),
            unreadableStreak: streak,
            now: now,
            minimumInterval: minimumInterval)
        let next = CaptainTodoSweep.nextUnreadableStreak(
            after: decision, snapshot: snapshot, previous: streak)
        switch decision {
        case let .remind(text):
            recordReminded(crewId: crewId, at: now, unreadableStreak: next)
            return text
        case .silent:
            // 读得回来但这一拍不提醒 —— 档位照样清零（时刻不动，只动档位）。
            if next != streak, let stamp = stored.lastRemindedAt {
                remember(crewId: crewId, Reminded(stamp: stamp, unreadableStreak: next))
            }
            return nil
        }
    }

    /// 测试用：清掉进程内那份退路，模拟「进程重启了」。
    static func forgetInProcessMemoryForTesting() {
        rememberedReminded.removeAll()
    }

    private let directory: URL

    init(directory: URL? = nil) {
        self.directory = directory ?? LocalWhiteboardStore.defaultDirectory
        try? FileManager.default.createDirectory(at: self.directory, withIntermediateDirectories: true)
    }

    /// **盘上读不出来时，提醒时刻退到进程内的那份。**
    ///
    /// 病根（2026-09-12 实测到，而且是被它打了八个多小时才看出来的）：数据目录
    /// 整片读不出来的那种事故里，**这本账自己也读不出来** —— `loadLocked` 回 nil、
    /// 兜成 `Row()`，于是 `lastRemindedAt` 恒为 nil，**地板间隔的输入没了**。
    /// 而 `CaptainTodoSweep.decide` 里那句「一本一直读不出来的账不该在每次空闲抖动时
    /// 都刷屏」正是靠它成立的 —— 结果那道闸在**唯一需要它的场合**失效，
    /// 机长每次空闲都被问一遍同一句话。
    ///
    /// **典型的「守卫的输入跟被守的东西一起坏了」**：两者共用同一个基座、同一个目录。
    /// 所以退路不能也放在盘上，只能放进程内（`--mcp-serve` 一 session 一进程、长期
    /// 存活，这份内存活得够久）。写成功时两处一起更新，读不出来时用内存那份。
    /// ## 进程内那份还不够（2026-09-12 傍晚补）
    ///
    /// 上面那条退路只活在**本进程**里。app 在故障期间重启一次，它就空了，
    /// 地板间隔又没了输入 —— 而这类故障一次能持续几小时，期间重启很正常。
    ///
    /// 所以再加一层，用的是这类故障**恰好还放行**的那几个操作：
    /// **建新文件、列目录、删文件**（被掐的只有「open 一个已存在的 inode」，
    /// 逐系统调用量过，见 `docs/internal/2026-09-12-eperm-cause-found.md`）。
    /// 把时刻写进**文件名**，于是读它只需要 `readdir`，永远不用 `open`。
    ///
    /// ## 三层取**最新**的那一份，不按层排优先（计划 #98 改）
    ///
    /// 原来是「盘上 → 进程内 → 文件名」按层兜底。加上退避档位之后这不够了：
    /// 盘上那份可能是**旧的**（故障期间写不进去，或者读得回来时还停在上一窗），
    /// 按层取就会拿旧档位去压新情况。所以三份里取时刻最新的；时刻相同时取档位
    /// **小**的 —— 同一时刻只有「清零」会改档位，小的那份才是后写的，而且
    /// 这个方向错了也只是多问一次，不会压住。
    func row(crewId: String) -> Row {
        let disk = withFileLock(crewId) { loadLocked(crewId) }
        // 盘上读不到：确认无从得知（保守当成没有），但提醒时刻和档位还记得。
        var row = disk.row ?? Row()
        let onDisk = row.lastRemindedAt.map {
            Reminded(stamp: $0, unreadableStreak: row.unreadableStreak ?? 0)
        }
        let latest = Self.latest(
            [onDisk, Self.rememberedReminded[crewId], markerReminded(crewId: crewId)]
                .compactMap { $0 })
        row.lastRemindedAt = latest?.stamp
        row.unreadableStreak = latest.map(\.unreadableStreak)
        return row
    }

    /// 一次提醒留下的事实：什么时候、当时连续第几次是因为读不出来。
    private struct Reminded {
        let stamp: String
        let unreadableStreak: Int
    }

    private static func latest(_ all: [Reminded]) -> Reminded? {
        all.max { a, b in
            a.stamp != b.stamp ? a.stamp < b.stamp : a.unreadableStreak > b.unreadableStreak
        }
    }

    /// 标记文件放哪。**独立子目录** —— 免得这些零字节文件被当成账的一部分。
    private var markerDirectory: URL {
        directory.appendingPathComponent("sweep-reminded", isDirectory: true)
    }

    /// 从文件名里读回「上次提醒时刻 + 档位」。只列目录，不打开任何文件。
    /// 文件名：`<crewId>.<ISO8601>.u<档位>.marker`；没有 `.u<档位>` 的按 0 算。
    private func markerReminded(crewId: String) -> Reminded? {
        let names = (try? FileManager.default.contentsOfDirectory(atPath: markerDirectory.path))
            ?? []
        let prefix = crewId + "."
        return Self.latest(names
            .filter { $0.hasPrefix(prefix) && $0.hasSuffix(".marker") }
            .map { name -> Reminded in
                let body = String(name.dropFirst(prefix.count).dropLast(".marker".count))
                let parts = body.components(separatedBy: ".u")
                // ISO8601 定宽，字典序 = 时间序
                return Reminded(stamp: parts[0],
                                unreadableStreak: parts.count > 1 ? Int(parts[1]) ?? 0 : 0)
            })
    }

    private func clearMarkers(crewId: String) {
        for name in (try? FileManager.default.contentsOfDirectory(atPath: markerDirectory.path)) ?? []
        where name.hasPrefix(crewId + ".") && name.hasSuffix(".marker") {
            try? FileManager.default.removeItem(at: markerDirectory.appendingPathComponent(name))
        }
    }

    /// 落一个标记，并把这个 crew 的旧标记删掉（`unlink` 在故障里也是通的）。
    private func writeMarker(crewId: String, _ reminded: Reminded) {
        guard (try? FileManager.default.createDirectory(
            at: markerDirectory, withIntermediateDirectories: true)) != nil else { return }
        let safe = reminded.stamp.replacingOccurrences(of: "/", with: "-")
        let keep = "\(crewId).\(safe).u\(reminded.unreadableStreak).marker"
        try? Data().write(to: markerDirectory.appendingPathComponent(keep))
        for name in (try? FileManager.default.contentsOfDirectory(atPath: markerDirectory.path)) ?? []
        where name.hasPrefix(crewId + ".") && name.hasSuffix(".marker") && name != keep {
            try? FileManager.default.removeItem(at: markerDirectory.appendingPathComponent(name))
        }
    }

    /// 进程内的「上次提醒时刻」，只在盘上读不出来时顶上。
    /// `nonisolated(unsafe)` 与本 store 其余部分同一口径（靠 flock 与单进程串行）。
    nonisolated(unsafe) private static var rememberedReminded: [String: Reminded] = [:]

    /// 记下一次被接受的确认。**提醒时刻一并清空** —— 确认之后重新计时，
    /// 免得「确认完又冒出新条目」时被上一次的地板间隔压着不吭声。
    /// **返回 nil = 真的落到磁盘上了。**没落盘却回一句「记下了」，机长下次空闲
    /// 会被重新问一遍同一批条目 —— 而它以为自己已经交代过了。
    @discardableResult
    func recordConfirmation(crewId: String,
                            _ confirmation: CaptainTodoSweep.Confirmation) -> Error? {
        // 确认之后重新计时：进程内那份和文件名标记一起清，否则「确认完又冒出新条目」
        // 会被上一次的地板间隔压着不吭声 —— 那正是这道闸最不该发生的事。
        Self.rememberedReminded[crewId] = nil
        clearMarkers(crewId: crewId)
        return withFileLock(crewId) {
            saveLocked(crewId: crewId, Row(confirmation: confirmation, lastRemindedAt: nil))
        }
    }

    /// 记下「刚提醒过」。**不碰确认** —— 提醒不会让已有的确认作废。
    @discardableResult
    func recordReminded(crewId: String, at date: Date, unreadableStreak: Int = 0) -> Error? {
        remember(crewId: crewId, Reminded(stamp: ISO8601DateFormatter().string(from: date),
                                          unreadableStreak: unreadableStreak))
    }

    @discardableResult
    private func remember(crewId: String, _ reminded: Reminded) -> Error? {
        // 先记进程内那份：盘上写不写得成都不影响「刚提醒过」这个事实，
        // 而地板间隔要的正是这个事实。
        Self.rememberedReminded[crewId] = reminded
        // 再落一个文件名标记：进程重启后还认得（见 `row(crewId:)` 上的注释）。
        writeMarker(crewId: crewId, reminded)
        return withFileLock(crewId) {
            let disk = loadLocked(crewId)
            // **读不出来就别整份重写**（计划 #98 顺手发现）。原来是 `loadLocked ?? Row()`
            // 再写回：写走的是 rename，故障期间照样落得了盘 —— 于是每次提醒都用一个
            // 空 `Row()` 盖掉那份读不出来的账，**机长上一次交的确认就这么没了**，故障一过
            // 又被问一遍同一批条目。时刻和档位此刻已经在进程内和文件名里了，不差这一份。
            guard !disk.unreadable else {
                return CocoaError(.fileReadNoPermission)
            }
            var row = disk.row ?? Row()
            row.lastRemindedAt = reminded.stamp
            row.unreadableStreak = reminded.unreadableStreak == 0 ? nil : reminded.unreadableStreak
            return saveLocked(crewId: crewId, row)
        }
    }

    // MARK: - 持久化

    private func fileURL(_ crewId: String) -> URL {
        directory.appendingPathComponent("\(crewId).todo-sweep.json")
    }

    private func withFileLock<T>(_ crewId: String, _ body: () -> T) -> T {
        MultiProcessJSONStore.withFileLock(
            directory.appendingPathComponent("\(crewId).todo-sweep.lock"), body)
    }

    /// 读不出来 → nil。**调用方一律按「没确认过」处理，也就是照常提醒** ——
    /// 这个方向的失败是安全的：宁可多问一次，不可因为一次读失败就把机制静默关掉。
    /// `unreadable` = 这次读出过事故（打不开 / 读到空但文件非空 / 解不开）。
    /// 跟「文件不存在」分开 —— 后者 `row` 也是 nil，但可以放心新写一份。
    private func loadLocked(_ crewId: String) -> (row: Row?, unreadable: Bool) {
        var incident = false
        let row = MultiProcessJSONStore.loadRowsLocked(
            Row.self, at: fileURL(crewId), onIncident: { _ in incident = true }).first
        return (row, incident)
    }

    private func saveLocked(crewId: String, _ row: Row) -> Error? {
        MultiProcessJSONStore.saveRowsLocked([row], to: fileURL(crewId))
    }
}

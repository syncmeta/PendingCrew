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
    /// 三层优先级：盘上那本账 → 进程内 → 文件名标记。越往后越粗，但都比「没有」强。
    func row(crewId: String) -> Row {
        guard var row = withFileLock(crewId, { loadLocked(crewId) }) else {
            // 盘上读不到：确认无从得知（保守当成没有），但提醒时刻还记得。
            return Row(confirmation: nil,
                       lastRemindedAt: Self.rememberedReminded[crewId]
                           ?? markerReminded(crewId: crewId))
        }
        if row.lastRemindedAt == nil {
            row.lastRemindedAt = Self.rememberedReminded[crewId] ?? markerReminded(crewId: crewId)
        }
        return row
    }

    /// 标记文件放哪。**独立子目录** —— 免得这些零字节文件被当成账的一部分。
    private var markerDirectory: URL {
        directory.appendingPathComponent("sweep-reminded", isDirectory: true)
    }

    /// 从文件名里读回「上次提醒时刻」。只列目录，不打开任何文件。
    private func markerReminded(crewId: String) -> String? {
        let names = (try? FileManager.default.contentsOfDirectory(atPath: markerDirectory.path))
            ?? []
        let prefix = crewId + "."
        return names
            .filter { $0.hasPrefix(prefix) && $0.hasSuffix(".marker") }
            .map { String($0.dropFirst(prefix.count).dropLast(".marker".count)) }
            .max()   // ISO8601 定宽，字典序 = 时间序
    }

    private func clearMarkers(crewId: String) {
        for name in (try? FileManager.default.contentsOfDirectory(atPath: markerDirectory.path)) ?? []
        where name.hasPrefix(crewId + ".") && name.hasSuffix(".marker") {
            try? FileManager.default.removeItem(at: markerDirectory.appendingPathComponent(name))
        }
    }

    /// 落一个标记，并把这个 crew 的旧标记删掉（`unlink` 在故障里也是通的）。
    private func writeMarker(crewId: String, stamp: String) {
        guard (try? FileManager.default.createDirectory(
            at: markerDirectory, withIntermediateDirectories: true)) != nil else { return }
        let safe = stamp.replacingOccurrences(of: "/", with: "-")
        let keep = "\(crewId).\(safe).marker"
        try? Data().write(to: markerDirectory.appendingPathComponent(keep))
        for name in (try? FileManager.default.contentsOfDirectory(atPath: markerDirectory.path)) ?? []
        where name.hasPrefix(crewId + ".") && name.hasSuffix(".marker") && name != keep {
            try? FileManager.default.removeItem(at: markerDirectory.appendingPathComponent(name))
        }
    }

    /// 进程内的「上次提醒时刻」，只在盘上读不出来时顶上。
    /// `nonisolated(unsafe)` 与本 store 其余部分同一口径（靠 flock 与单进程串行）。
    nonisolated(unsafe) private static var rememberedReminded: [String: String] = [:]

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
    func recordReminded(crewId: String, at date: Date) -> Error? {
        let stamp = ISO8601DateFormatter().string(from: date)
        // 先记进程内那份：盘上写不写得成都不影响「刚提醒过」这个事实，
        // 而地板间隔要的正是这个事实。
        Self.rememberedReminded[crewId] = stamp
        // 再落一个文件名标记：进程重启后还认得（见 `row(crewId:)` 上的注释）。
        writeMarker(crewId: crewId, stamp: stamp)
        return withFileLock(crewId) {
            var row = loadLocked(crewId) ?? Row()
            row.lastRemindedAt = stamp
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
    private func loadLocked(_ crewId: String) -> Row? {
        MultiProcessJSONStore.loadRowsLocked(
            Row.self, at: fileURL(crewId), onIncident: { _ in }).first
    }

    private func saveLocked(crewId: String, _ row: Row) -> Error? {
        MultiProcessJSONStore.saveRowsLocked([row], to: fileURL(crewId))
    }
}

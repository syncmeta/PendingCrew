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

    func row(crewId: String) -> Row {
        withFileLock(crewId) { loadLocked(crewId) } ?? Row()
    }

    /// 记下一次被接受的确认。**提醒时刻一并清空** —— 确认之后重新计时，
    /// 免得「确认完又冒出新条目」时被上一次的地板间隔压着不吭声。
    func recordConfirmation(crewId: String, _ confirmation: CaptainTodoSweep.Confirmation) {
        withFileLock(crewId) {
            saveLocked(crewId: crewId, Row(confirmation: confirmation, lastRemindedAt: nil))
        }
    }

    /// 记下「刚提醒过」。**不碰确认** —— 提醒不会让已有的确认作废。
    func recordReminded(crewId: String, at date: Date) {
        withFileLock(crewId) {
            var row = loadLocked(crewId) ?? Row()
            row.lastRemindedAt = ISO8601DateFormatter().string(from: date)
            saveLocked(crewId: crewId, row)
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

    private func saveLocked(crewId: String, _ row: Row) {
        MultiProcessJSONStore.saveRowsLocked([row], to: fileURL(crewId))
    }
}
